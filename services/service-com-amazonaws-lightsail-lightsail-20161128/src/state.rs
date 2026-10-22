// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lightsail-lightsail-20161128 service.
//!
//! The slice implemented so far is Lightsail's primary resource, the **instance**
//! (a virtual private server), the **block storage disk** (additional storage an
//! instance can attach), the **disk snapshot** (a point-in-time copy of a disk
//! or of an instance's system volume), the **domain** (a DNS zone the account holds
//! recordsets in), the **container service** (a compute resource containers are
//! deployed to), the **bucket** (an object-storage resource of the Lightsail
//! object storage service) together with the **access keys** it grants
//! programmatic access through, the **managed relational database** (a MySQL or
//! PostgreSQL database the service runs for the account), the append-only
//! **operation log** every mutating call
//! writes, and the service's static **Region catalog** — the Regions and
//! Availability Zones `GetRegions` lists and every other operation validates a
//! location against. Everything mutable in this module is reached through
//! [`with_state`], so no handler can observe a store whose clock-owed transitions
//! have not been applied yet.
//!
//! Two kinds of value live on a stored instance:
//!
//! * what the caller asked for — name, availability zone, blueprint, bundle,
//!   key pair, tags, add-ons, IP address type — kept verbatim; and
//! * what the service assigns — ARN, support code, creation time, addresses,
//!   hardware, firewall rules, user name and lifecycle state — each computed
//!   from the request plus this store's own allocators, never from a canned body.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessKey, AccessKeyLastUsed, AccessRules, AccountLevelBpaSync, AddOn, AddOnRequest,
    AvailabilityZone, Blueprint, Bucket, BucketAccessLogConfig, BucketBundle, BucketCorsConfig,
    BucketCorsRule, BucketState, Bundle, Container,
    ContainerImage,
    ContainerService, ContainerServiceDeployment, ContainerServiceDeploymentRequest,
    ContainerServiceECRImagePullerRole, ContainerServiceEndpoint,
    ContainerServiceHealthCheckConfig, ContainerServiceStateDetail, Domain, DomainEntry, Instance,
    InstanceHardware, InstanceMetadataOptions, InstanceNetworking, InstancePortInfo, InstanceState,
    MonthlyTransfer, Operation, PrivateRegistryAccess, Region, RelationalDatabaseBundle,
    ResourceLocation, ResourceReceivingAccess, Tag, REGION_NAME_VALUES,
};

// ---------------------------------------------------------------------------
// Service identity
// ---------------------------------------------------------------------------

/// This service's ARN namespace (also its sigv4 signing name and endpoint
/// prefix), from `smithy_query(query="service_info")`.
pub const ARN_SERVICE: &str = "lightsail";

/// The resource portion an instance ARN carries before its id, from the modelled
/// `Instance$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:Instance/244ad76f-…`).
pub const INSTANCE_ARN_RESOURCE_PREFIX: &str = "Instance/";

/// The `resourceType` an instance and its operations report, from the modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_INSTANCE: &str = "Instance";

/// The `resourceType` a container service reports, from the same modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_CONTAINER_SERVICE: &str = "ContainerService";

/// The resource portion a container service ARN carries before its id. Mirrors
/// the `Instance/<uuid>` form the modelled `Instance$arn` documentation shows,
/// with this resource's own `ResourceType` value.
pub const CONTAINER_SERVICE_ARN_RESOURCE_PREFIX: &str = "ContainerService/";

/// The region a request that carries no SigV4 credential scope is served in.
///
/// Lightsail's own default endpoint region, and one of the modelled `RegionName`
/// enum's values, so a resource created by an unsigned call still reports a
/// region the model admits.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `resourceType` a bucket and its operations report, from the same modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_BUCKET: &str = "Bucket";

/// The `resourceType` a managed relational database and its operations report,
/// from the same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_RELATIONAL_DATABASE: &str = "RelationalDatabase";

/// The resource portion a managed relational database's ARN carries before its
/// id. Mirrors the `Instance/<uuid>` form the modelled `Instance$arn`
/// documentation shows, with this resource's own `ResourceType` value.
pub const RELATIONAL_DATABASE_ARN_RESOURCE_PREFIX: &str = "RelationalDatabase/";

/// The resource portion a bucket ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const BUCKET_ARN_RESOURCE_PREFIX: &str = "Bucket/";

/// The resource portion a domain ARN carries before its id, from the modelled
/// `Domain$arn` documentation
/// (`arn:aws:lightsail:global:123456789101:Domain/824cede0-…`).
pub const DOMAIN_ARN_RESOURCE_PREFIX: &str = "Domain/";

/// The `resourceType` a domain and its operations report, from the modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_DOMAIN: &str = "Domain";

/// The ARN region token of a domain recordset: a DNS zone is not a regional
/// resource, which is why the modelled `Domain$arn` documentation shows `global`
/// where an instance ARN carries a region.
pub const DOMAIN_ARN_REGION: &str = "global";

/// The region a domain recordset reports in its `ResourceLocation`.
///
/// `ResourceLocation$regionName` targets the modelled `RegionName` enum, which
/// has no `global` member, and the modelled `InvalidInputException`
/// documentation says "Domain and distribution APIs are only available in the
/// N. Virginia (us-east-1) Amazon Web Services Region" — so that is the region a
/// recordset of this account lives in.
pub const DOMAIN_REGION_NAME: &str = "us-east-1";

/// The resource portion a disk ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const DISK_ARN_RESOURCE_PREFIX: &str = "Disk/";

/// The `resourceType` a block storage disk and its operations report, from the
/// modelled `ResourceType` enum — the value the modelled `Disk$resourceType`
/// documentation names ("The Lightsail resource type (`Disk`)").
pub const RESOURCE_TYPE_DISK: &str = "Disk";

/// The resource portion a disk snapshot ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const DISK_SNAPSHOT_ARN_RESOURCE_PREFIX: &str = "DiskSnapshot/";

/// The `resourceType` a disk snapshot and its operations report, from the
/// modelled `ResourceType` enum — the value the modelled
/// `DiskSnapshot$resourceType` documentation names ("The Lightsail resource type
/// (`DiskSnapshot`)").
pub const RESOURCE_TYPE_DISK_SNAPSHOT: &str = "DiskSnapshot";

/// Applied when `CreateInstances` omits `ipAddressType`.
///
/// From that member's own documentation: "The default value is `dualstack`."
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "dualstack";

/// The key pair an instance reports when the request named none: the Lightsail
/// default key pair, which the modelled `Instance$sshKeyName` documentation
/// names as its example (`LightsailDefaultKeyPair`).
pub const DEFAULT_SSH_KEY_NAME: &str = "LightsailDefaultKeyPair";

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// One `InstanceState`: the numeric code and the name Lightsail reports together.
///
/// A Lightsail instance is an EC2 instance, and `code` carries EC2's
/// `InstanceState.code` values verbatim — 0 `pending`, 16 `running`, 64
/// `stopping`, 80 `stopped` — which is also the pairing the modelled
/// `InstanceState` documentation shows (`code` 16 beside the name `running`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifecycle {
    pub code: i32,
    pub name: &'static str,
}

/// An instance that has been created or started but is not serving yet.
pub const PENDING: Lifecycle = Lifecycle {
    code: 0,
    name: "pending",
};
/// A serving instance.
pub const RUNNING: Lifecycle = Lifecycle {
    code: 16,
    name: "running",
};
/// An instance on its way down.
pub const STOPPING: Lifecycle = Lifecycle {
    code: 64,
    name: "stopping",
};
/// A stopped instance.
pub const STOPPED: Lifecycle = Lifecycle {
    code: 80,
    name: "stopped",
};

/// How long after the call that starts it the clock owes a resource its next
/// settled state: an instance's (`pending` -> `running`, `stopping` ->
/// `stopped`), a block storage disk's (`pending` -> `available`) and a container
/// service's (`PENDING` -> `READY`, and each step of `PENDING` -> `DEPLOYING` ->
/// `RUNNING`).
///
/// CHOSEN, not an AWS duration: AWS takes tens of seconds to minutes, which no
/// caller can be made to wait for here. The span is deliberately NON-ZERO, so a
/// caller that reads straight after `CreateInstances`, `StartInstance`,
/// `StopInstance`, `CreateDisk` or `CreateContainerService` still observes the
/// transient state exactly as it does against AWS.
pub const STATE_TRANSITION_MILLIS: i64 = 500;

/// A state change the clock already owes an instance.
#[derive(Debug, Clone, Copy)]
pub struct DueTransition {
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    /// The state the instance settles into.
    pub state: Lifecycle,
}

impl DueTransition {
    /// The transition `state`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, state: Lifecycle) -> Self {
        Self {
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            state,
        }
    }
}

/// `DiskState` values this slice uses, from the modelled `DiskState` enum and the
/// `Disk$state` documentation: a disk Lightsail is still provisioning is
/// `pending`, one that exists and is attached to nothing is `available`, and one
/// an instance is using is `in-use`.
pub const DISK_STATE_PENDING: &str = "pending";
/// See [`DISK_STATE_PENDING`].
pub const DISK_STATE_AVAILABLE: &str = "available";
/// See [`DISK_STATE_PENDING`].
pub const DISK_STATE_IN_USE: &str = "in-use";

/// `DiskSnapshotState` values this slice uses, from the modelled
/// `DiskSnapshotState` enum and the `DiskSnapshot$state` documentation ("The
/// status of the disk snapshot operation"): a snapshot Lightsail is still taking
/// is `pending` and one whose data is all captured is `completed`. The enum's
/// `error` and `unknown` members describe a snapshot that failed or whose status
/// Lightsail cannot report, neither of which this store can produce — nothing
/// here fails a copy.
pub const DISK_SNAPSHOT_STATE_PENDING: &str = "pending";
/// See [`DISK_SNAPSHOT_STATE_PENDING`].
pub const DISK_SNAPSHOT_STATE_COMPLETED: &str = "completed";

/// A state the clock already owes a resource whose state is a plain NAME: a
/// block storage disk's `DiskState` value, a disk snapshot's
/// `DiskSnapshotState` value, or a managed relational database's `state`.
///
/// Separate from [`DueTransition`] because those states are names with no
/// numeric code, where an instance's is an EC2 `InstanceState` (a code and a
/// name together).
#[derive(Debug, Clone, Copy)]
pub struct DueNamedState {
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    /// The state the resource settles into.
    pub state: &'static str,
}

impl DueNamedState {
    /// The transition to `state`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, state: &'static str) -> Self {
        Self {
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            state,
        }
    }
}

/// `RelationalDatabase$state` values this slice uses — "Describes the current
/// state of the database", a plain string in the model with no enum behind it.
///
/// A Lightsail managed database is an Amazon RDS DB instance, and the member
/// carries RDS's own DB instance status values: `creating` while the instance is
/// being provisioned and `available` once it accepts connections (Amazon RDS
/// User Guide, "Viewing DB instance status").
pub const RELATIONAL_DATABASE_STATE_CREATING: &str = "creating";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_AVAILABLE: &str = "available";

// ---------------------------------------------------------------------------
// Operation log
// ---------------------------------------------------------------------------

/// `Operation$status` values this slice uses, from the modelled `OperationStatus`
/// enum: a mutating call is answered `Started` and settles into `Succeeded` when
/// the work it began is done.
pub const OPERATION_STATUS_STARTED: &str = "Started";
/// See [`OPERATION_STATUS_STARTED`].
pub const OPERATION_STATUS_SUCCEEDED: &str = "Succeeded";

/// `Operation$operationType` values this slice writes, from the modelled
/// `OperationType` enum.
pub const OPERATION_TYPE_CREATE_INSTANCE: &str = "CreateInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_START_INSTANCE: &str = "StartInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_STOP_INSTANCE: &str = "StopInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_INSTANCE: &str = "DeleteInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DOMAIN: &str = "CreateDomain";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET: &str = "CreateBucket";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK: &str = "CreateDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE: &str = "CreateRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_SNAPSHOT: &str = "CreateDiskSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_FROM_SNAPSHOT: &str = "CreateDiskFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET_ACCESS_KEY: &str = "CreateBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_BUCKET_ACCESS_KEY: &str = "DeleteBucketAccessKey";
/// The `operationType` the log entry of a change to a zone's recordsets carries.
///
/// The modelled `OperationType` enum has `UpdateDomainEntry` and
/// `DeleteDomainEntry` but NO `CreateDomainEntry` member, and
/// `Operation$operationType` targets that enum — so adding an entry to a zone is
/// logged as the modelled recordset update, not as an unmodelled value.
pub const OPERATION_TYPE_UPDATE_DOMAIN_ENTRY: &str = "UpdateDomainEntry";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller, so each growth site carries a cap
// and nothing caller-sized reaches the store unchecked.
// ---------------------------------------------------------------------------

/// The most instances this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota (Lightsail's own default quota is far
/// tighter, and deliberately not enforced here because `GetInstances`' pagination
/// needs more instances than that to exercise). A create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_INSTANCES: usize = 500;

/// The most names one `CreateInstances` call may carry.
///
/// Emulator resource guard: the modelled `StringList` declares no length, so this
/// is where the bound on a caller-sized list lives.
pub const MAX_INSTANCE_NAMES_PER_REQUEST: usize = 50;

/// The most domain recordsets this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DOMAINS: usize = 500;

/// The most block storage disks this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DISKS: usize = 500;

/// The most managed relational databases this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_RELATIONAL_DATABASES: usize = 500;

/// The most disk snapshots this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DISK_SNAPSHOTS: usize = 500;

/// The most recordset entries this emulator will hold in one domain.
///
/// Emulator resource guard, NOT an AWS quota: an entry is caller-sized and the
/// zone's entry list grows with every create, so a create that would exceed this
/// is refused with the modelled `InvalidInputException`.
pub const MAX_DOMAIN_ENTRIES: usize = 1_000;

/// The most tags one instance may carry.
///
/// From the Lightsail tagging documentation the modelled `tags` member links to:
/// "You can add up to 50 tags to each Lightsail resource."
pub const MAX_TAGS: usize = 50;

/// The most add-on requests one `CreateInstances` call may carry. The modelled
/// `AddOnRequestList` declares no length, and the `AddOnType` enum has only two
/// members, so no useful request needs more than this.
pub const MAX_ADD_ONS: usize = 8;

/// The most operations the log keeps, oldest dropped first.
///
/// Emulator resource guard: the log is append-only, so without a cap a long-lived
/// emulator would grow without bound.
pub const MAX_OPERATION_HISTORY: usize = 2_000;

/// How many instances one `GetInstances` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more instances remain.
pub const INSTANCE_PAGE_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// Catalog paging
// ---------------------------------------------------------------------------

/// One page of `reported`: at most `page_size` entries from the one `page_token`
/// names onward, and the id of the first entry of the page after it — present
/// only while entries remain.
///
/// A page token NAMES an entry (through `id_of`) rather than carrying an index,
/// so a catalog that gained or lost an entry between two pages cannot make the
/// walk skip or repeat another one. `None` is the answer when the token names no
/// entry of `reported`: there is no page to report, which the caller turns into a
/// refusal. An absent token asks for the first page.
///
/// Shared by every published catalog this service pages (blueprints, instance
/// bundles), so each one walks its catalog exactly the same way.
///
/// TOTAL on every input, including an arbitrary caller-supplied token: the token
/// is only ever COMPARED against each entry's id, the start comes from
/// `position`, so it is an index this slice holds, the page is taken by
/// `skip`/`take` rather than by a slice range, and the next page's first index is
/// a `saturating_add` — so there is no indexing, no slicing, no unwrap and no
/// arithmetic that can overflow or panic, and a token naming no entry answers
/// `None`.
fn catalog_page<T: Clone>(
    reported: &[T],
    page_size: usize,
    page_token: Option<&str>,
    id_of: impl Fn(&T) -> Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    // A page carries at least one entry, so a walk over a non-empty catalog
    // always advances.
    let page_size = page_size.max(1);
    let start = match page_token {
        None => 0,
        Some(token) => reported.iter().position(|entry| id_of(entry) == Some(token))?,
    };
    let page = reported
        .iter()
        .skip(start)
        .take(page_size)
        .cloned()
        .collect::<Vec<_>>();
    let next_page_token = reported
        .get(start.saturating_add(page_size))
        .and_then(|entry| id_of(entry).map(str::to_string));
    Some((page, next_page_token))
}

// ---------------------------------------------------------------------------
// Bundles
// ---------------------------------------------------------------------------

/// One Lightsail instance plan size: "a set of specs describing your virtual
/// private server (or instance)" (the modelled `Bundle` documentation).
///
/// A bundle id is `<size>_<major>_<minor>`, optionally with a platform token
/// (`micro_win_2_0`), so the leading token names the size and the rest names the
/// bundle's version. ONE entry here therefore describes a size: the hardware and
/// the monthly transfer every plan of that size provides, plus the monthly price
/// of that size on each platform — the same hardware costs more on Windows
/// Server, which carries an operating-system licence.
///
/// This is a PUBLISHED catalog, not per-account state: every account sees the
/// same plans and no operation in this service creates, changes or retires one.
/// That is why it is a `const` table beside the blueprint and Region catalogs,
/// and why `GetBundles` reports it without reading or writing a stored resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BundleSpec {
    /// The leading token of every bundle id that maps here, and the plan's
    /// `Bundle$instanceType` — "The instance type (`micro`)".
    pub size: &'static str,
    /// `Bundle$cpuCount` — "The number of vCPUs included in the bundle".
    pub cpu_count: i32,
    /// `Bundle$ramSizeInGb` — "The amount of RAM in GB".
    pub ram_size_in_gb: f32,
    /// `Bundle$diskSizeInGb` — "The size of the SSD".
    pub disk_size_in_gb: i32,
    /// `InstanceNetworking$monthlyTransfer.gbPerMonthAllocated` and
    /// `Bundle$transferPerMonthInGb` — "The data transfer rate per month in GB".
    pub transfer_gb_per_month: i32,
    /// `Bundle$power` — "A numeric value that represents the power of the
    /// bundle", compared against a blueprint's `minPower` to decide whether the
    /// blueprint runs on the plan.
    pub power: i32,
    /// `Bundle$price` — "The price in US dollars of the bundle" — for the
    /// Linux/Unix plan of this size.
    pub price_usd_per_month: f32,
    /// `Bundle$price` for the Windows plan of this size.
    pub windows_price_usd_per_month: f32,
    /// `Bundle$isActive` — "A Boolean value indicating whether the bundle is
    /// active". A plan generation the service has retired stays in this table
    /// with `false` so an instance already running on it still resolves, while
    /// `GetBundles` reports it only when asked to include inactive bundles.
    pub is_active: bool,
    /// `Bundle$supportedAppCategories` — "Virtual computer blueprints that are
    /// supported by a Lightsail for Research bundle", of which the member's own
    /// documentation says "This parameter only applies to Lightsail for Research
    /// resources".
    ///
    /// The categories are the PLAN's, exactly as its platforms are, so they are
    /// carried by the entry and never taken from the request. A standard instance
    /// plan supports none, which is why it is empty on every entry of
    /// [`BUNDLE_SPECS`] and the reported member is then absent.
    pub app_categories: &'static [&'static str],
}

/// Every instance plan size this service serves, smallest first.
///
/// SOURCE: the Lightsail instance-plan table (the "Instances" plans of the
/// Lightsail pricing page, which `GetBundles` answers from) — each step doubles
/// RAM while the vCPU count, SSD size and monthly data transfer follow the plan
/// table, the Linux/Unix monthly price doubles with the plan from the $5 micro
/// plan up, and the Windows price of the same size adds that platform's licence.
/// The `power` values are the ones Lightsail publishes for its plans: they rise
/// with the plan, from 300 for the smallest through 500 for `micro` (the value
/// the modelled `Blueprint$minPower` documentation's own example names) and then
/// a further 1000 per doubling step.
///
/// Lightsail has retired none of these sizes, so every entry is active.
pub const BUNDLE_SPECS: &[BundleSpec] = &[
    BundleSpec {
        size: "nano",
        cpu_count: 1,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_gb_per_month: 1_024,
        power: 300,
        price_usd_per_month: 3.5,
        windows_price_usd_per_month: 8.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "micro",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
        power: 500,
        price_usd_per_month: 5.0,
        windows_price_usd_per_month: 12.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "small",
        cpu_count: 1,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 60,
        transfer_gb_per_month: 3_072,
        power: 1_000,
        price_usd_per_month: 10.0,
        windows_price_usd_per_month: 20.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 80,
        transfer_gb_per_month: 4_096,
        power: 2_000,
        price_usd_per_month: 20.0,
        windows_price_usd_per_month: 40.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 160,
        transfer_gb_per_month: 5_120,
        power: 3_000,
        price_usd_per_month: 40.0,
        windows_price_usd_per_month: 70.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "xlarge",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 320,
        transfer_gb_per_month: 6_144,
        power: 4_000,
        price_usd_per_month: 80.0,
        windows_price_usd_per_month: 120.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "2xlarge",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 640,
        transfer_gb_per_month: 7_168,
        power: 5_000,
        price_usd_per_month: 160.0,
        windows_price_usd_per_month: 240.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "4xlarge",
        cpu_count: 16,
        ram_size_in_gb: 64.0,
        disk_size_in_gb: 1_280,
        transfer_gb_per_month: 8_192,
        power: 6_000,
        price_usd_per_month: 320.0,
        windows_price_usd_per_month: 480.0,
        is_active: true,
        app_categories: &[],
    },
];

/// The bundle `bundle_id` names, or `None` when its leading size token is not one
/// this service serves.
pub fn bundle_spec(bundle_id: &str) -> Option<BundleSpec> {
    let size = bundle_id.split('_').next()?;
    BUNDLE_SPECS
        .iter()
        .find(|spec| spec.size == size)
        .copied()
}

/// The bundle sizes [`bundle_spec`] accepts, for a refusal that has to tell the
/// caller what to send instead.
pub fn bundle_sizes() -> String {
    BUNDLE_SPECS
        .iter()
        .map(|spec| spec.size)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The generation token the bundle ids this service publishes carry — the
/// `<major>_<minor>` tail of a bundle id, as the modelled `Bundle$bundleId`
/// documentation's `micro_x_x` shows.
///
/// A plan size is served at one generation at a time, so the generation is a
/// property of the CATALOG rather than of a size. An instance created on another
/// generation of a size this service serves still resolves its hardware, because
/// [`bundle_spec`] resolves a bundle id by its leading size token.
pub const BUNDLE_GENERATION: &str = "3_0";

/// The token a Windows bundle id carries between its size and its generation
/// (`micro_win_2_0`), which is what distinguishes the Windows plan of a size from
/// its Linux/Unix plan.
pub const BUNDLE_WINDOWS_TOKEN: &str = "win";

/// The platforms every plan size is published for, in the order a page reports
/// them, from the modelled `InstancePlatform` enum: "You can only launch a
/// `WINDOWS` bundle on a blueprint that supports the `WINDOWS` platform.
/// `LINUX_UNIX` blueprints require a `LINUX_UNIX` bundle"
/// (`Bundle$supportedPlatforms`), so a size is a separate bundle on each.
pub const BUNDLE_PLATFORMS: &[&str] = &[PLATFORM_LINUX_UNIX, PLATFORM_WINDOWS];

/// `Bundle$publicIpv4AddressCount` of every plan this catalog publishes — "an
/// integer that indicates the public ipv4 address count included in the bundle,
/// the value is either 0 or 1".
///
/// Every published plan is a standard plan that includes one public IPv4 address;
/// 0 is the count of an IPv6-only plan, which this service publishes none of.
pub const BUNDLE_PUBLIC_IPV4_ADDRESS_COUNT: i32 = 1;

/// How many bundles one `GetBundles` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more bundles remain. It is above the
/// published catalog's size, so the whole catalog is answered in one page — as
/// AWS answers it — and the paging walk still holds for a catalog that outgrows
/// it.
pub const BUNDLE_PAGE_SIZE: usize = 50;

/// The id a plan of `size` carries on `platform` at `generation`: the size token,
/// the platform token of a Windows plan, then the generation
/// ([`BUNDLE_WINDOWS_TOKEN`], [`BUNDLE_GENERATION`]).
pub fn bundle_id(size: &str, platform: &str, generation: &str) -> String {
    if platform == PLATFORM_WINDOWS {
        format!("{size}_{BUNDLE_WINDOWS_TOKEN}_{generation}")
    } else {
        format!("{size}_{generation}")
    }
}

/// The friendly name of a plan of `size` — `Bundle$name`, "A friendly name for
/// the bundle (`Micro`)".
///
/// DERIVED from the size token, as that documented example shows (`micro` ->
/// `Micro`): the token title-cased, with an `xlarge` token written `XLarge`
/// because its leading `x` abbreviates "extra" rather than starting a word
/// (`2xlarge` -> `2XLarge`).
pub fn bundle_size_name(size: &str) -> String {
    if let Some(multiplier) = size.strip_suffix("xlarge") {
        return format!("{multiplier}XLarge");
    }
    let mut chars = size.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl BundleSpec {
    /// The monthly price of this plan on `platform`: the Windows price on
    /// Windows, the Linux/Unix one otherwise.
    pub fn price(&self, platform: &str) -> f32 {
        if platform == PLATFORM_WINDOWS {
            self.windows_price_usd_per_month
        } else {
            self.price_usd_per_month
        }
    }

    /// This plan on `platform` as the modelled `Bundle`.
    pub fn view(&self, platform: &str) -> Bundle {
        Bundle {
            bundle_id: Some(bundle_id(self.size, platform, BUNDLE_GENERATION)),
            instance_type: Some(self.size.to_string()),
            name: Some(bundle_size_name(self.size)),
            price: Some(self.price(platform)),
            cpu_count: Some(self.cpu_count),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            disk_size_in_gb: Some(self.disk_size_in_gb),
            transfer_per_month_in_gb: Some(self.transfer_gb_per_month),
            power: Some(self.power),
            is_active: Some(self.is_active),
            supported_platforms: Some(vec![platform.to_string()]),
            // `supportedAppCategories` is the plan's OWN data, like its platform:
            // a standard plan publishes none and reports the member absent.
            supported_app_categories: (!self.app_categories.is_empty()).then(|| {
                self.app_categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect()
            }),
            public_ipv4_address_count: Some(BUNDLE_PUBLIC_IPV4_ADDRESS_COUNT),
        }
    }
}

/// The Lightsail for Research instance plans this service publishes.
///
/// A SEPARATE catalog from [`BUNDLE_SPECS`], exactly as [`LFR_BLUEPRINTS`] is
/// separate from [`BLUEPRINTS`]: `GetBundles$appCategory` is documented as
/// "Returns a list of bundles that are specific to Lightsail for Research. You
/// must use this parameter to view Lightsail for Research bundles", so the two
/// catalogs are disjoint and the request's category selects between them (see
/// [`bundle_catalog`]).
///
/// This emulator publishes no Lightsail for Research plan, so the catalog is
/// EMPTY and a request naming a category reports no bundle. An entry here would
/// have to name a real Research plan; inventing one would make `GetBundles`
/// report a plan no Research blueprint runs on.
pub const LFR_BUNDLE_SPECS: &[BundleSpec] = &[];

/// The catalog a `GetBundles` request naming `app_category` reports from: the
/// Lightsail for Research plans when it names a category, the instance plans when
/// it names none.
///
/// The VALUE is not re-checked here: `appCategory` targets the modelled
/// `AppCategory` enum, whose value set the centralized constraint walk enforces
/// before any handler runs, so every category that reaches this function is one
/// the model admits.
pub fn bundle_catalog(app_category: Option<&str>) -> &'static [BundleSpec] {
    match app_category {
        Some(_) => LFR_BUNDLE_SPECS,
        None => BUNDLE_SPECS,
    }
}

/// The bundles a `GetBundles` call reports out of `specs`: every active plan of
/// every size on each platform of [`BUNDLE_PLATFORMS`], plus the inactive
/// (retired) ones when `include_inactive` is set.
///
/// Every member of a reported bundle is the entry's own — the request selects the
/// catalog and the slice, and nothing it carries is reported back as plan data.
///
/// The order is `specs`' own, each size's platforms in [`BUNDLE_PLATFORMS`]'
/// order, so two reads answer identically. Taking the catalog as an argument is
/// what lets the slice rule be exercised against a catalog that holds a retired
/// plan, which the published one does not.
pub fn bundles_from(specs: &[BundleSpec], include_inactive: bool) -> Vec<Bundle> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .flat_map(|spec| BUNDLE_PLATFORMS.iter().map(|platform| spec.view(platform)))
        .collect()
}

/// The bundles a `GetBundles` call reports: [`bundles_from`] over the catalog
/// [`bundle_catalog`] selects.
pub fn bundles(include_inactive: bool, app_category: Option<&str>) -> Vec<Bundle> {
    bundles_from(bundle_catalog(app_category), include_inactive)
}

/// One page of `reported`, by the same token walk every published catalog of this
/// service uses ([`catalog_page`]): a `GetBundles` page token is the bundle id a
/// previous page reported as `nextPageToken`.
pub fn bundle_page(
    reported: &[Bundle],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<Bundle>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |bundle| {
        bundle.bundle_id.as_deref()
    })
}

/// The bundle ids `reported` holds, for a refusal that has to tell the caller
/// what a page token may name.
pub fn reported_bundle_ids(reported: &[Bundle]) -> String {
    reported
        .iter()
        .filter_map(|bundle| bundle.bundle_id.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Bucket bundles
// ---------------------------------------------------------------------------

/// One Lightsail **bucket** bundle: the monthly cost, storage quota and transfer
/// quota a bucket gets from that plan.
///
/// This is the service's published price list for object storage, so it is the
/// same for every caller and no call changes it — `GetBucketBundles` reports it,
/// and `CreateBucket`/`UpdateBucketBundle` name an entry of it by `bundle_id`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketBundleSpec {
    /// The id a caller sends for this plan. A bucket bundle id is
    /// `<size>_<major>_<minor>`, the same `<size>_<version>` form an instance
    /// bundle id carries.
    pub bundle_id: &'static str,
    /// `BucketBundle$storagePerMonthInGb` — "The storage size of the bundle, in
    /// GB."
    pub storage_per_month_in_gb: i32,
    /// `BucketBundle$transferPerMonthInGb` — "The monthly network transfer quota
    /// of the bundle", in GB.
    pub transfer_per_month_in_gb: i32,
    /// `BucketBundle$price` — "The monthly price of the bundle, in US dollars."
    pub price_usd_per_month: f32,
    /// `BucketBundle$isActive` — "Indicates whether the bundle is active. Use for
    /// a new or existing bucket."
    pub is_active: bool,
}

/// Every bucket bundle this service publishes, cheapest first: the three
/// Lightsail object-storage plans from the Lightsail pricing page's object
/// storage table — 5 GB of storage with 25 GB of data transfer for $1 a month,
/// 100 GB with 250 GB for $3, and 250 GB with 500 GB for $5 — each the first
/// (`_1_0`) generation of its size.
///
/// Lightsail has retired no bucket plan, so every published plan is active. A
/// retired generation belongs in this table with `is_active: false`: that is
/// what makes it reportable only when `GetBucketBundles` is asked to include
/// inactive bundles, while a bucket already running on it still resolves.
pub const BUCKET_BUNDLE_SPECS: &[BucketBundleSpec] = &[
    BucketBundleSpec {
        bundle_id: "small_1_0",
        storage_per_month_in_gb: 5,
        transfer_per_month_in_gb: 25,
        price_usd_per_month: 1.0,
        is_active: true,
    },
    BucketBundleSpec {
        bundle_id: "medium_1_0",
        storage_per_month_in_gb: 100,
        transfer_per_month_in_gb: 250,
        price_usd_per_month: 3.0,
        is_active: true,
    },
    BucketBundleSpec {
        bundle_id: "large_1_0",
        storage_per_month_in_gb: 250,
        transfer_per_month_in_gb: 500,
        price_usd_per_month: 5.0,
        is_active: true,
    },
];

/// The label Lightsail gives an object-storage plan, with `storage` the plan's
/// storage quota in GB.
///
/// SOURCE: the `BucketBundle$name` values Lightsail reports — `Object Storage
/// 5GB` for the 5 GB plan, `Object Storage 100GB` for the 100 GB one and `Object
/// Storage 250GB` for the 250 GB one. The label names the product and the plan's
/// storage size, so it is DERIVED from the size rather than carried per entry: a
/// plan added to [`BUCKET_BUNDLE_SPECS`] is labelled the same way.
pub fn bucket_bundle_name(storage_per_month_in_gb: i32) -> String {
    format!("Object Storage {storage_per_month_in_gb}GB")
}

impl BucketBundleSpec {
    /// This plan as the modelled `BucketBundle`.
    ///
    /// `name` is "The name of the bundle" — the label Lightsail gives an object
    /// storage plan — so it is DERIVED from [`Self::storage_per_month_in_gb`] by
    /// [`bucket_bundle_name`] rather than carried a second time.
    pub fn view(&self) -> BucketBundle {
        BucketBundle {
            bundle_id: Some(self.bundle_id.to_string()),
            name: Some(bucket_bundle_name(self.storage_per_month_in_gb)),
            price: Some(self.price_usd_per_month),
            storage_per_month_in_gb: Some(self.storage_per_month_in_gb),
            transfer_per_month_in_gb: Some(self.transfer_per_month_in_gb),
            is_active: Some(self.is_active),
        }
    }
}

/// The bundles a `GetBucketBundles` call reports out of `specs`: every active
/// plan, plus the inactive (retired) ones when `include_inactive` is set.
///
/// The order is `specs`' own, so two reads answer identically. Taking the catalog
/// as an argument is what lets the slice rule be exercised against a catalog that
/// holds a retired plan, which the published one does not yet.
pub fn bucket_bundles_from(specs: &[BucketBundleSpec], include_inactive: bool) -> Vec<BucketBundle> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .map(BucketBundleSpec::view)
        .collect()
}

/// The bucket bundles a `GetBucketBundles` call reports: [`bucket_bundles_from`]
/// over this service's published catalog, [`BUCKET_BUNDLE_SPECS`].
pub fn bucket_bundles(include_inactive: bool) -> Vec<BucketBundle> {
    bucket_bundles_from(BUCKET_BUNDLE_SPECS, include_inactive)
}

/// The bucket bundle `bundle_id` names — active or not, since a bucket already
/// running on a retired plan still has to resolve — or `None` when this service
/// publishes no such plan.
pub fn bucket_bundle_spec(bundle_id: &str) -> Option<BucketBundleSpec> {
    BUCKET_BUNDLE_SPECS
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .copied()
}

// ---------------------------------------------------------------------------
// Buckets
// ---------------------------------------------------------------------------

/// The `BucketState$code` of a bucket that is serving, from that member's own
/// documentation: "`OK` - The bucket is in a running state." (its only other
/// value, `Unknown`, says "Creation of the bucket might have timed-out", which no
/// bucket this store holds did).
pub const BUCKET_STATE_OK: &str = "OK";

/// `Bucket$objectVersioning` when versioning was asked for, from that member's
/// own documentation: "`Enabled` - Object versioning is enabled."
pub const OBJECT_VERSIONING_ENABLED: &str = "Enabled";

/// `Bucket$objectVersioning` when versioning was enabled and then turned off,
/// from that member's own documentation: "`Suspended` - Object versioning was
/// previously enabled but is currently suspended."
///
/// No stored bucket carries this value yet: suspending versioning is
/// `UpdateBucket`'s business, and that operation is still a 501 scaffold under
/// its own issue. The constant records the modelled value the store will hold
/// once it is implemented.
pub const OBJECT_VERSIONING_SUSPENDED: &str = "Suspended";

/// `Bucket$objectVersioning` when versioning was never asked for, from that
/// member's own documentation: "`NeverEnabled` - Object versioning has never been
/// enabled."
pub const OBJECT_VERSIONING_NEVER_ENABLED: &str = "NeverEnabled";

/// The `AccessRules$getObject` of a new bucket: the modelled `AccessType` value
/// `private`, which that member documents as "Sets all objects in the bucket to
/// private, making them readable only by you or anyone you give access to" — the
/// permission a bucket has until `UpdateBucket` widens it.
pub const ACCESS_TYPE_PRIVATE: &str = "private";

/// The `AccountLevelBpaSync$status` this emulator reports, from that member's own
/// documentation: "`NeverSynced` - Synchronization has not yet happened." Nothing
/// here fetches an Amazon S3 account-level block-public-access configuration, so
/// no synchronization has happened and none is claimed.
pub const ACCOUNT_LEVEL_BPA_NEVER_SYNCED: &str = "NeverSynced";

/// The `AccountLevelBpaSync$message` that goes with [`ACCOUNT_LEVEL_BPA_NEVER_SYNCED`],
/// from the modelled `BPAStatusMessage` documentation: "`SYNC_ON_HOLD` - The
/// synchronization has not yet happened. This status message occurs immediately
/// after you create your first Lightsail bucket."
pub const ACCOUNT_LEVEL_BPA_SYNC_ON_HOLD: &str = "SYNC_ON_HOLD";

/// The `AccessKey$status` of a bucket access key that is valid, from that
/// member's own documentation: "A status of `Active` means that the key is valid,
/// while `Inactive` means it is not." One of the two values of the modelled
/// `StatusType` enum the member targets.
pub const ACCESS_KEY_STATUS_ACTIVE: &str = "Active";

/// The `AccessKeyLastUsed$region` and `$serviceName` of an access key nothing has
/// signed a request with, from those members' own documentation: "This value is
/// `N/A` if the access key has not been used."
pub const ACCESS_KEY_LAST_USED_NEVER: &str = "N/A";

/// The most access keys one bucket may hold, from the modelled `AccessKey`
/// documentation: "You can have a maximum of two access keys per bucket."
///
/// An AWS limit, not an emulator guard: a create past it is refused and mints
/// nothing.
pub const MAX_BUCKET_ACCESS_KEYS: usize = 2;

/// The most buckets this emulator will hold, across every Region together.
///
/// Emulator resource guard on the store as a whole, NOT an AWS quota (which is
/// per Region and per account). A create that would exceed it is refused with
/// the modelled `InvalidInputException`.
pub const MAX_BUCKETS: usize = 100;

/// How many buckets one `GetBuckets` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more buckets remain.
pub const BUCKET_PAGE_SIZE: usize = 10;

/// The path space every bucket's address is published under, beneath the
/// endpoint the emulator is reached on.
///
/// AWS publishes a Lightsail bucket's address as a DNS name under the Amazon S3
/// zone that backs it (`<bucket>.s3.<region>.amazonaws.com`). That HOSTNAME is
/// environment-specific — no caller may assume it — while the PROMISE the field
/// makes is not: `Bucket$url` is "The URL of the bucket", so it has to be an
/// address the caller can actually reach. This emulator owns no DNS zone, so the
/// only address it can be reached on is the endpoint the request arrived at, and
/// the two labels AWS puts in the hostname (region and bucket name) are carried
/// in the path instead — never a literal AWS hostname, which would send the next
/// client out of the emulator entirely (see [`request_endpoint`]). The prefix
/// names this service and this resource type, so it cannot collide with another
/// service's modelled URI space.
///
/// Nothing claims or answers this path space yet: a bucket's object data plane
/// (put, get, list) is not implemented here, and it is `can_handle`'s business to
/// claim the path once it is, as `CONTAINER_SERVICE_PUBLIC_PATH_PREFIX` already
/// does for that resource. Until then the address says WHERE this emulator would
/// serve the bucket — within this emulator rather than out at AWS — and a client
/// following it gets the router's unknown-route answer.
pub const BUCKET_PUBLIC_PATH_PREFIX: &str = "/lightsail-bucket";

/// The URL of the bucket `name` in `region`, as announced to a caller that
/// reached this emulator at `endpoint` (its `Host`/`:authority`).
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on. See
/// [`BUCKET_PUBLIC_PATH_PREFIX`] for why the address is a path under the
/// request's own endpoint rather than an AWS hostname.
pub fn bucket_url(endpoint: &str, region: &str, name: &str) -> String {
    format!("https://{endpoint}{BUCKET_PUBLIC_PATH_PREFIX}/{region}/{name}")
}

/// The ARN of the bucket `id` in `region`. A bucket is regional, so its ARN
/// carries the region it was created in, and the resource portion mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows with
/// this resource's own `ResourceType` value.
pub fn bucket_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{BUCKET_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `objectVersioning` a bucket reports for the `enableObjectVersioning` its
/// create request carried: an absent flag is not a request to version, which is
/// the `NeverEnabled` case.
pub fn object_versioning_from_request(enable_object_versioning: Option<bool>) -> &'static str {
    if enable_object_versioning.unwrap_or(false) {
        OBJECT_VERSIONING_ENABLED
    } else {
        OBJECT_VERSIONING_NEVER_ENABLED
    }
}

/// The account-level block-public-access synchronization status `GetBuckets`
/// reports.
///
/// This emulator fetches nothing from Amazon S3, so the honest answer is the
/// never-synchronized one: no `lastSyncedAt` (the member documents "This value is
/// null when the account-level BPA configuration has not been synchronized"), and
/// account-level BPA therefore affects no Lightsail bucket here.
pub fn account_level_bpa_sync() -> AccountLevelBpaSync {
    AccountLevelBpaSync {
        status: Some(ACCOUNT_LEVEL_BPA_NEVER_SYNCED.to_string()),
        last_synced_at: None,
        message: Some(ACCOUNT_LEVEL_BPA_SYNC_ON_HOLD.to_string()),
        bpa_impacts_lightsail: Some(false),
    }
}

/// One access key of a bucket: what `CreateBucketAccessKey` minted and
/// `GetBucketAccessKeys` reports.
///
/// The key's SECRET is deliberately NOT a field here. Both the id and the secret
/// are minted by the emulator's credential store, which is the only thing that
/// creates credential material, and the secret "is returned only in response to
/// the `CreateBucketAccessKey` action" — so the create hands it to its caller and
/// this record keeps only the id, exactly as AWS keeps no retrievable copy.
#[derive(Debug, Clone)]
pub struct StoredBucketAccessKey {
    /// The id the credential store minted, which is this key's identity and the
    /// `accessKeyId` every read reports.
    pub access_key_id: String,
    /// One of the modelled `StatusType` values — see [`ACCESS_KEY_STATUS_ACTIVE`].
    pub status: String,
    pub created_at_ms: i64,
}

impl StoredBucketAccessKey {
    /// This record as the modelled `AccessKey`, for a read of the bucket's keys.
    ///
    /// No `secretAccessKey`: `GetBucketAccessKeys` "does not return the secret
    /// access key value of an access key", and nothing here holds one to return.
    ///
    /// `lastUsed` reports the never-used form the member documents (`N/A` region
    /// and service name, null `lastUsedDate`): this emulator records no use of a
    /// credential — a bucket's object data plane is not served here, so no request
    /// against this bucket can have been signed with the key — and claiming a use
    /// instant for work nothing did would be worse than saying it has not been
    /// used.
    pub fn view(&self) -> AccessKey {
        AccessKey {
            access_key_id: Some(self.access_key_id.clone()),
            secret_access_key: None,
            status: Some(self.status.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            last_used: Some(AccessKeyLastUsed {
                last_used_date: None,
                region: Some(ACCESS_KEY_LAST_USED_NEVER.to_string()),
                service_name: Some(ACCESS_KEY_LAST_USED_NEVER.to_string()),
            }),
        }
    }

    /// This record as the modelled `AccessKey` answered by the create that minted
    /// it: the one response that carries `secret_access_key`, and the one that
    /// carries NO `lastUsed` — the modelled `AccessKeyLastUsed` documents that
    /// "this object does not include data in the response of a
    /// `CreateBucketAccessKey` action".
    pub fn created_view(&self, secret_access_key: String) -> AccessKey {
        AccessKey {
            secret_access_key: Some(secret_access_key),
            last_used: None,
            ..self.view()
        }
    }
}

/// One stored bucket: the object-storage resource `CreateBucket` created.
///
/// What the caller asked for — name, bundle, tags, whether to version objects —
/// is kept verbatim; the ARN, support code, creation time, URL and state are
/// assigned by this store from the request plus its own allocators.
#[derive(Debug, Clone)]
pub struct StoredBucket {
    /// The bucket name the request named, kept verbatim — it is half of this
    /// record's store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The bundle currently applied, an id of [`BUCKET_BUNDLE_SPECS`].
    pub bundle_id: String,
    /// One of [`OBJECT_VERSIONING_ENABLED`], [`OBJECT_VERSIONING_SUSPENDED`] or
    /// [`OBJECT_VERSIONING_NEVER_ENABLED`].
    pub object_versioning: String,
    /// Whether the applied bundle may still be changed. A bucket's bundle can be
    /// updated only once within a monthly billing cycle, so a bucket that has not
    /// been moved yet can be.
    pub able_to_update_bundle: bool,
    /// The anonymous access permissions of the bucket's objects.
    pub access_rules: AccessRules,
    /// Accounts given read-only access to the bucket.
    pub readonly_access_accounts: Vec<String>,
    /// The Lightsail instances `SetResourceAccessForBucket` gave access to the
    /// bucket.
    pub resources_receiving_access: Vec<ResourceReceivingAccess>,
    /// Where the bucket's access logs are written, and whether they are written
    /// at all.
    pub access_log_config: BucketAccessLogConfig,
    /// The cross-origin resource sharing rules of the bucket.
    pub cors_rules: Vec<BucketCorsRule>,
    /// The access keys `CreateBucketAccessKey` minted for this bucket, in the
    /// order they were created — at most [`MAX_BUCKET_ACCESS_KEYS`] of them.
    pub access_keys: Vec<StoredBucketAccessKey>,
    /// One of the modelled `BucketState$code` values — see [`BUCKET_STATE_OK`].
    pub state_code: String,
}

impl StoredBucket {
    /// This record as the modelled `Bucket`, announced to a caller that reached
    /// this emulator at `endpoint` (its `Host`/`:authority`, as
    /// [`request_endpoint`] reads it).
    ///
    /// `url` is the one member that depends on the request: it is the address the
    /// caller is told to reach this bucket on, so it has to be the endpoint THIS
    /// request arrived at (see [`BUCKET_PUBLIC_PATH_PREFIX`]). A request that
    /// names no host at all names no endpoint to announce, so `url` is omitted
    /// rather than filled with an address nothing serves.
    ///
    /// `include_connected_resources` and `include_cors` are the `GetBuckets`
    /// flags of the same names: `resourcesReceivingAccess` is reported only when
    /// the caller asked for the connected resources, and `cors` only when it
    /// asked for the CORS configuration ("This field is only included in the
    /// response when CORS configuration is requested", per `Bucket$cors`).
    pub fn view(
        &self,
        endpoint: Option<&str>,
        include_connected_resources: bool,
        include_cors: bool,
    ) -> Bucket {
        Bucket {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_BUCKET.to_string()),
            tags: Some(self.tags.clone()),
            bundle_id: Some(self.bundle_id.clone()),
            url: endpoint
                .map(|endpoint| bucket_url(endpoint, &self.location.region_name, &self.name)),
            object_versioning: Some(self.object_versioning.clone()),
            able_to_update_bundle: Some(self.able_to_update_bundle),
            access_rules: Some(self.access_rules.clone()),
            readonly_access_accounts: Some(self.readonly_access_accounts.clone()),
            resources_receiving_access: include_connected_resources
                .then(|| self.resources_receiving_access.clone()),
            state: Some(BucketState {
                code: Some(self.state_code.clone()),
                // `BucketState$message` "describes the state of the bucket"; a
                // running bucket has nothing to explain.
                message: None,
            }),
            access_log_config: Some(self.access_log_config.clone()),
            cors: include_cors.then(|| BucketCorsConfig {
                rules: Some(self.cors_rules.clone()),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Blueprints
// ---------------------------------------------------------------------------

/// The default user name for connecting to an instance of each blueprint family,
/// from the Lightsail user guide's "Connecting to your instance" tables: the
/// operating-system blueprints connect as `ec2-user` except Debian (`admin`) and
/// Ubuntu (`ubuntu`), Windows connects as `Administrator`, and every
/// Bitnami-packaged application blueprint connects as `bitnami`.
///
/// Matching is by longest id prefix. A blueprint family that is not listed
/// reports NO `username`: the member is optional, and guessing one would be worse
/// than saying nothing.
const BLUEPRINT_USERNAMES: &[(&str, &str)] = &[
    ("almalinux", "ec2-user"),
    ("amazon_linux", "ec2-user"),
    ("centos", "ec2-user"),
    ("debian", "admin"),
    ("freebsd", "ec2-user"),
    ("opensuse", "ec2-user"),
    ("ubuntu", "ubuntu"),
    ("windows", "Administrator"),
    ("django", "bitnami"),
    ("drupal", "bitnami"),
    ("ghost", "bitnami"),
    ("gitlab", "bitnami"),
    ("joomla", "bitnami"),
    ("lamp", "bitnami"),
    ("magento", "bitnami"),
    ("mean", "bitnami"),
    ("nginx", "bitnami"),
    ("nodejs", "bitnami"),
    ("prestashop", "bitnami"),
    ("redmine", "bitnami"),
    ("wordpress", "bitnami"),
];

/// The user name an instance of `blueprint_id` connects as, or `None` when the
/// blueprint is not one of the families in [`BLUEPRINT_USERNAMES`].
pub fn blueprint_username(blueprint_id: &str) -> Option<&'static str> {
    BLUEPRINT_USERNAMES
        .iter()
        .filter(|(prefix, _)| blueprint_id.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, username)| *username)
}

/// Whether `blueprint_id` is a Windows blueprint. Every Lightsail Windows
/// blueprint id begins `windows_` (`windows_server_2022`); every other blueprint
/// is a Linux/Unix one.
pub fn blueprint_is_windows(blueprint_id: &str) -> bool {
    blueprint_id.starts_with("windows")
}

/// The friendly blueprint name, rendered from the blueprint id by title-casing
/// its underscore-separated words.
///
/// That is the derivation the modelled `Instance$blueprintName` documentation
/// shows: blueprint id `amazon_linux_2023` is reported as `Amazon Linux 2023`.
pub fn blueprint_name(blueprint_id: &str) -> String {
    blueprint_id
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One entry of the instance blueprint catalog: a virtual private server image
/// an instance can be created from.
///
/// This is a PUBLISHED catalog, not per-account state — every account sees the
/// same entries — which is why it is a `const` table rather than a field on the
/// store, exactly like the bundle and Region catalogs above.
#[derive(Debug, Clone, Copy)]
pub struct BlueprintSpec {
    /// `Blueprint$blueprintId`: the id a `CreateInstances` request names
    /// (`amazon_linux_2023`, `wordpress`).
    pub blueprint_id: &'static str,
    /// `Blueprint$type`, one of the modelled `BlueprintType` enum's two values
    /// (`smithy_query(query="shape",shape="BlueprintType")`): `os` for an
    /// operating-system image, `app` for an application stack.
    pub blueprint_type: &'static str,
    /// `Blueprint$isActive`. From that member's own documentation: "Inactive
    /// blueprints are listed to support customers with existing instances but
    /// are not necessarily available for launch of new instances" — and
    /// `CreateInstances$blueprintId` says "Use active blueprints when creating
    /// new instances", so only an active entry can be created from.
    pub is_active: bool,
}

/// One value of the modelled `BlueprintType` enum: an operating-system image.
pub const BLUEPRINT_TYPE_OS: &str = "os";

/// One value of the modelled `BlueprintType` enum: an application stack.
pub const BLUEPRINT_TYPE_APP: &str = "app";

/// The instance blueprints this service publishes — the catalog `GetBlueprints`
/// reports and `CreateInstances` resolves `blueprintId` against.
///
/// SOURCE: the blueprint ids Lightsail's instance-image list publishes (the
/// Lightsail console's "Select a blueprint" catalog, which `GetBlueprints`
/// answers from): the operating-system families `amazon_linux`, `ubuntu`,
/// `debian`, `almalinux`, `centos_stream`, `freebsd`, `opensuse` and
/// `windows_server`, each at the releases Lightsail offers, plus the
/// Bitnami-packaged and hosting-panel application stacks. An entry whose
/// upstream release has reached end of life stays in the catalog but is marked
/// inactive, which is the state Lightsail keeps a retired image in so existing
/// instances still resolve it while a new instance cannot be launched from it.
pub const BLUEPRINTS: &[BlueprintSpec] = &[
    // Operating systems.
    BlueprintSpec {
        blueprint_id: "amazon_linux_2",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "amazon_linux_2023",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "almalinux_9",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "centos_stream_9",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "debian_10",
        blueprint_type: BLUEPRINT_TYPE_OS,
        // Debian 10 is past its upstream end of life.
        is_active: false,
    },
    BlueprintSpec {
        blueprint_id: "debian_11",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "debian_12",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "freebsd_14",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "opensuse_15",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_18_04",
        blueprint_type: BLUEPRINT_TYPE_OS,
        // Ubuntu 18.04 LTS is past its upstream end of standard support.
        is_active: false,
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_20_04",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_22_04",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_24_04",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2016",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2019",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2022",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
    },
    // Application stacks.
    BlueprintSpec {
        blueprint_id: "cpanel_whm_almalinux",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "django",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "drupal",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "ghost_bitnami",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "gitlab",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "joomla",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "lamp_8_bitnami",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "magento",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "mean",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "nginx",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "nodejs",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "plesk_ubuntu_18_0_35",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "redmine",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "wordpress",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
    BlueprintSpec {
        blueprint_id: "wordpress_multisite",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
    },
];

/// The blueprint `blueprint_id` names — active or not, since an instance already
/// running a retired image still has to resolve it — or `None` when this service
/// publishes no such blueprint.
pub fn blueprint_spec(blueprint_id: &str) -> Option<BlueprintSpec> {
    BLUEPRINTS
        .iter()
        .find(|spec| spec.blueprint_id == blueprint_id)
        .copied()
}

/// Whether a new instance can be created from `blueprint_id`: the catalog holds
/// it AND the entry is still active.
pub fn blueprint_is_creatable(blueprint_id: &str) -> bool {
    blueprint_spec(blueprint_id).is_some_and(|spec| spec.is_active)
}

/// One value of the modelled `InstancePlatform` enum
/// (`smithy_query(query="shape",shape="InstancePlatform")`): a Linux/Unix image.
pub const PLATFORM_LINUX_UNIX: &str = "LINUX_UNIX";

/// The other value of the modelled `InstancePlatform` enum: a Windows
/// Server-based image.
pub const PLATFORM_WINDOWS: &str = "WINDOWS";

/// The Lightsail for Research virtual-computer blueprints this service publishes.
///
/// A SEPARATE catalog from [`BLUEPRINTS`]: `GetBlueprints$appCategory` is
/// documented as "Returns a list of blueprints that are specific to Lightsail for
/// Research. You must use this parameter to view Lightsail for Research
/// blueprints", so the two catalogs are disjoint and the request's category is
/// what selects between them (see [`blueprint_catalog`]).
///
/// This emulator publishes no Lightsail for Research blueprint, so the catalog is
/// EMPTY and a request naming a category reports no blueprint. An entry here
/// would have to name a real Research image; inventing one would make
/// `GetBlueprints` report a blueprint `CreateInstances` cannot create from.
pub const LFR_BLUEPRINTS: &[BlueprintSpec] = &[];

/// How many blueprints one `GetBlueprints` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more blueprints remain. It is above the
/// published catalog's size, so the whole catalog is answered in one page — as AWS
/// answers it — and the paging walk still holds for a catalog that outgrows it.
pub const BLUEPRINT_PAGE_SIZE: usize = 50;

/// Whether a word of a blueprint id names a version rather than the blueprint
/// family: a version word begins with a digit (`ubuntu_22_04` -> `22`, `04`;
/// `amazon_linux_2023` -> `2023`).
fn is_version_word(word: &str) -> bool {
    word.starts_with(|first: char| first.is_ascii_digit())
}

impl BlueprintSpec {
    /// `Blueprint$group` — "The group name of the blueprint (`amazon-linux`)".
    ///
    /// The group is the blueprint FAMILY: the id with its version words removed
    /// and its underscores written as hyphens, which is exactly the derivation
    /// that documented example shows (`amazon_linux_2023` -> `amazon-linux`).
    pub fn group(&self) -> String {
        self.blueprint_id
            .split('_')
            .filter(|word| !word.is_empty() && !is_version_word(word))
            .collect::<Vec<_>>()
            .join("-")
    }

    /// `Blueprint$version` — "The version number of the operating system,
    /// application, or stack (`2016.03.0`)".
    ///
    /// A Lightsail blueprint id spells that version in its own version words, so
    /// the version is those words joined with dots (`ubuntu_22_04` -> `22.04`,
    /// `plesk_ubuntu_18_0_35` -> `18.0.35`). A blueprint whose id names no
    /// version reports NONE: the member is optional, and an invented version
    /// number would say something the catalog does not know.
    pub fn version(&self) -> Option<String> {
        let version = self
            .blueprint_id
            .split('_')
            .filter(|word| is_version_word(word))
            .collect::<Vec<_>>()
            .join(".");
        (!version.is_empty()).then_some(version)
    }

    /// `Blueprint$platform` — "The operating system platform (either
    /// Linux/Unix-based or Windows Server-based) of the blueprint", one of the
    /// modelled `InstancePlatform` enum's two values. Windows is the platform of
    /// exactly the Windows blueprints ([`blueprint_is_windows`]), the same
    /// classification that decides which management port a new instance opens.
    pub fn platform(&self) -> &'static str {
        if blueprint_is_windows(self.blueprint_id) {
            PLATFORM_WINDOWS
        } else {
            PLATFORM_LINUX_UNIX
        }
    }

    /// `Blueprint$description` — "The description of the blueprint".
    ///
    /// DERIVED from the two facts the catalog holds about the entry: its friendly
    /// name and its `type`, which says whether the image is an operating system
    /// or an application stack preinstalled on one. A per-entry marketing
    /// paragraph is not something this catalog knows.
    pub fn description(&self) -> String {
        let name = blueprint_name(self.blueprint_id);
        if self.blueprint_type == BLUEPRINT_TYPE_APP {
            format!("{name} application stack, preinstalled on a Lightsail instance.")
        } else {
            format!("{name} operating system image for a Lightsail instance.")
        }
    }

    /// This entry as the modelled `Blueprint`.
    ///
    /// `name` is [`blueprint_name`], the same derivation `Instance$blueprintName`
    /// reports, so the catalog's name for a blueprint is the name an instance
    /// created from it carries.
    pub fn view(&self) -> Blueprint {
        Blueprint {
            blueprint_id: Some(self.blueprint_id.to_string()),
            name: Some(blueprint_name(self.blueprint_id)),
            group: Some(self.group()),
            r#type: Some(self.blueprint_type.to_string()),
            description: Some(self.description()),
            is_active: Some(self.is_active),
            // `minPower` is "The minimum bundle power required to run this
            // blueprint ... `0` indicates that the blueprint runs on all instance
            // sizes". Every blueprint this catalog publishes runs on every plan
            // [`BUNDLE_SPECS`] serves, which is what that 0 states.
            min_power: Some(0),
            version: self.version(),
            // `versionCode` is Lightsail's own revision counter for an image, and
            // `productUrl`/`licenseUrl` are per-image links. This catalog
            // publishes none of the three and all three members are optional, so
            // they are left ABSENT rather than carrying an invented value.
            version_code: None,
            product_url: None,
            license_url: None,
            platform: Some(self.platform().to_string()),
            // Only a Lightsail for Research blueprint carries a category, and it
            // is the category of the catalog the entry came from, so
            // [`blueprints_from`] stamps it rather than the entry.
            app_category: None,
        }
    }
}

/// The catalog a `GetBlueprints` request naming `app_category` reports from: the
/// Lightsail for Research blueprints when it names a category, the instance
/// blueprints when it names none.
///
/// From `GetBlueprints$appCategory`: "You must use this parameter to view
/// Lightsail for Research blueprints" — so the standard catalog answers a request
/// that names no category, and only a request that names one sees the Research
/// blueprints.
///
/// The VALUE is not re-checked here: `appCategory` targets the modelled
/// `AppCategory` enum, whose value set the centralized constraint walk enforces
/// before any handler runs (a category the model does not name is already a 400
/// `ValidationException`), so every category that reaches this function is one the
/// model admits.
pub fn blueprint_catalog(app_category: Option<&str>) -> &'static [BlueprintSpec] {
    match app_category {
        Some(_) => LFR_BLUEPRINTS,
        None => BLUEPRINTS,
    }
}

/// The blueprints a `GetBlueprints` call reports out of `specs`: every active
/// entry, plus the inactive (retired) ones when `include_inactive` is set, each
/// stamped with `app_category` — the category of the catalog it came from.
///
/// The order is `specs`' own, so two reads answer identically. Taking the catalog
/// as an argument is what lets the slice rule be exercised against a catalog
/// whose shape the published one does not have yet.
pub fn blueprints_from(
    specs: &[BlueprintSpec],
    include_inactive: bool,
    app_category: Option<&str>,
) -> Vec<Blueprint> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .map(|spec| Blueprint {
            app_category: app_category.map(str::to_string),
            ..spec.view()
        })
        .collect()
}

/// The blueprints a `GetBlueprints` call reports: [`blueprints_from`] over the
/// catalog [`blueprint_catalog`] selects.
pub fn blueprints(include_inactive: bool, app_category: Option<&str>) -> Vec<Blueprint> {
    blueprints_from(
        blueprint_catalog(app_category),
        include_inactive,
        app_category,
    )
}

/// One page of `reported`: at most `page_size` blueprints from the entry
/// `page_token` names onward, and the id of the first entry of the page after it
/// — present only while entries remain.
///
/// A page token NAMES a blueprint rather than carrying an index, so a catalog that
/// gained or lost an entry between two pages cannot make the walk skip or repeat
/// another one. `None` is the answer when the token names no blueprint of
/// `reported`: there is no page to report, which the caller turns into a refusal.
/// An absent token asks for the first page.
///
/// The walk itself is [`catalog_page`], shared with this service's other paged
/// catalogs; only the member a token names is this catalog's own.
pub fn blueprint_page(
    reported: &[Blueprint],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<Blueprint>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |blueprint| {
        blueprint.blueprint_id.as_deref()
    })
}

/// The blueprint ids `reported` holds, for a refusal that has to tell the caller
/// what a page token may name.
pub fn reported_blueprint_ids(reported: &[Blueprint]) -> String {
    reported
        .iter()
        .filter_map(|blueprint| blueprint.blueprint_id.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Relational database blueprints
// ---------------------------------------------------------------------------

// This section is a PUBLISHED catalog, not per-account state: the data every
// account sees the same, which the reporting operation over it answers from. Its
// source is named below, and the modelled view type is referred to by its full
// path so this section needs nothing from the import list above.

/// One entry of the managed-database blueprint catalog: a database engine and one
/// MAJOR version of it a database can be created from.
///
/// "A blueprint describes the major engine version of a database" (the modelled
/// `RelationalDatabaseBlueprint` documentation), while
/// `RelationalDatabaseBlueprint$engineVersion` carries the full version "(for
/// example, `5.7.23`)" — so a blueprint's id names the major version and its
/// `engineVersion` names the patch level that major version is served at.
#[derive(Debug, Clone, Copy)]
pub struct RelationalDatabaseBlueprintSpec {
    /// The database software this blueprint installs.
    ///
    /// The modelled `RelationalDatabaseEngine` enum
    /// (`smithy_query(query="shape",shape="RelationalDatabaseEngine")`) still
    /// carries the single value `mysql`, which the service has outgrown:
    /// Lightsail also serves PostgreSQL managed databases, and the recorded
    /// `GetRelationalDatabaseBlueprints` answer reports `postgres` blueprints
    /// beside the MySQL ones. The catalog reports the engines the service
    /// serves — the member is the engine of the blueprint, not of the enum's
    /// last revision.
    pub engine: &'static str,
    /// How the engine is written in prose, as the engine descriptions show it
    /// (`MySQL`).
    pub engine_display: &'static str,
    /// `RelationalDatabaseBlueprint$engineDescription`: the edition of the engine
    /// the blueprint installs.
    pub engine_description: &'static str,
    /// The major engine version this blueprint describes (`8.0`).
    pub major_version: &'static str,
    /// The full engine version the major version is served at (`8.0.41`).
    pub engine_version: &'static str,
    /// Whether this is the engine's default version — true for exactly one
    /// blueprint per engine.
    pub is_engine_default: bool,
}

/// The database blueprints this service serves: the MySQL blueprint first, then
/// the PostgreSQL ones by ascending major version — the order the recorded
/// `GetRelationalDatabaseBlueprints` answer reports them in, whose first entry is
/// `mysql_8_4`.
///
/// Source: the engines and major versions the Lightsail user guide's "Database
/// engines in Amazon Lightsail" lists for a managed database — MySQL 8.4 and
/// PostgreSQL 13 through 18 — each pinned to the patch release of that series the
/// service serves it at (`mysql_8_4` at the recorded `8.4.11`; each PostgreSQL
/// series at the latest community patch release of that series). A major version
/// the service retires or adds is a row removed or added here, and the default
/// flag moves with the engine's default version — the series the service creates
/// a database at when the caller names no other, which for PostgreSQL is the
/// newest series that is not the opt-in latest.
///
/// Both engines are served even though the modelled `RelationalDatabaseEngine`
/// enum still names only `mysql`: see [`RelationalDatabaseBlueprintSpec::engine`].
pub const RELATIONAL_DATABASE_BLUEPRINTS: &[RelationalDatabaseBlueprintSpec] = &[
    RelationalDatabaseBlueprintSpec {
        engine: "mysql",
        engine_display: "MySQL",
        engine_description: "MySQL Community Edition",
        major_version: "8.4",
        engine_version: "8.4.11",
        is_engine_default: true,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "13",
        engine_version: "13.21",
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "14",
        engine_version: "14.18",
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "15",
        engine_version: "15.13",
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "16",
        engine_version: "16.9",
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "17",
        engine_version: "17.5",
        is_engine_default: true,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "18",
        engine_version: "18.1",
        is_engine_default: false,
    },
];

/// How many blueprints one `GetRelationalDatabaseBlueprints` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more blueprints remain. It is above the
/// catalog's size, so the whole catalog is answered in one page — as AWS answers
/// it — and the paging walk still holds for a catalog that outgrows it.
pub const RELATIONAL_DATABASE_BLUEPRINT_PAGE_SIZE: usize = 10;

impl RelationalDatabaseBlueprintSpec {
    /// This blueprint's id: the engine and its major version, joined the way every
    /// Lightsail id token is (`mysql` + `8.0` -> `mysql_8_0`).
    pub fn blueprint_id(&self) -> String {
        format!("{}_{}", self.engine, self.major_version.replace('.', "_"))
    }

    /// This entry as the modelled `RelationalDatabaseBlueprint`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabaseBlueprint {
        crate::ops::types::RelationalDatabaseBlueprint {
            blueprint_id: Some(self.blueprint_id()),
            engine: Some(self.engine.to_string()),
            engine_version: Some(self.engine_version.to_string()),
            engine_description: Some(self.engine_description.to_string()),
            // `engineVersionDescription` describes the version the way the engine
            // itself is named beside it: "MySQL 5.7.44".
            engine_version_description: Some(format!(
                "{} {}",
                self.engine_display, self.engine_version
            )),
            is_engine_default: Some(self.is_engine_default),
        }
    }
}

/// The blueprint `blueprint_id` names, or `None` when the catalog has no such
/// blueprint.
pub fn relational_database_blueprint(
    blueprint_id: &str,
) -> Option<RelationalDatabaseBlueprintSpec> {
    RELATIONAL_DATABASE_BLUEPRINTS
        .iter()
        .find(|spec| spec.blueprint_id() == blueprint_id)
        .copied()
}

/// The blueprint ids [`relational_database_blueprint`] accepts, for a refusal that
/// has to tell the caller what to send instead.
pub fn relational_database_blueprint_ids() -> String {
    RELATIONAL_DATABASE_BLUEPRINTS
        .iter()
        .map(RelationalDatabaseBlueprintSpec::blueprint_id)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Relational database bundles
// ---------------------------------------------------------------------------

// Like the blueprint catalog above, this is PUBLISHED data rather than
// per-account state: the managed-database plan list, identical for every
// account, which the reporting operation over it answers from. No call changes
// it, so reading it leaves every later caller the answer it would have had.

/// One entry of the managed-database bundle catalog: "A bundle describes the
/// performance specifications of the database" (the modelled
/// `RelationalDatabaseBundle` documentation) — the hardware, the included
/// monthly data transfer and the monthly price a database created on that plan
/// runs with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelationalDatabaseBundleSpec {
    /// `RelationalDatabaseBundle$bundleId` — "The ID for the database bundle".
    ///
    /// A database bundle id is `<size>_<major>_<minor>`, the `<size>_<version>`
    /// form every Lightsail bundle id carries, with an `ha` token after the size
    /// on the high-availability plans (`micro_ha_1_0`).
    pub bundle_id: &'static str,
    /// `RelationalDatabaseBundle$name` — "The name for the database bundle": the
    /// label Lightsail gives the plan.
    pub name: &'static str,
    /// `RelationalDatabaseBundle$cpuCount` — "The number of virtual CPUs
    /// (vCPUs) for the database bundle."
    pub cpu_count: i32,
    /// `RelationalDatabaseBundle$ramSizeInGb` — "The amount of RAM in GB (for
    /// example, `2.0`) for the database bundle."
    pub ram_size_in_gb: f32,
    /// `RelationalDatabaseBundle$diskSizeInGb` — "The size of the disk for the
    /// database bundle."
    pub disk_size_in_gb: i32,
    /// `RelationalDatabaseBundle$transferPerMonthInGb` — "The data transfer rate
    /// per month in GB for the database bundle."
    pub transfer_per_month_in_gb: i32,
    /// `RelationalDatabaseBundle$price` — "The cost of the database bundle in US
    /// currency", per month.
    pub price_usd_per_month: f32,
    /// `RelationalDatabaseBundle$isEncrypted` — "A Boolean value indicating
    /// whether the database bundle is encrypted."
    pub is_encrypted: bool,
    /// `RelationalDatabaseBundle$isActive` — "A Boolean value indicating whether
    /// the database bundle is active." A plan the service has retired stays in
    /// the catalog as inactive: a database already running on it must still
    /// resolve, and `GetRelationalDatabaseBundles` reports it only when asked to
    /// include inactive bundles.
    pub is_active: bool,
}

/// Every managed-database plan this service publishes: the four standard
/// (single-database) plans cheapest first, then the four high-availability plans
/// cheapest first.
///
/// SOURCE: the managed-database plan table of the Lightsail pricing page — a
/// standard plan at $15 a month with 1 GB RAM, 1 vCPU and 40 GB of SSD storage,
/// $30 with 2 GB / 1 vCPU / 80 GB, $60 with 4 GB / 2 vCPUs / 120 GB and $115
/// with 8 GB / 2 vCPUs / 240 GB, with 100 GB of data transfer included on the
/// first three plans and 200 GB on the 8 GB plan — and the high-availability
/// column of the same table, which offers the same hardware with a standby in a
/// second Availability Zone for twice the standard plan's price ($30, $60, $120
/// and $230). Lightsail encrypts managed-database storage and its automatic
/// snapshots at rest on every plan, so every entry is encrypted.
///
/// Lightsail has retired no database plan, so every published plan is active; a
/// retired generation belongs here with `is_active: false`.
///
/// The ORDER is this slice's own (cheapest first, standard plans before
/// high-availability ones): it is what the paged walk reports, and it is stable
/// so two reads answer identically.
pub const RELATIONAL_DATABASE_BUNDLE_SPECS: &[RelationalDatabaseBundleSpec] = &[
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_1_0",
        name: "Micro",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 15.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_1_0",
        name: "Small",
        cpu_count: 1,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_1_0",
        name: "Medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "large_1_0",
        name: "Large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 115.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_ha_1_0",
        name: "Micro with High Availability",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_ha_1_0",
        name: "Small with High Availability",
        cpu_count: 1,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_ha_1_0",
        name: "Medium with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 120.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "large_ha_1_0",
        name: "Large with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 230.0,
        is_encrypted: true,
        is_active: true,
    },
];

/// How many database bundles one `GetRelationalDatabaseBundles` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more bundles remain. It is above the
/// catalog's size, so the whole catalog is answered in one page — as AWS answers
/// it — and the paging walk still holds for a catalog that outgrows it.
pub const RELATIONAL_DATABASE_BUNDLE_PAGE_SIZE: usize = 20;

impl RelationalDatabaseBundleSpec {
    /// Whether this plan is a high-availability one: a database created on it
    /// runs with a standby in a second Availability Zone.
    ///
    /// From the id's own form, documented on [`Self::bundle_id`]: a
    /// high-availability plan carries an `ha` token after the size
    /// (`micro_ha_1_0`), which the standard plan of the same size does not.
    pub fn is_high_availability(&self) -> bool {
        self.bundle_id.contains("_ha_")
    }

    /// This plan as the modelled `RelationalDatabaseBundle`.
    pub fn view(&self) -> RelationalDatabaseBundle {
        RelationalDatabaseBundle {
            bundle_id: Some(self.bundle_id.to_string()),
            name: Some(self.name.to_string()),
            price: Some(self.price_usd_per_month),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            disk_size_in_gb: Some(self.disk_size_in_gb),
            transfer_per_month_in_gb: Some(self.transfer_per_month_in_gb),
            cpu_count: Some(self.cpu_count),
            is_encrypted: Some(self.is_encrypted),
            is_active: Some(self.is_active),
        }
    }
}

/// The bundles a `GetRelationalDatabaseBundles` call reports out of `specs`,
/// before paging: every active plan, plus the inactive (retired) ones when
/// `include_inactive` is set.
///
/// The order is `specs`' own. Taking the catalog as an argument is what lets the
/// slice rule be exercised against a catalog that holds a retired plan, which the
/// published one does not yet.
///
/// Unlike its bucket counterpart [`bucket_bundles_from`], this answers the SPECS
/// rather than the modelled view: this operation is paged, and a page token names
/// an entry by its `bundle_id`, so [`relational_database_bundle_page`] slices the
/// specs first and calls [`RelationalDatabaseBundleSpec::view`] only on the
/// entries one page carries.
pub fn relational_database_bundles_from(
    specs: &[RelationalDatabaseBundleSpec],
    include_inactive: bool,
) -> Vec<RelationalDatabaseBundleSpec> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .copied()
        .collect()
}

/// The database bundles a `GetRelationalDatabaseBundles` call reports:
/// [`relational_database_bundles_from`] over this service's published catalog,
/// [`RELATIONAL_DATABASE_BUNDLE_SPECS`].
pub fn relational_database_bundles(include_inactive: bool) -> Vec<RelationalDatabaseBundleSpec> {
    relational_database_bundles_from(RELATIONAL_DATABASE_BUNDLE_SPECS, include_inactive)
}

/// One page of the database bundle catalog: the bundles it carries and the token
/// that opens the page after it.
#[derive(Debug, Clone)]
pub struct RelationalDatabaseBundlePage {
    /// The page's bundles, as the modelled view.
    pub bundles: Vec<RelationalDatabaseBundle>,
    /// The id of the first bundle of the NEXT page, present only while bundles
    /// remain.
    pub next_page_token: Option<String>,
}

/// One page of the slice of `specs` that `include_inactive` selects: the slice
/// from the entry `page_token` names onward, at most `page_size` entries, with
/// `next_page_token` naming the first entry of the page after it.
///
/// `None` when `page_token` names no entry of that slice — the caller refuses it
/// with the modelled `InvalidInputException`. A token names an ENTRY rather than
/// an index, so a catalog that gained or lost a plan between two pages cannot
/// make the walk skip or repeat another one. An absent — or empty — token asks
/// for the first page.
pub fn relational_database_bundle_page(
    specs: &[RelationalDatabaseBundleSpec],
    include_inactive: bool,
    page_token: Option<&str>,
    page_size: usize,
) -> Option<RelationalDatabaseBundlePage> {
    // A page carries at least one entry: a zero-length page would name its own
    // first entry as the next token and never advance.
    let page_size = page_size.max(1);
    let slice = relational_database_bundles_from(specs, include_inactive);

    let start = match page_token.filter(|token| !token.is_empty()) {
        None => 0,
        Some(token) => slice.iter().position(|spec| spec.bundle_id == token)?,
    };

    let mut bundles = Vec::new();
    let mut next_page_token = None;
    for spec in &slice[start..] {
        if bundles.len() == page_size {
            next_page_token = Some(spec.bundle_id.to_string());
            break;
        }
        bundles.push(spec.view());
    }

    Some(RelationalDatabaseBundlePage {
        bundles,
        next_page_token,
    })
}

/// The database bundle `bundle_id` names — active or not, since a database
/// already running on a retired plan still has to resolve — or `None` when this
/// service publishes no such plan.
///
/// This is the lookup a call that NAMES a plan resolves it through:
/// `CreateRelationalDatabase`'s `relationalDatabaseBundleId` and
/// `UpdateRelationalDatabase`, each still a scaffold stub with its own issue.
pub fn relational_database_bundle_spec(bundle_id: &str) -> Option<RelationalDatabaseBundleSpec> {
    RELATIONAL_DATABASE_BUNDLE_SPECS
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .copied()
}

/// The bundle ids a `GetRelationalDatabaseBundles` page token may name out of
/// `specs`, for a refusal that has to tell the caller what to send instead.
pub fn relational_database_bundle_ids(specs: &[RelationalDatabaseBundleSpec]) -> String {
    specs
        .iter()
        .map(|spec| spec.bundle_id)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Locations
// ---------------------------------------------------------------------------

/// The Lightsail region an availability zone belongs to, or `None` when the zone
/// is not `<region><zone-letter>` for a region Lightsail serves.
///
/// `availabilityZone` is documented as "us-east-2a (case sensitive)", and
/// `ResourceLocation$regionName` targets the modelled `RegionName` enum — so the
/// zone's region must be one of that enum's values, and the catalog is that enum
/// (`smithy_query(query="shape",shape="RegionName")`, generated here as
/// `REGION_NAME_VALUES`).
///
/// The enum, not [`REGION_SPECS`]: this is the set of Region names a stored
/// `ResourceLocation` may carry, which is wider than the Regions `GetRegions`
/// lists as served.
pub fn region_of_availability_zone(availability_zone: &str) -> Option<&'static str> {
    let zone_letter = availability_zone.chars().next_back()?;
    if !zone_letter.is_ascii_lowercase() {
        return None;
    }
    let region = &availability_zone[..availability_zone.len() - 1];
    REGION_NAME_VALUES
        .iter()
        .find(|known| **known == region)
        .copied()
}

/// The regions [`region_of_availability_zone`] accepts, for a refusal that has to
/// tell the caller what to send instead.
pub fn region_names() -> String {
    REGION_NAME_VALUES.join(", ")
}

// ---------------------------------------------------------------------------
// Region catalog
// ---------------------------------------------------------------------------

/// One Region Lightsail serves, as `GetRegions` reports it.
///
/// The catalog is the Regions the service SERVES, which is a subset of the
/// modelled `RegionName` enum (`smithy_query(query="shape",shape="RegionName")`,
/// generated here as `REGION_NAME_VALUES`): the enum is the set of Region names
/// a `ResourceLocation` may carry, and it still admits `us-west-1`, which
/// Lightsail does not operate in and does not list. Each entry adds the values
/// the enum cannot carry: the place AWS names the Region after
/// (`Region$displayName` — "The display name (`Ohio`)"), the area AWS
/// recommends it for (`Region$description` — "This region is recommended to
/// serve users in the eastern United States and eastern Canada") and the
/// Availability Zones the Region publishes. The continent code is not stored,
/// because it follows from the name: see [`continent_code_of_region`].
#[derive(Debug, Clone, Copy)]
pub struct RegionSpec {
    /// The `RegionName` value, as `Region$name` reports it.
    pub name: &'static str,
    /// The place the Region is named after, as `Region$displayName` reports it.
    pub display_name: &'static str,
    /// The area the Region is recommended for: the clause `Region$description`
    /// puts after "This region is recommended to serve users in".
    pub serves: &'static str,
    /// The zone letters of this Region's Availability Zones, in order: a zone
    /// name is the Region's name followed by one of these letters, the format
    /// `AvailabilityZone$zoneName` documents ("The format is `us-east-2a`
    /// (case-sensitive)").
    ///
    /// Per Region, because the count and the letters differ per Region and are
    /// not a run of `a`, `b`, `c`: `us-west-2` publishes four zones and Tokyo
    /// publishes `a`, `c` and `d` with no `b`.
    pub availability_zone_letters: &'static str,
}

impl RegionSpec {
    /// The sentence `Region$description` carries, in the form that member's own
    /// documentation shows.
    pub fn description(&self) -> String {
        format!(
            "This region is recommended to serve users in {}",
            self.serves
        )
    }

    /// The Availability Zones this Region publishes, as `GetRegions` reports
    /// them to a caller that asked for them: one per letter in
    /// [`Self::availability_zone_letters`], each named `<region><letter>` and
    /// serving.
    pub fn availability_zones(&self) -> Vec<AvailabilityZone> {
        self.zone_names()
            .into_iter()
            .map(|zone_name| AvailabilityZone {
                zone_name: Some(zone_name),
                state: Some(AVAILABILITY_ZONE_STATE_AVAILABLE.to_string()),
            })
            .collect()
    }

    /// The names of the Availability Zones this Region publishes, in the
    /// catalog's order — the `<region><letter>` form
    /// [`Self::availability_zone_letters`] documents.
    pub fn zone_names(&self) -> Vec<String> {
        self.availability_zone_letters
            .chars()
            .map(|letter| format!("{}{letter}", self.name))
            .collect()
    }

    /// This Region as the modelled `Region`, for a request served in
    /// `request_region`.
    ///
    /// BOTH zone lists are ALWAYS reported, empty when they carry nothing: they
    /// are list members, and an absent member is not an empty list to a typed
    /// caller — it hands an SDK a null where the caller iterates a collection,
    /// instead of saying "no zones to report here".
    ///
    /// A list carries zones only when the request asked for that list AND this
    /// Region is the one the request was served in: a Lightsail endpoint is
    /// regional and reports the Availability Zones of its OWN Region, so every
    /// other Region in the catalog answers an empty zone list however the
    /// include flags are set. That is what the recorded answer shows — the zones
    /// of one Region, the request's own, and empty lists for all the others.
    pub fn view(
        &self,
        include_zones: bool,
        include_relational_database_zones: bool,
        request_region: &str,
    ) -> Region {
        let is_request_region = self.name == request_region;
        let zones = |include: bool| {
            if include && is_request_region {
                self.availability_zones()
            } else {
                Vec::new()
            }
        };
        Region {
            continent_code: continent_code_of_region(self.name).map(str::to_string),
            description: Some(self.description()),
            display_name: Some(self.display_name.to_string()),
            name: Some(self.name.to_string()),
            availability_zones: Some(zones(include_zones)),
            // Lightsail publishes no zone list for databases separate from a
            // Region's own, so both lists draw on the same zones.
            relational_database_availability_zones: Some(zones(
                include_relational_database_zones,
            )),
        }
    }
}

/// Every Region this service serves, in the order `GetRegions` reports them:
/// the US Regions first — the recorded answer opens with `us-east-1` — then the
/// rest in the order AWS lists its Regions in, with the Regions opened later
/// appended. `GetRegions` walks this slice, so two calls answer one stable
/// order.
///
/// MEMBERSHIP: the Regions Lightsail operates in. That is a subset of the
/// modelled `RegionName` enum, which also admits `us-west-1`: Lightsail serves
/// no resources there and the recorded answer carries no such entry, so the
/// catalog has no row for it.
///
/// `display_name` is the city, state or country AWS names each Region after
/// (ASCII, as AWS reports it: `Sao Paulo`). `serves` is the geography that
/// Region is the closest one to — the clause after the sentence stem
/// `Region$description` documents. `us-east-1`, `us-east-2`, `us-west-2`,
/// `ap-northeast-1` and `ap-northeast-2` carry the clause AWS's own answer gives
/// for them (the two eastern US Regions share one clause); the rest name the
/// geography the Lightsail documentation recommends that Region for, because the
/// per-Region clause AWS writes follows no rule the model or the documentation
/// states.
///
/// `availability_zone_letters` is the Availability Zones the Lightsail user
/// guide's "Regions and Availability Zones" table lists for each Region: six in
/// `us-east-1`, four in `us-west-2` (`a` through `d`, the recorded set), two in
/// `ca-central-1`, and `a`, `c`, `d` in `ap-northeast-1`, which publishes no
/// `b`.
pub const REGION_SPECS: &[RegionSpec] = &[
    RegionSpec {
        name: "us-east-1",
        display_name: "Virginia",
        serves: "the eastern United States",
        availability_zone_letters: "abcdef",
    },
    RegionSpec {
        name: "us-east-2",
        display_name: "Ohio",
        serves: "the eastern United States",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "us-west-2",
        display_name: "Oregon",
        serves: "the northwestern United States, Alaska, and western Canada",
        availability_zone_letters: "abcd",
    },
    RegionSpec {
        name: "ap-south-1",
        display_name: "Mumbai",
        serves: "India",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-northeast-2",
        display_name: "Seoul",
        serves: "South Korea",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-1",
        display_name: "Singapore",
        serves: "Singapore and southeast Asia",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-2",
        display_name: "Sydney",
        serves: "Australia and New Zealand",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-northeast-1",
        display_name: "Tokyo",
        serves: "Japan",
        availability_zone_letters: "acd",
    },
    RegionSpec {
        name: "ca-central-1",
        display_name: "Montreal",
        serves: "Canada",
        availability_zone_letters: "ab",
    },
    RegionSpec {
        name: "eu-central-1",
        display_name: "Frankfurt",
        serves: "Germany and central Europe",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-1",
        display_name: "Ireland",
        serves: "Ireland and western Europe",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-2",
        display_name: "London",
        serves: "the United Kingdom",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-3",
        display_name: "Paris",
        serves: "France",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-north-1",
        display_name: "Stockholm",
        serves: "the Nordic countries",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "sa-east-1",
        display_name: "Sao Paulo",
        serves: "Brazil and eastern South America",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-east-1",
        display_name: "Hong Kong",
        serves: "Hong Kong and southern China",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-south-2",
        display_name: "Spain",
        serves: "Spain and Portugal",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-3",
        display_name: "Jakarta",
        serves: "Indonesia",
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-5",
        display_name: "Malaysia",
        serves: "Malaysia",
        availability_zone_letters: "abc",
    },
];

/// The Regions `GetRegions` reports, as the modelled `Region` list, for a
/// request served in `request_region` — [`REGION_SPECS`] viewed in its own
/// order.
///
/// One function, so the catalog is walked the same way wherever it is reported.
pub fn regions_view(
    include_zones: bool,
    include_relational_database_zones: bool,
    request_region: &str,
) -> Vec<Region> {
    REGION_SPECS
        .iter()
        .map(|spec| {
            spec.view(
                include_zones,
                include_relational_database_zones,
                request_region,
            )
        })
        .collect()
}

/// The catalog entry for the Region `name`, or `None` when this service serves
/// no such Region.
pub fn region_spec(name: &str) -> Option<&'static RegionSpec> {
    REGION_SPECS.iter().find(|spec| spec.name == name)
}

/// The catalog entry for the Region that publishes the Availability Zone `zone`,
/// or `None` when no Region this service serves publishes it.
///
/// STRICTER than [`region_of_availability_zone`], which checks only that the
/// zone is a `<region><letter>` for a Region the modelled `RegionName` enum
/// admits: a letter the Region does not publish names no zone a resource can be
/// created in, and this is what a caller that resolves a zone through
/// `GetRegions` gets back.
pub fn region_spec_of_availability_zone(zone: &str) -> Option<&'static RegionSpec> {
    REGION_SPECS
        .iter()
        .find(|spec| spec.zone_names().iter().any(|name| name == zone))
}

/// The continent code of the Region `name`, or `None` when its geography prefix
/// is not one this service serves.
///
/// An AWS Region name begins with the geography it sits in, so the continent
/// code follows from the name rather than needing its own table: `us-`/`ca-` are
/// North America, `sa-` South America, `eu-` Europe and `ap-` Asia Pacific —
/// the pairing the modelled `Region$continentCode` documentation shows ("The
/// continent code (`NA`, meaning North America)"). An unlisted prefix reports
/// NO continent code: the member is optional, and guessing one would be worse
/// than saying nothing.
pub fn continent_code_of_region(name: &str) -> Option<&'static str> {
    match name.split('-').next()? {
        "us" | "ca" => Some("NA"),
        "sa" => Some("SA"),
        "eu" => Some("EU"),
        "ap" => Some("AP"),
        _ => None,
    }
}

/// The `state` a serving Availability Zone reports.
///
/// A Lightsail Availability Zone is an EC2 one, and `available` is EC2's state
/// for a zone that is operating normally.
pub const AVAILABILITY_ZONE_STATE_AVAILABLE: &str = "available";

/// Where a stored record lives.
///
/// `availability_zone` is absent for a resource that is not zonal: the member
/// documents the `us-east-2a` form, which addresses one zone of one region, and
/// a DNS zone is held by the region as a whole — as is a container service — so
/// there is no zone to name.
#[derive(Debug, Clone)]
pub struct StoredLocation {
    pub availability_zone: Option<String>,
    pub region_name: String,
}

impl StoredLocation {
    /// The location of a resource created in one availability zone.
    pub fn zonal(availability_zone: &str, region_name: &str) -> Self {
        Self {
            availability_zone: Some(availability_zone.to_string()),
            region_name: region_name.to_string(),
        }
    }

    /// The location of a resource that lives in `region` as a whole.
    pub fn regional(region: &str) -> Self {
        Self {
            availability_zone: None,
            region_name: region.to_string(),
        }
    }

    /// The location of a domain recordset: [`DOMAIN_REGION_NAME`], and no
    /// availability zone at all.
    pub fn domain() -> Self {
        Self {
            availability_zone: None,
            region_name: DOMAIN_REGION_NAME.to_string(),
        }
    }

    /// This location as the modelled `ResourceLocation`.
    pub fn view(&self) -> ResourceLocation {
        ResourceLocation {
            availability_zone: self.availability_zone.clone(),
            region_name: Some(self.region_name.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

/// Base of the range public addresses are drawn from: `198.18.0.0/15`, the RFC
/// 2544 benchmarking range. It is reserved and routes nowhere, so an emulated
/// instance never advertises an address that belongs to somebody else.
const PUBLIC_IPV4_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// Base of the range private addresses are drawn from: `172.26.0.0/16`, the CIDR
/// a Lightsail region's default VPC uses.
const PRIVATE_IPV4_BASE: u32 = u32::from_be_bytes([172, 26, 0, 0]);

/// `base` offset by `offset`, as a dotted quad — the form the modelled
/// `IpAddress` pattern `^([0-9]{1,3}\.){3}[0-9]{1,3}$` requires.
fn ipv4(base: u32, offset: u32) -> String {
    let [a, b, c, d] = base.wrapping_add(offset).to_be_bytes();
    format!("{a}.{b}.{c}.{d}")
}

/// One address from `2001:DB8::/32`, the RFC 3849 documentation prefix, rendered
/// as the eight uppercase hex groups the modelled `Ipv6Address` pattern
/// `^([A-F0-9]{1,4}:){7}[A-F0-9]{1,4}$` requires.
fn ipv6(offset: u32) -> String {
    let high = (offset >> 16) & 0xFFFF;
    let low = offset & 0xFFFF;
    format!("2001:DB8:0:0:0:0:{high:X}:{low:X}")
}

/// Whether `ip_address_type` gives the instance an IPv4 address, from the
/// modelled `IpAddressType` enum's own documentation: `ipv4` is IPv4 only,
/// `ipv6` is IPv6 only and `dualstack` is both.
pub fn ip_address_type_has_ipv4(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv4" | "dualstack")
}

/// Whether `ip_address_type` gives the instance an IPv6 address. See
/// [`ip_address_type_has_ipv4`].
pub fn ip_address_type_has_ipv6(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv6" | "dualstack")
}

// ---------------------------------------------------------------------------
// Firewall
// ---------------------------------------------------------------------------

/// The management port a new instance's firewall opens: SSH for a Linux/Unix
/// blueprint, RDP for a Windows one.
const SSH_PORT: i32 = 22;
/// See [`SSH_PORT`].
const RDP_PORT: i32 = 3389;
/// The web port a new instance's firewall opens.
const HTTP_PORT: i32 = 80;

/// The inbound rules Lightsail opens on a newly created instance's firewall:
/// the platform's management port (SSH for Linux/Unix, RDP for Windows) and HTTP,
/// both reachable from anywhere. Taken from the Lightsail user guide's
/// "Firewalls and ports" description of a new instance's default rules.
///
/// The IPv6 side of each rule is present only when the instance has an IPv6
/// address at all, so the reported rules match the addresses the instance has.
fn default_ports(blueprint_id: &str, ip_address_type: &str) -> Vec<InstancePortInfo> {
    let management = if blueprint_is_windows(blueprint_id) {
        RDP_PORT
    } else {
        SSH_PORT
    };
    [management, HTTP_PORT]
        .into_iter()
        .map(|port| InstancePortInfo {
            from_port: port,
            to_port: port,
            protocol: Some("tcp".to_string()),
            access_from: Some("Anywhere (0.0.0.0/0)".to_string()),
            access_type: Some("Public".to_string()),
            common_name: None,
            access_direction: Some("inbound".to_string()),
            cidrs: ip_address_type_has_ipv4(ip_address_type)
                .then(|| vec!["0.0.0.0/0".to_string()]),
            ipv6_cidrs: ip_address_type_has_ipv6(ip_address_type)
                .then(|| vec!["::/0".to_string()]),
            cidr_list_aliases: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One stored instance: every member `GetInstance` reports, plus the launch
/// script `CreateInstances` accepted and the clock-owed transition its lifecycle
/// is waiting on.
#[derive(Debug, Clone)]
pub struct StoredInstance {
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub blueprint_id: String,
    pub bundle_id: String,
    pub add_ons: Vec<AddOn>,
    /// No static IP is attached: `AllocateStaticIp` / `AttachStaticIp` are not
    /// part of this slice, so nothing can have attached one.
    pub is_static_ip: bool,
    pub private_ip_address: Option<String>,
    pub public_ip_address: Option<String>,
    pub ipv6_addresses: Vec<String>,
    pub ip_address_type: String,
    pub bundle: BundleSpec,
    pub ssh_key_name: String,
    /// The launch script the request carried, kept so the operations that serve
    /// it can report it; no member of `Instance` exposes it.
    pub user_data: Option<String>,
    pub state: Lifecycle,
    /// The state the clock owes this instance, while one is outstanding.
    pub due: Option<DueTransition>,
}

impl StoredInstance {
    /// This record as the modelled `Instance`.
    pub fn view(&self) -> Instance {
        Instance {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_INSTANCE.to_string()),
            tags: Some(self.tags.clone()),
            blueprint_id: Some(self.blueprint_id.clone()),
            blueprint_name: Some(blueprint_name(&self.blueprint_id)),
            bundle_id: Some(self.bundle_id.clone()),
            add_ons: Some(self.add_ons.clone()),
            is_static_ip: Some(self.is_static_ip),
            private_ip_address: self.private_ip_address.clone(),
            public_ip_address: self.public_ip_address.clone(),
            ipv6_addresses: Some(self.ipv6_addresses.clone()),
            ip_address_type: Some(self.ip_address_type.clone()),
            hardware: Some(InstanceHardware {
                cpu_count: Some(self.bundle.cpu_count),
                ram_size_in_gb: Some(self.bundle.ram_size_in_gb),
                // The bundle's system disk is reported through
                // `GetInstance`'s own `hardware.disks`; attaching a block
                // storage disk is `AttachDisk`, which is not in this slice,
                // so the only disk an instance has is the system one.
                disks: Some(vec![self.system_disk()]),
            }),
            networking: Some(InstanceNetworking {
                monthly_transfer: Some(MonthlyTransfer {
                    gb_per_month_allocated: Some(self.bundle.transfer_gb_per_month),
                }),
                ports: Some(default_ports(&self.blueprint_id, &self.ip_address_type)),
            }),
            state: Some(InstanceState {
                code: Some(self.state.code),
                name: Some(self.state.name.to_string()),
            }),
            username: blueprint_username(&self.blueprint_id).map(str::to_string),
            ssh_key_name: Some(self.ssh_key_name.clone()),
            metadata_options: Some(self.metadata_options()),
        }
    }

    /// The instance's system disk: the bundle's SSD, mounted at the root and
    /// attached for as long as the instance exists.
    ///
    /// The system disk is not a Lightsail `Disk` resource of its own — nothing
    /// created it and no name or ARN addresses it — so it reports only what it
    /// has: its size, its path, and the instance it is attached to.
    /// `attachmentState` is the discontinued member the modelled docs say is
    /// "still included in the response", returning `attached` for a system disk.
    fn system_disk(&self) -> crate::ops::types::Disk {
        crate::ops::types::Disk {
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            size_in_gb: Some(self.bundle.disk_size_in_gb),
            is_system_disk: Some(true),
            path: Some("/dev/xvda".to_string()),
            // `DiskState` enum: a disk an instance is using is `in-use`.
            state: Some("in-use".to_string()),
            attached_to: Some(self.name.clone()),
            is_attached: Some(true),
            attachment_state: Some("attached".to_string()),
            ..Default::default()
        }
    }

    /// The instance metadata service's configuration.
    ///
    /// `state` is `applied` because nothing is mid-update:
    /// `UpdateInstanceMetadataOptions` is not part of this slice, so the options
    /// an instance carries are the ones it was created with. `httpProtocolIpv6`
    /// follows the instance's own IP address type — an instance without an IPv6
    /// address cannot reach an IPv6 metadata endpoint. `httpTokens` is `optional`
    /// and the hop limit is 1, the values Lightsail creates an instance with and
    /// that `UpdateInstanceMetadataOptions` exists to change.
    fn metadata_options(&self) -> InstanceMetadataOptions {
        InstanceMetadataOptions {
            state: Some("applied".to_string()),
            http_tokens: Some("optional".to_string()),
            http_endpoint: Some("enabled".to_string()),
            http_put_response_hop_limit: Some(1),
            http_protocol_ipv6: Some(
                if ip_address_type_has_ipv6(&self.ip_address_type) {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                },
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Block storage disks
// ---------------------------------------------------------------------------

/// The smallest block storage disk this service creates, in GB.
///
/// The size range Lightsail publishes for an additional block storage disk is
/// 8 GB to 16 TB; the modelled `CreateDisk$sizeInGb` carries no `@range`, so the
/// bound is enforced here rather than by the modelled-constraint walk.
pub const MIN_DISK_SIZE_IN_GB: i32 = 8;

/// The largest block storage disk this service creates, in GB: 16 TB, the upper
/// end of the same published range as [`MIN_DISK_SIZE_IN_GB`].
pub const MAX_DISK_SIZE_IN_GB: i32 = 16 * 1024;

/// IOPS per GB of disk, and the floor and ceiling that bracket the figure.
///
/// A Lightsail block storage disk is an EBS general purpose SSD (`gp2`) volume,
/// whose published baseline performance is 3 IOPS per GB, never below 100 IOPS
/// and never above 16,000.
const DISK_IOPS_PER_GB: i32 = 3;
/// See [`DISK_IOPS_PER_GB`].
const DISK_IOPS_FLOOR: i32 = 100;
/// See [`DISK_IOPS_PER_GB`].
const DISK_IOPS_CEILING: i32 = 16_000;

/// The IOPS a block storage disk of `size_in_gb` provides, from the baseline in
/// [`DISK_IOPS_PER_GB`].
///
/// DERIVED from the disk's own size, so every read of one disk reports the same
/// figure and a larger disk reports more of them.
pub fn disk_iops(size_in_gb: i32) -> i32 {
    size_in_gb
        .saturating_mul(DISK_IOPS_PER_GB)
        .clamp(DISK_IOPS_FLOOR, DISK_IOPS_CEILING)
}

/// One stored block storage disk: every member `GetDisk` reports, plus the
/// clock-owed transition its state is waiting on.
#[derive(Debug, Clone)]
pub struct StoredDisk {
    /// The disk name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub add_ons: Vec<AddOn>,
    pub size_in_gb: i32,
    /// The current `DiskState` value: one of [`DISK_STATE_PENDING`],
    /// [`DISK_STATE_AVAILABLE`] or [`DISK_STATE_IN_USE`].
    pub state: &'static str,
    /// The state the clock owes this disk, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instance this disk is attached to, while one is. `AttachDisk` is what
    /// sets it; a newly created disk is attached to nothing.
    pub attached_to: Option<String>,
    /// The device path the disk is mounted at on the instance it is attached to.
    /// `AttachDisk` carries it (`diskPath`), so an unattached disk has none.
    pub path: Option<String>,
}

impl StoredDisk {
    /// This record as the modelled `Disk`.
    pub fn view(&self) -> crate::ops::types::Disk {
        let attached = self.attached_to.is_some();
        crate::ops::types::Disk {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISK.to_string()),
            tags: Some(self.tags.clone()),
            add_ons: Some(self.add_ons.clone()),
            size_in_gb: Some(self.size_in_gb),
            // A disk `CreateDisk` created is additional block storage: the only
            // system disk is the one an instance's bundle provides, which is not
            // a `Disk` resource of its own.
            is_system_disk: Some(false),
            iops: Some(disk_iops(self.size_in_gb)),
            path: self.path.clone(),
            state: Some(self.state.to_string()),
            attached_to: self.attached_to.clone(),
            is_attached: Some(attached),
            // The discontinued member the modelled docs say is "still included
            // in the response" and that `isAttached` replaced, so it reports the
            // same fact in the older wording.
            attachment_state: Some(
                if attached {
                    "attached".to_string()
                } else {
                    "detached".to_string()
                },
            ),
            // Discontinued: the modelled `Disk$gbInUse` documentation says this
            // parameter "is now discontinued", so Lightsail reports none.
            gb_in_use: None,
            // The modelled `Disk$autoMountStatus` documentation says it "only
            // applies to Lightsail for Research resources"; a disk created
            // through `CreateDisk` is not one, so there is no mount to report.
            auto_mount_status: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Disk snapshots
// ---------------------------------------------------------------------------

/// The resource one disk snapshot was taken of: the modelled `DiskSnapshot`
/// reports the source's name AND its ARN, and a snapshot has exactly one source
/// — the modelled `CreateDiskSnapshot$diskName` and `$instanceName`
/// documentation says the two "are mutually exclusive" — so the pair is stored
/// as one value rather than as four independent members that could disagree.
#[derive(Debug, Clone)]
pub enum DiskSnapshotSource {
    /// A block storage disk, snapshotted through `CreateDiskSnapshot`'s
    /// `diskName`.
    Disk { name: String, arn: String },
    /// An instance's system volume, snapshotted through `instanceName`: "When
    /// this is defined, a snapshot of the instance's system volume is created."
    Instance { name: String, arn: String },
}

/// The `progress` a disk snapshot in `state` reports.
///
/// DERIVED from the snapshot's own state: a snapshot Lightsail is still taking
/// has nothing captured yet as far as this store knows, and one that completed
/// has captured all of the source volume. The percentage form is the one the
/// EC2 snapshot `Progress` this member mirrors uses (`"100%"`); the modelled
/// `DiskSnapshot$progress` documentation says only "The progress of the
/// snapshot", so the FORM is emulator-chosen while the fact it reports is the
/// store's own state.
fn disk_snapshot_progress(state: &str) -> &'static str {
    if state == DISK_SNAPSHOT_STATE_COMPLETED {
        "100%"
    } else {
        "0%"
    }
}

/// One stored disk snapshot: every member `GetDiskSnapshot` reports, plus the
/// clock-owed transition its state is waiting on.
#[derive(Debug, Clone)]
pub struct StoredDiskSnapshot {
    /// The snapshot name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The size of the snapshotted volume, copied from the source at the instant
    /// the snapshot was taken — the size a disk created from this snapshot
    /// starts at, so it does not follow a later change to the source.
    pub size_in_gb: i32,
    /// The current `DiskSnapshotState` value: [`DISK_SNAPSHOT_STATE_PENDING`] or
    /// [`DISK_SNAPSHOT_STATE_COMPLETED`].
    pub state: &'static str,
    /// The state the clock owes this snapshot, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// What the snapshot was taken of.
    pub source: DiskSnapshotSource,
}

impl StoredDiskSnapshot {
    /// This record as the modelled `DiskSnapshot`.
    pub fn view(&self) -> crate::ops::types::DiskSnapshot {
        let (from_disk_name, from_disk_arn, from_instance_name, from_instance_arn) = match
            &self.source
        {
            DiskSnapshotSource::Disk { name, arn } => {
                (Some(name.clone()), Some(arn.clone()), None, None)
            }
            DiskSnapshotSource::Instance { name, arn } => {
                (None, None, Some(name.clone()), Some(arn.clone()))
            }
        };
        crate::ops::types::DiskSnapshot {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISK_SNAPSHOT.to_string()),
            tags: Some(self.tags.clone()),
            size_in_gb: Some(self.size_in_gb),
            state: Some(self.state.to_string()),
            progress: Some(disk_snapshot_progress(self.state).to_string()),
            from_disk_name,
            from_disk_arn,
            from_instance_name,
            from_instance_arn,
            // `CreateDiskSnapshot` is an explicit snapshot the caller asked
            // for, so it is not one the `AutoSnapshot` add-on took.
            is_from_auto_snapshot: Some(false),
        }
    }
}

/// One stored domain recordset: the DNS zone `CreateDomain` created, with the
/// entries the account has added to it.
#[derive(Debug, Clone)]
pub struct StoredDomain {
    /// The domain name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The recordsets held in this zone, newest last. A zone is created empty:
    /// `CreateDomainEntry` is what adds an entry to it.
    pub entries: Vec<DomainEntry>,
}

impl StoredDomain {
    /// This record as the modelled `Domain`.
    pub fn view(&self) -> Domain {
        Domain {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DOMAIN.to_string()),
            tags: Some(self.tags.clone()),
            domain_entries: Some(self.entries.clone()),
            // Nothing has delegated a Route 53 registered domain to this zone:
            // that delegation is made in Route 53, which this emulator does not
            // serve, so there is no state to report.
            registered_domain_delegation_info: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Managed relational databases
// ---------------------------------------------------------------------------

/// The `parameterApplyStatus` a database whose parameters are all applied
/// reports.
///
/// A Lightsail managed database is an Amazon RDS DB instance, and the member
/// carries RDS's own parameter group status: `in-sync` means "the parameters are
/// applied"; `UpdateRelationalDatabaseParameters` is what moves a database out of
/// it.
pub const PARAMETER_APPLY_STATUS_IN_SYNC: &str = "in-sync";

/// The certificate authority a new database's server certificate is issued
/// under, as `caCertificateIdentifier` reports it.
///
/// `rds-ca-rsa2048-g1` is the default certificate authority for a new DB
/// instance ("Using SSL/TLS to encrypt a connection to a DB instance", Amazon
/// RDS User Guide). It is a value AWS allocated — nothing computes it — so a
/// single published name is the only honest source; it moves when the published
/// default does.
pub const RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIER: &str = "rds-ca-rsa2048-g1";

/// The DNS suffix a Lightsail managed database's master endpoint carries.
///
/// A Lightsail managed database IS an Amazon RDS DB instance — which is why
/// every member of the modelled `RelationalDatabase` is an RDS one — and AWS
/// publishes its `masterEndpoint.address` under its own
/// `rds.amazonaws.com` zone. That HOSTNAME is environment-specific and no
/// caller may assume it, while the PROMISE the member makes is not:
/// `RelationalDatabaseEndpoint$address` is "the DNS address of the database",
/// so it must never be an address that sends the next client OUT of the
/// emulator — the rule this crate already states for `Bucket$url` and a
/// container service's public address. This emulator owns no DNS zone, so the
/// address keeps the RDS SHAPE (`<db instance identifier>.<host label>.<region>`)
/// under a suffix that belongs to the emulator and cannot resolve off-machine,
/// exactly as a container service's private domain name does
/// (`PRIVATE_DOMAIN_SUFFIX`, `<service name>.service.local`): `.local` is
/// reserved for link-local multicast name resolution (RFC 6762 §3), so a name
/// under it can never address a host off this machine. The labels are
/// the database's own, so two databases never share an address.
///
/// This emulator serves no MySQL or PostgreSQL wire protocol on it, because the
/// core's native listener (`engine.listeners().tcp`) is TLS-from-the-first-byte
/// and neither engine's protocol begins with a TLS handshake. Everything the
/// control plane owns — resolving the database, its state, its hardware, its
/// credentials and its windows — is real.
const RELATIONAL_DATABASE_ENDPOINT_SUFFIX: &str = "rds.lightsail.local";

/// The prefix of the RDS DB instance identifier Lightsail gives a managed
/// database: Lightsail names the underlying instance after itself rather than
/// after the Lightsail resource, which is why a master endpoint begins `ls-`
/// instead of with the database's own name.
const RELATIONAL_DATABASE_INSTANCE_ID_PREFIX: &str = "ls-";

/// How many characters the minted host label of a master endpoint carries: the
/// 12-character DNS label RDS puts between the instance identifier and the
/// region.
const RELATIONAL_DATABASE_HOST_LABEL_LEN: usize = 12;

/// The alphabet that host label is drawn from. A DNS label is
/// case-insensitive and RDS renders this one in lowercase.
const RELATIONAL_DATABASE_HOST_LABEL_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The port a database of `engine` listens on: the engine's own default
/// (MySQL `3306`, PostgreSQL `5432`), the ports RDS documents as the default for
/// each engine under `CreateDBInstance$Port`.
///
/// Lightsail exposes no port parameter at all, so a database always listens on
/// its engine's default.
pub fn relational_database_port(engine: &str) -> i32 {
    if engine.to_ascii_lowercase().contains("postgres") {
        5432
    } else {
        3306
    }
}

/// The ARN of the managed relational database `id` in `region`.
pub fn relational_database_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RELATIONAL_DATABASE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The master endpoint address of a database whose underlying DB instance was
/// minted as `id`, whose host label is `host_label`, in `region`. See
/// [`RELATIONAL_DATABASE_ENDPOINT_SUFFIX`].
pub fn relational_database_endpoint_address(id: &str, host_label: &str, region: &str) -> String {
    format!(
        "{RELATIONAL_DATABASE_INSTANCE_ID_PREFIX}{}.{host_label}.{region}.{RELATIONAL_DATABASE_ENDPOINT_SUFFIX}",
        id.replace('-', "")
    )
}

/// The Availability Zone a resource of `resource_name` is created in when the
/// request names none, out of the zones `zones` its Region publishes.
///
/// DERIVED from the resource's own name rather than drawn per call, so the zone
/// a resource was placed in is the same on every later read, and `None` when the
/// Region publishes no zone at all.
pub fn default_availability_zone(resource_name: &str, zones: &[String]) -> Option<String> {
    let digest = Sha256::digest(resource_name.as_bytes());
    let index = usize::from(digest[0]) % zones.len().max(1);
    zones.get(index).cloned()
}

/// The zone a high-availability resource's standby runs in: the zone after
/// `primary` in the Region's own zone order, wrapping — so it is never the
/// primary's own zone, which is the point of a standby.
///
/// `None` when the Region publishes fewer than two zones, and then there is no
/// second zone to fail over to.
pub fn secondary_availability_zone(primary: &str, zones: &[String]) -> Option<String> {
    if zones.len() < 2 {
        return None;
    }
    let primary_index = zones.iter().position(|zone| zone == primary)?;
    zones.get((primary_index + 1) % zones.len()).cloned()
}

/// The master-credential constraints one database engine imposes, from the
/// modelled `CreateRelationalDatabase` input documentation, which states them per
/// engine: `masterDatabaseName` ("Must contain 1 to 64 letters or numbers" for
/// MySQL, 1 to 63 for PostgreSQL), `masterUsername` ("Must be 1 to 16 letters or
/// numbers" for MySQL, 1 to 63 for PostgreSQL) and `masterUserPassword` ("Must
/// contain from 8 to 41 characters" for MySQL, from 8 to 128 for PostgreSQL).
#[derive(Debug, Clone, Copy)]
pub struct MasterCredentialLimits {
    /// The most characters `masterDatabaseName` may carry.
    pub database_name_chars: usize,
    /// The most characters `masterUsername` may carry.
    pub username_chars: usize,
    /// The fewest and the most characters `masterUserPassword` may carry.
    pub password_chars: (usize, usize),
}

/// The limits [`MasterCredentialLimits`] states for `engine`.
///
/// The engine of a blueprint is the catalog's own
/// [`RelationalDatabaseBlueprintSpec::engine`] value, so the PostgreSQL set is
/// selected on that engine and every other engine the catalog serves is held to
/// the MySQL set the modelled documentation gives first.
pub fn master_credential_limits(engine: &str) -> MasterCredentialLimits {
    if engine.to_ascii_lowercase().contains("postgres") {
        MasterCredentialLimits {
            database_name_chars: 63,
            username_chars: 63,
            password_chars: (8, 128),
        }
    } else {
        MasterCredentialLimits {
            database_name_chars: 64,
            username_chars: 16,
            password_chars: (8, 41),
        }
    }
}

/// Refuse `value` unless it is an identifier a database engine accepts for
/// `member`: 1 to `max_chars` characters, beginning with a letter, the rest
/// letters, digits or underscores.
///
/// Both `masterDatabaseName` and `masterUsername` carry that same rule in the
/// modelled documentation ("Must begin with a letter. Subsequent characters can
/// be letters, underscores, or digits (0-9)"), differing only in the length each
/// engine allows. The engine's own reserved-word list is NOT checked here: it is
/// the engine's, not the service's, and this slice carries no copy of it.
pub fn validate_engine_identifier(
    member: &str,
    value: &str,
    max_chars: usize,
) -> Result<(), String> {
    let length = value.chars().count();
    if length == 0 || length > max_chars {
        return Err(format!(
            "{member} carries {length} characters, outside the 1 to {max_chars} this database \
             engine accepts"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
    {
        return Err(format!("{member} '{value}' must begin with a letter"));
    }
    if let Some(bad) = value
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || *character == '_'))
    {
        return Err(format!(
            "{member} '{value}' carries '{bad}': after the first character only letters, digits \
             and underscores are accepted"
        ));
    }
    Ok(())
}

/// Refuse `password` unless it is a master user password this service accepts:
/// `min` to `max` characters of printable ASCII, none of them a space, a `/`, a
/// `"` or an `@`.
///
/// From the modelled `masterUserPassword` documentation: "The password can
/// include any printable ASCII character except "/", """, or "@". It cannot
/// contain spaces", with the length bounds [`MasterCredentialLimits`] carries
/// per engine.
pub fn validate_master_user_password(
    password: &str,
    (min, max): (usize, usize),
) -> Result<(), String> {
    let length = password.chars().count();
    if length < min || length > max {
        return Err(format!(
            "masterUserPassword carries {length} characters, outside the {min} to {max} this \
             database engine accepts"
        ));
    }
    if let Some(bad) = password.chars().find(|character| {
        !character.is_ascii_graphic() || matches!(character, '/' | '"' | '@')
    }) {
        return Err(format!(
            "masterUserPassword carries '{bad}': a password carries printable ASCII characters \
             other than a space, '/', '\"' and '@'"
        ));
    }
    Ok(())
}

/// Minutes in a day, and in a week: the clock a backup (daily) and a
/// maintenance (weekly) window are measured on.
const MINUTES_PER_DAY: i64 = 24 * 60;
/// See [`MINUTES_PER_DAY`].
const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;

/// The fewest minutes a backup or maintenance window may span, from the modelled
/// `preferredBackupWindow` and `preferredMaintenanceWindow` documentation: "Must
/// be at least 30 minutes."
pub const MIN_WINDOW_MINUTES: i64 = 30;

/// The days a weekly maintenance window may name, from the modelled
/// `preferredMaintenanceWindow` documentation: "Valid days: Mon, Tue, Wed, Thu,
/// Fri, Sat, Sun." Index 0 is Monday, the first day that list names.
pub const MAINTENANCE_WINDOW_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// `hh24:mi` as a minute of the day, or `None` when it is not that form.
fn minute_of_day(time: &str) -> Option<i64> {
    let (hours, minutes) = time.split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(hours * 60 + minutes)
}

/// A minute of the day as `hh24:mi`.
fn render_time(minute_of_day: i64) -> String {
    let minute_of_day = minute_of_day.rem_euclid(MINUTES_PER_DAY);
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

/// The day index `day` names in [`MAINTENANCE_WINDOW_DAYS`], compared without
/// regard to case (the days are a presentation form, not an identifier).
fn day_index(day: &str) -> Option<i64> {
    MAINTENANCE_WINDOW_DAYS
        .iter()
        .position(|name| name.eq_ignore_ascii_case(day))
        .and_then(|index| i64::try_from(index).ok())
}

/// One window as the half-open minute interval `[start, start + span)` on its
/// own clock: minutes of the day for a daily window, minutes of the week for a
/// weekly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSpan {
    /// The minute the window opens.
    pub start: i64,
    /// How many minutes it spans.
    pub span: i64,
}

/// The daily window `window` describes, or a refusal naming `member` and the
/// constraint it broke.
///
/// From the modelled `preferredBackupWindow` documentation: "Must be in the
/// hh24:mi-hh24:mi format. Example: 16:00-16:30 … Must be at least 30 minutes."
/// A window whose end precedes its start runs over midnight, which is why the
/// span is taken on the daily clock.
pub fn parse_daily_window(member: &str, window: &str) -> Result<WindowSpan, String> {
    let (start, end) = window.split_once('-').ok_or_else(|| {
        format!("{member} '{window}' is not in the hh24:mi-hh24:mi format (16:00-16:30)")
    })?;
    let start = minute_of_day(start).ok_or_else(|| {
        format!("{member} '{window}' is not in the hh24:mi-hh24:mi format (16:00-16:30)")
    })?;
    let end = minute_of_day(end).ok_or_else(|| {
        format!("{member} '{window}' is not in the hh24:mi-hh24:mi format (16:00-16:30)")
    })?;
    let span = (end - start).rem_euclid(MINUTES_PER_DAY);
    if span < MIN_WINDOW_MINUTES {
        return Err(format!(
            "{member} '{window}' spans {span} minutes, fewer than the {MIN_WINDOW_MINUTES} a \
             window must be"
        ));
    }
    Ok(WindowSpan { start, span })
}

/// The weekly window `window` describes, or a refusal naming `member` and the
/// constraint it broke.
///
/// From the modelled `preferredMaintenanceWindow` documentation: "Must be in the
/// ddd:hh24:mi-ddd:hh24:mi format. Valid days: Mon, Tue, Wed, Thu, Fri, Sat,
/// Sun. Must be at least 30 minutes. … Example: Tue:17:00-Tue:17:30".
pub fn parse_weekly_window(member: &str, window: &str) -> Result<WindowSpan, String> {
    let malformed = || {
        format!(
            "{member} '{window}' is not in the ddd:hh24:mi-ddd:hh24:mi format \
             (Tue:17:00-Tue:17:30), with a day of [{}]",
            MAINTENANCE_WINDOW_DAYS.join(", ")
        )
    };
    let (start, end) = window.split_once('-').ok_or_else(malformed)?;
    let minute_of_week = |point: &str| -> Option<i64> {
        let (day, time) = point.split_once(':')?;
        Some(day_index(day)? * MINUTES_PER_DAY + minute_of_day(time)?)
    };
    let start = minute_of_week(start).ok_or_else(malformed)?;
    let end = minute_of_week(end).ok_or_else(malformed)?;
    let span = (end - start).rem_euclid(MINUTES_PER_WEEK);
    if span < MIN_WINDOW_MINUTES {
        return Err(format!(
            "{member} '{window}' spans {span} minutes, fewer than the {MIN_WINDOW_MINUTES} a \
             window must be"
        ));
    }
    Ok(WindowSpan { start, span })
}

/// The half-open minute ranges `window` covers on the daily clock, split where
/// it runs over midnight so two windows can be compared range by range.
///
/// A window of at most a day covers at most two ranges, so the pair is returned
/// as a fixed-size array — the second entry empty (`None`) for a window that
/// does not cross midnight — rather than as an allocation.
fn daily_ranges(window: WindowSpan) -> [Option<(i64, i64)>; 2] {
    let start = window.start.rem_euclid(MINUTES_PER_DAY);
    let span = window.span.min(MINUTES_PER_DAY);
    if start + span <= MINUTES_PER_DAY {
        [Some((start, start + span)), None]
    } else {
        [
            Some((start, MINUTES_PER_DAY)),
            Some((0, start + span - MINUTES_PER_DAY)),
        ]
    }
}

/// Whether the backup window `backup` and the maintenance window `maintenance`
/// conflict, which the modelled `preferredBackupWindow` documentation forbids
/// ("Must not conflict with the preferred maintenance window").
///
/// A backup window recurs EVERY day and a maintenance window on one day of the
/// week, so they conflict when they overlap in time of day — whichever day the
/// maintenance window names.
pub fn windows_conflict(backup: WindowSpan, maintenance: WindowSpan) -> bool {
    let maintenance = WindowSpan {
        start: maintenance.start.rem_euclid(MINUTES_PER_DAY),
        span: maintenance.span,
    };
    let maintenance_ranges = daily_ranges(maintenance);
    daily_ranges(backup)
        .into_iter()
        .flatten()
        .any(|(backup_lo, backup_hi)| {
            maintenance_ranges
                .into_iter()
                .flatten()
                .any(|(lo, hi)| backup_lo < hi && lo < backup_hi)
        })
}

/// How many 30-minute slots a day holds, the candidates a defaulted window is
/// drawn from.
const WINDOW_SLOTS_PER_DAY: i64 = MINUTES_PER_DAY / MIN_WINDOW_MINUTES;

/// How many slots the 8-hour block a defaulted window is selected from holds.
const WINDOW_SLOTS_PER_BLOCK: i64 = 8 * 60 / MIN_WINDOW_MINUTES;

/// The daily backup window a database gets when the request names none, as
/// `hh24:mi-hh24:mi`.
///
/// From the modelled `preferredBackupWindow` documentation: "The default is a
/// 30-minute window selected at random from an 8-hour block of time for each AWS
/// Region." The slot is DERIVED from the database's own name rather than drawn
/// per call, so every later read of the database reports the same window, and it
/// is taken from the first 8-hour block of the day so the defaults of the two
/// windows cannot conflict. When `avoid` is a maintenance window the caller
/// named, the slot walks forward in 30-minute steps until it clears that window;
/// `None` when the caller's window leaves no slot free at all.
pub fn default_preferred_backup_window(
    resource_name: &str,
    avoid: Option<WindowSpan>,
) -> Option<String> {
    let digest = Sha256::digest(resource_name.as_bytes());
    let slot = i64::from(digest[1]) % WINDOW_SLOTS_PER_BLOCK;
    (0..WINDOW_SLOTS_PER_DAY).find_map(|step| {
        let start = (slot + step).rem_euclid(WINDOW_SLOTS_PER_DAY) * MIN_WINDOW_MINUTES;
        let candidate = WindowSpan {
            start,
            span: MIN_WINDOW_MINUTES,
        };
        let clear = avoid.is_none_or(|avoid| !windows_conflict(candidate, avoid));
        clear.then(|| {
            format!(
                "{}-{}",
                render_time(start),
                render_time(start + MIN_WINDOW_MINUTES)
            )
        })
    })
}

/// The weekly maintenance window a database gets when the request names none, as
/// `ddd:hh24:mi-ddd:hh24:mi`.
///
/// From the modelled `preferredMaintenanceWindow` documentation: "The default is
/// a 30-minute window selected at random from an 8-hour block of time for each
/// AWS Region, occurring on a random day of the week." Both the day and the slot
/// are DERIVED from the database's own name, so every later read reports the same
/// window; the slot is taken from the second 8-hour block of the day and then
/// walks forward in 30-minute steps until it clears `backup`, because the two
/// windows may not conflict. `None` when the backup window leaves no slot free at
/// all.
pub fn default_preferred_maintenance_window(
    resource_name: &str,
    backup: WindowSpan,
) -> Option<String> {
    let digest = Sha256::digest(resource_name.as_bytes());
    let day = i64::from(digest[2]) % 7;
    let slot = WINDOW_SLOTS_PER_BLOCK + i64::from(digest[3]) % WINDOW_SLOTS_PER_BLOCK;
    (0..WINDOW_SLOTS_PER_DAY).find_map(|step| {
        let start = (slot + step).rem_euclid(WINDOW_SLOTS_PER_DAY) * MIN_WINDOW_MINUTES;
        let candidate = WindowSpan {
            start,
            span: MIN_WINDOW_MINUTES,
        };
        let clear = !windows_conflict(backup, candidate);
        clear.then(|| {
            let day = MAINTENANCE_WINDOW_DAYS[usize::try_from(day).unwrap_or(0)];
            // A 30-minute window never crosses midnight from this block, so both
            // ends name the same day.
            format!(
                "{day}:{}-{day}:{}",
                render_time(start),
                render_time(start + MIN_WINDOW_MINUTES)
            )
        })
    })
}

/// One stored managed relational database: every member `GetRelationalDatabase`
/// reports, the master user password `GetRelationalDatabaseMasterUserPassword`
/// answers with, and the clock-owed transition its state is waiting on.
#[derive(Debug, Clone)]
pub struct StoredRelationalDatabase {
    /// The database name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The blueprint the request named, resolved against the published catalog:
    /// the engine and the version every read reports come from it.
    pub blueprint: RelationalDatabaseBlueprintSpec,
    /// The bundle the request named, resolved against the published catalog: the
    /// hardware every read reports comes from it.
    pub bundle: RelationalDatabaseBundleSpec,
    pub master_database_name: String,
    pub master_username: String,
    /// The master user password the request carried, kept because
    /// `GetRelationalDatabaseMasterUserPassword` answers with it (its own
    /// operation, with its own issue); no member of `RelationalDatabase` exposes
    /// it. Absent when the request carried none.
    pub master_user_password: Option<String>,
    pub preferred_backup_window: String,
    pub preferred_maintenance_window: String,
    pub publicly_accessible: bool,
    pub backup_retention_enabled: bool,
    /// The standby's Availability Zone, on a high-availability bundle only: "The
    /// secondary database is used for failover support of a high availability
    /// database."
    pub secondary_availability_zone: Option<String>,
    /// The master endpoint address, assigned at creation so every later read
    /// reports the same one. See [`RELATIONAL_DATABASE_ENDPOINT_SUFFIX`].
    pub endpoint_address: String,
    /// The current state: [`RELATIONAL_DATABASE_STATE_CREATING`] or
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`].
    pub state: &'static str,
    /// The state the clock owes this database, while one is outstanding.
    pub due: Option<DueNamedState>,
}

impl StoredRelationalDatabase {
    /// This record as the modelled `RelationalDatabase`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabase {
        crate::ops::types::RelationalDatabase {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_RELATIONAL_DATABASE.to_string()),
            tags: Some(self.tags.clone()),
            relational_database_blueprint_id: Some(self.blueprint.blueprint_id()),
            relational_database_bundle_id: Some(self.bundle.bundle_id.to_string()),
            master_database_name: Some(self.master_database_name.clone()),
            hardware: Some(crate::ops::types::RelationalDatabaseHardware {
                cpu_count: Some(self.bundle.cpu_count),
                disk_size_in_gb: Some(self.bundle.disk_size_in_gb),
                ram_size_in_gb: Some(self.bundle.ram_size_in_gb),
            }),
            state: Some(self.state.to_string()),
            secondary_availability_zone: self.secondary_availability_zone.clone(),
            backup_retention_enabled: Some(self.backup_retention_enabled),
            // Nothing is mid-modification on a database this slice created:
            // `UpdateRelationalDatabase` is what leaves a value pending, so there
            // is nothing to report rather than an empty structure.
            pending_modified_values: None,
            engine: Some(self.blueprint.engine.to_string()),
            engine_version: Some(self.blueprint.engine_version.to_string()),
            // "The latest point in time to which the database can be restored":
            // no automated backup has been taken yet — the first one is taken in
            // the database's own backup window — so there is no such point.
            latest_restorable_time: None,
            master_username: Some(self.master_username.clone()),
            parameter_apply_status: Some(PARAMETER_APPLY_STATUS_IN_SYNC.to_string()),
            preferred_backup_window: Some(self.preferred_backup_window.clone()),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            publicly_accessible: Some(self.publicly_accessible),
            // The endpoint is announced once the database is serving: a database
            // still being created accepts no connection, and reports no address
            // to make one to.
            master_endpoint: (self.state == RELATIONAL_DATABASE_STATE_AVAILABLE).then(|| {
                crate::ops::types::RelationalDatabaseEndpoint {
                    port: Some(relational_database_port(self.blueprint.engine)),
                    address: Some(self.endpoint_address.clone()),
                }
            }),
            // A list member is reported EMPTY rather than left out: no
            // maintenance action is pending on a database this slice holds, and
            // an absent member hands a typed caller a null where it iterates.
            pending_maintenance_actions: Some(Vec::new()),
            ca_certificate_identifier: Some(
                RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIER.to_string(),
            ),
        }
    }
}

/// One entry of the operation log: what a mutating call did, and when.
#[derive(Debug, Clone)]
pub struct StoredOperation {
    pub id: String,
    pub resource_name: String,
    pub resource_type: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub is_terminal: bool,
    pub operation_details: Option<String>,
    pub operation_type: String,
    pub status: String,
    pub status_changed_at_ms: i64,
    pub error_code: Option<String>,
    pub error_details: Option<String>,
    /// When the clock owes this operation its terminal `Succeeded` status, while
    /// one is outstanding.
    pub settles_at_ms: Option<i64>,
}

impl StoredOperation {
    /// This record as the modelled `Operation`.
    pub fn view(&self) -> Operation {
        Operation {
            id: Some(self.id.clone()),
            resource_name: Some(self.resource_name.clone()),
            resource_type: Some(self.resource_type.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            is_terminal: Some(self.is_terminal),
            operation_details: self.operation_details.clone(),
            operation_type: Some(self.operation_type.clone()),
            status: Some(self.status.clone()),
            status_changed_at: Some(timestamp(self.status_changed_at_ms)),
            error_code: self.error_code.clone(),
            error_details: self.error_details.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Container services
// ---------------------------------------------------------------------------

/// `ContainerServiceState` values this slice uses, from the modelled enum and the
/// `ContainerService$state` documentation: `PENDING` while the service is being
/// created, `READY` once it runs without an active deployment, `DEPLOYING` while
/// it launches one, and `RUNNING` once a deployment is active.
pub const CONTAINER_SERVICE_PENDING: &str = "PENDING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_READY: &str = "READY";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_DEPLOYING: &str = "DEPLOYING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_RUNNING: &str = "RUNNING";

/// `ContainerServiceDeploymentState` values this slice uses, from the modelled
/// enum and the `ContainerServiceDeployment$state` documentation: `ACTIVATING`
/// while the deployment is being created, `ACTIVE` once it is running (at most
/// one at a time) and `INACTIVE` for one that ran before.
pub const DEPLOYMENT_STATE_ACTIVATING: &str = "ACTIVATING";
/// See [`DEPLOYMENT_STATE_ACTIVATING`].
pub const DEPLOYMENT_STATE_ACTIVE: &str = "ACTIVE";
/// See [`DEPLOYMENT_STATE_ACTIVATING`].
pub const DEPLOYMENT_STATE_INACTIVE: &str = "INACTIVE";

/// The `ContainerServiceStateDetail$code` reported while a deployment is being
/// created, from that member's own documentation: `CREATING_DEPLOYMENT` is one of
/// the codes "possible if your container service is in a `DEPLOYING` or
/// `UPDATING` state".
pub const STATE_DETAIL_CREATING_DEPLOYMENT: &str = "CREATING_DEPLOYMENT";

/// The most container services this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. A create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_CONTAINER_SERVICES: usize = 100;

/// The most public domain names one container service may carry.
///
/// From the modelled `publicDomainNames` documentation: "You can specify up to
/// four public domain names for a container service."
pub const MAX_PUBLIC_DOMAIN_NAMES: usize = 4;

/// The most containers one deployment may describe.
///
/// Emulator resource guard: the modelled `ContainerMap` declares no length, so
/// this is where the bound on a caller-sized map lives.
pub const MAX_CONTAINERS_PER_DEPLOYMENT: usize = 10;

/// The hardware and price one container service power provides.
///
/// The figures are the published Lightsail container service plans the modelled
/// `power` member points `GetContainerServicePowers` at: each step doubles RAM
/// and the base monthly price, and the vCPU count follows the plan table. The
/// `power_id` form is the one the modelled `ContainerServicePower$powerId`
/// documentation shows (`nano-1`).
#[derive(Debug, Clone, Copy)]
pub struct ContainerPowerSpec {
    /// The modelled `ContainerServicePowerName` value that selects this power.
    pub name: &'static str,
    pub power_id: &'static str,
    /// `ContainerServicePower$price`, the base monthly price in USD.
    pub price: f32,
    pub cpu_count: f32,
    pub ram_size_in_gb: f32,
}

/// Every container service power this slice serves. See [`ContainerPowerSpec`]
/// for the source.
pub const CONTAINER_SERVICE_POWERS: &[ContainerPowerSpec] = &[
    ContainerPowerSpec {
        name: "nano",
        power_id: "nano-1",
        price: 7.0,
        cpu_count: 0.25,
        ram_size_in_gb: 0.5,
    },
    ContainerPowerSpec {
        name: "micro",
        power_id: "micro-1",
        price: 10.0,
        cpu_count: 0.25,
        ram_size_in_gb: 1.0,
    },
    ContainerPowerSpec {
        name: "small",
        power_id: "small-1",
        price: 20.0,
        cpu_count: 0.5,
        ram_size_in_gb: 2.0,
    },
    ContainerPowerSpec {
        name: "medium",
        power_id: "medium-1",
        price: 40.0,
        cpu_count: 1.0,
        ram_size_in_gb: 4.0,
    },
    ContainerPowerSpec {
        name: "large",
        power_id: "large-1",
        price: 80.0,
        cpu_count: 2.0,
        ram_size_in_gb: 8.0,
    },
    ContainerPowerSpec {
        name: "xlarge",
        power_id: "xlarge-1",
        price: 160.0,
        cpu_count: 4.0,
        ram_size_in_gb: 16.0,
    },
];

/// The power `power` names, or `None` when it is not one this service serves.
pub fn container_service_power(power: &str) -> Option<ContainerPowerSpec> {
    CONTAINER_SERVICE_POWERS
        .iter()
        .find(|spec| spec.name == power)
        .copied()
}

/// The power names [`container_service_power`] accepts, for a refusal that has to
/// tell the caller what to send instead.
pub fn container_service_power_names() -> String {
    CONTAINER_SERVICE_POWERS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Documented defaults and bounds of a public endpoint's health check, each from
/// its own member's documentation on `ContainerServiceHealthCheckConfig`:
/// "The default value is `2`" (healthy and unhealthy thresholds, and the
/// timeout), "The default value is `5`" (the interval), "The default value is
/// `/`" (the path), and the success codes "between `200` and `499`".
const HEALTH_CHECK_DEFAULT_THRESHOLD: i32 = 2;
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
const HEALTH_CHECK_DEFAULT_TIMEOUT_SECONDS: i32 = 2;
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
const HEALTH_CHECK_DEFAULT_INTERVAL_SECONDS: i32 = 5;
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
const HEALTH_CHECK_DEFAULT_PATH: &str = "/";
/// See [`HEALTH_CHECK_DEFAULT_THRESHOLD`].
const HEALTH_CHECK_DEFAULT_SUCCESS_CODES: &str = "200-499";
/// "You can specify between 2 and 60 seconds" (`timeoutSeconds`).
const HEALTH_CHECK_TIMEOUT_SECONDS_RANGE: (i32, i32) = (2, 60);
/// "You can specify between 5 and 300 seconds" (`intervalSeconds`).
const HEALTH_CHECK_INTERVAL_SECONDS_RANGE: (i32, i32) = (5, 300);

/// How many characters the per-service token of a container service's public
/// address carries, from the example in the modelled `serviceName`
/// documentation (`https://container-service-1.ur4EXAMPLE2uq.us-east-2.cs.amazonlightsail.com`).
const DOMAIN_TOKEN_LEN: usize = 13;

/// The alphabet that token is drawn from: the mixed-case alphanumerics the same
/// example shows.
const DOMAIN_TOKEN_ALPHABET: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The path space every container service's public address is published under,
/// beneath the endpoint the emulator is reached on.
///
/// AWS publishes that address as a DNS name under its own
/// `cs.amazonlightsail.com` zone, and the modelled `serviceName` documentation
/// gives the textual form `https://<service>.<token>.<region>.cs.amazonlightsail.com`.
/// The HOSTNAME is environment-specific — no caller may assume it — while the
/// PROMISE the field makes is not: `ContainerService$url` is "The publicly
/// accessible URL of the container service", so it has to be an address the
/// caller can actually reach. This emulator owns no DNS zone, so the only
/// address it can be reached on is the endpoint the request arrived at, and the
/// three labels AWS puts in the hostname (region, service name, per-service
/// token) are carried in the path instead. The prefix names this service and
/// this resource type, so it cannot collide with another service's modelled URI
/// space.
pub const CONTAINER_SERVICE_PUBLIC_PATH_PREFIX: &str = "/lightsail-container-service";

/// The suffix of a container service's private domain name, which the Lightsail
/// container services guide gives as `<service name>.service.local` \u2014 resolvable
/// only inside the account's default VPC, as the modelled `privateDomainName`
/// documentation says.
const PRIVATE_DOMAIN_SUFFIX: &str = "service.local";

/// The ARN of the container service `id` in `region`.
pub fn container_service_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONTAINER_SERVICE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The publicly accessible URL of the container service `name` in `region`,
/// whose public address carries `token`, as announced to a caller that reached
/// this emulator at `endpoint` (its `Host`/`:authority`).
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on. See
/// [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`] for why the address is a path under
/// the request's own endpoint rather than an AWS hostname.
pub fn container_service_url(endpoint: &str, region: &str, name: &str, token: &str) -> String {
    format!("https://{endpoint}{CONTAINER_SERVICE_PUBLIC_PATH_PREFIX}/{region}/{name}/{token}")
}

/// Which container service a request to a published public URL addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerServicePublicTarget {
    pub region: String,
    pub name: String,
    /// The per-service token the announced URL carried, which must be the one
    /// that service was minted with: a URL is addressed to ONE service.
    pub domain_token: String,
}

/// The container service a public-URL request path addresses, or `None` when the
/// path is not one [`container_service_url`] publishes.
///
/// Anything the caller appended BENEATH those three segments is a path within the
/// deployed container: it names no part of this service, and nothing here routes
/// it (no container runs), so it identifies the same service as the announced URL
/// itself and is not carried.
pub fn parse_container_service_public_path(path: &str) -> Option<ContainerServicePublicTarget> {
    let rest = path
        .strip_prefix(CONTAINER_SERVICE_PUBLIC_PATH_PREFIX)?
        .strip_prefix('/')?;
    let mut segments = rest.splitn(4, '/');
    let (Some(region), Some(name), Some(token)) =
        (segments.next(), segments.next(), segments.next())
    else {
        return None;
    };
    if [region, name, token].iter().any(|segment| segment.is_empty()) {
        return None;
    }
    Some(ContainerServicePublicTarget {
        region: region.to_string(),
        name: name.to_string(),
        domain_token: token.to_string(),
    })
}

/// The private domain name of the container service `name`. See
/// [`PRIVATE_DOMAIN_SUFFIX`].
pub fn container_service_private_domain_name(name: &str) -> String {
    format!("{name}.{PRIVATE_DOMAIN_SUFFIX}")
}

/// The principal ARN of the container service `name` in `region`, and the ARN of
/// the role it pulls private container images with.
///
/// The ARN's PATH is emulator-chosen: AWS documents what the principal ARN is
/// FOR (a trust relationship between the caller's own account and the service)
/// but not how its string is built. It is an IAM role ARN carrying the service's
/// region, name and its own domain token, so two services never share one and a
/// value is stable across reads.
pub fn container_service_principal_arn(region: &str, name: &str, token: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/amazon/lightsail/{region}/containers/{name}/{token}"),
    )
}

/// See [`container_service_principal_arn`]: the same role, suffixed so the ECR
/// image puller principal is distinguishable from the service's own.
pub fn ecr_image_puller_principal_arn(region: &str, name: &str, token: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/amazon/lightsail/{region}/containers/{name}/{token}/ecr-image-puller"),
    )
}

/// One container image registered to a container service: the digest that was
/// pushed, the name this service assigned it, and when it was registered.
///
/// The assigned name has the form the modelled
/// `RegisterContainerImageRequest$label` documentation gives — "if the name of
/// your container service is `mycontainerservice`, and the label that you specify
/// is `mystaticwebsite`, then the name of the registered container image will be
/// `:mycontainerservice.mystaticwebsite.1`", the trailing number being the
/// version of the images sharing that label.
#[derive(Debug, Clone)]
pub struct StoredContainerImage {
    /// The name the service assigned the image.
    pub image: String,
    /// The digest of the pushed image, kept verbatim.
    pub digest: String,
    pub created_at_ms: i64,
}

impl StoredContainerImage {
    /// This record as the modelled `ContainerImage`.
    pub fn view(&self) -> ContainerImage {
        ContainerImage {
            image: Some(self.image.clone()),
            digest: Some(self.digest.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
        }
    }
}

/// One stored deployment of a container service: the containers it launches, the
/// public endpoint that fronts them, and where it is in its own lifecycle.
#[derive(Debug, Clone)]
pub struct StoredDeployment {
    /// 1 for a service's first deployment, incremented per deployment after.
    pub version: i32,
    pub state: &'static str,
    /// A `BTreeMap` so the containers are stored in one stable order, whatever
    /// order the request's map arrived in.
    pub containers: BTreeMap<String, Container>,
    pub public_endpoint: Option<ContainerServiceEndpoint>,
    pub created_at_ms: i64,
}

impl StoredDeployment {
    /// This record as the modelled `ContainerServiceDeployment`.
    pub fn view(&self) -> ContainerServiceDeployment {
        ContainerServiceDeployment {
            version: Some(self.version),
            state: Some(self.state.to_string()),
            containers: Some(
                self.containers
                    .iter()
                    .map(|(name, container)| (name.clone(), container.clone()))
                    .collect::<HashMap<String, Container>>(),
            ),
            public_endpoint: self.public_endpoint.clone(),
            created_at: Some(timestamp(self.created_at_ms)),
        }
    }
}

/// A phase change the clock owes a container service: the state it settles into,
/// and whether reaching it makes the waiting deployment the active one.
#[derive(Debug, Clone, Copy)]
pub struct OwedPhase {
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    pub state: &'static str,
    pub activates_next_deployment: bool,
}

/// One stored container service: what the caller asked for (name, power, scale,
/// tags, public domain names, private registry access and the deployment, if
/// any) plus what the service assigns (ARN, creation time, default domain,
/// principal ARNs and the lifecycle it is walking).
#[derive(Debug, Clone)]
pub struct StoredContainerService {
    pub name: String,
    pub arn: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub power: String,
    pub power_id: String,
    pub scale: i32,
    pub state: &'static str,
    pub is_disabled: bool,
    /// The per-service token in the service's public address, minted once so
    /// every read reports the same URL.
    pub domain_token: String,
    pub principal_arn: String,
    /// A `BTreeMap` so the certificate/domain pairs are stored in one stable
    /// order.
    pub public_domain_names: BTreeMap<String, Vec<String>>,
    pub ecr_image_puller_role_active: bool,
    pub current_deployment: Option<StoredDeployment>,
    pub next_deployment: Option<StoredDeployment>,
    /// The version the NEXT deployment of this service will carry.
    pub next_deployment_version: i32,
    /// The container images registered to this service, in registration order —
    /// the service's own registry, which `GetContainerImages` reports.
    ///
    /// A `Vec` and not a map: the modelled `ContainerImageList` documents no
    /// order, so registration order is the one stable total order the store has,
    /// and the assigned name already keys each entry uniquely.
    ///
    /// It stays EMPTY for as long as this emulator serves no container image
    /// registry to push to: an image is registered only after a `docker push`
    /// puts it in the service's private registry, which is why
    /// `RegisterContainerImage` refuses every digest (see that handler). Whoever
    /// serves that registry fills this field.
    pub images: Vec<StoredContainerImage>,
    /// The phases the clock still owes this service, soonest first.
    pub owed: VecDeque<OwedPhase>,
}

impl StoredContainerService {
    /// This record as the modelled `ContainerService`, announced to a caller that
    /// reached this emulator at `endpoint` (its `Host`/`:authority`, as
    /// [`request_endpoint`] reads it).
    ///
    /// `url` is the one member that depends on the request: it is the address the
    /// caller is told to reach this service's public endpoint on, so it has to be
    /// the endpoint THIS request arrived at (see
    /// [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`]). A request that names no host at
    /// all names no endpoint to announce, so `url` is omitted rather than filled
    /// with an address nothing serves.
    pub fn view(&self, endpoint: Option<&str>) -> ContainerService {
        ContainerService {
            container_service_name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_CONTAINER_SERVICE.to_string()),
            tags: Some(self.tags.clone()),
            power: Some(self.power.clone()),
            power_id: Some(self.power_id.clone()),
            state: Some(self.state.to_string()),
            state_detail: self.state_detail(),
            scale: Some(self.scale),
            current_deployment: self.current_deployment.as_ref().map(StoredDeployment::view),
            next_deployment: self.next_deployment.as_ref().map(StoredDeployment::view),
            is_disabled: Some(self.is_disabled),
            principal_arn: Some(self.principal_arn.clone()),
            private_domain_name: Some(container_service_private_domain_name(&self.name)),
            public_domain_names: Some(
                self.public_domain_names
                    .iter()
                    .map(|(certificate, domains)| (certificate.clone(), domains.clone()))
                    .collect::<HashMap<String, Vec<String>>>(),
            ),
            url: endpoint.map(|endpoint| {
                container_service_url(
                    endpoint,
                    &self.location.region_name,
                    &self.name,
                    &self.domain_token,
                )
            }),
            private_registry_access: Some(PrivateRegistryAccess {
                ecr_image_puller_role: Some(ContainerServiceECRImagePullerRole {
                    is_active: Some(self.ecr_image_puller_role_active),
                    // The role has a principal only while it is active: an
                    // inactive role grants nothing, so there is nothing for a
                    // repository policy to name.
                    principal_arn: self.ecr_image_puller_role_active.then(|| {
                        ecr_image_puller_principal_arn(
                            &self.location.region_name,
                            &self.name,
                            &self.domain_token,
                        )
                    }),
                }),
            }),
        }
    }

    /// The service's `stateDetail`, present only while a deployment is being
    /// created.
    ///
    /// The modelled `ContainerServiceStateDetail$code` documentation lists
    /// progress codes only for a `DEPLOYING` or `UPDATING` service, and reserves
    /// the two codes of a `PENDING` one for failures; nothing has failed here, so
    /// every other state reports no detail at all rather than an invented one.
    fn state_detail(&self) -> Option<ContainerServiceStateDetail> {
        if self.state != CONTAINER_SERVICE_DEPLOYING {
            return None;
        }
        let version = self.next_deployment.as_ref().map(|next| next.version);
        Some(ContainerServiceStateDetail {
            code: Some(STATE_DETAIL_CREATING_DEPLOYMENT.to_string()),
            message: Some(match version {
                Some(version) => format!("Creating deployment version {version}"),
                None => "Creating deployment".to_string(),
            }),
        })
    }

    /// Materialise every phase the clock had already made due at `now_ms`.
    ///
    /// Bounded by the number of phases a create enqueues, and the service drops
    /// out of the work as soon as it has none outstanding.
    fn advance(&mut self, now_ms: i64) {
        while let Some(phase) = self.owed.front().copied() {
            if phase.at_ms > now_ms {
                break;
            }
            self.owed.pop_front();
            self.state = phase.state;
            if !phase.activates_next_deployment {
                continue;
            }
            if let Some(mut next) = self.next_deployment.take() {
                next.state = DEPLOYMENT_STATE_ACTIVE;
                // "The container service can have only one deployment in an
                // active state at a time": whatever was running steps down.
                if let Some(mut previous) = self.current_deployment.replace(next) {
                    previous.state = DEPLOYMENT_STATE_INACTIVE;
                }
            }
        }
    }
}

/// The deployment `request` describes, at `version`, or a refusal naming the
/// member that is wrong.
///
/// A deployment launches containers, so it must name at least one, and each must
/// name the image it runs. A public endpoint must name one of those containers,
/// on a port that container opens, and its health check is filled in with the
/// documented defaults for whatever the request left out.
pub fn deployment_from_request(
    version: i32,
    now_ms: i64,
    request: &ContainerServiceDeploymentRequest,
) -> Result<StoredDeployment, String> {
    let containers: BTreeMap<String, Container> = request
        .containers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    if containers.is_empty() {
        return Err(
            "deployment.containers must describe at least one container to launch".to_string(),
        );
    }
    if containers.len() > MAX_CONTAINERS_PER_DEPLOYMENT {
        return Err(format!(
            "deployment.containers describes {} containers, more than the \
             {MAX_CONTAINERS_PER_DEPLOYMENT} this service accepts in one deployment",
            containers.len()
        ));
    }
    for (name, container) in &containers {
        if container
            .image
            .as_ref()
            .is_none_or(|image| image.trim().is_empty())
        {
            return Err(format!(
                "deployment.containers['{name}'].image must name the image the container runs"
            ));
        }
    }

    let public_endpoint = match &request.public_endpoint {
        None => None,
        Some(endpoint) => {
            let container = containers.get(&endpoint.container_name).ok_or_else(|| {
                format!(
                    "deployment.publicEndpoint.containerName '{}' names no container of this \
                     deployment, which describes [{}]",
                    endpoint.container_name,
                    containers.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            let open_ports = container.ports.clone().unwrap_or_default();
            if !open_ports.contains_key(&endpoint.container_port.to_string()) {
                let mut open: Vec<String> = open_ports.keys().cloned().collect();
                open.sort();
                return Err(format!(
                    "deployment.publicEndpoint.containerPort {} is not open on container '{}', \
                     which opens [{}]",
                    endpoint.container_port,
                    endpoint.container_name,
                    open.join(", ")
                ));
            }
            Some(ContainerServiceEndpoint {
                container_name: Some(endpoint.container_name.clone()),
                container_port: Some(endpoint.container_port),
                health_check: Some(health_check_from_request(endpoint.health_check.as_ref())?),
            })
        }
    };

    Ok(StoredDeployment {
        version,
        state: DEPLOYMENT_STATE_ACTIVATING,
        containers,
        public_endpoint,
        created_at_ms: now_ms,
    })
}

/// The health check `request` asks for, with every member it left out filled in
/// from that member's documented default, or a refusal naming the member that is
/// outside its documented range.
fn health_check_from_request(
    request: Option<&ContainerServiceHealthCheckConfig>,
) -> Result<ContainerServiceHealthCheckConfig, String> {
    let timeout_seconds = request
        .and_then(|config| config.timeout_seconds)
        .unwrap_or(HEALTH_CHECK_DEFAULT_TIMEOUT_SECONDS);
    let (timeout_min, timeout_max) = HEALTH_CHECK_TIMEOUT_SECONDS_RANGE;
    if !(timeout_min..=timeout_max).contains(&timeout_seconds) {
        return Err(format!(
            "deployment.publicEndpoint.healthCheck.timeoutSeconds must be between \
             {timeout_min} and {timeout_max}, got {timeout_seconds}"
        ));
    }
    let interval_seconds = request
        .and_then(|config| config.interval_seconds)
        .unwrap_or(HEALTH_CHECK_DEFAULT_INTERVAL_SECONDS);
    let (interval_min, interval_max) = HEALTH_CHECK_INTERVAL_SECONDS_RANGE;
    if !(interval_min..=interval_max).contains(&interval_seconds) {
        return Err(format!(
            "deployment.publicEndpoint.healthCheck.intervalSeconds must be between \
             {interval_min} and {interval_max}, got {interval_seconds}"
        ));
    }
    Ok(ContainerServiceHealthCheckConfig {
        healthy_threshold: Some(
            request
                .and_then(|config| config.healthy_threshold)
                .unwrap_or(HEALTH_CHECK_DEFAULT_THRESHOLD),
        ),
        unhealthy_threshold: Some(
            request
                .and_then(|config| config.unhealthy_threshold)
                .unwrap_or(HEALTH_CHECK_DEFAULT_THRESHOLD),
        ),
        timeout_seconds: Some(timeout_seconds),
        interval_seconds: Some(interval_seconds),
        path: Some(
            request
                .and_then(|config| config.path.clone())
                .unwrap_or_else(|| HEALTH_CHECK_DEFAULT_PATH.to_string()),
        ),
        success_codes: Some(
            request
                .and_then(|config| config.success_codes.clone())
                .unwrap_or_else(|| HEALTH_CHECK_DEFAULT_SUCCESS_CODES.to_string()),
        ),
    })
}

/// The public domain names `requested` carries, or a refusal when it carries more
/// than [`MAX_PUBLIC_DOMAIN_NAMES`] of them.
pub fn public_domain_names_from_request(
    requested: Option<&HashMap<String, Vec<String>>>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let names: BTreeMap<String, Vec<String>> = requested
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(certificate, mut domains)| {
            domains.sort();
            domains.dedup();
            (certificate, domains)
        })
        .collect();
    let total: usize = names.values().map(Vec::len).sum();
    if total > MAX_PUBLIC_DOMAIN_NAMES {
        return Err(format!(
            "publicDomainNames carries {total} domain names, more than the \
             {MAX_PUBLIC_DOMAIN_NAMES} a container service accepts"
        ));
    }
    Ok(names)
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-lightsail-lightsail-20161128. A handler
/// reaches it through [`with_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLightsailLightsail20161128State {
    /// Every instance, keyed by its name — the identifier every instance
    /// operation takes. A `BTreeMap` so `GetInstances` walks a stable, total
    /// order and a page token can name the next instance.
    pub instances: BTreeMap<String, StoredInstance>,
    /// Every domain recordset, keyed by its domain name — the identifier every
    /// domain operation takes. A `BTreeMap` so `GetDomains` walks a stable,
    /// total order.
    pub domains: BTreeMap<String, StoredDomain>,
    /// Every block storage disk, keyed by its disk name — the identifier every
    /// disk operation takes. A `BTreeMap` so `GetDisks` walks a stable, total
    /// order and a page token can name the next disk.
    pub disks: BTreeMap<String, StoredDisk>,
    /// Every disk snapshot, keyed by its snapshot name — the identifier every
    /// disk snapshot operation takes. A `BTreeMap` so `GetDiskSnapshots` walks a
    /// stable, total order and a page token can name the next snapshot.
    pub disk_snapshots: BTreeMap<String, StoredDiskSnapshot>,
    /// The append-only operation log, newest last: the source of truth for the
    /// `operations` every mutating instance call answers with.
    pub operations: Vec<StoredOperation>,
    /// Every container service, keyed by the region it was created in and its
    /// name — the pair that identifies it, because a container service name is
    /// unique within one region of the account and `GetContainerServices`
    /// reports "all of your container services in the Amazon Web Services Region
    /// where the request is made". A `BTreeMap` so a listing walks a stable,
    /// total order.
    pub container_services: BTreeMap<(String, String), StoredContainerService>,
    /// Every bucket, keyed by the region it was created in and its name — the
    /// pair that identifies it, because a bucket name is unique within one
    /// region of the account and `GetBuckets` reports "all of your buckets in the
    /// Amazon Web Services Region where the request is made". A `BTreeMap` so
    /// `GetBuckets` walks a stable, total order and a page token can name the
    /// next bucket.
    pub buckets: BTreeMap<(String, String), StoredBucket>,
    /// Every managed relational database, keyed by its name — the identifier
    /// every database operation takes. A `BTreeMap` so `GetRelationalDatabases`
    /// walks a stable, total order and a page token can name the next database.
    pub relational_databases: BTreeMap<String, StoredRelationalDatabase>,
    /// Offset of the next address pair to hand out, so two instances never share
    /// a public or a private address within one process.
    next_address: u32,
    /// Mixed into every minted id, so two ids minted in the same nanosecond
    /// differ.
    next_id: u64,
}

impl ComAmazonawsLightsailLightsail20161128State {
    /// The instance `name` names, for a reporting operation.
    pub fn instance(&self, name: &str) -> Option<&StoredInstance> {
        self.instances.get(name)
    }

    /// The instance `name` names, for an operation the spec says mutates it.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames an instance.
    pub fn instance_mut(&mut self, name: &str) -> Option<&mut StoredInstance> {
        self.instances.get_mut(name)
    }

    /// Store a newly created instance.
    pub fn insert_instance(&mut self, instance: StoredInstance) {
        self.instances.insert(instance.name.clone(), instance);
    }

    /// The domain recordset `name` names, for a reporting operation.
    pub fn domain(&self, name: &str) -> Option<&StoredDomain> {
        self.domains.get(name)
    }

    /// Store a newly created domain recordset.
    pub fn insert_domain(&mut self, domain: StoredDomain) {
        self.domains.insert(domain.name.clone(), domain);
    }

    /// The domain recordset `name` names, for an operation the spec says mutates
    /// it — adding, changing or removing one of the zone's entries.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames a zone.
    pub fn domain_mut(&mut self, name: &str) -> Option<&mut StoredDomain> {
        self.domains.get_mut(name)
    }

    /// The block storage disk `name` names, for a reporting operation.
    pub fn disk(&self, name: &str) -> Option<&StoredDisk> {
        self.disks.get(name)
    }

    /// Store a newly created block storage disk.
    pub fn insert_disk(&mut self, disk: StoredDisk) {
        self.disks.insert(disk.name.clone(), disk);
    }

    /// The disk snapshot `name` names, for a reporting operation.
    pub fn disk_snapshot(&self, name: &str) -> Option<&StoredDiskSnapshot> {
        self.disk_snapshots.get(name)
    }

    /// Store a newly created disk snapshot.
    pub fn insert_disk_snapshot(&mut self, snapshot: StoredDiskSnapshot) {
        self.disk_snapshots.insert(snapshot.name.clone(), snapshot);
    }

    /// Remove the instance `name` names, answering it. `None` when nothing
    /// matched.
    pub fn remove_instance(&mut self, name: &str) -> Option<StoredInstance> {
        self.instances.remove(name)
    }

    /// The container service `name` names in `region`, for a reporting
    /// operation.
    pub fn container_service(&self, region: &str, name: &str) -> Option<&StoredContainerService> {
        self.container_services
            .get(&(region.to_string(), name.to_string()))
    }

    /// The container service `name` names in `region`, for an operation that
    /// mutates it.
    pub fn container_service_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut StoredContainerService> {
        self.container_services
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Every container service in `region`, in the store's stable order.
    pub fn container_services_in(&self, region: &str) -> Vec<&StoredContainerService> {
        self.container_services
            .iter()
            .filter(|((service_region, _), _)| service_region == region)
            .map(|(_, service)| service)
            .collect()
    }

    /// Store a newly created container service.
    pub fn insert_container_service(&mut self, service: StoredContainerService) {
        self.container_services.insert(
            (service.location.region_name.clone(), service.name.clone()),
            service,
        );
    }

    /// The bucket `name` names in `region`, for a reporting operation.
    pub fn bucket(&self, region: &str, name: &str) -> Option<&StoredBucket> {
        self.buckets.get(&(region.to_string(), name.to_string()))
    }

    /// The managed relational database `name` names, for a reporting operation.
    pub fn relational_database(&self, name: &str) -> Option<&StoredRelationalDatabase> {
        self.relational_databases.get(name)
    }

    /// Store a newly created managed relational database.
    pub fn insert_relational_database(&mut self, database: StoredRelationalDatabase) {
        self.relational_databases
            .insert(database.name.clone(), database);
    }

    /// The bucket `name` names in `region`, for an operation the spec says
    /// mutates it — minting or withdrawing one of its access keys.
    ///
    /// `name` and the bucket's region are this record's store key, so neither may
    /// be reassigned through this handle; no operation in this slice renames or
    /// moves a bucket.
    pub fn bucket_mut(&mut self, region: &str, name: &str) -> Option<&mut StoredBucket> {
        self.buckets
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Store a newly created bucket.
    pub fn insert_bucket(&mut self, bucket: StoredBucket) {
        self.buckets.insert(
            (bucket.location.region_name.clone(), bucket.name.clone()),
            bucket,
        );
    }

    /// A token for one container service's default domain: see
    /// [`DOMAIN_TOKEN_LEN`].
    ///
    /// Draws from the SAME `next_id` counter [`Self::mint_uuid`] does, on
    /// purpose: one counter means two values minted in this process are never
    /// built from the same input, and the hash is domain-separated by its own
    /// label so a token and a uuid of the same counter value still differ.
    pub fn mint_domain_token(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(self.next_id.to_be_bytes());
        hasher.update(process_seed().to_be_bytes());
        hasher.update(b"container-service-domain");
        hasher
            .finalize()
            .iter()
            .take(DOMAIN_TOKEN_LEN)
            .map(|byte| {
                char::from(DOMAIN_TOKEN_ALPHABET[usize::from(*byte) % DOMAIN_TOKEN_ALPHABET.len()])
            })
            .collect()
    }

    /// The DNS label of one managed relational database's master endpoint: see
    /// [`RELATIONAL_DATABASE_HOST_LABEL_LEN`].
    ///
    /// Draws from the SAME `next_id` counter [`Self::mint_uuid`] does, for the
    /// reason [`Self::mint_domain_token`] gives, and is domain-separated by its
    /// own label so a host label and a uuid of the same counter value differ.
    pub fn mint_endpoint_host_label(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(self.next_id.to_be_bytes());
        hasher.update(process_seed().to_be_bytes());
        hasher.update(b"relational-database-endpoint");
        hasher
            .finalize()
            .iter()
            .take(RELATIONAL_DATABASE_HOST_LABEL_LEN)
            .map(|byte| {
                char::from(
                    RELATIONAL_DATABASE_HOST_LABEL_ALPHABET
                        [usize::from(*byte) % RELATIONAL_DATABASE_HOST_LABEL_ALPHABET.len()],
                )
            })
            .collect()
    }

    /// The next private/public IPv4 offset, consumed so no two instances in this
    /// process share an address.
    fn fresh_address_offset(&mut self) -> u32 {
        self.next_address = self.next_address.wrapping_add(1);
        self.next_address
    }

    /// The private address, public address and IPv6 addresses an instance with
    /// `ip_address_type` gets, each present only when that address type gives the
    /// instance one.
    pub fn fresh_addresses(
        &mut self,
        ip_address_type: &str,
    ) -> (Option<String>, Option<String>, Vec<String>) {
        let offset = self.fresh_address_offset();
        let has_ipv4 = ip_address_type_has_ipv4(ip_address_type);
        (
            has_ipv4.then(|| ipv4(PRIVATE_IPV4_BASE, offset)),
            has_ipv4.then(|| ipv4(PUBLIC_IPV4_BASE, offset)),
            if ip_address_type_has_ipv6(ip_address_type) {
                vec![ipv6(offset)]
            } else {
                Vec::new()
            },
        )
    }

    /// An identifier shaped like the UUID Lightsail answers with for an instance
    /// or an operation.
    pub fn mint_uuid(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(self.next_id.to_be_bytes());
        hasher.update(process_seed().to_be_bytes());
        let hex = hex::encode(hasher.finalize());
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Append `operation` to the log, dropping the oldest entries past
    /// [`MAX_OPERATION_HISTORY`] so the log stays bounded however many calls a
    /// caller makes.
    pub fn record_operation(&mut self, operation: StoredOperation) -> StoredOperation {
        self.operations.push(operation.clone());
        if let Some(excess) = self.operations.len().checked_sub(MAX_OPERATION_HISTORY) {
            self.operations.drain(..excess);
        }
        operation
    }

    /// Build and record an operation of `operation_type` against `resource_name`.
    ///
    /// `resource_type` is the kind of resource the operation acted on, from the
    /// modelled `ResourceType` enum, so the log says what each entry is about.
    ///
    /// `settles_at_ms` is the instant the clock owes the operation its terminal
    /// status — `None` for work that was already finished when the call
    /// returned, in which case the operation is answered terminal.
    pub fn start_operation(
        &mut self,
        now_ms: i64,
        operation_type: &str,
        resource_type: &str,
        resource_name: &str,
        location: &StoredLocation,
        settles_at_ms: Option<i64>,
    ) -> StoredOperation {
        let id = self.mint_uuid();
        let terminal = settles_at_ms.is_none();
        let operation = StoredOperation {
            id,
            resource_name: resource_name.to_string(),
            resource_type: resource_type.to_string(),
            created_at_ms: now_ms,
            location: location.clone(),
            is_terminal: terminal,
            operation_details: Some(resource_name.to_string()),
            operation_type: operation_type.to_string(),
            status: if terminal {
                OPERATION_STATUS_SUCCEEDED.to_string()
            } else {
                OPERATION_STATUS_STARTED.to_string()
            },
            status_changed_at_ms: now_ms,
            error_code: None,
            error_details: None,
            settles_at_ms,
        };
        self.record_operation(operation)
    }

    /// Materialise every transition the clock had already made due at `now_ms`:
    /// an instance, a disk, a disk snapshot or a managed relational database
    /// settles into the state it was owed, a container service walks
    /// into the phase it was owed (activating the deployment that was waiting),
    /// and an operation whose work is done settles into `Succeeded`.
    ///
    /// This is the one write a reporting operation performs. Each transition was
    /// owed at this instant whether or not anybody called, so applying it here
    /// cannot show a caller anything a later caller would not also see.
    ///
    /// Every walk is bounded by [`MAX_INSTANCES`], [`MAX_DISKS`],
    /// [`MAX_DISK_SNAPSHOTS`], [`MAX_CONTAINER_SERVICES`],
    /// [`MAX_RELATIONAL_DATABASES`] and
    /// [`MAX_OPERATION_HISTORY`], and a record drops out of the work as soon as
    /// it has nothing outstanding.
    fn advance_clock(&mut self, now_ms: i64) {
        for instance in self.instances.values_mut() {
            let Some(due) = instance.due else { continue };
            if due.at_ms <= now_ms {
                instance.state = due.state;
                instance.due = None;
            }
        }
        for disk in self.disks.values_mut() {
            let Some(due) = disk.due else { continue };
            if due.at_ms <= now_ms {
                disk.state = due.state;
                disk.due = None;
            }
        }
        for snapshot in self.disk_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
            }
        }
        for service in self.container_services.values_mut() {
            service.advance(now_ms);
        }
        for database in self.relational_databases.values_mut() {
            let Some(due) = database.due else { continue };
            if due.at_ms <= now_ms {
                database.state = due.state;
                database.due = None;
            }
        }
        for operation in &mut self.operations {
            let Some(settles_at_ms) = operation.settles_at_ms else {
                continue;
            };
            if settles_at_ms <= now_ms {
                operation.status = OPERATION_STATUS_SUCCEEDED.to_string();
                operation.is_terminal = true;
                operation.status_changed_at_ms = settles_at_ms;
                operation.settles_at_ms = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Input conversion and bounds
// ---------------------------------------------------------------------------

/// The add-ons `requests` enable, or a refusal naming the member that is wrong.
///
/// `addOnType` targets the modelled `AddOnType` enum, so only its two members are
/// accepted; each add-on is stored enabled, carrying the parameters its own
/// request structure supplies. An `AutoSnapshot` request that names no
/// `snapshotTimeOfDay` gets the hour Lightsail would pick for it, drawn here from
/// the resource's own name so the answer is the same on every later read.
pub fn add_ons_from_requests(
    resource_name: &str,
    requests: &[AddOnRequest],
) -> Result<Vec<AddOn>, String> {
    if requests.len() > MAX_ADD_ONS {
        return Err(format!(
            "addOns carries {} entries, more than the {MAX_ADD_ONS} this service accepts",
            requests.len()
        ));
    }
    requests
        .iter()
        .map(|request| match request.add_on_type.as_str() {
            "AutoSnapshot" => Ok(AddOn {
                name: Some("AutoSnapshot".to_string()),
                status: Some("Enabled".to_string()),
                snapshot_time_of_day: Some(
                    request
                        .auto_snapshot_add_on_request
                        .as_ref()
                        .and_then(|auto| auto.snapshot_time_of_day.clone())
                        .unwrap_or_else(|| default_snapshot_time_of_day(resource_name)),
                ),
                ..Default::default()
            }),
            "StopInstanceOnIdle" => Ok(AddOn {
                name: Some("StopInstanceOnIdle".to_string()),
                status: Some("Enabled".to_string()),
                threshold: request
                    .stop_instance_on_idle_request
                    .as_ref()
                    .and_then(|idle| idle.threshold.clone()),
                duration: request
                    .stop_instance_on_idle_request
                    .as_ref()
                    .and_then(|idle| idle.duration.clone()),
                ..Default::default()
            }),
            other => Err(format!(
                "addOns[].addOnType must be one of [{}], got '{other}'",
                crate::ops::types::ADD_ON_TYPE_VALUES.join(", ")
            )),
        })
        .collect()
}

/// The one modelled `AddOnType` member a block storage disk can carry: the
/// automatic snapshot add-on.
///
/// The other member stops a virtual computer after an idle period
/// (`StopInstanceOnIdleRequest` — "the amount of idle time in minutes after
/// which your virtual computer will automatically stop"), which a block storage
/// disk does not have.
pub const ADD_ON_TYPE_AUTO_SNAPSHOT: &str = "AutoSnapshot";

/// The add-ons `requests` enable on a DISK, or a refusal naming the member that
/// is wrong.
///
/// The same conversion as [`add_ons_from_requests`], narrowed to the one add-on
/// a disk can carry ([`ADD_ON_TYPE_AUTO_SNAPSHOT`]): asking a disk for the
/// idle-stop add-on is refused rather than stored as an add-on that can never
/// act. Every operation that creates a disk goes through here, so `CreateDisk`
/// and `CreateDiskFromSnapshot` accept exactly the same add-ons.
pub fn disk_add_ons_from_requests(
    disk_name: &str,
    requests: &[AddOnRequest],
) -> Result<Vec<AddOn>, String> {
    if let Some(request) = requests
        .iter()
        .find(|request| request.add_on_type != ADD_ON_TYPE_AUTO_SNAPSHOT)
    {
        return Err(format!(
            "addOns[].addOnType '{}' cannot be enabled on a disk: a disk accepts only \
             {ADD_ON_TYPE_AUTO_SNAPSHOT}",
            request.add_on_type
        ));
    }
    add_ons_from_requests(disk_name, requests)
}

/// Whether `value` is a date in the `YYYY-MM-DD` form an automatic snapshot is
/// addressed by.
///
/// The form is the modelled `AutoSnapshotDate` pattern
/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}$`; the members that carry a restore date
/// (`CreateDiskFromSnapshot$restoreDate` and its siblings) target a plain
/// `string`, whose documentation says "Must be specified in YYYY-MM-DD format",
/// so the constraint is enforced here rather than by the modelled-constraint
/// walk.
pub fn is_auto_snapshot_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|index| bytes[*index].is_ascii_digit())
}

/// The daily automatic-snapshot hour for `resource_name`, in the `HH:00` form the
/// modelled `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` requires.
///
/// Derived from the resource's own name rather than drawn per call, so every read
/// of the resource reports the same hour.
fn default_snapshot_time_of_day(resource_name: &str) -> String {
    let digest = Sha256::digest(resource_name.as_bytes());
    format!("{:02}:00", u32::from(digest[0]) % 24)
}

// ---------------------------------------------------------------------------
// Domain names
// ---------------------------------------------------------------------------

/// The most characters a domain name may carry.
///
/// RFC 1035 §2.3.4 caps an encoded domain name at 255 octets, which is 253
/// characters in the dotted presentation form a request uses.
pub const MAX_DOMAIN_NAME_CHARS: usize = 253;

/// The most characters one label of a domain name may carry, from the same
/// RFC 1035 §2.3.4 limit on a single label.
pub const MAX_DOMAIN_LABEL_CHARS: usize = 63;

/// Refuse `domain_name` unless it is a DNS domain name this service can hold a
/// recordset for, answering the member and the constraint it broke.
///
/// The modelled `DomainName` shape carries no pattern and no length, and the
/// modelled `CreateDomain` documentation says the name is "the specified domain
/// (example.com)" — so the rule is DNS's own: at least two dot-separated
/// labels (a name with no parent zone addresses no domain), each a non-empty
/// run of at most [`MAX_DOMAIN_LABEL_CHARS`] letters, digits and hyphens that
/// neither begins nor ends with a hyphen (RFC 1035 §2.3.1 preferred syntax,
/// widened to leading digits by RFC 1123 §2.1), and at most
/// [`MAX_DOMAIN_NAME_CHARS`] characters overall.
pub fn validate_domain_name(domain_name: &str) -> Result<(), String> {
    if domain_name.chars().count() > MAX_DOMAIN_NAME_CHARS {
        return Err(format!(
            "domainName carries {} characters, more than the {MAX_DOMAIN_NAME_CHARS} a \
             domain name may have",
            domain_name.chars().count()
        ));
    }
    let labels: Vec<&str> = domain_name.split('.').collect();
    if labels.len() < 2 {
        return Err(format!(
            "domainName '{domain_name}' is not a domain name: it must carry at least two \
             dot-separated labels (example.com)"
        ));
    }
    for label in labels {
        if label.is_empty() {
            return Err(format!(
                "domainName '{domain_name}' is not a domain name: every dot-separated label \
                 must be non-empty"
            ));
        }
        if label.chars().count() > MAX_DOMAIN_LABEL_CHARS {
            return Err(format!(
                "domainName label '{label}' carries {} characters, more than the \
                 {MAX_DOMAIN_LABEL_CHARS} a label may have",
                label.chars().count()
            ));
        }
        if !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(format!(
                "domainName label '{label}' is not a domain label: a label carries only \
                 ASCII letters, digits and hyphens"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "domainName label '{label}' is not a domain label: a label neither begins \
                 nor ends with a hyphen"
            ));
        }
    }
    Ok(())
}

/// The domain entry types a zone may hold, from the modelled `DomainEntry$type`
/// documentation: "address for IPv4 (A), address for IPv6 (AAAA), canonical name
/// (CNAME), mail exchanger (MX), name server (NS), start of authority (SOA),
/// service locator (SRV), or text (TXT)", listed there as the types that "can be
/// used".
///
/// The modelled `DomainEntryType` shape is a plain string, so this list — not a
/// generated enum — is what an entry's `type` is checked against.
pub const DOMAIN_ENTRY_TYPES: &[&str] = &["A", "AAAA", "CNAME", "MX", "NS", "SOA", "SRV", "TXT"];

/// The types `DOMAIN_ENTRY_TYPES` admits, for a refusal message.
pub fn domain_entry_types() -> String {
    DOMAIN_ENTRY_TYPES.join(", ")
}

/// Refuse `entry_name` unless it is a DNS name the zone `domain_name` can hold a
/// recordset for, answering the member and the constraint it broke.
///
/// A recordset entry lives INSIDE one zone, so its name is either the zone's own
/// name (the apex) or a name under it; `member` is the request member the name
/// came from, so the refusal names what the caller sent.
///
/// The syntax rule is DNS's own, and WIDER than [`validate_domain_name`]'s,
/// because a zone holds entries a zone name may not be: a label may be the `*`
/// wildcard (leftmost label only, RFC 4592 §2.1.1) and a label may carry an
/// underscore, which is what an `_tcp` service locator (RFC 2782) or a `_`-prefixed
/// validation record is made of. Names are compared case-insensitively (RFC 4343).
pub fn validate_domain_entry_name(
    member: &str,
    entry_name: &str,
    domain_name: &str,
) -> Result<(), String> {
    if entry_name.chars().count() > MAX_DOMAIN_NAME_CHARS {
        return Err(format!(
            "{member} carries {} characters, more than the {MAX_DOMAIN_NAME_CHARS} a \
             domain name may have",
            entry_name.chars().count()
        ));
    }
    let labels: Vec<&str> = entry_name.split('.').collect();
    if labels.len() < 2 {
        return Err(format!(
            "{member} '{entry_name}' is not a domain name: it must carry at least two \
             dot-separated labels (www.example.com)"
        ));
    }
    for (index, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(format!(
                "{member} '{entry_name}' is not a domain name: every dot-separated label \
                 must be non-empty"
            ));
        }
        if label.chars().count() > MAX_DOMAIN_LABEL_CHARS {
            return Err(format!(
                "{member} label '{label}' carries {} characters, more than the \
                 {MAX_DOMAIN_LABEL_CHARS} a label may have",
                label.chars().count()
            ));
        }
        if *label == "*" {
            if index == 0 {
                continue;
            }
            return Err(format!(
                "{member} '{entry_name}' is not a domain name: the '*' wildcard is only \
                 the leftmost label of a name"
            ));
        }
        if !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
        {
            return Err(format!(
                "{member} label '{label}' is not a domain label: a label carries only \
                 ASCII letters, digits, hyphens and underscores"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "{member} label '{label}' is not a domain label: a label neither begins \
                 nor ends with a hyphen"
            ));
        }
    }
    let name = entry_name.to_ascii_lowercase();
    let zone = domain_name.to_ascii_lowercase();
    if name != zone && !name.ends_with(&format!(".{zone}")) {
        return Err(format!(
            "{member} '{entry_name}' is not inside the domain '{domain_name}': an entry \
             names either the domain itself or a name under it"
        ));
    }
    Ok(())
}

/// Whether `entry` is the recordset entry `name`/`entry_type`/`target` describes:
/// the three values that identify one entry of a zone. Names and types are
/// compared case-insensitively (RFC 4343, and the type is a DNS record type).
pub fn is_same_domain_entry(
    entry: &DomainEntry,
    name: &str,
    entry_type: &str,
    target: &str,
) -> bool {
    let same = |stored: &Option<String>, requested: &str| {
        stored
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(requested))
    };
    same(&entry.name, name) && same(&entry.r#type, entry_type) && entry.target.as_deref() == Some(target)
}

// ---------------------------------------------------------------------------
// Clocks and ids
// ---------------------------------------------------------------------------

/// Now, in milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Epoch milliseconds as the modelled `IsoDate` timestamp.
pub fn timestamp(epoch_millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis)
}

/// A seed drawn once per process, so ids minted by two emulator processes from
/// the same counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos())
    })
}

/// The ARN of the instance `id` in `region`.
pub fn instance_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{INSTANCE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the block storage disk `id` in `region`.
pub fn disk_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DISK_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the disk snapshot `id` in `region`.
pub fn disk_snapshot_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{DISK_SNAPSHOT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the domain recordset `id`. A DNS zone is not regional, so its ARN
/// carries [`DOMAIN_ARN_REGION`] where an instance ARN carries a region.
pub fn domain_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        DOMAIN_ARN_REGION,
        &format!("{DOMAIN_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The support code an instance reports: the `{account}/{resource id}` form the
/// modelled `supportCode` member describes, so support can look the resource up
/// from the value alone.
pub fn support_code(id: &str) -> String {
    format!("{}/{id}", ::engine::emulator_account_id())
}

/// The region a request is scoped to: the one its SigV4 credential scope names,
/// or [`DEFAULT_REGION`] when the request carries no scope (an unsigned call) or
/// names a region Lightsail does not serve.
///
/// `ResourceLocation$regionName` targets the modelled `RegionName` enum, so the
/// answer is always one of that enum's values — a scope naming anything else is
/// not a region this service has resources in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| REGION_NAME_VALUES.contains(&region.as_str()))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The endpoint this emulator was reached on for this request: its `Host`
/// header, or the `:authority` an HTTP/2 request carries on the URI instead of a
/// `Host` header.
///
/// This is what an operation announcing an address a caller must be able to
/// reach has to build it from — never a literal AWS hostname, which would send
/// the next client out of the emulator (and out of a sandboxed run) entirely.
/// `None` when the request names no host at all, and then there is no endpoint
/// to announce.
pub fn request_endpoint(parts: &::http::request::Parts) -> Option<String> {
    parts
        .headers
        .get(::http::header::HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| parts.uri.authority().map(::http::uri::Authority::as_str))
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Reaching the store
// ---------------------------------------------------------------------------

/// Run `body` against this service's store, with every clock-owed transition
/// already applied.
///
/// EVERY handler goes through here, so no handler can observe an instance or a
/// disk that should already have settled. `None` means the engine is not the shared
/// in-memory one — answer it with a modelled server error, never with a fresh
/// engine.
pub fn with_state<Eng: Send + Sync + 'static, R>(
    engine: &Eng,
    body: impl FnOnce(&mut ComAmazonawsLightsailLightsail20161128State) -> R,
) -> Option<R> {
    let state = service_state(engine)?;
    let mut guard = state.lock();
    guard.advance_clock(now_ms());
    Some(body(&mut guard))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLightsailLightsail20161128State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLightsailLightsail20161128State>)
}

// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lightsail-lightsail-20161128 service.
//!
//! The slice implemented so far is Lightsail's primary resource, the **instance**
//! (a virtual private server), the **block storage disk** (additional storage an
//! instance can attach), the **domain** (a DNS zone the account holds
//! recordsets in), the **container service** (a compute resource containers are
//! deployed to), the append-only **operation log** every mutating call writes,
//! and the service's static **Region catalog** — the Regions and Availability
//! Zones `GetRegions` lists and every other operation validates a location
//! against. Everything mutable in this module is reached through [`with_state`],
//! so no handler can observe a store whose clock-owed transitions have not been
//! applied yet.
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
    AddOn, AddOnRequest, AvailabilityZone, BucketBundle, Container, ContainerImage,
    ContainerService, ContainerServiceDeployment, ContainerServiceDeploymentRequest,
    ContainerServiceECRImagePullerRole, ContainerServiceEndpoint,
    ContainerServiceHealthCheckConfig, ContainerServiceStateDetail, Domain, DomainEntry, Instance,
    InstanceHardware, InstanceMetadataOptions, InstanceNetworking, InstancePortInfo, InstanceState,
    MonthlyTransfer, Operation, PrivateRegistryAccess, Region, RelationalDatabaseBundle,
    ResourceLocation, Tag, REGION_NAME_VALUES,
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

/// A disk state the clock already owes a disk.
///
/// Separate from [`DueTransition`] because a disk's state is a value of the
/// modelled `DiskState` enum — a name with no numeric code — where an instance's
/// is an EC2 `InstanceState` (a code and a name together).
#[derive(Debug, Clone, Copy)]
pub struct DueDiskState {
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    /// The state the disk settles into.
    pub state: &'static str,
}

impl DueDiskState {
    /// The transition to `state`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, state: &'static str) -> Self {
        Self {
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            state,
        }
    }
}

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
pub const OPERATION_TYPE_CREATE_DISK: &str = "CreateDisk";
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
// Bundles
// ---------------------------------------------------------------------------

/// The hardware and monthly transfer one Lightsail bundle size provides.
///
/// A bundle id is `<size>_<major>_<minor>`, optionally with a platform token
/// (`micro_win_2_0`), so the leading token names the size and the rest names the
/// bundle's version. The figures are the published Lightsail instance plans:
/// each step doubles RAM, and the vCPU count, SSD size and data transfer follow
/// the plan table.
#[derive(Debug, Clone, Copy)]
pub struct BundleSpec {
    /// The leading token of every bundle id that maps here.
    pub size: &'static str,
    pub cpu_count: i32,
    pub ram_size_in_gb: f32,
    pub disk_size_in_gb: i32,
    /// `InstanceNetworking$monthlyTransfer.gbPerMonthAllocated`, in GB.
    pub transfer_gb_per_month: i32,
}

/// Every bundle size this slice serves. See [`BundleSpec`] for the source.
pub const BUNDLE_SPECS: &[BundleSpec] = &[
    BundleSpec {
        size: "nano",
        cpu_count: 1,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_gb_per_month: 1_024,
    },
    BundleSpec {
        size: "micro",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
    },
    BundleSpec {
        size: "small",
        cpu_count: 1,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 60,
        transfer_gb_per_month: 3_072,
    },
    BundleSpec {
        size: "medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 80,
        transfer_gb_per_month: 4_096,
    },
    BundleSpec {
        size: "large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 160,
        transfer_gb_per_month: 5_120,
    },
    BundleSpec {
        size: "xlarge",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 320,
        transfer_gb_per_month: 6_144,
    },
    BundleSpec {
        size: "2xlarge",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 640,
        transfer_gb_per_month: 7_168,
    },
    BundleSpec {
        size: "4xlarge",
        cpu_count: 16,
        ram_size_in_gb: 64.0,
        disk_size_in_gb: 1_280,
        transfer_gb_per_month: 8_192,
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
        self.availability_zone_letters
            .chars()
            .map(|letter| AvailabilityZone {
                zone_name: Some(format!("{}{letter}", self.name)),
                state: Some(AVAILABILITY_ZONE_STATE_AVAILABLE.to_string()),
            })
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
    pub due: Option<DueDiskState>,
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
    /// an instance or a disk settles into the state it was owed, a container service walks
    /// into the phase it was owed (activating the deployment that was waiting),
    /// and an operation whose work is done settles into `Succeeded`.
    ///
    /// This is the one write a reporting operation performs. Each transition was
    /// owed at this instant whether or not anybody called, so applying it here
    /// cannot show a caller anything a later caller would not also see.
    ///
    /// Every walk is bounded by [`MAX_INSTANCES`], [`MAX_DISKS`],
    /// [`MAX_CONTAINER_SERVICES`] and [`MAX_OPERATION_HISTORY`], and a record
    /// drops out of the work as soon as it has nothing outstanding.
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
        for service in self.container_services.values_mut() {
            service.advance(now_ms);
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

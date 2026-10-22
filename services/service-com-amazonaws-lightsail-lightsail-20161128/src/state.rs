// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lightsail-lightsail-20161128 service.
//!
//! The slice implemented so far is Lightsail's primary resource, the **instance**
//! (a virtual private server), together with the append-only **operation log**
//! every mutating instance call writes. Everything in this module is reached
//! through [`with_state`], so no handler can observe a store whose clock-owed
//! instance transitions have not been applied yet.
//!
//! Two kinds of value live on a stored instance:
//!
//! * what the caller asked for — name, availability zone, blueprint, bundle,
//!   key pair, tags, add-ons, IP address type — kept verbatim; and
//! * what the service assigns — ARN, support code, creation time, addresses,
//!   hardware, firewall rules, user name and lifecycle state — each computed
//!   from the request plus this store's own allocators, never from a canned body.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AddOn, AddOnRequest, Instance, InstanceHardware, InstanceMetadataOptions, InstanceNetworking,
    InstancePortInfo, InstanceState, MonthlyTransfer, Operation, ResourceLocation, Tag,
    REGION_NAME_VALUES,
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

/// How long after the call that starts it the clock owes an instance its settled
/// state (`pending` -> `running`, `stopping` -> `stopped`).
///
/// CHOSEN, not an AWS duration: AWS takes tens of seconds to minutes, which no
/// caller can be made to wait for here. The span is deliberately NON-ZERO, so a
/// caller that reads straight after `CreateInstances`, `StartInstance` or
/// `StopInstance` still observes the transient state exactly as it does against
/// AWS.
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

/// Where a stored record lives.
#[derive(Debug, Clone)]
pub struct StoredLocation {
    pub availability_zone: String,
    pub region_name: String,
}

impl StoredLocation {
    /// This location as the modelled `ResourceLocation`.
    pub fn view(&self) -> ResourceLocation {
        ResourceLocation {
            availability_zone: Some(self.availability_zone.clone()),
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
    /// The append-only operation log, newest last: the source of truth for the
    /// `operations` every mutating instance call answers with.
    pub operations: Vec<StoredOperation>,
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

    /// Remove the instance `name` names, answering it. `None` when nothing
    /// matched.
    pub fn remove_instance(&mut self, name: &str) -> Option<StoredInstance> {
        self.instances.remove(name)
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
    /// `settles_at_ms` is the instant the clock owes the operation its terminal
    /// status — `None` for work that was already finished when the call
    /// returned, in which case the operation is answered terminal.
    pub fn start_operation(
        &mut self,
        now_ms: i64,
        operation_type: &str,
        resource_name: &str,
        location: &StoredLocation,
        settles_at_ms: Option<i64>,
    ) -> StoredOperation {
        let id = self.mint_uuid();
        let terminal = settles_at_ms.is_none();
        let operation = StoredOperation {
            id,
            resource_name: resource_name.to_string(),
            resource_type: RESOURCE_TYPE_INSTANCE.to_string(),
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
    /// an instance settles into the state it was owed, and an operation whose
    /// work is done settles into `Succeeded`.
    ///
    /// This is the one write a reporting operation performs. Each transition was
    /// owed at this instant whether or not anybody called, so applying it here
    /// cannot show a caller anything a later caller would not also see.
    ///
    /// Both walks are bounded by [`MAX_INSTANCES`] and
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
/// the instance's own name so the answer is the same on every later read.
pub fn add_ons_from_requests(
    instance_name: &str,
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
                        .unwrap_or_else(|| default_snapshot_time_of_day(instance_name)),
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

/// The daily automatic-snapshot hour for `instance_name`, in the `HH:00` form the
/// modelled `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` requires.
///
/// Derived from the instance's own name rather than drawn per call, so every read
/// of the instance reports the same hour.
fn default_snapshot_time_of_day(instance_name: &str) -> String {
    let digest = Sha256::digest(instance_name.as_bytes());
    format!("{:02}:00", u32::from(digest[0]) % 24)
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

/// The support code an instance reports: the `{account}/{resource id}` form the
/// modelled `supportCode` member describes, so support can look the resource up
/// from the value alone.
pub fn support_code(id: &str) -> String {
    format!("{}/{id}", ::engine::emulator_account_id())
}

// ---------------------------------------------------------------------------
// Reaching the store
// ---------------------------------------------------------------------------

/// Run `body` against this service's store, with every clock-owed transition
/// already applied.
///
/// EVERY handler goes through here, so no handler can observe an instance that
/// should already have settled. `None` means the engine is not the shared
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

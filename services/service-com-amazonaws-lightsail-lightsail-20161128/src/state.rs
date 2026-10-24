// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lightsail-lightsail-20161128 service.
//!
//! The slice implemented so far is Lightsail's primary resource, the **instance**
//! (a virtual private server), the **block storage disk** (additional storage an
//! instance can attach), the **disk snapshot** (a point-in-time copy of a disk
//! or of an instance's system volume), the **instance snapshot** (a point-in-time
//! copy of a whole instance — its system volume, its blueprint and bundle, and
//! the block storage disks attached to it) together with the **export snapshot
//! record** (what an export of one of those snapshots to Amazon EC2 leaves
//! behind), the **domain** (a DNS zone the account holds
//! recordsets in), the **container service** (a compute resource containers are
//! deployed to), the **bucket** (an object-storage resource of the Lightsail
//! object storage service) together with the **access keys** it grants
//! programmatic access through — whose OBJECTS live in the one object store this
//! emulator holds, Amazon S3's own, which this module provisions a bucket in
//! ([`claim_bucket_object_store`]) rather than keeping a second object map here —
//! the **managed relational database** (a MySQL or PostgreSQL database the service
//! runs for the account) together with the **database snapshot** (a point-in-time
//! copy of one), the **load balancer** (a regional resource that spreads
//! HTTP traffic across the instances attached to it), the **static IP** (a public
//! IPv4 address the account reserves and attaches to an instance), the **CDN
//! distribution** (a global resource that caches and serves the content of an
//! origin resource), the peering of a Region's **Lightsail VPC** with the
//! account's default VPC there, the append-only **operation log** every mutating
//! call writes, and the service's static **Region catalog** — the Regions and
//! Availability Zones `GetRegions` lists and every other operation validates a
//! location against. Everything mutable in this module is reached through
//! [`with_state`], so no handler can observe a store whose clock-owed transitions
//! have not been applied yet.
//!
//! Two kinds of value live on a stored instance:
//!
//! * what the caller asked for — name, availability zone, blueprint, bundle,
//!   key pair, tags, add-ons, IP address type, and the inbound firewall rules a
//!   later `OpenInstancePublicPorts`, `CloseInstancePublicPorts` or
//!   `PutInstancePublicPorts` call makes of the platform's defaults — kept
//!   verbatim; and
//! * what the service assigns — ARN, support code, creation time, addresses,
//!   hardware, firewall rules, user name and lifecycle state — each computed
//!   from the request plus this store's own allocators, never from a canned body.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

// A bucket's OBJECT data plane is not a map of this module's own: a Lightsail bucket
// is an S3-compatible object store, and the service that serves the S3 object
// protocol in this emulator is Amazon S3, so a bucket is provisioned in that one
// store (see `claim_bucket_object_store`) and this is the outcome it reports.
pub use ::service_com_amazonaws_s3_amazons3::state::BucketCreation;

use crate::ops::types::{
    AccessKey, AccessKeyLastUsed, AccessRules, AccountLevelBpaSync, AddOn, AddOnRequest, Alarm,
    AvailabilityZone, Blueprint, Bucket, BucketAccessLogConfig, BucketBundle, BucketCorsConfig,
    BucketCorsRule, BucketState, Bundle, CacheBehavior, CacheBehaviorPerPath, CacheSettings,
    Container,
    ContainerImage,
    ContainerService, ContainerServiceDeployment, ContainerServiceDeploymentRequest,
    ContainerServiceECRImagePullerRole, ContainerServiceEndpoint,
    ContainerServiceHealthCheckConfig, ContainerServiceStateDetail, DestinationInfo,
    DistributionBundle, DistributionCustomErrorResponse,
    Domain, DomainEntry, ExportSnapshotRecord, ExportSnapshotRecordSourceInfo,
    Instance,
    InstanceHardware, InstanceHealthSummary, InstanceMetadataOptions, InstanceNetworking,
    InstancePortInfo, InstanceState, LightsailDistribution, LoadBalancer,
    LoadBalancerTlsCertificateSummary,
    MonitoredResourceInfo, MonthlyTransfer, Operation, Origin, PrivateRegistryAccess, Region,
    RelationalDatabaseBundle, RelationalDatabaseParameter,
    ResourceLocation, ResourceReceivingAccess,
    StaticIp, Tag, REGION_NAME_VALUES,
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

/// The `resourceType` a managed relational database SNAPSHOT reports, from the
/// same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_RELATIONAL_DATABASE_SNAPSHOT: &str = "RelationalDatabaseSnapshot";

/// The resource portion a managed relational database snapshot's ARN carries
/// before its id. Mirrors the `Instance/<uuid>` form the modelled `Instance$arn`
/// documentation shows, with this resource's own `ResourceType` value.
pub const RELATIONAL_DATABASE_SNAPSHOT_ARN_RESOURCE_PREFIX: &str = "RelationalDatabaseSnapshot/";

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

/// The refusal a domain or distribution call made outside [`DOMAIN_REGION_NAME`]
/// is answered with, as the modelled `InvalidInputException` documentation
/// states it (`smithy_query(query="doc",shape="InvalidInputException")`): those
/// APIs are served by the N. Virginia endpoint ALONE, so a call to them from any
/// other Region's endpoint is refused rather than served there.
pub const DOMAIN_REGION_ONLY_MESSAGE: &str = "Domain and distribution APIs are only available in \
     the N. Virginia (us-east-1) Amazon Web Services Region. Please set your Amazon Web Services \
     Region configuration to us-east-1 to create, view, or edit these resources.";

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

/// The resource portion an instance snapshot ARN carries before its id, from the
/// modelled `InstanceSnapshot$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:InstanceSnapshot/d23b5706-…`).
pub const INSTANCE_SNAPSHOT_ARN_RESOURCE_PREFIX: &str = "InstanceSnapshot/";

/// The `resourceType` an instance snapshot and its operations report, from the
/// modelled `ResourceType` enum — the value the modelled
/// `InstanceSnapshot$resourceType` documentation names ("The type of resource
/// (usually `InstanceSnapshot`)").
pub const RESOURCE_TYPE_INSTANCE_SNAPSHOT: &str = "InstanceSnapshot";

/// The resource portion an export snapshot record ARN carries before its id.
/// Mirrors the `Instance/<uuid>` form the modelled `Instance$arn` documentation
/// shows, with this resource's own `ResourceType` value.
pub const EXPORT_SNAPSHOT_RECORD_ARN_RESOURCE_PREFIX: &str = "ExportSnapshotRecord/";

/// The `resourceType` an export snapshot record reports, from the modelled
/// `ResourceType` enum — the value the modelled
/// `ExportSnapshotRecord$resourceType` documentation names ("The Lightsail
/// resource type (`ExportSnapshotRecord`)").
pub const RESOURCE_TYPE_EXPORT_SNAPSHOT_RECORD: &str = "ExportSnapshotRecord";

/// The resource portion a static IP ARN carries before its id, from the modelled
/// `StaticIp$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:StaticIp/9cbb4a9e-…`).
pub const STATIC_IP_ARN_RESOURCE_PREFIX: &str = "StaticIp/";

/// The `resourceType` a static IP and its operations report, from the modelled
/// `StaticIp$resourceType` documentation ("the resource type (usually
/// `StaticIp`)") and the same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_STATIC_IP: &str = "StaticIp";

/// The `resourceType` an alarm and its operations report, from the same modelled
/// `ResourceType` enum — the value the modelled `Alarm$resourceType` member
/// carries ("The Lightsail resource type of the alarm").
pub const RESOURCE_TYPE_ALARM: &str = "Alarm";

/// The `resourceType` the peering of a Region's Lightsail VPC with the account's
/// default VPC reports, from the same modelled `ResourceType` enum — its
/// `PeeredVpc` member, the only one that names a VPC peering.
pub const RESOURCE_TYPE_PEERED_VPC: &str = "PeeredVpc";

/// The `resourceType` a load balancer and its operations report, from the same
/// modelled `ResourceType` enum — the value the modelled
/// `LoadBalancer$resourceType` member carries ("The resource type
/// (`LoadBalancer`").
///
/// It is also what keys the alarm metric catalog, because the modelled
/// `PutAlarmRequest$metricName` documentation names load balancers as one of the
/// three kinds of resource an alarm monitors.
pub const RESOURCE_TYPE_LOAD_BALANCER: &str = "LoadBalancer";

/// The `resourceType` an SSL/TLS certificate of a load balancer and its
/// operations report, from the same modelled `ResourceType` enum — the value the
/// modelled `LoadBalancerTlsCertificate$resourceType` member carries ("The
/// resource type (`LoadBalancerTlsCertificate`)").
pub const RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE: &str = "LoadBalancerTlsCertificate";

/// The resource portion a load balancer ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const LOAD_BALANCER_ARN_RESOURCE_PREFIX: &str = "LoadBalancer/";

/// The resource portion an alarm ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const ALARM_ARN_RESOURCE_PREFIX: &str = "Alarm/";

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
/// `stopped`), a block storage disk's (`pending` -> `available`, and the
/// `in-use` -> `available` a `DetachDisk` starts) and a container
/// service's (`PENDING` -> `READY`, and each step of `PENDING` -> `DEPLOYING` ->
/// `RUNNING`).
///
/// CHOSEN, not an AWS duration: AWS takes tens of seconds to minutes, which no
/// caller can be made to wait for here. The span is deliberately NON-ZERO, so a
/// caller that reads straight after `CreateInstances`, `StartInstance`,
/// `StopInstance`, `CreateDisk`, `DetachDisk` or `CreateContainerService` still
/// observes the transient state exactly as it does against AWS.
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

/// `InstanceSnapshotState` values this slice uses, from the modelled
/// `InstanceSnapshotState` enum and the `InstanceSnapshot$state` documentation
/// ("The state the snapshot is in"): a snapshot Lightsail is still taking is
/// `pending` and one that can be restored is `available`. The enum's third
/// member, `error`, describes a snapshot that failed, which this store cannot
/// produce — nothing here fails a copy.
pub const INSTANCE_SNAPSHOT_STATE_PENDING: &str = "pending";
/// See [`INSTANCE_SNAPSHOT_STATE_PENDING`].
pub const INSTANCE_SNAPSHOT_STATE_AVAILABLE: &str = "available";

/// `RecordState` values this slice uses, from the modelled `RecordState` enum
/// and the `ExportSnapshotRecord$state` documentation ("The state of the export
/// snapshot record"): an export is answered `Started` and settles into
/// `Succeeded` once the resource it creates at the destination exists. The
/// enum's `Failed` member describes an export that failed, which this store
/// cannot produce — nothing here fails an export.
pub const RECORD_STATE_STARTED: &str = "Started";
/// See [`RECORD_STATE_STARTED`].
pub const RECORD_STATE_SUCCEEDED: &str = "Succeeded";

/// A state the clock already owes a resource whose state is a plain NAME: a
/// block storage disk's `DiskState` value, a disk snapshot's
/// `DiskSnapshotState` value, a managed relational database's `state`, or one of
/// that database's snapshots' `state`.
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
/// being provisioned, `available` once it accepts connections, `rebooting`
/// while its engine is being restarted, `stopping` while it is being shut down,
/// `stopped` once it is shut down and accepts no connection, and `starting`
/// while it is being brought back up (Amazon RDS User Guide, "Viewing DB
/// instance status").
pub const RELATIONAL_DATABASE_STATE_CREATING: &str = "creating";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_AVAILABLE: &str = "available";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_REBOOTING: &str = "rebooting";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_STOPPING: &str = "stopping";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_STOPPED: &str = "stopped";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_STARTING: &str = "starting";

/// How long a managed relational database may stay stopped before the service
/// starts it again on its own.
///
/// An AWS DURATION, kept at the published value rather than chosen: "If you
/// don't manually start your database instance after it has been stopped for
/// seven consecutive days, Amazon Lightsail automatically starts it for you"
/// (the modelled `StopRelationalDatabase` documentation).
pub const RELATIONAL_DATABASE_AUTO_START_AFTER_MILLIS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// `RelationalDatabaseSnapshot$state` values this slice uses — "The state of the
/// database snapshot", a plain string in the model with no enum behind it.
///
/// A Lightsail database snapshot is an Amazon RDS DB snapshot, and the member
/// carries RDS's own DB snapshot status values: `creating` while the snapshot is
/// being taken and `available` once it can be restored from (Amazon RDS API
/// Reference, `DBSnapshot$Status`). RDS's third value, `failed`, describes a
/// snapshot whose copy failed, which this store cannot produce — nothing here
/// fails a copy.
pub const RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING: &str = "creating";
/// See [`RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING`].
pub const RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE: &str = "available";

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
pub const OPERATION_TYPE_REBOOT_INSTANCE: &str = "RebootInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_INSTANCE: &str = "DeleteInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DOMAIN: &str = "CreateDomain";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET: &str = "CreateBucket";

/// The `Operation$operationType` a `DeleteBucket` call logs, from the modelled
/// `OperationType` enum.
pub const OPERATION_TYPE_DELETE_BUCKET: &str = "DeleteBucket";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK: &str = "CreateDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_DISK: &str = "AttachDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_DISK: &str = "DetachDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_STATIC_IP: &str = "DetachStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_INSTANCES_FROM_LOAD_BALANCER: &str =
    "DetachInstancesFromLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE: &str = "CreateRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE_SNAPSHOT: &str =
    "CreateRelationalDatabaseSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_RELATIONAL_DATABASE: &str = "DeleteRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE_FROM_SNAPSHOT: &str =
    "CreateRelationalDatabaseFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_RELATIONAL_DATABASE_PARAMETERS: &str =
    "UpdateRelationalDatabaseParameters";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_REBOOT_RELATIONAL_DATABASE: &str = "RebootRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_RELATIONAL_DATABASE: &str = "UpdateRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_START_RELATIONAL_DATABASE: &str = "StartRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_STOP_RELATIONAL_DATABASE: &str = "StopRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_LOAD_BALANCER: &str = "CreateLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_LOAD_BALANCER: &str = "DeleteLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_INSTANCES_TO_LOAD_BALANCER: &str =
    "AttachInstancesToLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "DeleteLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_SNAPSHOT: &str = "CreateDiskSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_INSTANCE_SNAPSHOT: &str = "CreateInstanceSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_FROM_SNAPSHOT: &str = "CreateDiskFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET_ACCESS_KEY: &str = "CreateBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_BUCKET_ACCESS_KEY: &str = "DeleteBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISTRIBUTION: &str = "CreateDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ALLOCATE_STATIC_IP: &str = "AllocateStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_PUT_ALARM: &str = "PutAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_ALARM: &str = "DeleteAlarm";
/// The `operationType` the log entry of a change to a zone's recordsets carries.
///
/// The modelled `OperationType` enum has `UpdateDomainEntry` and
/// `DeleteDomainEntry` but NO `CreateDomainEntry` member, and
/// `Operation$operationType` targets that enum — so adding an entry to a zone is
/// logged as the modelled recordset update, not as an unmodelled value.
pub const OPERATION_TYPE_UPDATE_DOMAIN_ENTRY: &str = "UpdateDomainEntry";
/// The `operationType` the log entry of an entry's removal from a zone carries,
/// the modelled `OperationType` enum's own `DeleteDomainEntry` member.
pub const OPERATION_TYPE_DELETE_DOMAIN_ENTRY: &str = "DeleteDomainEntry";
/// The `operationType` the log entry of a whole zone's removal carries, the
/// modelled `OperationType` enum's own `DeleteDomain` member.
pub const OPERATION_TYPE_DELETE_DOMAIN: &str = "DeleteDomain";
/// The `operationType` the log entry of an `ExportSnapshot` call carries.
///
/// `Operation$operationType` targets the modelled `OperationType` enum, whose
/// members this module otherwise keeps to (see
/// [`OPERATION_TYPE_UPDATE_DOMAIN_ENTRY`], where a modelled value states exactly
/// the same fact). That enum predates the Amazon EC2 export family — it carries
/// no member for `ExportSnapshot`, and none for `CreateCloudFormationStack`
/// either — and no member it does carry states what an export did: logging one
/// as a snapshot CREATE would misreport the call. A Smithy enum is open on the
/// wire (a client has to tolerate a value it does not know), so the honest entry
/// names the operation that ran, exactly as every other value here does.
pub const OPERATION_TYPE_EXPORT_SNAPSHOT: &str = "ExportSnapshot";
/// The `operationType` the log entry of a `PeerVpc` call carries.
///
/// The modelled `OperationType` enum carries no member for either VPC peering
/// call — it predates them, exactly as it predates the Amazon EC2 export family
/// (see [`OPERATION_TYPE_EXPORT_SNAPSHOT`], where the same reasoning is spelled
/// out) — and no member it does carry states what peering a VPC did. A Smithy
/// enum is open on the wire, so the honest entry names the operation that ran.
pub const OPERATION_TYPE_PEER_VPC: &str = "PeerVpc";
/// See [`OPERATION_TYPE_PEER_VPC`].
pub const OPERATION_TYPE_UNPEER_VPC: &str = "UnpeerVpc";

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

/// The most instance names ONE request may carry — `CreateInstances`'
/// `instanceNames`, or `AttachInstancesToLoadBalancer`'s.
///
/// Emulator resource guard: neither the modelled `StringList` nor the modelled
/// `ResourceNameList` declares a length, so this is where the bound on a
/// caller-sized list of instance names lives.
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

/// The most instance snapshots this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_INSTANCE_SNAPSHOTS: usize = 500;

/// The most managed relational database snapshots this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_RELATIONAL_DATABASE_SNAPSHOTS: usize = 500;

/// The most export snapshot records this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: every `ExportSnapshot` call adds a
/// record, so an export that would exceed it is refused with the modelled
/// `InvalidInputException`.
pub const MAX_EXPORT_SNAPSHOT_RECORDS: usize = 500;

/// The most static IP addresses this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota (Lightsail's own static-IP quota is
/// far tighter, and deliberately not enforced here so a suite can allocate as
/// many as a listing needs): an allocation that would exceed it is refused with
/// the modelled `InvalidInputException`.
pub const MAX_STATIC_IPS: usize = 500;

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

/// The most alarms this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a `PutAlarm` that would create one
/// past it is refused with the modelled `InvalidInputException`.
pub const MAX_ALARMS: usize = 500;

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

/// How many managed relational databases one `GetRelationalDatabases` page
/// carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more databases remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const RELATIONAL_DATABASE_PAGE_SIZE: usize = 10;

/// How many database snapshots one `GetRelationalDatabaseSnapshots` page
/// carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const RELATIONAL_DATABASE_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many instance snapshots one `GetInstanceSnapshots` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const INSTANCE_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many export snapshot records one `GetExportSnapshotRecords` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation either, only that
/// the response carries a `nextPageToken` when more records remain — "A next page
/// token is not returned if there are no more results to display."
pub const EXPORT_SNAPSHOT_RECORD_PAGE_SIZE: usize = 10;

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
/// `id_of` answers a [`std::borrow::Cow`], so an entry whose token is a member it
/// already holds lends it (`Cow::Borrowed`) while one whose token is DERIVED from a
/// member can compute it (`Cow::Owned`) — one walk serves both.
///
/// Shared by every listing this service pages (blueprints, instance bundles, the
/// HTTPS setup history), so each one walks its records exactly the same way.
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
    id_of: impl Fn(&T) -> Option<std::borrow::Cow<'_, str>>,
) -> Option<(Vec<T>, Option<String>)> {
    // A page carries at least one entry, so a walk over a non-empty catalog
    // always advances.
    let page_size = page_size.max(1);
    let start = match page_token {
        None => 0,
        Some(token) => reported
            .iter()
            .position(|entry| id_of(entry).as_deref() == Some(token))?,
    };
    let page = reported
        .iter()
        .skip(start)
        .take(page_size)
        .cloned()
        .collect::<Vec<_>>();
    let next_page_token = reported
        .get(start.saturating_add(page_size))
        .and_then(|entry| id_of(entry).map(std::borrow::Cow::into_owned));
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
        bundle.bundle_id.as_deref().map(std::borrow::Cow::Borrowed)
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

/// The URL of the bucket `name`, as announced to a caller that reached this
/// emulator at `endpoint` (its `Host`/`:authority`).
///
/// `Bucket$url` is "The URL of the bucket", so it has to be an address the caller
/// can actually reach — and what a caller reaches a Lightsail bucket WITH is an
/// S3 client signed with one of the bucket's access keys ("Access keys grant full
/// programmatic access to the specified bucket and its objects"). AWS publishes
/// that address as a DNS name in the Amazon S3 zone that backs the bucket; this
/// emulator owns no DNS zone, and a literal AWS hostname would send the next
/// client out of the emulator entirely (see [`request_endpoint`]), so the address
/// is the PATH-STYLE S3 address of the bucket under the endpoint this request
/// arrived at — `https://<endpoint>/<bucket>` — which is exactly where this
/// emulator's Amazon S3 data plane answers for the bucket this service
/// provisioned there (see [`claim_bucket_object_store`]).
///
/// No Region in the address: a bucket's name alone addresses it in the one object
/// namespace an S3 endpoint serves, which is also why that name has to be free in
/// it before a bucket can be created under it.
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on.
pub fn bucket_url(endpoint: &str, name: &str) -> String {
    format!("https://{endpoint}/{name}")
}

/// What this service records in the Amazon S3 store as the provisioner of a
/// bucket's object store, so a reader of the account's OWN S3 buckets can leave
/// it out (AWS holds a Lightsail bucket in a service-managed account, not in the
/// caller's).
pub const BUCKET_OBJECT_STORE_PROVISIONER: &str = ARN_SERVICE;

/// Claim the object store of a bucket named `name` in `region`: stand the bucket
/// up in this emulator's ONE object store, Amazon S3's own, so the bucket's
/// objects can be written and read with the access keys this service mints for
/// it. `versioned` is whether the bucket was created with object versioning
/// enabled, which is a property of how that store keeps the bucket's objects.
///
/// A Lightsail bucket IS an S3-compatible object store, and the thing that serves
/// the S3 object protocol in this emulator is the Amazon S3 service. So a bucket
/// this service creates is provisioned there rather than copied into a second
/// object map of this service's own: `PutObject`, `GetObject`, `ListObjectsV2`
/// and `DeleteObject` against the bucket are then served from the one store, and
/// what this service reports about the bucket's contents is read from the same
/// place.
///
/// Answers what the claim did ([`BucketCreation::NameTaken`] when the one bucket
/// namespace already holds that name, so the caller refuses the create rather
/// than taking over a bucket that is not its own), or `None` when that store
/// cannot be reached at all.
///
/// LOCK ORDER: this takes the S3 store's lock and holds no other while it does,
/// and it hands back an owned value rather than a guard — so it must be called
/// with NO guard of this service's store held. Keeping the cross-service access
/// one-directional and guard-free is what makes a lock cycle between the two
/// stores unreachable (the S3 service never reaches into this one).
pub fn claim_bucket_object_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    region: &str,
    versioned: bool,
) -> Option<BucketCreation> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    let claimed = store.provision_bucket(name, region, BUCKET_OBJECT_STORE_PROVISIONER);
    // `Bucket$objectVersioning` is `Enabled` when the create asked for versioning of the
    // bucket's objects, and that is what the OBJECT store has to do about it: "All
    // objects added to the bucket receive a unique version ID". A bucket created without
    // it keeps no versions, which is the state a fresh bucket is in there.
    //
    // The bucket is there: this guard is still held from the claim that just recorded it.
    // A store that nevertheless does not hold it would make the control plane report
    // versioning its data plane does not do, so say so rather than passing it over.
    if versioned && claimed == BucketCreation::Created {
        match store.bucket_mut(name) {
            Some(bucket) => bucket.set_versioning(
                ::service_com_amazonaws_s3_amazons3::state::VERSIONING_ENABLED_STATUS,
                None,
            ),
            None => ::tracing::warn!(
                bucket_name = %name,
                "the object store did not hold the bucket it had just recorded, so object \
                 versioning was not enabled on it"
            ),
        }
    }
    Some(claimed)
}

/// Release the object store of the bucket named `name`, with every object it
/// still held: what a delete of the bucket does, because "when you delete your
/// bucket, the bucket name is released and can be reused for a new bucket"
/// (`DeleteBucket`).
///
/// `true` when the object store held such a bucket, `false` when it did not, and
/// `None` when that store cannot be reached at all. The same LOCK ORDER as
/// [`claim_bucket_object_store`].
pub fn release_bucket_object_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
) -> Option<bool> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    Some(store.remove_bucket(name))
}

/// How many object keys the bucket named `name` holds in its object store — the
/// keys with a current object plus the keys only noncurrent versions are left of,
/// since a stored version is an object the bucket still holds.
///
/// A pure READ of the object store, for the one thing this service's control
/// plane has to know about a bucket's contents: whether the bucket is empty, which
/// is what decides a `DeleteBucket` without `forceDelete`. A name the object store
/// holds no bucket for holds no objects, and answers `0`: there is nothing there to
/// block a delete.
///
/// `None` when the object store cannot be reached at all. The same LOCK ORDER as
/// [`claim_bucket_object_store`].
pub fn bucket_object_count<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) -> Option<usize> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    Some(
        store
            .bucket(name)
            .map_or(0, |bucket| bucket.version_keys().len()),
    )
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
    /// and service name, null `lastUsedDate`): a request signed with this key is
    /// served by the emulator's Amazon S3 data plane, which keeps no record of
    /// WHICH credential signed it — nothing in this emulator records the use of a
    /// credential — so there is no use instant to report, and claiming one for
    /// work nothing recorded would be worse than saying the key has not been used.
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
    /// request arrived at (see [`bucket_url`]). A request that names no host at
    /// all names no endpoint to announce, so `url` is omitted rather than filled
    /// with an address nothing serves.
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
            url: endpoint.map(|endpoint| bucket_url(endpoint, &self.name)),
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
        blueprint.blueprint_id.as_deref().map(std::borrow::Cow::Borrowed)
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

    /// This blueprint's major version as the NUMBERS it is written with (`8.4`
    /// -> `[8, 4]`), so two major versions of one engine order by version rather
    /// than by the text they are spelled in — where `13` would sort before
    /// `8.4`. A component that is not a number counts as zero, so an entry added
    /// to the catalog in any other form still orders rather than panicking.
    fn major_version_numbers(&self) -> Vec<u32> {
        self.major_version
            .split('.')
            .map(|component| component.parse().unwrap_or(0))
            .collect()
    }

    /// Whether this blueprint describes a LATER major version of the engine than
    /// `other` does: a database's major version moves forward (`8.0` -> `8.4`),
    /// which is the direction `UpdateRelationalDatabase$relationalDatabaseBlueprintId`
    /// updates it in.
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.major_version_numbers() > other.major_version_numbers()
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

    /// Whether this plan is SMALLER than `other` — any of the three hardware
    /// dimensions the catalog publishes (vCPU count, RAM, disk size) below
    /// `other`'s.
    ///
    /// A restore is held to this: "When creating a new database from a snapshot,
    /// you cannot choose a bundle that is smaller than the bundle of the source
    /// database" (the modelled `CreateRelationalDatabaseFromSnapshot$relationalDatabaseBundleId`
    /// documentation). The comparison is over the published HARDWARE rather than
    /// over the catalog's order, so a plan the catalog gains sits in the ordering
    /// by what it offers; two plans with the same hardware (a standard plan and
    /// its high-availability twin) are neither smaller than the other, which is
    /// what makes a restore onto a standby-backed plan of the same size legal.
    pub fn is_smaller_than(&self, other: &Self) -> bool {
        self.cpu_count < other.cpu_count
            || self.ram_size_in_gb < other.ram_size_in_gb
            || self.disk_size_in_gb < other.disk_size_in_gb
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
/// `CreateRelationalDatabase`'s and `CreateRelationalDatabaseFromSnapshot`'s
/// `relationalDatabaseBundleId`.
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
// Relational database runtime parameters
// ---------------------------------------------------------------------------

// `GetRelationalDatabaseParameters` "returns all of the runtime parameters
// offered by the underlying database software, or engine, for a specific
// database" — so the parameter SET is the engine's, published data like the
// blueprint and bundle catalogs above, while which set a database has and what
// the memory-derived values in it come to are the database's own stored facts
// (its blueprint's engine, its bundle's RAM). Nothing here is per-account state:
// reporting it leaves every later caller the answer it would have had.

/// The `applyType` of a parameter a running engine can take on without being
/// restarted, as RDS and Lightsail report it.
pub const PARAMETER_APPLY_TYPE_DYNAMIC: &str = "dynamic";

/// The `applyType` of a parameter the engine only reads at startup, so a change
/// to it takes effect at the next reboot.
pub const PARAMETER_APPLY_TYPE_STATIC: &str = "static";

/// The `applyMethod` of a parameter a change takes effect on at once: the
/// modelled `RelationalDatabaseParameter$applyMethod` is "Indicates when
/// parameter updates are applied. Can be `immediate` or `pending-reboot`", and a
/// [dynamic](PARAMETER_APPLY_TYPE_DYNAMIC) parameter is exactly one a change can
/// be applied to immediately.
pub const PARAMETER_APPLY_METHOD_IMMEDIATE: &str = "immediate";

/// The `applyMethod` of a parameter a change waits for the next reboot on: a
/// [static](PARAMETER_APPLY_TYPE_STATIC) parameter, which is what this
/// operation's own documentation means by "whether changes require a reboot".
pub const PARAMETER_APPLY_METHOD_PENDING_REBOOT: &str = "pending-reboot";

/// The `dataType` of a parameter that is on or off.
pub const PARAMETER_DATA_TYPE_BOOLEAN: &str = "boolean";

/// The `dataType` of a parameter that takes a whole number.
pub const PARAMETER_DATA_TYPE_INTEGER: &str = "integer";

/// The `dataType` of a parameter that takes a fractional number.
pub const PARAMETER_DATA_TYPE_FLOAT: &str = "float";

/// The `dataType` of a parameter that takes free text.
pub const PARAMETER_DATA_TYPE_STRING: &str = "string";

/// Bytes in a gibibyte: the unit a bundle's `ramSizeInGb` is measured in, which
/// the memory-derived parameter values below are computed against.
const BYTES_PER_GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The memory, in bytes, of a database running on a bundle with `ram_size_in_gb`
/// gibibytes of RAM.
fn relational_database_memory_bytes(ram_size_in_gb: f32) -> i64 {
    (f64::from(ram_size_in_gb) * BYTES_PER_GIB).round() as i64
}

/// What a runtime parameter's CURRENT value is on a database of a given size.
///
/// A managed database runs with the service's own parameter group, and a
/// parameter in it is in one of exactly these three positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationalDatabaseParameterValue {
    /// The group pins nothing: the parameter runs at the database engine's own
    /// default. RDS reports such a parameter with no `ParameterValue` at all —
    /// its source is the engine, not the group — and so does Lightsail: the
    /// `get-relational-database-parameters` example in the AWS CLI reference
    /// shows entries carrying every other member and no `parameterValue`.
    EngineDefault,
    /// The group pins a fixed value, the same on every database.
    Fixed(&'static str),
    /// The group derives the value from the memory of the database's own
    /// hardware: `min(memory_bytes * numerator / denominator, cap)`.
    ///
    /// This is how RDS's default parameter groups express a memory-sized
    /// parameter — as a formula over `DBInstanceClassMemory` rather than as a
    /// number — and the formula is published per parameter (see the entries that
    /// use it below). The formula's literal text is an RDS implementation detail
    /// of its parameter-group format; what a database actually RUNS with is the
    /// formula resolved against its own hardware, so that resolved number is
    /// what this emulator reports, computed from the bundle the database is
    /// stored with rather than from any canned body.
    FromMemory {
        /// The factor the memory is multiplied by.
        numerator: i64,
        /// The divisor the product is divided by — the unit of the parameter
        /// (bytes, 8 kB pages, bytes per connection).
        denominator: i64,
        /// The ceiling the formula is capped at, where the published formula
        /// carries one.
        cap: Option<i64>,
    },
}

impl RelationalDatabaseParameterValue {
    /// This value on a database whose bundle carries `ram_size_in_gb` gibibytes
    /// of RAM, or `None` when the parameter is left at the engine's own default
    /// and so reports no value.
    pub fn resolve(self, ram_size_in_gb: f32) -> Option<String> {
        match self {
            Self::EngineDefault => None,
            Self::Fixed(value) => Some(value.to_string()),
            Self::FromMemory {
                numerator,
                denominator,
                cap,
            } => {
                let memory = relational_database_memory_bytes(ram_size_in_gb);
                // A denominator of zero would be a malformed catalog entry, not
                // a request the caller can make; clamping it keeps the reporting
                // path total.
                let value = memory.saturating_mul(numerator) / denominator.max(1);
                Some(cap.map_or(value, |cap| value.min(cap)).to_string())
            }
        }
    }
}

/// One runtime parameter of a database engine: everything the modelled
/// `RelationalDatabaseParameter` reports about it except the value, which
/// depends on the database ([`RelationalDatabaseParameterValue`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationalDatabaseParameterSpec {
    /// `RelationalDatabaseParameter$parameterName` — "the name of the
    /// parameter", spelled as the engine itself spells it.
    pub parameter_name: &'static str,
    /// Where this parameter's current value comes from.
    pub value: RelationalDatabaseParameterValue,
    /// `RelationalDatabaseParameter$allowedValues` — "the valid range of values
    /// for the parameter". `None` where the parameter admits any value of its
    /// data type and the engine publishes no range, and then the member is left
    /// out rather than reported empty.
    pub allowed_values: Option<&'static str>,
    /// `RelationalDatabaseParameter$applyType` — "the engine-specific parameter
    /// type": [dynamic](PARAMETER_APPLY_TYPE_DYNAMIC) when the running engine
    /// can take a change on, [static](PARAMETER_APPLY_TYPE_STATIC) when it only
    /// reads the parameter at startup.
    pub apply_type: &'static str,
    /// `RelationalDatabaseParameter$dataType` — "the valid data type for the
    /// parameter", one of the `PARAMETER_DATA_TYPE_*` values.
    pub data_type: &'static str,
    /// `RelationalDatabaseParameter$description` — "a description of the
    /// parameter", from the engine's own documentation of it.
    pub description: &'static str,
    /// `RelationalDatabaseParameter$isModifiable` — "whether the parameter can
    /// be modified". False for a parameter the managed service owns and does not
    /// let the customer change.
    pub is_modifiable: bool,
}

impl RelationalDatabaseParameterSpec {
    /// When a change to this parameter takes effect.
    ///
    /// DERIVED from [`Self::apply_type`] rather than tracked beside it: a
    /// dynamic parameter is one the running engine can take a change on
    /// ([`PARAMETER_APPLY_METHOD_IMMEDIATE`]), and a static one is only read at
    /// startup, so a change to it can only land at the next reboot
    /// ([`PARAMETER_APPLY_METHOD_PENDING_REBOOT`]) — which is what this
    /// operation's "whether changes require a reboot" reports. Keeping the two
    /// apart would let them disagree.
    pub fn apply_method(&self) -> &'static str {
        if self.apply_type == PARAMETER_APPLY_TYPE_DYNAMIC {
            PARAMETER_APPLY_METHOD_IMMEDIATE
        } else {
            PARAMETER_APPLY_METHOD_PENDING_REBOOT
        }
    }

    /// This parameter as the modelled `RelationalDatabaseParameter`, on a
    /// database whose bundle carries `ram_size_in_gb` gibibytes of RAM.
    pub fn view(&self, ram_size_in_gb: f32) -> RelationalDatabaseParameter {
        RelationalDatabaseParameter {
            parameter_name: Some(self.parameter_name.to_string()),
            parameter_value: self.value.resolve(ram_size_in_gb),
            allowed_values: self.allowed_values.map(str::to_string),
            apply_method: Some(self.apply_method().to_string()),
            apply_type: Some(self.apply_type.to_string()),
            data_type: Some(self.data_type.to_string()),
            description: Some(self.description.to_string()),
            is_modifiable: Some(self.is_modifiable),
        }
    }
}

/// The runtime parameters of the MySQL engine, as a Lightsail MySQL database
/// reports them.
///
/// SOURCE: each entry's default, range and dynamic/static nature is the one the
/// MySQL manual's "Server System Variables" publishes for that variable, and the
/// description is that variable's own; the two memory-derived values and the
/// pinned time zone are what the managed service's parameter group sets on top
/// of the engine's defaults — RDS publishes `innodb_buffer_pool_size` as
/// `{DBInstanceClassMemory*3/4}` and `max_connections` as
/// `{DBInstanceClassMemory/12582880}` in its MySQL default parameter group, and
/// documents that a DB instance's time zone is UTC unless the customer changes
/// it. A parameter the group leaves alone reports no value
/// ([`RelationalDatabaseParameterValue::EngineDefault`]).
///
/// The ORDER is this slice's own — by parameter name, as RDS lists a parameter
/// group — and it is stable, so two reads answer identically and a page token
/// names the same entry in both.
pub const MYSQL_RELATIONAL_DATABASE_PARAMETERS: &[RelationalDatabaseParameterSpec] = &[
    RelationalDatabaseParameterSpec {
        parameter_name: "allow-suspicious-udfs",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Controls whether user-defined functions that have only an xxx symbol for the main function can be loaded",
        // The managed service loads no such function and does not let the
        // customer ask for one.
        is_modifiable: false,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "auto_increment_increment",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1-65535"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Controls the increment between successive AUTO_INCREMENT values",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "autocommit",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Sets the autocommit mode, in which every statement is committed as a transaction of its own",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "innodb_buffer_pool_size",
        // RDS: `{DBInstanceClassMemory*3/4}` — three quarters of the hardware's
        // memory, in bytes.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 3,
            denominator: 4,
            cap: None,
        },
        allowed_values: Some("5242880-9223372036854775807"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The size in bytes of the buffer pool, the memory area where InnoDB caches table and index data",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "long_query_time",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0-31536000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_FLOAT,
        description: "The minimum execution time in seconds for a statement to be logged as slow",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "max_allowed_packet",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1024-1073741824"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The maximum size in bytes of one packet, or of any generated or intermediate string",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "max_connections",
        // RDS: `{DBInstanceClassMemory/12582880}` — one connection per 12 MB of
        // the hardware's memory.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 1,
            denominator: 12_582_880,
            cap: None,
        },
        allowed_values: Some("1-100000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The maximum permitted number of simultaneous client connections",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "slow_query_log",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Enables or disables the slow query log",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "time_zone",
        // The managed service runs a database in UTC unless the customer
        // changes it.
        value: RelationalDatabaseParameterValue::Fixed("UTC"),
        // A time zone name, not a range: the engine publishes none.
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_STRING,
        description: "The time zone the server interprets and reports date and time values in",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "wait_timeout",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1-31536000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The number of seconds the server waits for activity on a noninteractive connection before closing it",
        is_modifiable: true,
    },
];

/// The runtime parameters of the PostgreSQL engine, as a Lightsail PostgreSQL
/// database reports them.
///
/// SOURCE: each entry's range, unit and reset-required (static) nature is the
/// one the PostgreSQL documentation's "Server Configuration" publishes for that
/// parameter, and the description is that parameter's own; the two
/// memory-derived values and the pinned time zone are what the managed service's
/// parameter group sets on top of them — RDS publishes `shared_buffers` as
/// `{DBInstanceClassMemory/32768}` (its unit is an 8 kB page, so that is a
/// quarter of the hardware's memory) and `max_connections` as
/// `LEAST({DBInstanceClassMemory/9531392},5000)` in its PostgreSQL default
/// parameter group, and documents that a DB instance's time zone is UTC unless
/// the customer changes it.
///
/// The ORDER is this slice's own, by parameter name, for the same reason as the
/// MySQL catalog's.
pub const POSTGRES_RELATIONAL_DATABASE_PARAMETERS: &[RelationalDatabaseParameterSpec] = &[
    RelationalDatabaseParameterSpec {
        parameter_name: "autovacuum",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Controls whether the server runs the autovacuum launcher daemon",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "client_encoding",
        value: RelationalDatabaseParameterValue::EngineDefault,
        // A character set name, not a range: the parameter admits every
        // encoding the server was built with.
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_STRING,
        description: "Sets the client-side character set encoding",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "log_min_duration_statement",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("-1-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the minimum execution time in milliseconds above which a statement is logged; -1 disables the logging",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "maintenance_work_mem",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1024-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum memory in kilobytes a maintenance operation such as VACUUM or CREATE INDEX may use",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "max_connections",
        // RDS: `LEAST({DBInstanceClassMemory/9531392},5000)` — one connection
        // per about 9 MB of the hardware's memory, never more than 5000.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 1,
            denominator: 9_531_392,
            cap: Some(5000),
        },
        allowed_values: Some("1-262143"),
        // PostgreSQL reads `max_connections` only at startup.
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum number of concurrent connections to the database server",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "shared_buffers",
        // RDS: `{DBInstanceClassMemory/32768}`, in the parameter's own unit of
        // 8 kB pages.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 1,
            denominator: 32_768,
            cap: None,
        },
        allowed_values: Some("16-1073741823"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the number of 8 kB shared memory buffers the server uses",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "statement_timeout",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum allowed duration in milliseconds of any statement; 0 disables the timeout",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "timezone",
        value: RelationalDatabaseParameterValue::Fixed("UTC"),
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_STRING,
        description: "Sets the time zone the server displays and interprets time stamps in",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "work_mem",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("64-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum memory in kilobytes a query operation such as a sort or a hash table may use before writing to disk",
        is_modifiable: true,
    },
];

/// How many parameters one `GetRelationalDatabaseParameters` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more parameters remain. It is above
/// either engine's parameter set, so a database's whole set is answered in one
/// page, and the paging walk still holds for a set that outgrows it.
pub const RELATIONAL_DATABASE_PARAMETER_PAGE_SIZE: usize = 50;

/// The runtime parameters the database engine `engine` offers: the catalog of
/// the engine the database's blueprint names.
///
/// The engine is matched the way [`relational_database_port`] matches it — on
/// the engine FAMILY — so a blueprint of any PostgreSQL major version resolves
/// to the PostgreSQL set and everything else to the MySQL one, which is the
/// engine the modelled `RelationalDatabaseEngine` enum names.
pub fn relational_database_parameter_specs(
    engine: &str,
) -> &'static [RelationalDatabaseParameterSpec] {
    if engine.to_ascii_lowercase().contains("postgres") {
        POSTGRES_RELATIONAL_DATABASE_PARAMETERS
    } else {
        MYSQL_RELATIONAL_DATABASE_PARAMETERS
    }
}

/// One page of a database's runtime parameters: the parameters it carries and
/// the token that opens the page after it.
#[derive(Debug, Clone)]
pub struct RelationalDatabaseParameterPage {
    /// The page's parameters, as the modelled view.
    pub parameters: Vec<RelationalDatabaseParameter>,
    /// The name of the first parameter of the NEXT page, present only while
    /// parameters remain.
    pub next_page_token: Option<String>,
}

/// One page of `specs` as a database with `ram_size_in_gb` gibibytes of RAM
/// reports them: the slice from the parameter `page_token` names onward, at most
/// `page_size` entries, with `next_page_token` naming the first parameter of the
/// page after it.
///
/// `None` when `page_token` names no parameter of the set — the caller refuses it
/// with the modelled `InvalidInputException`. A token names a PARAMETER rather
/// than an index, so a set that gained or lost an entry between two pages cannot
/// make the walk skip or repeat another one. An absent — or empty — token asks
/// for the first page.
pub fn relational_database_parameter_page(
    specs: &[RelationalDatabaseParameterSpec],
    ram_size_in_gb: f32,
    page_token: Option<&str>,
    page_size: usize,
) -> Option<RelationalDatabaseParameterPage> {
    // A page carries at least one entry: a zero-length page would name its own
    // first entry as the next token and never advance.
    let page_size = page_size.max(1);

    let start = match page_token.filter(|token| !token.is_empty()) {
        None => 0,
        Some(token) => specs
            .iter()
            .position(|spec| spec.parameter_name == token)?,
    };

    let mut parameters = Vec::new();
    let mut next_page_token = None;
    for spec in &specs[start..] {
        if parameters.len() == page_size {
            next_page_token = Some(spec.parameter_name.to_string());
            break;
        }
        parameters.push(spec.view(ram_size_in_gb));
    }

    Some(RelationalDatabaseParameterPage {
        parameters,
        next_page_token,
    })
}

/// The parameter names a `GetRelationalDatabaseParameters` page token may name
/// out of `specs`, for a refusal that has to tell the caller what to send
/// instead.
pub fn relational_database_parameter_names(specs: &[RelationalDatabaseParameterSpec]) -> String {
    specs
        .iter()
        .map(|spec| spec.parameter_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The parameter `parameter_name` names in the set the engine `engine` offers,
/// or `None` when that engine offers no such parameter — a parameter of ANOTHER
/// engine included, since a database has only its own engine's set.
pub fn relational_database_parameter_spec(
    engine: &str,
    parameter_name: &str,
) -> Option<RelationalDatabaseParameterSpec> {
    relational_database_parameter_specs(engine)
        .iter()
        .find(|spec| spec.parameter_name == parameter_name)
        .copied()
}

/// The most parameters one `UpdateRelationalDatabaseParameters` request may
/// carry.
///
/// A Lightsail managed database is an RDS DB instance and this call is RDS's
/// `ModifyDBParameterGroup`, which documents "A maximum of 20 parameters can be
/// modified in a single request" — so this is AWS's own bound on the
/// caller-sized list, not an emulator guard.
pub const MAX_RELATIONAL_DATABASE_PARAMETER_UPDATES: usize = 20;

/// The `parameterApplyStatus` of a database holding a parameter value the
/// RUNNING engine has not loaded yet, beside
/// [`PARAMETER_APPLY_STATUS_IN_SYNC`].
///
/// A Lightsail managed database is an RDS DB instance and the member carries
/// RDS's own parameter group status. `RebootRelationalDatabase` is what clears
/// it: "Parameters marked with a pending-reboot apply type are applied only
/// after the database is rebooted using the reboot relational database
/// operation" (the modelled `UpdateRelationalDatabaseParameters`
/// documentation).
pub const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// A parameter value a database's parameter group carries because an
/// `UpdateRelationalDatabaseParameters` call set it.
///
/// Only a parameter of the database's own engine set can be set, so the map of
/// these a database holds is bounded by that set however many updates a caller
/// makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredParameterOverride {
    /// The value the update set, which every later read of the parameter
    /// reports: the modification reaches the parameter group at once.
    pub value: String,
    /// The apply method the update was made with, which the parameter reports
    /// as its `applyMethod` from then on.
    pub apply_method: &'static str,
    /// Whether the RUNNING engine is still waiting for a reboot to load this
    /// value — true for a value applied `pending-reboot`. It is what puts the
    /// database in [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`], and the engine
    /// coming back up from a `RebootRelationalDatabase` restart is what clears
    /// it ([`StoredRelationalDatabase::load_pending_parameters`]).
    pub pending_reboot: bool,
}

/// One resolved parameter modification: which parameter of the engine's set an
/// `UpdateRelationalDatabaseParameters` entry named, the value it sets and the
/// method it is applied with.
///
/// An update carries only these three: every other member of the modelled
/// `RelationalDatabaseParameter` DESCRIBES the parameter (its type, its allowed
/// values, its description, whether it is modifiable) and is the engine's to
/// report, so a request that spells one differently cannot change what the
/// service reports about the parameter.
#[derive(Debug, Clone)]
pub struct RelationalDatabaseParameterUpdate {
    /// The parameter the update names.
    pub spec: RelationalDatabaseParameterSpec,
    /// The value to set, already checked against that parameter.
    pub value: String,
    /// [`PARAMETER_APPLY_METHOD_IMMEDIATE`] or
    /// [`PARAMETER_APPLY_METHOD_PENDING_REBOOT`].
    pub apply_method: &'static str,
}

/// The modification `requested` makes to a parameter of `engine`, or a refusal
/// naming the member that is wrong and the constraint it broke.
///
/// `member` is how the request spells the entry under check (`parameters[0]`),
/// so a refusal points the caller at the entry it must fix.
pub fn relational_database_parameter_update(
    engine: &str,
    member: &str,
    requested: &RelationalDatabaseParameter,
) -> Result<RelationalDatabaseParameterUpdate, String> {
    let parameter_name = requested
        .parameter_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            format!("{member}.parameterName is required: it names the parameter to update")
        })?;
    let spec = relational_database_parameter_spec(engine, parameter_name).ok_or_else(|| {
        format!(
            "{member}.parameterName '{parameter_name}' is not a parameter the {engine} engine \
             offers; use GetRelationalDatabaseParameters to list this database's parameters \
             ([{}])",
            relational_database_parameter_names(relational_database_parameter_specs(engine))
        )
    })?;
    if !spec.is_modifiable {
        return Err(format!(
            "{member}.parameterName '{parameter_name}' is not modifiable: the {engine} engine \
             reports isModifiable false for it"
        ));
    }

    let value = requested.parameter_value.as_deref().ok_or_else(|| {
        format!(
            "{member}.parameterValue is required: it carries the value to set on parameter \
             '{parameter_name}'"
        )
    })?;
    validate_relational_database_parameter_value(
        &spec,
        &format!("{member}.parameterValue"),
        value,
    )?;

    // "Indicates when parameter updates are applied. Can be `immediate` or
    // `pending-reboot`": an update that names neither is applied the way the
    // parameter's own apply type admits, and one that names `immediate` for a
    // parameter the engine reads only at startup is refused rather than
    // silently held back.
    let apply_method = match requested.apply_method.as_deref() {
        None => spec.apply_method(),
        Some(PARAMETER_APPLY_METHOD_PENDING_REBOOT) => PARAMETER_APPLY_METHOD_PENDING_REBOOT,
        Some(PARAMETER_APPLY_METHOD_IMMEDIATE) => {
            if spec.apply_type != PARAMETER_APPLY_TYPE_DYNAMIC {
                return Err(format!(
                    "{member}.applyMethod '{PARAMETER_APPLY_METHOD_IMMEDIATE}' cannot apply \
                     parameter '{parameter_name}', whose applyType is '{}': a change to it takes \
                     effect only when the database is rebooted, so apply it with \
                     '{PARAMETER_APPLY_METHOD_PENDING_REBOOT}'",
                    spec.apply_type
                ));
            }
            PARAMETER_APPLY_METHOD_IMMEDIATE
        }
        Some(other) => {
            return Err(format!(
                "{member}.applyMethod '{other}' is not an apply method: it must be \
                 '{PARAMETER_APPLY_METHOD_IMMEDIATE}' or \
                 '{PARAMETER_APPLY_METHOD_PENDING_REBOOT}'"
            ))
        }
    };

    Ok(RelationalDatabaseParameterUpdate {
        spec,
        value: value.to_string(),
        apply_method,
    })
}

/// `value` checked against `spec`'s own data type and published value set, or a
/// refusal naming the constraint it broke.
///
/// `member` is how the request spells the member carrying the value, so the
/// refusal points at it.
pub fn validate_relational_database_parameter_value(
    spec: &RelationalDatabaseParameterSpec,
    member: &str,
    value: &str,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!(
            "{member} is empty: parameter '{}' takes a {} value",
            spec.parameter_name, spec.data_type
        ));
    }
    // A parameter's `dataType` is "the valid data type for the parameter", so a
    // value that is not of that type is refused before its range is looked at.
    let typed = match spec.data_type {
        PARAMETER_DATA_TYPE_INTEGER | PARAMETER_DATA_TYPE_BOOLEAN => {
            let number: i64 = value.parse().map_err(|_| {
                format!(
                    "{member} '{value}' is not a whole number: parameter '{}' has dataType {}",
                    spec.parameter_name, spec.data_type
                )
            })?;
            number as f64
        }
        PARAMETER_DATA_TYPE_FLOAT => value.parse::<f64>().map_err(|_| {
            format!(
                "{member} '{value}' is not a number: parameter '{}' has dataType {}",
                spec.parameter_name, spec.data_type
            )
        })?,
        // Free text: there is no type to fail, only the value set below.
        _ => f64::NAN,
    };

    // `allowedValues` is "the valid range of values for the parameter", absent
    // where the engine publishes none — and then any value of the data type is
    // accepted.
    let Some(allowed_values) = spec.allowed_values else {
        return Ok(());
    };
    let admitted = allowed_values.split(',').any(|allowed| {
        let allowed = allowed.trim();
        // An entry is either a value the parameter takes verbatim or a
        // `low-high` range a numeric value may fall inside.
        allowed == value
            || parse_allowed_range(allowed).is_some_and(|(low, high)| (low..=high).contains(&typed))
    });
    if admitted {
        return Ok(());
    }
    Err(format!(
        "{member} '{value}' is outside the allowedValues of parameter '{}' ({allowed_values})",
        spec.parameter_name
    ))
}

/// The `low-high` range `allowed` names, or `None` when it names a single value.
///
/// A lower bound may itself be negative (`-1-2147483647`), so the separator is
/// the first `-` that is not the sign at the front.
fn parse_allowed_range(allowed: &str) -> Option<(f64, f64)> {
    let separator = allowed
        .char_indices()
        .skip(1)
        .find(|(_, character)| *character == '-')
        .map(|(index, _)| index)?;
    let low = allowed[..separator].parse::<f64>().ok()?;
    let high = allowed[separator + 1..].parse::<f64>().ok()?;
    Some((low, high))
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

// The shapes and the value catalog THIS slice reads, imported beside it rather
// than in the module's one shared `use` at the top: that list is where every
// concurrent change to this service adds a name, so a name added there collides
// with every other change in the same window, while these lines are the
// firewall's own. (The same reasoning the router's `tests/all.rs` records for
// where a suite is declared.)
use crate::ops::types::{InstancePortState, PortInfo, NETWORK_PROTOCOL_VALUES};

/// The `operationType` the log entry of each firewall change carries, from the
/// modelled `OperationType` enum — kept with the firewall slice they belong to,
/// for the reason the `use` above gives.
pub const OPERATION_TYPE_OPEN_INSTANCE_PUBLIC_PORTS: &str = "OpenInstancePublicPorts";
/// See [`OPERATION_TYPE_OPEN_INSTANCE_PUBLIC_PORTS`].
pub const OPERATION_TYPE_CLOSE_INSTANCE_PUBLIC_PORTS: &str = "CloseInstancePublicPorts";
/// See [`OPERATION_TYPE_OPEN_INSTANCE_PUBLIC_PORTS`].
pub const OPERATION_TYPE_PUT_INSTANCE_PUBLIC_PORTS: &str = "PutInstancePublicPorts";

/// The most inbound firewall rules one instance's firewall may hold.
///
/// Emulator resource guard, NOT an AWS quota: every `OpenInstancePublicPorts`
/// and `PutInstancePublicPorts` call grows the rule set, so a call that would
/// take it past this is refused with the modelled `InvalidInputException`. Kept
/// here rather than with the store's other bounds, for the reason the `use`
/// above gives.
pub const MAX_PORT_RULES: usize = 200;

/// The most addresses one firewall rule's `cidrs`, `ipv6Cidrs` or
/// `cidrListAliases` list may carry.
///
/// Emulator resource guard: the modelled `StringList` declares no length, so
/// this is where the bound on a caller-sized address list lives.
pub const MAX_PORT_CIDRS: usize = 50;

/// The most `portInfos` entries one `PutInstancePublicPorts` request may carry.
///
/// Emulator resource guard: the modelled `PortInfoList` declares no length, and
/// this bound is what caps the WORK a request buys, before any entry of it is
/// looked at. It is deliberately above [`MAX_PORT_RULES`] — twice it — because a
/// request may name one rule more than once and such entries collapse, so the
/// count of entries and the count of rules are not the same bound.
pub const MAX_PORT_INFOS_PER_REQUEST: usize = 2 * MAX_PORT_RULES;

/// The management port a new instance's firewall opens: SSH for a Linux/Unix
/// blueprint, RDP for a Windows one.
const SSH_PORT: i32 = 22;
/// See [`SSH_PORT`].
const RDP_PORT: i32 = 3389;
/// The web port a new instance's firewall opens.
const HTTP_PORT: i32 = 80;

/// The `PortState` every rule an instance's firewall holds reports, from the
/// modelled `InstancePortState$state` documentation: "The port state for
/// Lightsail instances is always `open`." A port that is not open is not a rule
/// of the firewall at all — `CloseInstancePublicPorts` removes it.
pub const PORT_STATE_OPEN: &str = "open";

/// The `accessDirection` every rule reports, from the modelled
/// `InstancePortInfo$accessDirection` documentation: "Lightsail currently
/// supports only `inbound` access direction."
pub const ACCESS_DIRECTION_INBOUND: &str = "inbound";

/// The `accessType` every rule these operations manage reports: they are the
/// instance's PUBLIC ports (`OpenInstancePublicPorts` /
/// `PutInstancePublicPorts`), and `Public` is the modelled `PortAccessType`
/// value for that.
pub const PORT_ACCESS_TYPE_PUBLIC: &str = "Public";

/// The protocol a rule carries when the request named none.
///
/// CHOSEN: `PortInfo$protocol` is optional and carries no modelled `@default`,
/// and `tcp` is the protocol this service's own default firewall rules use (see
/// [`default_port_rules`]), so it is what a rule that names no protocol means.
pub const DEFAULT_NETWORK_PROTOCOL: &str = "tcp";

/// Every IPv4 address, in the CIDR notation `PortInfo$cidrs` takes.
pub const ANY_IPV4_CIDR: &str = "0.0.0.0/0";
/// Every IPv6 address, in the CIDR notation `PortInfo$ipv6Cidrs` takes.
pub const ANY_IPV6_CIDR: &str = "::/0";

/// The `accessFrom` a rule open to every address reports, from the modelled
/// `InstancePortInfo$accessFrom` documentation, whose example is exactly this
/// text (`Anywhere (0.0.0.0/0)`).
pub const ACCESS_FROM_ANYWHERE_IPV4: &str = "Anywhere (0.0.0.0/0)";
/// The same for a rule open to every IPv6 address only.
pub const ACCESS_FROM_ANYWHERE_IPV6: &str = "Anywhere (::/0)";
/// The `accessFrom` a rule open to named addresses reports: the same member's
/// documentation — "or `Custom` if a specific IP address or range of IP
/// addresses is allowed".
pub const ACCESS_FROM_CUSTOM: &str = "Custom";

/// The only `cidrListAliases` entry Lightsail accepts, from the modelled
/// `PortInfo$cidrListAliases` documentation: "The only alias currently supported
/// is `lightsail-connect`, which allows IP addresses of the browser-based
/// RDP/SSH client in the Lightsail console to connect to your instance."
pub const CIDR_LIST_ALIAS_LIGHTSAIL_CONNECT: &str = "lightsail-connect";

/// One inbound rule of an instance's firewall: the port range and protocol it
/// opens, and the addresses allowed to reach the instance through it.
///
/// The port range and the protocol together IDENTIFY the rule — that is the key
/// `OpenInstancePublicPorts` upserts, `CloseInstancePublicPorts` removes and
/// `PutInstancePublicPorts` replaces the whole set of — and the addresses are
/// what the caller asked for, kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPortRule {
    pub from_port: i32,
    pub to_port: i32,
    pub protocol: String,
    pub cidrs: Vec<String>,
    pub ipv6_cidrs: Vec<String>,
    pub cidr_list_aliases: Vec<String>,
}

impl StoredPortRule {
    /// The key that identifies this rule within one firewall: its port range and
    /// its protocol.
    pub fn key(&self) -> (i32, i32, &str) {
        (self.from_port, self.to_port, self.protocol.as_str())
    }

    /// Whether this rule opens the same ports, over the same protocol, as
    /// `other` — i.e. whether the two are the same rule of one firewall.
    pub fn is_same_rule(&self, other: &Self) -> bool {
        self.key() == other.key()
    }

    /// The rule as the modelled `InstancePortState` a firewall read reports.
    pub fn port_state_view(&self) -> InstancePortState {
        InstancePortState {
            from_port: self.from_port,
            to_port: self.to_port,
            protocol: Some(self.protocol.clone()),
            state: Some(PORT_STATE_OPEN.to_string()),
            cidrs: list_or_absent(&self.cidrs),
            ipv6_cidrs: list_or_absent(&self.ipv6_cidrs),
            cidr_list_aliases: list_or_absent(&self.cidr_list_aliases),
        }
    }

    /// The rule as the modelled `InstancePortInfo` an instance's `networking`
    /// reports.
    ///
    /// `accessFrom` and `accessType` are DERIVED from the rule's own addresses,
    /// so one rule reads the same way through both views.
    pub fn port_info_view(&self) -> InstancePortInfo {
        InstancePortInfo {
            from_port: self.from_port,
            to_port: self.to_port,
            protocol: Some(self.protocol.clone()),
            access_from: Some(self.access_from().to_string()),
            access_type: Some(PORT_ACCESS_TYPE_PUBLIC.to_string()),
            // `commonName` is "The common name of the port information", which
            // this service has no source for — absent rather than invented.
            common_name: None,
            access_direction: Some(ACCESS_DIRECTION_INBOUND.to_string()),
            cidrs: list_or_absent(&self.cidrs),
            ipv6_cidrs: list_or_absent(&self.ipv6_cidrs),
            cidr_list_aliases: list_or_absent(&self.cidr_list_aliases),
        }
    }

    /// Where this rule allows access from: `Anywhere (…)` when it admits a whole
    /// address family, `Custom` when it names addresses.
    ///
    /// A rule that admits a whole family ALONGSIDE narrower blocks still reads as
    /// `Anywhere`: the narrower entries add nothing the wide one does not already
    /// admit, so they do not make the access custom. A rule open to BOTH families
    /// reports the IPv4 form, the one this member's own documentation gives
    /// (`Anywhere (0.0.0.0/0)`); its IPv6 half is reported where it belongs, in
    /// `ipv6Cidrs`, so nothing about the rule is lost.
    fn access_from(&self) -> &'static str {
        if self.cidrs.iter().any(|cidr| cidr == ANY_IPV4_CIDR) {
            ACCESS_FROM_ANYWHERE_IPV4
        } else if self.ipv6_cidrs.iter().any(|cidr| cidr == ANY_IPV6_CIDR) {
            ACCESS_FROM_ANYWHERE_IPV6
        } else {
            ACCESS_FROM_CUSTOM
        }
    }
}

/// `values` as a modelled list member, absent when it carries nothing: an empty
/// address list says the rule names no address of that family, which is not the
/// same claim as an empty list of them.
fn list_or_absent(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// The inbound rules Lightsail opens on a newly created instance's firewall:
/// the platform's management port (SSH for Linux/Unix, RDP for Windows) and HTTP,
/// both reachable from anywhere. Taken from the Lightsail user guide's
/// "Firewalls and ports" description of a new instance's default rules.
///
/// The IPv6 side of each rule is present only when the instance has an IPv6
/// address at all, so the reported rules match the addresses the instance has.
pub fn default_port_rules(blueprint_id: &str, ip_address_type: &str) -> Vec<StoredPortRule> {
    let management = if blueprint_is_windows(blueprint_id) {
        RDP_PORT
    } else {
        SSH_PORT
    };
    let (cidrs, ipv6_cidrs) = anywhere_cidrs(ip_address_type);
    [management, HTTP_PORT]
        .into_iter()
        .map(|port| StoredPortRule {
            from_port: port,
            to_port: port,
            protocol: DEFAULT_NETWORK_PROTOCOL.to_string(),
            cidrs: cidrs.clone(),
            ipv6_cidrs: ipv6_cidrs.clone(),
            cidr_list_aliases: Vec::new(),
        })
        .collect()
}

/// The IPv4 and IPv6 "every address" blocks an instance with `ip_address_type`
/// can be reached on: each present only when that address type gives the
/// instance an address of the family, because a rule cannot admit an address
/// family the instance does not have.
fn anywhere_cidrs(ip_address_type: &str) -> (Vec<String>, Vec<String>) {
    (
        if ip_address_type_has_ipv4(ip_address_type) {
            vec![ANY_IPV4_CIDR.to_string()]
        } else {
            Vec::new()
        },
        if ip_address_type_has_ipv6(ip_address_type) {
            vec![ANY_IPV6_CIDR.to_string()]
        } else {
            Vec::new()
        },
    )
}

/// The rule a `PortInfo` asks for, on an instance whose addresses are
/// `ip_address_type`'s — or a refusal naming the member that is wrong.
///
/// `member` is the input member the structure came from (`portInfo`,
/// `portInfos[2]`), so a refusal tells the caller which entry of the request to
/// fix.
///
/// What the model cannot express is checked here: the protocol must be one of
/// the modelled `NetworkProtocol` values, a port RANGE must run forwards, each
/// address must be in the notation its own member takes, and an alias must be
/// one Lightsail publishes. A rule that names no address at all is taken as open
/// to anywhere — Lightsail's own default rules are, and a rule admitting nothing
/// would open a port nothing can reach.
pub fn port_rule_from_request(
    member: &str,
    port_info: &PortInfo,
    ip_address_type: &str,
) -> Result<StoredPortRule, String> {
    let protocol = port_info
        .protocol
        .clone()
        .unwrap_or_else(|| DEFAULT_NETWORK_PROTOCOL.to_string());
    // `PortInfo$protocol` targets the modelled `NetworkProtocol` enum, so the
    // centralized walk has already refused any other value a REQUEST carried;
    // this keeps the rule total over whatever protocol reaches this function,
    // including the one substituted just above for a request that named none.
    if !NETWORK_PROTOCOL_VALUES.contains(&protocol.as_str()) {
        return Err(format!(
            "{member}.protocol must be one of [{}], got '{protocol}'",
            NETWORK_PROTOCOL_VALUES.join(", ")
        ));
    }
    // For `tcp`, `udp` and `all` the two members are the ends of a port range,
    // so the range has to run forwards. For `icmp` and `icmpv6` they are not a
    // range at all: `fromPort` is the ICMP type and `toPort` the ICMP code, as
    // both members' own documentation says, so no ordering holds between them.
    if !protocol_ports_are_icmp(&protocol) && port_info.from_port > port_info.to_port {
        return Err(format!(
            "{member}.fromPort {} is past {member}.toPort {}: a {protocol} rule opens a \
             range of ports, so fromPort must not be greater than toPort",
            port_info.from_port, port_info.to_port
        ));
    }
    let cidrs = checked_cidrs(
        &format!("{member}.cidrs"),
        port_info.cidrs.as_deref().unwrap_or_default(),
        is_ipv4_cidr,
        "an IPv4 address or an IPv4 range in CIDR notation",
    )?;
    let ipv6_cidrs = checked_cidrs(
        &format!("{member}.ipv6Cidrs"),
        port_info.ipv6_cidrs.as_deref().unwrap_or_default(),
        is_ipv6_cidr,
        "an IPv6 address or an IPv6 range in CIDR notation",
    )?;
    let cidr_list_aliases = checked_cidrs(
        &format!("{member}.cidrListAliases"),
        port_info.cidr_list_aliases.as_deref().unwrap_or_default(),
        |alias| alias == CIDR_LIST_ALIAS_LIGHTSAIL_CONNECT,
        "the only alias this service publishes, 'lightsail-connect'",
    )?;
    let (cidrs, ipv6_cidrs) =
        if cidrs.is_empty() && ipv6_cidrs.is_empty() && cidr_list_aliases.is_empty() {
            anywhere_cidrs(ip_address_type)
        } else {
            (cidrs, ipv6_cidrs)
        };
    Ok(StoredPortRule {
        from_port: port_info.from_port,
        to_port: port_info.to_port,
        protocol,
        cidrs,
        ipv6_cidrs,
        cidr_list_aliases,
    })
}

/// Whether `protocol`'s `fromPort`/`toPort` are an ICMP type and code rather
/// than the ends of a port range, from those members' own documentation.
fn protocol_ports_are_icmp(protocol: &str) -> bool {
    matches!(protocol, "icmp" | "icmpv6")
}

/// `entries` when every one of them satisfies `accepted` and the list is within
/// [`MAX_PORT_CIDRS`], or a refusal naming `member`, the entry that is wrong and
/// the `notation` that member takes.
fn checked_cidrs(
    member: &str,
    entries: &[String],
    accepted: impl Fn(&str) -> bool,
    notation: &str,
) -> Result<Vec<String>, String> {
    if entries.len() > MAX_PORT_CIDRS {
        return Err(format!(
            "{member} carries {} entries, more than the {MAX_PORT_CIDRS} this service \
             accepts",
            entries.len()
        ));
    }
    for entry in entries {
        if !accepted(entry) {
            return Err(format!("{member} entry '{entry}' is not {notation}"));
        }
    }
    Ok(entries.to_vec())
}

/// Whether `value` is an IPv4 address or a block of them in CIDR notation.
///
/// A bare address is accepted because `PortInfo$cidrs` says it is: "To allow the
/// IP address `192.0.2.44`, specify `192.0.2.44` or `192.0.2.44/32`" — so it is
/// kept as the caller wrote it rather than rewritten.
fn is_ipv4_cidr(value: &str) -> bool {
    is_cidr::<std::net::Ipv4Addr>(value, 32)
}

/// Whether `value` is an IPv6 address or a block of them in CIDR notation. See
/// [`is_ipv4_cidr`]; `PortInfo$ipv6Cidrs` takes the same notation over the IPv6
/// address family.
fn is_ipv6_cidr(value: &str) -> bool {
    is_cidr::<std::net::Ipv6Addr>(value, 128)
}

/// Whether `value` is an address of `Addr`, optionally followed by a prefix
/// length of at most `address_bits`.
///
/// The ADDRESS is parsed by the standard library's own parser for that family,
/// which is the parser for this notation: only what `Addr` admits is accepted,
/// and what remains of CIDR notation is the one decimal prefix length this
/// checks. No dependency is added for it, which keeps this change out of
/// `Cargo.lock` — the file every concurrent change in this repository also
/// writes.
fn is_cidr<Addr: std::str::FromStr>(value: &str, address_bits: u32) -> bool {
    let (address, prefix) = match value.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (value, None),
    };
    if address.parse::<Addr>().is_err() {
        return false;
    }
    match prefix {
        None => true,
        Some(prefix) => {
            !prefix.is_empty()
                && prefix.chars().all(|digit| digit.is_ascii_digit())
                && prefix.parse::<u32>().is_ok_and(|bits| bits <= address_bits)
        }
    }
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
    /// The inbound rules this instance's firewall holds, in a stable total order
    /// (see [`StoredInstance::sort_ports`]), so every read reports the same
    /// sequence. Created with the platform's defaults, then whatever
    /// `OpenInstancePublicPorts`, `CloseInstancePublicPorts` and
    /// `PutInstancePublicPorts` have made of them.
    pub ports: Vec<StoredPortRule>,
    pub state: Lifecycle,
    /// The state the clock owes this instance, while one is outstanding.
    pub due: Option<DueTransition>,
}

/// The device path an instance's system disk is exposed at.
///
/// A Lightsail instance is an EC2 instance whose root volume is the first device
/// of the machine, which is `/dev/xvda` on the Xen-style device naming an EC2
/// Linux instance uses. It is reported as the system disk's `path`, and it is
/// therefore a path no block storage disk can be attached at.
pub const SYSTEM_DISK_PATH: &str = "/dev/xvda";

impl StoredInstance {
    /// This record as the modelled `Instance`.
    ///
    /// `attached_disks` are the block storage disks the store has attached to
    /// this instance, which `hardware.disks` reports after the system disk. They
    /// are passed in because an attachment lives on the DISK record (the disk
    /// names the instance it is attached to), so only the store can resolve
    /// them — see [`ComAmazonawsLightsailLightsail20161128State::instance_view`].
    pub fn view(&self, attached_disks: Vec<crate::ops::types::Disk>) -> Instance {
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
                // The bundle's system disk first, then every block storage disk
                // `AttachDisk` has attached to this instance: an instance's
                // hardware is the volumes it actually has, so a disk attached to
                // it is reported here for as long as the attachment stands.
                disks: Some(
                    std::iter::once(self.system_disk())
                        .chain(attached_disks)
                        .collect(),
                ),
            }),
            networking: Some(InstanceNetworking {
                monthly_transfer: Some(MonthlyTransfer {
                    gb_per_month_allocated: Some(self.bundle.transfer_gb_per_month),
                }),
                ports: Some(self.ports.iter().map(StoredPortRule::port_info_view).collect()),
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

    /// Open `rule` on this instance's firewall — or refuse, when that would take
    /// the firewall past [`MAX_PORT_RULES`].
    ///
    /// A rule is identified by its port range and protocol, so opening one that
    /// is already open REPLACES the addresses allowed through it rather than
    /// holding the same ports twice: "Opens ports for a specific Amazon
    /// Lightsail instance, and specifies the IP addresses allowed to connect to
    /// the instance through the ports, and the protocol."
    pub fn open_port(&mut self, rule: StoredPortRule) -> Result<(), String> {
        match self
            .ports
            .iter_mut()
            .find(|stored| stored.is_same_rule(&rule))
        {
            Some(stored) => *stored = rule,
            None => {
                if self.ports.len() >= MAX_PORT_RULES {
                    return Err(format!(
                        "the firewall of instance '{}' already holds {MAX_PORT_RULES} rules, \
                         the most this emulator keeps",
                        self.name
                    ));
                }
                self.ports.push(rule);
            }
        }
        self.sort_ports();
        Ok(())
    }

    /// Close the rule `from_port`/`to_port`/`protocol` names, answering whether
    /// the firewall held it.
    ///
    /// Closing a port that is not open leaves the firewall exactly as it is: the
    /// request's goal — that those ports are not open — already holds.
    pub fn close_port(&mut self, from_port: i32, to_port: i32, protocol: &str) -> bool {
        let before = self.ports.len();
        self.ports
            .retain(|stored| stored.key() != (from_port, to_port, protocol));
        self.ports.len() != before
    }

    /// Replace this instance's whole firewall with `rules` — or refuse, when the
    /// RULES they collapse to are past [`MAX_PORT_RULES`] (entries naming one
    /// rule are one rule, so the bound is on what the firewall would hold).
    ///
    /// This is `PutInstancePublicPorts`' own semantics: "This action also closes
    /// all currently open ports that are not included in the request." Two
    /// entries that name the same ports and protocol are the same rule, so the
    /// last of them is what the firewall holds.
    ///
    /// PRECONDITION: `rules` is already bounded by the caller — the handler
    /// refuses a request carrying more than [`MAX_PORT_INFOS_PER_REQUEST`]
    /// entries before building any of them — so the collapse below allocates a
    /// bounded amount however large a request was sent.
    pub fn replace_ports(&mut self, rules: Vec<StoredPortRule>) -> Result<(), String> {
        debug_assert!(
            rules.len() <= MAX_PORT_INFOS_PER_REQUEST,
            "replace_ports was handed {} rules, past the {MAX_PORT_INFOS_PER_REQUEST} \
             entries one request may carry",
            rules.len()
        );
        // Two entries that name the same ports and protocol are the same rule,
        // so they are collapsed BEFORE the bound is applied: the guard measures
        // what the firewall would actually hold, not how many entries the
        // request wrote it with. The collapse is keyed, not scanned, so the work
        // is linear in the entries however many there are.
        let mut collapsed: HashMap<(i32, i32, String), StoredPortRule> = HashMap::new();
        for rule in rules {
            collapsed.insert(
                (rule.from_port, rule.to_port, rule.protocol.clone()),
                rule,
            );
        }
        if collapsed.len() > MAX_PORT_RULES {
            return Err(format!(
                "portInfos names {} rules, more than the {MAX_PORT_RULES} this emulator \
                 keeps on one firewall",
                collapsed.len()
            ));
        }
        self.ports = collapsed.into_values().collect();
        self.sort_ports();
        Ok(())
    }

    /// Keep the firewall in one total order — by port range, then protocol — so
    /// every read reports its rules in the same sequence however they were
    /// opened.
    fn sort_ports(&mut self) {
        self.ports.sort_by(|left, right| {
            (left.from_port, left.to_port, left.protocol.as_str()).cmp(&(
                right.from_port,
                right.to_port,
                right.protocol.as_str(),
            ))
        });
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
            path: Some(SYSTEM_DISK_PATH.to_string()),
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
    ///
    /// A disk the clock owes [`DISK_STATE_AVAILABLE`] while it is still attached
    /// is one `DetachDisk` has started detaching: settling into `available` is
    /// what releases the attachment, because a disk attached to nothing is what
    /// `available` means (see [`ComAmazonawsLightsailLightsail20161128State::advance_clock`]).
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

// ---------------------------------------------------------------------------
// Instance snapshots
// ---------------------------------------------------------------------------

/// One stored instance snapshot: every member `GetInstanceSnapshot` and
/// `GetInstanceSnapshots` report, plus the clock-owed transition its state is
/// waiting on.
///
/// A snapshot is a point-in-time copy, so everything it says about its SOURCE is
/// copied out of the instance record at the instant the snapshot was taken and
/// never followed afterwards: the snapshot keeps reporting the blueprint, bundle,
/// system disk size and attached block storage disks the instance had then, even
/// after the instance is changed or deleted.
#[derive(Debug, Clone)]
pub struct StoredInstanceSnapshot {
    /// The snapshot name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The size of the snapshotted system volume in GB, copied from the source
    /// instance's bundle — "The size in GB of the SSD".
    pub size_in_gb: i32,
    /// The current `InstanceSnapshotState` value:
    /// [`INSTANCE_SNAPSHOT_STATE_PENDING`] or
    /// [`INSTANCE_SNAPSHOT_STATE_AVAILABLE`].
    pub state: &'static str,
    /// The state the clock owes this snapshot, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The name of the instance the snapshot was taken of.
    pub from_instance_name: String,
    /// That instance's ARN.
    pub from_instance_arn: String,
    /// That instance's blueprint id.
    pub from_blueprint_id: String,
    /// That instance's bundle id.
    pub from_bundle_id: String,
    /// Every block storage disk attached to the source instance at the instant
    /// the snapshot was taken, copied whole: `InstanceSnapshot$fromAttachedDisks`
    /// is "an array of disk objects containing information about all block
    /// storage disks", and a disk later detached from the instance was still
    /// part of what this snapshot captured.
    pub from_attached_disks: Vec<StoredDisk>,
}

impl StoredInstanceSnapshot {
    /// This record as the modelled `InstanceSnapshot`.
    pub fn view(&self) -> crate::ops::types::InstanceSnapshot {
        crate::ops::types::InstanceSnapshot {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_INSTANCE_SNAPSHOT.to_string()),
            tags: Some(self.tags.clone()),
            state: Some(self.state.to_string()),
            // The modelled `InstanceSnapshot$progress` documentation says this
            // member "is populated only for disk snapshots, and is null for
            // instance snapshots", so an instance snapshot reports none.
            progress: None,
            from_attached_disks: Some(
                self.from_attached_disks
                    .iter()
                    .map(StoredDisk::view)
                    .collect(),
            ),
            from_instance_name: Some(self.from_instance_name.clone()),
            from_instance_arn: Some(self.from_instance_arn.clone()),
            from_blueprint_id: Some(self.from_blueprint_id.clone()),
            from_bundle_id: Some(self.from_bundle_id.clone()),
            // `CreateInstanceSnapshot` is an explicit snapshot the caller asked
            // for, so it is not one the `AutoSnapshot` add-on took.
            is_from_auto_snapshot: Some(false),
            size_in_gb: Some(self.size_in_gb),
        }
    }
}

// ---------------------------------------------------------------------------
// Export snapshot records
// ---------------------------------------------------------------------------

/// The prefix a minted export snapshot record name carries.
///
/// CHOSEN: Lightsail does not document how it names an export snapshot record,
/// only that the record HAS a name the caller never supplied — the modelled
/// `ExportSnapshot` input takes nothing but the snapshot to export, while
/// `ExportSnapshotRecord$name` targets `ResourceName` (pattern
/// `^\w[\w\-]*\w$`). So the name is minted here: this prefix, which says which
/// operation created the record, followed by the id this store allocated for the
/// record — the same id its ARN carries, so one value identifies it everywhere.
pub const EXPORT_SNAPSHOT_RECORD_NAME_PREFIX: &str = "ExportSnapshot-";

/// The id prefix of the Amazon EC2 resource an exported INSTANCE snapshot
/// becomes, from the modelled `ExportSnapshot` documentation: "Exported instance
/// snapshots appear in Amazon EC2 as Amazon Machine Images (AMIs)". See
/// [`ec2_destination_id`].
pub const EC2_IMAGE_ID_PREFIX: &str = "ami-";

/// The id prefix of the Amazon EC2 resource an exported DISK snapshot becomes,
/// from the same modelled documentation: "Exported disk snapshots appear in
/// Amazon EC2 as Amazon EBS volumes". See [`ec2_destination_id`].
pub const EC2_VOLUME_ID_PREFIX: &str = "vol-";

/// How many hexadecimal characters an EC2 resource id carries after its prefix:
/// the 17 of the long-form id format every EC2 resource created since 2018 uses
/// (`vol-0123456789abcdef0`), which is the form `DestinationInfo$id` reports for
/// a resource created in EC2.
const EC2_RESOURCE_ID_HEX_LEN: usize = 17;

/// The destination service an export snapshot record reports, as
/// `DestinationInfo$service` ("The destination service of the record").
///
/// `ExportSnapshot` exports to Amazon EC2 and to nothing else, so the one value
/// this member can carry is that service, named by its own ARN namespace and
/// endpoint prefix (`ec2`) — the identifier this emulator names a service with
/// everywhere else.
pub const EXPORT_DESTINATION_SERVICE: &str = "ec2";

/// The id of the Amazon EC2 resource an export creates: `prefix` (which kind of
/// resource the exported snapshot became) plus the long-form EC2 id's
/// [`EC2_RESOURCE_ID_HEX_LEN`] hexadecimal characters, taken from the store-minted
/// `id` so no two exports in this process name the same destination resource.
pub fn ec2_destination_id(prefix: &str, id: &str) -> String {
    let hex = id.replace('-', "");
    format!(
        "{prefix}{}",
        hex.get(..EC2_RESOURCE_ID_HEX_LEN).unwrap_or(hex.as_str())
    )
}

/// The ARN of the export snapshot record `id` in `region`.
pub fn export_snapshot_record_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{EXPORT_SNAPSHOT_RECORD_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// One stored export snapshot record: the record `ExportSnapshot` created, with
/// the clock-owed transition its state is waiting on.
///
/// Every `ExportSnapshot` call appends ONE record — exporting the same snapshot
/// twice is two exports, and so two records — which is why these are keyed by a
/// minted name rather than by the snapshot's.
#[derive(Debug, Clone)]
pub struct StoredExportSnapshotRecord {
    /// The record name this store minted — this record's store key and the
    /// `name` every read reports. See [`EXPORT_SNAPSHOT_RECORD_NAME_PREFIX`].
    pub name: String,
    pub arn: String,
    pub created_at_ms: i64,
    /// Where the record is: the location of the snapshot that was exported,
    /// because "Snapshots are exported to the same Amazon Web Services Region in
    /// Amazon EC2 as the source Lightsail snapshot".
    pub location: StoredLocation,
    /// The current `RecordState` value: [`RECORD_STATE_STARTED`] or
    /// [`RECORD_STATE_SUCCEEDED`].
    pub state: &'static str,
    /// The state the clock owes this record, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The snapshot this export was taken of, captured at the instant of the
    /// export — so the record keeps describing it even if the snapshot itself
    /// later changes or is deleted.
    pub source_info: ExportSnapshotRecordSourceInfo,
    /// The id of the Amazon EC2 resource this export creates. See
    /// [`ec2_destination_id`].
    pub destination_id: String,
}

impl StoredExportSnapshotRecord {
    /// This record as the modelled `ExportSnapshotRecord`.
    pub fn view(&self) -> ExportSnapshotRecord {
        ExportSnapshotRecord {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_EXPORT_SNAPSHOT_RECORD.to_string()),
            state: Some(self.state.to_string()),
            source_info: Some(self.source_info.clone()),
            // `DestinationInfo$id` is "The ID of the resource created at the
            // destination", and while the export is still `Started` nothing has
            // been created there yet — so the destination is reported from the
            // instant the export succeeded, not before.
            destination_info: (self.state == RECORD_STATE_SUCCEEDED).then(|| DestinationInfo {
                id: Some(self.destination_id.clone()),
                service: Some(EXPORT_DESTINATION_SERVICE.to_string()),
            }),
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
// Static IPs
// ---------------------------------------------------------------------------

/// One stored static IP: the public IPv4 address `AllocateStaticIp` reserved for
/// the account under a name, and the instance it is attached to while one is.
///
/// The address itself is drawn from the store's own allocator, so it is unique
/// within this process and stable for the life of the allocation — a static IP is
/// static precisely because every later read reports the same address.
#[derive(Debug, Clone)]
pub struct StoredStaticIp {
    /// The static IP name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// The reserved address, in the dotted-quad form the modelled `IpAddress`
    /// pattern requires.
    pub ip_address: String,
    /// The instance this static IP is attached to, while one is. `AttachStaticIp`
    /// is what sets it; a newly allocated static IP is attached to nothing.
    pub attached_to: Option<String>,
}

impl StoredStaticIp {
    /// This record as the modelled `StaticIp`.
    pub fn view(&self) -> StaticIp {
        StaticIp {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_STATIC_IP.to_string()),
            ip_address: Some(self.ip_address.clone()),
            attached_to: self.attached_to.clone(),
            // `isAttached` is exactly "whether the static IP is attached", so it
            // is derived from the one stored attachment rather than tracked
            // beside it, where the two could disagree.
            is_attached: Some(self.attached_to.is_some()),
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

/// The certificate authorities a database's server certificate can be issued
/// under — the identifiers `caCertificateIdentifier` may name.
///
/// Source: the certificate authorities "Using SSL/TLS to encrypt a connection to
/// a DB instance" (Amazon RDS User Guide) publishes for a DB instance — the
/// 2019 authority and the three current ones, which differ in the key the
/// certificate carries (RSA 2048, RSA 4096 and ECC P-384). Each is a value AWS
/// allocated, so the published list is the only honest source; an authority AWS
/// adds or retires is a row added or removed here.
pub const RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS: &[&str] = &[
    "rds-ca-2019",
    "rds-ca-rsa2048-g1",
    "rds-ca-rsa4096-g1",
    "rds-ca-ecc384-g1",
];

/// The published certificate authority `identifier` names, compared exactly —
/// an identifier is a name AWS allocated, not a presentation form.
pub fn relational_database_ca_certificate(identifier: &str) -> Option<&'static str> {
    RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS
        .iter()
        .copied()
        .find(|published| *published == identifier)
}

/// The identifiers [`relational_database_ca_certificate`] accepts, for a refusal
/// that names them.
pub fn relational_database_ca_certificate_identifiers() -> String {
    RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS.join(", ")
}

/// What `PendingModifiedRelationalDatabaseValues$masterUserPassword` reports
/// while a password change is waiting for the database's next maintenance
/// window.
///
/// The master user password is a secret, and the operation that answers with one
/// is `GetRelationalDatabaseMasterUserPassword`; a database read may say only
/// THAT a new password is pending, never what it is. Amazon RDS masks the same
/// member of its own `PendingModifiedValues` the same way.
pub const MASKED_MASTER_USER_PASSWORD: &str = "****";

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
/// and neither engine's protocol begins with a TLS handshake: a MySQL server
/// sends its Initial Handshake Packet in the clear and only then upgrades, on
/// the client's SSL-request packet, and PostgreSQL negotiates the same way
/// through its SSLRequest message — so the core's `rustls` acceptor fails the
/// handshake of every real database driver before the connection ever reaches a
/// handler (`vendor/server/src/listeners.rs`: "TLS-only ... a plaintext/HTTP
/// preface also fails the handshake here and never reaches the handler"), and a
/// service may not bind a socket of its own. Serving a database data plane
/// needs a core seam that hands over a connection WITHOUT terminating TLS first
/// (so the protocol server can do the in-band upgrade itself); until the core
/// offers one, no address this service could announce would answer the MySQL or
/// PostgreSQL protocol. Everything the control plane owns — resolving the
/// database, its state, its hardware, its credentials and its windows — is real.
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

/// How many characters the master user password
/// [`ComAmazonawsLightsailLightsail20161128State::mint_master_user_password`]
/// generates carries.
///
/// CHOSEN: Lightsail publishes no length for the "new strong password generated
/// by Lightsail" (the modelled `rotateMasterUserPassword` documentation), only
/// that it is strong. It sits inside the narrowest range any engine this service
/// serves accepts ([`master_credential_limits`]: MySQL's 8 to 41 characters),
/// so a minted password satisfies the same rule a caller-supplied one is held
/// to, whichever engine the database runs.
const MINTED_MASTER_USER_PASSWORD_LEN: usize = 20;

/// The characters a minted master user password is drawn from: printable ASCII
/// other than a space, `/`, `"` and `@` — exactly the set
/// [`validate_master_user_password`] accepts, from the modelled
/// `masterUserPassword` documentation.
const MINTED_MASTER_USER_PASSWORD_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!#$%&'()*+,-.:;<=>?[\\]^_`{|}~";

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
    // Each component must be exactly two ASCII DIGITS. `hh24:mi` admits no
    // sign, no space and no single-digit form — and a component that is only
    // digits cannot parse to a negative number, which the bounds below would
    // otherwise let through and carry into the arithmetic.
    if !is_two_ascii_digits(hours) || !is_two_ascii_digits(minutes) {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    // Both components are bounded above, so the answer is at most 23*60+59; the
    // arithmetic is checked anyway, so no unchecked multiplication is left on a
    // path a request reaches.
    i64::from(hours)
        .checked_mul(60)?
        .checked_add(i64::from(minutes))
}

/// Whether `component` is exactly two ASCII digits — the form each component of
/// `hh24:mi` takes.
fn is_two_ascii_digits(component: &str) -> bool {
    component.len() == 2 && component.chars().all(|digit| digit.is_ascii_digit())
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

/// The members of a database one `UpdateRelationalDatabase` call changes that
/// can be left WAITING: the master user password, the engine version (which a
/// major-version blueprint update moves) and the backup retention setting.
///
/// Exactly the three the modelled `PendingModifiedRelationalDatabaseValues`
/// carries — the structure `RelationalDatabase$pendingModifiedValues` reports a
/// pending modification through — so a change to any OTHER member has no way to
/// be reported pending and therefore takes effect at once.
///
/// An entry is `None` when the call did not ask for that member to change.
#[derive(Debug, Clone, Default)]
pub struct RelationalDatabaseModification {
    /// The password the master user gets, whether the caller supplied it or
    /// asked the service to mint one.
    pub master_user_password: Option<String>,
    /// The blueprint the database moves to: a later major version of the engine
    /// it already runs, which is what `engineVersion` then reports.
    pub blueprint: Option<RelationalDatabaseBlueprintSpec>,
    /// Whether automated backup retention is on.
    pub backup_retention_enabled: Option<bool>,
}

impl RelationalDatabaseModification {
    /// Whether this call asked for none of the three to change, so there is
    /// nothing to apply and nothing to leave pending.
    pub fn is_empty(&self) -> bool {
        self.master_user_password.is_none()
            && self.blueprint.is_none()
            && self.backup_retention_enabled.is_none()
    }

    /// Take `later`'s entries over this one's: a second call that modifies the
    /// same member while the first is still waiting replaces what it will be
    /// applied with, and leaves the members it says nothing about pending.
    fn merge(&mut self, later: Self) {
        if let Some(password) = later.master_user_password {
            self.master_user_password = Some(password);
        }
        if let Some(blueprint) = later.blueprint {
            self.blueprint = Some(blueprint);
        }
        if let Some(enabled) = later.backup_retention_enabled {
            self.backup_retention_enabled = Some(enabled);
        }
    }
}

/// A modification an `UpdateRelationalDatabase` call left for the database's
/// next maintenance window, and the instant the clock applies it at.
#[derive(Debug, Clone)]
pub struct PendingRelationalDatabaseModification {
    /// Epoch milliseconds at which the modification becomes due.
    ///
    /// AWS applies a deferred modification in the database's own
    /// `preferredMaintenanceWindow`, which is up to a week away and no caller
    /// can be made to wait for here, so the instant is the one every other
    /// clock-owed change of this service uses
    /// ([`STATE_TRANSITION_MILLIS`] after the call). The span is deliberately
    /// NON-ZERO, so a caller that reads straight after the call still observes
    /// the modification pending exactly as it does against AWS.
    pub at_ms: i64,
    /// What becomes of the database when it is due.
    pub change: RelationalDatabaseModification,
}

impl PendingRelationalDatabaseModification {
    /// This modification as the modelled
    /// `PendingModifiedRelationalDatabaseValues`.
    ///
    /// A member nothing is waiting on is left out rather than reported null, and
    /// a waiting password is reported MASKED
    /// ([`MASKED_MASTER_USER_PASSWORD`]).
    pub fn view(&self) -> crate::ops::types::PendingModifiedRelationalDatabaseValues {
        crate::ops::types::PendingModifiedRelationalDatabaseValues {
            master_user_password: self
                .change
                .master_user_password
                .as_ref()
                .map(|_| MASKED_MASTER_USER_PASSWORD.to_string()),
            engine_version: self
                .change
                .blueprint
                .map(|blueprint| blueprint.engine_version.to_string()),
            backup_retention_enabled: self.change.backup_retention_enabled,
        }
    }
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
    /// The certificate authority this database's server certificate is issued
    /// under, as `caCertificateIdentifier` reports it: the service's default
    /// ([`RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIER`]) until an
    /// `UpdateRelationalDatabase` call associates another published one with the
    /// database.
    pub ca_certificate_identifier: String,
    /// The current state: [`RELATIONAL_DATABASE_STATE_CREATING`],
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`],
    /// [`RELATIONAL_DATABASE_STATE_REBOOTING`],
    /// [`RELATIONAL_DATABASE_STATE_STOPPING`],
    /// [`RELATIONAL_DATABASE_STATE_STOPPED`] or
    /// [`RELATIONAL_DATABASE_STATE_STARTING`].
    pub state: &'static str,
    /// The state the clock owes this database, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The parameters an `UpdateRelationalDatabaseParameters` call has set on
    /// this database, by parameter name — the per-database half of what
    /// `GetRelationalDatabaseParameters` reports, the other half being the
    /// engine's own published set ([`relational_database_parameter_specs`]). A
    /// parameter nothing has set is absent, and reports what the engine and this
    /// database's own hardware give it.
    pub parameter_overrides: BTreeMap<String, StoredParameterOverride>,
    /// The modification an `UpdateRelationalDatabase` call left for this
    /// database's next maintenance window, while one is outstanding — what
    /// `pendingModifiedValues` reports until the clock applies it.
    pub pending_modification: Option<PendingRelationalDatabaseModification>,
}

impl StoredRelationalDatabase {
    /// `parameters` as THIS database reports them: an entry an
    /// `UpdateRelationalDatabaseParameters` call has set on this database
    /// carries the value it set and the apply method it was set with, and every
    /// other entry is the engine's published one, unchanged.
    ///
    /// The value an update set is reported straight away, whichever apply method
    /// it used: the modification reaches the database's parameter group when the
    /// call is answered, and whether the RUNNING engine has loaded it is what
    /// [`Self::parameter_apply_status`] reports instead.
    pub fn applied_parameters(
        &self,
        parameters: Vec<RelationalDatabaseParameter>,
    ) -> Vec<RelationalDatabaseParameter> {
        parameters
            .into_iter()
            .map(|parameter| {
                let Some(set) = parameter
                    .parameter_name
                    .as_deref()
                    .and_then(|name| self.parameter_overrides.get(name))
                else {
                    return parameter;
                };
                RelationalDatabaseParameter {
                    parameter_value: Some(set.value.clone()),
                    apply_method: Some(set.apply_method.to_string()),
                    ..parameter
                }
            })
            .collect()
    }

    /// Apply `update` to this database's parameter group.
    ///
    /// The value is carried from here on, and a value applied
    /// `pending-reboot` also leaves the RUNNING engine behind — which is what
    /// [`Self::parameter_apply_status`] reports.
    pub fn set_parameter(&mut self, update: RelationalDatabaseParameterUpdate) {
        let pending_reboot = update.apply_method == PARAMETER_APPLY_METHOD_PENDING_REBOOT;
        self.parameter_overrides.insert(
            update.spec.parameter_name.to_string(),
            StoredParameterOverride {
                value: update.value,
                apply_method: update.apply_method,
                pending_reboot,
            },
        );
    }

    /// Change the three members of this database a modification can carry,
    /// either now (`immediately`) or at its next maintenance window.
    ///
    /// "When true, applies changes immediately. When false, applies changes
    /// during the preferred maintenance window" (the modelled
    /// `applyImmediately` documentation), which is why a deferred change is held
    /// on the record and reported by `pendingModifiedValues` rather than written
    /// into it. Applying immediately applies what an EARLIER call left waiting
    /// too — the member it set is a change the database is already carrying
    /// towards, and nothing may be left waiting once a call has said to apply
    /// now — with this call's own value winning where both name one. Answers the
    /// instant the change takes effect at, or `None` when it already has —
    /// which is also what a call that changes none of the three answers.
    pub fn modify(
        &mut self,
        change: RelationalDatabaseModification,
        immediately: bool,
        now_ms: i64,
    ) -> Option<i64> {
        if immediately {
            let mut merged = self
                .pending_modification
                .take()
                .map(|pending| pending.change)
                .unwrap_or_default();
            merged.merge(change);
            if !merged.is_empty() {
                self.apply_modification(merged);
            }
            return None;
        }
        if change.is_empty() {
            return None;
        }
        let at_ms = now_ms.saturating_add(STATE_TRANSITION_MILLIS);
        match &mut self.pending_modification {
            Some(pending) => {
                pending.at_ms = at_ms;
                pending.change.merge(change);
            }
            None => {
                self.pending_modification = Some(PendingRelationalDatabaseModification {
                    at_ms,
                    change,
                })
            }
        }
        Some(at_ms)
    }

    /// Write `change` into this record: the database carries the new values from
    /// here on, and nothing is waiting on them any more.
    fn apply_modification(&mut self, change: RelationalDatabaseModification) {
        if let Some(password) = change.master_user_password {
            self.master_user_password = Some(password);
        }
        if let Some(blueprint) = change.blueprint {
            self.blueprint = blueprint;
        }
        if let Some(enabled) = change.backup_retention_enabled {
            self.backup_retention_enabled = enabled;
        }
    }

    /// Apply the modification the clock owes this database, once its maintenance
    /// window is due. A no-op while nothing is waiting, or while the instant is
    /// still ahead.
    pub fn apply_due_modification(&mut self, now_ms: i64) {
        if self
            .pending_modification
            .as_ref()
            .is_none_or(|pending| pending.at_ms > now_ms)
        {
            return;
        }
        if let Some(pending) = self.pending_modification.take() {
            self.apply_modification(pending.change);
        }
    }

    /// Start the restart of this database's engine: the database reports
    /// [`RELATIONAL_DATABASE_STATE_REBOOTING`] and the clock owes it
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`] again.
    ///
    /// A restart happens INSIDE the database: nothing else on the record is
    /// rewritten — the ARN, the hardware, the credentials, the windows and the
    /// master endpoint a client connects to are all the ones the database
    /// already had, exactly as an RDS DB instance keeps its endpoint across a
    /// reboot.
    ///
    /// PRECONDITION: the database has no transition outstanding. A reboot is
    /// accepted only in [`RELATIONAL_DATABASE_STATE_AVAILABLE`], the one state
    /// where nothing is owed, so the `due` written here replaces none — a caller
    /// that reached this from a state with a transition pending would drop it.
    pub fn begin_reboot(&mut self, now_ms: i64) {
        self.state = RELATIONAL_DATABASE_STATE_REBOOTING;
        self.due = Some(DueNamedState::after(
            now_ms,
            RELATIONAL_DATABASE_STATE_AVAILABLE,
        ));
    }

    /// Start shutting this database's engine down: the database reports
    /// [`RELATIONAL_DATABASE_STATE_STOPPING`] and the clock owes it
    /// [`RELATIONAL_DATABASE_STATE_STOPPED`].
    ///
    /// Stopping happens INSIDE the database, exactly as a restart does: nothing
    /// else on the record is rewritten, the master endpoint included — an RDS DB
    /// instance keeps its endpoint while it is stopped, which is what lets the
    /// same address serve again once it is started.
    ///
    /// PRECONDITION: the database has no transition outstanding. A stop is
    /// accepted only in [`RELATIONAL_DATABASE_STATE_AVAILABLE`], the one state
    /// where nothing is owed, so the `due` written here replaces none.
    pub fn begin_stop(&mut self, now_ms: i64) {
        self.state = RELATIONAL_DATABASE_STATE_STOPPING;
        self.due = Some(DueNamedState::after(
            now_ms,
            RELATIONAL_DATABASE_STATE_STOPPED,
        ));
    }

    /// Start this database's engine back up: the database reports
    /// [`RELATIONAL_DATABASE_STATE_STARTING`] and the clock owes it
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`].
    ///
    /// Starting re-provisions nothing — the record is the one the database had
    /// while it was stopped, the master endpoint a client connects to above all.
    ///
    /// The `due` written here REPLACES the one a stopped database carries: the
    /// service's own seven-day automatic start
    /// ([`RELATIONAL_DATABASE_AUTO_START_AFTER_MILLIS`]) is what this call does
    /// by hand, so it is no longer owed once the caller has asked for it.
    pub fn begin_start(&mut self, now_ms: i64) {
        self.state = RELATIONAL_DATABASE_STATE_STARTING;
        self.due = Some(DueNamedState::after(
            now_ms,
            RELATIONAL_DATABASE_STATE_AVAILABLE,
        ));
    }

    /// The engine has (re)started — after a `RebootRelationalDatabase` call, or
    /// after a `StartRelationalDatabase` brought a stopped database back up — so
    /// every parameter value that was waiting for a restart is now loaded by it:
    /// "Parameters marked with a pending-reboot apply type are applied only
    /// after the database is rebooted using the reboot relational database
    /// operation" (the modelled `UpdateRelationalDatabaseParameters`
    /// documentation).
    ///
    /// The VALUES are untouched — the parameter group carried them already; what
    /// changes is that the running engine has them, which is what
    /// [`Self::parameter_apply_status`] reports.
    pub fn load_pending_parameters(&mut self) {
        for set in self.parameter_overrides.values_mut() {
            set.pending_reboot = false;
        }
    }

    /// `RelationalDatabase$parameterApplyStatus`: DERIVED from the parameter
    /// group rather than tracked beside it, where the two could disagree — a
    /// database is [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`] exactly while it
    /// holds a parameter value the running engine has not loaded, and
    /// [`PARAMETER_APPLY_STATUS_IN_SYNC`] otherwise.
    pub fn parameter_apply_status(&self) -> &'static str {
        if self
            .parameter_overrides
            .values()
            .any(|set| set.pending_reboot)
        {
            PARAMETER_APPLY_STATUS_PENDING_REBOOT
        } else {
            PARAMETER_APPLY_STATUS_IN_SYNC
        }
    }

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
            // What an `UpdateRelationalDatabase` call left for this database's
            // next maintenance window, while something is waiting; a database
            // with nothing mid-modification reports nothing rather than an
            // empty structure.
            pending_modified_values: self
                .pending_modification
                .as_ref()
                .map(PendingRelationalDatabaseModification::view),
            engine: Some(self.blueprint.engine.to_string()),
            engine_version: Some(self.blueprint.engine_version.to_string()),
            // "The latest point in time to which the database can be restored":
            // no automated backup has been taken yet — the first one is taken in
            // the database's own backup window — so there is no such point.
            latest_restorable_time: None,
            master_username: Some(self.master_username.clone()),
            parameter_apply_status: Some(self.parameter_apply_status().to_string()),
            preferred_backup_window: Some(self.preferred_backup_window.clone()),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            publicly_accessible: Some(self.publicly_accessible),
            // The endpoint is announced once the database has finished being
            // provisioned: a database still being created accepts no connection,
            // and reports no address to make one to. Once announced it is the
            // database's from then on — a restart does not take it away, exactly
            // as an RDS DB instance keeps its endpoint across a reboot — so it is
            // reported for every state other than `creating`.
            master_endpoint: (self.state != RELATIONAL_DATABASE_STATE_CREATING).then(|| {
                crate::ops::types::RelationalDatabaseEndpoint {
                    port: Some(relational_database_port(self.blueprint.engine)),
                    address: Some(self.endpoint_address.clone()),
                }
            }),
            // A list member is reported EMPTY rather than left out: no
            // maintenance action is pending on a database this slice holds, and
            // an absent member hands a typed caller a null where it iterates.
            pending_maintenance_actions: Some(Vec::new()),
            ca_certificate_identifier: Some(self.ca_certificate_identifier.clone()),
        }
    }
}

/// One stored managed relational database snapshot: every member
/// `GetRelationalDatabaseSnapshot` and `GetRelationalDatabaseSnapshots` report,
/// plus the clock-owed transition its state is waiting on.
///
/// A snapshot is a point-in-time copy, so everything it says about its SOURCE is
/// copied out of the source record at the instant the snapshot was taken and
/// never followed afterwards: the snapshot keeps reporting the engine, version,
/// disk size, bundle and blueprint the database had then, even after the
/// database is resized or deleted.
#[derive(Debug, Clone)]
pub struct StoredRelationalDatabaseSnapshot {
    /// The snapshot name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The source database's engine, copied from its blueprint.
    pub engine: String,
    /// The source database's engine version, copied from its blueprint.
    pub engine_version: String,
    /// The size of the snapshotted database disk in GB, copied from the source
    /// database's bundle.
    pub size_in_gb: i32,
    /// The name of the database the source database served, copied from the
    /// source record: a snapshot captures the database it was taken of, so
    /// `CreateRelationalDatabaseFromSnapshot` restores a database serving the
    /// same database name — even after the source has been deleted. No member of
    /// `RelationalDatabaseSnapshot` exposes it.
    pub master_database_name: String,
    /// The master user name the source database was created with, copied from
    /// the source record for the same reason: a restored database is reached
    /// with the credentials of the database the snapshot was taken of.
    pub master_username: String,
    /// The master user password the source database held, copied from the source
    /// record so a restore reports the credentials the snapshot captured.
    /// Absent when the source carried none.
    pub master_user_password: Option<String>,
    /// The current state: [`RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING`] or
    /// [`RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE`].
    pub state: &'static str,
    /// The state the clock owes this snapshot, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The name of the database the snapshot was taken of.
    pub from_relational_database_name: String,
    /// That database's ARN.
    pub from_relational_database_arn: String,
    /// That database's bundle id.
    pub from_relational_database_bundle_id: String,
    /// That database's blueprint id.
    pub from_relational_database_blueprint_id: String,
}

impl StoredRelationalDatabaseSnapshot {
    /// This record as the modelled `RelationalDatabaseSnapshot`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabaseSnapshot {
        crate::ops::types::RelationalDatabaseSnapshot {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_RELATIONAL_DATABASE_SNAPSHOT.to_string()),
            tags: Some(self.tags.clone()),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            size_in_gb: Some(self.size_in_gb),
            state: Some(self.state.to_string()),
            from_relational_database_name: Some(self.from_relational_database_name.clone()),
            from_relational_database_arn: Some(self.from_relational_database_arn.clone()),
            from_relational_database_bundle_id: Some(
                self.from_relational_database_bundle_id.clone(),
            ),
            from_relational_database_blueprint_id: Some(
                self.from_relational_database_blueprint_id.clone(),
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
// Instance HTTPS setup
//
// The append-only history of the `SetupInstanceHttps` calls made against one
// instance: what `GetSetupHistory` reports back. It sits beside the operation
// log, which every one of those calls also writes an entry to.
// ---------------------------------------------------------------------------

/// The `Operation$operationType` a `SetupInstanceHttps` call logs, the modelled
/// `OperationType` enum's own `SetupInstanceHttps` member.
pub const OPERATION_TYPE_SETUP_INSTANCE_HTTPS: &str = "SetupInstanceHttps";

/// `SetupStatus` values this slice uses, from the modelled enum: a setup is
/// `inProgress` for as long as the certificate the request asked for is still
/// being put in place, and `succeeded` once it is.
pub const SETUP_STATUS_IN_PROGRESS: &str = "inProgress";
/// See [`SETUP_STATUS_IN_PROGRESS`].
pub const SETUP_STATUS_SUCCEEDED: &str = "succeeded";

/// How many setup records one `GetSetupHistory` page carries, from that
/// operation's own documentation: "Returns detailed information for five of the
/// most recent `SetupInstanceHttps` requests that were ran on the target
/// instance."
pub const SETUP_HISTORY_PAGE_SIZE: usize = 5;

/// The length at which a `SetupInstanceHttps` domain name counts as a short one,
/// from that operation's documentation: "If you provide more than one domain name
/// in the request, at least one name must be less than or equal to 63 characters
/// in length."
pub const SETUP_DOMAIN_NAME_SHORT_LENGTH: usize = 63;

/// The most HTTPS setup records this emulator keeps, oldest dropped first.
///
/// Emulator resource guard: the history is append-only, exactly like the
/// operation log, so without a cap a long-lived emulator would grow without
/// bound.
pub const MAX_INSTANCE_HTTPS_SETUPS: usize = 2_000;

/// One `SetupInstanceHttps` request against one instance, and what became of it.
///
/// The record is what `GetSetupHistory` reports, so it holds the request as it
/// was submitted together with the instance it ran against AS IT WAS THEN: this
/// is a history, and the resource members of a history entry do not change when
/// the instance later does.
#[derive(Debug, Clone)]
pub struct StoredInstanceHttpsSetup {
    /// The operation-log entry this call wrote, which is also what identifies
    /// the record: `SetupHistory$operationId` is "A GUID that's used to identify
    /// the operation".
    pub operation_id: String,
    pub instance_name: String,
    pub domain_names: Vec<String>,
    pub certificate_provider: String,
    /// The renewal contact the request carried. Kept, never reported: the
    /// modelled `SetupRequest` has no member for it, because it "Returns
    /// information that was submitted during the `SetupInstanceHttps` request.
    /// Email information is redacted for privacy."
    pub email_address: String,
    /// The instance's ARN when the setup ran.
    pub resource_arn: String,
    /// The instance's creation time when the setup ran.
    pub resource_created_at_ms: i64,
    /// Where that instance was.
    pub resource_location: StoredLocation,
    /// A value of the modelled `SetupStatus` enum: [`SETUP_STATUS_IN_PROGRESS`]
    /// until the clock settles it into [`SETUP_STATUS_SUCCEEDED`].
    pub status: String,
    /// When the clock owes this setup its terminal `succeeded` status, while one
    /// is outstanding.
    pub settles_at_ms: Option<i64>,
}

impl StoredInstanceHttpsSetup {
    /// The `SetupHistoryPageToken` that names this record.
    ///
    /// The modelled token admits `^[A-Za-z0-9+/=]+$` at 24..40 characters, which
    /// a GUID's own hyphens do not satisfy, so the token is that GUID's 32 hex
    /// digits — still naming exactly one record, and within the modelled length.
    pub fn page_token(&self) -> String {
        self.operation_id.replace('-', "")
    }

    /// This record as the modelled `SetupHistory`.
    pub fn view(&self) -> crate::ops::types::SetupHistory {
        crate::ops::types::SetupHistory {
            operation_id: Some(self.operation_id.clone()),
            request: Some(crate::ops::types::SetupRequest {
                instance_name: Some(self.instance_name.clone()),
                domain_names: Some(self.domain_names.clone()),
                certificate_provider: Some(self.certificate_provider.clone()),
            }),
            resource: Some(crate::ops::types::SetupHistoryResource {
                name: Some(self.instance_name.clone()),
                arn: Some(self.resource_arn.clone()),
                created_at: Some(timestamp(self.resource_created_at_ms)),
                location: Some(self.resource_location.view()),
                resource_type: Some(RESOURCE_TYPE_INSTANCE.to_string()),
            }),
            // `SetupExecutionDetails` is one COMMAND run on the instance's
            // operating system, with the stdout, the stderr and the script
            // version it produced. This emulator runs no command on an instance,
            // so there is no command output to report, and inventing some would
            // claim output nothing produced. The list is reported EMPTY rather
            // than left out, so a typed caller iterating it still gets a list.
            execution_details: Some(Vec::new()),
            status: Some(self.status.clone()),
        }
    }
}

/// Accept the domain names a `SetupInstanceHttps` request carries, or refuse with
/// a message naming the member that is wrong and the constraint it broke.
///
/// Each name's own length and charset, and the list's own 1..10 length, are
/// modelled constraints the centralized walk already refused outside. What is
/// left is the rule the model cannot express, from the operation's own
/// documentation: "If you provide more than one domain name in the request, at
/// least one name must be less than or equal to 63 characters in length."
pub fn validate_setup_domain_names(domain_names: &[String]) -> Result<(), String> {
    let short = domain_names
        .iter()
        .any(|name| name.chars().count() <= SETUP_DOMAIN_NAME_SHORT_LENGTH);
    if domain_names.len() > 1 && !short {
        return Err(format!(
            "domainNames carries {} names and every one of them is longer than \
             {SETUP_DOMAIN_NAME_SHORT_LENGTH} characters: a request naming more than one \
             domain must name at least one of at most {SETUP_DOMAIN_NAME_SHORT_LENGTH} \
             characters",
            domain_names.len()
        ));
    }
    Ok(())
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
// Instance metrics
// ---------------------------------------------------------------------------

/// Every metric an instance publishes, each with the `unit` its data is
/// published in.
///
/// The list is the modelled `GetInstanceMetricDataRequest$metricName`
/// documentation — "Valid instance metric names are listed below, along with the
/// most useful `statistics` to include in your request, and the published `unit`
/// value" — and the `unit` of each row is the "published unit" that entry states.
/// Source:
/// `smithy_query(query="doc",shape="GetInstanceMetricDataRequest",member="metricName")`.
/// It covers every member of the modelled `InstanceMetricName` enum, which is
/// what `GetInstanceMetricData`'s boundary walk admits.
///
/// WIDER than the instance rows of [`ALARM_METRICS`]: that list is the narrower
/// set of metrics an ALARM can watch, and `MetadataNoToken` is published by an
/// instance without being one of them.
pub const INSTANCE_METRIC_UNITS: &[(&str, &str)] = &[
    ("BurstCapacityPercentage", "Percent"),
    ("BurstCapacityTime", "Seconds"),
    ("CPUUtilization", "Percent"),
    ("NetworkIn", "Bytes"),
    ("NetworkOut", "Bytes"),
    ("StatusCheckFailed", "Count"),
    ("StatusCheckFailed_Instance", "Count"),
    ("StatusCheckFailed_System", "Count"),
    ("MetadataNoToken", "Count"),
    ("DiskReadOps", "Count"),
    ("DiskWriteOps", "Count"),
    ("DiskReadBytes", "Bytes"),
    ("DiskWriteBytes", "Bytes"),
    ("DiskTotalReadTime", "Seconds"),
    ("DiskTotalWriteTime", "Seconds"),
    ("DiskQueueLength", "Count"),
];

/// The unit the instance metric `metric_name` is published in, or `None` when
/// this catalog does not know the metric.
pub fn instance_metric_unit(metric_name: &str) -> Option<&'static str> {
    INSTANCE_METRIC_UNITS
        .iter()
        .find(|(name, _)| *name == metric_name)
        .map(|(_, unit)| *unit)
}

// ---------------------------------------------------------------------------
// Alarms
// ---------------------------------------------------------------------------

/// The `AlarmState` a newly created alarm reports, from the modelled `PutAlarm`
/// documentation: "When this action creates an alarm, the alarm state is
/// immediately set to INSUFFICIENT_DATA."
///
/// It is also where an alarm of this store STAYS until something moves it: the
/// store holds no metric datapoints at all (this emulator runs no instance,
/// container, load balancer or database workload and receives no measurement of
/// one, which is why `GetInstanceMetricData` and
/// `GetContainerServiceMetricData` both answer an empty `metricData`; the
/// remaining `Get*MetricData` operations are still scaffolds with their own
/// issues), and
/// the modelled `Alarm$state`
/// documentation defines `INSUFFICIENT_DATA` as exactly that — "the metric is not
/// available, or not enough data is available for the metric to determine the
/// alarm state". Nothing here invents an evaluation that no metric data could
/// support.
pub const ALARM_STATE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The one `AlarmState` a notification trigger list must name, from the modelled
/// `PutAlarmRequest$notificationTriggers` documentation: "When you specify a
/// notification trigger, the ALARM state must be specified."
pub const ALARM_STATE_ALARM: &str = "ALARM";

/// The `treatMissingData` an alarm gets when the request names none, from that
/// member's own documentation: "If treatMissingData is not specified, the default
/// behavior of missing is used."
pub const TREAT_MISSING_DATA_DEFAULT: &str = "missing";

/// `Alarm$notificationEnabled` when the request names none, from the modelled
/// `PutAlarmRequest$notificationEnabled` documentation: "Notifications are
/// enabled by default if you don't specify this parameter."
pub const NOTIFICATION_ENABLED_DEFAULT: bool = true;

/// `Alarm$period`, the period in seconds the statistic is applied over.
///
/// From the modelled `PutAlarmRequest$evaluationPeriods` documentation: "Each
/// evaluation period is five minutes long." Five minutes is 300 seconds, which is
/// within the modelled `MetricPeriod` range (60..=86400).
pub const ALARM_PERIOD_SECONDS: i32 = 300;

/// The fewest `evaluationPeriods` an alarm accepts, from that member's own
/// documentation: "You can specify a minimum valuation period of 1 (5 minutes),
/// and a maximum evaluation period of 288 (24 hours)."
pub const MIN_EVALUATION_PERIODS: i32 = 1;
/// The most `evaluationPeriods` an alarm accepts. See [`MIN_EVALUATION_PERIODS`].
pub const MAX_EVALUATION_PERIODS: i32 = 288;

/// The most alarms one metric of one monitored resource may carry, from the
/// modelled `PutAlarmRequest$metricName` documentation: "You can configure up to
/// two alarms per metric."
pub const MAX_ALARMS_PER_METRIC: usize = 2;

/// How many alarms one `GetAlarms` page carries.
///
/// The modelled `GetAlarmsResult$nextPageToken` only says a paginated response
/// returns one, so the page size is this emulator's: small enough that a handful
/// of alarms exercises the walk.
pub const ALARM_PAGE_SIZE: usize = 10;

/// One metric an alarm can watch: which resource type publishes it, and the
/// `unit` and `statistic` the service reports on an alarm built from it.
///
/// `PutAlarm` takes neither the unit nor the statistic, yet `Alarm` reports both,
/// so each is derived from the metric — it is a property of the metric, not of
/// the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricSpec {
    /// A value of the modelled `MetricName` enum.
    pub name: &'static str,
    /// The resource type that publishes it, from the `RESOURCE_TYPE_*`
    /// constants.
    pub resource_type: &'static str,
    /// `Alarm$unit`: a value of the modelled `MetricUnit` enum.
    pub unit: &'static str,
    /// `Alarm$statistic`: a value of the modelled `MetricStatistic` enum.
    pub statistic: &'static str,
}

/// Every metric an alarm can watch, by resource type.
///
/// WHICH metrics each resource type publishes is the list in the modelled
/// `PutAlarmRequest$metricName` documentation ("The following metrics are
/// available for each resource type"), which is narrower than the `MetricName`
/// enum: `FreeableMemory` and `SwapUsage` are enum members that list names for no
/// resource type, so no alarm can be put on them.
///
/// The `unit` of each row is the "published `unit` value" that metric's own
/// documentation states, and the `statistic` is the one it names as most useful —
/// `Average` where it names several including `Average`, otherwise the first it
/// names. The sources are
/// `smithy_query(query="doc",shape="GetInstanceMetricDataRequest",member="metricName")`,
/// the same for `GetLoadBalancerMetricDataRequest` and for
/// `GetRelationalDatabaseMetricDataRequest`.
pub const ALARM_METRICS: &[MetricSpec] = &[
    // Instances.
    MetricSpec {
        name: "BurstCapacityPercentage",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Percent",
        statistic: "Average",
    },
    MetricSpec {
        name: "BurstCapacityTime",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "CPUUtilization",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Percent",
        statistic: "Average",
    },
    MetricSpec {
        name: "NetworkIn",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "NetworkOut",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "StatusCheckFailed",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "StatusCheckFailed_Instance",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "StatusCheckFailed_System",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskReadOps",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskWriteOps",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskReadBytes",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskWriteBytes",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskTotalReadTime",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "DiskTotalWriteTime",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "DiskQueueLength",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Average",
    },
    // Load balancers.
    MetricSpec {
        name: "ClientTLSNegotiationErrorCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HealthyHostCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Average",
    },
    MetricSpec {
        name: "UnhealthyHostCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Average",
    },
    MetricSpec {
        name: "HTTPCode_LB_4XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_LB_5XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_2XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_3XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_4XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_5XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "InstanceResponseTime",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "RejectedConnectionCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "RequestCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    // Relational databases.
    MetricSpec {
        name: "CPUUtilization",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Percent",
        statistic: "Average",
    },
    MetricSpec {
        name: "DatabaseConnections",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Count",
        statistic: "Maximum",
    },
    MetricSpec {
        name: "DiskQueueDepth",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "FreeStorageSpace",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "NetworkReceiveThroughput",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Bytes/Second",
        statistic: "Average",
    },
    MetricSpec {
        name: "NetworkTransmitThroughput",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Bytes/Second",
        statistic: "Average",
    },
];

/// The metric `metric_name` names on a resource of `resource_type`, or `None`
/// when that resource type does not publish it — in which case no alarm can watch
/// it there.
pub fn alarm_metric(resource_type: &str, metric_name: &str) -> Option<&'static MetricSpec> {
    ALARM_METRICS
        .iter()
        .find(|spec| spec.resource_type == resource_type && spec.name == metric_name)
}

/// Every metric name a resource of `resource_type` publishes, comma separated —
/// what a refusal names so the caller can pick one that exists.
pub fn alarm_metric_names(resource_type: &str) -> String {
    ALARM_METRICS
        .iter()
        .filter(|spec| spec.resource_type == resource_type)
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A resource an alarm monitors, as the alarm has to report it.
///
/// From the modelled `PutAlarmRequest$monitoredResourceName` documentation:
/// "Instances, load balancers, and relational databases are the only Lightsail
/// resources that can currently be monitored by alarms." Of those three, this
/// slice stores instances and relational databases; a load balancer cannot be
/// created yet, so no name resolves to one.
#[derive(Debug, Clone)]
pub struct MonitoredResource {
    pub name: String,
    pub arn: String,
    pub resource_type: &'static str,
    /// The monitored resource's own location: an alarm watches a resource of one
    /// region, so this is the region the alarm itself lives in.
    pub location: StoredLocation,
}

impl MonitoredResource {
    /// This resource as the modelled `MonitoredResourceInfo`.
    pub fn view(&self) -> MonitoredResourceInfo {
        MonitoredResourceInfo {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            resource_type: Some(self.resource_type.to_string()),
        }
    }
}

/// One stored alarm: the configuration `PutAlarm` accepted, the members the
/// service assigned, and the state the alarm is in.
#[derive(Debug, Clone)]
pub struct StoredAlarm {
    /// The alarm name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// The resource this alarm watches, as it stood when the alarm was put.
    pub monitored_resource: MonitoredResource,
    /// The metric this alarm watches, resolved against [`ALARM_METRICS`]: the
    /// `unit` and `statistic` every read reports come from it.
    pub metric: MetricSpec,
    pub comparison_operator: String,
    pub threshold: f64,
    pub evaluation_periods: i32,
    pub datapoints_to_alarm: Option<i32>,
    pub treat_missing_data: String,
    pub contact_protocols: Vec<String>,
    pub notification_triggers: Vec<String>,
    pub notification_enabled: bool,
    pub tags: Vec<Tag>,
    /// The current `AlarmState`. A create sets
    /// [`ALARM_STATE_INSUFFICIENT_DATA`]; an update leaves whatever is here
    /// alone, because "when you update an existing alarm, its state is left
    /// unchanged".
    pub state: String,
}

impl StoredAlarm {
    /// This record as the modelled `Alarm`.
    pub fn view(&self) -> Alarm {
        Alarm {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_ALARM.to_string()),
            support_code: Some(self.support_code.clone()),
            monitored_resource_info: Some(self.monitored_resource.view()),
            comparison_operator: Some(self.comparison_operator.clone()),
            evaluation_periods: Some(self.evaluation_periods),
            period: Some(ALARM_PERIOD_SECONDS),
            threshold: Some(self.threshold),
            datapoints_to_alarm: self.datapoints_to_alarm,
            treat_missing_data: Some(self.treat_missing_data.clone()),
            statistic: Some(self.metric.statistic.to_string()),
            metric_name: Some(self.metric.name.to_string()),
            state: Some(self.state.clone()),
            unit: Some(self.metric.unit.to_string()),
            contact_protocols: Some(self.contact_protocols.clone()),
            notification_triggers: Some(self.notification_triggers.clone()),
            notification_enabled: Some(self.notification_enabled),
            tags: Some(self.tags.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Load balancers
// ---------------------------------------------------------------------------

/// `LoadBalancerState` values this slice uses, from the modelled enum and the
/// `LoadBalancer$state` documentation ("The status of your load balancer"): a
/// load balancer Lightsail is still standing up is `provisioning`, and one that
/// is serving is `active`.
///
/// The enum's `active_impaired`, `failed` and `unknown` members describe a load
/// balancer whose instances are unhealthy, one whose creation failed, and one
/// whose status Lightsail cannot report — none of which this store produces,
/// because nothing here fails a provision.
pub const LOAD_BALANCER_STATE_PROVISIONING: &str = "provisioning";
/// See [`LOAD_BALANCER_STATE_PROVISIONING`].
pub const LOAD_BALANCER_STATE_ACTIVE: &str = "active";

/// `LoadBalancerProtocol` values, from the modelled enum and the
/// `LoadBalancer$protocol` documentation: "You can't just have `HTTP_HTTPS`, but
/// you can have just `HTTP`" — so a load balancer with no SSL/TLS certificate
/// ATTACHED serves `HTTP`, and one with an attached certificate serves both.
pub const LOAD_BALANCER_PROTOCOL_HTTP: &str = "HTTP";
/// See [`LOAD_BALANCER_PROTOCOL_HTTP`].
pub const LOAD_BALANCER_PROTOCOL_HTTP_HTTPS: &str = "HTTP_HTTPS";

/// The public ports a load balancer listens on, from the modelled
/// `LoadBalancer$publicPorts` documentation: "For HTTP, use port 80. For HTTPS,
/// use port 443."
pub const LOAD_BALANCER_HTTP_PORT: i32 = 80;
/// See [`LOAD_BALANCER_HTTP_PORT`].
pub const LOAD_BALANCER_HTTPS_PORT: i32 = 443;

/// The health check path a load balancer uses when the request named none, from
/// the modelled `CreateLoadBalancerRequest$healthCheckPath` documentation: "If
/// you didn't specify a health check path, Lightsail uses the root path of your
/// website (`"/"`)."
pub const DEFAULT_HEALTH_CHECK_PATH: &str = "/";

/// The most load balancers this account may hold in ONE Region.
///
/// An AWS quota, not an emulator guard: the modelled `CreateLoadBalancer`
/// documentation states it — "You can create up to 10 load balancers per AWS
/// Region in your account."
pub const MAX_LOAD_BALANCERS_PER_REGION: usize = 10;

/// How many load balancers one `GetLoadBalancers` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more load balancers remain. Kept
/// below [`MAX_LOAD_BALANCERS_PER_REGION`] so an account that holds the most
/// load balancers one Region admits still pages.
pub const LOAD_BALANCER_PAGE_SIZE: usize = 5;

/// The modelled `LoadBalancerAttributeName` members, which are the keys of
/// `LoadBalancer$configurationOptions` ("A string to string map of the
/// configuration options for your load balancer").
pub const LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH: &str = "HealthCheckPath";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_ENABLED: &str = "SessionStickinessEnabled";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_COOKIE_DURATION: &str =
    "SessionStickiness_LB_CookieDurationSeconds";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_HTTPS_REDIRECTION_ENABLED: &str = "HttpsRedirectionEnabled";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_TLS_POLICY_NAME: &str = "TlsPolicyName";

/// The session-stickiness cookie lifetime a load balancer starts with, in
/// seconds.
///
/// A Lightsail load balancer IS an Elastic Load Balancer, whose
/// `stickiness.lb_cookie.duration_seconds` attribute AWS documents as
/// "`86400` seconds (1 day)" by default; Lightsail surfaces that same attribute
/// as `SessionStickiness_LB_CookieDurationSeconds`.
pub const DEFAULT_SESSION_STICKINESS_COOKIE_DURATION_SECONDS: i64 = 86_400;

/// The DNS suffix a load balancer's `dnsName` is published under.
///
/// AWS publishes it under its own `elb.amazonaws.com` zone. That HOSTNAME is
/// environment-specific and no caller may assume it, while the PROMISE
/// `LoadBalancer$dnsName` makes is not — it is "The DNS name of your Lightsail
/// load balancer", so it must never be a name that sends the next client OUT of
/// the emulator. This emulator owns no DNS zone, so the name keeps the ELB SHAPE
/// (`<load balancer name>-<id>.<region>`) under a suffix that belongs to the
/// emulator and cannot resolve off-machine, exactly as a managed database's
/// master endpoint does ([`RELATIONAL_DATABASE_ENDPOINT_SUFFIX`]): `.local` is
/// reserved for link-local multicast name resolution (RFC 6762 §3).
///
/// This emulator terminates no HTTP traffic on it: a load balancer's DATA plane
/// is not served. Everything the control plane owns — the record, its state,
/// its ports, its configuration options and its certificate summaries — is
/// real.
const LOAD_BALANCER_DNS_SUFFIX: &str = "lb.lightsail.local";

/// How many characters of the load balancer's own id go into its `dnsName`: the
/// 10-character suffix ELB puts between the load balancer's name and its Region
/// (`my-load-balancer-1234567890.us-east-2.elb.amazonaws.com`).
const LOAD_BALANCER_DNS_ID_LEN: usize = 10;

/// The ARN of the load balancer `id` in `region`.
pub fn load_balancer_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LOAD_BALANCER_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `dnsName` of the load balancer `name` in `region`, minted as `id`. See
/// [`LOAD_BALANCER_DNS_SUFFIX`].
pub fn load_balancer_dns_name(region: &str, name: &str, id: &str) -> String {
    let token: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(LOAD_BALANCER_DNS_ID_LEN)
        .collect();
    format!("{name}-{token}.{region}.{LOAD_BALANCER_DNS_SUFFIX}")
}

/// The SSL/TLS certificate a load balancer holds: the one
/// `CreateLoadBalancer`'s certificate members asked for, or one
/// `CreateLoadBalancerTlsCertificate` added.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancerCertificate {
    /// The certificate name the request named, kept verbatim — it is what every
    /// certificate operation addresses this certificate by.
    pub name: String,
    /// The domain the certificate is associated with.
    pub domain_name: String,
    /// The alternative domains and subdomains the certificate also covers.
    pub alternative_names: Vec<String>,
    /// Whether the certificate is attached to the load balancer, which is what
    /// `LoadBalancerTlsCertificateSummary$isAttached` reports. A certificate is
    /// attached only by `AttachLoadBalancerTlsCertificate`, never by the call
    /// that requested it: it has to be validated first.
    pub is_attached: bool,
}

/// One stored load balancer: the configuration the caller asked for, the members
/// the service assigned, and the lifecycle it is in.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancer {
    /// The load balancer name the request named, kept verbatim — it is this
    /// record's store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    /// Where the load balancer lives. A load balancer carries no availability
    /// zone: the modelled `LoadBalancer$location` documentation says "Lightsail
    /// automatically creates your load balancer across Availability Zones", so
    /// there is no single zone to name.
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub dns_name: String,
    /// The current `LoadBalancerState`.
    pub state: &'static str,
    /// The state the clock owes this load balancer, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instance port traffic is directed to, kept as the request asked.
    pub instance_port: i32,
    pub health_check_path: String,
    pub ip_address_type: String,
    /// The TLS security policy the request named, or `None` when it named none.
    pub tls_policy_name: Option<String>,
    /// The certificate this load balancer holds, if any.
    pub certificate: Option<StoredLoadBalancerCertificate>,
    /// The instances registered with this load balancer, in registration order
    /// — what `instanceHealthSummary` reports one entry per.
    pub attached_instances: Vec<String>,
    /// Whether session stickiness is on, and for how long its cookie lives.
    ///
    /// `CreateLoadBalancer` exposes no stickiness parameter at all and its
    /// documentation says "To change additional load balancer settings, use the
    /// UpdateLoadBalancerAttribute operation", so a load balancer is created
    /// without stickiness and that operation is what turns it on.
    pub session_stickiness_enabled: bool,
    /// See [`DEFAULT_SESSION_STICKINESS_COOKIE_DURATION_SECONDS`].
    pub session_stickiness_cookie_duration_seconds: i64,
    /// Whether HTTP requests are redirected to HTTPS. Off on a load balancer
    /// this call just created, for the same reason stickiness is: it is an
    /// `UpdateLoadBalancerAttribute` setting, and there is no HTTPS listener to
    /// redirect to until a certificate is attached.
    pub https_redirection_enabled: bool,
}

impl StoredLoadBalancer {
    /// Whether an SSL/TLS certificate is ATTACHED to this load balancer, which
    /// is what decides its protocol and its public ports.
    fn serves_https(&self) -> bool {
        self.certificate
            .as_ref()
            .is_some_and(|certificate| certificate.is_attached)
    }

    /// The modelled `configurationOptions` map: one entry per
    /// `LoadBalancerAttributeName` this record holds a value for.
    fn configuration_options(&self) -> HashMap<String, String> {
        let mut options = HashMap::new();
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH.to_string(),
            self.health_check_path.clone(),
        );
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_ENABLED.to_string(),
            self.session_stickiness_enabled.to_string(),
        );
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_COOKIE_DURATION.to_string(),
            self.session_stickiness_cookie_duration_seconds.to_string(),
        );
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_HTTPS_REDIRECTION_ENABLED.to_string(),
            self.https_redirection_enabled.to_string(),
        );
        if let Some(tls_policy_name) = &self.tls_policy_name {
            options.insert(
                LOAD_BALANCER_ATTRIBUTE_TLS_POLICY_NAME.to_string(),
                tls_policy_name.clone(),
            );
        }
        options
    }

    /// This record as the modelled `LoadBalancer`.
    pub fn view(&self) -> LoadBalancer {
        let serves_https = self.serves_https();
        LoadBalancer {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_LOAD_BALANCER.to_string()),
            tags: Some(self.tags.clone()),
            dns_name: Some(self.dns_name.clone()),
            state: Some(self.state.to_string()),
            protocol: Some(
                if serves_https {
                    LOAD_BALANCER_PROTOCOL_HTTP_HTTPS
                } else {
                    LOAD_BALANCER_PROTOCOL_HTTP
                }
                .to_string(),
            ),
            public_ports: Some(if serves_https {
                vec![LOAD_BALANCER_HTTP_PORT, LOAD_BALANCER_HTTPS_PORT]
            } else {
                vec![LOAD_BALANCER_HTTP_PORT]
            }),
            health_check_path: Some(self.health_check_path.clone()),
            instance_port: Some(self.instance_port),
            // A registered instance starts out `initial`, and the modelled
            // `instanceHealthReason` documentation gives the reason for that:
            // "`Lb.RegistrationInProgress` - The target instance is in the
            // process of being registered with the load balancer." This
            // emulator runs no health checks, so no instance ever leaves it.
            instance_health_summary: Some(
                self.attached_instances
                    .iter()
                    .map(|instance_name| InstanceHealthSummary {
                        instance_name: Some(instance_name.clone()),
                        instance_health: Some(INSTANCE_HEALTH_INITIAL.to_string()),
                        instance_health_reason: Some(
                            INSTANCE_HEALTH_REASON_REGISTRATION_IN_PROGRESS.to_string(),
                        ),
                    })
                    .collect(),
            ),
            tls_certificate_summaries: Some(
                self.certificate
                    .iter()
                    .map(|certificate| LoadBalancerTlsCertificateSummary {
                        name: Some(certificate.name.clone()),
                        is_attached: Some(certificate.is_attached),
                    })
                    .collect(),
            ),
            configuration_options: Some(self.configuration_options()),
            ip_address_type: Some(self.ip_address_type.clone()),
            https_redirection_enabled: Some(self.https_redirection_enabled),
            tls_policy_name: self.tls_policy_name.clone(),
        }
    }
}

/// The `InstanceHealthState` an instance registered with a load balancer starts
/// in, from the modelled enum and the `instanceHealthReason` documentation.
pub const INSTANCE_HEALTH_INITIAL: &str = "initial";
/// See [`INSTANCE_HEALTH_INITIAL`].
pub const INSTANCE_HEALTH_REASON_REGISTRATION_IN_PROGRESS: &str = "Lb.RegistrationInProgress";

// ---------------------------------------------------------------------------
// CDN distributions
// ---------------------------------------------------------------------------

/// The `resourceType` a content delivery network distribution reports, from the
/// modelled `ResourceType` enum — the value the
/// `LightsailDistribution$resourceType` documentation names ("The Lightsail
/// resource type (`Distribution`)").
pub const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The resource portion a distribution ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const DISTRIBUTION_ARN_RESOURCE_PREFIX: &str = "Distribution/";

/// The Region every distribution is located in, whichever Region its origin is
/// in and whichever Region a request is made from.
///
/// From the modelled `LightsailDistribution$location` documentation: "Lightsail
/// distributions are global resources that can reference an origin in any
/// Amazon Web Services Region, and distribute its content globally. However,
/// all distributions are located in the `us-east-1` Region."
pub const DISTRIBUTION_REGION_NAME: &str = "us-east-1";

/// `LightsailDistribution$status` while the configuration a call stored is
/// still being deployed to the CDN, and once it has been.
///
/// The member is a plain string in the model ("The status of the
/// distribution"). A Lightsail distribution IS an Amazon CloudFront
/// distribution, and it carries CloudFront's own distribution status values:
/// `InProgress` while a change propagates and `Deployed` once, as the
/// CloudFront API documents for `Distribution$Status`, "the distribution's
/// information is fully propagated to all CloudFront edge locations".
pub const DISTRIBUTION_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`DISTRIBUTION_STATUS_IN_PROGRESS`].
pub const DISTRIBUTION_STATUS_DEPLOYED: &str = "Deployed";

/// The most distributions this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. A create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DISTRIBUTIONS: usize = 100;

/// The most per-path cache behaviours one distribution may carry.
///
/// Emulator resource guard: the modelled `CacheBehaviorList` declares no length,
/// so this is where the bound on a caller-sized list lives.
pub const MAX_CACHE_BEHAVIORS_PER_DISTRIBUTION: usize = 25;

/// The most custom error responses one distribution may carry.
///
/// Emulator resource guard, for the same reason as
/// [`MAX_CACHE_BEHAVIORS_PER_DISTRIBUTION`]: the modelled
/// `DistributionCustomErrorResponseList` declares no length.
pub const MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION: usize = 25;

/// How many distributions one `GetDistributions` page carries.
///
/// The operation is paginated — its `pageToken` is "The token to advance to the
/// next page of results from your request" — but the model states no page size,
/// so this is the store's own, small enough that a modest number of
/// distributions already exercises the paging.
pub const DISTRIBUTION_PAGE_SIZE: usize = 10;

/// The smallest and largest `responseTimeout`, in seconds, an origin may be
/// given, and the one an origin that names none gets.
///
/// From the modelled `InputOrigin$responseTimeout` documentation: "The minimum
/// timeout is 1 second, the maximum is 60 seconds, and the default (if you
/// don't specify otherwise) is 30 seconds."
pub const ORIGIN_RESPONSE_TIMEOUT_SECONDS_RANGE: (i32, i32) = (1, 60);
/// See [`ORIGIN_RESPONSE_TIMEOUT_SECONDS_RANGE`].
pub const DEFAULT_ORIGIN_RESPONSE_TIMEOUT_SECONDS: i32 = 30;

/// The DNS suffix a distribution's `domainName` is published under.
///
/// AWS publishes it in CloudFront's own `cloudfront.net` zone. That HOSTNAME is
/// environment-specific and no caller may assume it, while the promise
/// `LightsailDistribution$domainName` makes — "The domain name of the
/// distribution" — is not: it must never be a name that sends the next client
/// OUT of the emulator. This emulator owns no DNS zone, so the name keeps the
/// CloudFront SHAPE (`d<token>`) under a suffix that belongs to the emulator and
/// cannot resolve off-machine, exactly as a load balancer's `dnsName` does (see
/// [`LOAD_BALANCER_DNS_SUFFIX`]): `.local` is reserved for link-local multicast
/// name resolution (RFC 6762 §3).
///
/// This emulator serves no CDN data plane on it: a distribution caches and
/// serves nothing. Everything the control plane owns — the record, its status,
/// its bundle, its origin, its cache configuration and its operation log entry
/// — is real.
const DISTRIBUTION_DOMAIN_SUFFIX: &str = "cdn.lightsail.local";

/// How many characters of the distribution's own id go into its `domainName`:
/// the 13 CloudFront puts after the leading `d` of a distribution domain name
/// (`d111111abcdef8.cloudfront.net`).
const DISTRIBUTION_DOMAIN_ID_LEN: usize = 13;

/// One Lightsail **distribution** bundle: the monthly cost and monthly network
/// transfer quota a distribution gets from that plan.
///
/// This is the service's published price list for CDN distributions, so it is
/// the same for every caller and no call changes it — `GetDistributionBundles`
/// reports it, and `CreateDistribution`/`UpdateDistributionBundle` name an entry
/// of it by `bundle_id`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistributionBundleSpec {
    /// The id a caller sends for this plan. A distribution bundle id is
    /// `<size>_<major>_<minor>`, the same form a bucket bundle id carries.
    pub bundle_id: &'static str,
    /// `DistributionBundle$transferPerMonthInGb` — "The monthly network transfer
    /// quota of the bundle", in GB.
    pub transfer_per_month_in_gb: i32,
    /// `DistributionBundle$price` — "The monthly price, in US dollars, of the
    /// bundle."
    pub price_usd_per_month: f32,
    /// `DistributionBundle$isActive` — "Indicates whether the bundle is active,
    /// and can be specified for a new or existing distribution."
    pub is_active: bool,
}

/// Every distribution bundle this service publishes, cheapest first: the
/// Lightsail pricing page's CDN distribution table — 50 GB of data transfer for
/// $2.50 a month, 200 GB for $5 and 500 GB for $10 — each the first (`_1_0`)
/// generation of its size.
///
/// Lightsail has retired no distribution plan, so every published plan is
/// active. A retired generation belongs in this table with `is_active: false`:
/// that is what makes it unavailable to a new distribution while one already
/// running on it still resolves.
pub const DISTRIBUTION_BUNDLE_SPECS: &[DistributionBundleSpec] = &[
    DistributionBundleSpec {
        bundle_id: "small_1_0",
        transfer_per_month_in_gb: 50,
        price_usd_per_month: 2.5,
        is_active: true,
    },
    DistributionBundleSpec {
        bundle_id: "medium_1_0",
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 5.0,
        is_active: true,
    },
    DistributionBundleSpec {
        bundle_id: "large_1_0",
        transfer_per_month_in_gb: 500,
        price_usd_per_month: 10.0,
        is_active: true,
    },
];

/// The label Lightsail gives a CDN plan, with `transfer` the plan's monthly
/// network transfer quota in GB.
///
/// SOURCE: the `DistributionBundle$name` values Lightsail reports, which name
/// the plan's transfer quota (`50GB`). The label is therefore DERIVED from the
/// quota rather than carried per entry: a plan added to
/// [`DISTRIBUTION_BUNDLE_SPECS`] is labelled the same way.
pub fn distribution_bundle_name(transfer_per_month_in_gb: i32) -> String {
    format!("{transfer_per_month_in_gb}GB")
}

impl DistributionBundleSpec {
    /// This plan as the modelled `DistributionBundle`.
    ///
    /// `name` is "The name of the distribution bundle", so it is DERIVED from
    /// [`Self::transfer_per_month_in_gb`] by [`distribution_bundle_name`] rather
    /// than carried a second time.
    pub fn view(&self) -> DistributionBundle {
        DistributionBundle {
            bundle_id: Some(self.bundle_id.to_string()),
            name: Some(distribution_bundle_name(self.transfer_per_month_in_gb)),
            price: Some(self.price_usd_per_month),
            transfer_per_month_in_gb: Some(self.transfer_per_month_in_gb),
            is_active: Some(self.is_active),
        }
    }
}

/// The distribution bundle `bundle_id` names — active or not, since a
/// distribution already running on a retired plan still has to resolve — or
/// `None` when this service publishes no such plan.
pub fn distribution_bundle_spec(bundle_id: &str) -> Option<DistributionBundleSpec> {
    DISTRIBUTION_BUNDLE_SPECS
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .copied()
}

/// Whether a NEW distribution may be created on the plan `bundle_id` names: the
/// plan has to be one this service publishes AND still active, because
/// `DistributionBundle$isActive` is what says a bundle "can be specified for a
/// new or existing distribution".
pub fn distribution_bundle_is_creatable(bundle_id: &str) -> bool {
    distribution_bundle_spec(bundle_id).is_some_and(|spec| spec.is_active)
}

/// The ARN of the distribution `id`. Every distribution is located in
/// [`DISTRIBUTION_REGION_NAME`], so that is the Region its ARN carries.
pub fn distribution_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        DISTRIBUTION_REGION_NAME,
        &format!("{DISTRIBUTION_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `domainName` of the distribution minted as `id`. See
/// [`DISTRIBUTION_DOMAIN_SUFFIX`].
pub fn distribution_domain_name(id: &str) -> String {
    let token: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(DISTRIBUTION_DOMAIN_ID_LEN)
        .collect();
    format!("d{token}.{DISTRIBUTION_DOMAIN_SUFFIX}")
}

/// What an origin name resolves to: the kind of resource it addresses, and the
/// public DNS name this store publishes for that resource, if it publishes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOrigin {
    /// The resolved resource's own `ResourceType` value, which is what
    /// `Origin$resourceType` reports.
    pub resource_type: &'static str,
    /// `Origin$originPublicDNS`, "The public DNS of the origin" — filled only
    /// for a resource this store publishes a public DNS NAME for (a load
    /// balancer's `dnsName`). An instance is published at an IP address and a
    /// bucket and a container service at an address under the endpoint the
    /// request arrived at, so neither has a public DNS name to report and the
    /// optional member is left out rather than guessed at.
    pub public_dns: Option<String>,
}

/// One stored distribution: the CDN distribution `CreateDistribution` created.
///
/// What the caller asked for — name, bundle, origin, cache behaviour and
/// settings, custom error responses, IP address type, certificate name, minimum
/// viewer TLS version, default root object and tags — is kept verbatim; the ARN,
/// support code, creation time, domain name, status and the origin's resolved
/// resource type are assigned by this store from the request plus its own
/// allocators.
#[derive(Debug, Clone)]
pub struct StoredDistribution {
    /// The distribution name the request named, kept verbatim — it is this
    /// record's store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    /// Where the distribution lives: [`DISTRIBUTION_REGION_NAME`], and no
    /// availability zone, because a distribution is a global resource rather
    /// than a zonal one.
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The address this distribution is published at, minted by
    /// [`distribution_domain_name`].
    pub domain_name: String,
    /// One of [`DISTRIBUTION_STATUS_IN_PROGRESS`] /
    /// [`DISTRIBUTION_STATUS_DEPLOYED`].
    pub status: &'static str,
    /// The status the clock owes this distribution, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// Whether the distribution is enabled. `CreateDistribution` exposes no
    /// `isEnabled` parameter and `UpdateDistribution`'s documentation gives it
    /// as the way to "enable the distribution", so a created distribution is
    /// enabled and that operation is what turns it off.
    pub is_enabled: bool,
    /// The bundle currently applied, an id of [`DISTRIBUTION_BUNDLE_SPECS`].
    pub bundle_id: String,
    /// Whether the applied bundle may still be changed. A distribution's bundle
    /// can be updated "only one time within your monthly Amazon Web Services
    /// billing cycle" (`UpdateDistributionBundle`), so one that has not been
    /// moved yet can be.
    pub able_to_update_bundle: bool,
    /// The SSL/TLS certificate attached to the distribution, if the request
    /// named one.
    pub certificate_name: Option<String>,
    /// The alternate domain names of the distribution, which come from the
    /// domains of an attached certificate: a distribution this call created has
    /// none of its own.
    pub alternative_domain_names: Vec<String>,
    /// The origin the distribution pulls content from: what the request asked
    /// for, plus the resource type this store resolved the origin name to.
    pub origin: Origin,
    /// See [`ResolvedOrigin::public_dns`].
    pub origin_public_dns: Option<String>,
    pub default_cache_behavior: CacheBehavior,
    /// The cache behaviour settings the request carried, or `None` when it
    /// carried none — the model documents no default for any member of
    /// `CacheSettings`, so nothing is invented for a request that set none.
    pub cache_behavior_settings: Option<CacheSettings>,
    pub cache_behaviors: Vec<CacheBehaviorPerPath>,
    pub ip_address_type: String,
    pub viewer_minimum_tls_protocol_version: Option<String>,
    pub default_root_object: Option<String>,
    pub custom_error_responses: Vec<DistributionCustomErrorResponse>,
}

impl StoredDistribution {
    /// This record as the modelled `LightsailDistribution`.
    pub fn view(&self) -> LightsailDistribution {
        LightsailDistribution {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISTRIBUTION.to_string()),
            // A list member is reported EMPTY rather than left out: an absent
            // member hands a typed caller a null where it iterates.
            alternative_domain_names: Some(self.alternative_domain_names.clone()),
            status: Some(self.status.to_string()),
            is_enabled: Some(self.is_enabled),
            domain_name: Some(self.domain_name.clone()),
            bundle_id: Some(self.bundle_id.clone()),
            certificate_name: self.certificate_name.clone(),
            origin: Some(self.origin.clone()),
            origin_public_dns: self.origin_public_dns.clone(),
            default_cache_behavior: Some(self.default_cache_behavior.clone()),
            cache_behavior_settings: self.cache_behavior_settings.clone(),
            cache_behaviors: Some(self.cache_behaviors.clone()),
            able_to_update_bundle: Some(self.able_to_update_bundle),
            ip_address_type: Some(self.ip_address_type.clone()),
            tags: Some(self.tags.clone()),
            viewer_minimum_tls_protocol_version: self
                .viewer_minimum_tls_protocol_version
                .clone(),
            default_root_object: self.default_root_object.clone(),
            custom_error_responses: Some(self.custom_error_responses.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Released attachments
// ---------------------------------------------------------------------------

/// One attachment a resource's removal released: everything the operation log
/// needs to record the detachment that removal performed.
///
/// A release is a MUTATION of the record that held the attachment, so it is
/// logged like any other, under the modelled `OperationType` member for
/// detaching that kind of resource and located where that resource lives.
#[derive(Debug, Clone)]
pub struct ReleasedAttachment {
    /// The `Operation$operationType` this release is logged under, from the
    /// modelled `OperationType` enum.
    pub operation_type: &'static str,
    /// The `Operation$resourceType` of the record the attachment was released
    /// on, from the modelled `ResourceType` enum.
    pub resource_type: &'static str,
    /// That record's own name — the resource the log entry is about.
    pub resource_name: String,
    /// Where that record lives, so the entry is located where the resource is
    /// rather than where the removed resource was.
    pub location: StoredLocation,
}

/// The outcome of removing one instance: the record itself, and every
/// attachment the removal released
/// (see [`ComAmazonawsLightsailLightsail20161128State::remove_instance`]).
#[derive(Debug)]
pub struct RemovedInstance {
    /// The instance record that was removed.
    pub instance: StoredInstance,
    /// One entry per attachment the removal released, in the store's own
    /// stable order: disks by name, then static IPs by name, then load
    /// balancers by region and name — every one of those maps is a
    /// `BTreeMap`, so the order is the same on every call.
    pub released_attachments: Vec<ReleasedAttachment>,
}

// ---------------------------------------------------------------------------
// VPC peering
// ---------------------------------------------------------------------------

/// Where one Region's Lightsail VPC peering is in its lifecycle.
///
/// Lightsail peers a Region's Lightsail VPC with the account's default VPC in
/// that Region on request — `PeerVpc` "peers the Lightsail VPC with the user's
/// default VPC", `UnpeerVpc` "unpeers the Lightsail VPC from the user's default
/// VPC" — and each of those calls answers an operation-log entry rather than the
/// finished connection, so the connection is NOT established (nor torn down) at
/// the instant the call returns: the clock owes it the rest of the walk, like
/// every other non-terminal Lightsail resource in this store.
///
/// `IsVpcPeered` reports one boolean over this walk, and the boolean is about
/// whether a connection EXISTS: a VPC being peered is not peered yet, and one
/// being unpeered still is (see [`VpcPeeringPhase::is_peered`]). The phases
/// themselves are this store's own — the model declares no shape for a Lightsail
/// VPC peering, only the `PeeredVpc` resource type and that one boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpcPeeringPhase {
    /// `PeerVpc` has been called and the connection is being established.
    Peering,
    /// The connection is established: the Lightsail VPC is peered.
    Peered,
    /// `UnpeerVpc` has been called and the connection is being torn down. It is
    /// still up, so the VPC is still peered.
    Unpeering,
    /// The connection is gone. The Lightsail VPC is not peered, and `PeerVpc`
    /// may peer it again.
    Unpeered,
}

impl VpcPeeringPhase {
    /// Whether a Lightsail VPC in this phase IS peered — the one thing
    /// `IsVpcPeered` answers.
    ///
    /// True from the instant the connection is established until the instant it
    /// is gone, so a caller that peers and reads straight back still observes
    /// the transient "not peered yet", exactly as it does against AWS.
    pub fn is_peered(self) -> bool {
        matches!(self, Self::Peered | Self::Unpeering)
    }
}

/// A phase the clock already owes a Region's Lightsail VPC peering.
///
/// Separate from [`DueTransition`] and [`DueNamedState`] because a peering's
/// phase is neither an EC2 `InstanceState` (a code and a name) nor a modelled
/// state name, but this store's own [`VpcPeeringPhase`].
#[derive(Debug, Clone, Copy)]
pub struct DueVpcPeeringPhase {
    /// Epoch milliseconds at which the phase became (or becomes) due.
    pub at_ms: i64,
    /// The phase the peering settles into.
    pub phase: VpcPeeringPhase,
}

impl DueVpcPeeringPhase {
    /// The walk to `phase`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, phase: VpcPeeringPhase) -> Self {
        Self {
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            phase,
        }
    }
}

/// The peering of ONE Region's Lightsail VPC with the account's default VPC
/// there.
///
/// Per Region, because Lightsail's API is regional and the Lightsail VPC
/// `PeerVpc` peers is the one in the Region the request is made in: peering in
/// one Region leaves every other Region's Lightsail VPC unpeered.
#[derive(Debug, Clone)]
pub struct StoredVpcPeering {
    /// Where the connection is now.
    pub phase: VpcPeeringPhase,
    /// What the clock owes it next, while it owes it anything.
    pub due: Option<DueVpcPeeringPhase>,
}

impl StoredVpcPeering {
    /// The peering a `PeerVpc` call at `now_ms` starts: the connection is being
    /// established, and the clock owes it [`VpcPeeringPhase::Peered`].
    pub fn peering(now_ms: i64) -> Self {
        Self {
            phase: VpcPeeringPhase::Peering,
            due: Some(DueVpcPeeringPhase::after(now_ms, VpcPeeringPhase::Peered)),
        }
    }

    /// Start the teardown an `UnpeerVpc` call at `now_ms` asks for: the
    /// connection is still up, and the clock owes it
    /// [`VpcPeeringPhase::Unpeered`].
    pub fn start_unpeering(&mut self, now_ms: i64) {
        self.phase = VpcPeeringPhase::Unpeering;
        self.due = Some(DueVpcPeeringPhase::after(
            now_ms,
            VpcPeeringPhase::Unpeered,
        ));
    }

    /// Whether this Region's Lightsail VPC is peered.
    pub fn is_peered(&self) -> bool {
        self.phase.is_peered()
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
    /// Every instance snapshot, keyed by its snapshot name — the identifier every
    /// instance snapshot operation takes. A `BTreeMap` so `GetInstanceSnapshots`
    /// walks a stable, total order and a page token can name the next snapshot.
    pub instance_snapshots: BTreeMap<String, StoredInstanceSnapshot>,
    /// Every export snapshot record, keyed by the name this store minted for it
    /// — the only identifier an export snapshot record has, because the caller
    /// names none. A `BTreeMap` so `GetExportSnapshotRecords` walks a stable,
    /// total order and a page token can name the next record.
    pub export_snapshot_records: BTreeMap<String, StoredExportSnapshotRecord>,
    /// The append-only operation log, newest last: the source of truth for the
    /// `operations` every mutating instance call answers with.
    pub operations: Vec<StoredOperation>,
    /// The append-only `SetupInstanceHttps` history, newest last: the source of
    /// truth for what `GetSetupHistory` reports for an instance. A `Vec` for the
    /// same reason the operation log is one — it is a log, several entries of it
    /// name the same instance, and `GetSetupHistory` reports the most recent
    /// ones first.
    pub instance_https_setups: Vec<StoredInstanceHttpsSetup>,
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
    /// Every managed relational database snapshot, keyed by its snapshot name —
    /// the identifier every database snapshot operation takes. A `BTreeMap` so
    /// `GetRelationalDatabaseSnapshots` walks a stable, total order and a page
    /// token can name the next snapshot.
    pub relational_database_snapshots: BTreeMap<String, StoredRelationalDatabaseSnapshot>,
    /// Every static IP, keyed by its static IP name — the identifier every static
    /// IP operation takes, and the same key an instance is held under, because a
    /// static IP is attached to an instance by name. A `BTreeMap` so
    /// `GetStaticIps` walks a stable, total order and a page token can name the
    /// next static IP.
    pub static_ips: BTreeMap<String, StoredStaticIp>,
    /// Every alarm, keyed by its name — the identifier every alarm operation
    /// takes, and the one `PutAlarm` creates or updates a record under. A
    /// `BTreeMap` so `GetAlarms` walks a stable, total order and a page token can
    /// name the next alarm.
    pub alarms: BTreeMap<String, StoredAlarm>,
    /// Every load balancer, keyed by the region it was created in and its name.
    ///
    /// A load balancer is a REGIONAL resource — the modelled
    /// `CreateLoadBalancer` documentation caps them "per AWS Region in your
    /// account" — so the region is part of its identity, exactly as it is for a
    /// container service and a bucket. A `BTreeMap` so a listing walks a stable,
    /// total order and a page token can name the next load balancer.
    pub load_balancers: BTreeMap<(String, String), StoredLoadBalancer>,
    /// The peering of each Region's Lightsail VPC with the account's default VPC
    /// there, keyed by the Region's name. A Region with no entry is one whose
    /// Lightsail VPC nothing has ever asked to peer.
    ///
    /// Unbounded by any `MAX_` of its own, and it needs none: the key is what
    /// [`request_region`] answers, which is always a value of the modelled
    /// `RegionName` enum, so the map holds at most one entry per Region that
    /// enum declares however many calls a caller makes. A `BTreeMap` so the walk
    /// in [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] is in a
    /// stable, total order.
    pub vpc_peerings: BTreeMap<String, StoredVpcPeering>,
    /// Every CDN distribution, keyed by its name — the identifier every
    /// distribution operation takes.
    ///
    /// NOT keyed by region, unlike a bucket or a load balancer: a distribution
    /// is a global resource and "all distributions are located in the
    /// `us-east-1` Region" (`LightsailDistribution$location`), so one name
    /// addresses one distribution of the account whichever Region a request is
    /// made from. A `BTreeMap` so `GetDistributions` walks a stable, total order
    /// and a page token can name the next distribution.
    pub distributions: BTreeMap<String, StoredDistribution>,
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

    /// Remove the domain recordset `name` names, answering it. `None` when
    /// nothing matched.
    ///
    /// A zone's recordset entries live on the record, so removing it removes
    /// "the specified domain recordset and all of its domain records" — what the
    /// modelled `DeleteDomain` documentation says the call deletes — in one go,
    /// with no second map to keep in step.
    pub fn remove_domain(&mut self, name: &str) -> Option<StoredDomain> {
        self.domains.remove(name)
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

    /// The block storage disk `name` names, for an operation the spec says
    /// mutates it — attaching it to an instance, or releasing the attachment it
    /// holds.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames a disk.
    pub fn disk_mut(&mut self, name: &str) -> Option<&mut StoredDisk> {
        self.disks.get_mut(name)
    }

    /// The instance `name` names as the modelled `Instance`, with every block
    /// storage disk attached to it reported in its hardware.
    ///
    /// The one way a reporting operation renders an instance: an attachment
    /// lives on the DISK record, so assembling the view needs the whole store,
    /// not just the instance.
    pub fn instance_view(&self, name: &str) -> Option<Instance> {
        let instance = self.instances.get(name)?;
        Some(instance.view(self.attached_disk_views(name)))
    }

    /// Every block storage disk attached to the instance `instance_name` names,
    /// in the store's own stable order (by disk name).
    pub fn disks_attached_to(&self, instance_name: &str) -> Vec<&StoredDisk> {
        self.disks
            .values()
            .filter(|disk| disk.attached_to.as_deref() == Some(instance_name))
            .collect()
    }

    /// The same disks as [`Self::disks_attached_to`], as the modelled `Disk`.
    pub fn attached_disk_views(&self, instance_name: &str) -> Vec<crate::ops::types::Disk> {
        self.disks_attached_to(instance_name)
            .into_iter()
            .map(StoredDisk::view)
            .collect()
    }

    /// The disk snapshot `name` names, for a reporting operation.
    pub fn disk_snapshot(&self, name: &str) -> Option<&StoredDiskSnapshot> {
        self.disk_snapshots.get(name)
    }

    /// The static IP `name` names, for a reporting operation.
    pub fn static_ip(&self, name: &str) -> Option<&StoredStaticIp> {
        self.static_ips.get(name)
    }

    /// Store a newly allocated static IP.
    pub fn insert_static_ip(&mut self, static_ip: StoredStaticIp) {
        self.static_ips.insert(static_ip.name.clone(), static_ip);
    }

    /// Store a newly created disk snapshot.
    pub fn insert_disk_snapshot(&mut self, snapshot: StoredDiskSnapshot) {
        self.disk_snapshots.insert(snapshot.name.clone(), snapshot);
    }

    /// The instance snapshot `name` names, for a reporting operation.
    pub fn instance_snapshot(&self, name: &str) -> Option<&StoredInstanceSnapshot> {
        self.instance_snapshots.get(name)
    }

    /// Store a newly created instance snapshot.
    pub fn insert_instance_snapshot(&mut self, snapshot: StoredInstanceSnapshot) {
        self.instance_snapshots
            .insert(snapshot.name.clone(), snapshot);
    }

    /// The export snapshot record `name` names, for a reporting operation.
    pub fn export_snapshot_record(&self, name: &str) -> Option<&StoredExportSnapshotRecord> {
        self.export_snapshot_records.get(name)
    }

    /// Store a newly created export snapshot record.
    pub fn insert_export_snapshot_record(&mut self, record: StoredExportSnapshotRecord) {
        self.export_snapshot_records
            .insert(record.name.clone(), record);
    }

    /// Remove the instance `name` names, answering the record and every
    /// attachment its removal released. `None` when nothing matched.
    ///
    /// Removing a resource also releases every attachment another stored
    /// resource holds to it: an attachment names "the resources to which" one
    /// resource "is attached" (the modelled `Disk$attachedTo` and
    /// `StaticIp$attachedTo`), and a resource this account no longer holds is
    /// not one of them — so the removal and the release are one step, and no
    /// read can observe an attachment to a record that is gone.
    pub fn remove_instance(&mut self, name: &str) -> Option<RemovedInstance> {
        let instance = self.instances.remove(name)?;
        let released_attachments = self.release_attachments_to_instance(name);
        Some(RemovedInstance {
            instance,
            released_attachments,
        })
    }

    /// Release every attachment the store holds to the instance `name`,
    /// answering one entry per release so the caller can log it.
    ///
    /// Three kinds of record can hold one: a block storage disk (`attachedTo`
    /// plus the device `path` it is mounted at), a static IP (`attachedTo`), and
    /// a load balancer (the instance registered with it). Each returns to the
    /// state it has while it is attached to nothing — for a disk that is
    /// `available`, the modelled `DiskState` value for a disk that "exists and
    /// is attached to nothing" next to `in-use`.
    ///
    /// Every walk is bounded: the disks by [`MAX_DISKS`], the static IPs by
    /// [`MAX_STATIC_IPS`], and the load balancers by
    /// [`MAX_LOAD_BALANCERS_PER_REGION`] per region.
    fn release_attachments_to_instance(&mut self, name: &str) -> Vec<ReleasedAttachment> {
        let mut released = Vec::new();
        for disk in self.disks.values_mut() {
            if disk.attached_to.as_deref() != Some(name) {
                continue;
            }
            disk.attached_to = None;
            disk.path = None;
            disk.state = DISK_STATE_AVAILABLE;
            // A detachment the clock still owed this disk has nothing left to
            // do: the attachment it was going to release is already released,
            // and leaving the `due` in place would only re-apply the same
            // settle. A `due` of any OTHER state is left alone — it is not this
            // attachment's.
            if disk.due.is_some_and(|due| due.state == DISK_STATE_AVAILABLE) {
                disk.due = None;
            }
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_DISK,
                resource_type: RESOURCE_TYPE_DISK,
                resource_name: disk.name.clone(),
                location: disk.location.clone(),
            });
        }
        for static_ip in self.static_ips.values_mut() {
            if static_ip.attached_to.as_deref() != Some(name) {
                continue;
            }
            // The address stays ALLOCATED to the account — nothing released the
            // reservation, only the attachment — so the record keeps every
            // other member it has.
            static_ip.attached_to = None;
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_STATIC_IP,
                resource_type: RESOURCE_TYPE_STATIC_IP,
                resource_name: static_ip.name.clone(),
                location: static_ip.location.clone(),
            });
        }
        for balancer in self.load_balancers.values_mut() {
            if !balancer
                .attached_instances
                .iter()
                .any(|attached| attached == name)
            {
                continue;
            }
            balancer
                .attached_instances
                .retain(|attached| attached != name);
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_INSTANCES_FROM_LOAD_BALANCER,
                resource_type: RESOURCE_TYPE_LOAD_BALANCER,
                resource_name: balancer.name.clone(),
                location: balancer.location.clone(),
            });
        }
        released
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

    /// The CDN distribution `name` names, for a reporting operation.
    pub fn distribution(&self, name: &str) -> Option<&StoredDistribution> {
        self.distributions.get(name)
    }

    /// Store a newly created CDN distribution.
    pub fn insert_distribution(&mut self, distribution: StoredDistribution) {
        self.distributions
            .insert(distribution.name.clone(), distribution);
    }

    /// What the origin name `name` addresses in `region`, or `None` when this
    /// account holds no resource of that name there.
    ///
    /// An origin "can be a Lightsail instance, bucket, container service, or
    /// load balancer" (`InputOrigin`), and those are looked up in exactly that
    /// order — a name can only be taken by one of them, since each kind refuses
    /// a create whose name is already its own. An instance is held by name
    /// alone, so its own `location` is what decides whether it is in `region`.
    pub fn resolve_origin(&self, region: &str, name: &str) -> Option<ResolvedOrigin> {
        if self
            .instance(name)
            .is_some_and(|instance| instance.location.region_name == region)
        {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_INSTANCE,
                public_dns: None,
            });
        }
        if self.bucket(region, name).is_some() {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_BUCKET,
                public_dns: None,
            });
        }
        if self.container_service(region, name).is_some() {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_CONTAINER_SERVICE,
                public_dns: None,
            });
        }
        if let Some(balancer) = self.load_balancer(region, name) {
            return Some(ResolvedOrigin {
                resource_type: RESOURCE_TYPE_LOAD_BALANCER,
                public_dns: Some(balancer.dns_name.clone()),
            });
        }
        None
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

    /// The managed relational database `name` names, for an operation the spec
    /// says mutates it — setting a runtime parameter of its parameter group.
    ///
    /// `name` is this record's store key, so it may not be reassigned through
    /// this handle; no operation in this slice renames a database.
    pub fn relational_database_mut(
        &mut self,
        name: &str,
    ) -> Option<&mut StoredRelationalDatabase> {
        self.relational_databases.get_mut(name)
    }

    /// Remove the managed relational database `name` names, answering the record
    /// it removed. `None` when nothing matched.
    ///
    /// Nothing else in the store names a database it is attached to — a database
    /// attaches to no instance, disk or load balancer — so this removes exactly
    /// one record. A SNAPSHOT of it is deliberately left where it is: a snapshot
    /// is a point-in-time copy that carries its own copy of everything it reports
    /// about its source ([`StoredRelationalDatabaseSnapshot`]), which is what lets
    /// the final snapshot a delete takes outlive the database it was taken of.
    pub fn remove_relational_database(
        &mut self,
        name: &str,
    ) -> Option<StoredRelationalDatabase> {
        self.relational_databases.remove(name)
    }

    /// The managed relational database snapshot `name` names, for a reporting
    /// operation.
    pub fn relational_database_snapshot(
        &self,
        name: &str,
    ) -> Option<&StoredRelationalDatabaseSnapshot> {
        self.relational_database_snapshots.get(name)
    }

    /// The message a request that would take this store past
    /// [`MAX_RELATIONAL_DATABASE_SNAPSHOTS`] is refused with, or `None` when
    /// there is room for one more snapshot.
    ///
    /// The ONE place that bound is checked, so every operation that creates a
    /// managed relational database snapshot — `CreateRelationalDatabaseSnapshot`,
    /// `StopRelationalDatabase`'s pre-stop snapshot and
    /// `DeleteRelationalDatabase`'s final one — refuses at the same count with
    /// the same wording. Each lifts it into its own modelled
    /// `InvalidInputException`.
    pub fn relational_database_snapshot_capacity_refusal(&self) -> Option<String> {
        (self.relational_database_snapshots.len() >= MAX_RELATIONAL_DATABASE_SNAPSHOTS).then(|| {
            format!(
                "creating a relational database snapshot would take this account past \
                 the {MAX_RELATIONAL_DATABASE_SNAPSHOTS} this emulator holds"
            )
        })
    }

    /// Store a newly created managed relational database snapshot.
    pub fn insert_relational_database_snapshot(
        &mut self,
        snapshot: StoredRelationalDatabaseSnapshot,
    ) {
        self.relational_database_snapshots
            .insert(snapshot.name.clone(), snapshot);
    }

    /// Take a point-in-time copy of `source` under the snapshot name `name`,
    /// store it and answer the record that was stored.
    ///
    /// The ONE place a database snapshot is built, whichever call asks for it —
    /// `CreateRelationalDatabaseSnapshot` on its own, the pre-stop snapshot a
    /// `StopRelationalDatabase` takes before it shuts the engine down, or the
    /// final snapshot a `DeleteRelationalDatabase` takes before it removes the
    /// database — so all of them report the same copy of the same source.
    ///
    /// Everything the snapshot says about its SOURCE — and the master database
    /// name and credentials a later `CreateRelationalDatabaseFromSnapshot`
    /// restores it with — is read out of `source` as it stands now and never
    /// followed afterwards, which is what lets a snapshot outlive the database it
    /// was taken of. The copy is not restorable yet: the
    /// clock owes it [`RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE`], named by
    /// the `due` the answered record carries, so the caller can log the operation
    /// that settle completes.
    ///
    /// PRECONDITION: `name` is a snapshot name this account does not already hold
    /// and the store is below [`MAX_RELATIONAL_DATABASE_SNAPSHOTS`] — a caller
    /// that skipped either check would replace a record a restore may be using,
    /// or grow the store past its bound.
    pub fn snapshot_relational_database(
        &mut self,
        source: &StoredRelationalDatabase,
        name: String,
        tags: Vec<Tag>,
        now_ms: i64,
    ) -> StoredRelationalDatabaseSnapshot {
        let id = self.mint_uuid();
        let snapshot = StoredRelationalDatabaseSnapshot {
            name,
            arn: relational_database_snapshot_arn(&source.location.region_name, &id),
            support_code: support_code(&id),
            created_at_ms: now_ms,
            location: source.location.clone(),
            tags,
            engine: source.blueprint.engine.to_string(),
            engine_version: source.blueprint.engine_version.to_string(),
            // The snapshotted disk is the database bundle's own storage — the
            // size `GetRelationalDatabase` reports as its hardware disk size.
            size_in_gb: source.bundle.disk_size_in_gb,
            // A snapshot is a copy of the DATABASE, so the database name and the
            // master credentials it was created with are captured here too: a
            // later `CreateRelationalDatabaseFromSnapshot` restores a database
            // reached with them, and it must still do so after the source has
            // been deleted. No member of `RelationalDatabaseSnapshot` reports
            // them, exactly as no member of `RelationalDatabase` reports a
            // password.
            master_database_name: source.master_database_name.clone(),
            master_username: source.master_username.clone(),
            master_user_password: source.master_user_password.clone(),
            state: RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING,
            due: Some(DueNamedState::after(
                now_ms,
                RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE,
            )),
            from_relational_database_name: source.name.clone(),
            from_relational_database_arn: source.arn.clone(),
            from_relational_database_bundle_id: source.bundle.bundle_id.to_string(),
            from_relational_database_blueprint_id: source.blueprint.blueprint_id(),
        };
        self.insert_relational_database_snapshot(snapshot.clone());
        snapshot
    }

    /// The alarm `name` names, for a reporting operation.
    pub fn alarm(&self, name: &str) -> Option<&StoredAlarm> {
        self.alarms.get(name)
    }

    /// Store the alarm `PutAlarm` just created or re-configured.
    ///
    /// An alarm name addresses one alarm, so putting a name that is already taken
    /// REPLACES that record: "specify the name of an existing alarm to update, and
    /// overwrite the previous configuration of the alarm".
    pub fn insert_alarm(&mut self, alarm: StoredAlarm) {
        self.alarms.insert(alarm.name.clone(), alarm);
    }

    /// Remove the alarm `name` names, answering it. `None` when nothing matched.
    pub fn remove_alarm(&mut self, name: &str) -> Option<StoredAlarm> {
        self.alarms.remove(name)
    }

    /// How many alarms other than `excluding` already watch `metric` on the
    /// resource `monitored_resource_name` names.
    ///
    /// `excluding` is the name `PutAlarm` is writing, because re-putting an
    /// existing alarm overwrites that one record rather than adding another.
    pub fn alarms_on_metric(
        &self,
        monitored_resource_name: &str,
        metric_name: &str,
        excluding: &str,
    ) -> usize {
        self.alarms
            .values()
            .filter(|alarm| {
                alarm.name != excluding
                    && alarm.monitored_resource.name == monitored_resource_name
                    && alarm.metric.name == metric_name
            })
            .count()
    }

    /// The resource an alarm would monitor under `name`, or `None` when this
    /// account holds no alarmable resource of that name.
    ///
    /// See [`MonitoredResource`] for which resource types those are.
    pub fn monitored_resource(&self, name: &str) -> Option<MonitoredResource> {
        if let Some(instance) = self.instances.get(name) {
            return Some(MonitoredResource {
                name: instance.name.clone(),
                arn: instance.arn.clone(),
                resource_type: RESOURCE_TYPE_INSTANCE,
                location: StoredLocation::regional(&instance.location.region_name),
            });
        }
        let database = self.relational_databases.get(name)?;
        Some(MonitoredResource {
            name: database.name.clone(),
            arn: database.arn.clone(),
            resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
            location: StoredLocation::regional(&database.location.region_name),
        })
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

    /// Remove the bucket `name` names in `region`, answering it. `None` when
    /// nothing matched.
    ///
    /// Only the control-plane RECORD: the bucket's objects live in this emulator's
    /// one object store, which [`release_bucket_object_store`] is what releases
    /// (it takes that store's lock, so it is called with no guard of this store
    /// held).
    pub fn remove_bucket(&mut self, region: &str, name: &str) -> Option<StoredBucket> {
        self.buckets.remove(&(region.to_string(), name.to_string()))
    }

    /// The load balancer `name` names in `region`, for a reporting operation.
    pub fn load_balancer(&self, region: &str, name: &str) -> Option<&StoredLoadBalancer> {
        self.load_balancers
            .get(&(region.to_string(), name.to_string()))
    }

    /// Every load balancer in `region`, in the store's stable order.
    pub fn load_balancers_in(&self, region: &str) -> Vec<&StoredLoadBalancer> {
        self.load_balancers
            .iter()
            .filter(|((balancer_region, _), _)| balancer_region == region)
            .map(|(_, balancer)| balancer)
            .collect()
    }

    /// The load balancer `name` names in `region`, for an operation the spec says
    /// mutates it — registering an instance with it, releasing one, or removing
    /// an SSL/TLS certificate it holds.
    ///
    /// The `(region, name)` pair is this record's store key, so neither may be
    /// reassigned through this handle; no operation in this slice renames a load
    /// balancer or moves one between Regions.
    pub fn load_balancer_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut StoredLoadBalancer> {
        self.load_balancers
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Remove the load balancer `name` names in `region`, answering it. `None`
    /// when nothing matched.
    ///
    /// Everything the load balancer holds goes with the record it lives on: its
    /// SSL/TLS certificates — "Deletes a Lightsail load balancer and all its
    /// associated SSL/TLS certificates" ([the modelled `DeleteLoadBalancer`
    /// documentation]) — its configuration options and the registrations of the
    /// instances that were attached to it, so no read can observe a certificate
    /// or a registration of a load balancer this account no longer holds, and
    /// there is no second map to keep in step. An INSTANCE that was registered
    /// is untouched: a registration lives on the load balancer, and nothing
    /// about the instance itself changes when the load balancer it served goes
    /// away.
    ///
    /// [the modelled `DeleteLoadBalancer` documentation]: crate::ops::delete_load_balancer
    pub fn remove_load_balancer(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<StoredLoadBalancer> {
        self.load_balancers
            .remove(&(region.to_string(), name.to_string()))
    }

    /// Store a newly created load balancer.
    pub fn insert_load_balancer(&mut self, balancer: StoredLoadBalancer) {
        self.load_balancers.insert(
            (balancer.location.region_name.clone(), balancer.name.clone()),
            balancer,
        );
    }

    /// A token of `len` characters drawn from `alphabet`, for the purpose
    /// `label` names.
    ///
    /// Draws from the SAME `next_id` counter [`Self::mint_uuid`] does, on
    /// purpose: one counter means two values minted in this process are never
    /// built from the same input, and the hash is domain-separated by `label`
    /// so two tokens of the same counter value still differ. Every minted token
    /// of this store goes through here, so they cannot drift apart.
    fn mint_token(&mut self, label: &[u8], len: usize, alphabet: &[u8]) -> String {
        // One SHA-256 digest is 32 bytes, so a token asked for longer than that
        // would be silently cut short rather than drawn.
        debug_assert!(len <= 32, "a minted token draws at most 32 characters");
        if alphabet.is_empty() {
            return String::new();
        }
        self.next_id = self.next_id.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(self.next_id.to_be_bytes());
        hasher.update(process_seed().to_be_bytes());
        hasher.update(label);
        hasher
            .finalize()
            .iter()
            .take(len)
            .map(|byte| char::from(alphabet[usize::from(*byte) % alphabet.len()]))
            .collect()
    }

    /// A token for one container service's default domain: see
    /// [`DOMAIN_TOKEN_LEN`].
    pub fn mint_domain_token(&mut self) -> String {
        self.mint_token(
            b"container-service-domain",
            DOMAIN_TOKEN_LEN,
            DOMAIN_TOKEN_ALPHABET,
        )
    }

    /// The DNS label of one managed relational database's master endpoint: see
    /// [`RELATIONAL_DATABASE_HOST_LABEL_LEN`].
    pub fn mint_endpoint_host_label(&mut self) -> String {
        self.mint_token(
            b"relational-database-endpoint",
            RELATIONAL_DATABASE_HOST_LABEL_LEN,
            RELATIONAL_DATABASE_HOST_LABEL_ALPHABET,
        )
    }

    /// A strong master user password for one managed relational database: the
    /// password `UpdateRelationalDatabase$rotateMasterUserPassword` asks the
    /// service to generate, which
    /// `GetRelationalDatabaseMasterUserPassword` is then what answers with.
    pub fn mint_master_user_password(&mut self) -> String {
        self.mint_token(
            b"relational-database-master-user-password",
            MINTED_MASTER_USER_PASSWORD_LEN,
            MINTED_MASTER_USER_PASSWORD_ALPHABET,
        )
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

    /// The next public IPv4 address to reserve for a static IP, consumed so no
    /// two static IPs — and no static IP and instance — in this process share an
    /// address.
    ///
    /// Drawn from the SAME counter and the SAME public range
    /// [`Self::fresh_addresses`] gives an instance its public address from: one
    /// counter means one address is handed out once, whichever resource asked for
    /// it.
    pub fn fresh_static_ip_address(&mut self) -> String {
        let offset = self.fresh_address_offset();
        ipv4(PUBLIC_IPV4_BASE, offset)
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

    /// The peering of the Lightsail VPC in `region`, for a reporting operation.
    pub fn vpc_peering(&self, region: &str) -> Option<&StoredVpcPeering> {
        self.vpc_peerings.get(region)
    }

    /// The peering of the Lightsail VPC in `region`, for an operation the spec
    /// says mutates it — the teardown `UnpeerVpc` starts.
    ///
    /// `region` is this record's store key, so it must not be reassigned through
    /// this handle: a peering belongs to the Region whose VPC it peers.
    pub fn vpc_peering_mut(&mut self, region: &str) -> Option<&mut StoredVpcPeering> {
        self.vpc_peerings.get_mut(region)
    }

    /// Store the peering `PeerVpc` just started for the Lightsail VPC in
    /// `region`, replacing the spent record of a peering that has since been
    /// torn down.
    pub fn insert_vpc_peering(&mut self, region: &str, peering: StoredVpcPeering) {
        self.vpc_peerings.insert(region.to_string(), peering);
    }

    /// Whether the Lightsail VPC in `region` is peered with the account's
    /// default VPC — what `IsVpcPeered` answers.
    ///
    /// A Region nothing has peered reports `false`, which is also what a Region
    /// whose peering has been torn down reports.
    pub fn is_vpc_peered(&self, region: &str) -> bool {
        self.vpc_peering(region)
            .is_some_and(StoredVpcPeering::is_peered)
    }

    /// The log entry `id` names, for a reporting operation.
    ///
    /// The log is append-only and bounded, so this is a linear scan of at most
    /// [`MAX_OPERATION_HISTORY`] entries.
    pub fn operation(&self, id: &str) -> Option<&StoredOperation> {
        self.operations.iter().find(|entry| entry.id == id)
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

    /// Append `setup` to the `SetupInstanceHttps` history, dropping the oldest
    /// records past [`MAX_INSTANCE_HTTPS_SETUPS`] so the history stays bounded
    /// however many calls a caller makes.
    ///
    /// The bound is over the WHOLE history, as the operation log's own is: the
    /// log is one append-only sequence, so a store driven past the bound drops
    /// its oldest records whichever instance they name.
    pub fn record_instance_https_setup(&mut self, setup: StoredInstanceHttpsSetup) {
        self.instance_https_setups.push(setup);
        if let Some(excess) = self
            .instance_https_setups
            .len()
            .checked_sub(MAX_INSTANCE_HTTPS_SETUPS)
        {
            self.instance_https_setups.drain(..excess);
        }
    }

    /// One page of the HTTPS setup history of the resource `resource_name`, and
    /// the token naming the first record of the page after it — present only
    /// while records remain.
    ///
    /// The most recent record comes first, and a page carries at most
    /// [`SETUP_HISTORY_PAGE_SIZE`] of them. The walk itself is [`catalog_page`],
    /// shared with this service's other paged listings; only the token a record
    /// is named by is this history's own — it is DERIVED from the record's
    /// operation id (see [`StoredInstanceHttpsSetup::page_token`]) rather than a
    /// member it holds, which is what `catalog_page`'s `Cow` answer is for. So a
    /// token naming no record of this resource's history answers `None`, which
    /// the caller turns into a refusal, and an absent token asks for the first
    /// page.
    pub fn setup_history_page(
        &self,
        resource_name: &str,
        page_token: Option<&str>,
    ) -> Option<(Vec<crate::ops::types::SetupHistory>, Option<String>)> {
        let reported = self
            .instance_https_setups
            .iter()
            .rev()
            .filter(|setup| setup.instance_name == resource_name)
            .collect::<Vec<_>>();
        let (page, next_page_token) = catalog_page(
            &reported,
            SETUP_HISTORY_PAGE_SIZE,
            page_token,
            // Owned, because the token is the operation id's hex digits rather
            // than a member the record holds. The walk is over at most
            // [`MAX_INSTANCE_HTTPS_SETUPS`] records, so the comparison is
            // bounded.
            |setup| Some(std::borrow::Cow::Owned(setup.page_token())),
        )?;
        Some((
            page.iter().map(|setup| setup.view()).collect::<Vec<_>>(),
            next_page_token,
        ))
    }

    /// Materialise every transition the clock had already made due at `now_ms`:
    /// an instance, a disk, a disk snapshot, an instance snapshot, an export
    /// snapshot record, a managed
    /// relational database, one of its snapshots, or a load balancer
    /// settles into the state it was owed, a container service walks
    /// into the phase it was owed (activating the deployment that was waiting),
    /// a Region's Lightsail VPC peering walks into the phase it was owed
    /// (established, or gone), a CDN distribution settles into the status it was
    /// owed, and an operation whose work is done settles into `Succeeded`.
    ///
    /// This is the one write a reporting operation performs. Each transition was
    /// owed at this instant whether or not anybody called, so applying it here
    /// cannot show a caller anything a later caller would not also see.
    ///
    /// Every walk is bounded by [`MAX_INSTANCES`], [`MAX_DISKS`],
    /// [`MAX_DISK_SNAPSHOTS`], [`MAX_INSTANCE_SNAPSHOTS`],
    /// [`MAX_EXPORT_SNAPSHOT_RECORDS`],
    /// [`MAX_CONTAINER_SERVICES`],
    /// [`MAX_RELATIONAL_DATABASES`],
    /// [`MAX_RELATIONAL_DATABASE_SNAPSHOTS`], [`MAX_LOAD_BALANCERS_PER_REGION`],
    /// [`MAX_DISTRIBUTIONS`] and
    /// [`MAX_OPERATION_HISTORY`] — and, for the peering walk, by the modelled
    /// `RegionName` enum [`vpc_peerings`](Self::vpc_peerings) is keyed by — and a
    /// record drops out of the work as soon as it has nothing outstanding.
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
                // A disk that has settled into `available` is attached to
                // nothing — that is what the modelled `DiskState` value means
                // next to `in-use` — so settling there is also what completes
                // the detachment `DetachDisk` started: the attachment and the
                // device path it was mounted at are released together. A disk
                // that reached `available` from `pending` was attached to
                // nothing already, so the same step is a no-op for it. An
                // operation that ATTACHES a disk therefore has to put it in
                // `in-use` and clear the `due` it carries, or the settle it was
                // owed would release the attachment that operation just made.
                if disk.state == DISK_STATE_AVAILABLE {
                    disk.attached_to = None;
                    disk.path = None;
                }
            }
        }
        for snapshot in self.disk_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
            }
        }
        for snapshot in self.instance_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
            }
        }
        for record in self.export_snapshot_records.values_mut() {
            let Some(due) = record.due else { continue };
            if due.at_ms <= now_ms {
                record.state = due.state;
                record.due = None;
            }
        }
        for service in self.container_services.values_mut() {
            service.advance(now_ms);
        }
        for database in self.relational_databases.values_mut() {
            // A modification left for the database's maintenance window is
            // applied when that window is due, whatever state the database is
            // in — the window comes round on the clock, not on a state change.
            database.apply_due_modification(now_ms);
            let Some(due) = database.due else { continue };
            if due.at_ms <= now_ms {
                // The states whose engine is DOWN and coming back up: one being
                // restarted, and one being started again after a stop.
                let engine_coming_up = database.state == RELATIONAL_DATABASE_STATE_REBOOTING
                    || database.state == RELATIONAL_DATABASE_STATE_STARTING;
                database.state = due.state;
                database.due = None;
                // A database that has come back up has an engine that read its
                // whole parameter group as it started, so this is the instant a
                // value applied `pending-reboot` is loaded — the one thing a
                // reboot changes about the record.
                if engine_coming_up && database.state == RELATIONAL_DATABASE_STATE_AVAILABLE {
                    database.load_pending_parameters();
                }
                // A database that has finished shutting down is owed one more
                // transition, the service's own: "If you don't manually start
                // your database instance after it has been stopped for seven
                // consecutive days, Amazon Lightsail automatically starts it for
                // you" (the modelled `StopRelationalDatabase` documentation).
                // It takes the same two steps a caller's `StartRelationalDatabase`
                // takes — `starting`, then serving — and
                // `StartRelationalDatabase` replaces this `due` when a caller
                // starts the database first.
                if database.state == RELATIONAL_DATABASE_STATE_STOPPED {
                    database.due = Some(DueNamedState {
                        at_ms: due
                            .at_ms
                            .saturating_add(RELATIONAL_DATABASE_AUTO_START_AFTER_MILLIS),
                        state: RELATIONAL_DATABASE_STATE_STARTING,
                    });
                } else if database.state == RELATIONAL_DATABASE_STATE_STARTING {
                    // The automatic start's second step: a start a CALLER made
                    // owes `available` from the handler, so the only database
                    // that settles INTO `starting` is one the clock started.
                    database.due = Some(DueNamedState::after(
                        due.at_ms,
                        RELATIONAL_DATABASE_STATE_AVAILABLE,
                    ));
                }
            }
        }
        for snapshot in self.relational_database_snapshots.values_mut() {
            let Some(due) = snapshot.due else { continue };
            if due.at_ms <= now_ms {
                snapshot.state = due.state;
                snapshot.due = None;
            }
        }
        for balancer in self.load_balancers.values_mut() {
            let Some(due) = balancer.due else { continue };
            if due.at_ms <= now_ms {
                balancer.state = due.state;
                balancer.due = None;
            }
        }
        for peering in self.vpc_peerings.values_mut() {
            let Some(due) = peering.due else { continue };
            if due.at_ms <= now_ms {
                peering.phase = due.phase;
                peering.due = None;
            }
        }
        for distribution in self.distributions.values_mut() {
            let Some(due) = distribution.due else { continue };
            if due.at_ms <= now_ms {
                distribution.status = due.state;
                distribution.due = None;
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
        for setup in &mut self.instance_https_setups {
            let Some(settles_at_ms) = setup.settles_at_ms else {
                continue;
            };
            if settles_at_ms <= now_ms {
                setup.status = SETUP_STATUS_SUCCEEDED.to_string();
                setup.settles_at_ms = None;
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

/// Order two modelled timestamps. A `DateTime` carries seconds plus a
/// sub-second part, so both are compared — two instants one nanosecond apart
/// are not the same instant.
pub fn timestamp_key(value: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (value.secs(), value.subsec_nanos())
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

/// The ARN of the instance snapshot `id` in `region`.
pub fn instance_snapshot_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{INSTANCE_SNAPSHOT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the managed relational database snapshot `id` in `region`.
pub fn relational_database_snapshot_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RELATIONAL_DATABASE_SNAPSHOT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the alarm `id` in `region`.
pub fn alarm_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ALARM_ARN_RESOURCE_PREFIX}{id}"),
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

/// The ARN of the static IP `id` in `region`.
pub fn static_ip_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{STATIC_IP_ARN_RESOURCE_PREFIX}{id}"),
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
    signed_region(parts)
        .filter(|region| REGION_NAME_VALUES.contains(&region.as_str()))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region this request's SigV4 credential scope names, verbatim, and `None`
/// when it carries no scope at all.
///
/// The one place this crate reads the Region off the request: what the scope says
/// and what the service does with it are two different questions, and the callers
/// below answer them differently.
fn signed_region(parts: &::http::request::Parts) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region)
}

/// The refusal for a domain or distribution call this request cannot be served
/// by, and `None` when it can.
///
/// Those APIs are served by the N. Virginia endpoint ALONE (see
/// [`DOMAIN_REGION_ONLY_MESSAGE`]), so what decides is the Region the request's
/// SigV4 credential scope names — whatever it names, not only the Regions this
/// service holds resources in, since a scope naming a Region Lightsail does not
/// serve is no more us-east-1 than any other. A request carrying no scope at all
/// is served in [`DEFAULT_REGION`], which IS the domain Region, so it is served
/// rather than refused.
///
/// The same documented restriction covers the distribution (and distribution
/// certificate) operations: `CreateDistribution` and `GetDistributions` call
/// THIS helper, and so must any other one of them when it is implemented,
/// rather than re-deriving the rule — so the restriction stays in one place.
pub fn domain_region_refusal(parts: &::http::request::Parts) -> Option<String> {
    let signed = signed_region(parts)?;
    (signed != DOMAIN_REGION_NAME).then(|| DOMAIN_REGION_ONLY_MESSAGE.to_string())
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

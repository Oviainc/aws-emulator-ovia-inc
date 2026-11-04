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
//! behind), the **domain** (a DNS zone the account holds recordsets in), the
//! **container service** (a compute resource containers are
//! deployed to), the **bucket** (an object-storage resource of the Lightsail
//! object storage service) together with the **access keys** it grants
//! programmatic access through — whose OBJECTS live in the one object store this
//! emulator holds, Amazon S3's own, which this module provisions a bucket in
//! ([`claim_bucket_object_store`]) rather than keeping a second object map here —
//! the **managed relational database** (a MySQL or PostgreSQL database the service
//! runs for the account) together with the **database snapshot** (a point-in-time
//! copy of one) and the **event log** each database carries of what the service
//! has done to it, the **load balancer** (a regional resource that spreads
//! HTTP traffic across the instances attached to it), the **static IP** (a public
//! IPv4 address the account reserves and attaches to an instance), the **SSH key
//! pair** (the public half of a key pair the account holds, which an instance's
//! `sshKeyName` names), the **CDN
//! distribution** (a global resource that caches and serves the content of an
//! origin resource) together with the **certificate** (an SSL/TLS certificate
//! the account requests for a domain, which is attached to a distribution so it
//! accepts HTTPS traffic for that certificate's domains), the peering of a
//! Region's **Lightsail VPC** with the account's default VPC there, the
//! temporary **container image registry log in** credentials
//! `CreateContainerServiceRegistryLogin` hands out for the account's container
//! image registry, the **contact method** (the email address or mobile phone
//! number the account's notifications are sent to, one per protocol per
//! Region), the append-only **operation log** every mutating
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

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
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
    AttachedDisk, AutoSnapshotDetails,
    AvailabilityZone, Blueprint, Bucket, BucketAccessLogConfig, BucketBundle, BucketCorsConfig,
    BucketCorsRule, BucketState, Bundle, CacheBehavior, CacheBehaviorPerPath, CacheSettings,
    Certificate, CertificateSummary, Container,
    ContainerImage,
    ContainerService, ContainerServiceDeployment, ContainerServiceDeploymentRequest,
    ContainerServiceECRImagePullerRole, ContainerServiceEndpoint,
    ContactMethod,
    ContainerServiceHealthCheckConfig, ContainerServicePower, ContainerServiceStateDetail,
    DestinationInfo,
    DistributionBundle, DistributionCustomErrorResponse,
    Domain, DomainEntry, DomainValidationRecord, ExportSnapshotRecord,
    ExportSnapshotRecordSourceInfo,
    HostKeyAttributes, Instance,
    InstanceHardware, InstanceHealthSummary, InstanceMetadataOptions, InstanceNetworking,
    InstancePortInfo, InstanceState, KeyPair, LightsailDistribution, LoadBalancer,
    LoadBalancerTlsCertificate, LoadBalancerTlsCertificateDomainValidationOption,
    LoadBalancerTlsCertificateDomainValidationRecord, LoadBalancerTlsCertificateRenewalSummary,
    LoadBalancerTlsCertificateSummary,
    MonitoredResourceInfo, MonthlyTransfer, Operation, Origin, PartnerInfo, PrivateRegistryAccess,
    Region,
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

/// The `resourceType` an SSH key pair and its operations report, from the
/// modelled `KeyPair$resourceType` documentation ("The resource type (usually
/// `KeyPair`)") and the same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_KEY_PAIR: &str = "KeyPair";

/// The resource portion a key pair ARN carries before its id, from the modelled
/// `KeyPair$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:KeyPair/05859e3d-…`).
pub const KEY_PAIR_ARN_RESOURCE_PREFIX: &str = "KeyPair/";

/// The `resourceType` an alarm and its operations report, from the same modelled
/// `ResourceType` enum — the value the modelled `Alarm$resourceType` member
/// carries ("The Lightsail resource type of the alarm").
pub const RESOURCE_TYPE_ALARM: &str = "Alarm";

/// The `resourceType` a contact method and its operations report, from the same
/// modelled `ResourceType` enum — the value the modelled
/// `ContactMethod$resourceType` member carries ("The Lightsail resource type of
/// the contact method").
pub const RESOURCE_TYPE_CONTACT_METHOD: &str = "ContactMethod";

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

/// The resource portion a contact method ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const CONTACT_METHOD_ARN_RESOURCE_PREFIX: &str = "ContactMethod/";

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

impl Lifecycle {
    /// Whether an instance in this state is one of the two an operation
    /// documented for "a running or stopped instance" acts on.
    ///
    /// The crate's ONE owner of that pair, asked by every operation whose own
    /// documented scope is those two states, so a settled instance and one the
    /// clock is still moving ([`PENDING`], [`STOPPING`]) are told apart in a
    /// single place.
    pub fn is_running_or_stopped(self) -> bool {
        self == RUNNING || self == STOPPED
    }
}

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

/// How long after the `AutoSnapshot` add-on is enabled on a resource this store
/// takes the FIRST automatic snapshot of it.
///
/// CHOSEN, not an AWS duration: the add-on takes a copy once a day at the hour
/// it carries, so against AWS the first copy lands up to a day after the call
/// that enabled the add-on — a wait no caller can be made to serve here, and one
/// that would leave the copies the add-on exists to take unobservable. The span
/// is deliberately NON-ZERO, so a caller that reads straight after enabling the
/// add-on still sees a resource no copy has been taken of yet. Every later copy
/// follows the add-on's own daily hour ([`next_auto_snapshot_due_ms`]).
pub const AUTO_SNAPSHOT_FIRST_MILLIS: i64 = STATE_TRANSITION_MILLIS;

/// A state change the clock already owes an instance.
#[derive(Debug, Clone, Copy)]
pub struct DueTransition {
    /// Epoch milliseconds at which this change STARTED being owed: the instant
    /// of the call that scheduled it. It is what a progress report measures
    /// from, so the progress follows the transition that was actually scheduled
    /// rather than assuming every one spans the same time.
    pub started_at_ms: i64,
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    /// The state the instance settles into.
    pub state: Lifecycle,
}

impl DueTransition {
    /// The transition `state`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, state: Lifecycle) -> Self {
        Self {
            started_at_ms: now_ms,
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            state,
        }
    }

    /// How far through this transition the instant `now_ms` is, as a whole
    /// percentage — what an operation reporting the progress of the work that
    /// scheduled it answers with.
    pub fn percentage_complete_at(&self, now_ms: i64) -> i32 {
        elapsed_percentage(self.started_at_ms, self.at_ms, now_ms)
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

/// The verbs the disk calls name themselves by in a refusal their disk's state
/// blocks, one per operation that has such a guard, so the sentence
/// [`disk_blocked_by_state_message`] builds carries the verb of the call that
/// was refused.
pub const DISK_VERB_ATTACH: &str = "attach";
/// See [`DISK_VERB_ATTACH`].
pub const DISK_VERB_DETACH: &str = "detach";
/// See [`DISK_VERB_ATTACH`].
pub const DISK_VERB_DELETE: &str = "delete";

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

/// `AutoSnapshotStatus` values this slice uses, from the modelled
/// `AutoSnapshotStatus` enum and the `AutoSnapshotDetails$status` documentation
/// ("The status of the automatic snapshot"): a copy the service is still taking
/// reports `InProgress`, and one whose data is all captured reports `Success`.
/// The enum's `Failed` member describes a copy that failed, which this store
/// cannot produce — nothing here fails a copy — and its `NotFound` member a copy
/// the service has no record of, which is not reported at all.
pub const AUTO_SNAPSHOT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AUTO_SNAPSHOT_STATUS_IN_PROGRESS`].
pub const AUTO_SNAPSHOT_STATUS_SUCCESS: &str = "Success";

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

/// `RelationalDatabaseEvent$eventCategories` values this slice uses — "The
/// category that the database event belongs to", a plain string in the model with
/// no enum behind it.
///
/// A Lightsail managed database is an Amazon RDS DB instance, so what happens to
/// it is an RDS DB instance event and carries RDS's own event categories (Amazon
/// RDS User Guide, "Amazon RDS event categories and event messages"):
/// `creation` while the database is being provisioned, `availability` for its
/// engine going down and coming back up, `backup` for a snapshot of it being
/// taken, and `configuration change` for a modification reaching it.
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION: &str = "creation";
/// See [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`].
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY: &str = "availability";
/// See [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`].
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP: &str = "backup";
/// See [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`].
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE: &str = "configuration change";

/// The event a database logs once it has finished being provisioned and accepts
/// connections.
///
/// CHOSEN wording: the model publishes no event text, only that
/// `RelationalDatabaseEvent$message` is "the message of the database event". What
/// the service DOES guarantee, and what this slice keeps, is that one entry is
/// logged per change the service makes to the database, at the instant it
/// happened, in the categories that change falls in.
pub const RELATIONAL_DATABASE_EVENT_CREATED: &str = "Database created";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when the engine is asked to
/// go down — a restart or a stop.
pub const RELATIONAL_DATABASE_EVENT_SHUTDOWN: &str = "Database shutdown";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a restart has brought
/// the engine back up.
pub const RELATIONAL_DATABASE_EVENT_RESTARTED: &str = "Database restarted";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when the engine has finished
/// shutting down.
pub const RELATIONAL_DATABASE_EVENT_STOPPED: &str = "Database stopped";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a stopped database's
/// engine is serving again.
pub const RELATIONAL_DATABASE_EVENT_STARTED: &str = "Database started";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a snapshot of the
/// database starts being taken.
pub const RELATIONAL_DATABASE_EVENT_BACKUP_STARTED: &str = "Backing up database";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when that snapshot can be
/// restored from.
pub const RELATIONAL_DATABASE_EVENT_BACKUP_FINISHED: &str = "Finished database backup";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a value reaches the
/// database's parameter group.
pub const RELATIONAL_DATABASE_EVENT_PARAMETER_GROUP_CHANGED: &str =
    "Applied change to database parameter group";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a modification reaches
/// the database, followed by the members it changed.
pub const RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED: &str = "Applied database modification";

/// The window `GetRelationalDatabaseEvents` reports when the request names none.
///
/// An AWS-published value: "The number of minutes in the past from which to
/// retrieve events. … Default: 60" (the modelled `durationInMinutes`
/// documentation).
pub const RELATIONAL_DATABASE_EVENT_DEFAULT_DURATION_MINUTES: i32 = 60;

/// The shortest window `GetRelationalDatabaseEvents` accepts: "The minimum is 1"
/// (the modelled `durationInMinutes` documentation).
pub const RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES: i32 = 1;

/// The longest window `GetRelationalDatabaseEvents` accepts: "the maximum is 14
/// days (20160 minutes)" (the modelled `durationInMinutes` documentation).
pub const RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES: i32 = 20_160;

/// How long an entry stays in a database's event log.
///
/// The longest window a caller may ask for, so an entry older than this can
/// never be reported again and is dropped rather than kept for ever.
pub const RELATIONAL_DATABASE_EVENT_RETENTION_MILLIS: i64 =
    RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES as i64 * 60_000;

/// The most events one database's log keeps, oldest dropped first.
///
/// Emulator resource guard, like [`MAX_OPERATION_HISTORY`]: the log is
/// append-only, so without a cap a long-lived emulator would grow without bound.
pub const MAX_RELATIONAL_DATABASE_EVENTS: usize = 500;

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
pub const OPERATION_TYPE_DELETE_KNOWN_HOST_KEYS: &str = "DeleteKnownHostKeys";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DOMAIN: &str = "CreateDomain";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET: &str = "CreateBucket";

/// The `Operation$operationType` a `DeleteBucket` call logs, from the modelled
/// `OperationType` enum.
pub const OPERATION_TYPE_DELETE_BUCKET: &str = "DeleteBucket";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_BUCKET: &str = "UpdateBucket";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_BUCKET_BUNDLE: &str = "UpdateBucketBundle";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK: &str = "CreateDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_DISK: &str = "AttachDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_DISK: &str = "DetachDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_DISK: &str = "DeleteDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_STATIC_IP: &str = "AttachStaticIp";
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
pub const OPERATION_TYPE_DELETE_RELATIONAL_DATABASE_SNAPSHOT: &str =
    "DeleteRelationalDatabaseSnapshot";
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
pub const OPERATION_TYPE_UPDATE_LOAD_BALANCER_ATTRIBUTE: &str = "UpdateLoadBalancerAttribute";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "CreateLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "DeleteLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "AttachLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_SNAPSHOT: &str = "CreateDiskSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_DISK_SNAPSHOT: &str = "DeleteDiskSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_INSTANCE_SNAPSHOT: &str = "CreateInstanceSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_INSTANCE_SNAPSHOT: &str = "DeleteInstanceSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_FROM_SNAPSHOT: &str = "CreateDiskFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_INSTANCES_FROM_SNAPSHOT: &str = "CreateInstancesFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET_ACCESS_KEY: &str = "CreateBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_BUCKET_ACCESS_KEY: &str = "DeleteBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISTRIBUTION: &str = "CreateDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_DISTRIBUTION: &str = "DeleteDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ALLOCATE_STATIC_IP: &str = "AllocateStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_RELEASE_STATIC_IP: &str = "ReleaseStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_PUT_ALARM: &str = "PutAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_ALARM: &str = "DeleteAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_TEST_ALARM: &str = "TestAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_CERTIFICATE: &str = "CreateCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_CERTIFICATE: &str = "DeleteCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_CERTIFICATE_TO_DISTRIBUTION: &str =
    "AttachCertificateToDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_CERTIFICATE_FROM_DISTRIBUTION: &str =
    "DetachCertificateFromDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_RESET_DISTRIBUTION_CACHE: &str = "ResetDistributionCache";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_DISTRIBUTION: &str = "UpdateDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_DISTRIBUTION_BUNDLE: &str = "UpdateDistributionBundle";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_CONTACT_METHOD: &str = "CreateContactMethod";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_CONTACT_METHOD: &str = "DeleteContactMethod";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_SEND_CONTACT_METHOD_VERIFICATION: &str = "SendContactMethodVerification";
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
/// The `operationType` the log entry of a `CopySnapshot` call carries — and the
/// ONE statement of the rule every call the modelled `OperationType` enum
/// declares no member for follows.
///
/// `Operation$operationType` targets that enum, whose own members this module
/// reports wherever one of them states what the call did (see
/// [`OPERATION_TYPE_UPDATE_DOMAIN_ENTRY`]). The enum LAGS this API: it declares
/// no member for copying a snapshot, for a change to a resource's tags, for the
/// deletion of one automatic snapshot, for a key pair's deletion, for either VPC
/// peering call, for the Amazon EC2 export family or for the stack an export is
/// turned into — and no member it does carry states what one of those calls did,
/// so reporting a copy or an export as a snapshot CREATE would misreport it. The
/// member is a string on the wire and a Smithy enum is open there (a client has
/// to tolerate a value it does not know), so EVERY entry this log holds names the
/// call that wrote it, whether or not the enum declares that name: one rule for
/// the whole log `GetOperations` and `GetOperationsForResource` report, rather
/// than entries that say nothing about what ran.
pub const OPERATION_TYPE_COPY_SNAPSHOT: &str = "CopySnapshot";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`], the one rule for the `operationType` of
/// a call the modelled `OperationType` enum declares no member for.
pub const OPERATION_TYPE_TAG_RESOURCE: &str = "TagResource";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_UNTAG_RESOURCE: &str = "UntagResource";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_DELETE_AUTO_SNAPSHOT: &str = "DeleteAutoSnapshot";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_EXPORT_SNAPSHOT: &str = "ExportSnapshot";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_DELETE_KEY_PAIR: &str = "DeleteKeyPair";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_PEER_VPC: &str = "PeerVpc";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_UNPEER_VPC: &str = "UnpeerVpc";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DISABLE_ADD_ON: &str = "DisableAddOn";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ENABLE_ADD_ON: &str = "EnableAddOn";
/// The `operationType` the log entry of a `SetIpAddressType` call carries, the
/// modelled `OperationType` enum's own `SetIpAddressType` member.
pub const OPERATION_TYPE_SET_IP_ADDRESS_TYPE: &str = "SetIpAddressType";
/// The `operationType` the log entry of a `SetResourceAccessForBucket` call
/// carries, the modelled `OperationType` enum's own `SetResourceAccessForBucket`
/// member.
pub const OPERATION_TYPE_SET_RESOURCE_ACCESS_FOR_BUCKET: &str = "SetResourceAccessForBucket";
/// The `operationType` the log entry of an `UpdateInstanceMetadataOptions` call
/// carries, the modelled `OperationType` enum's own
/// `UpdateInstanceMetadataOptions` member.
pub const OPERATION_TYPE_UPDATE_INSTANCE_METADATA_OPTIONS: &str = "UpdateInstanceMetadataOptions";
/// The `operationType` the log entry of a `StartGUISession` call carries, the
/// modelled `OperationType` enum's own `StartGUISession` member.
pub const OPERATION_TYPE_START_GUI_SESSION: &str = "StartGUISession";
/// The `operationType` the log entry of a `StopGUISession` call carries, the
/// modelled `OperationType` enum's own `StopGUISession` member.
pub const OPERATION_TYPE_STOP_GUI_SESSION: &str = "StopGUISession";

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

/// The most automatic snapshots this emulator holds of ONE resource.
///
/// Emulator resource guard, not a refusal: nobody asks for an automatic
/// snapshot, so there is no request to refuse — the `AutoSnapshot` add-on takes
/// one a day for as long as it is enabled, and the oldest copy drops out once a
/// new one would take the resource past this. It is also what bounds the walk
/// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] and
/// `GetAutoSnapshots` make over one resource's copies.
pub const MAX_AUTO_SNAPSHOTS_PER_RESOURCE: usize = 7;

/// The most resources this emulator holds KEPT automatic snapshots of — copies
/// a delete left in the account because it was not asked to delete the
/// resource's add-ons, so the resource they were taken of is gone and the copies
/// are not.
///
/// Emulator resource guard, not a refusal: nobody asks for an automatic
/// snapshot, so there is no request to refuse — the copies kept longest ago drop
/// out once a further delete would take the store past this, exactly as the
/// oldest entry drops out of the operation log ([`MAX_OPERATION_HISTORY`]). It
/// is also what keeps the walk
/// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] makes over the
/// schedules bounded once a resource no longer has to be held for its copies to
/// be.
pub const MAX_KEPT_AUTO_SNAPSHOT_SOURCES: usize = 500;

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

/// How many database events one `GetRelationalDatabaseEvents` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more events remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every paged listing in this slice pages the same
/// way.
pub const RELATIONAL_DATABASE_EVENT_PAGE_SIZE: usize = 10;

/// How many log lines one `GetRelationalDatabaseLogEvents` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that "if
/// your results are paginated, the response will return a next forward token
/// and/or next backward token that you can specify as the page token in a
/// subsequent request" (the modelled `pageToken` documentation). Matches
/// [`INSTANCE_PAGE_SIZE`], so every paged read in this slice pages the same way.
pub const RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE: usize = 10;

/// How many instance snapshots one `GetInstanceSnapshots` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const INSTANCE_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many block storage disks one `GetDisks` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more disks remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const DISK_PAGE_SIZE: usize = 10;

/// How many disk snapshots one `GetDiskSnapshots` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more snapshots remain. Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const DISK_SNAPSHOT_PAGE_SIZE: usize = 10;

/// How many static IPs one `GetStaticIps` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more static IPs remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const STATIC_IP_PAGE_SIZE: usize = 10;

/// How many SSH key pairs one `GetKeyPairs` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more key pairs remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const KEY_PAIR_PAGE_SIZE: usize = 10;

/// How many export snapshot records one `GetExportSnapshotRecords` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation either, only that
/// the response carries a `nextPageToken` when more records remain — "A next page
/// token is not returned if there are no more results to display."
pub const EXPORT_SNAPSHOT_RECORD_PAGE_SIZE: usize = 10;

/// How many domain recordsets one `GetDomains` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more recordsets remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every stored-resource listing in this slice pages
/// the same way.
pub const DOMAIN_PAGE_SIZE: usize = 10;

/// How many resource names one `GetActiveNames` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more names remain — "A next page
/// token is not returned if there are no more results to display." Matches
/// [`INSTANCE_PAGE_SIZE`], so every paged listing in this slice pages the same
/// way.
pub const ACTIVE_NAME_PAGE_SIZE: usize = 10;

/// How many log entries one `GetOperationsForResource` page carries.
///
/// CHOSEN: Lightsail publishes a page size for `GetOperations` only ("Results
/// are returned from oldest to newest, up to a maximum of 200") and none for
/// `GetOperationsForResource`, which documents only that "a next page token is
/// not returned if there are no more results to display". Matches
/// [`INSTANCE_PAGE_SIZE`], so every listing in this slice pages the same way.
pub const OPERATIONS_FOR_RESOURCE_PAGE_SIZE: usize = 10;

/// How many log entries one `GetOperations` page carries.
///
/// PUBLISHED, from that operation's own documentation: "Returns information
/// about all operations. Results are returned from oldest to newest, up to a
/// maximum of 200." It is therefore NOT the chosen size the rest of this
/// slice's listings share.
pub const OPERATIONS_PAGE_SIZE: usize = 200;

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
// Resource paging
// ---------------------------------------------------------------------------

/// The records of a name-keyed map from the one `page_token` names onward, in the
/// map's own total order — where a page of a stored-resource listing starts.
///
/// A page token NAMES a record rather than carrying an index, so a record created
/// or deleted between two pages cannot make the walk skip or repeat another one.
/// No token asks for the first page; whether the token names a record of the walk
/// is [`resource_page`]'s to answer, which is why this starts the walk at the first
/// name at or after it either way.
///
/// TOTAL on every input, including an arbitrary caller-supplied token: the token is
/// only ever COMPARED against the keys, so there is no indexing, no slicing, no
/// unwrap and no arithmetic that can overflow or panic.
pub fn records_from<'a, R>(
    records: &'a BTreeMap<String, R>,
    page_token: Option<&str>,
) -> impl Iterator<Item = (&'a str, &'a R)> {
    records
        .range(page_token.unwrap_or_default().to_string()..)
        .map(|(name, record)| (name.as_str(), record))
}

/// The records of a `(region, name)`-keyed map that belong to `region`, from the
/// one `page_token` names onward, in name order.
///
/// The same start [`records_from`] computes over a map keyed by name alone, for a
/// resource whose identity includes the Region it was created in: the key pair
/// orders this Region's records together, so the walk leaves the Region behind at
/// the first key of the next one and never reports another Region's record.
pub fn regional_records_from<'a, R>(
    records: &'a BTreeMap<(String, String), R>,
    region: &'a str,
    page_token: Option<&str>,
) -> impl Iterator<Item = (&'a str, &'a R)> {
    records
        .range((region.to_string(), page_token.unwrap_or_default().to_string())..)
        .take_while(move |((record_region, _), _)| record_region.as_str() == region)
        .map(|((_, name), record)| (name.as_str(), record))
}

/// The records of a name-keyed map that are LOCATED in `region` — `region_of`
/// reads the Region off each record — from the one `page_token` names onward, in
/// name order.
///
/// What a listing the model scopes to "your AWS account and region" reports: the
/// resource is keyed by name alone, because its name is unique across the
/// account, but it was created in one Region and keeps that Region on its stored
/// location, so the Region the request is signed for is what decides whether this
/// listing reports it. The same narrowing [`regional_records_from`] gets from the
/// key pair of a resource whose identity includes its Region, expressed as the
/// filter [`resource_page`] documents — so the page size counts the records
/// reported, the next page starts at the first record of this Region after them,
/// and a token naming a record of ANOTHER Region names no page of this listing.
///
/// TOTAL on every input, including an arbitrary caller-supplied token: the token
/// and the Region are only ever COMPARED against the records, so there is no
/// indexing, no slicing, no unwrap and no arithmetic that can overflow or panic.
pub fn records_in_region_from<'a, R>(
    records: &'a BTreeMap<String, R>,
    region: &'a str,
    page_token: Option<&str>,
    region_of: impl Fn(&R) -> &str + 'a,
) -> impl Iterator<Item = (&'a str, &'a R)> {
    records_from(records, page_token).filter(move |(_, record)| region_of(record) == region)
}

/// One page of a resource listing: at most `page_size` of `records`, each as
/// `view_of` reports it, plus the name of the first record AFTER them — named only
/// while records remain.
///
/// `None` is the answer when `page_token` names no record this listing reports:
/// there is no page to report, which the caller turns into a refusal. A token is
/// only ever answered by the record it NAMES — the first record of the page it
/// opens — so a token the listing never handed out is refused rather than resumed
/// at the next record after it, which would answer a well-formed page with every
/// earlier record silently missing. An absent token asks for the first page.
///
/// Shared by every stored-resource listing this service pages, so each one walks
/// its records exactly the same way; [`catalog_page`] is the same walk over this
/// service's static catalogs. The start of the page comes from [`records_from`],
/// [`regional_records_from`] or [`records_in_region_from`], and a listing whose
/// request NARROWS which records it reports puts that filter on the iterator
/// ([`Iterator::filter`]): the page size then counts the records reported, the
/// next page starts at the first record after them that this listing reports too,
/// and a token naming a record this listing does not report names no page of it.
///
/// TOTAL on every input: the token is only ever COMPARED against the names, records
/// are only ever taken from the iterator in order and the page is bounded by
/// `page_size`, so there is no indexing, no slicing, no unwrap and no arithmetic
/// that can overflow or panic. `page_size` is at least one, so a walk over a
/// listing that holds records always advances.
pub fn resource_page<'a, R: 'a, T>(
    records: impl IntoIterator<Item = (&'a str, &'a R)>,
    page_token: Option<&str>,
    page_size: usize,
    mut view_of: impl FnMut(&'a str, &'a R) -> T,
) -> Option<(Vec<T>, Option<String>)> {
    let page_size = page_size.max(1);
    let mut records = records.into_iter().peekable();
    if let Some(token) = page_token.filter(|token| !token.is_empty()) {
        // The walk starts at the first name at or after the token, so the token
        // names a record of it exactly when that record IS the token.
        if records.peek().map(|(name, _)| *name) != Some(token) {
            return None;
        }
    }
    let mut page = Vec::new();
    for (name, record) in records {
        if page.len() == page_size {
            return Some((page, Some(name.to_string())));
        }
        page.push(view_of(name, record));
    }
    Some((page, None))
}

/// The message a stored-resource listing refuses an unrecognised `page_token`
/// with, where `reported` names what that listing reports ("an instance").
///
/// One wording for all of them: the refusal says which token was sent, that it
/// names nothing the listing reports, and what a page token is — so a caller that
/// sent a token of another listing, or one it invented, is told what to send
/// instead.
pub fn invalid_page_token_message(token: &str, reported: &str) -> String {
    format!(
        "pageToken {token:?} does not name {reported} this listing reports; a page token is the \
         name a previous page reported as nextPageToken"
    )
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
/// Lightsail pricing page, which `GetBundles` answers from), at the generation
/// [`BUNDLE_GENERATION`] publishes. Each step doubles RAM while the SSD size and
/// the monthly data transfer follow the plan table; every plan up to and
/// including the 8 GB one carries two vCPUs, after which the vCPU count doubles
/// with the plan; the Linux/Unix monthly price is the plan table's, and the
/// Windows plan of the same size costs more because it carries that platform's
/// licence. The `power` values are the ones Lightsail publishes for its plans:
/// 298 for the smallest, 500 for `micro` (the value the modelled
/// `Blueprint$minPower` documentation's own example names) and then a further
/// 1000 per doubling step.
///
/// Lightsail has retired none of these sizes, so every entry is active.
pub const BUNDLE_SPECS: &[BundleSpec] = &[
    BundleSpec {
        size: "nano",
        cpu_count: 2,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_gb_per_month: 1_024,
        power: 298,
        price_usd_per_month: 5.0,
        windows_price_usd_per_month: 9.5,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "micro",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
        power: 500,
        price_usd_per_month: 7.0,
        windows_price_usd_per_month: 16.0,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "small",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 60,
        transfer_gb_per_month: 3_072,
        power: 1_000,
        price_usd_per_month: 12.0,
        windows_price_usd_per_month: 26.0,
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
        price_usd_per_month: 24.0,
        windows_price_usd_per_month: 46.0,
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
        price_usd_per_month: 44.0,
        windows_price_usd_per_month: 86.0,
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
        price_usd_per_month: 84.0,
        windows_price_usd_per_month: 166.0,
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
        price_usd_per_month: 164.0,
        windows_price_usd_per_month: 326.0,
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
        price_usd_per_month: 324.0,
        windows_price_usd_per_month: 646.0,
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
/// `Micro`): the token with its first LETTER upper-cased, so a size token that
/// opens with a multiplier capitalises the letter after it (`2xlarge` ->
/// `2Xlarge`) — the form the recorded plan names carry.
pub fn bundle_size_name(size: &str) -> String {
    let Some(first_letter) = size.find(char::is_alphabetic) else {
        return size.to_string();
    };
    let (multiplier, word) = size.split_at(first_letter);
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => {
            multiplier.to_string() + &first.to_uppercase().collect::<String>() + chars.as_str()
        }
        None => multiplier.to_string(),
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
/// The order is by PLATFORM first — every plan of a platform in
/// [`BUNDLE_PLATFORMS`]' order, each platform's plans in `specs`' own order — so
/// the Linux/Unix plans of every size precede the Windows ones, which is the
/// order the catalog is published in. Two reads answer identically. Taking the
/// catalog as an argument is what lets the slice rule be exercised against a
/// catalog that holds a retired plan, which the published one does not.
pub fn bundles_from(specs: &[BundleSpec], include_inactive: bool) -> Vec<Bundle> {
    BUNDLE_PLATFORMS
        .iter()
        .flat_map(|platform| {
            specs
                .iter()
                .filter(|spec| include_inactive || spec.is_active)
                .map(|spec| spec.view(platform))
        })
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

/// The plan out of `specs` that a bucket may be PUT ON — created on, or moved
/// onto — or `None` when `bundle_id` names none.
///
/// A plan qualifies when the catalog holds it and it is still active:
/// `BucketBundle$isActive` is what says a bundle can be used — "Indicates
/// whether the bundle is active. Use for a new or existing bucket." — so a
/// retired plan is reportable (and still resolves for a bucket already running
/// on it, see [`bucket_bundle_spec`]) without being one a request can name.
///
/// Taking the catalog as an argument is what lets this rule be exercised against
/// a catalog that holds a retired plan, which the published one does not yet.
pub fn applicable_bucket_bundle_in(
    specs: &[BucketBundleSpec],
    bundle_id: &str,
) -> Option<BucketBundleSpec> {
    specs
        .iter()
        .find(|spec| spec.bundle_id == bundle_id && spec.is_active)
        .copied()
}

/// The plan a bucket may be put on, out of this service's published catalog:
/// [`applicable_bucket_bundle_in`] over [`BUCKET_BUNDLE_SPECS`]. The one owner of
/// that rule, which `CreateBucket` and `UpdateBucketBundle` both ask.
pub fn applicable_bucket_bundle(bundle_id: &str) -> Option<BucketBundleSpec> {
    applicable_bucket_bundle_in(BUCKET_BUNDLE_SPECS, bundle_id)
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
/// What `UpdateBucket` records for a request asking to suspend versioning of the
/// bucket's objects — see [`object_versioning_from_update`].
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

/// The `SetResourceAccessForBucket$access` value that gives a Lightsail resource
/// access to a bucket, from that member's own documentation: "`allow` - Allows
/// access to the bucket and its objects."
pub const RESOURCE_BUCKET_ACCESS_ALLOW: &str = "allow";

/// The `SetResourceAccessForBucket$access` value that takes that access away
/// again, from that member's own documentation: "`deny` - Denies access to the
/// bucket and its objects. Use this setting to remove access for a resource
/// previously set to allow."
pub const RESOURCE_BUCKET_ACCESS_DENY: &str = "deny";

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

/// Record the object versioning of the bucket named `name` in its object store:
/// what makes the bucket's objects actually be kept as versions, or stop being
/// kept, once `UpdateBucket` has changed the bucket's `objectVersioning`.
///
/// The control plane's value and the object store's are two halves of ONE
/// setting — "All objects added to the bucket receive a unique version ID" — so a
/// change to the first is carried to the second the way
/// [`claim_bucket_object_store`] carries a create's.
///
/// `true` when the object store held such a bucket, `false` when it did not, and
/// `None` when that store cannot be reached at all. The same LOCK ORDER as
/// [`claim_bucket_object_store`].
pub fn set_bucket_object_store_versioning<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    versioned: bool,
) -> Option<bool> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    let Some(bucket) = store.bucket_mut(name) else {
        return Some(false);
    };
    bucket.set_versioning(
        if versioned {
            ::service_com_amazonaws_s3_amazons3::state::VERSIONING_ENABLED_STATUS
        } else {
            ::service_com_amazonaws_s3_amazons3::state::VERSIONING_SUSPENDED_STATUS
        },
        None,
    );
    Some(true)
}

/// How many object keys the bucket named `name` holds in its object store — the
/// keys with a current object plus the keys only noncurrent versions are left of,
/// since a stored version is an object the bucket still holds.
///
/// KEYS, not versions: a key several versions are stored of counts once here. What
/// the bucket's storage metrics are over is the versions themselves and their
/// bytes — see [`bucket_object_storage`].
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

/// What a bucket's object store holds, measured over every object version it
/// keeps: the two quantities a bucket's storage metrics are computed from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BucketStorage {
    /// How many objects the bucket holds, every stored version counted —
    /// a noncurrent version is an object the bucket still holds.
    ///
    /// Delete markers are NOT among them: a marker holds no object and no bytes,
    /// which is why the object store keeps it as a version of its own
    /// (`::service_com_amazonaws_s3_amazons3::state::StoredVersion`).
    pub objects: u64,
    /// How many bytes those objects' payloads are, summed over the same set.
    pub bytes: u64,
}

/// Measure what the bucket named `name` holds in its object store right now:
/// the object versions it keeps and the bytes they are.
///
/// The one place this service measures a bucket's contents, for the storage
/// metrics `GetBucketMetricData` reports — the same object store
/// [`bucket_object_count`] reads whether the bucket is empty from, because that
/// store is the only place that knows what a bucket holds.
///
/// A bucket this emulator holds no in-progress multipart upload PARTS for
/// contributes none: the object store records an upload's settled members but no
/// part of one, so there is nothing of a part to count or to size.
///
/// A name the object store holds no bucket for holds nothing, and measures zero.
/// A pure READ: `None` only when that store cannot be reached at all. The same
/// LOCK ORDER as [`claim_bucket_object_store`].
pub fn bucket_object_storage<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
) -> Option<BucketStorage> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    let Some(bucket) = store.bucket(name) else {
        return Some(BucketStorage::default());
    };
    let mut measured = BucketStorage::default();
    for key in bucket.version_keys() {
        for version in bucket.object_versions(key) {
            if let Some(object) = version.as_object() {
                measured.objects = measured.objects.saturating_add(1);
                measured.bytes = measured
                    .bytes
                    .saturating_add(u64::try_from(object.body.len()).unwrap_or(u64::MAX));
            }
        }
    }
    Some(measured)
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

/// The two values an update may ask a bucket's object versioning to become, from
/// the `UpdateBucketRequest$versioning` documentation: "`Enabled` - Enables
/// versioning of objects in the specified bucket" and "`Suspended` - Suspends
/// versioning of objects in the specified bucket".
///
/// `NeverEnabled` is deliberately NOT among them: it is the state of a bucket
/// versioning was never asked for, not something a request can ask a bucket to
/// go back to.
pub const BUCKET_VERSIONING_REQUEST_VALUES: &[&str] =
    &[OBJECT_VERSIONING_ENABLED, OBJECT_VERSIONING_SUSPENDED];

/// The `objectVersioning` a bucket reports once an update asked for
/// `versioning`, or `None` when that is not one of the values such a request may
/// carry ([`BUCKET_VERSIONING_REQUEST_VALUES`]).
///
/// The answer is the catalog's own spelling, so what the bucket reports is a
/// modelled value rather than however the request spelled it.
pub fn object_versioning_from_update(versioning: &str) -> Option<&'static str> {
    BUCKET_VERSIONING_REQUEST_VALUES
        .iter()
        .find(|value| **value == versioning)
        .copied()
}

/// How many digits an Amazon Web Services account ID is: the emulator's own
/// account (`engine::emulator_account_id`) is one of them, and so is every
/// account a caller can name in a request.
pub const AWS_ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an Amazon Web Services account ID — [`AWS_ACCOUNT_ID_DIGITS`]
/// decimal digits and nothing else.
///
/// The one place this crate decides that, for every member that takes an account
/// of its own rather than a resource of this one.
pub fn is_aws_account_id(value: &str) -> bool {
    value.len() == AWS_ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
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
    /// When this bucket's bundle was last changed; `None` on one that has never
    /// been moved. A bucket's bundle can be updated only once within a monthly
    /// billing cycle, so the instant of that change is what says whether another
    /// one is available — see [`StoredBucket::able_to_update_bundle`].
    pub bundle_updated_at_ms: Option<i64>,
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
            able_to_update_bundle: Some(self.able_to_update_bundle(now_ms())),
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

    /// Whether the one bundle change this bucket's monthly billing cycle allows
    /// is available at `now_ms` — the `ableToUpdateBundle` a read reports.
    pub fn able_to_update_bundle(&self, now_ms: i64) -> bool {
        bundle_change_available(self.bundle_updated_at_ms, now_ms)
    }

    /// Give the resource `name` of kind `resource_type` access to this bucket
    /// (`allow`), or take that access away again (`deny`).
    ///
    /// A resource either receives access or it does not, so this is a SET keyed
    /// on the resource's name: a grant to a resource that already has access
    /// adds no second entry, and a revoke of one that has none removes nothing.
    /// `SetResourceAccessForBucket$access` documents `deny` as exactly that
    /// inverse — "use this setting to remove access for a resource previously
    /// set to allow" — so one call of each kind is enough to reach either state
    /// and repeating it changes nothing. Entries are kept in the order access
    /// was given, so every read reports the same sequence; the set is bounded by
    /// the resources this store holds, since a name appears in it at most once.
    ///
    /// Answers whether the set CHANGED, so a caller that has to log what it
    /// changed (the release a resource's removal performs — see
    /// [`ComAmazonawsLightsailLightsail20161128State::remove_instance`]) can tell
    /// a grant it released from one that was never held.
    pub fn set_resource_access(&mut self, name: &str, resource_type: &str, allow: bool) -> bool {
        let held = self
            .resources_receiving_access
            .iter()
            .position(|resource| resource.name.as_deref() == Some(name));
        match (allow, held) {
            (true, None) => {
                self.resources_receiving_access
                    .push(ResourceReceivingAccess {
                        name: Some(name.to_string()),
                        resource_type: Some(resource_type.to_string()),
                    });
                true
            }
            (false, Some(index)) => {
                self.resources_receiving_access.remove(index);
                true
            }
            // Already in the state the request asks for: nothing to change.
            (true, Some(_)) | (false, None) => false,
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
    /// `Blueprint$group` — "The group name of the blueprint (`amazon-linux`)".
    ///
    /// The group is PUBLISHED per entry rather than derived from the id: the
    /// catalog groups an image family under the name the service gives that
    /// family, which is not always the id's own words — the recorded
    /// `windows_server_2025` entry is grouped `windows_2025`, where its family
    /// carries the release and drops the `server` word.
    pub group: &'static str,
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
    /// `Blueprint$productUrl` — "The product URL to learn more about the image
    /// or blueprint", the page the image's VENDOR publishes about it, or empty
    /// where the catalog publishes none and the member is reported absent.
    pub product_url: &'static str,
    /// `Blueprint$licenseUrl` — "The end-user license agreement URL for the
    /// image or blueprint", the licence terms that vendor publishes, or empty
    /// where the catalog publishes none and the member is reported absent.
    pub license_url: &'static str,
}

/// One value of the modelled `BlueprintType` enum: an operating-system image.
pub const BLUEPRINT_TYPE_OS: &str = "os";

/// One value of the modelled `BlueprintType` enum: an application stack.
pub const BLUEPRINT_TYPE_APP: &str = "app";

/// The pages Microsoft publishes about Windows Server and about the licence
/// terms its images are offered under — `Blueprint$productUrl` and
/// `Blueprint$licenseUrl` of every Windows entry of [`BLUEPRINTS`], which the
/// recorded answer reports both of.
const WINDOWS_SERVER_PRODUCT_URL: &str = "https://www.microsoft.com/en-us/windows-server";
/// See [`WINDOWS_SERVER_PRODUCT_URL`].
const WINDOWS_SERVER_LICENSE_URL: &str =
    "https://www.microsoft.com/licensing/docs/view/Product-Terms";

/// The licence agreement Bitnami publishes for the application stacks it
/// packages — `Blueprint$licenseUrl` of every Bitnami-packaged entry of
/// [`BLUEPRINTS`].
const BITNAMI_LICENSE_URL: &str = "https://bitnami.com/agreement";

/// `Blueprint$versionCode` — "The version code" — of every entry of
/// [`BLUEPRINTS`]: Lightsail's own revision counter for the image it publishes
/// at that version, which starts at the first revision and is the value the
/// recorded entries carry.
pub const BLUEPRINT_VERSION_CODE: &str = "1";

/// `Blueprint$minPower` of a Windows image — "The minimum bundle power required
/// to run this blueprint".
///
/// A Windows image does not run on the smallest instance plan, so its floor is
/// the next plan's power: the `micro` plan's [`BundleSpec::power`], which is the
/// value the recorded Windows entries report. A Linux/Unix image this catalog
/// publishes runs on every plan, which `0` states — "`0` indicates that the
/// blueprint runs on all instance sizes".
pub const WINDOWS_BLUEPRINT_MIN_POWER: i32 = 500;

/// `Blueprint$minPower` of an image that runs on every plan this service
/// publishes. See [`WINDOWS_BLUEPRINT_MIN_POWER`].
pub const BLUEPRINT_MIN_POWER_ANY_SIZE: i32 = 0;

/// The instance blueprints this service publishes — the catalog `GetBlueprints`
/// reports and `CreateInstances` resolves `blueprintId` against.
///
/// SOURCE: the blueprint ids Lightsail's instance-image list publishes (the
/// Lightsail console's "Select a blueprint" catalog, which `GetBlueprints`
/// answers from): the operating-system families `windows_server`,
/// `amazon_linux`, `ubuntu`, `debian`, `almalinux`, `centos_stream`, `freebsd`
/// and `opensuse`, each at the releases Lightsail offers, plus the
/// Bitnami-packaged and hosting-panel application stacks. An entry whose
/// upstream release has reached end of life stays in the catalog but is marked
/// inactive, which is the state Lightsail keeps a retired image in so existing
/// instances still resolve it while a new instance cannot be launched from it.
///
/// The ORDER is the catalog's published one, which `GetBlueprints` walks: the
/// Windows images first, newest release first, then the Linux/Unix operating
/// systems and last the application stacks — the order the recorded answer
/// reports, whose first two entries are `windows_server_2025` and
/// `windows_server_2022`.
pub const BLUEPRINTS: &[BlueprintSpec] = &[
    // Windows Server operating systems, newest release first.
    BlueprintSpec {
        blueprint_id: "windows_server_2025",
        group: "windows_2025",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2022",
        group: "windows_2022",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2019",
        group: "windows_2019",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2016",
        group: "windows_2016",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    // Linux/Unix operating systems.
    BlueprintSpec {
        blueprint_id: "amazon_linux_2023",
        group: "amazon-linux",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://aws.amazon.com/linux/amazon-linux-2023/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "amazon_linux_2",
        group: "amazon-linux",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://aws.amazon.com/amazon-linux-2/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_24_04",
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_22_04",
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_20_04",
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_18_04",
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        // Ubuntu 18.04 LTS is past its upstream end of standard support.
        is_active: false,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "debian_12",
        group: "debian",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.debian.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "debian_11",
        group: "debian",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.debian.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "debian_10",
        group: "debian",
        blueprint_type: BLUEPRINT_TYPE_OS,
        // Debian 10 is past its upstream end of life.
        is_active: false,
        product_url: "https://www.debian.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "almalinux_9",
        group: "almalinux",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://almalinux.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "centos_stream_9",
        group: "centos-stream",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.centos.org/centos-stream/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "freebsd_14",
        group: "freebsd",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.freebsd.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "opensuse_15",
        group: "opensuse",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.opensuse.org/",
        license_url: "",
    },
    // Application stacks.
    BlueprintSpec {
        blueprint_id: "wordpress",
        group: "wordpress",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/wordpress",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "wordpress_multisite",
        group: "wordpress-multisite",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/wordpress-multisite",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "lamp_8_bitnami",
        group: "lamp-bitnami",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/lamp",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "nodejs",
        group: "nodejs",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/nodejs",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "joomla",
        group: "joomla",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/joomla",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "drupal",
        group: "drupal",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/drupal",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "magento",
        group: "magento",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/magento",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "mean",
        group: "mean",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/mean",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "ghost_bitnami",
        group: "ghost-bitnami",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/ghost",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "redmine",
        group: "redmine",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/redmine",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "nginx",
        group: "nginx",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/nginx",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "django",
        group: "django",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/django",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "gitlab",
        group: "gitlab",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/gitlab",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "prestashop",
        group: "prestashop",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/prestashop",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "cpanel_whm_almalinux",
        group: "cpanel-whm-almalinux",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://cpanel.net/",
        license_url: "https://cpanel.net/legal/",
    },
    BlueprintSpec {
        blueprint_id: "plesk_ubuntu_18_0_35",
        group: "plesk-ubuntu",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://www.plesk.com/",
        license_url: "https://www.plesk.com/legal/",
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

    /// `Blueprint$minPower` — "The minimum bundle power required to run this
    /// blueprint".
    ///
    /// The power of the smallest instance plan the image runs on: a Windows
    /// image does not run on the smallest plan
    /// ([`WINDOWS_BLUEPRINT_MIN_POWER`]), and a Linux/Unix image this catalog
    /// publishes runs on every plan, which `0` states
    /// ([`BLUEPRINT_MIN_POWER_ANY_SIZE`]).
    pub fn min_power(&self) -> i32 {
        if self.platform() == PLATFORM_WINDOWS {
            WINDOWS_BLUEPRINT_MIN_POWER
        } else {
            BLUEPRINT_MIN_POWER_ANY_SIZE
        }
    }

    /// This entry as the modelled `Blueprint`.
    ///
    /// `name` is [`blueprint_name`], the same derivation `Instance$blueprintName`
    /// reports, so the catalog's name for a blueprint is the name an instance
    /// created from it carries.
    pub fn view(&self) -> Blueprint {
        // `productUrl` and `licenseUrl` are optional, and an entry whose vendor
        // publishes neither carries neither: an empty link in the catalog is
        // reported as the ABSENT member rather than as an empty string.
        let link = |url: &'static str| (!url.is_empty()).then(|| url.to_string());
        Blueprint {
            blueprint_id: Some(self.blueprint_id.to_string()),
            name: Some(blueprint_name(self.blueprint_id)),
            group: Some(self.group.to_string()),
            r#type: Some(self.blueprint_type.to_string()),
            description: Some(self.description()),
            is_active: Some(self.is_active),
            min_power: Some(self.min_power()),
            version: self.version(),
            version_code: Some(BLUEPRINT_VERSION_CODE.to_string()),
            product_url: link(self.product_url),
            license_url: link(self.license_url),
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
    /// The revision label the engine's releases carry in their version
    /// description, or empty where that engine's releases carry none.
    ///
    /// A managed PostgreSQL release is published as a REVISION of its community
    /// version (`R1` being the first revision of that version), which is what
    /// the recorded `engineVersionDescription` of every PostgreSQL blueprint
    /// ends with; a MySQL release carries no such label, and the recorded MySQL
    /// blueprint's description ends at its version. So the label belongs to the
    /// ENGINE rather than to the description, and
    /// [`RelationalDatabaseBlueprintSpec::view`] appends it when the engine has
    /// one.
    pub engine_version_revision: &'static str,
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
        engine_version_revision: "",
        is_engine_default: true,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "13",
        engine_version: "13.23-rds.20260514",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "14",
        engine_version: "14.24",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "15",
        engine_version: "15.19",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "16",
        engine_version: "16.15",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "17",
        engine_version: "17.11",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "18",
        engine_version: "18.5",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: true,
    },
];

/// The revision label every managed PostgreSQL release this service serves is
/// published at: the first revision of its community version. See
/// [`RelationalDatabaseBlueprintSpec::engine_version_revision`].
const POSTGRES_ENGINE_VERSION_REVISION: &str = "R1";

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

    /// `RelationalDatabaseBlueprint$engineVersionDescription`: the engine named
    /// beside the version it is served at, followed by that release's revision
    /// label where the engine's releases carry one (`PostgreSQL 13.23-R1`,
    /// `MySQL 8.4.11`). See [`Self::engine_version_revision`].
    pub fn engine_version_description(&self) -> String {
        let version = &self.engine_version;
        let engine = &self.engine_display;
        if self.engine_version_revision.is_empty() {
            format!("{engine} {version}")
        } else {
            format!("{engine} {version}-{}", self.engine_version_revision)
        }
    }

    /// This entry as the modelled `RelationalDatabaseBlueprint`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabaseBlueprint {
        crate::ops::types::RelationalDatabaseBlueprint {
            blueprint_id: Some(self.blueprint_id()),
            engine: Some(self.engine.to_string()),
            engine_version: Some(self.engine_version.to_string()),
            engine_description: Some(self.engine_description.to_string()),
            // `engineVersionDescription` describes the version the way the
            // engine itself is named beside it ("MySQL 5.7.44"), followed by the
            // revision label the engine's releases carry, where it has one
            // ([`Self::engine_version_revision`]).
            engine_version_description: Some(self.engine_version_description()),
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

/// One page of `reported`: at most `page_size` database blueprints from the entry
/// `page_token` names onward, and the id of the first entry of the page after it
/// — present only while entries remain.
///
/// `None` is the answer when the token names no blueprint of `reported`: there is
/// no page to report, which the caller turns into a refusal. An absent token asks
/// for the first page.
///
/// The walk itself is [`catalog_page`], shared with this service's other paged
/// catalogs; only the member a token names is this catalog's own, and it is
/// DERIVED from the entry rather than held by it, which is what that walk's `Cow`
/// answer is for.
pub fn relational_database_blueprint_page(
    reported: &[RelationalDatabaseBlueprintSpec],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<RelationalDatabaseBlueprintSpec>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |spec| {
        Some(std::borrow::Cow::Owned(spec.blueprint_id()))
    })
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

/// Every managed-database plan this service publishes: the standard
/// (single-database) plans of the generation it serves, cheapest first, then
/// their high-availability twins, then the plans of the generation it has
/// retired.
///
/// SOURCE: the managed-database plan table of the Lightsail pricing page — a
/// standard plan at $15 a month with 1 GB RAM, 2 vCPUs and 40 GB of SSD storage,
/// $30 with 2 GB / 2 vCPUs / 80 GB, $60 with 4 GB / 2 vCPUs / 120 GB, $115 with
/// 8 GB / 2 vCPUs / 240 GB, $230 with 16 GB / 4 vCPUs / 480 GB and $460 with
/// 32 GB / 8 vCPUs / 960 GB, with 100 GB of data transfer included on the three
/// smallest plans, 200 GB on the 8 GB plan, 300 GB on the 16 GB one and 400 GB
/// on the largest — the price doubling with the plan above the 8 GB one — and
/// the high-availability column of the same table, which offers the same
/// hardware with a standby in a second Availability Zone for twice the standard
/// plan's price. Lightsail encrypts managed-database storage and its automatic
/// snapshots at rest on every plan, so every entry is encrypted.
///
/// The GENERATION is a property of the catalog, as it is for an instance plan:
/// the plans published today are the `2_0` generation, and the superseded `1_0`
/// generation stays here as INACTIVE — a database created on it still resolves
/// its hardware, and `GetRelationalDatabaseBundles` reports it only when asked
/// to include inactive bundles.
///
/// The ORDER is this slice's own (cheapest first, standard plans before
/// high-availability ones, the published generation before the retired one): it
/// is what the paged walk reports, and it is stable so two reads answer
/// identically.
pub const RELATIONAL_DATABASE_BUNDLE_SPECS: &[RelationalDatabaseBundleSpec] = &[
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_2_0",
        name: "Micro",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 15.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_2_0",
        name: "Small",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_2_0",
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
        bundle_id: "large_2_0",
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
        bundle_id: "xlarge_2_0",
        name: "Xlarge",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 480,
        transfer_per_month_in_gb: 300,
        price_usd_per_month: 230.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "2xlarge_2_0",
        name: "2Xlarge",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 960,
        transfer_per_month_in_gb: 400,
        price_usd_per_month: 460.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_ha_2_0",
        name: "Micro with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_ha_2_0",
        name: "Small with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_ha_2_0",
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
        bundle_id: "large_ha_2_0",
        name: "Large with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 230.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "xlarge_ha_2_0",
        name: "Xlarge with High Availability",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 480,
        transfer_per_month_in_gb: 300,
        price_usd_per_month: 460.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "2xlarge_ha_2_0",
        name: "2Xlarge with High Availability",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 960,
        transfer_per_month_in_gb: 400,
        price_usd_per_month: 920.0,
        is_encrypted: true,
        is_active: true,
    },
    // The superseded first generation: still resolvable for a database created
    // on it, and reported only when a call asks for inactive bundles.
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_1_0",
        name: "Micro",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 15.0,
        is_encrypted: true,
        is_active: false,
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
        is_active: false,
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
        is_active: false,
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
        is_active: false,
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
        is_active: false,
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
        is_active: false,
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
        is_active: false,
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
        is_active: false,
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
    if is_postgres_engine(engine) {
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

/// The Region names a stored `ResourceLocation` may carry — the modelled
/// `RegionName` enum's values (`smithy_query(query="shape",shape="RegionName")`,
/// generated here as `REGION_NAME_VALUES`) — for a refusal that has to tell the
/// caller what to send instead.
///
/// The enum is WIDER than the Regions `GetRegions` lists as served, so a refusal
/// about where a resource can be CREATED names
/// [`served_region_names`] instead.
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
    /// puts after "is recommended to serve users in".
    pub serves: &'static str,
    /// Whether this Region is an OPT-IN Region — one AWS leaves disabled until an
    /// account enables it, which every Region introduced after the opt-in
    /// mechanism was published is.
    ///
    /// It is the description that carries this: the sentence of an opt-in Region
    /// opens by saying so ([`RegionSpec::description`]), and this response has no
    /// other member that tells a caller the Region is disabled by default.
    pub is_opt_in: bool,
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
    /// documentation shows: what the Region is recommended for, opened by the
    /// opt-in clause where the Region is one an account must enable
    /// ([`Self::is_opt_in`]).
    pub fn description(&self) -> String {
        let opening = if self.is_opt_in {
            "This opt-in Region (Region that is disabled by default)"
        } else {
            "This region"
        };
        format!("{opening} is recommended to serve users in {}", self.serves)
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
/// Region is recommended for — the clause after the sentence stem
/// `Region$description` documents, as AWS's own answer gives it for that Region
/// (`eu-west-1` and `eu-west-2` share one clause, and `eu-central-1`'s names a
/// whole set of continents). `is_opt_in` marks the Regions AWS leaves disabled
/// until an account enables them — the Regions introduced after the opt-in
/// mechanism was published (`ap-east-1`, `ap-southeast-3`, `ap-southeast-5` and
/// `eu-south-2` of this catalog) — whose description opens with the opt-in
/// clause instead.
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
        is_opt_in: false,
        availability_zone_letters: "abcdef",
    },
    RegionSpec {
        name: "us-east-2",
        display_name: "Ohio",
        serves: "the eastern United States",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "us-west-2",
        display_name: "Oregon",
        serves: "the northwestern United States, Alaska, and western Canada",
        is_opt_in: false,
        availability_zone_letters: "abcd",
    },
    RegionSpec {
        name: "ap-south-1",
        display_name: "Mumbai",
        serves: "India",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-northeast-2",
        display_name: "Seoul",
        serves: "South Korea",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-1",
        display_name: "Singapore",
        serves: "Singapore and southeast Asia",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-2",
        display_name: "Sydney",
        serves: "Australia and New Zealand",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-northeast-1",
        display_name: "Tokyo",
        serves: "Japan",
        is_opt_in: false,
        availability_zone_letters: "acd",
    },
    RegionSpec {
        name: "ca-central-1",
        display_name: "Montreal",
        serves: "Canada",
        is_opt_in: false,
        availability_zone_letters: "ab",
    },
    RegionSpec {
        name: "eu-central-1",
        display_name: "Frankfurt",
        serves: "Europe, the Middle East, and Africa",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-1",
        display_name: "Ireland",
        serves: "Ireland, the United Kingdom, and Iceland",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-2",
        display_name: "London",
        serves: "Ireland, the United Kingdom, and Iceland",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-3",
        display_name: "Paris",
        serves: "France",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-north-1",
        display_name: "Stockholm",
        serves: "Stockholm and north Europe",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "sa-east-1",
        display_name: "Sao Paulo",
        serves: "Brazil and eastern South America",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-east-1",
        display_name: "Hong Kong",
        serves: "Hong Kong",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-south-2",
        display_name: "Spain",
        serves: "Spain",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-3",
        display_name: "Jakarta",
        serves: "Indonesia",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-5",
        display_name: "Malaysia",
        serves: "Malaysia",
        is_opt_in: true,
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
/// A zone name is a served Region's name followed by one of the zone letters
/// THAT Region publishes, so a well-formed `<region><letter>` whose letter the
/// Region does not publish — and a zone of a Region the catalog does not hold —
/// resolves to nothing: it names no place a resource can be created.
pub fn region_spec_of_availability_zone(zone: &str) -> Option<&'static RegionSpec> {
    REGION_SPECS
        .iter()
        .find(|spec| spec.zone_names().iter().any(|name| name == zone))
}

/// The Regions `GetRegions` reports as served, for a refusal that has to tell
/// the caller where it can create a resource.
pub fn served_region_names() -> String {
    REGION_SPECS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Region a resource the caller asks to create in the Availability Zone
/// `zone` belongs to, or the sentence that request is refused with.
///
/// The ONE resolution every operation that creates a resource in a
/// caller-named zone goes through, so one zone string cannot be accepted by one
/// of them and refused by another. `availabilityZone` is documented as a value
/// the caller takes from `GetRegions` ("Use the `get regions` operation to list
/// the Availability Zones where Lightsail is currently available"), so the zone
/// must be one a served Region PUBLISHES — see
/// [`region_spec_of_availability_zone`] — rather than merely one that reads like
/// a zone name, and a zone this service publishes nowhere is refused instead of
/// stored in a `ResourceLocation` no `GetRegions` answer accounts for.
///
/// `member` is the path of the member the zone arrived in, so the refusal names
/// what the CALLER sent rather than the member of whichever operation the
/// resolution is shared with.
pub fn created_in_availability_zone(
    member: &str,
    zone: &str,
) -> Result<&'static RegionSpec, String> {
    region_spec_of_availability_zone(zone).ok_or_else(|| {
        format!(
            "{member} '{zone}' is not an Availability Zone this service publishes: it must be \
             one of the zones GetRegions reports for a region of [{}]",
            served_region_names()
        )
    })
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
/// address must be in the notation its own member takes, each address must be of
/// a family `ip_address_type` gives the instance, and an alias must be one
/// Lightsail publishes. A rule that names no address at all is taken as open
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
    // Each address member's own path, spelled once: both the notation check and
    // the family check below name the member the caller has to fix.
    let cidrs_member = format!("{member}.cidrs");
    let ipv6_cidrs_member = format!("{member}.ipv6Cidrs");
    let cidrs = checked_cidrs(
        &cidrs_member,
        port_info.cidrs.as_deref().unwrap_or_default(),
        is_ipv4_cidr,
        "an IPv4 address or an IPv4 range in CIDR notation",
    )?;
    let ipv6_cidrs = checked_cidrs(
        &ipv6_cidrs_member,
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
    // Each address member is checked against the families the INSTANCE has, not
    // only against its own notation: a rule cannot admit an address family the
    // instance does not have, so an address of a family `ip_address_type` gives
    // the instance none of admits nothing, and is refused rather than stored.
    // That is the same rule the substitution below applies to a request naming
    // no address, and the one [`default_port_rules`] applies to a new instance's
    // firewall. `cidrListAliases` names no address family of its own: it is a
    // preconfigured range Lightsail publishes, so it is left to its own check
    // above.
    check_family_the_instance_has(&cidrs_member, &cidrs, ip_address_type, AddressFamily::Ipv4)?;
    check_family_the_instance_has(
        &ipv6_cidrs_member,
        &ipv6_cidrs,
        ip_address_type,
        AddressFamily::Ipv6,
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

/// One of the two address families a firewall rule's addresses can be of: the
/// family `cidrs` carries, and the one `ipv6Cidrs` carries.
#[derive(Debug, Clone, Copy)]
enum AddressFamily {
    Ipv4,
    Ipv6,
}

impl AddressFamily {
    /// Whether an instance with `ip_address_type` has an address of this family,
    /// asked of the ONE predicate per family this service already keeps.
    fn given_by(self, ip_address_type: &str) -> bool {
        match self {
            Self::Ipv4 => ip_address_type_has_ipv4(ip_address_type),
            Self::Ipv6 => ip_address_type_has_ipv6(ip_address_type),
        }
    }

    /// The family's name, as a refusal names it.
    fn label(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }
}

/// `Ok(())` when `entries` names no address at all or an instance with
/// `ip_address_type` has an address of `family`, and otherwise a refusal naming
/// `member`, that family and the address type that gives the instance none of it.
///
/// An instance is reachable only on the families its `ip_address_type` gives it,
/// so a rule naming an address of any other family would admit nothing — see
/// [`anywhere_cidrs`], which leaves out the same family for the same reason. The
/// family decides BOTH halves of the check, so the predicate asked and the name
/// reported cannot disagree.
fn check_family_the_instance_has(
    member: &str,
    entries: &[String],
    ip_address_type: &str,
    family: AddressFamily,
) -> Result<(), String> {
    if entries.is_empty() || family.given_by(ip_address_type) {
        return Ok(());
    }
    let family = family.label();
    Err(format!(
        "{member} names {family} addresses, but the instance's ipAddressType \
         '{ip_address_type}' gives it no {family} address: a rule cannot admit an address \
         family the instance does not have"
    ))
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
    /// The GUI (Amazon DCV) session access `StartGUISession` and
    /// `CreateGUISessionAccessDetails` created for this virtual computer, while
    /// one exists. A session runs ON the instance, so it lives on the instance's
    /// own record: deleting the instance takes its session with it.
    pub gui_session: Option<StoredGuiSession>,
    /// How this instance's metadata service is configured — what
    /// `UpdateInstanceMetadataOptions` sets and `Instance$metadataOptions`
    /// reports.
    pub metadata_options: StoredMetadataOptions,
    /// The host keys the service has RECORDED for this instance — the known
    /// host keys the Lightsail browser-based SSH and RDP clients authenticate
    /// it by, each carrying "the time that the SSH host key or RDP certificate
    /// was recorded by Lightsail" (`HostKeyAttributes$witnessedAt`).
    ///
    /// They live on the instance's own record, as its GUI session access does:
    /// a recorded host key is the service's record of THAT instance, so
    /// deleting the instance takes its recorded host keys with it.
    ///
    /// An instance's record starts empty — nothing is recorded until the
    /// browser-based access path witnesses the instance's key — and
    /// `DeleteKnownHostKeys` is what empties it again, which is how a caller
    /// recovers from a host key mismatch.
    pub known_host_keys: Vec<HostKeyAttributes>,
}

/// The state of an instance's metadata options while the service is still
/// putting a new selection in place: "`pending` - The metadata options are being
/// updated. The instance is not yet ready to process metadata traffic with the
/// new selection" (the modelled `InstanceMetadataOptions$state`).
pub const INSTANCE_METADATA_STATE_PENDING: &str = "pending";
/// The state once the selection is in place: "`applied` - The metadata options
/// have been successfully applied to the instance" (same member).
pub const INSTANCE_METADATA_STATE_APPLIED: &str = "applied";

/// The `HttpTokens` value a metadata request needs no signed token under, and
/// the value this setting takes when a request names none:
/// "If the parameter is not specified in the request, the default state is
/// `optional`" (the modelled `UpdateInstanceMetadataOptionsRequest$httpTokens`).
pub const HTTP_TOKENS_OPTIONAL: &str = "optional";

/// The `HttpEndpoint` value an instance's metadata endpoint answers on, which is
/// how Lightsail creates an instance: the endpoint serves, and
/// `UpdateInstanceMetadataOptions` is what turns it off.
pub const HTTP_ENDPOINT_ENABLED: &str = "enabled";

/// The `HttpProtocolIpv6` values the metadata service's IPv6 endpoint takes.
pub const HTTP_PROTOCOL_IPV6_ENABLED: &str = "enabled";
/// See [`HTTP_PROTOCOL_IPV6_ENABLED`].
pub const HTTP_PROTOCOL_IPV6_DISABLED: &str = "disabled";

/// The one Region whose instances have an IPv6 endpoint on their metadata
/// service at all, from that setting's own documented availability note
/// (`smithy_query(query="doc",shape="UpdateInstanceMetadataOptionsRequest",member="httpProtocolIpv6")`):
/// "This parameter is available only for instances in the Europe (Stockholm)
/// Amazon Web Services Region (eu-north-1)."
pub const IPV6_METADATA_ENDPOINT_REGION_NAME: &str = "eu-north-1";

/// The refusal a request that sets `httpProtocolIpv6` on an instance outside
/// [`IPV6_METADATA_ENDPOINT_REGION_NAME`] is answered with, as that member's own
/// documentation states the restriction.
pub const IPV6_METADATA_ENDPOINT_REGION_ONLY_MESSAGE: &str =
    "The httpProtocolIpv6 parameter is available only for instances in the Europe (Stockholm) \
     Amazon Web Services Region (eu-north-1).";

/// Whether an instance in `region_name` has the metadata service's IPv6 endpoint
/// to enable or disable.
///
/// The crate's ONE owner of that setting's documented Region restriction, asked
/// both by the operation that sets the member and by the projection that reports
/// it, so the restriction is decided in a single place.
pub fn region_has_ipv6_metadata_endpoint(region_name: &str) -> bool {
    region_name == IPV6_METADATA_ENDPOINT_REGION_NAME
}

/// The refusal for a request that sets `httpProtocolIpv6` on an instance in
/// `region_name`, and `None` when that Region's instances have the endpoint the
/// member configures.
pub fn ipv6_metadata_region_refusal(region_name: &str) -> Option<String> {
    (!region_has_ipv6_metadata_endpoint(region_name))
        .then(|| IPV6_METADATA_ENDPOINT_REGION_ONLY_MESSAGE.to_string())
}

/// The hop limit an instance's metadata service is created with: a metadata
/// response reaches the instance itself and goes no further, which is the one
/// hop a request from the instance needs.
pub const DEFAULT_HTTP_PUT_RESPONSE_HOP_LIMIT: i32 = 1;

/// The smallest hop limit the metadata service accepts.
///
/// The hop limit is the IP time-to-live of the metadata service's PUT response,
/// so a response has to be allowed at least the ONE hop that reaches the
/// requesting instance; a limit of zero would let no response arrive at all.
pub const MIN_HTTP_PUT_RESPONSE_HOP_LIMIT: i32 = 1;

/// The largest hop limit the metadata service accepts — the ceiling the instance
/// metadata service publishes for this setting, "Possible values: Integers from
/// 1 to 64" (the EC2 instance metadata service's own
/// `HttpPutResponseHopLimit`, which a Lightsail instance's metadata service is).
pub const MAX_HTTP_PUT_RESPONSE_HOP_LIMIT: i32 = 64;

/// One instance's metadata service configuration: the values
/// `UpdateInstanceMetadataOptions` sets, and the instant the clock owes the
/// instance a selection that is still being put in place.
#[derive(Debug, Clone)]
pub struct StoredMetadataOptions {
    pub http_tokens: String,
    pub http_endpoint: String,
    pub http_put_response_hop_limit: i32,
    /// What a caller has set the IPv6 metadata endpoint to, and `None` while no
    /// caller has set it: an instance nobody has configured reaches that
    /// endpoint exactly when it has IPv6 addressing at all, so the value
    /// reported then follows the instance's own address type.
    pub http_protocol_ipv6: Option<String>,
    /// The instant the selection now being put in place is applied, while one
    /// is: until then the options report `pending`, and from then on `applied`.
    pub applied_at_ms: Option<i64>,
}

impl Default for StoredMetadataOptions {
    /// The configuration Lightsail creates an instance's metadata service with:
    /// the endpoint serving, no signed token required, and the one hop that
    /// reaches the instance. Nothing is in flight, so it is `applied` already.
    fn default() -> Self {
        Self {
            http_tokens: HTTP_TOKENS_OPTIONAL.to_string(),
            http_endpoint: HTTP_ENDPOINT_ENABLED.to_string(),
            http_put_response_hop_limit: DEFAULT_HTTP_PUT_RESPONSE_HOP_LIMIT,
            http_protocol_ipv6: None,
            applied_at_ms: None,
        }
    }
}

impl StoredMetadataOptions {
    /// This configuration as the modelled `InstanceMetadataOptions`, for an
    /// instance whose own addressing is `ip_address_type` and which lives in
    /// `region_name`.
    ///
    /// `state` is read from what is outstanding rather than stored twice: a
    /// selection the clock still owes the instance is `pending`, and one it has
    /// nothing left to do for is `applied`.
    ///
    /// `httpProtocolIpv6` is the metadata service's IPv6 endpoint, which only an
    /// instance in [`IPV6_METADATA_ENDPOINT_REGION_NAME`] has at all, so an
    /// instance anywhere else reports it disabled whatever its own addressing:
    /// there is no such endpoint there to reach.
    pub fn view(&self, ip_address_type: &str, region_name: &str) -> InstanceMetadataOptions {
        InstanceMetadataOptions {
            state: Some(
                if self.applied_at_ms.is_some() {
                    INSTANCE_METADATA_STATE_PENDING
                } else {
                    INSTANCE_METADATA_STATE_APPLIED
                }
                .to_string(),
            ),
            http_tokens: Some(self.http_tokens.clone()),
            http_endpoint: Some(self.http_endpoint.clone()),
            http_put_response_hop_limit: Some(self.http_put_response_hop_limit),
            http_protocol_ipv6: Some(if !region_has_ipv6_metadata_endpoint(region_name) {
                HTTP_PROTOCOL_IPV6_DISABLED.to_string()
            } else {
                self.http_protocol_ipv6.clone().unwrap_or_else(|| {
                    if ip_address_type_has_ipv6(ip_address_type) {
                        HTTP_PROTOCOL_IPV6_ENABLED.to_string()
                    } else {
                        HTTP_PROTOCOL_IPV6_DISABLED.to_string()
                    }
                })
            }),
        }
    }
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
    ///
    /// `next_auto_snapshot_time_of_day` is when the next automatic copy of this
    /// instance is due, which lives on the SCHEDULE its add-on runs on rather
    /// than on this record, so it too is passed in by the store
    /// ([`ComAmazonawsLightsailLightsail20161128State::next_auto_snapshot_time_of_day`]).
    pub fn view(
        &self,
        attached_disks: Vec<crate::ops::types::Disk>,
        next_auto_snapshot_time_of_day: Option<String>,
    ) -> Instance {
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
            add_ons: Some(add_ons_view(&self.add_ons, next_auto_snapshot_time_of_day)),
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
            metadata_options: Some(
                self.metadata_options
                    .view(&self.ip_address_type, &self.location.region_name),
            ),
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

    /// Put `selection` in place on this instance's metadata service, answering
    /// the instant the clock owes the instance the applied options — `None` when
    /// they are applied already.
    ///
    /// A RUNNING instance is serving metadata traffic under the options it has,
    /// so a new selection is `pending` until the service has it in place: "When
    /// you modify the parameters on a running instance, the GetInstance or
    /// GetInstances API operation initially responds with a state of pending.
    /// After the parameter modifications are successfully applied, the state
    /// changes to applied" (the modelled `UpdateInstanceMetadataOptions`
    /// documentation). An instance that is not running is processing no metadata
    /// traffic at all, so there is nothing to put in place: the options it will
    /// serve under are already the new ones.
    ///
    /// The only two states a selection is ever put in place from are `running`
    /// and `stopped` — the operation that calls this refuses an instance in any
    /// other state — so the instance is serving metadata traffic or it is down,
    /// and never one the clock is still moving.
    pub fn apply_metadata_options(
        &mut self,
        selection: MetadataOptionsSelection,
        now_ms: i64,
    ) -> Option<i64> {
        // "If the parameter is not specified in the request, the default state is
        // `optional`" — the one member of this input whose absence is a value
        // rather than "the existing state is maintained".
        self.metadata_options.http_tokens = selection
            .http_tokens
            .unwrap_or_else(|| HTTP_TOKENS_OPTIONAL.to_string());
        if let Some(http_endpoint) = selection.http_endpoint {
            self.metadata_options.http_endpoint = http_endpoint;
        }
        if let Some(hop_limit) = selection.http_put_response_hop_limit {
            self.metadata_options.http_put_response_hop_limit = hop_limit;
        }
        if let Some(http_protocol_ipv6) = selection.http_protocol_ipv6 {
            self.metadata_options.http_protocol_ipv6 = Some(http_protocol_ipv6);
        }
        let applied_at_ms = (self.state == RUNNING)
            .then(|| now_ms.saturating_add(STATE_TRANSITION_MILLIS));
        self.metadata_options.applied_at_ms = applied_at_ms;
        applied_at_ms
    }
}

/// What one `UpdateInstanceMetadataOptions` request asks of an instance's
/// metadata service: a member it carries is what that setting becomes, and one
/// it leaves out is the caller naming no value for it.
#[derive(Debug, Clone, Default)]
pub struct MetadataOptionsSelection {
    pub http_tokens: Option<String>,
    pub http_endpoint: Option<String>,
    pub http_put_response_hop_limit: Option<i32>,
    pub http_protocol_ipv6: Option<String>,
}

// ---------------------------------------------------------------------------
// GUI (Amazon DCV) sessions
// ---------------------------------------------------------------------------

/// The modelled `Status` values a virtual computer's GUI session reports, from
/// the `Status` enum (`crate::ops::types::STATUS_VALUES`).
///
/// `notStarted` is reported when no session is up and nothing is bringing one
/// up: a running virtual computer whose session nobody has opened yet — opening
/// one is `StartGUISession`'s business ("Use StartGUISession to open the
/// session", the modelled `CreateGUISessionAccessDetails` documentation) — and
/// every state of a computer that is not running, because an Amazon DCV session
/// runs ON that computer.
pub const GUI_SESSION_STATUS_NOT_STARTED: &str = "notStarted";
/// The virtual computer the session would run on is still being set up, so the
/// session cannot be brought up yet.
pub const GUI_SESSION_STATUS_SETTING_UP_INSTANCE: &str = "settingUpInstance";
/// The session's access details exist and the session is coming up.
pub const GUI_SESSION_STATUS_STARTING: &str = "starting";
/// The session is serving: its two URLs reach a session that is up.
pub const GUI_SESSION_STATUS_STARTED: &str = "started";
/// The session is being terminated: the close a `StopGUISession` call asked for
/// is still in flight, so the session is on its way down.
pub const GUI_SESSION_STATUS_STOPPING: &str = "stopping";
/// The session has been terminated — "The session will close and any unsaved
/// data will be lost" (the modelled `StopGUISession` documentation) — so the
/// URLs it was published under reach nothing and only a start brings a session
/// back on that virtual computer.
pub const GUI_SESSION_STATUS_STOPPED: &str = "stopped";
/// The session's active window ([`GUI_SESSION_ACTIVE_MILLIS`]) has run out. The
/// caller resumes it with `StartGUISession` — "Use this action to resume the
/// session after it expires" (the modelled `StartGUISession` documentation) —
/// which is why creating access details does NOT silently start another hour:
/// that call reports a session, it does not open one.
pub const GUI_SESSION_STATUS_START_EXPIRED: &str = "startExpired";

/// How long a GUI session stays active once it has started.
///
/// AWS DOCUMENTED, not chosen: "The session will be active for 1 hour" — the
/// modelled `StartGUISession` documentation — so the window keeps that value
/// rather than being shortened to something a test can wait out.
pub const GUI_SESSION_ACTIVE_MILLIS: i64 = 60 * 60 * 1_000;

/// The name of the PRIMARY session of the pair this service creates: "The
/// primary URL initiates a web-based Amazon DCV session to the virtual
/// computer's application" (the modelled `CreateGUISessionAccessDetails`
/// documentation), so the primary session is the APPLICATION one.
pub const GUI_SESSION_NAME_APPLICATION: &str = "application";

/// The name of the secondary session: "The secondary URL initiates a web-based
/// Amazon DCV session to the virtual computer's operating session" — the
/// operating system of the virtual computer, from the same documentation.
pub const GUI_SESSION_NAME_OPERATING_SYSTEM: &str = "operating-system";

/// The path space a virtual computer's GUI session URLs are published under,
/// beneath the endpoint the emulator is reached on.
///
/// `Session$url` is "The session URL", which AWS publishes in its own Amazon DCV
/// zone. This emulator owns no DNS zone, and a literal AWS hostname would send
/// the next client out of the emulator entirely (see [`request_endpoint`]), so
/// the address names the endpoint THIS request arrived at and carries the
/// session it addresses in the path — the same convention a container service's
/// public URL follows (see [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`]). The prefix
/// names this service and this resource, so it cannot collide with another
/// service's modelled URI space.
///
/// What the address identifies is real — the session this store created, under
/// the endpoint the caller reached — but the Amazon DCV session PROTOCOL is not
/// emulated: nothing here serves a remote desktop, so following the URL is not
/// answered by this service. The URL is the control-plane value the operation
/// models, not a promise of a GUI.
pub const GUI_SESSION_PATH_PREFIX: &str = "/lightsail-gui-session";

/// The URL of the `session_name` session of the virtual computer
/// `resource_name`, whose access details carry `token`, as announced to a caller
/// that reached this emulator at `endpoint` (its `Host`/`:authority`).
///
/// The token is in the address because the URL addresses ONE session of the
/// pair this call created, not the virtual computer: a URL created for an
/// earlier session is a different address.
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on.
pub fn gui_session_url(
    endpoint: &str,
    resource_name: &str,
    token: &str,
    session_name: &str,
) -> String {
    format!("https://{endpoint}{GUI_SESSION_PATH_PREFIX}/{resource_name}/{token}/{session_name}")
}

/// How far through the span `start_ms`..`end_ms` the instant `now_ms` is, as a
/// whole percentage in `0..=100`.
///
/// This is what a `percentageComplete` member reports: a span that has not begun
/// is 0, one that is over is 100, and a span of zero or negative length is
/// complete (there is nothing left to wait for).
pub fn elapsed_percentage(start_ms: i64, end_ms: i64, now_ms: i64) -> i32 {
    let span = end_ms.saturating_sub(start_ms);
    if span <= 0 {
        return 100;
    }
    let elapsed = now_ms.saturating_sub(start_ms).clamp(0, span);
    i32::try_from(elapsed.saturating_mul(100) / span).unwrap_or(100)
}

/// The termination of one GUI session a caller asked for: when the close was
/// asked for, and the instant the clock owes the session its `stopped` status.
///
/// A close is not instantaneous — `stopping` is a modelled `Status` member, so a
/// session on its way down is a state AWS reports — and it takes
/// [`STATE_TRANSITION_MILLIS`], the same span every other transition in this
/// store takes, so a caller that reads straight after the call still observes it.
#[derive(Debug, Clone, Copy)]
pub struct GuiSessionStop {
    /// The instant the close was asked for, which the progress of the close is
    /// measured from.
    pub requested_at_ms: i64,
    /// The instant the clock owes the session its `stopped` status.
    pub at_ms: i64,
}

impl GuiSessionStop {
    /// A close asked for at `now_ms`, due [`STATE_TRANSITION_MILLIS`] later.
    pub fn after(now_ms: i64) -> Self {
        Self {
            requested_at_ms: now_ms,
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
        }
    }

    /// Whether the session this close belongs to has reached `stopped` by
    /// `now_ms`.
    pub fn is_complete_at(&self, now_ms: i64) -> bool {
        self.at_ms <= now_ms
    }

    /// How far through this close the instant `now_ms` is, as a whole
    /// percentage — what `percentageComplete` reports while a session is on its
    /// way down, and 100 once it is down.
    pub fn percentage_complete_at(&self, now_ms: i64) -> i32 {
        elapsed_percentage(self.requested_at_ms, self.at_ms, now_ms)
    }
}

/// The GUI (Amazon DCV) session access this store has created for one virtual
/// computer: the token its URLs are addressed by, and the instants that decide
/// the session's status — when it finishes coming up, when its active window runs
/// out, and, once a caller has terminated it, when it finishes closing.
///
/// The URLs themselves are NOT stored: they are built per request from the
/// endpoint that request arrived at (see [`gui_session_url`]), so a caller that
/// reaches the emulator on a different address is told an address it can reach.
#[derive(Debug, Clone)]
pub struct StoredGuiSession {
    /// The token both of this session pair's URLs carry, minted by the store's
    /// own allocator ([`ComAmazonawsLightsailLightsail20161128State::mint_uuid`]).
    pub token: String,
    /// When the access details were created.
    pub created_at_ms: i64,
    /// The instant the clock owes the session its `started` status.
    pub starts_at_ms: i64,
    /// The close a caller asked for on this session, once one has been asked
    /// for: `None` for a session nothing has terminated.
    pub stop: Option<GuiSessionStop>,
}

impl StoredGuiSession {
    /// Access details minted at `now_ms` carrying `token`, for a session that is
    /// still coming up.
    ///
    /// The session is `starting` for [`STATE_TRANSITION_MILLIS`] — a non-zero
    /// span, so a caller that reads straight after the call still observes the
    /// transient status — and `started` after it.
    pub fn starting(token: String, now_ms: i64) -> Self {
        Self::starting_once_ready(token, now_ms, now_ms)
    }

    /// Access details minted at `now_ms` carrying `token`, for a session on a
    /// virtual computer that is itself serving from `ready_at_ms`.
    ///
    /// A session runs ON the computer, so it cannot be up before the computer
    /// is: the session comes up [`STATE_TRANSITION_MILLIS`] after the LATER of
    /// the two instants, which is `now_ms` for a computer that is already
    /// running and the instant the clock owes that computer its running state
    /// for one that is still coming up.
    pub fn starting_once_ready(token: String, now_ms: i64, ready_at_ms: i64) -> Self {
        Self {
            token,
            created_at_ms: now_ms,
            starts_at_ms: ready_at_ms
                .max(now_ms)
                .saturating_add(STATE_TRANSITION_MILLIS),
            stop: None,
        }
    }

    /// The instant this session's active window runs out: an hour after it
    /// started (see [`GUI_SESSION_ACTIVE_MILLIS`]).
    pub fn expires_at_ms(&self) -> i64 {
        self.starts_at_ms.saturating_add(GUI_SESSION_ACTIVE_MILLIS)
    }

    /// Whether this session is serving, or still on its way to serving, at
    /// `now_ms`.
    ///
    /// A session a caller has terminated is NOT live however much of its active
    /// window is left: the close is what ended it, so the next start resumes the
    /// computer's session as a new one rather than handing back this address.
    pub fn is_live_at(&self, now_ms: i64) -> bool {
        self.stop.is_none() && now_ms < self.expires_at_ms()
    }

    /// The modelled `Status` this session is in at `now_ms`: `starting` until it
    /// has come up, `started` for its active hour, and `startExpired` once that
    /// hour has run out — or, for a session a caller has terminated, `stopping`
    /// until the close is through and `stopped` after it, because the close is
    /// what the session's status is about from the instant it was asked for.
    pub fn status_at(&self, now_ms: i64) -> &'static str {
        if let Some(stop) = self.stop {
            if stop.is_complete_at(now_ms) {
                GUI_SESSION_STATUS_STOPPED
            } else {
                GUI_SESSION_STATUS_STOPPING
            }
        } else if now_ms < self.starts_at_ms {
            GUI_SESSION_STATUS_STARTING
        } else if now_ms < self.expires_at_ms() {
            GUI_SESSION_STATUS_STARTED
        } else {
            GUI_SESSION_STATUS_START_EXPIRED
        }
    }

    /// `percentageComplete` for this session at `now_ms`: how far it is through
    /// coming up, and 100 once it is up — the start it reports the completion of
    /// is over, whether the session is still serving or its hour has run out. For
    /// a session a caller has terminated it is how far that close has got, which
    /// is the work the call reporting it started.
    pub fn percentage_complete_at(&self, now_ms: i64) -> i32 {
        if let Some(stop) = self.stop {
            stop.percentage_complete_at(now_ms)
        } else if now_ms < self.starts_at_ms {
            elapsed_percentage(self.created_at_ms, self.starts_at_ms, now_ms)
        } else {
            100
        }
    }

    /// The pair of modelled `Session`s this access carries for the virtual
    /// computer `resource_name`: the primary (application) session first, then
    /// the secondary (operating system) one.
    ///
    /// A request that names no host at all names no endpoint to announce, so
    /// `url` is omitted rather than filled with an address nothing serves — the
    /// same rule a container service's `url` follows.
    pub fn views(&self, resource_name: &str, endpoint: Option<&str>) -> Vec<crate::ops::types::Session> {
        [
            (GUI_SESSION_NAME_APPLICATION, true),
            (GUI_SESSION_NAME_OPERATING_SYSTEM, false),
        ]
        .into_iter()
        .map(|(name, is_primary)| crate::ops::types::Session {
            name: Some(name.to_string()),
            url: endpoint
                .map(|endpoint| gui_session_url(endpoint, resource_name, &self.token, name)),
            is_primary: Some(is_primary),
        })
        .collect()
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
    ///
    /// `next_auto_snapshot_time_of_day` is when the next automatic copy of this
    /// disk is due. It lives on the SCHEDULE the disk's add-on runs on rather
    /// than on this record, so the store passes it in
    /// ([`ComAmazonawsLightsailLightsail20161128State::next_auto_snapshot_time_of_day`]).
    pub fn view(&self, next_auto_snapshot_time_of_day: Option<String>) -> crate::ops::types::Disk {
        let attached = self.attached_to.is_some();
        crate::ops::types::Disk {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_DISK.to_string()),
            tags: Some(self.tags.clone()),
            add_ons: Some(add_ons_view(&self.add_ons, next_auto_snapshot_time_of_day)),
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

    /// Record this disk as attached to the instance `instance_name` and exposed
    /// at the device `path`: the attachment and the path are what the modelled
    /// `Disk$attachedTo` and `$path` report, and the disk is [`DISK_STATE_IN_USE`]
    /// for as long as the attachment stands.
    ///
    /// A settle to [`DISK_STATE_AVAILABLE`] the clock still owed this disk is
    /// dropped here: settling there is what RELEASES an attachment (see
    /// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`]), so
    /// leaving it in place would un-attach the disk this very call attached. A
    /// settle to any OTHER state is left alone — it is not this attachment's.
    /// Every operation that attaches an EXISTING disk goes through here, so the
    /// invariant lives next to the members it protects rather than in each
    /// handler. (A disk a restore CREATES already attached is stored attached,
    /// owed nothing, by the insert that creates it.)
    pub fn attach_to(&mut self, instance_name: String, path: String) {
        self.attached_to = Some(instance_name);
        self.path = Some(path);
        self.state = DISK_STATE_IN_USE;
        if self.due.is_some_and(|due| due.state == DISK_STATE_AVAILABLE) {
            self.due = None;
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
    /// Whether this snapshot was created from an AUTOMATIC snapshot, which is
    /// what `DiskSnapshot$isFromAutoSnapshot` reports: false for a snapshot the
    /// caller asked the service to take, true for one copied from a copy the
    /// `AutoSnapshot` add-on took.
    pub is_from_auto_snapshot: bool,
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
            is_from_auto_snapshot: Some(self.is_from_auto_snapshot),
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
    /// Whether this snapshot was created from an AUTOMATIC snapshot, which is
    /// what `InstanceSnapshot$isFromAutoSnapshot` reports: false for a snapshot
    /// the caller asked the service to take, true for one copied from a copy the
    /// `AutoSnapshot` add-on took.
    pub is_from_auto_snapshot: bool,
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
                    // What this snapshot CAPTURED of each disk, as it was then.
                    // When a further automatic copy of one of those disks falls
                    // due is a live fact about the disk itself — the schedule its
                    // own add-on runs on — and not something a capture taken in
                    // the past can report, so the captured add-on carries no next
                    // daily time.
                    .map(|disk| disk.view(None))
                    .collect(),
            ),
            from_instance_name: Some(self.from_instance_name.clone()),
            from_instance_arn: Some(self.from_instance_arn.clone()),
            from_blueprint_id: Some(self.from_blueprint_id.clone()),
            from_bundle_id: Some(self.from_bundle_id.clone()),
            is_from_auto_snapshot: Some(self.is_from_auto_snapshot),
            size_in_gb: Some(self.size_in_gb),
        }
    }
}

// ---------------------------------------------------------------------------
// Automatic snapshots
//
// An AUTOMATIC snapshot is a copy the `AutoSnapshot` add-on takes of the
// resource it is enabled for, once a day at the hour that add-on carries. It is
// not a resource of its own: it has no name, no ARN and no tags, it is addressed
// by the resource it was taken of plus the DATE it was taken on
// (`GetAutoSnapshots`, `DeleteAutoSnapshot$date`), and nothing but the clock
// creates one. So the copies are held beside the schedule that takes them,
// keyed by the resource, rather than in a map of named records like the
// snapshots a caller asks for.
// ---------------------------------------------------------------------------

/// One automatic snapshot the `AutoSnapshot` add-on took.
#[derive(Debug, Clone)]
pub struct StoredAutoSnapshot {
    /// The UTC date the copy was taken on, in the `YYYY-MM-DD` form
    /// `AutoSnapshotDetails$date` reports and `DeleteAutoSnapshot$date`
    /// addresses it by. One copy a day, so this identifies it among the
    /// resource's copies.
    pub date: String,
    /// The instant the service started taking the copy, which
    /// `AutoSnapshotDetails$createdAt` reports.
    pub created_at_ms: i64,
    /// The current `AutoSnapshotStatus` value:
    /// [`AUTO_SNAPSHOT_STATUS_IN_PROGRESS`] or
    /// [`AUTO_SNAPSHOT_STATUS_SUCCESS`].
    pub status: &'static str,
    /// The status the clock owes this copy, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The block storage disks attached to the INSTANCE this copy was taken of
    /// at the instant it was taken, captured whole — `CopySnapshot` turns one of
    /// these copies into a manual instance snapshot, which describes the disks it
    /// captured as the resources they are. `AutoSnapshotDetails$fromAttachedDisks`
    /// reports them as "the block storage disks attached to the instance when the
    /// automatic snapshot was created": an empty list for an instance that had
    /// none, and `None` for a copy of a DISK, which is one volume and has no
    /// disks attached to it.
    pub from_attached_disks: Option<Vec<StoredDisk>>,
}

impl StoredAutoSnapshot {
    /// This record as the modelled `AutoSnapshotDetails`.
    pub fn view(&self) -> AutoSnapshotDetails {
        AutoSnapshotDetails {
            date: Some(self.date.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            status: Some(self.status.to_string()),
            // What the modelled `AttachedDisk` carries of each captured disk:
            // the device path it was mounted at and its size.
            from_attached_disks: self.from_attached_disks.as_ref().map(|disks| {
                disks
                    .iter()
                    .map(|disk| AttachedDisk {
                        path: disk.path.clone(),
                        size_in_gb: Some(disk.size_in_gb),
                    })
                    .collect()
            }),
        }
    }
}

/// What a delete recorded about the resource a set of automatic snapshots was
/// taken of, once that resource's own record is gone.
///
/// A delete that is not asked to delete the resource's add-ons leaves the copies
/// the add-on took in the account, and the record that said where that resource
/// was is no longer there to ask — so what a later call still needs of it is
/// recorded here.
#[derive(Debug, Clone)]
pub struct KeptAutoSnapshotSource {
    /// The instant the resource's record was removed, which is what orders one
    /// account's kept copies against [`MAX_KEPT_AUTO_SNAPSHOT_SOURCES`].
    pub removed_at_ms: i64,
    /// Where that resource was: what an operation logged against one of these
    /// copies is located at, exactly as it was while the resource was held.
    pub location: StoredLocation,
}

/// What the `AutoSnapshot` add-on of one resource has taken, and when it next
/// takes a copy.
#[derive(Debug, Default)]
pub struct StoredAutoSnapshots {
    /// The instant the next automatic snapshot of the resource is due, while
    /// the add-on is enabled on it. `None` once the add-on is disabled: the
    /// copies already taken stay, and no further one is owed.
    pub due_at_ms: Option<i64>,
    /// The copies taken so far, oldest first — the order they were taken, and
    /// the order `GetAutoSnapshots` reports them in. At most
    /// [`MAX_AUTO_SNAPSHOTS_PER_RESOURCE`] of them.
    pub taken: Vec<StoredAutoSnapshot>,
    /// Set once the resource these copies were taken of has been deleted and the
    /// copies were KEPT, which is what a delete that was not asked to delete the
    /// resource's add-ons does. `None` for as long as this account holds that
    /// resource, whose own record is then what says where it is.
    pub kept_source: Option<KeptAutoSnapshotSource>,
}

impl StoredAutoSnapshots {
    /// Record the copy the schedule owed at `at_ms`, capturing
    /// `from_attached_disks`.
    ///
    /// The copy is taken `InProgress` and the CLOCK settles it into `Success`:
    /// the modelled `AddOn$snapshotTimeOfDay` documentation says the snapshot
    /// "is automatically created between the time shown and up to 45 minutes
    /// after", so there is an interval during which the service holds a copy it
    /// has not finished taking.
    ///
    /// One copy a day: an automatic snapshot is addressed by its date, so a date
    /// this resource already holds a copy for is not copied a second time.
    fn take(&mut self, at_ms: i64, from_attached_disks: Option<Vec<StoredDisk>>) {
        let date = utc_date(at_ms);
        if self.taken.iter().any(|taken| taken.date == date) {
            return;
        }
        self.taken.push(StoredAutoSnapshot {
            date,
            created_at_ms: at_ms,
            status: AUTO_SNAPSHOT_STATUS_IN_PROGRESS,
            due: Some(DueNamedState::after(at_ms, AUTO_SNAPSHOT_STATUS_SUCCESS)),
            from_attached_disks,
        });
        // The oldest copy drops out once a new one would take the resource past
        // what this store holds of it.
        while self.taken.len() > MAX_AUTO_SNAPSHOTS_PER_RESOURCE {
            self.taken.remove(0);
        }
    }

    /// Every copy as the modelled `AutoSnapshotDetails`, oldest first.
    fn view(&self) -> Vec<AutoSnapshotDetails> {
        self.taken.iter().map(StoredAutoSnapshot::view).collect()
    }
}

/// What a request to delete ONE automatic snapshot of a resource found.
///
/// A deletion addresses a copy by the resource it was taken of plus the date it
/// was taken on, so there are three outcomes and the caller answers each
/// differently: no such resource (the absent value of the
/// [`Option`](ComAmazonawsLightsailLightsail20161128State::delete_auto_snapshot)
/// this is wrapped in), a resource holding no copy for that date, and the copy
/// removed.
#[derive(Debug)]
pub enum AutoSnapshotDeletion {
    /// The copy was removed. Carries what an operation-log entry for the
    /// deletion needs: the kind of resource the copy was taken of, and where
    /// that resource is.
    Deleted {
        /// The `ResourceType` the resource the copy was taken of reports.
        resource_type: &'static str,
        /// Where that resource is.
        location: StoredLocation,
    },
    /// The resource exists and holds no copy taken on the requested date.
    NoCopyForDate {
        /// The `ResourceType` that resource reports, for the refusal's message.
        resource_type: &'static str,
    },
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
    /// Which KIND of snapshot this export was taken of, as the modelled
    /// `ExportSnapshotRecordSourceType` names it: [`RESOURCE_TYPE_INSTANCE_SNAPSHOT`]
    /// or [`RESOURCE_TYPE_DISK_SNAPSHOT`].
    ///
    /// The distinction is what the export produced in Amazon EC2 — a machine
    /// image for an instance snapshot, a volume for a disk snapshot (see
    /// [`EC2_IMAGE_ID_PREFIX`] and [`EC2_VOLUME_ID_PREFIX`]) — so an operation
    /// that consumes the exported resource asks the record for it rather than
    /// re-deriving it.
    pub fn exported_snapshot_type(&self) -> &str {
        self.source_info.resource_type.as_deref().unwrap_or_default()
    }

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

// ---------------------------------------------------------------------------
// CloudFormation stack records
//
// Everything `CreateCloudFormationStack` and `GetCloudFormationStackRecords`
// need that is not a field of the store itself lives in this one section — the
// resource's identifiers, bounds, page size and stored record — rather than
// beside each of its neighbours' own constants. The record's clock-owed
// `Started` -> `Succeeded` settle is applied by
// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`], bounded by
// [`MAX_CLOUD_FORMATION_STACK_RECORDS`], like every other resource's.
// ---------------------------------------------------------------------------

/// The resource portion a CloudFormation stack record ARN carries before its id.
/// Mirrors the `Instance/<uuid>` form the modelled `Instance$arn` documentation
/// shows, with this resource's own `ResourceType` value.
pub const CLOUD_FORMATION_STACK_RECORD_ARN_RESOURCE_PREFIX: &str = "CloudFormationStackRecord/";

/// The `resourceType` a CloudFormation stack record reports, from the modelled
/// `ResourceType` enum — the value the modelled
/// `CloudFormationStackRecord$resourceType` documentation names ("The Lightsail
/// resource type (`CloudFormationStackRecord`)").
pub const RESOURCE_TYPE_CLOUD_FORMATION_STACK_RECORD: &str = "CloudFormationStackRecord";

/// The `operationType` the log entry of a `CreateCloudFormationStack` call
/// carries: the name of the call, under the one rule
/// [`OPERATION_TYPE_COPY_SNAPSHOT`] states for every call the modelled
/// `OperationType` enum declares no member for.
pub const OPERATION_TYPE_CREATE_CLOUD_FORMATION_STACK: &str = "CreateCloudFormationStack";

/// The most CloudFormation stack records this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: every `CreateCloudFormationStack`
/// call adds a record, so a call that would exceed it is refused with the
/// modelled `InvalidInputException`.
pub const MAX_CLOUD_FORMATION_STACK_RECORDS: usize = 500;

/// How many CloudFormation stack records one `GetCloudFormationStackRecords`
/// page carries.
///
/// CHOSEN, for the same reason as [`EXPORT_SNAPSHOT_RECORD_PAGE_SIZE`]: Lightsail
/// publishes no page size for this operation either, only that the response
/// carries a `nextPageToken` when more records remain.
pub const CLOUD_FORMATION_STACK_RECORD_PAGE_SIZE: usize = 10;

/// The prefix a minted CloudFormation stack record name carries, from the
/// modelled `CloudFormationStackRecord$name` documentation: "It starts with
/// `CloudFormationStackRecord` followed by a GUID." The separating hyphen keeps
/// the name inside the modelled `ResourceName` pattern `^\w[\w\-]*\w$`.
pub const CLOUD_FORMATION_STACK_RECORD_NAME_PREFIX: &str = "CloudFormationStackRecord-";

/// The destination service a CloudFormation stack record reports, as
/// `DestinationInfo$service`.
///
/// `CloudFormationStackRecord$destinationInfo` describes "the destination
/// service, which is AWS CloudFormation, and the Amazon Resource Name (ARN) of
/// the AWS CloudFormation stack", so the one value this member can carry is that
/// service, named by its own ARN namespace and endpoint prefix
/// (`cloudformation`) — the identifier this emulator names a service with
/// everywhere else.
pub const CLOUD_FORMATION_DESTINATION_SERVICE: &str = "cloudformation";

/// The prefix the name of the AWS CloudFormation stack this operation creates
/// carries.
///
/// CHOSEN: Lightsail does not document how it names that stack, only that the
/// record reports the stack's ARN — and a stack ARN carries the stack's name. So
/// the name is minted here: this prefix, which says which service created the
/// stack, followed by the record's own id, so one value identifies the stack and
/// the record that tracks it. A letter followed by letters, digits and hyphens
/// is also a legal CloudFormation stack name.
pub const CLOUD_FORMATION_STACK_NAME_PREFIX: &str = "Lightsail-";

/// The most bytes of `userData` one instance entry may carry.
///
/// `InstanceEntry$userData` is "a launch script you can create that configures a
/// server with additional user data" for the new Amazon EC2 instance, and Amazon
/// EC2 caps the user data of an instance at 16 KB in raw form (Amazon EC2 User
/// Guide, "Run commands on your Linux instance at launch": "the user data must
/// be no larger than 16 KB").
pub const MAX_USER_DATA_BYTES: usize = 16 * 1024;

/// The ARN of the CloudFormation stack record `id` in `region`.
pub fn cloud_formation_stack_record_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CLOUD_FORMATION_STACK_RECORD_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The name of the AWS CloudFormation stack the record `id` names tracks. See
/// [`CLOUD_FORMATION_STACK_NAME_PREFIX`].
pub fn cloud_formation_stack_name(id: &str) -> String {
    format!("{CLOUD_FORMATION_STACK_NAME_PREFIX}{id}")
}

/// The ARN of the AWS CloudFormation stack `stack_name`, whose own id is
/// `stack_id`, in `region`.
///
/// A CloudFormation stack ARN carries the stack's name AND the id CloudFormation
/// minted for it (`stack/<name>/<id>`), under that service's own ARN namespace —
/// the stack is CloudFormation's resource, not Lightsail's.
pub fn cloud_formation_stack_arn(region: &str, stack_name: &str, stack_id: &str) -> String {
    ::engine::emulator_arn(
        CLOUD_FORMATION_DESTINATION_SERVICE,
        region,
        &format!("stack/{stack_name}/{stack_id}"),
    )
}

/// Whether `value` is an Amazon EC2 instance type.
///
/// `InstanceEntry$instanceType` is "the instance type (`t2.micro`) to use for
/// the new Amazon EC2 instance" and targets the modelled `NonEmptyString`, whose
/// only constraint is the `\S` pattern — so the form is Amazon EC2's own: an
/// instance type name is a family and a size separated by a single dot. The
/// family is a non-empty run of lowercase ASCII letters, digits and hyphens that
/// neither begins nor ends with a hyphen, because EC2 families carry hyphens
/// (`mac2-m2pro.metal`, `c7i-flex.large`, `u7i-12tb.224xlarge` — see the sibling
/// Amazon EC2 emulator's `instance_family_of`); the size is a non-empty run of
/// lowercase ASCII letters and digits (`micro`, `2xlarge`, `metal`).
///
/// A FORM check, deliberately: Lightsail models this member as a free-form
/// `NonEmptyString`, not as Amazon EC2's `InstanceType` enum, and the catalog of
/// types a Region offers is Amazon EC2's state rather than this service's — so a
/// well-formed name that no Region offers is accepted here rather than pulling
/// the whole sibling Amazon EC2 service crate (whose `instance_type_is_modelled`
/// carries that catalog) into this one to check one string. What this refuses is
/// a value that is not an instance type NAME at all.
pub fn is_ec2_instance_type(value: &str) -> bool {
    let Some((family, size)) = value.split_once('.') else {
        return false;
    };
    let alphanumeric = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    };
    // `split_once` splits at the FIRST dot, so a value carrying a second one
    // leaves it in `size`, which `alphanumeric` refuses.
    alphanumeric(size)
        && !family.starts_with('-')
        && !family.ends_with('-')
        && family.split('-').all(alphanumeric)
}

/// One stored CloudFormation stack record: the record `CreateCloudFormationStack`
/// created, with the clock-owed transition its state is waiting on.
///
/// Every `CreateCloudFormationStack` call appends ONE record — building a second
/// Amazon EC2 instance from the same export is a second stack, and so a second
/// record — which is why these are keyed by a minted name rather than by the
/// export snapshot record's.
#[derive(Debug, Clone)]
pub struct StoredCloudFormationStackRecord {
    /// The record name this store minted — this record's store key and the
    /// `name` every read reports. See
    /// [`CLOUD_FORMATION_STACK_RECORD_NAME_PREFIX`].
    pub name: String,
    pub arn: String,
    pub created_at_ms: i64,
    /// Where the record is: the Availability Zone the instance entry asked the
    /// new Amazon EC2 instance to be created in, and that zone's region.
    pub location: StoredLocation,
    /// The current `RecordState` value: [`RECORD_STATE_STARTED`] or
    /// [`RECORD_STATE_SUCCEEDED`].
    pub state: &'static str,
    /// The state the clock owes this record, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The export snapshot record this stack was built from, captured at the
    /// instant of the call — so the record keeps describing it even if that
    /// record itself later changes.
    pub source_info: crate::ops::types::CloudFormationStackRecordSourceInfo,
    /// The ARN of the AWS CloudFormation stack this record tracks, which is what
    /// `destinationInfo` reports. See [`cloud_formation_stack_arn`].
    pub stack_arn: String,
    /// The Amazon EC2 instance type the stack was told to create, kept verbatim.
    /// No modelled read reports it — a stack record describes the stack, not its
    /// template — so it is held here as what the stack was created from.
    pub instance_type: String,
    /// The `PortInfoSourceType` the stack was told to configure the new
    /// instance's firewall from, kept verbatim. Not reported, for the same
    /// reason as [`Self::instance_type`].
    pub port_info_source: String,
    /// The launch script the stack was told to configure the new instance with,
    /// kept verbatim; absent when the entry carried none. Not reported, for the
    /// same reason as [`Self::instance_type`].
    pub user_data: Option<String>,
}

impl StoredCloudFormationStackRecord {
    /// This record as the modelled `CloudFormationStackRecord`.
    pub fn view(&self) -> crate::ops::types::CloudFormationStackRecord {
        crate::ops::types::CloudFormationStackRecord {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_CLOUD_FORMATION_STACK_RECORD.to_string()),
            state: Some(self.state.to_string()),
            // `sourceInfo` is "a list of objects describing the source of the
            // CloudFormation stack record", and one call builds its stack from
            // exactly one export snapshot record — so one entry.
            source_info: Some(vec![self.source_info.clone()]),
            // Unlike an export's destination — an Amazon EC2 resource that does
            // not exist until the export succeeds — the stack this record
            // tracks is created by the call that appended the record, and
            // `destinationInfo` is "the destination service ... and the ARN of
            // the AWS CloudFormation stack". So it is reported from the start;
            // the record's `state` is what says whether that stack has finished
            // creating the instance.
            destination_info: Some(DestinationInfo {
                id: Some(self.stack_arn.clone()),
                service: Some(CLOUD_FORMATION_DESTINATION_SERVICE.to_string()),
            }),
        }
    }

    /// The export snapshot record this stack was built from, as the record's own
    /// source description names it.
    pub fn source_export_snapshot_record_name(&self) -> Option<&str> {
        self.source_info.name.as_deref()
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
// SSH key pairs
// ---------------------------------------------------------------------------

/// One stored SSH key pair: the PUBLIC half Lightsail holds for the account,
/// under the name an instance's `sshKeyName` names it by.
///
/// Lightsail holds only the public half — the modelled `DeleteKeyPair`
/// documentation deletes a key pair "by removing the public key from Amazon
/// Lightsail" — so that is all a record here holds beside what the service
/// assigned it. The private half is handed to the caller by the call that
/// created the pair and is never stored.
#[derive(Debug, Clone)]
pub struct StoredKeyPair {
    /// The key pair name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The fingerprint of the public half: "The RSA fingerprint of the key pair"
    /// (`KeyPair$fingerprint`). It is what a delete's `expectedFingerprint`
    /// states it expects to be deleting.
    pub fingerprint: String,
}

impl StoredKeyPair {
    /// This record as the modelled `KeyPair`.
    pub fn view(&self) -> KeyPair {
        KeyPair {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_KEY_PAIR.to_string()),
            tags: Some(self.tags.clone()),
            fingerprint: Some(self.fingerprint.clone()),
        }
    }

    /// Whether this is the account's Lightsail DEFAULT key pair — the one
    /// [`DEFAULT_SSH_KEY_NAME`] names, which an instance created without a key
    /// pair of its own is launched with.
    ///
    /// The record's NAME is what decides, because that name is how every
    /// Lightsail operation addresses the default pair.
    pub fn is_default(&self) -> bool {
        self.name == DEFAULT_SSH_KEY_NAME
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

/// Whether `engine` is the PostgreSQL engine FAMILY.
///
/// The ONE engine classification every per-engine helper here shares (the port,
/// the parameter catalog, the log streams and the master-credential limits), so
/// they cannot drift apart.
///
/// It answers true when any SEPARATOR-DELIMITED token of the value is
/// `postgres` or `postgresql`, case-insensitively — which covers the catalog's
/// own [`RelationalDatabaseBlueprintSpec::engine`] value (`postgres`) and a
/// blueprint id that carries the version beside it (`postgres_17`), the two
/// forms this crate passes. An unseparated spelling (`postgres17`) is NOT
/// PostgreSQL to this predicate; everything that is not PostgreSQL is MySQL, the
/// engine the modelled `RelationalDatabaseEngine` enum names.
pub fn is_postgres_engine(engine: &str) -> bool {
    engine
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            token.eq_ignore_ascii_case("postgres") || token.eq_ignore_ascii_case("postgresql")
        })
}

/// The port a database of `engine` listens on: the engine's own default
/// (MySQL `3306`, PostgreSQL `5432`), the ports RDS documents as the default for
/// each engine under `CreateDBInstance$Port`.
///
/// Lightsail exposes no port parameter at all, so a database always listens on
/// its engine's default.
pub fn relational_database_port(engine: &str) -> i32 {
    if is_postgres_engine(engine) {
        5432
    } else {
        3306
    }
}

/// The log streams a MySQL managed database publishes, in the order
/// [`relational_database_log_streams`] reports them.
///
/// A Lightsail managed database is an Amazon RDS DB instance, and a log stream is
/// one of the log FILES that instance's engine writes: the RDS MySQL log-file
/// names (Amazon RDS User Guide, "MySQL database log files" — the error log
/// `error/mysql-error.log` with its hourly-rotated running copy
/// `error/mysql-error-running.log`, the general query log
/// `general/mysql-general.log` and the slow query log
/// `slowquery/mysql-slowquery.log`).
///
/// CHOSEN where AWS publishes no list: Lightsail's own API documents only that
/// `GetRelationalDatabaseLogStreams` answers "a list of available log streams"
/// and names none of them, so this is the engine's documented log-file set. The
/// hourly `error/mysql-error-running.log.<hour>` copies and the audit log are
/// left out: the first is a rotation artefact of a server that has been running
/// for hours, and the second needs an option group Lightsail does not offer.
pub const MYSQL_LOG_STREAMS: &[&str] = &[
    "error/mysql-error-running.log",
    "error/mysql-error.log",
    "general/mysql-general.log",
    "slowquery/mysql-slowquery.log",
];

/// The log streams a PostgreSQL managed database publishes. See
/// [`MYSQL_LOG_STREAMS`] for where the names come from: PostgreSQL writes ONE
/// log, `error/postgresql.log` (Amazon RDS User Guide, "PostgreSQL database log
/// files"), whose hourly-stamped copies are left out for the same reason the
/// MySQL rotations are — and because a stream name has to be stable across
/// reads.
pub const POSTGRES_LOG_STREAMS: &[&str] = &["error/postgresql.log"];

/// What the name of an ERROR log stream begins with, for both engines this
/// slice serves.
///
/// A database's log files fall in the groups the engine writes them in, and the
/// group is the first path segment of the stream name: the error log the engine
/// writes its own startup and shutdown to is under `error/` for both engines
/// ([`MYSQL_LOG_STREAMS`], [`POSTGRES_LOG_STREAMS`]), while a stream under
/// `general/` or `slowquery/` is a QUERY log — a record of the statements
/// clients ran, not of the engine itself.
pub const RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX: &str = "error/";

/// The log streams a database of `engine` publishes.
///
/// The set is the ENGINE's, like the port and the master-credential limits, so a
/// MySQL database and a PostgreSQL one answer different streams. The engine of a
/// blueprint is the catalog's own [`RelationalDatabaseBlueprintSpec::engine`]
/// value, so the PostgreSQL set is selected on that engine and every other engine
/// the catalog serves answers the MySQL set.
pub fn relational_database_log_streams(engine: &str) -> &'static [&'static str] {
    if is_postgres_engine(engine) {
        POSTGRES_LOG_STREAMS
    } else {
        MYSQL_LOG_STREAMS
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
    if is_postgres_engine(engine) {
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

/// The three versions of a managed relational database's master user password
/// `GetRelationalDatabaseMasterUserPassword$passwordVersion` can name, from the
/// modelled `RelationalDatabasePasswordVersion` enum.
///
/// `CURRENT` is the password the master user is reached with now, `PREVIOUS` the
/// one it replaced, and `PENDING` the one a modification will rotate to
/// `CURRENT`.
pub const RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT: &str = "CURRENT";
/// See [`RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT`].
pub const RELATIONAL_DATABASE_PASSWORD_VERSION_PREVIOUS: &str = "PREVIOUS";
/// See [`RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT`].
pub const RELATIONAL_DATABASE_PASSWORD_VERSION_PENDING: &str = "PENDING";

/// The password version a request that names none asks for: "Default: CURRENT"
/// (the modelled `passwordVersion` documentation).
pub const DEFAULT_RELATIONAL_DATABASE_PASSWORD_VERSION: &str =
    RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT;

/// ONE version of a managed relational database's master user password: the
/// secret itself and the instant that version was created.
///
/// A version is created by the call that sets the password — the create or the
/// `UpdateRelationalDatabase` that named or minted it — and keeps that instant
/// for as long as it is held, so the instant
/// `GetRelationalDatabaseMasterUserPassword$createdAt` reports for a version
/// does not move when the version rotates from `PENDING` to `CURRENT` or from
/// `CURRENT` to `PREVIOUS`.
#[derive(Clone)]
pub struct StoredMasterUserPassword {
    /// The secret. Reported by `GetRelationalDatabaseMasterUserPassword` alone;
    /// every other read of a database masks or omits it.
    pub password: String,
    /// Epoch milliseconds at which this version was created.
    pub created_at_ms: i64,
}

impl StoredMasterUserPassword {
    /// The version `password`, created at `created_at_ms`.
    pub fn new(password: impl Into<String>, created_at_ms: i64) -> Self {
        Self {
            password: password.into(),
            created_at_ms,
        }
    }
}

/// The stored database this version hangs off derives `Debug`, so a formatter
/// reaches every field of it; this one writes the instant and withholds the
/// secret, which is handed out by the one operation that answers with it and
/// nowhere else.
impl std::fmt::Debug for StoredMasterUserPassword {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredMasterUserPassword")
            .field("created_at_ms", &self.created_at_ms)
            .finish_non_exhaustive()
    }
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
    /// The password version the master user gets, whether the caller supplied
    /// the secret or asked the service to mint one. Created at the instant of
    /// the call that asked for it, which is the instant it keeps once it reaches
    /// the database.
    pub master_user_password: Option<StoredMasterUserPassword>,
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

/// One entry of a managed relational database's event log: what the service did
/// to the database, when it did it, and the categories that change falls in —
/// the modelled `RelationalDatabaseEvent` as this store holds it.
///
/// The log is the DATABASE's own: it is appended to by the service's own changes
/// to that database and by nothing else, so a read of it reports what happened
/// rather than anything the reading request carried.
#[derive(Debug, Clone)]
pub struct StoredRelationalDatabaseEvent {
    /// This entry's position in its database's log, drawn from the entry before
    /// it so it is never reused: the `pageToken`
    /// `GetRelationalDatabaseEvents` names a page by, which an entry needs
    /// because nothing else about an event identifies it — two changes can carry
    /// the same message at the same instant.
    pub sequence: u64,
    /// Epoch milliseconds at which the change happened.
    pub created_at_ms: i64,
    /// What happened, from this module's `RELATIONAL_DATABASE_EVENT_*` wording.
    pub message: String,
    /// The categories the change falls in, from the
    /// `RELATIONAL_DATABASE_EVENT_CATEGORY_*` values.
    pub categories: Vec<&'static str>,
}

impl StoredRelationalDatabaseEvent {
    /// This entry as the modelled `RelationalDatabaseEvent` of the database
    /// `resource` names — "the database that the database event relates to".
    pub fn view(&self, resource: &str) -> crate::ops::types::RelationalDatabaseEvent {
        crate::ops::types::RelationalDatabaseEvent {
            resource: Some(resource.to_string()),
            created_at: Some(timestamp(self.created_at_ms)),
            message: Some(self.message.clone()),
            // A list member is reported as a list, so a typed caller iterating
            // the categories of an event is handed one.
            event_categories: Some(
                self.categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect(),
            ),
        }
    }

    /// The page token that names THIS entry.
    pub fn page_token(&self) -> String {
        self.sequence.to_string()
    }

    /// This entry as one LINE of the database's log file — the modelled
    /// `LogEvent` `GetRelationalDatabaseLogEvents` reports: the instant the line
    /// was written and what it says.
    ///
    /// The same happening the event log reports, seen through the other API:
    /// `GetRelationalDatabaseEvents` reports it as the service's event (with the
    /// database it relates to and the categories it falls in), and a log stream
    /// reports it as the line the engine wrote, which carries neither.
    pub fn log_event_view(&self) -> crate::ops::types::LogEvent {
        crate::ops::types::LogEvent {
            created_at: Some(timestamp(self.created_at_ms)),
            message: Some(self.message.clone()),
        }
    }
}

/// Which end of a log stream a `GetRelationalDatabaseLogEvents` page is read
/// from, and the position in that stream it is read at.
///
/// A position is a line's [`StoredRelationalDatabaseEvent::sequence`], so a
/// token NAMES a place in the log rather than carrying an index into one answer:
/// a stream that has been written to since the token was issued still answers
/// the lines after the one the token stopped at, and a line retention has
/// dropped cannot make the walk skip or repeat another.
///
/// Rendered as the direction (`f` forward, `b` backward) and that position —
/// the two tokens this operation reports are values a caller only ever hands
/// back, so the rendering is this store's own and is parsed back by
/// [`Self::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogEventPageToken {
    /// Read the line AT this position and the ones after it (forward), or the
    /// ones BEFORE it (backward).
    pub forward: bool,
    /// The position in the stream the next read starts (or ends) at.
    pub position: u64,
}

impl LogEventPageToken {
    /// The forward token at `position`.
    pub fn forward(position: u64) -> Self {
        Self {
            forward: true,
            position,
        }
    }

    /// The backward token at `position`.
    pub fn backward(position: u64) -> Self {
        Self {
            forward: false,
            position,
        }
    }

    /// `token` as this store issues one, or `None` when it is not one at all —
    /// which the caller turns into a refusal rather than quietly reading from
    /// the head of the log.
    ///
    /// TOTAL on every input: the value is split on the one separator and the
    /// position is parsed, so an arbitrary caller-supplied string answers
    /// `None` instead of panicking.
    pub fn parse(token: &str) -> Option<Self> {
        let (direction, position) = token.split_once('/')?;
        let forward = match direction {
            "f" => true,
            "b" => false,
            _ => return None,
        };
        Some(Self {
            forward,
            position: position.parse().ok()?,
        })
    }
}

impl std::fmt::Display for LogEventPageToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let direction = if self.forward { 'f' } else { 'b' };
        write!(formatter, "{direction}/{}", self.position)
    }
}

/// One page of a log stream: the lines it reported, and the two tokens that read
/// the page after it and the page before it.
///
/// Both tokens are always present, whether the page carried a line or not: a
/// stream is appended to while a caller reads it, so a caller that has reached
/// the end of one still needs the token that picks up the lines written next.
#[derive(Debug, Clone)]
pub struct LogEventPage {
    /// The lines of the page, in the order they were written.
    pub events: Vec<crate::ops::types::LogEvent>,
    /// `nextForwardToken`: the lines after this page.
    pub next_forward_token: LogEventPageToken,
    /// `nextBackwardToken`: the lines before this page.
    pub next_backward_token: LogEventPageToken,
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
    /// The CURRENT version of the master user password: the secret the request
    /// carried, the one the service generated for a request that named none, or
    /// the one a later `UpdateRelationalDatabase` set. Kept because
    /// `GetRelationalDatabaseMasterUserPassword` is what answers with it; no
    /// member of `RelationalDatabase` exposes it. Every database carries one —
    /// it is the credential its engine authenticates the master user by.
    pub master_user_password: StoredMasterUserPassword,
    /// The PREVIOUS version of the master user password: the version the CURRENT
    /// one replaced, which `GetRelationalDatabaseMasterUserPassword` answers for
    /// `passwordVersion` `PREVIOUS`. Absent until a password change has replaced
    /// one.
    pub previous_master_user_password: Option<StoredMasterUserPassword>,
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
    /// The modification an `UpdateRelationalDatabase` call has left for this
    /// database's next maintenance window, while one is outstanding — what
    /// `pendingModifiedValues` reports until the clock applies it.
    pub pending_modification: Option<PendingRelationalDatabaseModification>,
    /// This database's event log, oldest first — the source of truth for what
    /// `GetRelationalDatabaseEvents` reports.
    ///
    /// One entry per change the SERVICE made to this database: its engine going
    /// down and coming back up, a snapshot of it being taken, a modification or
    /// a parameter value reaching it. Appended through
    /// [`StoredRelationalDatabase::record_event`], which is also what keeps the
    /// log bounded.
    pub events: Vec<StoredRelationalDatabaseEvent>,
    /// The token the next entry of this database's log is named by, counted on
    /// from the last one minted rather than from the entries still held, so a
    /// token is never reused after retention has dropped the entry it named.
    pub next_event_sequence: u64,
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
    ///
    /// The modification reaches the parameter group when the call is answered,
    /// so that is a change to the database and its log carries one entry for it.
    ///
    /// ONE rule for the whole log: an entry says something about this database
    /// CHANGED, so the apply is logged only where it moves the parameter group —
    /// a value, or the apply method it was set with, that differs from the one
    /// this database already carried. Re-sending the value a parameter already
    /// holds changes nothing and logs nothing, exactly as re-sending a
    /// modification does ([`Self::apply_modification`]).
    pub fn set_parameter(&mut self, update: RelationalDatabaseParameterUpdate, now_ms: i64) {
        let pending_reboot = update.apply_method == PARAMETER_APPLY_METHOD_PENDING_REBOOT;
        let parameter_name = update.spec.parameter_name.to_string();
        let moved = self
            .parameter_overrides
            .get(&parameter_name)
            .is_none_or(|held| {
                held.value != update.value || held.apply_method != update.apply_method
            });
        self.parameter_overrides.insert(
            parameter_name.clone(),
            StoredParameterOverride {
                value: update.value,
                apply_method: update.apply_method,
                pending_reboot,
            },
        );
        if moved {
            self.record_event(
                now_ms,
                format!("{RELATIONAL_DATABASE_EVENT_PARAMETER_GROUP_CHANGED}: {parameter_name}"),
                &[RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
            );
        }
    }

    /// Append one entry to this database's event log: `message` happened to it
    /// at `at_ms`, in `categories`.
    ///
    /// The ONE place an event is logged, so every change the service makes to a
    /// database is recorded the same way, and the log is bounded here rather
    /// than by whoever logs: an entry older than the longest window a caller may
    /// ask for ([`RELATIONAL_DATABASE_EVENT_RETENTION_MILLIS`]) can never be
    /// reported again, and the count is capped at
    /// [`MAX_RELATIONAL_DATABASE_EVENTS`] for the same reason the operation log
    /// is capped.
    pub fn record_event(
        &mut self,
        at_ms: i64,
        message: impl Into<String>,
        categories: &[&'static str],
    ) {
        // The token is drawn from a counter of its own rather than from the
        // entry before it, so a number is never handed out twice however many
        // entries retention has dropped: a token a client is still holding can
        // then only name the entry it was reported for.
        let sequence = self.next_event_sequence;
        self.next_event_sequence = self.next_event_sequence.wrapping_add(1);
        let event = StoredRelationalDatabaseEvent {
            sequence,
            created_at_ms: at_ms,
            message: message.into(),
            categories: categories.to_vec(),
        };
        // The log is held OLDEST FIRST, which is the order it is reported in, so
        // an entry is placed by its instant rather than appended: two changes
        // that fall due in the same pass of the clock are not necessarily made
        // due in the order the pass walks them. An entry at the same instant as
        // one already held goes after it, so changes that share an instant keep
        // the order they happened in.
        let at = self
            .events
            .iter()
            .rposition(|held| held.created_at_ms <= at_ms)
            .map_or(0, |position| position.saturating_add(1));
        self.events.insert(at, event);
        let oldest_reportable = at_ms.saturating_sub(RELATIONAL_DATABASE_EVENT_RETENTION_MILLIS);
        self.events
            .retain(|event| event.created_at_ms >= oldest_reportable);
        if let Some(excess) = self.events.len().checked_sub(MAX_RELATIONAL_DATABASE_EVENTS) {
            self.events.drain(..excess);
        }
    }

    /// One page of this database's events inside the window that opens at
    /// `since_ms`, and the token naming the first event of the page after it —
    /// present only while events remain.
    ///
    /// Oldest first, as the log holds them, and at most
    /// [`RELATIONAL_DATABASE_EVENT_PAGE_SIZE`] of them. The walk is
    /// [`catalog_page`], shared with this service's other paged listings; only
    /// the token an entry is named by is this log's own
    /// ([`StoredRelationalDatabaseEvent::page_token`]). `None` is the answer
    /// when `page_token` names no event of that window — there is no page to
    /// report, which the caller turns into a refusal — and an absent token asks
    /// for the first page.
    pub fn event_page(
        &self,
        since_ms: i64,
        page_token: Option<&str>,
    ) -> Option<(
        Vec<crate::ops::types::RelationalDatabaseEvent>,
        Option<String>,
    )> {
        let reported = self
            .events
            .iter()
            .filter(|event| event.created_at_ms >= since_ms)
            .collect::<Vec<_>>();
        let (page, next_page_token) = catalog_page(
            &reported,
            RELATIONAL_DATABASE_EVENT_PAGE_SIZE,
            page_token,
            // Owned, because an event is named by its sequence number rather
            // than by a string member it holds. The walk is over at most
            // [`MAX_RELATIONAL_DATABASE_EVENTS`] entries, so it is bounded.
            |event| Some(std::borrow::Cow::Owned(event.page_token())),
        )?;
        Some((
            page.iter().map(|event| event.view(&self.name)).collect(),
            next_page_token,
        ))
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
                self.apply_modification(merged, now_ms);
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
    ///
    /// `at_ms` is the instant the modification reached the database — the call's
    /// own instant when it asked to apply now, and the instant it fell due when
    /// the clock applied it — which is what the entry this logs is timestamped
    /// at.
    fn apply_modification(&mut self, change: RelationalDatabaseModification, at_ms: i64) {
        // A member is a CHANGE only where the value differs from the one the
        // database already carries: a modification that asks for what the
        // database already has leaves it as it is and logs nothing, the same
        // rule the members an update applies at once are held to.
        let mut changed: Vec<&'static str> = Vec::new();
        if let Some(password) = change.master_user_password {
            // A modification that asks for the password the database already
            // holds replaces nothing: the version it holds keeps both its
            // secret and the instant it was created at, so a read of CURRENT
            // does not move.
            if self.master_user_password.password != password.password {
                changed.push("masterUserPassword");
                // The version the new one replaces becomes PREVIOUS, keeping
                // the instant it was created at: it is the same version, one
                // step further back.
                self.previous_master_user_password =
                    Some(std::mem::replace(&mut self.master_user_password, password));
            }
        }
        if let Some(blueprint) = change.blueprint {
            if blueprint.blueprint_id() != self.blueprint.blueprint_id() {
                changed.push("engineVersion");
            }
            self.blueprint = blueprint;
        }
        if let Some(enabled) = change.backup_retention_enabled {
            if enabled != self.backup_retention_enabled {
                changed.push("backupRetentionEnabled");
            }
            self.backup_retention_enabled = enabled;
        }
        if !changed.is_empty() {
            self.record_event(
                at_ms,
                format!(
                    "{RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED}: {}",
                    changed.join(", ")
                ),
                &[RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
            );
        }
    }

    /// The version of this database's master user password `version` names —
    /// one of the `RELATIONAL_DATABASE_PASSWORD_VERSION_*` values — or `None`
    /// when this database holds no such version.
    ///
    /// `CURRENT` is the password the master user is reached with now, `PREVIOUS`
    /// the version it replaced, and `PENDING` the version a modification
    /// waiting for this database's maintenance window will rotate to `CURRENT`:
    /// "After the PENDING password rotates to CURRENT, the PENDING password is
    /// no longer available" (the modelled `passwordVersion` documentation),
    /// which holds here because the version is taken from the modification
    /// still outstanding, and the clock takes that away when it applies it.
    pub fn master_user_password_version(
        &self,
        version: &str,
    ) -> Option<&StoredMasterUserPassword> {
        match version {
            RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT => Some(&self.master_user_password),
            RELATIONAL_DATABASE_PASSWORD_VERSION_PREVIOUS => {
                self.previous_master_user_password.as_ref()
            }
            RELATIONAL_DATABASE_PASSWORD_VERSION_PENDING => self
                .pending_modification
                .as_ref()
                .and_then(|pending| pending.change.master_user_password.as_ref()),
            _ => None,
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
            self.apply_modification(pending.change, pending.at_ms);
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
        // The engine goes down here and comes back up when the transition is
        // due, so the restart is TWO entries in the log, each at its own
        // instant.
        self.record_event(
            now_ms,
            RELATIONAL_DATABASE_EVENT_SHUTDOWN,
            &[RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        );
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
        // As with a restart: the engine goes down here, and the log carries a
        // second entry when it has finished shutting down.
        self.record_event(
            now_ms,
            RELATIONAL_DATABASE_EVENT_SHUTDOWN,
            &[RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        );
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

    /// The log streams this database publishes right now, as
    /// `GetRelationalDatabaseLogStreams` answers them.
    ///
    /// A log stream is a log file the database's ENGINE writes, so the set is the
    /// engine's own ([`relational_database_log_streams`]) — and it exists only
    /// once that engine has run: a database still in
    /// [`RELATIONAL_DATABASE_STATE_CREATING`] has started no engine and written
    /// no log, so it publishes no stream, exactly as it announces no master
    /// endpoint. Every later state keeps the files — a database in
    /// [`RELATIONAL_DATABASE_STATE_REBOOTING`] has already written its error log
    /// and the restart is itself logged — so the gate is on `creating` alone,
    /// the same one `master_endpoint` uses.
    pub fn log_streams(&self) -> Vec<String> {
        if self.state == RELATIONAL_DATABASE_STATE_CREATING {
            return Vec::new();
        }
        relational_database_log_streams(self.blueprint.engine)
            .iter()
            .map(|stream| (*stream).to_string())
            .collect()
    }

    /// Whether `stream` is one of the log streams this database publishes right
    /// now — the set [`Self::log_streams`] answers, read without building it.
    pub fn publishes_log_stream(&self, stream: &str) -> bool {
        self.state != RELATIONAL_DATABASE_STATE_CREATING
            && relational_database_log_streams(self.blueprint.engine).contains(&stream)
    }

    /// The lines the log stream `stream` of this database holds, in the order
    /// they were written.
    ///
    /// A log stream is a log FILE the database's engine writes, so its lines are
    /// what that engine has done — and what this store holds of that is the
    /// database's own event log: the entries in the
    /// [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`] and
    /// [`RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY`] categories are the
    /// engine coming up and going down, which is exactly what an engine records
    /// in its ERROR log. So an error-log stream
    /// ([`RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX`]) reports those entries
    /// rather than this record keeping a second copy of them, and the two reads
    /// of one happening — the service's event and the engine's log line — can
    /// never disagree.
    ///
    /// A QUERY log (`general/`, `slowquery/`) holds a line per statement a
    /// CLIENT ran, and this emulator serves no database protocol, so no
    /// statement has ever reached an engine here and every one of those streams
    /// is empty — the same reason a container service's log reports no line
    /// (`crate::ops::get_container_log`).
    ///
    /// Ordered by sequence, which is the order the service logged them in: a
    /// file is read in the order it was WRITTEN, while the event log's own
    /// reader orders by instant.
    fn log_stream_lines(&self, stream: &str) -> Vec<&StoredRelationalDatabaseEvent> {
        if !stream.starts_with(RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX) {
            return Vec::new();
        }
        let mut lines = self
            .events
            .iter()
            .filter(|event| {
                event
                    .categories
                    .contains(&RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION)
                    || event
                        .categories
                        .contains(&RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY)
            })
            .collect::<Vec<&StoredRelationalDatabaseEvent>>();
        lines.sort_by_key(|line| line.sequence);
        lines
    }

    /// One page of the log stream `stream` of this database: the lines it holds
    /// inside the half-open interval `[from_ms, to_ms)` — an absent bound leaves
    /// that side of the interval open — read from the end `start_from_head`
    /// names, or from the place `page_token` stopped at when the request carried
    /// one.
    ///
    /// At most [`RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE`] lines, always in the
    /// order they were written whichever end the read started from: the end
    /// chooses WHICH lines the first page carries, not the order they are
    /// reported in.
    ///
    /// A stream this database does not publish answers an empty page here; the
    /// caller refuses such a name ([`Self::publishes_log_stream`]) rather than
    /// reporting one.
    pub fn log_event_page(
        &self,
        stream: &str,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        start_from_head: bool,
        page_token: Option<LogEventPageToken>,
    ) -> LogEventPage {
        // A page carries at least one line, so a walk over a non-empty stream
        // always advances.
        let page_size = RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE.max(1);
        // "Specified in Coordinated Universal Time (UTC)": the interval is
        // half-open, so a line written AT `startTime` is inside it and one
        // written at `endTime` is not — which is what lets a caller walk a log
        // in abutting intervals without reading a line twice.
        let reported = self
            .log_stream_lines(stream)
            .into_iter()
            .filter(|line| from_ms.is_none_or(|from_ms| line.created_at_ms >= from_ms))
            .filter(|line| to_ms.is_none_or(|to_ms| line.created_at_ms < to_ms))
            .collect::<Vec<&StoredRelationalDatabaseEvent>>();
        let page: Vec<&StoredRelationalDatabaseEvent> = match page_token {
            // Forward from the position the last page stopped at: the line at
            // that position has not been reported yet.
            Some(token) if token.forward => reported
                .iter()
                .copied()
                .filter(|line| line.sequence >= token.position)
                .take(page_size)
                .collect(),
            // Backward from it: the page that ENDS just before that line.
            Some(token) => {
                let before = reported
                    .iter()
                    .copied()
                    .filter(|line| line.sequence < token.position)
                    .collect::<Vec<&StoredRelationalDatabaseEvent>>();
                let start = before.len().saturating_sub(page_size);
                before.into_iter().skip(start).collect()
            }
            // "If true is specified, the log event starts from the head of the
            // log. If false is specified, the log event starts from the tail of
            // the log."
            None if start_from_head => reported.iter().copied().take(page_size).collect(),
            None => {
                let start = reported.len().saturating_sub(page_size);
                reported.iter().copied().skip(start).collect()
            }
        };
        let (forward_at, backward_at) = match (page.first(), page.last()) {
            // The page after this one starts at the line after its last, and
            // the page before it ends at its first.
            (Some(first), Some(last)) => (last.sequence.saturating_add(1), first.sequence),
            // Nothing was reported, so the read stopped where it started: at
            // the token's own position, at the head of the log (position zero)
            // for a read from the head, and at its end — the position the next
            // line written will carry — for a read from the tail. A caller that
            // has reached the end of the stream can poll the forward token for
            // the lines written after this answer.
            _ => {
                let stopped = match page_token {
                    Some(token) => token.position,
                    None if start_from_head => 0,
                    None => self.next_event_sequence,
                };
                (stopped, stopped)
            }
        };
        LogEventPage {
            events: page
                .iter()
                .map(|line| line.log_event_view())
                .collect::<Vec<crate::ops::types::LogEvent>>(),
            next_forward_token: LogEventPageToken::forward(forward_at),
            next_backward_token: LogEventPageToken::backward(backward_at),
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
    /// The kind of call this entry records: a value of the modelled
    /// `OperationType` enum where it declares one, and otherwise the name of the
    /// call itself (see [`OPERATION_TYPE_COPY_SNAPSHOT`]) — never nothing, so an
    /// entry always says what ran.
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
/// it launches one, `RUNNING` once a deployment is active, `UPDATING` while "the
/// container service capacity or its custom domains are being updated", and
/// `DISABLED` for one that "is disabled, and its active deployment and
/// containers, if any, are shut down".
pub const CONTAINER_SERVICE_PENDING: &str = "PENDING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_READY: &str = "READY";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_DEPLOYING: &str = "DEPLOYING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_RUNNING: &str = "RUNNING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_UPDATING: &str = "UPDATING";
/// See [`CONTAINER_SERVICE_PENDING`].
pub const CONTAINER_SERVICE_DISABLED: &str = "DISABLED";

/// The `operationType` the log entry of a `CreateContainerService` call carries,
/// the modelled `OperationType` enum's own `CreateContainerService` member.
///
/// Every mutating container-service call writes the account's operation log the
/// same way the rest of this service's mutating calls do (see
/// [`ComAmazonawsLightsailLightsail20161128State::start_operation`]), so `GetOperations` and
/// `GetOperationsForResource` report it. The entry names the modelled
/// `ResourceType` enum's [`RESOURCE_TYPE_CONTAINER_SERVICE`] member and the
/// service's own name — including for the image-scoped calls, whose resource the
/// enum has no member for.
pub const OPERATION_TYPE_CREATE_CONTAINER_SERVICE: &str = "CreateContainerService";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_UPDATE_CONTAINER_SERVICE: &str = "UpdateContainerService";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_DELETE_CONTAINER_SERVICE: &str = "DeleteContainerService";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_CREATE_CONTAINER_SERVICE_DEPLOYMENT: &str =
    "CreateContainerServiceDeployment";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_REGISTER_CONTAINER_IMAGE: &str = "RegisterContainerImage";
/// See [`OPERATION_TYPE_CREATE_CONTAINER_SERVICE`].
pub const OPERATION_TYPE_DELETE_CONTAINER_IMAGE: &str = "DeleteContainerImage";

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

/// How many deployments of one container service the store keeps.
///
/// A service accumulates one deployment per create, so the history a caller can
/// drive has to be bounded — and the service itself bounds it: the
/// `GetContainerServiceDeployments` documentation states that "a set number of
/// deployments are kept before the oldest one is replaced with the newest one",
/// and points at the service's published quotas for the number. The number is
/// not in the model, so the figure here is the emulator's; what it implements is
/// the documented rule that the OLDEST deployment is the one dropped.
pub const MAX_DEPLOYMENTS_PER_SERVICE: usize = 50;

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

impl ContainerPowerSpec {
    /// This power as the modelled `ContainerServicePower`.
    ///
    /// `name` is "The friendly name of the power (`nano`)" and `powerId` "The ID
    /// of the power (`nano-1`)", so the two are reported from the catalog's own
    /// columns rather than derived from one another.
    pub fn view(&self) -> ContainerServicePower {
        ContainerServicePower {
            power_id: Some(self.power_id.to_string()),
            name: Some(self.name.to_string()),
            price: Some(self.price),
            cpu_count: Some(self.cpu_count),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            // `isActive` is "whether the power is active and can be specified for
            // container services", which is exactly what
            // [`container_service_power`] answers: that lookup is the one a
            // create or an update resolves a requested `power` against, so the
            // flag is read off it rather than restated as a column nothing
            // consults. A power this service stopped accepting therefore reports
            // itself inactive without anything else having to be kept in step.
            is_active: Some(container_service_power(self.name).is_some()),
        }
    }
}

/// The powers a `GetContainerServicePowers` call reports out of `specs`.
///
/// Every entry is reported — the result shape carries no filter and the operation
/// takes no member, so which powers can be SPECIFIED is said by each entry's
/// `isActive` rather than by leaving one out — in `specs`' own order, so two reads
/// answer identically.
pub fn container_service_powers_from(specs: &[ContainerPowerSpec]) -> Vec<ContainerServicePower> {
    specs.iter().map(ContainerPowerSpec::view).collect()
}

/// The container service powers a `GetContainerServicePowers` call reports:
/// [`container_service_powers_from`] over this service's published catalog,
/// [`CONTAINER_SERVICE_POWERS`].
pub fn container_service_powers() -> Vec<ContainerServicePower> {
    container_service_powers_from(CONTAINER_SERVICE_POWERS)
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

// ---------------------------------------------------------------------------
// Container API metadata
// ---------------------------------------------------------------------------

/// The key one container-API metadatum carries its NAME under.
///
/// `ContainerServiceMetadataEntry` is modelled as a free `map<string, string>`
/// (`smithy_query(query="shape",shape="ContainerServiceMetadataEntry")`), so the
/// keys are not in the model: Lightsail publishes each metadatum as the pair
/// `name`/`value`, which is the pair the documented way of reading this catalog
/// selects on (`--query "metadata[?name=='lightsailctlVersion'].value"`).
pub const CONTAINER_API_METADATA_NAME_KEY: &str = "name";

/// The key one container-API metadatum carries its VALUE under. See
/// [`CONTAINER_API_METADATA_NAME_KEY`].
pub const CONTAINER_API_METADATA_VALUE_KEY: &str = "value";

/// The metadatum that reports the current version of the Lightsail Control
/// (lightsailctl) plugin — the one the modelled `metadata` member calls out
/// ("Metadata about Lightsail containers, such as the current version of the
/// Lightsail Control (lightsailctl) plugin").
pub const CONTAINER_API_METADATA_LIGHTSAILCTL_VERSION: &str = "lightsailctlVersion";

/// The lightsailctl plugin release this emulator's container API publishes, as
/// that plugin's `major.minor.patch` version. It is the version a client compares
/// its locally installed plugin against before pushing a container image.
pub const LIGHTSAILCTL_VERSION: &str = "1.0.7";

/// The container-API metadata catalog, as `(name, value)` pairs in the order the
/// service reports them.
///
/// Service-wide published data, like the bundle and blueprint tables above: it is
/// the same for every caller and no operation of this service creates, changes or
/// retires an entry, so it lives here rather than in a per-account record.
pub const CONTAINER_API_METADATA_SPECS: &[(&str, &str)] = &[(
    CONTAINER_API_METADATA_LIGHTSAILCTL_VERSION,
    LIGHTSAILCTL_VERSION,
)];

/// `specs` as the modelled entry list: one entry per metadatum, each a map of its
/// name and its value under [`CONTAINER_API_METADATA_NAME_KEY`] and
/// [`CONTAINER_API_METADATA_VALUE_KEY`].
pub fn container_api_metadata_from(specs: &[(&str, &str)]) -> Vec<HashMap<String, String>> {
    specs
        .iter()
        .map(|(name, value)| {
            HashMap::from([
                (
                    CONTAINER_API_METADATA_NAME_KEY.to_string(),
                    (*name).to_string(),
                ),
                (
                    CONTAINER_API_METADATA_VALUE_KEY.to_string(),
                    (*value).to_string(),
                ),
            ])
        })
        .collect()
}

/// The published container-API metadata catalog, as the modelled entry list.
pub fn container_api_metadata() -> Vec<HashMap<String, String>> {
    container_api_metadata_from(CONTAINER_API_METADATA_SPECS)
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
/// "You can specify values between `200` and `499`" (`successCodes`).
const HEALTH_CHECK_SUCCESS_CODES_RANGE: (i32, i32) = (200, 499);
/// What separates the values of a success-code list: "You can specify multiple
/// values (for example, `200,202`)" (`successCodes`).
const SUCCESS_CODES_VALUE_SEPARATOR: char = ',';
/// What separates the two bounds of a success-code range: "or a range of values
/// (for example, `200-299`)" (`successCodes`).
const SUCCESS_CODES_RANGE_SEPARATOR: char = '-';

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

// ---------------------------------------------------------------------------
// The container image registry of the account
// ---------------------------------------------------------------------------

/// How long one set of container image registry log in credentials stays valid.
///
/// A duration the service DOCUMENTS, not a chosen one: the modelled
/// `ContainerServiceRegistryLogin$expiresAt` states it itself — "The log in
/// credentials expire 12 hours after they are created, at which point you will
/// need to create a new set of log in credentials using the
/// `CreateContainerServiceRegistryLogin` action."
pub const REGISTRY_LOGIN_LIFETIME_MS: i64 = 12 * 60 * 60 * 1_000;

/// The path space the container image registry of this account is published
/// under, beneath the endpoint the emulator is reached on.
///
/// `ContainerServiceRegistryLogin$registry` is "The address to use to push
/// container images to the container image registry of a Lightsail account", so
/// it is a promise that the caller which read it can reach that address. AWS
/// publishes the registry as a DNS name of its own; the HOSTNAME is
/// environment-specific while the promise is not, this emulator owns no DNS
/// zone, and a literal AWS hostname would send the next client out of the
/// emulator entirely (see [`request_endpoint`]). So the registry is published
/// beneath the request's own endpoint, with the region AWS puts in its hostname
/// carried in the path instead — exactly as
/// [`CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`] carries a container service's
/// labels. The prefix names this service and this resource, so it cannot
/// collide with another service's modelled URI space.
pub const CONTAINER_IMAGE_REGISTRY_PATH_PREFIX: &str = "/lightsail-container-image-registry";

/// The address a `docker login` — and the pushes after it — are pointed at for
/// this account's registry in `region`, as announced to a caller that reached
/// this emulator at `endpoint` (its `Host`/`:authority`).
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on.
pub fn container_image_registry_url(endpoint: &str, region: &str) -> String {
    format!("https://{endpoint}{CONTAINER_IMAGE_REGISTRY_PATH_PREFIX}/{region}")
}

/// Which registry a request to a published registry address addresses, and what
/// inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerImageRegistryTarget {
    /// The region whose registry the announced address names.
    pub region: String,
    /// What the client appended BENEATH that address: the registry protocol's
    /// own request target (`/v2/…` for a container image registry client), its
    /// leading `/` included, or the empty string where the client appended
    /// nothing at all.
    pub path: String,
}

/// The registry a request to a published registry address addresses, or `None`
/// when the path is not one [`container_image_registry_url`] publishes.
///
/// Anything the caller appended BENEATH the region is the registry protocol's
/// own path (`/v2/…` for a container image registry client): it names no part of
/// this service's modelled URI space, so it is carried through to the registry
/// the address names.
pub fn parse_container_image_registry_path(path: &str) -> Option<ContainerImageRegistryTarget> {
    let rest = path
        .strip_prefix(CONTAINER_IMAGE_REGISTRY_PATH_PREFIX)?
        .strip_prefix('/')?;
    let (region, beneath) = match rest.find('/') {
        Some(cut) => rest.split_at(cut),
        None => (rest, ""),
    };
    (!region.is_empty()).then(|| ContainerImageRegistryTarget {
        region: region.to_string(),
        path: beneath.to_string(),
    })
}

/// The most sets of registry log in credentials this store keeps per region.
///
/// Emulator resource guard, NOT an AWS quota: AWS caps the number of live sets
/// nowhere, and the clock already forgets every set
/// [`REGISTRY_LOGIN_LIFETIME_MS`] after it was created, which bounds the store
/// for any ordinary caller. This bounds it for one that asks for more than that
/// within a single window; the set dropped is the OLDEST, which is the one
/// closest to expiring anyway.
pub const MAX_REGISTRY_LOGINS_PER_REGION: usize = 500;

/// One set of container image registry log in credentials this account was
/// handed: the username it logs in with, what a presented password is checked
/// against, and when the set expires.
///
/// The password itself is NOT kept. It was minted by the emulator's credential
/// store, carried to its caller in the one response that creates it, and only
/// its SHA-256 digest is stored — so nothing can read a usable password back out
/// of this store, the same reason a bucket access key keeps only its id here
/// (see [`StoredBucketAccessKey`]).
#[derive(Debug, Clone)]
pub struct StoredRegistryLogin {
    /// The username this set logs in with, which is also what identifies it: the
    /// id the credential store minted for it.
    pub username: String,
    /// The SHA-256 digest of the password, hex — see [`StoredRegistryLogin`].
    password_digest: String,
    pub created_at_ms: i64,
    /// The instant this set expires: [`REGISTRY_LOGIN_LIFETIME_MS`] after it was
    /// created.
    pub expires_at_ms: i64,
}

impl StoredRegistryLogin {
    /// The set created at `created_at_ms` that logs in as `username` with
    /// `password` — which is hashed here and not retained.
    pub fn new(username: String, password: &str, created_at_ms: i64) -> Self {
        Self {
            username,
            password_digest: registry_password_digest(password),
            created_at_ms,
            expires_at_ms: created_at_ms.saturating_add(REGISTRY_LOGIN_LIFETIME_MS),
        }
    }

    /// Whether `password` is the password this set logs in with.
    pub fn accepts(&self, password: &str) -> bool {
        // DIGESTS are compared, not secrets: the stored value is the SHA-256 of
        // a 712-character random token, so what the comparison can tell a
        // guesser is at most how far a digest matched, which nobody can turn
        // back into the password that produced it.
        self.password_digest == registry_password_digest(password)
    }
}

/// The SHA-256 of a registry password, hex — the only form of one this store
/// holds. See [`StoredRegistryLogin`].
fn registry_password_digest(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// What a push put IN the container image registry of the account
// ---------------------------------------------------------------------------

/// The algorithm the registry addresses its content by, which is the one the
/// registry protocol's digests name.
pub const REGISTRY_DIGEST_ALGORITHM: &str = "sha256";

/// The number of hex characters a [`REGISTRY_DIGEST_ALGORITHM`] digest is
/// written with: SHA-256 is 32 bytes, two characters each.
const REGISTRY_DIGEST_HEX_LEN: usize = 64;

/// The digest of `content`, spelled the way the registry protocol spells one:
/// the algorithm, a colon, and the hex of the digest over exactly those bytes.
///
/// This is computed over the content that arrived, never taken from what a
/// client claimed: it is what makes the registry content-addressed, so a stored
/// blob or manifest is reachable only under the digest of its own bytes.
pub fn registry_digest(content: &[u8]) -> String {
    format!(
        "{REGISTRY_DIGEST_ALGORITHM}:{}",
        hex::encode(Sha256::digest(content))
    )
}

/// Whether `digest` is spelled the way this registry addresses content: the
/// algorithm it holds content under, a colon, and that algorithm's number of
/// lower-case hex characters.
pub fn is_registry_digest(digest: &str) -> bool {
    digest
        .strip_prefix(REGISTRY_DIGEST_ALGORITHM)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|hex| {
            hex.len() == REGISTRY_DIGEST_HEX_LEN
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// The most bytes one blob may put in the registry.
///
/// An emulator resource guard, NOT a registry limit: the registry protocol caps
/// a blob nowhere, and a real image layer is far larger than this. The registry
/// here holds every byte it accepts in this process's memory, so what one caller
/// can make it hold has to be bounded — a push past this is refused rather than
/// half-stored.
pub const MAX_REGISTRY_BLOB_BYTES: usize = 32 * 1024 * 1024;

/// The most bytes one region's registry holds at once — blobs, manifests and
/// in-flight uploads together. The same guard as [`MAX_REGISTRY_BLOB_BYTES`],
/// for the total rather than one blob.
pub const MAX_REGISTRY_BYTES_PER_REGION: usize = 128 * 1024 * 1024;

/// The most blob uploads one region's registry keeps in flight at once.
///
/// An emulator resource guard as well: an upload session holds what has been
/// sent to it so far, and a client that starts sessions without finishing them
/// would otherwise keep them all. The session dropped is the OLDEST, which is
/// the one most likely already abandoned; a client whose session is gone is
/// answered that it is unknown and can start another.
pub const MAX_REGISTRY_UPLOADS_PER_REGION: usize = 64;

/// The most container images one container service's registry holds at once.
///
/// An emulator resource guard, not an AWS quota: a service accumulates one
/// entry per registration, so the count a caller can drive has to be bounded.
/// The entry dropped is the oldest registration.
pub const MAX_CONTAINER_IMAGES_PER_SERVICE: usize = 500;

/// What ONE entry of a region's registry costs this process beyond the bytes it
/// holds: the map node it lives in, the headers of its owned `String`s and
/// `Vec`s, and its fixed fields.
///
/// Deliberately a generous over-estimate of that overhead (a one-byte blob
/// really costs this process on the order of 140 bytes: a 71-character digest
/// key plus two allocation headers, a timestamp and a tree node), so the
/// accounting errs towards refusing rather than towards holding more than
/// [`MAX_REGISTRY_BYTES_PER_REGION`] says. Charging it is what makes that bound
/// a bound on MEMORY and on the entry COUNT: every entry costs at least this
/// much, so one region's registry holds at most
/// `MAX_REGISTRY_BYTES_PER_REGION / REGISTRY_ENTRY_OVERHEAD_BYTES` entries
/// however small the content pushed into them is.
pub const REGISTRY_ENTRY_OVERHEAD_BYTES: usize = 128;

/// What an entry whose owned bytes are `parts` costs a region's registry: those
/// bytes together with [`REGISTRY_ENTRY_OVERHEAD_BYTES`].
///
/// The ONE place an entry's cost is worked out, so what
/// [`StoredImageRegistry::bytes_held`] accounts for and what a write is refused
/// for passing cannot drift apart.
fn registry_entry_bytes(parts: &[usize]) -> usize {
    parts.iter().fold(REGISTRY_ENTRY_OVERHEAD_BYTES, |total, part| {
        total.saturating_add(*part)
    })
}

/// One blob the registry holds: the bytes that were pushed, and when they
/// arrived.
#[derive(Debug, Clone)]
pub struct StoredRegistryBlob {
    /// The bytes themselves, held verbatim so a later read answers what was
    /// pushed rather than something built to look like it.
    pub content: Vec<u8>,
    pub pushed_at_ms: i64,
}

/// One image manifest the registry holds: the bytes that were pushed, the media
/// type they were pushed as, the repository they were pushed to, and when.
#[derive(Debug, Clone)]
pub struct StoredRegistryManifest {
    pub content: Vec<u8>,
    /// The media type the manifest is held in, which is what a read of it
    /// answers with.
    ///
    /// One of the manifest media types the registry stores, as the plane that
    /// accepted the push resolved it — never the spelling a client sent, so a
    /// value held here is always a media type this registry chose.
    pub media_type: String,
    pub repository: String,
    pub pushed_at_ms: i64,
}

/// One blob upload in flight: the repository it was started against, what has
/// been sent to it so far, and when it started.
#[derive(Debug, Clone)]
pub struct StoredBlobUpload {
    pub repository: String,
    pub content: Vec<u8>,
    pub started_at_ms: i64,
}

/// What the registry answered to content offered to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryWrite {
    /// The content is held, addressed by this digest.
    Stored(String),
    /// Holding it would take this region's registry past `limit` bytes.
    OutOfSpace { limit: usize },
}

/// What the registry answered to bytes offered to an upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadAppend {
    /// The session now holds `held` bytes.
    Appended { held: usize },
    /// No such upload session: never started, started against another
    /// repository, already finished, or dropped to keep the region within
    /// [`MAX_REGISTRY_UPLOADS_PER_REGION`].
    Unknown,
    /// Taking them would pass `limit` bytes.
    OutOfSpace { limit: usize },
}

/// What the registry answered to a request finishing an upload session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadFinish {
    /// The whole blob is held, addressed by this digest.
    Stored(String),
    /// What the session holds has this digest, not the one the request claimed,
    /// so nothing was stored and the session is untouched.
    DigestMismatch { computed: String },
    /// No such upload session — see [`UploadAppend::Unknown`].
    Unknown,
    /// Holding it would pass `limit` bytes.
    OutOfSpace { limit: usize },
}

/// The container image registry of the account in ONE region: the blobs and
/// image manifests pushed to it, the tag each repository's tags resolve to, and
/// the uploads in flight.
///
/// Content-addressed, as the registry protocol is: a blob and a manifest are
/// held under the digest of their own bytes, so the registry can only answer
/// with what was actually pushed to it.
#[derive(Debug, Default)]
pub struct StoredImageRegistry {
    /// Every blob, keyed by its digest. Keyed registry-wide and not per
    /// repository because the digest IS the identity of the content.
    pub blobs: BTreeMap<String, StoredRegistryBlob>,
    /// Every image manifest, keyed by its digest — the digest a `docker push`
    /// reports and `RegisterContainerImage` takes.
    pub manifests: BTreeMap<String, StoredRegistryManifest>,
    /// The manifest digest each repository's tags name, keyed by the repository
    /// and the tag.
    pub tags: BTreeMap<(String, String), String>,
    /// The blob uploads in flight, keyed by the session id the registry minted
    /// for each.
    pub uploads: BTreeMap<String, StoredBlobUpload>,
}

impl StoredImageRegistry {
    /// How many bytes this registry holds: its blobs, its manifests, the tags
    /// pointing at them, and what its in-flight uploads have taken so far.
    ///
    /// Every entry is charged what it costs this PROCESS rather than only the
    /// content it was pushed: the string it is keyed by (a digest, or a
    /// repository and a tag), the owned strings on the record itself, and
    /// [`REGISTRY_ENTRY_OVERHEAD_BYTES`] for the rest of it. Otherwise a client
    /// pushing a stream of distinct one-byte blobs — each one a digest of its
    /// own, so none of them is the blob already there — would be charged a byte
    /// for an entry costing a hundred times that, and
    /// [`MAX_REGISTRY_BYTES_PER_REGION`] would bound neither this process's
    /// memory nor the number of entries one caller can make it hold.
    ///
    /// A tag is counted for the reason it always was: a tag pointing at a
    /// manifest the registry already holds costs no manifest bytes, so without
    /// counting it a client could grow this registry by tagging one manifest
    /// over and over.
    pub fn bytes_held(&self) -> usize {
        let blobs: usize = self
            .blobs
            .iter()
            .map(|(digest, blob)| registry_entry_bytes(&[digest.len(), blob.content.len()]))
            .fold(0, usize::saturating_add);
        let manifests: usize = self
            .manifests
            .iter()
            .map(|(digest, manifest)| {
                registry_entry_bytes(&[
                    digest.len(),
                    manifest.content.len(),
                    manifest.repository.len(),
                    manifest.media_type.len(),
                ])
            })
            .fold(0, usize::saturating_add);
        let tags: usize = self
            .tags
            .iter()
            .map(|((repository, tag), digest)| {
                registry_entry_bytes(&[repository.len(), tag.len(), digest.len()])
            })
            .fold(0, usize::saturating_add);
        let uploads: usize = self
            .uploads
            .iter()
            .map(|(session, upload)| {
                registry_entry_bytes(&[
                    session.len(),
                    upload.repository.len(),
                    upload.content.len(),
                ])
            })
            .fold(0, usize::saturating_add);
        blobs
            .saturating_add(manifests)
            .saturating_add(tags)
            .saturating_add(uploads)
    }

    /// Whether this registry holds the blob `digest` names.
    pub fn has_blob(&self, digest: &str) -> bool {
        self.blobs.contains_key(digest)
    }

    /// The blob `digest` names.
    pub fn blob(&self, digest: &str) -> Option<&StoredRegistryBlob> {
        self.blobs.get(digest)
    }

    /// Whether this registry holds the image manifest `digest` names.
    pub fn has_manifest(&self, digest: &str) -> bool {
        self.manifests.contains_key(digest)
    }

    /// The digest the `reference` of `repository` resolves to: a reference that
    /// IS a digest names content directly, and any other reference is a tag,
    /// which names whatever manifest was last pushed under it.
    ///
    /// A DIGEST is resolved registry-wide, by design: the registry is the
    /// ACCOUNT's and content-addressed, so a digest names the same content
    /// whichever repository path addresses it (the property that lets a client
    /// mount content it has already pushed instead of sending it again). A TAG
    /// is the repository's own, so it resolves only under the repository it was
    /// pushed to.
    pub fn resolve_reference(&self, repository: &str, reference: &str) -> Option<String> {
        if self.manifests.contains_key(reference) {
            return Some(reference.to_string());
        }
        self.tags
            .get(&(repository.to_string(), reference.to_string()))
            .cloned()
    }

    /// The manifest the `reference` of `repository` resolves to.
    pub fn manifest(&self, repository: &str, reference: &str) -> Option<&StoredRegistryManifest> {
        let digest = self.resolve_reference(repository, reference)?;
        self.manifests.get(&digest)
    }

    /// The tags `repository` holds, in one stable order.
    pub fn tags_of(&self, repository: &str) -> Vec<String> {
        self.tags
            .keys()
            .filter(|(held, _)| held == repository)
            .map(|(_, tag)| tag.clone())
            .collect()
    }

    /// Whether anything was pushed to `repository` — a manifest of its own, or
    /// a tag naming one.
    pub fn holds_repository(&self, repository: &str) -> bool {
        self.manifests
            .values()
            .any(|manifest| manifest.repository == repository)
            || self.tags.keys().any(|(held, _)| held == repository)
    }

    /// Hold `content` as a blob, addressed by the digest of its own bytes.
    ///
    /// Content the registry already holds costs it nothing: the digest names
    /// those exact bytes, so a re-push of them is the blob that is already
    /// there.
    pub fn put_blob(&mut self, content: Vec<u8>, now_ms: i64) -> RegistryWrite {
        let digest = registry_digest(&content);
        if self.blobs.contains_key(&digest) {
            return RegistryWrite::Stored(digest);
        }
        // What the ENTRY costs, not only what was pushed: the same accounting
        // `bytes_held` reports, so a push is refused exactly when holding it
        // would take this region past its bound.
        if !self.can_hold(registry_entry_bytes(&[digest.len(), content.len()])) {
            return RegistryWrite::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        self.blobs.insert(
            digest.clone(),
            StoredRegistryBlob {
                content,
                pushed_at_ms: now_ms,
            },
        );
        RegistryWrite::Stored(digest)
    }

    /// Hold `content` as an image manifest of `repository`, addressed by the
    /// digest of its own bytes, and point `tag` at it when the push named one.
    ///
    /// What the write needs is judged for the manifest and the tag TOGETHER,
    /// before either is held: content the registry already holds and a tag it
    /// already has cost it nothing.
    pub fn put_manifest(
        &mut self,
        repository: &str,
        tag: Option<&str>,
        content: Vec<u8>,
        media_type: &str,
        now_ms: i64,
    ) -> RegistryWrite {
        let digest = registry_digest(&content);
        let fresh_manifest = !self.manifests.contains_key(&digest);
        let tag_key = tag.map(|tag| (repository.to_string(), tag.to_string()));
        let fresh_tag = tag_key
            .as_ref()
            .is_some_and(|key| !self.tags.contains_key(key));
        let needed = if fresh_manifest {
            registry_entry_bytes(&[
                digest.len(),
                content.len(),
                repository.len(),
                media_type.len(),
            ])
        } else {
            0
        }
        .saturating_add(
            tag_key
                .as_ref()
                .filter(|_| fresh_tag)
                .map_or(0, |(repository, tag)| {
                    registry_entry_bytes(&[repository.len(), tag.len(), digest.len()])
                }),
        );
        if !self.can_hold(needed) {
            return RegistryWrite::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        if fresh_manifest {
            self.manifests.insert(
                digest.clone(),
                StoredRegistryManifest {
                    content,
                    media_type: media_type.to_string(),
                    repository: repository.to_string(),
                    pushed_at_ms: now_ms,
                },
            );
        }
        if let Some(key) = tag_key {
            // A tag is a MOVING reference: pushing to one that is taken points
            // it at the manifest just pushed, which is what the protocol says a
            // tag is.
            self.tags.insert(key, digest.clone());
        }
        RegistryWrite::Stored(digest)
    }

    /// Record one blob upload started against `repository` under the session id
    /// `session`, dropping the oldest session once this region holds
    /// [`MAX_REGISTRY_UPLOADS_PER_REGION`].
    pub fn start_upload(&mut self, session: String, repository: &str, now_ms: i64) {
        self.uploads.insert(
            session,
            StoredBlobUpload {
                repository: repository.to_string(),
                content: Vec::new(),
                started_at_ms: now_ms,
            },
        );
        while self.uploads.len() > MAX_REGISTRY_UPLOADS_PER_REGION {
            let oldest = self
                .uploads
                .iter()
                .min_by(|left, right| {
                    left.1
                        .started_at_ms
                        .cmp(&right.1.started_at_ms)
                        .then_with(|| left.0.cmp(right.0))
                })
                .map(|(id, _)| id.clone());
            let Some(oldest) = oldest else { break };
            // A session dropped early stops accepting the bytes its client was
            // sending it, so the eviction is traceable rather than a silent
            // surprise. Only the session id is logged; no uploaded byte is.
            ::tracing::warn!(
                session = %oldest,
                limit = MAX_REGISTRY_UPLOADS_PER_REGION,
                "the oldest container image blob upload was dropped to keep this region's \
                 registry within the store's bound"
            );
            self.uploads.remove(&oldest);
        }
    }

    /// The upload session `session` of `repository` names.
    ///
    /// Scoped by the repository too: a session was started against ONE
    /// repository, so the same id under another repository's path names no
    /// session of it.
    pub fn upload(&self, session: &str, repository: &str) -> Option<&StoredBlobUpload> {
        self.uploads
            .get(session)
            .filter(|upload| upload.repository == repository)
    }

    /// Take `content` into the upload session `session` of `repository`.
    pub fn append_to_upload(
        &mut self,
        session: &str,
        repository: &str,
        content: &[u8],
    ) -> UploadAppend {
        if self.upload(session, repository).is_none() {
            return UploadAppend::Unknown;
        }
        let held_elsewhere = self
            .bytes_held()
            .saturating_sub(self.uploads.get(session).map_or(0, |u| u.content.len()));
        let Some(upload) = self.uploads.get_mut(session) else {
            return UploadAppend::Unknown;
        };
        let would_hold = upload.content.len().saturating_add(content.len());
        // Both bounds are judged BEFORE the bytes are taken, so a session that
        // cannot take them keeps exactly what it had.
        if would_hold > MAX_REGISTRY_BLOB_BYTES {
            return UploadAppend::OutOfSpace {
                limit: MAX_REGISTRY_BLOB_BYTES,
            };
        }
        if held_elsewhere.saturating_add(would_hold) > MAX_REGISTRY_BYTES_PER_REGION {
            return UploadAppend::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        upload.content.extend_from_slice(content);
        UploadAppend::Appended {
            held: upload.content.len(),
        }
    }

    /// Finish the upload session `session` of `repository` with the trailing
    /// `content` a client sent on the finishing request, holding the whole blob
    /// as one only when `claimed` is the digest of everything that session
    /// holds.
    ///
    /// The session SURVIVES every answer but a stored one — a claimed digest
    /// that is not the content's leaves the bytes where they were, so the
    /// client can carry on with that session rather than start again.
    pub fn finish_upload(
        &mut self,
        session: &str,
        repository: &str,
        content: &[u8],
        claimed: &str,
        now_ms: i64,
    ) -> UploadFinish {
        let Some(upload) = self.upload(session, repository) else {
            return UploadFinish::Unknown;
        };
        let would_hold = upload.content.len().saturating_add(content.len());
        let held_elsewhere = self.bytes_held().saturating_sub(upload.content.len());
        if would_hold > MAX_REGISTRY_BLOB_BYTES {
            return UploadFinish::OutOfSpace {
                limit: MAX_REGISTRY_BLOB_BYTES,
            };
        }
        if held_elsewhere.saturating_add(would_hold) > MAX_REGISTRY_BYTES_PER_REGION {
            return UploadFinish::OutOfSpace {
                limit: MAX_REGISTRY_BYTES_PER_REGION,
            };
        }
        // The digest of everything the blob would be, taken over the session's
        // own bytes and the trailing ones in order and WITHOUT joining them
        // first: the digest of the content is what decides, so it is computed
        // before anything is stored or dropped.
        let mut hasher = Sha256::new();
        hasher.update(&upload.content);
        hasher.update(content);
        let computed = format!(
            "{REGISTRY_DIGEST_ALGORITHM}:{}",
            hex::encode(hasher.finalize())
        );
        if computed != claimed {
            return UploadFinish::DigestMismatch { computed };
        }
        let Some(mut upload) = self.uploads.remove(session) else {
            return UploadFinish::Unknown;
        };
        upload.content.extend_from_slice(content);
        match self.put_blob(upload.content, now_ms) {
            RegistryWrite::Stored(digest) => UploadFinish::Stored(digest),
            RegistryWrite::OutOfSpace { limit } => UploadFinish::OutOfSpace { limit },
        }
    }

    /// Take the upload session `session` of `repository` out of this registry —
    /// what cancelling an upload does with it.
    pub fn take_upload(&mut self, session: &str, repository: &str) -> Option<StoredBlobUpload> {
        // The session has to be one of THIS repository's; `?` answers `None`
        // for any other, which is what a caller asking for one reads.
        self.upload(session, repository)?;
        self.uploads.remove(session)
    }

    /// Whether this registry can take `additional` more bytes and stay within
    /// [`MAX_REGISTRY_BYTES_PER_REGION`].
    fn can_hold(&self, additional: usize) -> bool {
        self.bytes_held().saturating_add(additional) <= MAX_REGISTRY_BYTES_PER_REGION
    }
}

/// The name a container service assigns an image registered to it as the
/// `version`-th of `label`.
///
/// The form is the one the modelled `RegisterContainerImageRequest$label`
/// documentation gives: "if the name of your container service is
/// `mycontainerservice`, and the label that you specify is `mystaticwebsite`,
/// then the name of the registered container image will be
/// `:mycontainerservice.mystaticwebsite.1`".
pub fn container_image_name(service_name: &str, label: &str, version: i32) -> String {
    format!(":{service_name}.{label}.{version}")
}

/// One container image registered to a container service: the label it was
/// registered under, which version of that label it is, the digest that was
/// pushed, and when it was registered.
///
/// The assigned NAME is not stored beside them: it is
/// [`container_image_name`] of the holding service's name, the label and the
/// version, so one record cannot disagree with the name it is reported under.
#[derive(Debug, Clone)]
pub struct StoredContainerImage {
    /// The label the registering request named.
    pub label: String,
    /// Which version of that label this image is — 1 for the first image
    /// registered under it, and one more for each after, as the modelled
    /// `RegisterContainerImageRequest$label` documentation describes.
    pub version: i32,
    /// The digest of the pushed image, kept verbatim.
    pub digest: String,
    pub created_at_ms: i64,
}

impl StoredContainerImage {
    /// The name the container service `service_name` assigned this image.
    pub fn name(&self, service_name: &str) -> String {
        container_image_name(service_name, &self.label, self.version)
    }

    /// This record as the modelled `ContainerImage`, as the container service
    /// `service_name` reports it.
    pub fn view(&self, service_name: &str) -> ContainerImage {
        ContainerImage {
            image: Some(self.name(service_name)),
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
    /// Whether this deployment ever BECAME ACTIVE, i.e. whether the service
    /// launched the containers it declares.
    ///
    /// `INACTIVE` covers two deployments the service tells apart: one that ran
    /// and stepped down when a later deployment replaced it, and one that was
    /// replaced while it was still `ACTIVATING` and so never launched anything.
    /// This is the bit that says which, so the containers that "previously ran"
    /// are only the ones a deployment that did activate declares.
    pub activated: bool,
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
    /// Every deployment this service has been given, NEWEST VERSION FIRST.
    ///
    /// The ONE place a deployment of this service lives: the record that is
    /// `ACTIVE` is what `ContainerService$currentDeployment` reports and the one
    /// that is `ACTIVATING` is its `nextDeployment`, so a deployment is never
    /// held twice and the history, the current and the waiting deployment can
    /// never disagree. Bounded by [`MAX_DEPLOYMENTS_PER_SERVICE`].
    pub deployments: VecDeque<StoredDeployment>,
    /// The version the NEXT deployment of this service will carry.
    pub next_deployment_version: i32,
    /// The container images registered to this service, in registration order —
    /// the service's own registry, which `GetContainerImages` reports.
    ///
    /// A `Vec` and not a map: the modelled `ContainerImageList` documents no
    /// order, so registration order is the one stable total order the store has,
    /// and the label and version of each entry already key it uniquely.
    ///
    /// An entry gets here only through `RegisterContainerImage`, which takes
    /// the digest of an image that was PUSHED to the container image registry
    /// of the account (`StoredImageRegistry`) — so a registered image always
    /// names content this emulator holds.
    pub images: Vec<StoredContainerImage>,
    /// The phases the clock still owes this service, soonest first.
    pub owed: VecDeque<OwedPhase>,
}

impl StoredContainerService {
    /// Register the pushed image `digest` to this service under `label`, and
    /// answer the record that was stored.
    ///
    /// The version assigned is one past the highest this label's registered
    /// images carry, which is the rule the modelled
    /// `RegisterContainerImageRequest$label` documentation states: "If you push
    /// and register another container image to the same Lightsail container
    /// service, with the same label, then the version number for the new
    /// registered container image will be `2`. If you push and register another
    /// container image, the version number will be `3`, and so on." Two labels
    /// are counted independently, each starting at 1, and the version keeps the
    /// name of every image this registry holds distinct.
    ///
    /// The registry is bounded by [`MAX_CONTAINER_IMAGES_PER_SERVICE`]: past
    /// that, the oldest registration is dropped.
    pub fn register_image(
        &mut self,
        label: &str,
        digest: &str,
        now_ms: i64,
    ) -> StoredContainerImage {
        let version = self
            .images
            .iter()
            .filter(|image| image.label == label)
            .map(|image| image.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let registered = StoredContainerImage {
            label: label.to_string(),
            version,
            digest: digest.to_string(),
            created_at_ms: now_ms,
        };
        self.images.push(registered.clone());
        if let Some(excess) = self.images.len().checked_sub(MAX_CONTAINER_IMAGES_PER_SERVICE) {
            for evicted in self.images.drain(..excess) {
                // An image the store had to forget stops being reported before
                // anything deleted it, so the eviction is traceable rather than
                // a silent surprise.
                ::tracing::warn!(
                    service = %self.name,
                    image = %evicted.name(&self.name),
                    limit = MAX_CONTAINER_IMAGES_PER_SERVICE,
                    "the oldest registered container image was dropped to keep this container \
                     service within the store's bound"
                );
            }
        }
        registered
    }

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
            current_deployment: self.current_deployment().map(StoredDeployment::view),
            next_deployment: self.next_deployment().map(StoredDeployment::view),
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
        let version = self.next_deployment().map(|next| next.version);
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
            let Some(activating) = self
                .deployments
                .iter()
                .position(|deployment| deployment.state == DEPLOYMENT_STATE_ACTIVATING)
            else {
                continue;
            };
            // "The container service can have only one deployment in an active
            // state at a time": whatever was running steps down to the state of
            // a deployment that ran before, and stays in the history.
            for deployment in self.deployments.iter_mut() {
                if deployment.state == DEPLOYMENT_STATE_ACTIVE {
                    deployment.state = DEPLOYMENT_STATE_INACTIVE;
                }
            }
            self.deployments[activating].state = DEPLOYMENT_STATE_ACTIVE;
            // This deployment is launching its containers now, so they are the
            // ones that RAN on this service from here on — including after a
            // later deployment supersedes it, which is what distinguishes it
            // from a deployment that was replaced before it ever activated.
            self.deployments[activating].activated = true;
        }
    }

    /// The deployment currently running on this service, if any — the one in
    /// `ACTIVE`, of which the model admits at most one.
    pub fn current_deployment(&self) -> Option<&StoredDeployment> {
        self.deployments
            .iter()
            .find(|deployment| deployment.state == DEPLOYMENT_STATE_ACTIVE)
    }

    /// The deployment waiting to become the running one, if any — the one in
    /// `ACTIVATING`, of which a service has at most one (see
    /// [`Self::record_deployment`]).
    pub fn next_deployment(&self) -> Option<&StoredDeployment> {
        self.deployments
            .iter()
            .find(|deployment| deployment.state == DEPLOYMENT_STATE_ACTIVATING)
    }

    /// Add `deployment` to this service's history as the deployment WAITING to
    /// become the running one.
    ///
    /// A service launches one deployment at a time, so a deployment that was
    /// still `ACTIVATING` is superseded by this one: it was created, and it is
    /// not running, which is what `INACTIVE` reports. The history is kept
    /// newest-version-first and bounded by [`MAX_DEPLOYMENTS_PER_SERVICE`],
    /// past which the OLDEST deployment is the one dropped.
    pub fn record_deployment(&mut self, deployment: StoredDeployment) {
        for superseded in self.deployments.iter_mut() {
            if superseded.state == DEPLOYMENT_STATE_ACTIVATING {
                superseded.state = DEPLOYMENT_STATE_INACTIVE;
            }
        }
        self.deployments.push_front(deployment);
        while self.deployments.len() > MAX_DEPLOYMENTS_PER_SERVICE {
            if let Some(evicted) = self.deployments.pop_back() {
                // A deployment the store had to forget stops being reported
                // before anything replaced it, so the eviction is traceable
                // rather than a silent surprise.
                ::tracing::warn!(
                    service = %self.name,
                    version = evicted.version,
                    limit = MAX_DEPLOYMENTS_PER_SERVICE,
                    "the oldest deployment was dropped to keep this container service within \
                     the store's bound"
                );
            }
        }
    }

    /// This service's deployments as the modelled `ContainerServiceDeploymentList`.
    ///
    /// Newest version first: the operation's own documentation says "the newest
    /// version is listed at the top of the response".
    pub fn deployments_view(&self) -> Vec<ContainerServiceDeployment> {
        self.deployments
            .iter()
            .map(StoredDeployment::view)
            .collect()
    }

    /// Every deployment of this service that BECAME ACTIVE and the store still
    /// holds, newest version first: the one running now and the ones it
    /// superseded.
    ///
    /// A deployment still `ACTIVATING` is NOT among them, and neither is one
    /// that was replaced before it ever activated: neither launched a container.
    pub fn deployments_that_ran(&self) -> impl Iterator<Item = &StoredDeployment> {
        self.deployments
            .iter()
            .filter(|deployment| deployment.activated)
    }

    /// Whether `container` is one that is running, or previously ran, on this
    /// service — i.e. one that a deployment of it that became active declares.
    pub fn has_run_container(&self, container: &str) -> bool {
        self.deployments_that_ran()
            .any(|deployment| deployment.containers.contains_key(container))
    }

    /// The name of every container that is running, or previously ran, on this
    /// service, in one stable order and without repeats (a container of the same
    /// name in two deployments is one container of this service).
    ///
    /// Bounded by [`MAX_CONTAINERS_PER_DEPLOYMENT`] per deployment the store
    /// keeps, so the set is bounded however many deployments the service has had.
    pub fn containers_that_ran(&self) -> Vec<&str> {
        self.deployments_that_ran()
            .flat_map(|deployment| deployment.containers.keys().map(String::as_str))
            .collect::<BTreeSet<&str>>()
            .into_iter()
            .collect()
    }
}

/// The deployment `request` describes, at `version`, or a refusal naming the
/// member that is wrong.
///
/// A deployment launches containers, so it must name at least one, and each must
/// name the image it runs. A public endpoint must name one of those containers,
/// on a port that container opens, its health check is filled in with the
/// documented defaults for whatever the request left out, and every health-check
/// member the request did fill in is checked against the value domain that
/// member's own documentation states.
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
        // A deployment the request just described has not launched anything
        // yet: the clock activates it.
        activated: false,
        containers,
        public_endpoint,
        created_at_ms: now_ms,
    })
}

/// The health check `request` asks for, with every member it left out filled in
/// from that member's documented default, or a refusal naming the member whose
/// value falls outside the domain that member's own documentation states.
///
/// Every member that documents a domain is checked against it: the two second
/// counts against their published ranges, and the success codes against the HTTP
/// code domain and list form their member documents (see
/// [`validate_success_codes`]). The thresholds and the path document a default
/// each and no domain at all, so any value the model admits for them is one the
/// service takes.
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
    let success_codes = request
        .and_then(|config| config.success_codes.clone())
        .unwrap_or_else(|| HEALTH_CHECK_DEFAULT_SUCCESS_CODES.to_string());
    validate_success_codes(&success_codes)?;
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
        success_codes: Some(success_codes),
    })
}

/// `codes` read as the documented value domain of a health check's
/// `successCodes`, or a refusal naming what falls outside it.
///
/// The member documents the domain whole: the HTTP codes a container's response
/// is checked against are "values between `200` and `499`", given as "multiple
/// values (for example, `200,202`) or a range of values (for example,
/// `200-299`)". So a value names at least one code; each value it lists is one
/// code or a pair of codes running from the lower one to the higher one, written
/// the way the documented examples write them (decimal digits and the two
/// separators, nothing else); and every code named lies within the documented
/// bounds.
fn validate_success_codes(codes: &str) -> Result<(), String> {
    let (min, max) = HEALTH_CHECK_SUCCESS_CODES_RANGE;
    let malformed = || {
        format!(
            "deployment.publicEndpoint.healthCheck.successCodes must be HTTP codes between \
             {min} and {max}, given as multiple values (for example, \
             200{SUCCESS_CODES_VALUE_SEPARATOR}202) or a range of values (for example, \
             200{SUCCESS_CODES_RANGE_SEPARATOR}299), got '{codes}'"
        )
    };
    let code = |text: &str| -> Result<i32, String> {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed());
        }
        let value: i32 = text.parse().map_err(|_| malformed())?;
        if !(min..=max).contains(&value) {
            return Err(format!(
                "deployment.publicEndpoint.healthCheck.successCodes must be HTTP codes between \
                 {min} and {max}, got {value}"
            ));
        }
        Ok(value)
    };
    for value in codes.split(SUCCESS_CODES_VALUE_SEPARATOR) {
        // A value carrying the range separator is the documented range form, so
        // both of its bounds are codes; anything else is one code on its own.
        match value.split_once(SUCCESS_CODES_RANGE_SEPARATOR) {
            None => {
                code(value)?;
            }
            Some((low, high)) => {
                let (low, high) = (code(low)?, code(high)?);
                if low > high {
                    return Err(format!(
                        "deployment.publicEndpoint.healthCheck.successCodes range \
                         '{low}{SUCCESS_CODES_RANGE_SEPARATOR}{high}' must run from the lower \
                         HTTP code to the higher one"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The public domain names `requested` carries, or a refusal when it carries more
/// than [`MAX_PUBLIC_DOMAIN_NAMES`] of them or one of them is not a domain name.
///
/// This is the ONE owner of the half of the rule the REQUEST decides on its own,
/// whichever operation records those names: how many names the map may carry, and
/// that every name listed under a key is a domain name — the member is "the public
/// domain names to use with the container service, such as example.com and
/// www.example.com" — asked of [`validate_domain_name`], this crate's one checker
/// of that (which is also what refuses the wildcard forms the certificate
/// documentation says are not supported, because `*` is not a character a DNS
/// label carries). Whether the certificate each group of names is keyed by is one
/// the account holds and the certificate authority has issued is decided by the
/// store — see
/// [`ComAmazonawsLightsailLightsail20161128State::unusable_public_domain_certificate`].
///
/// How many names the map carries is bounded BEFORE a single one is examined, so
/// the per-name walk below is bounded by [`MAX_PUBLIC_DOMAIN_NAMES`] rather than
/// by what the caller sent.
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
    for domain in names.values().flatten() {
        validate_domain_name(domain).map_err(|detail| {
            format!(
                "publicDomainNames entry '{domain}' is not a public domain name of a \
                 container service: {detail}"
            )
        })?;
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
/// It is also where an alarm of this store STAYS until something moves it, and
/// the one call that moves one is `TestAlarm` — which puts an alarm into the
/// state its caller asks to test: no
/// metric an alarm can watch has a datapoint at all (this emulator runs no
/// instance, container, load balancer or database workload and receives no
/// measurement of one, which is why `GetInstanceMetricData`,
/// `GetContainerServiceMetricData` and `GetLoadBalancerMetricData` all answer an
/// empty `metricData`; `GetRelationalDatabaseMetricData` is still a scaffold with
/// its own issue), and
/// the modelled `Alarm$state`
/// documentation defines `INSUFFICIENT_DATA` as exactly that — "the metric is not
/// available, or not enough data is available for the metric to determine the
/// alarm state". Nothing here invents an evaluation that no metric data could
/// support.
///
/// A bucket's STORAGE metrics are the one series this store does measure — they
/// are a count and a size of what the bucket itself holds, not a measurement of a
/// running workload, so `GetBucketMetricData` computes them — and no alarm is
/// affected: [`ALARM_METRICS`] carries no bucket row, because the modelled
/// `PutAlarmRequest$metricName` documentation lists no bucket metric.
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
/// resources that can currently be monitored by alarms." All three of those
/// resolve to one of these, each addressed the way its own operations address it
/// — see [`ComAmazonawsLightsailLightsail20161128State::monitored_resource`].
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
    /// unchanged"; and a test sets the state it was asked to test.
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
// Contact methods
// ---------------------------------------------------------------------------

/// The `ContactProtocol` values a contact method is created under, from the
/// modelled enum: an email address, or the mobile phone number SMS text messages
/// are sent to.
pub const CONTACT_PROTOCOL_EMAIL: &str = "Email";
/// See [`CONTACT_PROTOCOL_EMAIL`].
pub const CONTACT_PROTOCOL_SMS: &str = "SMS";

/// `ContactMethodStatus` values this slice uses, from the modelled enum's own
/// documentation: a contact method whose verification has not been completed yet
/// is `PendingVerification`, and one notifications can be sent through is
/// `Valid`.
///
/// The enum's `Invalid` member describes a contact method whose verification
/// EXPIRED ("an attempt was made to verify the contact method, but the
/// verification has expired"), which this store never produces: that expiry is
/// owed to a verification nobody completed, and nothing here completes or
/// abandons one.
pub const CONTACT_METHOD_STATUS_PENDING_VERIFICATION: &str = "PendingVerification";
/// See [`CONTACT_METHOD_STATUS_PENDING_VERIFICATION`].
pub const CONTACT_METHOD_STATUS_VALID: &str = "Valid";

/// The status a contact method of `protocol` is created in.
///
/// Only an EMAIL contact method has a verification to wait on: the modelled
/// `ContactMethodVerificationProtocol` enum — the one
/// `SendContactMethodVerification` takes — declares `Email` and nothing else, so
/// no other protocol has a verification to send, and a contact method of one is
/// usable as created.
pub fn contact_method_initial_status(protocol: &str) -> &'static str {
    if protocol == CONTACT_PROTOCOL_EMAIL {
        CONTACT_METHOD_STATUS_PENDING_VERIFICATION
    } else {
        CONTACT_METHOD_STATUS_VALID
    }
}

/// The Regions that serve the `SMS` contact protocol, from the modelled
/// `CreateContactMethodRequest$protocol` documentation: "The SMS protocol is
/// supported only in the following Amazon Web Services Regions", which then
/// lists exactly these six.
pub const SMS_SUPPORTED_REGIONS: &[&str] = &[
    "us-east-1",
    "us-west-2",
    "eu-west-1",
    "ap-northeast-1",
    "ap-southeast-1",
    "ap-southeast-2",
];

/// Whether `region` serves the `SMS` contact protocol; see
/// [`SMS_SUPPORTED_REGIONS`].
pub fn sms_supported_in(region: &str) -> bool {
    SMS_SUPPORTED_REGIONS.contains(&region)
}

/// The most digits a mobile phone number carries, from the modelled
/// `CreateContactMethodRequest$contactEndpoint` documentation: "Phone numbers
/// that follow this format can have a maximum of 15 digits, and they are
/// prefixed with the plus character (+) and the country code."
pub const MAX_E164_DIGITS: usize = 15;

/// Why `contact_endpoint` is not a destination of `protocol`, or `None` when it
/// is one.
///
/// The model constrains that member's LENGTH alone (1 to 256 characters), so the
/// form each protocol's destination takes is checked here: an `Email` endpoint is
/// an email address, and an `SMS` endpoint is the E.164 phone number that
/// member's own documentation requires.
pub fn contact_endpoint_rejection(protocol: &str, contact_endpoint: &str) -> Option<String> {
    if protocol == CONTACT_PROTOCOL_SMS {
        let digits = contact_endpoint.strip_prefix('+').unwrap_or_default();
        if digits.is_empty()
            || digits.len() > MAX_E164_DIGITS
            || !digits.chars().all(|c| c.is_ascii_digit())
        {
            return Some(format!(
                "contactEndpoint '{contact_endpoint}' is not a mobile phone number in E.164 \
                 format: the plus character and the country code, then at most \
                 {MAX_E164_DIGITS} digits in all"
            ));
        }
        return None;
    }
    if !is_email_address(contact_endpoint) {
        return Some(format!(
            "contactEndpoint '{contact_endpoint}' is not an email address: a local part, an @, \
             and a domain of at least two labels"
        ));
    }
    None
}

/// Whether `value` is an email address: one `@`, a non-empty local part, and a
/// domain of at least two non-empty labels, with no whitespace anywhere.
fn is_email_address(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|label| !label.is_empty())
}

/// The name a contact method is created under: the protocol it sends through and
/// a minted id.
///
/// The caller names no contact method, so the service assigns the name. It
/// satisfies the modelled `ContactMethod$name` pattern `^\w[\w\-]*\w$` and is
/// unique among the account's contact methods, because the id is.
pub fn contact_method_name(protocol: &str, id: &str) -> String {
    format!("{protocol}-{id}")
}

/// One stored contact method: the destination `CreateContactMethod` accepted,
/// the members the service assigned, and the verification status it is in.
#[derive(Debug, Clone)]
pub struct StoredContactMethod {
    /// The `ContactProtocol` this contact method sends through — with the Region
    /// it was created in, this record's store key.
    pub protocol: String,
    /// The destination the request named, kept verbatim.
    pub contact_endpoint: String,
    /// The name this store minted for the contact method; see
    /// [`contact_method_name`].
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// The current `ContactMethodStatus`; see
    /// [`contact_method_initial_status`].
    pub status: String,
    pub tags: Vec<Tag>,
}

impl StoredContactMethod {
    /// This record as the modelled `ContactMethod`.
    pub fn view(&self) -> ContactMethod {
        ContactMethod {
            contact_endpoint: Some(self.contact_endpoint.clone()),
            status: Some(self.status.clone()),
            protocol: Some(self.protocol.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_CONTACT_METHOD.to_string()),
            support_code: Some(self.support_code.clone()),
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

/// The shortest session-stickiness cookie lifetime that attribute admits, in
/// seconds.
///
/// Exactly the source [`DEFAULT_SESSION_STICKINESS_COOKIE_DURATION_SECONDS`]
/// comes from: the Elastic Load Balancing load balancer attribute
/// `stickiness.lb_cookie.duration_seconds`, which Lightsail surfaces as
/// `SessionStickiness_LB_CookieDurationSeconds`, is documented as "The range is
/// 1 second to 1 week (604800 seconds)". Lightsail's own model states only that
/// the value "must be an integer that represents the cookie duration in
/// seconds", so the attribute's published range is what bounds it.
pub const MIN_SESSION_STICKINESS_COOKIE_DURATION_SECONDS: i64 = 1;

/// The longest session-stickiness cookie lifetime that attribute admits, in
/// seconds — the one week of the same published range. See
/// [`MIN_SESSION_STICKINESS_COOKIE_DURATION_SECONDS`].
pub const MAX_SESSION_STICKINESS_COOKIE_DURATION_SECONDS: i64 = 604_800;

/// The health check path `path` describes, or `None` when it describes none.
///
/// `LoadBalancer$healthCheckPath` is a `NonEmptyString` (pattern `\S`), so a
/// path that carries nothing but whitespace is not one any read of this resource
/// could ever answer; and the member is "the path to ping on the target", which
/// the whitespace around it is no part of, so what is stored is the path
/// itself. The one owner of that rule, shared by every operation that takes a
/// health check path from a request.
pub fn health_check_path_value(path: &str) -> Option<String> {
    let trimmed = path.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether this service publishes a TLS security policy named `name`.
///
/// Both `CreateLoadBalancer$tlsPolicyName` and the `TlsPolicyName` attribute of
/// `UpdateLoadBalancerAttribute` document the same rule — "Use the
/// GetLoadBalancerTlsPolicies action to get a list of TLS policy names that you
/// can specify" — so a name that catalog does not report names no policy a load
/// balancer can be given.
pub fn publishes_load_balancer_tls_policy(name: &str) -> bool {
    load_balancer_tls_policy_spec(name).is_some()
}

/// The refusal message for a TLS security policy name this service does not
/// publish, naming the request member that carried it and the names that are
/// available instead.
pub fn unpublished_tls_policy_message(member: &str, name: &str) -> String {
    format!(
        "{member} '{name}' is not a TLS security policy this service publishes: \
         GetLoadBalancerTlsPolicies reports {}",
        reported_load_balancer_tls_policy_names(&load_balancer_tls_policies())
    )
}

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

/// The most SSL/TLS certificates ONE load balancer may hold at a time.
///
/// An AWS quota, not an emulator guard: the modelled
/// `CreateLoadBalancerTlsCertificate$certificateName` documentation states it —
/// "Each Lightsail load balancer can have up to 2 certificates associated with
/// it at one time."
pub const MAX_CERTIFICATES_PER_LOAD_BALANCER: usize = 2;

/// The most SSL/TLS certificates of load balancers this account may hold at a
/// time, across every Region.
///
/// An AWS quota too, from the same documentation: "You can have up to 10
/// certificates in your account at one time."
pub const MAX_LOAD_BALANCER_TLS_CERTIFICATES: usize = 10;

/// The most alternative domains and subdomains ONE load balancer certificate
/// may also cover.
///
/// An AWS quota too, from the modelled
/// `CreateLoadBalancerTlsCertificate$certificateAlternativeNames`
/// documentation: "You can have a maximum of 9 alternative names (in addition
/// to the 1 primary domain)."
pub const MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES: usize = 9;

/// The most ENTRIES an alternative-name list may carry before it is de-duped.
///
/// Emulator resource guard, NOT an AWS quota, and the bound that makes the
/// de-duping affordable: the names a request repeats are only known to be
/// repeats once every entry has been examined, so the number of entries a
/// request may send is capped BEFORE any of that work runs rather than after
/// it. Generous next to the
/// [`MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES`] distinct domains
/// that can survive de-duping; a list carrying more entries than this is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DOMAIN_NAME_LIST_ENTRIES: usize = 100;

/// The alternative domains a certificate actually covers, given the primary
/// domain it is for and the names a request listed.
///
/// "Lightsail will de-dupe the names for you": a name the list repeats, and a
/// name that already IS the certificate's primary domain, add no domain to the
/// certificate. The ONE owner of that reduction, so every operation that
/// stores a certificate's alternative domains stores the same shape. The walk
/// is bounded by [`MAX_DOMAIN_NAME_LIST_ENTRIES`], which every caller checks
/// before reading the list.
pub fn alternative_domain_names(primary: &str, requested: Vec<String>) -> Vec<String> {
    let mut covered: Vec<String> = Vec::new();
    for name in requested {
        if name != primary && !covered.contains(&name) {
            covered.push(name);
        }
    }
    covered
}

/// The alternative domains a certificate covers, given the primary domain it is
/// for and the list the request member `member` carried, or the refusal message
/// that member earned.
///
/// The ONE owner of the rule ANY requested alternative-name list has to
/// satisfy, whichever certificate the request describes: every entry has to be
/// a domain name (which is also what refuses the wildcards the documentation
/// says are not supported, because `*` is not a character a DNS label carries),
/// and the list the de-duping LEAVES has to be within `max_covered` — the
/// maximum both certificate kinds document "in addition to the primary domain",
/// applied to what [`alternative_domain_names`] leaves rather than to a list
/// whose repeats Lightsail itself would have dropped.
///
/// Whether an entry is a repeat is only known once it has been examined, so how
/// many entries the list may carry is bounded HERE, before a single one is
/// looked at: every validation and every de-dupe comparison below is then
/// bounded by [`MAX_DOMAIN_NAME_LIST_ENTRIES`] rather than by what the caller
/// sent.
fn requested_alternative_domain_names(
    member: &str,
    primary: &str,
    requested: Vec<String>,
    max_covered: usize,
) -> Result<Vec<String>, String> {
    if requested.len() > MAX_DOMAIN_NAME_LIST_ENTRIES {
        return Err(format!(
            "{member} carries {} entries, more than the \
             {MAX_DOMAIN_NAME_LIST_ENTRIES} this service reads for one certificate",
            requested.len()
        ));
    }
    for name in &requested {
        validate_domain_name(name).map_err(|detail| {
            format!(
                "{member} entry '{name}' is not an alternative domain of this certificate: \
                 {detail}"
            )
        })?;
    }
    let covered = alternative_domain_names(primary, requested);
    if covered.len() > max_covered {
        return Err(format!(
            "{member} names {} alternative domains, more than the {max_covered} a \
             certificate may have in addition to its primary domain",
            covered.len()
        ));
    }
    Ok(covered)
}

/// The alternative domains a load balancer's SSL/TLS certificate covers for the
/// `certificateDomainName`/`certificateAlternativeNames` pair a request asked
/// for, or the refusal message those members earned.
///
/// The ONE owner of the rule a request describing a certificate of a load
/// balancer has to satisfy, whichever operation carries those members: the
/// primary domain and every alternative name have to be domain names (which is
/// also what refuses the wildcards the documentation says are not supported,
/// because `*` is not a character a DNS label carries), and the de-duped list
/// has to be within the documented
/// [`MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES`].
///
/// "Lightsail will de-dupe the names for you": a name the list repeats, and a
/// name that already IS the primary domain, add no domain to the certificate —
/// so the documented maximum is applied to what [`alternative_domain_names`]
/// leaves rather than to a list whose repeats Lightsail itself would have
/// dropped.
///
/// The entry bound, the per-entry validation and the de-duping are
/// [`requested_alternative_domain_names`]'s, the one owner of that rule for
/// every certificate this store holds; what is this path's own is the member
/// names the refusals have to carry and the maximum this certificate kind
/// documents.
pub fn load_balancer_certificate_alternative_names(
    domain_name: &str,
    requested: Vec<String>,
) -> Result<Vec<String>, String> {
    validate_domain_name(domain_name).map_err(|detail| {
        format!(
            "certificateDomainName '{domain_name}' is not the domain name of a \
             certificate: {detail}"
        )
    })?;
    requested_alternative_domain_names(
        "certificateAlternativeNames",
        domain_name,
        requested,
        MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES,
    )
}

/// The resource portion an ARN of a load balancer's SSL/TLS certificate carries
/// before its id. Mirrors the `Instance/<uuid>` form the modelled
/// `Instance$arn` documentation shows, with this resource's own `ResourceType`
/// value.
pub const LOAD_BALANCER_TLS_CERTIFICATE_ARN_RESOURCE_PREFIX: &str = "LoadBalancerTlsCertificate/";

/// The ARN of the load balancer certificate `id` in `region`. A certificate
/// belongs to the load balancer that holds it, which is a regional resource, so
/// it carries that load balancer's Region.
pub fn load_balancer_tls_certificate_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LOAD_BALANCER_TLS_CERTIFICATE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `LoadBalancerTlsCertificate$status` a certificate this account just
/// asked for is in, from the modelled `LoadBalancerTlsCertificateStatus` enum:
/// Lightsail validates the certificate's domains before the certificate
/// authority issues it. The same spelling as [`CERTIFICATE_STATUS_PENDING_VALIDATION`],
/// which the certificate slice of this store reports.
pub const LOAD_BALANCER_TLS_CERTIFICATE_STATUS_PENDING_VALIDATION: &str =
    CERTIFICATE_STATUS_PENDING_VALIDATION;

/// The `LoadBalancerTlsCertificate$status` of a validated certificate, from the
/// same enum — the terminal status the clock owes a certificate this account
/// asked for.
pub const LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED: &str = CERTIFICATE_STATUS_ISSUED;

/// `LoadBalancerTlsCertificateRenewalSummary$renewalStatus` of an issued
/// certificate, from the modelled `LoadBalancerTlsCertificateRenewalStatus`
/// enum: "Lightsail is attempting to automatically validate the domain names of
/// the certificate. No further action is required." The renewal summary
/// "contains information about the status of Lightsail's managed renewal for the
/// certificate", and there is a renewal to manage only once the certificate
/// authority has issued one — so this is the only renewal status a certificate
/// of this store reports, and a certificate still being validated for the first
/// time reports no renewal summary at all.
pub const LOAD_BALANCER_TLS_CERTIFICATE_RENEWAL_STATUS_PENDING_AUTO_RENEWAL: &str =
    "PENDING_AUTO_RENEWAL";

/// The algorithm the certificate authority signed the certificate with, as
/// `LoadBalancerTlsCertificate$signatureAlgorithm` reports it.
///
/// It follows from [`CERTIFICATE_KEY_ALGORITHM`]: the certificate carries an RSA
/// key, and AWS Certificate Manager — which is what issues a Lightsail SSL/TLS
/// certificate — signs an RSA certificate with SHA-256, so the X.509 signature
/// algorithm is `sha256WithRSAEncryption`, printed in the form a certificate's
/// signature algorithm is reported in.
pub const LOAD_BALANCER_TLS_CERTIFICATE_SIGNATURE_ALGORITHM: &str = "SHA256WITHRSA";

/// The type of the DNS record a domain of a certificate is validated through, as
/// `LoadBalancerTlsCertificateDomainValidationRecord$type` describes it ("The
/// type of validation record. For example, `CNAME` for domain validation").
pub const LOAD_BALANCER_TLS_CERTIFICATE_VALIDATION_RECORD_TYPE: &str = "CNAME";

/// The `subject` of a certificate issued for `domain_name`: "the name of the
/// entity that is associated with the public key contained in the certificate",
/// which for an SSL/TLS certificate is the X.509 subject distinguished name,
/// whose common name is the domain the certificate is for.
fn load_balancer_tls_certificate_subject(domain_name: &str) -> String {
    format!("CN={domain_name}")
}

/// The SSL/TLS certificate a load balancer holds: the one
/// `CreateLoadBalancer`'s certificate members asked for, or one
/// `CreateLoadBalancerTlsCertificate` added.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancerCertificate {
    /// The certificate name the request named, kept verbatim — it is what every
    /// certificate operation addresses this certificate by.
    pub name: String,
    /// The ARN this certificate was minted with, from its own id — see
    /// [`load_balancer_tls_certificate_arn`].
    pub arn: String,
    /// The support code built from that same id, so support can look the
    /// certificate up from the value alone ([`support_code`]).
    pub support_code: String,
    /// The instant the request that asked for this certificate was served.
    pub created_at_ms: i64,
    /// Where the certificate is: the location of the load balancer that holds
    /// it, which is the Region that load balancer — and so this certificate —
    /// belongs to.
    ///
    /// Kept on the record, exactly as a certificate of the account keeps one
    /// ([`StoredCertificate::location`]): it is what the modelled `location`
    /// member reports and what locates the operation-log entry a change to this
    /// certificate writes. A load balancer's Region is half its store key, so
    /// this can never drift from the holder's own location.
    pub location: StoredLocation,
    /// The domain the certificate is associated with.
    pub domain_name: String,
    /// The alternative domains and subdomains the certificate also covers, as
    /// [`alternative_domain_names`] reduced them — every writer of this field
    /// goes through that one owner, so a repeat and the primary domain are
    /// never among them.
    pub alternative_names: Vec<String>,
    /// Whether the certificate is attached to the load balancer, which is what
    /// `LoadBalancerTlsCertificateSummary$isAttached` reports. A certificate is
    /// attached only by `AttachLoadBalancerTlsCertificate`, never by the call
    /// that requested it: it has to be validated first.
    pub is_attached: bool,
    /// The tags the request asked for, kept verbatim — "the tag keys and
    /// optional values to add to the resource during create".
    pub tags: Vec<Tag>,
    /// The serial number the certificate was minted with, as
    /// `LoadBalancerTlsCertificate$serial` reports it.
    pub serial: String,
    /// The current `LoadBalancerTlsCertificateStatus`.
    pub status: &'static str,
    /// The status the clock owes this certificate, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instant the certificate authority issues this certificate — the same
    /// instant the clock owes it
    /// [`LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED`] — which is what
    /// `issuedAt`, `notBefore` and `notAfter` are reported from once it is
    /// issued.
    pub issued_at_ms: i64,
}

impl StoredLoadBalancerCertificate {
    /// Whether the certificate authority has issued this certificate.
    pub fn is_issued(&self) -> bool {
        self.status == LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED
    }

    /// Every domain this certificate covers: the primary domain it is for, then
    /// each alternative domain it also covers.
    ///
    /// The alternative domains went through [`alternative_domain_names`] before
    /// they were stored, so none of them repeats and none of them is the
    /// primary domain — this list is the set of domains the certificate secures
    /// traffic for, with its primary domain first.
    pub fn domain_names(&self) -> Vec<String> {
        let mut names = vec![self.domain_name.clone()];
        names.extend(self.alternative_names.iter().cloned());
        names
    }

    /// The `LoadBalancerTlsCertificateDomainStatus` every domain of this
    /// certificate is in: a certificate the authority has issued is one whose
    /// domains Lightsail validated, and one it has not issued yet is still
    /// waiting for exactly that.
    fn domain_validation_status(&self) -> &'static str {
        if self.is_issued() {
            DOMAIN_VALIDATION_STATUS_SUCCESS
        } else {
            DOMAIN_VALIDATION_STATUS_PENDING
        }
    }

    /// This record as the modelled `LoadBalancerTlsCertificate`, on the load
    /// balancer `load_balancer_name` — the load balancer that holds it, which is
    /// what `loadBalancerName` reports and which the certificate is reached
    /// through. `location` comes from this record: it is the location of that
    /// same load balancer, kept here because a change to this certificate has
    /// to locate its own log entry.
    pub fn view(&self, load_balancer_name: &str) -> LoadBalancerTlsCertificate {
        let issued = self.is_issued();
        let validation_status = self.domain_validation_status();
        let domain_names = self.domain_names();
        LoadBalancerTlsCertificate {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE.to_string()),
            tags: Some(self.tags.clone()),
            load_balancer_name: Some(load_balancer_name.to_string()),
            is_attached: Some(self.is_attached),
            status: Some(self.status.to_string()),
            domain_name: Some(self.domain_name.clone()),
            // One record per domain the certificate covers, each carrying the
            // domain it is about, the kind of DNS record that validates it and
            // the state that validation is in. `name` is that domain because
            // that is what the member documents it to be: "A fully qualified
            // domain name in the certificate. For example, `example.com`."
            // `value` is left out and so is `dnsRecordCreationState`: this
            // emulator owns no DNS zone to publish a validation record in, so
            // there is no value a caller could create and no creation of one to
            // report — the same choice the certificate slice of this store
            // makes ([`StoredCertificate::detail`]).
            domain_validation_records: Some(
                domain_names
                    .iter()
                    .map(
                        |domain_name| LoadBalancerTlsCertificateDomainValidationRecord {
                            name: Some(domain_name.clone()),
                            r#type: Some(
                                LOAD_BALANCER_TLS_CERTIFICATE_VALIDATION_RECORD_TYPE.to_string(),
                            ),
                            domain_name: Some(domain_name.clone()),
                            validation_status: Some(validation_status.to_string()),
                            ..Default::default()
                        },
                    )
                    .collect(),
            ),
            // Nothing here fails a validation, so no certificate of this store
            // carries a `failureReason`; and none is revoked, which is what
            // `revocationReason` and `revokedAt` are "present only when" —
            // their status is `REVOKED`.
            failure_reason: None,
            revocation_reason: None,
            revoked_at: None,
            // Nothing was issued while the certificate's domains were still
            // being validated, so none of the issuance members is reported
            // until it is: a certificate has no issuer, no validity window and
            // no issue date before the certificate authority gives it one.
            issued_at: issued.then(|| timestamp(self.issued_at_ms)),
            issuer: issued.then(|| CERTIFICATE_ISSUER_CA.to_string()),
            not_before: issued.then(|| timestamp(self.issued_at_ms)),
            not_after: issued
                .then(|| timestamp(self.issued_at_ms.saturating_add(CERTIFICATE_VALIDITY_MILLIS))),
            key_algorithm: Some(CERTIFICATE_KEY_ALGORITHM.to_string()),
            signature_algorithm: Some(
                LOAD_BALANCER_TLS_CERTIFICATE_SIGNATURE_ALGORITHM.to_string(),
            ),
            serial: Some(self.serial.clone()),
            subject: Some(load_balancer_tls_certificate_subject(&self.domain_name)),
            // Lightsail manages the renewal of the certificate it ISSUED:
            // nothing renews a certificate the authority has not issued yet, so
            // a certificate still being validated for the first time reports no
            // renewal summary. The renewal is of the same domains, so its
            // options carry one entry per domain, in the same order the
            // validation records are reported in.
            renewal_summary: issued.then(|| LoadBalancerTlsCertificateRenewalSummary {
                renewal_status: Some(
                    LOAD_BALANCER_TLS_CERTIFICATE_RENEWAL_STATUS_PENDING_AUTO_RENEWAL.to_string(),
                ),
                domain_validation_options: Some(
                    domain_names
                        .iter()
                        .map(
                            |domain_name| LoadBalancerTlsCertificateDomainValidationOption {
                                domain_name: Some(domain_name.clone()),
                                validation_status: Some(validation_status.to_string()),
                            },
                        )
                        .collect(),
                ),
            }),
            // The X.509 subject alternative name extension of the certificate,
            // which covers every domain it secures traffic for — its primary
            // domain among them, because that is the domain its `subject`
            // names. Last, because it takes the one list of covered domains
            // every member above reads.
            subject_alternative_names: Some(domain_names),
        }
    }
}

/// One instance registered with a load balancer: the instance it names, and the
/// instant its deregistration is due while one is outstanding.
///
/// A registration is a record of the load balancer, not of the instance: it is
/// what `instanceHealthSummary` reports one entry per.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancerRegistration {
    /// The instance name the registration was made for, kept verbatim — it is
    /// what `InstanceHealthSummary$instanceName` reports.
    pub name: String,
    /// The instant the clock owes this registration its removal, while a
    /// deregistration is outstanding. While it is set the instance is still
    /// registered and still reported, in the modelled `draining` health state.
    pub drains_at_ms: Option<i64>,
}

impl StoredLoadBalancerRegistration {
    /// Whether this registration is being deregistered.
    pub fn is_draining(&self) -> bool {
        self.drains_at_ms.is_some()
    }

    /// This registration as the modelled `InstanceHealthSummary`.
    fn view(&self) -> InstanceHealthSummary {
        // A registered instance starts out `initial`, and the modelled
        // `instanceHealthReason` documentation gives the reason for that:
        // "`Lb.RegistrationInProgress` - The target instance is in the process
        // of being registered with the load balancer." This emulator runs no
        // health checks, so no instance leaves it except by being deregistered,
        // which the same enum names: "`Instance.DeregistrationInProgress` - The
        // target instance is in the process of being deregistered".
        let (health, reason) = if self.is_draining() {
            (
                INSTANCE_HEALTH_DRAINING,
                INSTANCE_HEALTH_REASON_DEREGISTRATION_IN_PROGRESS,
            )
        } else {
            (
                INSTANCE_HEALTH_INITIAL,
                INSTANCE_HEALTH_REASON_REGISTRATION_IN_PROGRESS,
            )
        };
        InstanceHealthSummary {
            instance_name: Some(self.name.clone()),
            instance_health: Some(health.to_string()),
            instance_health_reason: Some(reason.to_string()),
        }
    }
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
    /// The SSL/TLS certificates this load balancer holds, in the order they
    /// were added — at most [`MAX_CERTIFICATES_PER_LOAD_BALANCER`] of them.
    pub certificates: Vec<StoredLoadBalancerCertificate>,
    /// The instances registered with this load balancer, in registration order
    /// — what `instanceHealthSummary` reports one entry per.
    pub attached_instances: Vec<StoredLoadBalancerRegistration>,
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

/// ONE of a load balancer's configuration options, with the value already
/// resolved out of the `attributeValue` string the request carried.
///
/// The modelled `attributeValue` is a string whatever the attribute is, and each
/// `LoadBalancerAttributeName` documents its own value domain — a ping path, a
/// boolean, a number of seconds, a published TLS policy name. This is what a
/// value of one of those domains looks like once it has been read out of that
/// string, so the store never holds a value the attribute does not admit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadBalancerAttribute {
    /// `HealthCheckPath` — "the path to ping on the target".
    HealthCheckPath(String),
    /// `SessionStickinessEnabled` — whether session stickiness is active.
    SessionStickinessEnabled(bool),
    /// `SessionStickiness_LB_CookieDurationSeconds` — "the cookie duration in
    /// seconds".
    SessionStickinessCookieDurationSeconds(i64),
    /// `HttpsRedirectionEnabled` — whether HTTP to HTTPS redirection is active.
    HttpsRedirectionEnabled(bool),
    /// `TlsPolicyName` — "the name of the TLS policy".
    TlsPolicyName(String),
}

impl StoredLoadBalancer {
    /// Whether an SSL/TLS certificate is ATTACHED to this load balancer, which
    /// is what decides its protocol and its public ports.
    fn serves_https(&self) -> bool {
        self.certificates
            .iter()
            .any(|certificate| certificate.is_attached)
    }

    /// Register the instance `name` with this load balancer.
    ///
    /// An instance is registered with a load balancer at most once — the
    /// modelled `instanceHealthSummary` carries one entry per registered
    /// instance — so registering one that is already registered leaves its
    /// place in the registration order as it is. A registration that was
    /// DRAINING stops draining: the instance the deregistration was going to
    /// remove is one this call asks the load balancer to serve again.
    pub fn register_instance(&mut self, name: &str) {
        match self
            .attached_instances
            .iter_mut()
            .find(|registration| registration.name == name)
        {
            Some(registration) => registration.drains_at_ms = None,
            None => self.attached_instances.push(StoredLoadBalancerRegistration {
                name: name.to_string(),
                drains_at_ms: None,
            }),
        }
    }

    /// Begin the deregistration of the instance `name`, due at `at_ms`, and
    /// answer the instant its removal is owed — or `None` when that instance is
    /// not registered with this load balancer and there is nothing to remove.
    ///
    /// A deregistration already in flight keeps the instant it was already owed
    /// rather than pushing it further out, so repeating the call cannot hold an
    /// instance registered for longer.
    pub fn start_deregistration(&mut self, name: &str, at_ms: i64) -> Option<i64> {
        let registration = self
            .attached_instances
            .iter_mut()
            .find(|registration| registration.name == name)?;
        Some(*registration.drains_at_ms.get_or_insert(at_ms))
    }

    /// The certificate `name` names among the ones this load balancer holds.
    ///
    /// A load balancer's certificate is addressed by its own name among THIS
    /// load balancer's certificates, which is how every certificate operation
    /// reaches one: the same name may name a certificate of another load
    /// balancer, and the two are different records.
    pub fn certificate(&self, name: &str) -> Option<&StoredLoadBalancerCertificate> {
        self.certificate_index(name)
            .and_then(|index| self.certificates.get(index))
    }

    /// WHERE the certificate `name` names sits among the ones this load
    /// balancer holds — what an operation that REMOVES one needs. The one
    /// lookup rule, shared with [`Self::certificate`].
    pub fn certificate_index(&self, name: &str) -> Option<usize> {
        self.certificates
            .iter()
            .position(|certificate| certificate.name == name)
    }

    /// Set ONE of this load balancer's configuration options to `attribute`.
    ///
    /// The counterpart of [`Self::configuration_options`]: every
    /// `LoadBalancerAttributeName` this record reports a value for is written
    /// here and read there, so the option a caller sets is the one the next read
    /// answers. "You can only update one attribute at a time", which is why this
    /// takes exactly one.
    pub fn set_attribute(&mut self, attribute: LoadBalancerAttribute) {
        match attribute {
            LoadBalancerAttribute::HealthCheckPath(path) => self.health_check_path = path,
            LoadBalancerAttribute::SessionStickinessEnabled(enabled) => {
                self.session_stickiness_enabled = enabled;
            }
            LoadBalancerAttribute::SessionStickinessCookieDurationSeconds(seconds) => {
                self.session_stickiness_cookie_duration_seconds = seconds;
            }
            LoadBalancerAttribute::HttpsRedirectionEnabled(enabled) => {
                self.https_redirection_enabled = enabled;
            }
            LoadBalancerAttribute::TlsPolicyName(name) => self.tls_policy_name = Some(name),
        }
    }

    /// Make the certificate `name` the one this load balancer terminates HTTPS
    /// with — the ONE owner of that rule, as
    /// [`ComAmazonawsLightsailLightsail20161128State::issued_certificate`] is for
    /// the certificates of the account.
    ///
    /// A load balancer terminates HTTPS with ONE certificate, so attaching one
    /// detaches whichever was attached: "Use the AttachLoadBalancerTlsCertificate
    /// action with the non-attached certificate, and it will replace the existing
    /// one and become the attached certificate" (the modelled
    /// `AttachLoadBalancerTlsCertificate` documentation). Attaching the one
    /// already attached leaves the record in the state the request asks for.
    ///
    /// The certificate has to be one this load balancer holds AND one the
    /// certificate authority has already issued — "Once you create and validate
    /// your certificate, you can attach it to your load balancer": a
    /// certificate still being validated secures nothing, so there is no HTTPS
    /// traffic the load balancer could terminate with it. Both are decided
    /// BEFORE a single certificate is written — the name through
    /// [`Self::certificate_index`] and the issuance through
    /// [`StoredLoadBalancerCertificate::is_issued`] — so a refused request
    /// cannot leave an attachment cleared or move one. Both walks are bounded by
    /// [`MAX_CERTIFICATES_PER_LOAD_BALANCER`].
    pub fn attach_certificate(&mut self, name: &str) -> Result<(), CertificateNotUsable> {
        let index = self
            .certificate_index(name)
            .ok_or(CertificateNotUsable::DoesNotExist)?;
        let requested = &self.certificates[index];
        if !requested.is_issued() {
            return Err(CertificateNotUsable::NotIssued {
                status: requested.status,
            });
        }
        for certificate in &mut self.certificates {
            certificate.is_attached = certificate.name == name;
        }
        Ok(())
    }

    /// Every SSL/TLS certificate this load balancer holds, as the modelled
    /// `LoadBalancerTlsCertificate`, in the order they were added — what
    /// `GetLoadBalancerTlsCertificates` reports.
    ///
    /// A certificate is reported on the load balancer that holds it, so each
    /// view carries this load balancer's name, and the location each record
    /// keeps is this load balancer's own; the walk is bounded by
    /// [`MAX_CERTIFICATES_PER_LOAD_BALANCER`].
    pub fn certificate_views(&self) -> Vec<LoadBalancerTlsCertificate> {
        self.certificates
            .iter()
            .map(|certificate| certificate.view(&self.name))
            .collect()
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
            // One entry per registered instance, in registration order, each
            // reporting the health its own registration is in (see
            // [`StoredLoadBalancerRegistration::view`]).
            instance_health_summary: Some(
                self.attached_instances
                    .iter()
                    .map(StoredLoadBalancerRegistration::view)
                    .collect(),
            ),
            tls_certificate_summaries: Some(
                self.certificates
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

/// The `InstanceHealthState` an instance whose deregistration is outstanding is
/// in, from the modelled enum: it is still registered and still finishing the
/// requests it holds, which is what `DetachInstancesFromLoadBalancer`'s own
/// documentation describes ("This operation waits until the instances are no
/// longer needed before they are detached from the load balancer").
pub const INSTANCE_HEALTH_DRAINING: &str = "draining";
/// See [`INSTANCE_HEALTH_DRAINING`] — the modelled `InstanceHealthReason` value
/// for it.
pub const INSTANCE_HEALTH_REASON_DEREGISTRATION_IN_PROGRESS: &str =
    "Instance.DeregistrationInProgress";

/// The unit the load balancer metric `metric_name` is published in, or `None`
/// when this service publishes no such metric for a load balancer.
///
/// Every member of the modelled `LoadBalancerMetricName` enum — which is what
/// `GetLoadBalancerMetricData`'s boundary walk admits — is also a metric an alarm
/// can watch on a load balancer, and the "published `unit` value" each metric's
/// own documentation states is already the `unit` of that metric's
/// [`ALARM_METRICS`] row, so this reads that one catalog rather than restating
/// it. (`INSTANCE_METRIC_UNITS` is its own list only because an instance
/// publishes a metric no alarm can watch.) Source:
/// `smithy_query(query="doc",shape="GetLoadBalancerMetricDataRequest",member="metricName")`.
pub fn load_balancer_metric_unit(metric_name: &str) -> Option<&'static str> {
    alarm_metric(RESOURCE_TYPE_LOAD_BALANCER, metric_name).map(|spec| spec.unit)
}

// ---------------------------------------------------------------------------
// Load balancer TLS security policies
// ---------------------------------------------------------------------------

use crate::ops::types::LoadBalancerTlsPolicy;

/// How many TLS security policies one `GetLoadBalancerTlsPolicies` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that "a next
/// page token is not returned if there are no more results to display". Matches
/// [`LOAD_BALANCER_PAGE_SIZE`], so every listing of this slice pages the same
/// way, and it is at least the number of policies this service publishes, so the
/// catalog is answered whole in one page — as AWS answers it — while the paging
/// walk still holds for a catalog that outgrows it.
pub const LOAD_BALANCER_TLS_POLICY_PAGE_SIZE: usize = 5;

/// The protocol versions a published policy may negotiate, oldest first.
///
/// SOURCE: the wire tokens the recorded `GetLoadBalancerTlsPolicies` answer
/// carries for a policy's protocol versions — the `Protocol-` prefixed form of
/// each TLS version, which is the value a client echoes back when configuring a
/// load balancer, so `LoadBalancerTlsPolicy$protocols` ("The protocols used in a
/// given TLS security policy") is not interchangeable with the bare version
/// name. The capitalisation is the recorded one, which differs between the
/// tokens.
const TLS_PROTOCOL_1_0: &str = "Protocol-TLSv1";
/// See [`TLS_PROTOCOL_1_0`].
const TLS_PROTOCOL_1_1: &str = "Protocol-TLSV1.1";
/// See [`TLS_PROTOCOL_1_0`].
const TLS_PROTOCOL_1_2: &str = "Protocol-TLSv1.2";

/// Every protocol version a policy that still accepts TLS 1.0 negotiates.
const TLS_PROTOCOLS_1_0_AND_LATER: &[&str] =
    &[TLS_PROTOCOL_1_0, TLS_PROTOCOL_1_1, TLS_PROTOCOL_1_2];
/// The only protocol version a TLS 1.2 policy negotiates.
const TLS_PROTOCOLS_1_2_ONLY: &[&str] = &[TLS_PROTOCOL_1_2];

/// The cipher suites the DEFAULT policy offers, in order of preference — the
/// suites of the load-balancing service's own 2016-08 security policy, which the
/// `TLS-2016-08` Lightsail policy is.
///
/// "The ciphers are listed in order of preference"
/// (`LoadBalancerTlsPolicy$ciphers`), so this order is part of the published
/// data: the ephemeral-key (`ECDHE`) suites come first, 128-bit before 256-bit
/// within each key exchange, and the static-RSA suites last.
const TLS_DEFAULT_CIPHERS: &[&str] = &[
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES128-SHA256",
    "ECDHE-RSA-AES128-SHA256",
    "ECDHE-ECDSA-AES128-SHA",
    "ECDHE-RSA-AES128-SHA",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
    "ECDHE-ECDSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA",
    "ECDHE-ECDSA-AES256-SHA",
    "AES128-GCM-SHA256",
    "AES128-SHA256",
    "AES128-SHA",
    "AES256-GCM-SHA384",
    "AES256-SHA256",
    "AES256-SHA",
];

/// The cipher suites a RESTRICTED forward-secrecy policy offers: the
/// ephemeral-key (`ECDHE`) suites of [`TLS_DEFAULT_CIPHERS`], in the same order
/// of preference and without its static-RSA ones — a session key those cannot
/// leak with the server's private key is what forward secrecy means — kept to
/// the suites whose handshake and record MAC use SHA-256 or SHA-384, so the
/// SHA-1 suites are left out.
const TLS_FORWARD_SECRECY_RESTRICTED_CIPHERS: &[&str] = &[
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES128-SHA256",
    "ECDHE-RSA-AES128-SHA256",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
    "ECDHE-ECDSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA384",
];

/// One TLS security policy a load balancer of this service can be given: the
/// protocol versions it negotiates and the cipher suites it offers, in order of
/// preference.
///
/// This is PUBLISHED data, not per-account state: every account sees the same
/// policies and no operation in this service creates, changes or withdraws one.
/// So it is a `const` table beside this service's other published catalogs
/// (instance bundles, blueprints, Regions), `GetLoadBalancerTlsPolicies` reports
/// it without reading or writing a stored resource, and
/// `CreateLoadBalancer$tlsPolicyName` names an entry of it — "Use the
/// GetLoadBalancerTlsPolicies action to get a list of TLS policy names that you
/// can specify".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadBalancerTlsPolicySpec {
    /// `LoadBalancerTlsPolicy$name` — "The name of the TLS security policy", and
    /// the value a load balancer's `tlsPolicyName` carries.
    pub name: &'static str,
    /// `LoadBalancerTlsPolicy$isDefault` — "A Boolean value that indicates
    /// whether the TLS security policy is the default", so exactly one entry of
    /// the catalog carries it.
    pub is_default: bool,
    /// `LoadBalancerTlsPolicy$protocols` — "The protocols used in a given TLS
    /// security policy", oldest first.
    pub protocols: &'static [&'static str],
    /// `LoadBalancerTlsPolicy$ciphers` — "The ciphers used by the TLS security
    /// policy", which "are listed in order of preference".
    pub ciphers: &'static [&'static str],
}

/// Every TLS security policy this service publishes, the default first and then
/// the forward-secrecy policy.
///
/// SOURCE: the Lightsail TLS security policies of the Developer Guide's
/// "Configuring TLS security policies on your Amazon Lightsail load balancers",
/// which the recorded `GetLoadBalancerTlsPolicies` answer reports — the default
/// `TLS-2016-08` policy and the restricted forward-secrecy (`FS`) policy, whose
/// name carries the protocol floor (`1-2`), the restricted cipher set (`Res`)
/// and the date of the policy. A policy the service does not publish has no row
/// here: a client offering one would configure a policy the service does not
/// have. Each row is the load-balancing service's own security policy of that
/// name, so its protocol versions and its ordered cipher suites are that
/// policy's.
pub const LOAD_BALANCER_TLS_POLICY_SPECS: &[LoadBalancerTlsPolicySpec] = &[
    LoadBalancerTlsPolicySpec {
        name: "TLS-2016-08",
        is_default: true,
        protocols: TLS_PROTOCOLS_1_0_AND_LATER,
        ciphers: TLS_DEFAULT_CIPHERS,
    },
    LoadBalancerTlsPolicySpec {
        name: "TLS-FS-1-2-Res-2019-08",
        is_default: false,
        protocols: TLS_PROTOCOLS_1_2_ONLY,
        ciphers: TLS_FORWARD_SECRECY_RESTRICTED_CIPHERS,
    },
];

/// The TLS version a protocol token names, without the wire prefix the token
/// carries: the token from its first digit onward (`Protocol-TLSv1.2` -> `1.2`).
fn tls_protocol_version(protocol: &str) -> &str {
    protocol
        .find(|character: char| character.is_ascii_digit())
        .map_or(protocol, |first_digit| &protocol[first_digit..])
}

/// The TLS versions of a policy, as one English list: the items separated by
/// commas with `and` before the last, and the serial comma once there are three
/// or more.
fn tls_version_list(versions: &[&str]) -> String {
    match versions {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first, last] => format!("{first} and {last}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// The description a policy reports.
///
/// DERIVED from the policy's own protocol versions rather than carried per
/// entry, so a policy added to [`LOAD_BALANCER_TLS_POLICY_SPECS`] is described
/// the same way: the sentence states which TLS versions the policy enables, the
/// one fact the recorded descriptions carry — `Enables TLS versions 1, 1.1, and
/// 1.2` for the policy that admits all three, and `Enables only TLS version 1.2`
/// for one that admits a single version.
pub fn load_balancer_tls_policy_description(protocols: &[&str]) -> String {
    let versions: Vec<&str> = protocols
        .iter()
        .map(|protocol| tls_protocol_version(protocol))
        .collect();
    match versions.as_slice() {
        [] => "Enables no TLS version".to_string(),
        [only] => format!("Enables only TLS version {only}"),
        several => format!("Enables TLS versions {}", tls_version_list(several)),
    }
}

impl LoadBalancerTlsPolicySpec {
    /// This policy as the modelled `LoadBalancerTlsPolicy`.
    ///
    /// `description` is DERIVED by [`load_balancer_tls_policy_description`] from
    /// this entry's protocols rather than carried a second time.
    pub fn view(&self) -> LoadBalancerTlsPolicy {
        LoadBalancerTlsPolicy {
            name: Some(self.name.to_string()),
            is_default: Some(self.is_default),
            description: Some(load_balancer_tls_policy_description(self.protocols)),
            protocols: Some(self.protocols.iter().map(|p| (*p).to_string()).collect()),
            ciphers: Some(self.ciphers.iter().map(|c| (*c).to_string()).collect()),
        }
    }
}

/// The policies a `GetLoadBalancerTlsPolicies` call reports out of `specs`, in
/// `specs`' own order, so two reads answer identically.
///
/// Taking the catalog as an argument is what lets the reporting rule be
/// exercised against catalogs the published one is only one of.
pub fn load_balancer_tls_policies_from(
    specs: &[LoadBalancerTlsPolicySpec],
) -> Vec<LoadBalancerTlsPolicy> {
    specs.iter().map(LoadBalancerTlsPolicySpec::view).collect()
}

/// The policies a `GetLoadBalancerTlsPolicies` call reports:
/// [`load_balancer_tls_policies_from`] over this service's published catalog,
/// [`LOAD_BALANCER_TLS_POLICY_SPECS`].
pub fn load_balancer_tls_policies() -> Vec<LoadBalancerTlsPolicy> {
    load_balancer_tls_policies_from(LOAD_BALANCER_TLS_POLICY_SPECS)
}

/// One page of `reported`, by the same token walk every published catalog of this
/// service uses ([`catalog_page`]): a page token is the policy name a previous
/// page reported as `nextPageToken`.
pub fn load_balancer_tls_policy_page(
    reported: &[LoadBalancerTlsPolicy],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<LoadBalancerTlsPolicy>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |policy| {
        policy.name.as_deref().map(std::borrow::Cow::Borrowed)
    })
}

/// The policy names `reported` holds, for a refusal that has to tell the caller
/// what a page token may name.
pub fn reported_load_balancer_tls_policy_names(reported: &[LoadBalancerTlsPolicy]) -> String {
    reported
        .iter()
        .filter_map(|policy| policy.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The published TLS security policy `name` names, or `None` when this service
/// publishes no such policy.
pub fn load_balancer_tls_policy_spec(name: &str) -> Option<LoadBalancerTlsPolicySpec> {
    LOAD_BALANCER_TLS_POLICY_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .copied()
}

// ---------------------------------------------------------------------------
// SSL/TLS certificates and CDN distributions
// ---------------------------------------------------------------------------

/// The `resourceType` a certificate and its operations report, from the modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_CERTIFICATE: &str = "Certificate";

/// The `resourceType` a content delivery network distribution reports, from the
/// modelled `ResourceType` enum — the value the
/// `LightsailDistribution$resourceType` documentation names ("The Lightsail
/// resource type (`Distribution`)").
pub const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The resource portion a certificate ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const CERTIFICATE_ARN_RESOURCE_PREFIX: &str = "Certificate/";

/// The `CertificateStatus` a certificate this account just requested is in, from
/// the modelled enum: Lightsail has to validate the domain before the
/// certificate authority issues it.
pub const CERTIFICATE_STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The `CertificateStatus` of a validated certificate, from the modelled enum.
/// `AttachCertificateToDistribution`'s `certificateName` documentation says
/// "Only certificates with a status of ISSUED can be attached to a
/// distribution".
pub const CERTIFICATE_STATUS_ISSUED: &str = "ISSUED";

/// The `CertificateDomainValidationStatus` of a domain whose validation is still
/// outstanding, from the modelled enum.
pub const DOMAIN_VALIDATION_STATUS_PENDING: &str = "PENDING_VALIDATION";

/// The `CertificateDomainValidationStatus` of a validated domain, from the
/// modelled enum.
pub const DOMAIN_VALIDATION_STATUS_SUCCESS: &str = "SUCCESS";

/// The algorithm a Lightsail certificate's key pair is generated with, as
/// `Certificate$keyAlgorithm` reports it.
///
/// A Lightsail SSL/TLS certificate is an AWS Certificate Manager certificate,
/// and AWS documents `RSA_2048` as the key algorithm ACM requests a certificate
/// with unless the caller asks for another — and Lightsail's `CreateCertificate`
/// takes no key-algorithm parameter, so there is no other to ask for.
pub const CERTIFICATE_KEY_ALGORITHM: &str = "RSA_2048";

/// The certificate authority that issued the certificate, as
/// `Certificate$issuerCA` reports it: Amazon's own, because Lightsail requests
/// the certificate from AWS Certificate Manager rather than from a CA the caller
/// names.
pub const CERTIFICATE_ISSUER_CA: &str = "Amazon";

/// How long an issued certificate stays valid: `notAfter` is `notBefore` plus
/// this span.
///
/// A DOCUMENTED duration, not a chosen one: AWS documents that "ACM
/// certificates are valid for 13 months (395 days)", and a Lightsail
/// certificate is an ACM certificate.
pub const CERTIFICATE_VALIDITY_MILLIS: i64 = 395 * 24 * 60 * 60 * 1_000;

/// The most alternate domains one certificate may carry.
///
/// A DOCUMENTED limit, not a guard: the modelled
/// `CreateCertificate$subjectAlternativeNames` documentation says "You can
/// specify a maximum of nine alternate domains (in addition to the primary
/// domain name)".
pub const MAX_SUBJECT_ALTERNATIVE_NAMES: usize = 9;

/// The alternate domains an account's SSL/TLS certificate covers for the
/// `domainName`/`subjectAlternativeNames` pair a request asked for, or the
/// refusal message that list earned.
///
/// The rule is [`requested_alternative_domain_names`]' — the one owner of it
/// for every certificate this store holds — with this certificate kind's own
/// member name and its own documented maximum: a name the list repeats, and a
/// name that already IS the certificate's primary domain, add no domain to the
/// certificate, so the documented maximum is applied to what the de-duping
/// leaves and what is STORED is that reduced list.
///
/// The primary domain is validated by the caller, which has its own
/// `domainName` refusal to answer with.
pub fn certificate_subject_alternative_names(
    domain_name: &str,
    requested: Vec<String>,
) -> Result<Vec<String>, String> {
    requested_alternative_domain_names(
        "subjectAlternativeNames",
        domain_name,
        requested,
        MAX_SUBJECT_ALTERNATIVE_NAMES,
    )
}

/// The most SSL/TLS certificates this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: a create that would exceed it is
/// refused with the modelled `InvalidInputException`.
pub const MAX_CERTIFICATES: usize = 500;

/// How many certificates one `GetCertificates` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more certificates remain.
pub const CERTIFICATE_PAGE_SIZE: usize = 10;

/// The ARN of the certificate `id`. A certificate, like the distribution it is
/// attached to, lives in [`DISTRIBUTION_REGION_NAME`] — "only certificates
/// created in the us-east-1 Amazon Web Services Region can be attached to
/// Lightsail distributions" — so that is the Region its ARN carries.
pub fn certificate_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        DISTRIBUTION_REGION_NAME,
        &format!("{CERTIFICATE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The serial number of the certificate minted as `id`, as
/// `Certificate$serialNumber` reports it.
///
/// An X.509 serial number is printed as colon-separated hex byte pairs, and this
/// one is built from the certificate's own minted id — so it identifies that one
/// certificate and is the same on every later read. An odd trailing digit is
/// dropped, so every group is a whole byte; a minted uuid carries 32 hex digits,
/// which the assertion below pins.
pub fn certificate_serial_number(id: &str) -> String {
    let mut digits: Vec<char> = id.chars().filter(char::is_ascii_hexdigit).collect();
    debug_assert!(
        digits.len() >= 2,
        "a certificate id carries at least one hex byte, got {id:?}"
    );
    digits.truncate(digits.len() - digits.len() % 2);
    digits
        .chunks(2)
        .map(|pair| pair.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join(":")
}

/// Why the certificate a request named is not one the account may secure
/// traffic with.
///
/// The rule is the same whichever operation records the reference — the create
/// that names a certificate on a new distribution, the attach that sets one
/// later, the container service that keys its public domain names by one, or the
/// attach that puts one of a load balancer's certificates in front of its HTTPS
/// traffic — so it is owned here, and each handler only renders the refusal in
/// its own error channel. See
/// [`ComAmazonawsLightsailLightsail20161128State::issued_certificate`] for the
/// certificates of the account and [`StoredLoadBalancer::attach_certificate`]
/// for the ones a load balancer holds.
#[derive(Debug, Clone)]
pub enum CertificateNotUsable {
    /// This account holds no certificate of that name: Lightsail's
    /// `DoesNotExist` envelope, built through
    /// [`crate::scaffolded::Scaffolded::resource_does_not_exist`] with
    /// [`RESOURCE_TYPE_CERTIFICATE`].
    DoesNotExist,
    /// The certificate exists but the certificate authority has not issued it,
    /// so it secures nothing. The variant carries the status the record is in —
    /// a `CertificateStatus`, or the `LoadBalancerTlsCertificateStatus` of a
    /// certificate a load balancer holds — which is what the refusing
    /// operation's message names: see
    /// [`certificate_not_issued_for_distribution`],
    /// [`certificate_not_issued_for_load_balancer`] and
    /// [`certificate_not_issued_for_public_domain_names`].
    NotIssued { status: &'static str },
}

/// Lightsail's message for a request that would make a certificate the
/// certificate authority has not issued the one a distribution serves HTTPS
/// traffic with: "Only certificates with a status of ISSUED can be attached to a
/// distribution" (the modelled
/// `AttachCertificateToDistribution$certificateName` documentation).
pub fn certificate_not_issued_for_distribution(name: &str, status: &str) -> String {
    format!(
        "The certificate {name} is {status}: only certificates with a status of \
         {CERTIFICATE_STATUS_ISSUED} can be attached to a distribution."
    )
}

/// Lightsail's message for a request that would make a certificate the
/// certificate authority has not issued the one a load balancer terminates
/// HTTPS with: "Once you create and validate your certificate, you can attach
/// it to your load balancer" (the modelled
/// `AttachLoadBalancerTlsCertificate` documentation).
pub fn certificate_not_issued_for_load_balancer(name: &str, status: &str) -> String {
    format!(
        "The certificate {name} is {status}: once you create and validate your certificate, \
         you can attach it to your load balancer."
    )
}

/// Lightsail's message for a request that would key a container service's public
/// domain names by a certificate the certificate authority has not issued: "You
/// must create and validate an SSL/TLS certificate before you can use public
/// domain names with your container service" (the modelled
/// `CreateContainerServiceRequest$publicDomainNames` documentation).
pub fn certificate_not_issued_for_public_domain_names(name: &str, status: &str) -> String {
    format!(
        "The certificate {name} is {status}: you must create and validate an SSL/TLS \
         certificate before you can use public domain names with your container service."
    )
}

/// One stored SSL/TLS certificate: the request the caller made, the members the
/// service assigned, and the validation state it is in.
#[derive(Debug, Clone)]
pub struct StoredCertificate {
    /// The certificate name the request named, kept verbatim — it is this
    /// record's store key and the name every certificate operation addresses it
    /// by.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    /// Where the certificate lives — always [`DISTRIBUTION_REGION_NAME`], for
    /// the reason that constant gives.
    ///
    /// Kept on the record rather than re-derived: the modelled `Certificate`
    /// carries no location member, but every operation that LOGS about this
    /// certificate has to locate its entry where the resource is, and this is
    /// where that location comes from.
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The domain the certificate is for, kept as the request asked.
    pub domain_name: String,
    /// The alternate domains and subdomains the certificate also covers, as
    /// [`alternative_domain_names`] reduced them — every writer of this field
    /// goes through that one owner (see
    /// [`certificate_subject_alternative_names`]), so a repeat and the primary
    /// domain are never among them — in the order the request first named
    /// them.
    pub subject_alternative_names: Vec<String>,
    pub serial_number: String,
    /// The current `CertificateStatus`.
    pub status: &'static str,
    /// The status the clock owes this certificate, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instant the certificate authority issues this certificate — the same
    /// instant the clock owes it [`CERTIFICATE_STATUS_ISSUED`] — which is what
    /// `issuedAt`, `notBefore` and `notAfter` are reported from once it is
    /// issued.
    pub issued_at_ms: i64,
}

impl StoredCertificate {
    /// Whether the certificate authority has issued this certificate.
    pub fn is_issued(&self) -> bool {
        self.status == CERTIFICATE_STATUS_ISSUED
    }

    /// Every domain this certificate covers: the domain it is for, then each
    /// alternate domain it also covers.
    ///
    /// The alternate domains went through [`alternative_domain_names`] before
    /// they were stored, so none of them repeats and none of them is the
    /// primary domain — this list is the set of domains the certificate secures
    /// traffic for, with its primary domain first.
    ///
    /// This is what a distribution the certificate is attached to reports as its
    /// `alternativeDomainNames`: "After the certificate is attached, your
    /// distribution accepts HTTPS traffic for all of the domains that are
    /// associated with the certificate."
    pub fn domain_names(&self) -> Vec<String> {
        let mut names = vec![self.domain_name.clone()];
        names.extend(self.subject_alternative_names.iter().cloned());
        names
    }

    /// This certificate as the attachment a distribution serves HTTPS traffic
    /// through: the name the distribution reports as its `certificateName`,
    /// together with every domain this certificate covers.
    ///
    /// Read off ONE certificate at once, so the two halves of the attachment
    /// cannot come from different places, and applied by
    /// [`StoredDistribution::attach_certificate`].
    pub fn distribution_attachment(&self) -> DistributionCertificateAttachment {
        DistributionCertificateAttachment {
            certificate_name: self.name.clone(),
            alternative_domain_names: self.domain_names(),
        }
    }

    /// This record as the modelled `CertificateSummary`.
    ///
    /// `in_use_resource_count` is how many resources the certificate is attached
    /// to, which only the whole store knows — see
    /// [`ComAmazonawsLightsailLightsail20161128State::certificate_in_use_resource_count`].
    ///
    /// `include_details` is `GetCertificates`'s own
    /// `includeCertificateDetails`: without it "the response will include only
    /// the certificate Amazon Resource Name (ARN), certificate name, domain name,
    /// and tags".
    pub fn view(&self, in_use_resource_count: i32, include_details: bool) -> CertificateSummary {
        CertificateSummary {
            certificate_arn: Some(self.arn.clone()),
            certificate_name: Some(self.name.clone()),
            domain_name: Some(self.domain_name.clone()),
            certificate_detail: include_details.then(|| self.detail(in_use_resource_count)),
            tags: Some(self.tags.clone()),
        }
    }

    /// This record as the modelled `Certificate`, the full detail of the
    /// certificate.
    pub fn detail(&self, in_use_resource_count: i32) -> Certificate {
        let issued = self.is_issued();
        Certificate {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            domain_name: Some(self.domain_name.clone()),
            status: Some(self.status.to_string()),
            serial_number: Some(self.serial_number.clone()),
            subject_alternative_names: Some(self.subject_alternative_names.clone()),
            // One record per domain the certificate covers. This emulator owns
            // no DNS zone to publish a validation CNAME in, so a record carries
            // the domain it is about and the state its validation is in rather
            // than a `resourceRecord` nobody could create.
            domain_validation_records: Some(
                self.domain_names()
                    .into_iter()
                    .map(|domain_name| DomainValidationRecord {
                        domain_name: Some(domain_name),
                        validation_status: Some(
                            if issued {
                                DOMAIN_VALIDATION_STATUS_SUCCESS
                            } else {
                                DOMAIN_VALIDATION_STATUS_PENDING
                            }
                            .to_string(),
                        ),
                        ..Default::default()
                    })
                    .collect(),
            ),
            in_use_resource_count,
            key_algorithm: Some(CERTIFICATE_KEY_ALGORITHM.to_string()),
            created_at: Some(timestamp(self.created_at_ms)),
            // Nothing was issued while the certificate was still being
            // validated, so none of the issuance members is reported until it
            // is: a certificate has no issuer, no validity window and no issue
            // date before the certificate authority gives it one.
            issued_at: issued.then(|| timestamp(self.issued_at_ms)),
            issuer_ca: issued.then(|| CERTIFICATE_ISSUER_CA.to_string()),
            not_before: issued.then(|| timestamp(self.issued_at_ms)),
            not_after: issued
                .then(|| timestamp(self.issued_at_ms.saturating_add(CERTIFICATE_VALIDITY_MILLIS))),
            tags: Some(self.tags.clone()),
            support_code: Some(self.support_code.clone()),
            ..Default::default()
        }
    }
}

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

/// The status of a reset of a distribution's cached content while the reset is
/// still being applied across the CDN, and once it has been.
///
/// `ResetDistributionCacheResult$status` ("The status of the reset cache
/// request") and `GetDistributionLatestCacheResetResult$status` ("The status of
/// the last cache reset") are plain strings in the model with no enum behind
/// them. A Lightsail distribution IS an Amazon CloudFront distribution (which is
/// why its own status carries CloudFront's distribution status values, see
/// [`DISTRIBUTION_STATUS_IN_PROGRESS`]) and deleting its cached content is a
/// CloudFront invalidation, so a reset carries CloudFront's own
/// `Invalidation$Status`: `InProgress` while the invalidation is being applied
/// and `Completed` once it has been, as the CloudFront API documents for that
/// member ("When the invalidation batch is finished, the status is Completed").
/// That a LATER read reports the status of the last reset at all is what says
/// the value changes after the call that started it: a status fixed for the life
/// of the record would be nothing to go back and read.
pub const CACHE_RESET_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`CACHE_RESET_STATUS_IN_PROGRESS`].
pub const CACHE_RESET_STATUS_COMPLETED: &str = "Completed";

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

/// The `errorCachingMinTTL`, in seconds, a custom error response that names none
/// of its own is cached for.
///
/// From the modelled `DistributionCustomErrorResponse$errorCachingMinTTL`
/// documentation: "The minimum time, in seconds, that the distribution caches the
/// custom error response before requesting the object again from the origin. If
/// you don't specify a value, the default is 10 seconds."
pub const DEFAULT_ERROR_CACHING_MIN_TTL_SECONDS: i64 = 10;

/// The character a custom error response's `responsePagePath` has to begin with.
///
/// The member is modelled as a bare `string`, and its documentation states the
/// form the model cannot express: "The path to the custom error page that the
/// distribution returns to the viewer (for example, /404.html). The path must
/// begin with a forward slash (/) and reference an object that is available from
/// the origin."
const CUSTOM_ERROR_PAGE_PATH_PREFIX: char = '/';

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

/// The bundles a `GetDistributionBundles` call reports out of `specs`: EVERY
/// plan the catalog holds, retired ones included.
///
/// The request carries no member to select a slice with — unlike the bucket and
/// database catalogs, which have an `includeInactive` flag — and
/// `DistributionBundle$isActive` is what tells a caller which of the reported
/// plans a new or existing distribution may be put on, so leaving a retired plan
/// out would hide the flag's only subject.
///
/// The order is `specs`' own, so two reads answer identically. Taking the catalog
/// as an argument is what lets the rule be exercised against a catalog that holds
/// a retired plan, which the published one does not yet.
pub fn distribution_bundles_from(specs: &[DistributionBundleSpec]) -> Vec<DistributionBundle> {
    specs.iter().map(DistributionBundleSpec::view).collect()
}

/// The distribution bundles a `GetDistributionBundles` call reports:
/// [`distribution_bundles_from`] over this service's published catalog,
/// [`DISTRIBUTION_BUNDLE_SPECS`].
pub fn distribution_bundles() -> Vec<DistributionBundle> {
    distribution_bundles_from(DISTRIBUTION_BUNDLE_SPECS)
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

/// Whether a distribution may be put on the plan `bundle_id` names, out of the
/// catalog `specs`: the plan has to be one the catalog holds AND still active,
/// because `DistributionBundle$isActive` is what says a bundle "can be specified
/// for a new or existing distribution".
///
/// Taking the catalog as an argument is what lets the rule be exercised against a
/// catalog that holds a retired plan, which the published one does not yet — the
/// same reason [`distribution_bundles_from`] takes one.
pub fn distribution_bundle_is_applicable_in(
    specs: &[DistributionBundleSpec],
    bundle_id: &str,
) -> bool {
    specs
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .is_some_and(|spec| spec.is_active)
}

/// Whether a distribution may be put on the plan `bundle_id` names:
/// [`distribution_bundle_is_applicable_in`] over this service's published
/// catalog, [`DISTRIBUTION_BUNDLE_SPECS`].
///
/// One rule over both calls that name a plan — `CreateDistribution` for a new
/// distribution and `UpdateDistributionBundle` for one already running — so a
/// retired plan is refused the same way whichever of them names it.
pub fn distribution_bundle_is_applicable(bundle_id: &str) -> bool {
    distribution_bundle_is_applicable_in(DISTRIBUTION_BUNDLE_SPECS, bundle_id)
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

/// The custom error responses `requested` asks a distribution to serve, each with
/// every member it left out filled in from that member's documented default, or a
/// refusal when the list carries more entries than one distribution holds or an
/// entry states a page path the distribution could not fetch from its origin.
///
/// A member the model documents a default for is one the SERVICE fills in, so a
/// stored — and therefore reported — entry carries it whether or not the request
/// did, exactly as the origin's `responseTimeout` and the distribution's
/// `ipAddressType` do. `errorCachingMinTTL` is the only member of
/// `DistributionCustomErrorResponse` whose documentation states a default; the
/// other three are reported as the request stated them.
///
/// `responsePagePath` is the path of an object of the origin, which is what
/// beginning with [`CUSTOM_ERROR_PAGE_PATH_PREFIX`] says, so an entry stating one
/// in any other form states no page the distribution could serve. An entry that
/// carries no page path states no path to check.
///
/// The ONE owner of that rule, so `CreateDistribution` and `UpdateDistribution`
/// cannot record the list two different ways.
pub fn custom_error_responses_from_request(
    requested: &[DistributionCustomErrorResponse],
) -> Result<Vec<DistributionCustomErrorResponse>, String> {
    if requested.len() > MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION {
        return Err(format!(
            "customErrorResponses carries {} entries, more than the {} one distribution holds",
            requested.len(),
            MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION
        ));
    }
    for (index, response) in requested.iter().enumerate() {
        let Some(path) = response.response_page_path.as_deref() else {
            continue;
        };
        if !path.starts_with(CUSTOM_ERROR_PAGE_PATH_PREFIX) {
            return Err(format!(
                "customErrorResponses[{index}].responsePagePath '{path}' is not the path to an \
                 object of the origin: the path must begin with a forward slash \
                 ({CUSTOM_ERROR_PAGE_PATH_PREFIX}) and reference an object that is available from \
                 the origin"
            ));
        }
    }
    Ok(requested
        .iter()
        .map(|response| {
            let mut recorded = response.clone();
            recorded.error_caching_min_ttl = Some(
                recorded
                    .error_caching_min_ttl
                    .unwrap_or(DEFAULT_ERROR_CACHING_MIN_TTL_SECONDS),
            );
            recorded
        })
        .collect())
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

/// The last reset of ONE distribution's cached content: what a
/// `ResetDistributionCache` call started, and the whole of what
/// `GetDistributionLatestCacheReset` reports.
///
/// A distribution holds the LATEST reset only — both operations answer a single
/// timestamp and a single status, "the last cache reset" — so a new reset
/// replaces the record of the one before it rather than appending to a history
/// nothing reports.
#[derive(Debug, Clone, Copy)]
pub struct StoredCacheReset {
    /// When the reset was requested: `createTime`, "The timestamp of the reset
    /// cache request".
    pub created_at_ms: i64,
    /// One of [`CACHE_RESET_STATUS_IN_PROGRESS`] /
    /// [`CACHE_RESET_STATUS_COMPLETED`].
    pub status: &'static str,
    /// The status the clock owes this reset, while one is outstanding.
    pub due: Option<DueNamedState>,
}

impl StoredCacheReset {
    /// The reset a `ResetDistributionCache` call at `now_ms` starts: the
    /// invalidation is being applied, and the clock owes it
    /// [`CACHE_RESET_STATUS_COMPLETED`].
    ///
    /// The span is non-zero, so a caller that resets and reads straight back
    /// still observes the reset in progress, exactly as it does against AWS.
    pub fn started(now_ms: i64) -> Self {
        Self {
            created_at_ms: now_ms,
            status: CACHE_RESET_STATUS_IN_PROGRESS,
            due: Some(DueNamedState::after(now_ms, CACHE_RESET_STATUS_COMPLETED)),
        }
    }
}

/// One stored distribution: the CDN distribution `CreateDistribution` created.
///
/// What the caller asked for — name, bundle, origin, cache behaviour and
/// settings, custom error responses, IP address type, certificate name, minimum
/// viewer TLS version, default root object and tags — is kept as asked for, with
/// the documented default of a member the request left out filled in; the ARN,
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
    /// When this distribution's bundle was last changed; `None` on one that has
    /// never been moved. A distribution's bundle can be updated "only one time
    /// within your monthly Amazon Web Services billing cycle"
    /// (`UpdateDistributionBundle`), so the instant of that change is what says
    /// whether another one is available — see
    /// [`StoredDistribution::able_to_update_bundle`].
    pub bundle_updated_at_ms: Option<i64>,
    /// The SSL/TLS certificate attached to the distribution, if the request
    /// named one or `AttachCertificateToDistribution` attached one.
    pub certificate_name: Option<String>,
    /// The alternate domain names of the distribution, which come from the
    /// domains of an attached certificate: a distribution with none attached has
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
    /// The custom error responses this distribution serves, as
    /// [`custom_error_responses_from_request`] recorded them: the entries the
    /// request carried, with each member the model documents a default for
    /// filled in.
    pub custom_error_responses: Vec<DistributionCustomErrorResponse>,
    /// The last reset of this distribution's cached content, once one has been
    /// requested: `None` on a distribution whose cache has never been reset,
    /// which has no timestamp and no status of a reset to report.
    pub latest_cache_reset: Option<StoredCacheReset>,
}

/// The attachment of one SSL/TLS certificate to a distribution: the certificate
/// the distribution names, and the domains it accepts HTTPS traffic for.
///
/// The pair travels as ONE value — built from the certificate itself by
/// [`StoredCertificate::distribution_attachment`] and applied by
/// [`StoredDistribution::attach_certificate`] — so no caller can record the
/// certificate without the domains it covers.
#[derive(Debug, Clone)]
pub struct DistributionCertificateAttachment {
    /// The certificate's own name, as this account addresses it.
    certificate_name: String,
    /// Every domain that certificate covers, from
    /// [`StoredCertificate::domain_names`].
    alternative_domain_names: Vec<String>,
}

impl StoredDistribution {
    /// Record the SSL/TLS certificate `attachment` on this distribution, so it
    /// names that certificate and publishes every domain the certificate covers
    /// — the ONE owner of that rule.
    ///
    /// The domains a distribution accepts HTTPS traffic for come from the
    /// certificate attached to it ("your distribution accepts HTTPS traffic for
    /// all of the domains that are associated with the certificate"), so making
    /// the attachment writes BOTH members, never one without the other. Every
    /// operation that attaches one — the create that names a certificate for a
    /// new distribution as much as the attach and the update that set one later
    /// — asks here, so the attachment cannot drift between them. A distribution
    /// holds ONE attached certificate, so this replaces whatever was attached
    /// before.
    pub fn attach_certificate(&mut self, attachment: DistributionCertificateAttachment) {
        self.certificate_name = Some(attachment.certificate_name);
        self.alternative_domain_names = attachment.alternative_domain_names;
    }

    /// Release the SSL/TLS certificate this distribution serves HTTPS traffic
    /// with, so it names none and publishes none of that certificate's domains —
    /// the ONE owner of that rule.
    ///
    /// A distribution that serves no certificate of its own publishes no
    /// alternate domain names: the domains it accepts HTTPS traffic for come
    /// from the certificate attached to it ("your distribution accepts HTTPS
    /// traffic for all of the domains that are associated with the
    /// certificate"), so releasing the attachment clears BOTH members, never
    /// one without the other — the exact reverse of
    /// [`Self::attach_certificate`]. Every operation that releases the
    /// attachment asks here, so the release cannot drift between them.
    /// Idempotent: a distribution that has no certificate attached already
    /// satisfies it.
    pub fn detach_certificate(&mut self) {
        self.certificate_name = None;
        self.alternative_domain_names = Vec::new();
    }

    /// Whether the one bundle change this distribution's monthly billing cycle
    /// allows is available at `now_ms` — the `ableToUpdateBundle` a read reports.
    pub fn able_to_update_bundle(&self, now_ms: i64) -> bool {
        bundle_change_available(self.bundle_updated_at_ms, now_ms)
    }

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
            able_to_update_bundle: Some(self.able_to_update_bundle(now_ms())),
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

/// One reference a resource's removal released: everything the operation log
/// needs to record the release that removal performed.
///
/// A release is a MUTATION of the record that held the reference, so it is
/// logged like any other, under the modelled `OperationType` member for the
/// call that makes the same change by hand — the detach for an attachment, the
/// delete for a record whose reference is required and so cannot be left empty
/// — and located where the record that changed lives.
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
    /// One entry per reference the removal released, in the store's own
    /// stable order: disks by name, then static IPs by name, then load
    /// balancers by region and name, then buckets by region and name, then the
    /// alarms that monitored the removed instance by alarm name — every one of
    /// those maps is a `BTreeMap`, so the order is the same on every call.
    pub released_attachments: Vec<ReleasedAttachment>,
}

/// The outcome of removing one managed relational database: the record itself,
/// and every reference another record held to it that the removal released
/// (see
/// [`ComAmazonawsLightsailLightsail20161128State::remove_relational_database`]).
#[derive(Debug)]
pub struct RemovedRelationalDatabase {
    /// The database record that was removed.
    pub database: StoredRelationalDatabase,
    /// One entry per reference the removal released, in the store's own stable
    /// order: the alarms that monitored the removed database, by alarm name
    /// (that map is a `BTreeMap`, so the order is the same on every call).
    pub released_references: Vec<ReleasedAttachment>,
}

/// The outcome of removing one bucket: the record itself, and every reference
/// another record held to it that the removal released
/// (see [`ComAmazonawsLightsailLightsail20161128State::remove_bucket`]).
#[derive(Debug)]
pub struct RemovedBucket {
    /// The bucket record that was removed.
    pub bucket: StoredBucket,
    /// One entry per reference the removal released, in the store's own stable
    /// order: the buckets whose access logs were saved in the removed one, by
    /// region and name (that map is a `BTreeMap`, so the order is the same on
    /// every call).
    pub released_references: Vec<ReleasedAttachment>,
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
// Account profile
// ---------------------------------------------------------------------------

/// The `ProfileType` of an account that is not enrolled in the Lightsail partner
/// program, from that enum member's own documentation: "`Lightsailor` – The
/// account is not enrolled in the Lightsail partner program."
pub const PROFILE_TYPE_LIGHTSAILOR: &str = "Lightsailor";

/// The `ProfileType` of an account that IS enrolled, from the other member of the
/// same modelled enum: "`LightsailPartner` – The account is enrolled in the
/// Lightsail partner program."
pub const PROFILE_TYPE_LIGHTSAIL_PARTNER: &str = "LightsailPartner";

/// The `PartnerStatus` of a membership whose tier benefits the account has, from
/// that member's own documentation: "`Active` – The membership is active, and the
/// benefits of the current tier are available to the account."
pub const PARTNER_STATUS_ACTIVE: &str = "Active";

/// The `PartnerStatus` of a membership whose tier benefits are withheld, from the
/// other value the same member documents: "`Suspended` – The membership is
/// suspended, and the benefits of the tier are not available to the account."
pub const PARTNER_STATUS_SUSPENDED: &str = "Suspended";

/// The Lightsail partner program membership of this account, held only while the
/// account is enrolled in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPartnerMembership {
    /// When the account was enrolled in the program — the instant
    /// `PartnerInfo$enrolledAt` reports, and a required member of it.
    pub enrolled_at_ms: i64,
    /// The tier of the membership: one of the modelled `TierName` values.
    /// Optional, because `PartnerInfo$tierName` is — a membership that is in no
    /// tier reports none.
    pub tier_name: Option<String>,
    /// The status of the membership: one of the modelled `PartnerStatus` values
    /// ([`PARTNER_STATUS_ACTIVE`], [`PARTNER_STATUS_SUSPENDED`]).
    pub status: String,
}

impl StoredPartnerMembership {
    /// This membership as the modelled `PartnerInfo`.
    pub fn view(&self) -> PartnerInfo {
        PartnerInfo {
            enrolled_at: timestamp(self.enrolled_at_ms),
            tier_name: self.tier_name.clone(),
            status: self.status.clone(),
        }
    }
}

/// The profile of the account this emulator serves: the Lightsail partner program
/// membership it holds, if any.
///
/// The membership is the whole of the record, because the profile TYPE follows
/// from it — an account is a partner exactly while it holds a membership and a
/// Lightsailor otherwise — so the type and the membership can never disagree.
/// Nothing in this service's API enrols an account in the program (enrolment
/// happens outside Lightsail), so a store nothing has seeded holds no
/// membership, which is what makes the profile of a fresh account the
/// not-enrolled one.
#[derive(Debug, Clone, Default)]
pub struct StoredAccountProfile {
    /// The account's partner program membership, and `None` while it is not
    /// enrolled.
    pub partner: Option<StoredPartnerMembership>,
}

impl StoredAccountProfile {
    /// The modelled `ProfileType` of this profile: an enrolled account is a
    /// partner, every other account is a Lightsailor.
    pub fn profile_type(&self) -> &'static str {
        if self.partner.is_some() {
            PROFILE_TYPE_LIGHTSAIL_PARTNER
        } else {
            PROFILE_TYPE_LIGHTSAILOR
        }
    }

    /// The membership as the modelled `PartnerInfo`, which is reported "only for
    /// accounts that have a `profileType` of `LightsailPartner`" — an account
    /// holding no membership has none to report.
    pub fn partner_view(&self) -> Option<PartnerInfo> {
        self.partner.as_ref().map(StoredPartnerMembership::view)
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
    ///
    /// The key is the name ALONE, not the `(region, name)` pair a bucket and a
    /// load balancer are held under, because every modelled disk operation
    /// addresses a disk by `diskName` and nothing else. The disk is still a
    /// LOCATED record — its [`StoredDisk::location`] carries the region of the
    /// `availabilityZone` its create named — and the one listing of it is scoped
    /// to a region ("all block storage disks in your AWS account and region"), so
    /// `GetDisks` selects that region's records on the walk rather than the key
    /// doing it.
    pub disks: BTreeMap<String, StoredDisk>,
    /// Every disk snapshot, keyed by its snapshot name — the identifier every
    /// disk snapshot operation takes. A `BTreeMap` so `GetDiskSnapshots` walks a
    /// stable, total order and a page token can name the next snapshot.
    pub disk_snapshots: BTreeMap<String, StoredDiskSnapshot>,
    /// Every instance snapshot, keyed by its snapshot name — the identifier every
    /// instance snapshot operation takes. A `BTreeMap` so `GetInstanceSnapshots`
    /// walks a stable, total order and a page token can name the next snapshot.
    pub instance_snapshots: BTreeMap<String, StoredInstanceSnapshot>,
    /// What the `AutoSnapshot` add-on has taken of each resource it is enabled
    /// on, keyed by that resource's `ResourceType` and its name — the pair that
    /// identifies it, because an automatic snapshot has no name of its own and
    /// one name can be both an instance's and a disk's. A `BTreeMap` so the
    /// walk in
    /// [`ComAmazonawsLightsailLightsail20161128State::advance_clock`] is in a
    /// stable, total order.
    ///
    /// Bounded by [`MAX_INSTANCES`] plus [`MAX_DISKS`] (an entry belongs to one
    /// instance or disk this account holds) and, within one entry, by
    /// [`MAX_AUTO_SNAPSHOTS_PER_RESOURCE`].
    pub auto_snapshots: BTreeMap<(&'static str, String), StoredAutoSnapshots>,
    /// Every export snapshot record, keyed by the name this store minted for it
    /// — the only identifier an export snapshot record has, because the caller
    /// names none. A `BTreeMap` so `GetExportSnapshotRecords` walks a stable,
    /// total order and a page token can name the next record.
    pub export_snapshot_records: BTreeMap<String, StoredExportSnapshotRecord>,
    /// Every CloudFormation stack record, keyed by the name this store minted
    /// for it — the only identifier a CloudFormation stack record has, because
    /// the caller names none. A `BTreeMap` so
    /// `GetCloudFormationStackRecords` walks a stable, total order and a page
    /// token can name the next record.
    pub cloud_formation_stack_records: BTreeMap<String, StoredCloudFormationStackRecord>,
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
    /// Every SSH key pair, keyed by its key pair name — the identifier every key
    /// pair operation takes. A `BTreeMap` so `GetKeyPairs` walks a stable, total
    /// order and a page token can name the next key pair.
    ///
    /// Keyed by name alone, like the instance whose `sshKeyName` names one:
    /// both are served by name whichever Region the request is made from. That
    /// is this store's existing shape for a resource an instance refers to, and
    /// it is a DEVIATION worth knowing about — against AWS a key pair belongs to
    /// one Region, so a pair held in another Region is not addressable from this
    /// endpoint. The key to revisit is here, together with `GetKeyPairs`'
    /// Region-scoped listing, once the calls that create a key pair are
    /// implemented.
    pub key_pairs: BTreeMap<String, StoredKeyPair>,
    /// Every alarm, keyed by its name — the identifier every alarm operation
    /// takes, and the one `PutAlarm` creates or updates a record under. A
    /// `BTreeMap` so `GetAlarms` walks a stable, total order and a page token can
    /// name the next alarm.
    pub alarms: BTreeMap<String, StoredAlarm>,
    /// Every contact method, keyed by the Region it was created in and the
    /// `ContactProtocol` it sends through — the pair that identifies it, because
    /// "you can add one email address and one mobile phone number contact method
    /// in each Amazon Web Services Region" (`CreateContactMethod`), and every
    /// contact-method operation is scoped to the Region the request is made in.
    ///
    /// Unbounded by any `MAX_` of its own, and it needs none: both halves of the
    /// key come from a modelled enum — the Region from [`request_region`], which
    /// only ever answers a `RegionName` value, and the protocol from
    /// `ContactProtocol` — so the map holds at most one entry per Region and
    /// protocol however many calls a caller makes. A `BTreeMap` so
    /// `GetContactMethods` walks a stable, total order.
    pub contact_methods: BTreeMap<(String, String), StoredContactMethod>,
    /// Every load balancer, keyed by the region it was created in and its name.
    ///
    /// A load balancer is a REGIONAL resource — the modelled
    /// `CreateLoadBalancer` documentation caps them "per AWS Region in your
    /// account" — so the region is part of its identity, exactly as it is for a
    /// container service and a bucket. A `BTreeMap` so a listing walks a stable,
    /// total order and a page token can name the next load balancer.
    pub load_balancers: BTreeMap<(String, String), StoredLoadBalancer>,
    /// The live sets of container image registry log in credentials, keyed by
    /// the region the creating request was made in, newest last.
    ///
    /// The registry is the ACCOUNT's — `CreateContainerServiceRegistryLogin`
    /// creates credentials for "the container image registry of your Lightsail
    /// account" — and the address each set was handed carries the region that
    /// request was signed for, so that region is what a later log in presents
    /// its credentials at. The clock forgets a set 12 hours after it was created
    /// (see [`Self::advance_clock`]), so every set held here is still valid.
    pub registry_logins: BTreeMap<String, Vec<StoredRegistryLogin>>,
    /// The container image registry of the account, keyed by the region whose
    /// announced address a push reached it at.
    ///
    /// One registry per region, for the same reason the log in credentials are
    /// held per region: the address `CreateContainerServiceRegistryLogin` hands
    /// out carries the region the creating request was made in, so that is the
    /// registry a push and the `RegisterContainerImage` after it address. A
    /// region nothing has ever pushed to holds no entry. Each registry is
    /// bounded by [`MAX_REGISTRY_BYTES_PER_REGION`], and the number of them by
    /// the modelled `RegionName` enum: an entry is created only for a region a
    /// push authenticated in, and a set of log in credentials is held under the
    /// region [`request_region`] answers, which is always one of that enum's
    /// values.
    pub container_image_registries: BTreeMap<String, StoredImageRegistry>,
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
    /// Every SSL/TLS certificate, keyed by its certificate name — the identifier
    /// every certificate operation takes. NOT keyed by region, for the same
    /// reason a distribution is not: a Lightsail certificate is addressed from
    /// the one Region its API is served in (see [`DISTRIBUTION_REGION_NAME`]). A
    /// `BTreeMap` so `GetCertificates` walks a stable, total order and a page
    /// token can name the next certificate.
    pub certificates: BTreeMap<String, StoredCertificate>,
    /// The profile of the account this emulator serves — the Lightsail partner
    /// program membership it holds — which `GetProfile` reports.
    ///
    /// ONE record rather than a map: a profile belongs to the ACCOUNT, and this
    /// emulator serves one account (`engine::emulator_account_id`), so there is
    /// one profile however many Regions a caller reaches it from.
    pub account_profile: StoredAccountProfile,
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
        // A create that enabled the `AutoSnapshot` add-on started that add-on's
        // daily schedule at the instant of the create, so the schedule is
        // anchored there rather than at whenever somebody next calls.
        let name = instance.name.clone();
        let created_at_ms = instance.created_at_ms;
        let hour = enabled_auto_snapshot_hour(&instance.add_ons);
        self.discard_kept_auto_snapshots(&name);
        self.instances.insert(name.clone(), instance);
        self.reconcile_auto_snapshots(created_at_ms, RESOURCE_TYPE_INSTANCE, &name, hour.as_deref());
    }

    /// Create the GUI (Amazon DCV) session access of the virtual computer
    /// `name`, answering what was stored — or `None` when no instance of that
    /// name is stored, in which case nothing was written and no id was spent.
    ///
    /// The presence check comes BEFORE the mint on purpose: a call that resolves
    /// no instance must not consume an identifier, and the mint and the write
    /// live together here so no caller can be answered with a session the store
    /// does not hold.
    ///
    /// A session started on a virtual computer that is still coming up is up
    /// once that computer is: the session is a session ON the computer, so the
    /// instant it finishes coming up follows the one the clock already owes the
    /// computer its running state (see
    /// [`StoredGuiSession::starting_once_ready`]).
    pub fn create_gui_session(&mut self, name: &str, now_ms: i64) -> Option<StoredGuiSession> {
        let instance = self.instances.get(name)?;
        // Only a transition INTO `running` makes the computer ready; one into
        // any other state leaves the session's start where this call is.
        let ready_at_ms = instance
            .due
            .filter(|due| due.state == RUNNING)
            .map_or(now_ms, |due| due.at_ms);
        let token = self.mint_uuid();
        let session = StoredGuiSession::starting_once_ready(token, now_ms, ready_at_ms);
        self.instances.get_mut(name)?.gui_session = Some(session.clone());
        Some(session)
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
        // The same anchoring an instance's schedule gets
        // ([`Self::insert_instance`]): the add-on a create enabled has been
        // running since that create.
        let name = disk.name.clone();
        let created_at_ms = disk.created_at_ms;
        let hour = enabled_auto_snapshot_hour(&disk.add_ons);
        self.discard_kept_auto_snapshots(&name);
        self.disks.insert(name.clone(), disk);
        self.reconcile_auto_snapshots(created_at_ms, RESOURCE_TYPE_DISK, &name, hour.as_deref());
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

    /// Remove the block storage disk `name` names, answering the record that was
    /// removed. `None` when nothing matched.
    ///
    /// A disk is removable only while it is attached to nothing (the modelled
    /// `DeleteDisk` documentation: "The disk must be in the available state (not
    /// attached to a Lightsail instance)"), which the caller is what enforces,
    /// so there is no attachment left for this removal to release — unlike
    /// [`Self::remove_instance`], whose resource other records do hold
    /// attachments to. A snapshot taken of the disk is a resource of its own and
    /// keeps the source name and ARN it recorded, so it outlives the disk.
    ///
    /// `delete_add_ons` says the delete was asked to delete the disk's add-ons
    /// with it, which is what decides the fate of the copies the disk's add-on
    /// took — see [`Self::settle_auto_snapshots_of_removed`].
    pub fn remove_disk(&mut self, name: &str, delete_add_ons: bool) -> Option<StoredDisk> {
        let disk = self.disks.remove(name)?;
        self.settle_auto_snapshots_of_removed(
            RESOURCE_TYPE_DISK,
            name,
            &disk.location,
            delete_add_ons,
        );
        Some(disk)
    }

    /// The instance `name` names as the modelled `Instance`, with every block
    /// storage disk attached to it reported in its hardware.
    ///
    /// The one way a reporting operation renders an instance: an attachment
    /// lives on the DISK record, so assembling the view needs the whole store,
    /// not just the instance.
    pub fn instance_view(&self, name: &str) -> Option<Instance> {
        Some(self.instance_view_of(self.instances.get(name)?))
    }

    /// The same view for an instance record a caller already holds — the listing
    /// walk, which has the record in hand and renders it exactly as `GetInstance`
    /// does rather than keeping a second rendering of an instance.
    pub fn instance_view_of(&self, instance: &StoredInstance) -> Instance {
        instance.view(
            self.attached_disk_views(&instance.name),
            self.next_auto_snapshot_time_of_day(RESOURCE_TYPE_INSTANCE, &instance.name),
        )
    }

    /// The disk record `disk` as the modelled `Disk`, with the daily time its
    /// next automatic copy is due at.
    ///
    /// The one way a reporting operation renders a disk: that time lives on the
    /// schedule the disk's add-on runs on, so rendering a disk needs the store
    /// and not just the disk record.
    pub fn disk_view(&self, disk: &StoredDisk) -> crate::ops::types::Disk {
        disk.view(self.next_auto_snapshot_time_of_day(RESOURCE_TYPE_DISK, &disk.name))
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
            .map(|disk| self.disk_view(disk))
            .collect()
    }

    /// The daily time the next automatic snapshot of the resource `name` of kind
    /// `resource_type` is due at, in the `HH:00` UTC form the modelled
    /// `AddOn$nextSnapshotTimeOfDay` reports ("the next daily time an automatic
    /// snapshot will be created"), and `None` when the store owes that resource
    /// no further copy.
    ///
    /// READ from the schedule the resource's `AutoSnapshot` add-on runs on
    /// ([`StoredAutoSnapshots::due_at_ms`]), which is the instant the clock takes
    /// the next copy at — so the time reported is the hour that copy lands in,
    /// and it follows the schedule as the clock re-arms it after each copy. A
    /// resource whose add-on is disabled is owed nothing and reports nothing.
    fn next_auto_snapshot_time_of_day(
        &self,
        resource_type: &'static str,
        name: &str,
    ) -> Option<String> {
        self.auto_snapshots
            .get(&(resource_type, name.to_string()))?
            .due_at_ms
            .map(utc_hour_of_day)
    }

    /// The same disks as [`Self::disks_attached_to`], captured whole for an
    /// automatic snapshot of the instance: a copy of that snapshot describes the
    /// disks it captured as the resources they are, so the records are kept
    /// rather than reduced to what `AutoSnapshotDetails$fromAttachedDisks`
    /// reports of them.
    fn attached_disk_captures(&self, instance_name: &str) -> Vec<StoredDisk> {
        self.disks_attached_to(instance_name)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The automatic snapshots this store holds of the resource `name` names,
    /// together with the `ResourceType` that resource reports — what
    /// `GetAutoSnapshots` answers for it. `None` when that name addresses no
    /// automatic snapshots of this account at all.
    ///
    /// The name is resolved as every operation that addresses a resource's
    /// automatic snapshots resolves one ([`Self::auto_snapshot_source`]), so a
    /// delete that kept a resource's copies kept them reportable under the name
    /// they were taken under. A resource whose add-on has never been enabled
    /// holds no copy, which is an EMPTY list rather than a refusal: the resource
    /// exists, and the answer is that nothing has been copied of it.
    pub fn auto_snapshots_of(&self, name: &str) -> Option<(&'static str, Vec<AutoSnapshotDetails>)> {
        let (resource_type, _) = self.auto_snapshot_source(name)?;
        let reported = self
            .auto_snapshots
            .get(&(resource_type, name.to_string()))
            .map(StoredAutoSnapshots::view)
            .unwrap_or_default();
        Some((resource_type, reported))
    }

    /// The automatic snapshot of the resource `name` of kind `resource_type`
    /// that `date` addresses — the latest RESTORABLE copy of it when `date` is
    /// absent, which is what a request asking for "the latest available
    /// automatic snapshot" addresses. `None` when the resource holds no copy the
    /// request addresses.
    ///
    /// The add-on takes one copy a day ([`StoredAutoSnapshots::take`]), so a
    /// date addresses exactly one of a resource's copies; the latest restorable
    /// one is the newest copy the service has finished taking, the copies being
    /// held oldest first. A copy that is still being taken is held, and
    /// addressable by its date — what a caller may do with one is the caller's
    /// own rule, so the lookup answers it rather than hiding it.
    ///
    /// The ONE lookup every operation that addresses an automatic snapshot of a
    /// resource resolves it through, so the same request shape finds the same
    /// copy at whichever of those doors it arrives.
    pub fn auto_snapshot(
        &self,
        resource_type: &'static str,
        name: &str,
        date: Option<&str>,
    ) -> Option<&StoredAutoSnapshot> {
        let taken = self
            .auto_snapshots
            .get(&(resource_type, name.to_string()))
            .map(|schedule| schedule.taken.as_slice())
            .unwrap_or_default();
        match date {
            Some(date) => taken.iter().find(|copy| copy.date == date),
            None => taken
                .iter()
                .rev()
                .find(|copy| copy.status == AUTO_SNAPSHOT_STATUS_SUCCESS),
        }
    }

    /// Remove the automatic snapshot of the resource `name` names that was taken
    /// on `date`. `None` when that name addresses no automatic snapshots of this
    /// account at all.
    ///
    /// A copy is addressed by the resource it was taken of plus its DATE — it
    /// has no name and no ARN of its own — and the add-on takes one copy a day,
    /// so the date identifies exactly one of a resource's copies. The name is
    /// resolved as every other operation that addresses an automatic snapshot
    /// resolves it ([`Self::auto_snapshot_source`]), so a copy a delete kept is
    /// deletable under the name it was taken under.
    ///
    /// Removing a copy takes nothing else with it: the other copies stay, and
    /// the schedule keeps the instant its next copy falls due, so a deletion
    /// neither stops the add-on nor makes it take a replacement. A resource left
    /// holding no copy and owed none — its add-on is disabled, or the resource
    /// itself is gone and this was the last copy a delete kept of it — holds no
    /// schedule either, exactly as it did before the add-on was ever enabled.
    pub fn delete_auto_snapshot(
        &mut self,
        name: &str,
        date: &str,
    ) -> Option<AutoSnapshotDeletion> {
        let (resource_type, location) = self.auto_snapshot_source(name)?;
        let key = (resource_type, name.to_string());
        let found = self
            .auto_snapshots
            .get_mut(&key)
            .and_then(|schedule| {
                let at = schedule
                    .taken
                    .iter()
                    .position(|taken| taken.date == date)?;
                schedule.taken.remove(at);
                Some(schedule.taken.is_empty() && schedule.due_at_ms.is_none())
            });
        match found {
            Some(emptied) => {
                if emptied {
                    self.auto_snapshots.remove(&key);
                }
                Some(AutoSnapshotDeletion::Deleted {
                    resource_type,
                    location,
                })
            }
            None => Some(AutoSnapshotDeletion::NoCopyForDate { resource_type }),
        }
    }

    /// Bring the automatic snapshot schedule of the resource `name` of kind
    /// `resource_type` into line with its add-ons at `now_ms`, where `hour` is
    /// the daily hour its enabled `AutoSnapshot` add-on carries and `None` says
    /// it carries no such add-on.
    ///
    /// The one place a schedule is started or stopped, so every operation that
    /// writes a resource's add-ons — the creates, through
    /// [`Self::insert_instance`] and [`Self::insert_disk`], and the add-on
    /// changes, through [`Self::with_add_on_record`] — starts exactly the same
    /// schedule. Enabling the add-on makes the service owe the resource a copy:
    /// the first one falls due a short interval later
    /// ([`AUTO_SNAPSHOT_FIRST_MILLIS`]), and from then on the copies follow the
    /// add-on's own daily hour. An ENABLED add-on therefore always has its next
    /// copy outstanding, which is also what
    /// [`Self::next_auto_snapshot_time_of_day`] reports. Disabling it stops the
    /// schedule and keeps the copies already taken, which are the account's
    /// until something deletes them.
    fn reconcile_auto_snapshots(
        &mut self,
        now_ms: i64,
        resource_type: &'static str,
        name: &str,
        hour: Option<&str>,
    ) {
        let key = (resource_type, name.to_string());
        let Some(hour) = hour else {
            if let Some(schedule) = self.auto_snapshots.get_mut(&key) {
                schedule.due_at_ms = None;
                if schedule.taken.is_empty() {
                    self.auto_snapshots.remove(&key);
                }
            }
            return;
        };
        let schedule = self.auto_snapshots.entry(key).or_default();
        // An add-on already taking copies keeps the schedule it is on, so
        // modifying it does not restart the wait.
        if schedule.due_at_ms.is_some() {
            return;
        }
        let first_due_ms = now_ms.saturating_add(AUTO_SNAPSHOT_FIRST_MILLIS);
        schedule.due_at_ms = Some(
            if schedule
                .taken
                .last()
                .is_none_or(|taken| taken.date != utc_date(first_due_ms))
            {
                first_due_ms
            } else {
                // A resource that already holds the copy for the day the first
                // one would fall on is not copied twice because the add-on was
                // switched off and on again: the copy it is owed is the next one
                // the add-on's own daily hour comes round for.
                next_auto_snapshot_due_ms(first_due_ms, hour)
            },
        );
    }

    /// The daily hour the enabled `AutoSnapshot` add-on of the resource `name`
    /// of kind `resource_type` carries, and `None` when this account holds no
    /// such resource or that resource has the add-on disabled.
    fn auto_snapshot_hour(&self, resource_type: &'static str, name: &str) -> Option<String> {
        let add_ons = if resource_type == RESOURCE_TYPE_INSTANCE {
            &self.instances.get(name)?.add_ons
        } else {
            &self.disks.get(name)?.add_ons
        };
        enabled_auto_snapshot_hour(add_ons)
    }

    /// Settle the automatic snapshots of the resource `name` of kind
    /// `resource_type`, whose record a delete has just removed from `location`,
    /// where `delete_add_ons` says that delete was asked to delete the
    /// resource's add-ons with it.
    ///
    /// The add-on itself lives on the resource's record, so it goes with that
    /// record either way; what the request decides is the fate of what the
    /// add-on PRODUCED. A delete asked for the add-ons takes the copies with
    /// them. A delete that was not asked KEEPS them — nothing asked for them to
    /// be deleted — and stops the schedule: no further copy is owed of a
    /// resource this account no longer holds, and the copies that are left stay
    /// addressed by the name they were taken under
    /// ([`Self::auto_snapshot_source`]) until something deletes them
    /// ([`Self::delete_auto_snapshot`]), or a new resource takes that name over
    /// ([`Self::discard_kept_auto_snapshots`]).
    ///
    /// A resource whose add-on took no copy leaves nothing behind either way:
    /// an empty schedule that is owed nothing is not held, exactly as it is not
    /// held before the add-on is ever enabled.
    fn settle_auto_snapshots_of_removed(
        &mut self,
        resource_type: &'static str,
        name: &str,
        location: &StoredLocation,
        delete_add_ons: bool,
    ) {
        let key = (resource_type, name.to_string());
        if delete_add_ons {
            self.auto_snapshots.remove(&key);
            return;
        }
        let Some(schedule) = self.auto_snapshots.get_mut(&key) else {
            return;
        };
        schedule.due_at_ms = None;
        if schedule.taken.is_empty() {
            self.auto_snapshots.remove(&key);
            return;
        }
        schedule.kept_source = Some(KeptAutoSnapshotSource {
            removed_at_ms: now_ms(),
            location: location.clone(),
        });
        self.drop_oldest_kept_auto_snapshots();
    }

    /// Drop the kept copies of the resources removed longest ago until this
    /// store holds no more than [`MAX_KEPT_AUTO_SNAPSHOT_SOURCES`] resources'
    /// worth of them.
    fn drop_oldest_kept_auto_snapshots(&mut self) {
        let mut kept: Vec<(i64, (&'static str, String))> = self
            .auto_snapshots
            .iter()
            .filter_map(|(key, schedule)| {
                Some((schedule.kept_source.as_ref()?.removed_at_ms, key.clone()))
            })
            .collect();
        if kept.len() <= MAX_KEPT_AUTO_SNAPSHOT_SOURCES {
            return;
        }
        kept.sort();
        let excess = kept.len() - MAX_KEPT_AUTO_SNAPSHOT_SOURCES;
        for (_, key) in kept.into_iter().take(excess) {
            self.auto_snapshots.remove(&key);
        }
    }

    /// Discard every automatic snapshot kept under the name `name`, called as a
    /// resource is stored under that name.
    ///
    /// A copy is addressed by the name of the resource it was taken of, and a
    /// resource created under that name takes the name over: the copies kept
    /// under it were taken of a resource this account no longer holds, so they
    /// are not this one's, and a resource starts out with the copies its own
    /// add-on has taken — none. Both kinds the add-on applies to are discarded,
    /// whichever kind the new resource is: a name is unique per resource type,
    /// so copies kept of an instance would otherwise sit behind a disk created
    /// under the same name and come back when that disk went. A schedule the
    /// account still holds the resource of is left exactly as it is, which is
    /// what keeps a resource's own copies across the writes that re-store its
    /// record.
    fn discard_kept_auto_snapshots(&mut self, name: &str) {
        for resource_type in [RESOURCE_TYPE_INSTANCE, RESOURCE_TYPE_DISK] {
            let key = (resource_type, name.to_string());
            if self
                .auto_snapshots
                .get(&key)
                .is_some_and(|schedule| schedule.kept_source.is_some())
            {
                self.auto_snapshots.remove(&key);
            }
        }
    }

    /// The kind of resource the name `name` addresses automatic snapshots of,
    /// and where that resource is: an instance or a block storage disk this
    /// account holds, or — once a delete has kept its copies — the resource
    /// those copies were taken of.
    ///
    /// ONE name addresses one resource, and the resources an automatic snapshot
    /// is taken of are the ones the `AutoSnapshot` add-on describes ("an Amazon
    /// Lightsail instance or disk"), so this asks the same two maps the add-on
    /// resolution does ([`Self::with_add_on_record`]), in the same order, and
    /// then the copies kept under that name. The ONE place an operation that
    /// addresses a resource's automatic snapshots resolves the name, so the same
    /// name finds the same copies at whichever of those doors it arrives.
    fn auto_snapshot_source(&self, name: &str) -> Option<(&'static str, StoredLocation)> {
        if let Some(instance) = self.instances.get(name) {
            return Some((RESOURCE_TYPE_INSTANCE, instance.location.clone()));
        }
        if let Some(disk) = self.disks.get(name) {
            return Some((RESOURCE_TYPE_DISK, disk.location.clone()));
        }
        [RESOURCE_TYPE_INSTANCE, RESOURCE_TYPE_DISK]
            .into_iter()
            .find_map(|resource_type| {
                let kept = self
                    .auto_snapshots
                    .get(&(resource_type, name.to_string()))?
                    .kept_source
                    .as_ref()?;
                Some((resource_type, kept.location.clone()))
            })
    }

    /// The disk snapshot `name` names, for a reporting operation.
    pub fn disk_snapshot(&self, name: &str) -> Option<&StoredDiskSnapshot> {
        self.disk_snapshots.get(name)
    }

    /// The static IP `name` names, for a reporting operation.
    pub fn static_ip(&self, name: &str) -> Option<&StoredStaticIp> {
        self.static_ips.get(name)
    }

    /// The static IP attached to the instance `instance_name`, while one is.
    ///
    /// The attachment lives on the STATIC IP record — the address is what is
    /// attached, exactly as a disk names the instance it is attached to — so the
    /// one way to find it is to ask the static IPs, and this is where that walk
    /// lives rather than in each handler that needs it. The walk is bounded by
    /// [`MAX_STATIC_IPS`].
    pub fn attached_static_ip(&self, instance_name: &str) -> Option<&StoredStaticIp> {
        self.static_ips
            .values()
            .find(|static_ip| static_ip.attached_to.as_deref() == Some(instance_name))
    }

    /// Store a newly allocated static IP.
    pub fn insert_static_ip(&mut self, static_ip: StoredStaticIp) {
        self.static_ips.insert(static_ip.name.clone(), static_ip);
    }

    /// Remove the static IP `name` names, answering it. `None` when nothing
    /// matched.
    ///
    /// Giving up the reservation gives up the attachment with it, for the reason
    /// spelled out on [`Self::remove_instance`]: an attachment names "the
    /// resources to which" a resource "is attached" (the modelled
    /// `StaticIp$attachedTo`), and an address this account no longer holds is
    /// not one of them — so the removal and the release are one step and no read
    /// can observe an instance holding an address the store does not have.
    ///
    /// The instance that held it keeps public addressing, drawn afresh from this
    /// store's one address allocator — see
    /// [`Self::redraw_public_address_without_static_ip`], which is where that
    /// fallback lives for every call that releases an attachment.
    pub fn remove_static_ip(&mut self, name: &str) -> Option<StoredStaticIp> {
        let static_ip = self.static_ips.remove(name)?;
        if let Some(instance_name) = static_ip.attached_to.clone() {
            self.redraw_public_address_without_static_ip(&instance_name);
        }
        Some(static_ip)
    }

    /// Release the attachment the static IP `name` holds, answering the instance
    /// it was attached to. `None` when the account holds no static IP of that
    /// name, or that static IP is attached to nothing — in which case nothing
    /// was changed.
    ///
    /// Only the ATTACHMENT is released: the address stays reserved for the
    /// account, so the record keeps every other member it has and a later read
    /// still reports the reservation — releasing the reservation is
    /// [`Self::remove_static_ip`]'s business. The instance it came off falls
    /// back to an ordinary public address, through the same
    /// [`Self::redraw_public_address_without_static_ip`] a release puts it on,
    /// so the two calls leave an instance in the same state.
    pub fn detach_static_ip(&mut self, name: &str) -> Option<String> {
        let instance_name = self.static_ips.get_mut(name)?.attached_to.take()?;
        self.redraw_public_address_without_static_ip(&instance_name);
        Some(instance_name)
    }

    /// Attach the static IP `name` to the instance `instance_name`, answering the
    /// reserved address the instance is reachable at from now on. `None` when the
    /// account holds no static IP of that name or no such instance — in which
    /// case nothing was changed.
    ///
    /// Both halves of the attachment are this ONE step, exactly as releasing it
    /// is [`Self::detach_static_ip`]'s: the attachment itself lives on the static
    /// IP record (the modelled `StaticIp$attachedTo` names the resource "to which
    /// the static IP is attached"), and the instance it names is reachable at the
    /// reserved address, because an attached static IP is the only thing that
    /// pins an instance's public IPv4 address — so `Instance$isStaticIp`, "a
    /// Boolean value indicating whether this instance has a static IP assigned to
    /// it", is true for as long as the attachment stands. Nothing is drawn from
    /// the address allocator here: the address the instance takes on is the one
    /// the reservation already holds, which is what makes it static.
    ///
    /// Attaching the static IP an instance already holds writes exactly what is
    /// already stored, so the call is idempotent; whether an attachment may be
    /// made at all — the static IP is free, the instance holds none, the instance
    /// has public IPv4 addressing — is the calling operation's question, decided
    /// before this is reached.
    pub fn attach_static_ip(&mut self, name: &str, instance_name: &str) -> Option<String> {
        let ip_address = self.static_ips.get(name)?.ip_address.clone();
        // The instance is written FIRST, so a name this store holds no instance
        // under leaves the reservation exactly as it was rather than pointing it
        // at an instance that is not there.
        let instance = self.instance_mut(instance_name)?;
        instance.is_static_ip = true;
        instance.public_ip_address = Some(ip_address.clone());
        self.static_ips.get_mut(name)?.attached_to = Some(instance_name.to_string());
        Some(ip_address)
    }

    /// Put the instance `instance_name` back on the public addressing it holds
    /// while no static IP is attached to it, drawn afresh from this store's one
    /// address allocator.
    ///
    /// An attached static IP is the ONLY thing that pins an instance's public
    /// IPv4 address ("To use the same IP address after stopping and starting an
    /// instance, create a static IP address and attach it to the instance" — the
    /// modelled `StartInstance` documentation), so an instance with none holds an
    /// ordinary address the platform assigned it, and `Instance$isStaticIp` — "a
    /// Boolean value indicating whether this instance has a static IP assigned to
    /// it" — is false again. The private address and the IPv6 addresses are left
    /// where they are: a static IP reserves one public IPv4 address (the modelled
    /// `StaticIp$ipAddress`), so it pinned neither.
    fn redraw_public_address_without_static_ip(&mut self, instance_name: &str) {
        // Which addresses the instance has at all is its own `ipAddressType`,
        // exactly as it is on a start; an instance this store does not hold has
        // none to redraw.
        let Some(ip_address_type) = self
            .instance(instance_name)
            .map(|instance| instance.ip_address_type.clone())
        else {
            return;
        };
        // ONE draw from the one allocator every address in this store comes out
        // of, so the address the instance falls back to is one no resource in
        // this process has held. The private address and the IPv6 addresses it
        // also yields are dropped — the instance keeps its own — which spends
        // the offset rather than wasting it.
        let (_private, public, _ipv6) = self.fresh_addresses(&ip_address_type);
        if let Some(instance) = self.instance_mut(instance_name) {
            instance.is_static_ip = false;
            instance.public_ip_address = public;
        }
    }

    /// Store a newly created disk snapshot.
    pub fn insert_disk_snapshot(&mut self, snapshot: StoredDiskSnapshot) {
        self.disk_snapshots.insert(snapshot.name.clone(), snapshot);
    }

    /// Remove the disk snapshot `name` names, answering it. `None` when nothing
    /// matched.
    ///
    /// Nothing else in the store points AT a snapshot: a disk created from one
    /// copies what it needs out of the snapshot record as it is created, and an
    /// export record captures its source the same way — which is what lets the
    /// service say that "regardless of which prior snapshots have been deleted,
    /// all active snapshots will have access to all the information needed to
    /// restore the disk". So removing the record is the whole deletion, and the
    /// clock-owed transition it was carrying goes with it.
    pub fn remove_disk_snapshot(&mut self, name: &str) -> Option<StoredDiskSnapshot> {
        self.disk_snapshots.remove(name)
    }

    /// The instance snapshot `name` names, for a reporting operation.
    pub fn instance_snapshot(&self, name: &str) -> Option<&StoredInstanceSnapshot> {
        self.instance_snapshots.get(name)
    }

    /// Whether this account already holds a snapshot called `name`, of EITHER
    /// kind of snapshot the service's snapshot operations address.
    ///
    /// A snapshot name addresses ONE snapshot: every operation that takes a
    /// snapshot name takes it unqualified — `ExportSnapshot`'s
    /// `sourceSnapshotName`, `CopySnapshot`'s `sourceSnapshotName`, the
    /// `resourceName` the tag plane resolves ([`Self::with_tagged_record`]) —
    /// and resolves it by asking the disk snapshots and then the instance
    /// snapshots, so a name both maps held would leave the second record
    /// unreachable through every one of them. This is the one place the
    /// question "is this snapshot name taken?" is asked, so each operation that
    /// MINTS a snapshot name refuses exactly the names that resolution already
    /// answers for.
    pub fn holds_snapshot_named(&self, name: &str) -> bool {
        self.disk_snapshots.contains_key(name) || self.instance_snapshots.contains_key(name)
    }

    /// Store a newly created instance snapshot.
    pub fn insert_instance_snapshot(&mut self, snapshot: StoredInstanceSnapshot) {
        self.instance_snapshots
            .insert(snapshot.name.clone(), snapshot);
    }

    /// Remove the instance snapshot `name` names, answering it. `None` when
    /// nothing matched.
    ///
    /// Nothing else in the store points AT an instance snapshot: the snapshot
    /// itself holds the point-in-time copy of its source — the blueprint, the
    /// bundle, the system volume size and the block storage disks the instance
    /// had when it was taken — and an instance created from it, or an export of
    /// it, copies what it needs out of that record as it is created. So removing
    /// the record is the whole deletion: the clock-owed transition it was still
    /// carrying goes with it, and the instance it was taken of, together with
    /// every other snapshot of that instance, is untouched.
    pub fn remove_instance_snapshot(&mut self, name: &str) -> Option<StoredInstanceSnapshot> {
        self.instance_snapshots.remove(name)
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

    /// Store a newly created CloudFormation stack record.
    pub fn insert_cloud_formation_stack_record(
        &mut self,
        record: StoredCloudFormationStackRecord,
    ) {
        self.cloud_formation_stack_records
            .insert(record.name.clone(), record);
    }

    /// The CloudFormation stack record built from the export snapshot record
    /// `export_snapshot_record_name` whose stack has not finished yet, if the
    /// account holds one.
    ///
    /// The walk is bounded by [`MAX_CLOUD_FORMATION_STACK_RECORDS`].
    pub fn cloud_formation_stack_record_in_progress_for(
        &self,
        export_snapshot_record_name: &str,
    ) -> Option<&StoredCloudFormationStackRecord> {
        self.cloud_formation_stack_records.values().find(|record| {
            record.state == RECORD_STATE_STARTED
                && record.source_export_snapshot_record_name() == Some(export_snapshot_record_name)
        })
    }

    /// Remove the instance `name` names, answering the record and every
    /// reference its removal released. `None` when nothing matched.
    ///
    /// Removing a resource also releases every live reference another stored
    /// record holds to it — an attachment, an access grant or an alarm's
    /// monitored resource: each of those names a resource this account holds
    /// (the modelled `Disk$attachedTo` and `StaticIp$attachedTo` name "the
    /// resources to which" one resource "is attached";
    /// `Bucket$resourcesReceivingAccess` describes "Lightsail instances that
    /// have access to the bucket"; an alarm's `monitoredResourceName` resolves
    /// through [`Self::monitored_resource`]), and a resource this account no
    /// longer holds is not one of them — so the removal and the release are
    /// one step, and no read can observe a reference to a record that is gone.
    /// `delete_add_ons` says the delete was asked to delete the instance's
    /// add-ons with it, which is what decides the fate of the copies the
    /// instance's add-on took — see
    /// [`Self::settle_auto_snapshots_of_removed`].
    pub fn remove_instance(
        &mut self,
        name: &str,
        delete_add_ons: bool,
    ) -> Option<RemovedInstance> {
        let instance = self.instances.remove(name)?;
        self.settle_auto_snapshots_of_removed(
            RESOURCE_TYPE_INSTANCE,
            name,
            &instance.location,
            delete_add_ons,
        );
        let released_attachments = self.release_attachments_to_instance(name);
        Some(RemovedInstance {
            instance,
            released_attachments,
        })
    }

    /// Release every reference the store holds to the instance `name`,
    /// answering one entry per release so the caller can log it.
    ///
    /// Five kinds of record can hold one. Four of them keep the reference in a
    /// member they can also hold EMPTY, so the release is a rewrite of that
    /// member: a block storage disk (`attachedTo` plus the device `path` it is
    /// mounted at), a static IP (`attachedTo`), a load balancer (the instance
    /// registered with it), and a bucket (the instance it reports in
    /// `resourcesReceivingAccess`, which "describe Lightsail instances that have
    /// access to the bucket"). Each returns to the state it has while it holds
    /// nothing — for a disk that is `available`, the modelled `DiskState` value
    /// for a disk that "exists and is attached to nothing" next to `in-use`; for
    /// a bucket, one fewer resource receiving access.
    ///
    /// The fifth is an ALARM, whose reference is required rather than optional,
    /// so it is removed with the resource instead — see
    /// [`Self::release_alarms_monitoring`].
    ///
    /// A DISTRIBUTION's origin is not among them: a distribution cannot be left
    /// holding none, so the reference is not released here but stands in the
    /// delete's way instead — see
    /// [`Self::in_use_as_distribution_origin`].
    ///
    /// Every walk is bounded: the disks by [`MAX_DISKS`], the static IPs by
    /// [`MAX_STATIC_IPS`], the load balancers by
    /// [`MAX_LOAD_BALANCERS_PER_REGION`] per region, the buckets by
    /// [`MAX_BUCKETS`] and the alarms by [`MAX_ALARMS`].
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
            // other member it has. The instance is not put back on platform
            // addressing here (as `detach_static_ip` does through
            // [`Self::redraw_public_address_without_static_ip`]): this walk runs
            // while the instance itself is being removed, so there is no record
            // left to hold an address.
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
                .any(|registration| registration.name == name)
            {
                continue;
            }
            balancer
                .attached_instances
                .retain(|registration| registration.name != name);
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_DETACH_INSTANCES_FROM_LOAD_BALANCER,
                resource_type: RESOURCE_TYPE_LOAD_BALANCER,
                resource_name: balancer.name.clone(),
                location: balancer.location.clone(),
            });
        }
        for bucket in self.buckets.values_mut() {
            // The grant lives on the BUCKET's record and is keyed on the
            // instance's name alone, so taking it back is the same write
            // `SetResourceAccessForBucket` makes with `deny` — which is why the
            // release is logged under that modelled `OperationType`, against the
            // bucket whose record changed.
            if !bucket.set_resource_access(name, RESOURCE_TYPE_INSTANCE, false) {
                continue;
            }
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_SET_RESOURCE_ACCESS_FOR_BUCKET,
                resource_type: RESOURCE_TYPE_BUCKET,
                resource_name: bucket.name.clone(),
                location: bucket.location.clone(),
            });
        }
        released.extend(self.release_alarms_monitoring(RESOURCE_TYPE_INSTANCE, name));
        released
    }

    /// Remove every alarm the store holds that monitors the resource `name` of
    /// kind `resource_type`, answering one entry per removal so the caller can
    /// log it.
    ///
    /// An alarm NAMES the resource it watches: `monitoredResourceName` is a
    /// required member of the put that creates one, the record keeps what that
    /// name resolved to ([`Self::monitored_resource`], which resolves only a
    /// resource this account holds) and every read reports it as
    /// `Alarm$monitoredResourceInfo`, "an object that lists information about
    /// the resource monitored by the alarm". That reference cannot be left
    /// empty the way a disk's `attachedTo` can, so an alarm whose monitored
    /// resource is removed goes with it rather than being rewritten — which is
    /// also the only outcome the put itself admits, since it refuses to create
    /// an alarm on a name that resolves to nothing. Each removal is logged as
    /// that alarm's own `DeleteAlarm`, against the alarm whose record went, so
    /// the per-metric alarm budget [`Self::alarms_on_metric`] counts and the
    /// `GetAlarms` listing hold only alarms watching resources the account
    /// still has.
    ///
    /// The walk is bounded by [`MAX_ALARMS`].
    fn release_alarms_monitoring(
        &mut self,
        resource_type: &'static str,
        name: &str,
    ) -> Vec<ReleasedAttachment> {
        let monitoring: Vec<String> = self
            .alarms
            .values()
            .filter(|alarm| {
                alarm.monitored_resource.resource_type == resource_type
                    && alarm.monitored_resource.name == name
            })
            .map(|alarm| alarm.name.clone())
            .collect();
        let mut released = Vec::with_capacity(monitoring.len());
        for alarm_name in monitoring {
            // The names were just read out of this map under the same guard, so
            // every one of them is still there.
            if let Some(alarm) = self.alarms.remove(&alarm_name) {
                released.push(ReleasedAttachment {
                    operation_type: OPERATION_TYPE_DELETE_ALARM,
                    resource_type: RESOURCE_TYPE_ALARM,
                    resource_name: alarm.name,
                    location: alarm.location,
                });
            }
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

    /// Remove the container service `name` names in `region`, answering the
    /// record that was stored under it, or `None` when this account holds no
    /// container service of that name there.
    ///
    /// Everything a container service carries lives ON this record — its power
    /// and scale, its tags, the groups of public domain names it served, the
    /// access its image puller role held, its deployment history and the
    /// container images registered to it — so removing the record is what
    /// removes them all, and no later read can report a deployment or a
    /// registered image of a service this account no longer holds. The name is
    /// free again for a service that inherits nothing from this one, exactly as
    /// it is for every other removal here. A CERTIFICATE a group of public
    /// domain names was keyed by is NOT removed with it: a certificate is a
    /// resource of its own, so it is released by this removal rather than
    /// deleted by it. The operation log is append-only, so the entries this
    /// service's earlier calls wrote stay readable.
    pub fn remove_container_service(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<StoredContainerService> {
        self.container_services
            .remove(&(region.to_string(), name.to_string()))
    }

    /// The bucket `name` names in `region`, for a reporting operation.
    pub fn bucket(&self, region: &str, name: &str) -> Option<&StoredBucket> {
        self.buckets.get(&(region.to_string(), name.to_string()))
    }

    /// The CDN distribution `name` names, for a reporting operation.
    pub fn distribution(&self, name: &str) -> Option<&StoredDistribution> {
        self.distributions.get(name)
    }

    /// The most recent cache reset of ANY distribution this account holds, or
    /// `None` when no distribution's cache has ever been reset.
    ///
    /// What `GetDistributionLatestCacheReset` reports when the request names no
    /// distribution: its `distributionName` documentation says that "when
    /// omitted, the response includes the latest cache reset timestamp of all
    /// your distributions", and the result carries ONE timestamp and ONE status
    /// — so the answer is the latest of them. Ties go to the distribution that
    /// sorts last in the store's own total order, which is stable across reads.
    pub fn latest_cache_reset(&self) -> Option<StoredCacheReset> {
        self.distributions
            .values()
            .filter_map(|distribution| distribution.latest_cache_reset)
            .max_by_key(|reset| reset.created_at_ms)
    }

    /// Store a newly created CDN distribution.
    pub fn insert_distribution(&mut self, distribution: StoredDistribution) {
        self.distributions
            .insert(distribution.name.clone(), distribution);
    }

    /// Remove the CDN distribution `name` names, answering the record that was
    /// stored under it, or `None` when this account holds no distribution of
    /// that name.
    ///
    /// Everything a distribution carries lives ON this record — its origin, its
    /// cache behaviours, its tags, the certificate it names and the last reset
    /// of its cached content — so removing the record is what removes them all.
    /// A certificate itself is NOT removed with it: a certificate is a resource
    /// of its own, and the number of resources it is attached to is counted from
    /// the records that name it ([`Self::certificate_in_use_resource_count`]),
    /// so it is released by this removal rather than deleted by it. The
    /// operation log is append-only, so the entries a deleted distribution's
    /// earlier calls wrote stay readable.
    pub fn remove_distribution(&mut self, name: &str) -> Option<StoredDistribution> {
        self.distributions.remove(name)
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

    /// How many of this account's distributions pull their content from the
    /// resource `name` names in `region`, which this store resolved to
    /// `resource_type`.
    ///
    /// Counted from the distribution records that HOLD the origin rather than
    /// from a counter kept on the origin resource, so the number cannot drift
    /// from the distributions that actually name it — the same way
    /// [`Self::certificate_in_use_resource_count`] counts the attachments of a
    /// certificate. A distribution's origin is held as the name, the Region and
    /// the resource type the create resolved ([`StoredDistribution::origin`]),
    /// so a resource is the origin of a distribution exactly when all three
    /// match. The walk is bounded by [`MAX_DISTRIBUTIONS`].
    pub fn distributions_with_origin(&self, region: &str, name: &str, resource_type: &str) -> usize {
        self.distributions
            .values()
            .filter(|distribution| {
                distribution.origin.name.as_deref() == Some(name)
                    && distribution.origin.region_name.as_deref() == Some(region)
                    && distribution.origin.resource_type.as_deref() == Some(resource_type)
            })
            .count()
    }

    /// Why a delete of the resource `name` names in `region`, which this store
    /// holds as `resource_type`, cannot be served while distributions of this
    /// account still pull their content from it — or `None` when none does, so
    /// nothing stands in the delete's way.
    ///
    /// A distribution pulls its content from ONE origin resource of this
    /// account, and both write paths refuse an origin that addresses nothing
    /// ([`Self::resolve_origin`]), so a resource a distribution names is in use
    /// by it and a delete that would leave that distribution pulling from a
    /// resource the account no longer holds is refused. The condition is the one
    /// the modelled `DeleteBucketRequest$forceDelete` documents ("You must force
    /// delete the bucket if it has one of the following conditions: The bucket
    /// is the origin of a distribution"), counted from the distribution records
    /// that name the resource ([`Self::distributions_with_origin`]) — so it is
    /// the same condition whichever of the four kinds of origin resource the
    /// delete names, and a delete that offers a force (`DeleteBucket` is the
    /// only one that does) is what overrides it.
    pub fn in_use_as_distribution_origin(
        &self,
        region: &str,
        name: &str,
        resource_type: &str,
    ) -> Option<String> {
        let condition = origin_of_distributions_condition(self.distributions_with_origin(
            region,
            name,
            resource_type,
        ))?;
        Some(format!(
            "{resource_type} is in use: {name} ({condition}); delete the distribution, or \
             point it at another origin, before deleting the {resource_type}"
        ))
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

    /// Remove the managed relational database `name` names, answering the
    /// record it removed together with every reference the removal released.
    /// `None` when nothing matched.
    ///
    /// No attachment names a database — a database attaches to no instance,
    /// disk or load balancer — so the one kind of record that can hold a live
    /// reference to it is an ALARM, which a database can be the monitored
    /// resource of: see [`Self::release_alarms_monitoring`] for why that
    /// reference is removed with the database rather than rewritten. A SNAPSHOT
    /// of it is deliberately left where it is: a snapshot is a point-in-time
    /// copy that carries its own copy of everything it reports about its source
    /// ([`StoredRelationalDatabaseSnapshot`]), which is what lets the final
    /// snapshot a delete takes outlive the database it was taken of.
    pub fn remove_relational_database(
        &mut self,
        name: &str,
    ) -> Option<RemovedRelationalDatabase> {
        let database = self.relational_databases.remove(name)?;
        let released_references =
            self.release_alarms_monitoring(RESOURCE_TYPE_RELATIONAL_DATABASE, name);
        Some(RemovedRelationalDatabase {
            database,
            released_references,
        })
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

    /// Remove the managed relational database snapshot `name` names, answering
    /// it. `None` when nothing matched.
    ///
    /// Nothing else in the store points AT a database snapshot: a database
    /// restored from one copies everything it needs out of the snapshot record
    /// as it is created ([`Self::snapshot_relational_database`] captures the
    /// source the same way), so each snapshot is an independent point-in-time
    /// copy and removing its record is the whole deletion — the clock-owed
    /// transition it was still carrying goes with it, and the database it was
    /// taken of, and every other snapshot of that database, are untouched.
    pub fn remove_relational_database_snapshot(
        &mut self,
        name: &str,
    ) -> Option<StoredRelationalDatabaseSnapshot> {
        self.relational_database_snapshots.remove(name)
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
            master_user_password: Some(source.master_user_password.password.clone()),
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
        // Taking a snapshot is something the service does TO the source
        // database, so the source's own event log carries it — and a second
        // entry when the copy can be restored from (see [`Self::advance_clock`]).
        if let Some(database) = self
            .relational_databases
            .get_mut(&snapshot.from_relational_database_name)
        {
            database.record_event(
                now_ms,
                format!(
                    "{RELATIONAL_DATABASE_EVENT_BACKUP_STARTED}: {}",
                    snapshot.name
                ),
                &[RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP],
            );
        }
        snapshot
    }

    /// The alarm `name` names, for a reporting operation.
    pub fn alarm(&self, name: &str) -> Option<&StoredAlarm> {
        self.alarms.get(name)
    }

    /// The alarm `name` names, for an operation the spec says mutates it —
    /// putting it into the state a test asks for.
    ///
    /// The name is this record's store key, so it may not be reassigned through
    /// this handle; no operation in this slice renames an alarm.
    pub fn alarm_mut(&mut self, name: &str) -> Option<&mut StoredAlarm> {
        self.alarms.get_mut(name)
    }

    /// The contact method `protocol` addresses in `region`, for an operation the
    /// spec says mutates it — sending it a fresh verification request.
    ///
    /// The `(region, protocol)` pair is this record's store key, so neither may
    /// be reassigned through this handle; a contact method never changes protocol
    /// or Region.
    pub fn contact_method_mut(
        &mut self,
        region: &str,
        protocol: &str,
    ) -> Option<&mut StoredContactMethod> {
        self.contact_methods
            .get_mut(&(region.to_string(), protocol.to_string()))
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

    /// The SSH key pair `name` names, for a reporting operation.
    pub fn key_pair(&self, name: &str) -> Option<&StoredKeyPair> {
        self.key_pairs.get(name)
    }

    /// Store a newly created or imported SSH key pair.
    pub fn insert_key_pair(&mut self, key_pair: StoredKeyPair) {
        self.key_pairs.insert(key_pair.name.clone(), key_pair);
    }

    /// Remove the SSH key pair `name` names, answering it. `None` when nothing
    /// matched.
    ///
    /// Removing the record is what removes the public key from the service,
    /// which is the whole of what the modelled `DeleteKeyPair` deletes: an
    /// instance launched with that pair keeps the `sshKeyName` its own record
    /// carries, because the pair is named there rather than referenced.
    pub fn remove_key_pair(&mut self, name: &str) -> Option<StoredKeyPair> {
        self.key_pairs.remove(name)
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

    /// The contact method of `protocol` in `region`, for a reporting operation.
    pub fn contact_method(&self, region: &str, protocol: &str) -> Option<&StoredContactMethod> {
        self.contact_methods
            .get(&(region.to_string(), protocol.to_string()))
    }

    /// Every contact method of `region`, in the store's own total order.
    pub fn contact_methods_in(&self, region: &str) -> Vec<&StoredContactMethod> {
        self.contact_methods
            .iter()
            .filter(|((method_region, _), _)| method_region == region)
            .map(|(_, method)| method)
            .collect()
    }

    /// Store the contact method `CreateContactMethod` just created in `region`.
    ///
    /// One Region holds one contact method per protocol, and the model declares
    /// no operation that updates one, so creating under a protocol that already
    /// has a contact method REPLACES that record rather than adding a second.
    pub fn insert_contact_method(&mut self, region: &str, method: StoredContactMethod) {
        self.contact_methods
            .insert((region.to_string(), method.protocol.clone()), method);
    }

    /// Remove the contact method of `protocol` in `region`, answering it. `None`
    /// when nothing matched.
    pub fn remove_contact_method(
        &mut self,
        region: &str,
        protocol: &str,
    ) -> Option<StoredContactMethod> {
        self.contact_methods
            .remove(&(region.to_string(), protocol.to_string()))
    }

    /// The resource an alarm would monitor under `name` for a request made in
    /// `region`, or `None` when this account holds no alarmable resource of that
    /// name there.
    ///
    /// ONE name addresses one resource — the reasoning is spelled out on
    /// [`Self::with_tagged_record`]: `GetActiveNames` reports the account's
    /// resources as a single flat list of names with no resource type
    /// qualifying them, and each kind's own create refuses a name already
    /// taken — so at most one of the three below holds a given name and the
    /// order they are searched in is not a tie-break.
    ///
    /// Which resource types those are is [`MonitoredResource`]; each is searched
    /// the way that resource's OWN operations address it, the way
    /// [`Self::with_tagged_record`] searches them, so an alarm can watch exactly
    /// what those operations serve and no more. An instance and a relational
    /// database are held by name alone — `GetInstance` and
    /// `GetRelationalDatabase` serve them whichever Region the request is made
    /// from — and a load balancer is held under its Region together with its
    /// name, because a load balancer is a REGIONAL resource whose own
    /// documentation caps them "per AWS Region in your account", so the one a
    /// name addresses is the one in `region`.
    pub fn monitored_resource(&self, region: &str, name: &str) -> Option<MonitoredResource> {
        if let Some(instance) = self.instances.get(name) {
            return Some(MonitoredResource {
                name: instance.name.clone(),
                arn: instance.arn.clone(),
                resource_type: RESOURCE_TYPE_INSTANCE,
                location: StoredLocation::regional(&instance.location.region_name),
            });
        }
        if let Some(database) = self.relational_databases.get(name) {
            return Some(MonitoredResource {
                name: database.name.clone(),
                arn: database.arn.clone(),
                resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
                location: StoredLocation::regional(&database.location.region_name),
            });
        }
        let balancer = self.load_balancer(region, name)?;
        Some(MonitoredResource {
            name: balancer.name.clone(),
            arn: balancer.arn.clone(),
            resource_type: RESOURCE_TYPE_LOAD_BALANCER,
            location: StoredLocation::regional(&balancer.location.region_name),
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

    /// Remove the bucket `name` names in `region`, answering it together with
    /// every reference the removal released. `None` when nothing matched.
    ///
    /// Only the control-plane RECORD: the bucket's objects live in this emulator's
    /// one object store, which [`release_bucket_object_store`] is what releases
    /// (it takes that store's lock, so it is called with no guard of this store
    /// held).
    ///
    /// One kind of record can hold a live reference to a bucket that the removal
    /// itself has to release: ANOTHER bucket's access log configuration, whose
    /// `destination` "can be a Lightsail bucket in the same account, and in the
    /// same Amazon Web Services Region as the source bucket" — see
    /// [`Self::release_access_log_destinations`]. A DISTRIBUTION's origin is not
    /// among them, for the reason spelled out on
    /// [`Self::release_attachments_to_instance`]: a distribution cannot be left
    /// holding none, so that reference stands in the delete's way instead.
    pub fn remove_bucket(&mut self, region: &str, name: &str) -> Option<RemovedBucket> {
        let bucket = self
            .buckets
            .remove(&(region.to_string(), name.to_string()))?;
        // Released AFTER the record is gone, so the walk cannot reach the bucket
        // being removed: a bucket whose access logs were saved in itself holds
        // no reference once its own record is removed, and nothing is logged
        // against a record this account no longer holds.
        let released_references = self.release_access_log_destinations(region, name);
        Some(RemovedBucket {
            bucket,
            released_references,
        })
    }

    /// Release every access log configuration that saved its logs in the bucket
    /// `name` in `region`, answering one entry per release so the caller can log
    /// it.
    ///
    /// A bucket's access logs are saved in another bucket of the same account and
    /// the same Region, and the destination "is required when enabling the access
    /// log for a bucket, and should be omitted when disabling the access log", so
    /// a source bucket whose destination this account no longer holds has nowhere
    /// to write: its configuration returns to the state it has while it names no
    /// destination — the DISABLED form, exactly the write `UpdateBucket` makes
    /// when it is asked to turn the access log off. Only buckets of the SAME
    /// Region are walked, because that is the only Region a destination resolves
    /// in.
    ///
    /// The walk is bounded by [`MAX_BUCKETS`].
    fn release_access_log_destinations(
        &mut self,
        region: &str,
        name: &str,
    ) -> Vec<ReleasedAttachment> {
        let mut released = Vec::new();
        for ((bucket_region, _), bucket) in self.buckets.iter_mut() {
            if bucket_region != region
                || bucket.access_log_config.destination.as_deref() != Some(name)
            {
                continue;
            }
            bucket.access_log_config = BucketAccessLogConfig {
                enabled: false,
                destination: None,
                prefix: None,
            };
            // The configuration lives on the SOURCE bucket's record, so taking
            // the destination back is the same write `UpdateBucket` makes when
            // disabling the access log — which is why the release is logged
            // under that modelled `OperationType`, against the bucket whose
            // record changed.
            released.push(ReleasedAttachment {
                operation_type: OPERATION_TYPE_UPDATE_BUCKET,
                resource_type: RESOURCE_TYPE_BUCKET,
                resource_name: bucket.name.clone(),
                location: bucket.location.clone(),
            });
        }
        released
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

    /// How many SSL/TLS certificates of load balancers this account holds,
    /// across every Region — what [`MAX_LOAD_BALANCER_TLS_CERTIFICATES`]
    /// bounds.
    ///
    /// Counted from the load balancers that hold them rather than from a
    /// counter, so the number cannot drift from the records that actually carry
    /// a certificate. The walk is bounded by
    /// [`MAX_LOAD_BALANCERS_PER_REGION`] load balancers per Region, each
    /// holding at most [`MAX_CERTIFICATES_PER_LOAD_BALANCER`].
    pub fn load_balancer_tls_certificate_count(&self) -> usize {
        self.load_balancers
            .values()
            .map(|balancer| balancer.certificates.len())
            .sum()
    }

    /// Store a newly created load balancer.
    pub fn insert_load_balancer(&mut self, balancer: StoredLoadBalancer) {
        self.load_balancers.insert(
            (balancer.location.region_name.clone(), balancer.name.clone()),
            balancer,
        );
    }

    /// The certificate `name` names, for a reporting operation.
    pub fn certificate(&self, name: &str) -> Option<&StoredCertificate> {
        self.certificates.get(name)
    }

    /// The certificate `name` names, when it is one this account may secure
    /// traffic with — the ONE owner of that rule.
    ///
    /// A certificate a resource of this account references has to be a
    /// certificate this account holds, addressed by its exact name, and one the
    /// certificate authority has already issued: "Only certificates with a
    /// status of ISSUED can be attached to a distribution", and "You must create
    /// and validate an SSL/TLS certificate before you can use public domain
    /// names with your container service." Every operation that records a
    /// certificate reference — on a distribution, or as the key of a container
    /// service's public domain names — asks here FIRST, so a request naming a
    /// certificate this account does not hold, or one still being validated, is
    /// refused before anything is written and no record is left naming a
    /// certificate that secures nothing.
    pub fn issued_certificate(
        &self,
        name: &str,
    ) -> Result<&StoredCertificate, CertificateNotUsable> {
        let certificate = self
            .certificate(name)
            .ok_or(CertificateNotUsable::DoesNotExist)?;
        if !certificate.is_issued() {
            return Err(CertificateNotUsable::NotIssued {
                status: certificate.status,
            });
        }
        Ok(certificate)
    }

    /// Why the public domain names `names` carries are not ones a container
    /// service may serve, or `None` when every certificate they are keyed by is
    /// one this account holds and the certificate authority has issued.
    ///
    /// A container service's `publicDomainNames` is keyed by the NAME of the
    /// SSL/TLS certificate that secures the domains listed under it, so the
    /// certificate that key names answers the same exists-and-issued rule a
    /// distribution's certificate does — asked of the store's one owner of it,
    /// [`Self::issued_certificate`]. Whichever operation records those names
    /// asks here BEFORE its first write, so a refused request leaves no
    /// container service reporting a domain under a certificate that secures
    /// nothing. The map is ordered, so the certificate the refusal names is the
    /// same one on every run.
    pub fn unusable_public_domain_certificate(
        &self,
        names: &BTreeMap<String, Vec<String>>,
    ) -> Option<(String, CertificateNotUsable)> {
        names.keys().find_map(|certificate_name| {
            self.issued_certificate(certificate_name)
                .err()
                .map(|refusal| (certificate_name.clone(), refusal))
        })
    }

    /// Store a newly requested certificate.
    pub fn insert_certificate(&mut self, certificate: StoredCertificate) {
        self.certificates
            .insert(certificate.name.clone(), certificate);
    }

    /// Remove the SSL/TLS certificate `name` names, answering the record that
    /// was stored under it, or `None` when this account holds no certificate of
    /// that name.
    ///
    /// Everything a certificate carries lives ON this record — the domains it
    /// covers and their validation, its tags, its serial number and the window
    /// it is valid for — so removing the record is what removes them all, and
    /// no later read, listing or tag change can reach a certificate this
    /// account no longer holds. Nothing else in the store is rewritten: a
    /// distribution and a container service are each a resource of their own,
    /// and whether any resource still references this certificate is what the
    /// caller asks [`Self::certificate_in_use_resource_count`] BEFORE removing
    /// it, so no record is ever left naming a certificate that secures
    /// nothing.
    /// The operation log is append-only, so the entries this certificate's
    /// earlier calls wrote stay readable.
    pub fn remove_certificate(&mut self, name: &str) -> Option<StoredCertificate> {
        self.certificates.remove(name)
    }

    /// How many resources of this account the certificate `name` is attached to,
    /// which is what `Certificate$inUseResourceCount` reports — "the number of
    /// Lightsail resources that the certificate is attached to".
    ///
    /// A resource is attached to a certificate exactly when its own record
    /// references it, and this store records such a reference in the two places
    /// [`Self::issued_certificate`] guards on the way in: a DISTRIBUTION names
    /// it in `certificateName`, and a CONTAINER SERVICE keys a group of its
    /// `publicDomainNames` by it. Both are counted, so every resource whose
    /// record would be left naming a certificate that secures nothing is one
    /// this number reports — which is what the delete's in-use refusal rests on.
    ///
    /// Counted from the records that hold the reference rather than from a
    /// counter, so the number cannot drift from the resources that actually
    /// name the certificate. A certificate of this store is held by its name
    /// alone while a container service is held per Region, and the write side
    /// resolves a certificate by name alone too, so a service of ANY Region
    /// that keys its domains by this certificate is one of the resources it is
    /// attached to. The walk is bounded by [`MAX_DISTRIBUTIONS`] plus
    /// [`MAX_CONTAINER_SERVICES`].
    pub fn certificate_in_use_resource_count(&self, name: &str) -> i32 {
        let distributions = self
            .distributions
            .values()
            .filter(|distribution| distribution.certificate_name.as_deref() == Some(name))
            .count();
        let container_services = self
            .container_services
            .values()
            .filter(|service| service.public_domain_names.contains_key(name))
            .count();
        i32::try_from(distributions.saturating_add(container_services)).unwrap_or(i32::MAX)
    }

    /// The certificate `name` names as the modelled `CertificateSummary`, with
    /// the resources it is attached to counted.
    ///
    /// The one way a reporting operation renders a certificate: the reference
    /// lives on the record of the resource that holds it — a distribution, or a
    /// container service whose public domain names it keys — so assembling the
    /// view needs the whole store, not just the certificate.
    pub fn certificate_view(&self, name: &str, include_details: bool) -> Option<CertificateSummary> {
        let certificate = self.certificates.get(name)?;
        Some(self.certificate_record_view(name, certificate, include_details))
    }

    /// [`Self::certificate_view`] for a caller that already HOLDS the record —
    /// a page walk, which took the record from the store to decide it belongs
    /// on the page and so has nothing left to look up.
    ///
    /// This is where the assembly itself lives, and [`Self::certificate_view`]
    /// is the name lookup in front of it, so the single-record read and the
    /// listing render a certificate through ONE pairing of the record with the
    /// count only the whole store knows: a change to how a reported certificate
    /// is assembled is made here once and reaches both.
    pub fn certificate_record_view(
        &self,
        name: &str,
        certificate: &StoredCertificate,
        include_details: bool,
    ) -> CertificateSummary {
        certificate.view(
            self.certificate_in_use_resource_count(name),
            include_details,
        )
    }

    /// The distribution `name` names, for an operation the spec says mutates it
    /// — attaching a certificate to it, or releasing the one it holds.
    ///
    /// `name` is this record's store key, so it must not be reassigned through
    /// this handle; no operation in this slice renames a distribution.
    pub fn distribution_mut(&mut self, name: &str) -> Option<&mut StoredDistribution> {
        self.distributions.get_mut(name)
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

    /// One page of the log entries recorded against `resource_name`, oldest
    /// first, together with the id of the first entry of the page after it —
    /// present only while entries remain.
    ///
    /// The log is append-only, so an entry outlives the resource it was
    /// recorded against: a `DeleteInstance` entry is still reported under the
    /// name of the instance it deleted. Reporting is therefore by NAME over the
    /// log, never conditional on the resource still existing, and a name the log
    /// holds nothing for reports an empty page.
    ///
    /// A page token NAMES the entry a page starts at (an operation id) rather
    /// than carrying an index, so an entry appended between two pages cannot
    /// make the walk skip or repeat another one. `None` is the answer when the
    /// token names no entry of this resource's slice of the log — there is no
    /// page to report, which the caller turns into a refusal.
    ///
    /// The walk is bounded by [`MAX_OPERATION_HISTORY`].
    ///
    /// The page is located over the STORED entries and the modelled view is
    /// built only for the entries the page actually carries, so a resource with
    /// a long slice of the log is not viewed in full to answer ten entries.
    pub fn operations_for_resource_page(
        &self,
        resource_name: &str,
        page_token: Option<&str>,
    ) -> Option<(Vec<Operation>, Option<String>)> {
        let reported: Vec<&StoredOperation> = self
            .operations
            .iter()
            .filter(|entry| entry.resource_name == resource_name)
            .collect();
        let (page, next_page_token) =
            catalog_page(
                &reported,
                OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
                page_token,
                |entry| {
                    // An entry's token is its own `id`, a member it already holds,
                    // so the walk LENDS it rather than cloning it.
                    Some(std::borrow::Cow::Borrowed(entry.id.as_str()))
                },
            )?;
        Some((
            page.into_iter().map(StoredOperation::view).collect(),
            next_page_token,
        ))
    }

    /// One page of the WHOLE log — every entry recorded against every resource
    /// of this account — oldest first, together with the id of the first entry
    /// of the page after it, present only while entries remain.
    ///
    /// This is [`Self::operations_for_resource_page`] without the name filter,
    /// and it pages exactly the same way (the one [`catalog_page`] walk, a token
    /// that NAMES the entry a page starts at, `None` when the token names no
    /// entry of the log) at the size `GetOperations` publishes,
    /// [`OPERATIONS_PAGE_SIZE`].
    ///
    /// The log is append-only, so insertion order IS oldest first and an entry
    /// outlives the resource it was recorded against. The walk is bounded by
    /// [`MAX_OPERATION_HISTORY`], and the modelled view is built only for the
    /// entries the page carries.
    pub fn operations_page(
        &self,
        page_token: Option<&str>,
    ) -> Option<(Vec<Operation>, Option<String>)> {
        let reported: Vec<&StoredOperation> = self.operations.iter().collect();
        let (page, next_page_token) =
            catalog_page(&reported, OPERATIONS_PAGE_SIZE, page_token, |entry| {
                // An entry's token is its own `id`, a member it already holds,
                // so the walk LENDS it rather than cloning it.
                Some(std::borrow::Cow::Borrowed(entry.id.as_str()))
            })?;
        Some((
            page.into_iter().map(StoredOperation::view).collect(),
            next_page_token,
        ))
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

    /// The live set of registry log in credentials for `region` that `username`
    /// and `password` log in with, if the store holds one.
    ///
    /// Every set the clock has already expired is gone before this runs (see
    /// [`Self::advance_clock`]), so a match here is a set that is still valid.
    pub fn registry_login(
        &self,
        region: &str,
        username: &str,
        password: &str,
    ) -> Option<&StoredRegistryLogin> {
        self.registry_logins
            .get(region)?
            .iter()
            .find(|login| login.username == username && login.accepts(password))
    }

    /// Record a set of registry log in credentials just created in `region`,
    /// dropping the oldest once that region holds
    /// [`MAX_REGISTRY_LOGINS_PER_REGION`].
    pub fn insert_registry_login(&mut self, region: &str, login: StoredRegistryLogin) {
        let logins = self.registry_logins.entry(region.to_string()).or_default();
        logins.push(login);
        if let Some(excess) = logins.len().checked_sub(MAX_REGISTRY_LOGINS_PER_REGION) {
            // A set the store had to forget early stops logging in before the
            // expiry its caller was handed, so the eviction is traceable rather
            // than a silent surprise. The username is the id this store holds;
            // no password or digest is logged.
            for evicted in logins.drain(..excess) {
                ::tracing::warn!(
                    region = %region,
                    username = %evicted.username,
                    created_at_epoch_millis = evicted.created_at_ms,
                    limit = MAX_REGISTRY_LOGINS_PER_REGION,
                    "the oldest container image registry log in credentials were dropped to \
                     keep this region within the store's bound"
                );
            }
        }
    }

    /// The container image registry of the account in `region`, for a reporting
    /// read. `None` where nothing has ever been pushed to it: a region with no
    /// registry answers every read about its content the same way an empty one
    /// does, so no read creates one.
    pub fn image_registry(&self, region: &str) -> Option<&StoredImageRegistry> {
        self.container_image_registries.get(region)
    }

    /// The container image registry of the account in `region`, for a push,
    /// which is what creates that region's registry the first time something is
    /// pushed to it.
    pub fn image_registry_mut(&mut self, region: &str) -> &mut StoredImageRegistry {
        self.container_image_registries
            .entry(region.to_string())
            .or_default()
    }

    /// Whether the registry in `region` holds the image `digest` names — the
    /// question `RegisterContainerImage` asks of the digest a caller presents.
    ///
    /// The digest of an IMAGE is the digest of its manifest, which is what a
    /// push reports and what the caller registers; a blob of the image (a layer
    /// or its configuration) is content of the image rather than the image, so
    /// it is not one.
    pub fn image_registry_holds_image(&self, region: &str, digest: &str) -> bool {
        self.image_registry(region)
            .is_some_and(|registry| registry.has_manifest(digest))
    }

    /// Start one blob upload against `repository` in `region`'s registry, and
    /// answer the session id the registry minted for it.
    ///
    /// The id comes from this store's own minting (see [`Self::mint_uuid`]), so
    /// nothing a client sends names a session.
    pub fn start_blob_upload(&mut self, region: &str, repository: &str, now_ms: i64) -> String {
        let session = self.mint_uuid();
        self.image_registry_mut(region)
            .start_upload(session.clone(), repository, now_ms);
        session
    }

    /// Build and record an operation of `operation_type` against
    /// `resource_name` — the ONE place an operation-log entry is built, so every
    /// entry this service writes carries the same members and names the call
    /// that wrote it whether or not the modelled `OperationType` enum declares a
    /// member for that call (see [`OPERATION_TYPE_COPY_SNAPSHOT`]).
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
    /// a certificate reaches the validation status it was owed,
    /// a Region's Lightsail VPC peering walks into the phase it was owed
    /// (established, or gone), a CDN distribution settles into the status it was
    /// owed, the `AutoSnapshot` add-on of a resource takes the automatic
    /// snapshot its schedule owed (and one it has finished taking reports
    /// `Success`), an operation whose work is done settles into `Succeeded`, and a set
    /// of container image registry log in credentials past its 12-hour validity
    /// is forgotten.
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
    /// [`MAX_CERTIFICATES`], [`MAX_DISTRIBUTIONS`] and
    /// [`MAX_OPERATION_HISTORY`] — and, for the peering walk, by the modelled
    /// `RegionName` enum [`vpc_peerings`](Self::vpc_peerings) is keyed by — and a
    /// record drops out of the work as soon as it has nothing outstanding. The
    /// automatic snapshot walk is bounded by
    /// [`auto_snapshots`](Self::auto_snapshots)' own bounds.
    fn advance_clock(&mut self, now_ms: i64) {
        for instance in self.instances.values_mut() {
            if let Some(due) = instance.due {
                if due.at_ms <= now_ms {
                    instance.state = due.state;
                    instance.due = None;
                }
            }
            // A metadata-options selection the service was still putting in
            // place is in place once its instant has come: the options were
            // applied then whether or not anybody asked.
            if let Some(applied_at_ms) = instance.metadata_options.applied_at_ms {
                if applied_at_ms <= now_ms {
                    instance.metadata_options.applied_at_ms = None;
                }
            }
            // An Amazon DCV session runs ON the instance, so it does not survive
            // the instance leaving `running`: the session was gone the instant
            // the computer stopped serving, whether or not anybody asked, so the
            // access is released here rather than by whoever happens to look. A
            // session whose hour has run out WHILE the computer kept running is
            // left where it is: `CreateGUISessionAccessDetails` reports it as
            // `startExpired`, which is what tells a caller to resume it.
            //
            // A session on a computer that is still COMING UP is left where it
            // is: that computer has not stopped serving, it has not started
            // yet, and the session's own start already waits for it
            // (`StoredGuiSession::starting_once_ready`).
            if (instance.state == STOPPING || instance.state == STOPPED)
                && instance.gui_session.is_some()
            {
                instance.gui_session = None;
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
        for record in self.cloud_formation_stack_records.values_mut() {
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
                let was_creating = database.state == RELATIONAL_DATABASE_STATE_CREATING;
                let was_rebooting = database.state == RELATIONAL_DATABASE_STATE_REBOOTING;
                database.state = due.state;
                database.due = None;
                // Settling into a new state is a change to the database, so the
                // database's event log carries one entry for it, timestamped at
                // the instant it fell due rather than at the instant someone
                // happened to read it.
                let settled = match database.state {
                    RELATIONAL_DATABASE_STATE_AVAILABLE if was_creating => {
                        Some(RELATIONAL_DATABASE_EVENT_CREATED)
                    }
                    RELATIONAL_DATABASE_STATE_AVAILABLE if was_rebooting => {
                        Some(RELATIONAL_DATABASE_EVENT_RESTARTED)
                    }
                    RELATIONAL_DATABASE_STATE_AVAILABLE => {
                        Some(RELATIONAL_DATABASE_EVENT_STARTED)
                    }
                    RELATIONAL_DATABASE_STATE_STOPPED => {
                        Some(RELATIONAL_DATABASE_EVENT_STOPPED)
                    }
                    // A database the clock has only moved towards its engine
                    // coming back up (`starting`) has nothing to report yet: it
                    // is not serving, and the entry for it is logged when it is.
                    _ => None,
                };
                if let Some(message) = settled {
                    let category = if was_creating {
                        RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION
                    } else {
                        RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY
                    };
                    database.record_event(due.at_ms, message, &[category]);
                }
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
                // The copy can be restored from now, which completes the backup
                // of the database it was taken of: the second of the two entries
                // that backup leaves in THAT database's event log (the first is
                // logged where the snapshot is taken,
                // [`Self::snapshot_relational_database`]). A snapshot whose
                // source has since been deleted has no log to append to.
                if let Some(database) = self
                    .relational_databases
                    .get_mut(&snapshot.from_relational_database_name)
                {
                    database.record_event(
                        due.at_ms,
                        format!(
                            "{RELATIONAL_DATABASE_EVENT_BACKUP_FINISHED}: {}",
                            snapshot.name
                        ),
                        &[RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP],
                    );
                }
            }
        }
        for balancer in self.load_balancers.values_mut() {
            // A registration whose deregistration has fallen due is gone: the
            // instance was going to come off the load balancer at that instant
            // whether or not anybody asked again, so the clock is what removes
            // it rather than whoever happens to read the load balancer next.
            balancer.attached_instances.retain(|registration| {
                registration.drains_at_ms.is_none_or(|at_ms| at_ms > now_ms)
            });
            // A certificate whose validation has fallen due is issued: the
            // certificate authority issued it at that instant whether or not
            // anybody asked, so the clock is what moves its status rather than
            // whoever happens to read the certificate next.
            for certificate in &mut balancer.certificates {
                let Some(due) = certificate.due.filter(|due| due.at_ms <= now_ms) else {
                    continue;
                };
                certificate.status = due.state;
                certificate.due = None;
            }
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
            if let Some(due) = distribution.due.filter(|due| due.at_ms <= now_ms) {
                distribution.status = due.state;
                distribution.due = None;
            }
            // A reset of the distribution's cached content settles on its OWN
            // clock: deleting cached content is not a configuration change, so
            // it neither waits for the distribution's deployment nor delays it.
            let Some(reset) = distribution.latest_cache_reset.as_mut() else {
                continue;
            };
            if let Some(due) = reset.due.filter(|due| due.at_ms <= now_ms) {
                reset.status = due.state;
                reset.due = None;
            }
        }
        for certificate in self.certificates.values_mut() {
            let Some(due) = certificate.due else { continue };
            if due.at_ms <= now_ms {
                certificate.status = due.state;
                certificate.due = None;
            }
        }
        // The `AutoSnapshot` add-on takes a copy of the resource it is enabled
        // on once a day: that copy was owed at the instant the schedule named
        // whether or not anybody called, so the clock is what takes it — and
        // what settles one the service has finished taking into `Success`.
        //
        // The copies due are collected first because taking one READS the rest
        // of the store (the disks attached to an instance), which cannot be done
        // while the schedule it is written to is borrowed.
        let owed: Vec<(&'static str, String)> = self
            .auto_snapshots
            .iter()
            .filter(|(_, schedule)| schedule.due_at_ms.is_some_and(|at_ms| at_ms <= now_ms))
            .map(|(key, _)| key.clone())
            .collect();
        for (resource_type, name) in owed {
            // A resource whose add-on has been disabled, or that this account no
            // longer holds, is owed no copy — and the hour the add-on carries is
            // what says when the next one falls due.
            let Some(hour) = self.auto_snapshot_hour(resource_type, &name) else {
                continue;
            };
            // What the copy captures: for a virtual private server, the block
            // storage disks attached to it at this instant; a disk's own copy is
            // of one volume, which has no disks attached to it.
            let captured = (resource_type == RESOURCE_TYPE_INSTANCE)
                .then(|| self.attached_disk_captures(&name));
            let Some(schedule) = self.auto_snapshots.get_mut(&(resource_type, name)) else {
                continue;
            };
            let Some(due_at_ms) = schedule.due_at_ms else {
                continue;
            };
            schedule.take(due_at_ms, captured);
            schedule.due_at_ms = Some(next_auto_snapshot_due_ms(due_at_ms, &hour));
        }
        for schedule in self.auto_snapshots.values_mut() {
            for taken in &mut schedule.taken {
                let Some(due) = taken.due.filter(|due| due.at_ms <= now_ms) else {
                    continue;
                };
                taken.status = due.state;
                taken.due = None;
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
        // A set of container image registry log in credentials expires 12 hours
        // after it was created whether or not anybody calls again
        // ([`REGISTRY_LOGIN_LIFETIME_MS`]), so a set the clock has taken past
        // its expiry is forgotten rather than left loggable-in with.
        for logins in self.registry_logins.values_mut() {
            logins.retain(|login| login.expires_at_ms > now_ms);
        }
        self.registry_logins.retain(|_, logins| !logins.is_empty());
    }

    /// The name of every resource this account holds that a request made in
    /// `region` addresses — what `GetActiveNames` reports: "the names of all
    /// active (not deleted) resources" as one flat list, with no resource type
    /// qualifying them.
    ///
    /// A SET, so the names come back in the store's own total order and a name
    /// two kinds of record somehow both hold is reported once: the answer is a
    /// set of NAMES, not one entry per record.
    ///
    /// Which records are the account's resources is the same question
    /// [`Self::with_tagged_record`] answers for one name, so this walks the same
    /// maps the same way: the ones keyed by name alone hold the resources every
    /// operation of theirs serves by name whichever Region the request is made
    /// from, and the ones keyed by the Region together with the name hold the
    /// resources whose identity includes a Region, of which only `region`'s own
    /// are addressable there. Beside them are the kinds whose modelled shape
    /// carries no `tags` member, so no tag operation resolves them — the static
    /// IP, the export snapshot record and the CloudFormation stack record. A
    /// resource a delete removed is held by no map, so its name is gone from
    /// here with it, which is what makes these the ACTIVE names.
    ///
    /// Every walk is bounded by the `MAX_` of the map it walks.
    pub fn active_names(&self, region: &str) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        // The maps keyed by name alone: the key IS the name the resource's own
        // reader serves it under.
        names.extend(self.instances.keys().cloned());
        names.extend(self.disks.keys().cloned());
        names.extend(self.disk_snapshots.keys().cloned());
        names.extend(self.instance_snapshots.keys().cloned());
        names.extend(self.export_snapshot_records.keys().cloned());
        names.extend(self.cloud_formation_stack_records.keys().cloned());
        names.extend(self.domains.keys().cloned());
        names.extend(self.relational_databases.keys().cloned());
        names.extend(self.relational_database_snapshots.keys().cloned());
        names.extend(self.static_ips.keys().cloned());
        names.extend(self.alarms.keys().cloned());
        names.extend(self.key_pairs.keys().cloned());
        names.extend(self.distributions.keys().cloned());
        names.extend(self.certificates.keys().cloned());
        // The maps keyed by the Region together with the name.
        names.extend(names_in_region(&self.buckets, region));
        names.extend(names_in_region(&self.container_services, region));
        names.extend(names_in_region(&self.load_balancers, region));
        // A load balancer's SSL/TLS certificate is a resource of its own, named
        // by the request that asked for it and gone once it is deleted — and the
        // certificates live on the load balancer's record, which is held under
        // the Region they therefore belong to as well.
        names.extend(
            self.load_balancers_in(region)
                .into_iter()
                .flat_map(|balancer| balancer.certificates.iter())
                .map(|certificate| certificate.name.clone()),
        );
        // A contact method is held under its Region and its PROTOCOL, so its
        // name is the one on the record — the name this store minted, which
        // `GetContactMethods` reports.
        names.extend(
            self.contact_methods_in(region)
                .into_iter()
                .map(|method| method.name.clone()),
        );
        names
    }

    /// Run `body` against the tag-carrying record `name` names in `region` —
    /// whichever kind of resource that turns out to be — together with the
    /// `ResourceType` that kind reports. `None` when this account holds no
    /// tag-carrying resource of that name there.
    ///
    /// ONE name addresses one resource: `GetActiveNames` answers "the names of
    /// all active (not deleted) resources" of the account as a single flat list,
    /// with no resource type qualifying them, so at most one of the maps below
    /// holds a given name. Each map is searched the way that resource's OWN
    /// operations address it, so a tag change reaches exactly what they reach
    /// and no more: the maps keyed by `(region, name)` hold the resources whose
    /// identity includes a Region — a bucket, a container service, a load
    /// balancer, whose listings report "all of your … in the Amazon Web Services
    /// Region where the request is made" — so one of those is resolved in
    /// `region` alone; the maps keyed by name alone hold the resources every
    /// operation of theirs takes by name alone (`GetInstance`, `GetDisk`,
    /// `GetRelationalDatabase` and the rest serve them whichever Region the
    /// request is made from), so a tag change does the same rather than refusing
    /// a resource their own readers serve. A contact method is the one resource
    /// whose store key is neither: it is held under its Region and its PROTOCOL,
    /// so the map is resolved by scanning that Region's few entries for the name
    /// the store minted. A load balancer's SSL/TLS certificate is held on the
    /// record of the load balancer that holds it, which is a record of its
    /// Region, so it is scanned for there the same way.
    ///
    /// The one place a tag change resolves a resource, so every operation that
    /// edits tags admits exactly the same set of resources — the ones whose
    /// modelled shape carries a `tags` member.
    pub fn with_tagged_record<R>(
        &mut self,
        region: &str,
        name: &str,
        body: impl FnOnce(&mut dyn TaggedRecord, &'static str) -> R,
    ) -> Option<R> {
        if let Some(record) = self.instances.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_INSTANCE));
        }
        if let Some(record) = self.disks.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISK));
        }
        if let Some(record) = self.disk_snapshots.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISK_SNAPSHOT));
        }
        if let Some(record) = self.instance_snapshots.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_INSTANCE_SNAPSHOT));
        }
        if let Some(record) = self.domains.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DOMAIN));
        }
        if let Some(record) = self.relational_databases.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_RELATIONAL_DATABASE));
        }
        if let Some(record) = self.relational_database_snapshots.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_RELATIONAL_DATABASE_SNAPSHOT));
        }
        if let Some(record) = self.alarms.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_ALARM));
        }
        // A key pair is held under its name alone, the name every key pair
        // operation addresses it by, so a tag change resolves it that way too.
        if let Some(record) = self.key_pairs.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_KEY_PAIR));
        }
        if let Some(record) = self.distributions.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISTRIBUTION));
        }
        // A certificate is held under its name alone, the name
        // `GetCertificates` serves it by, so a tag change resolves it that way
        // too — exactly like the distribution it is attached to.
        if let Some(record) = self.certificates.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_CERTIFICATE));
        }
        // Only the Region-keyed maps need the pair, so it is built here rather
        // than above: a name the maps keyed by name alone hold never allocates
        // it.
        let regional = (region.to_string(), name.to_string());
        if let Some(record) = self.buckets.get_mut(&regional) {
            return Some(body(record, RESOURCE_TYPE_BUCKET));
        }
        if let Some(record) = self.container_services.get_mut(&regional) {
            return Some(body(record, RESOURCE_TYPE_CONTAINER_SERVICE));
        }
        if let Some(record) = self.load_balancers.get_mut(&regional) {
            return Some(body(record, RESOURCE_TYPE_LOAD_BALANCER));
        }
        // An SSL/TLS certificate of a load balancer is a tag-carrying resource
        // of its own: the modelled `LoadBalancerTlsCertificate` carries a `tags`
        // member, the call that asks for one stores the tags its request named,
        // and `GetActiveNames` reports the certificate's name among the
        // account's. Its record lives ON the load balancer that holds it, which
        // is held under the Region, so it is resolved in `region` too — by its
        // own name among that Region's load balancers' certificates, which is
        // how every certificate operation addresses one. The walk is bounded by
        // [`MAX_LOAD_BALANCERS_PER_REGION`] records of at most
        // [`MAX_CERTIFICATES_PER_LOAD_BALANCER`] certificates each.
        //
        // A certificate name is scoped to the load balancer that holds it —
        // "the same name may name a certificate of another load balancer, and
        // the two are different records"
        // ([`StoredLoadBalancer::certificate`]) — so where two load balancers
        // of one Region each hold a certificate of some name, that name
        // addresses the certificate of the first of them in the store's own
        // order: the one record the Region's flat set of active names reports
        // that name for.
        if let Some(record) = self
            .load_balancers
            .iter_mut()
            .filter(|((balancer_region, _), _)| balancer_region == region)
            .flat_map(|(_, balancer)| balancer.certificates.iter_mut())
            .find(|certificate| certificate.name == name)
        {
            return Some(body(record, RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE));
        }
        // A contact method is held under its Region and its protocol, so the
        // name it is tagged by — the one this store minted for it, which
        // `GetContactMethods` reports — is found by walking that Region's
        // entries. One Region holds at most one contact method per protocol, so
        // the walk is over as many entries as the modelled `ContactProtocol`
        // enum has members.
        if let Some(record) = self
            .contact_methods
            .iter_mut()
            .filter(|((method_region, _), _)| method_region == region)
            .map(|(_, method)| method)
            .find(|method| method.name == name)
        {
            return Some(body(record, RESOURCE_TYPE_CONTACT_METHOD));
        }
        None
    }

    /// Run `body` against the add-on-carrying record `name` names — whichever
    /// kind of resource that turns out to be — together with the `ResourceType`
    /// that kind reports. `None` when this account holds no add-on-carrying
    /// resource of that name.
    ///
    /// ONE name addresses one resource (the reasoning is spelled out on
    /// [`Self::with_tagged_record`]), and the resources whose modelled shape
    /// carries an `addOns` member are the virtual private server and the block
    /// storage disk, each of which every operation of theirs addresses by name
    /// alone. The one place an add-on change resolves a resource, so every
    /// operation that edits add-ons admits exactly the same set of them.
    ///
    /// The automatic snapshot schedule of the record is reconciled with
    /// whatever add-ons it carries once `body` has run
    /// ([`Self::reconcile_auto_snapshots`]) — so an add-on change starts or
    /// stops the schedule, and a call that refused the request and wrote
    /// nothing leaves the schedule exactly as it was.
    pub fn with_add_on_record<R>(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut dyn AddOnRecord, &'static str) -> R,
    ) -> Option<R> {
        let now_ms = now_ms();
        let (outcome, resource_type, hour) = if let Some(record) = self.instances.get_mut(name) {
            let outcome = body(record, RESOURCE_TYPE_INSTANCE);
            let hour = enabled_auto_snapshot_hour(&record.add_ons);
            (outcome, RESOURCE_TYPE_INSTANCE, hour)
        } else {
            // The other resource type add-ons apply to; a name that is neither
            // names no resource at all.
            let record = self.disks.get_mut(name)?;
            let outcome = body(record, RESOURCE_TYPE_DISK);
            let hour = enabled_auto_snapshot_hour(&record.add_ons);
            (outcome, RESOURCE_TYPE_DISK, hour)
        };
        self.reconcile_auto_snapshots(now_ms, resource_type, name, hour.as_deref());
        Some(outcome)
    }
}

/// The names of the records of a `(region, name)`-keyed map that belong to
/// `region`.
///
/// What a resource whose identity includes a Region is named by there: the
/// second half of its store key. Used by
/// [`ComAmazonawsLightsailLightsail20161128State::active_names`], which walks
/// three such maps and so asks the question three times.
fn names_in_region<'a, R>(
    records: &'a BTreeMap<(String, String), R>,
    region: &'a str,
) -> impl Iterator<Item = String> + 'a {
    records
        .keys()
        .filter(move |(record_region, _)| record_region == region)
        .map(|(_, name)| name.clone())
}

// ---------------------------------------------------------------------------
// Resource add-ons
//
// An add-on is held on the stored record of the resource it is enabled for,
// beside the rest of what that resource is. The trait below is what lets one
// add-on operation reach them all without knowing which kind of resource it was
// handed: see
// [`ComAmazonawsLightsailLightsail20161128State::with_add_on_record`].
// ---------------------------------------------------------------------------

/// What a stored record must expose for an add-on operation to work on it: where
/// it is, and the add-ons it has enabled.
pub trait AddOnRecord {
    /// Where the resource is, for the operation-log entry an add-on change
    /// writes.
    fn location(&self) -> &StoredLocation;
    /// The add-ons the resource has enabled now.
    fn add_ons(&self) -> &[AddOn];
    /// Replace the add-ons the resource has enabled.
    fn set_add_ons(&mut self, add_ons: Vec<AddOn>);
}

/// Implement [`AddOnRecord`] for each stored record that holds `location` and
/// `add_ons` — which every add-on-carrying Lightsail resource's record does, so
/// the implementation is the same for all of them.
macro_rules! add_on_records {
    ($($record:ident),+ $(,)?) => {
        $(impl AddOnRecord for $record {
            fn location(&self) -> &StoredLocation {
                &self.location
            }

            fn add_ons(&self) -> &[AddOn] {
                &self.add_ons
            }

            fn set_add_ons(&mut self, add_ons: Vec<AddOn>) {
                self.add_ons = add_ons;
            }
        })+
    };
}

add_on_records!(StoredInstance, StoredDisk);

/// What an add-on operation needs from the CALLER's request — the member path a
/// refusal names, and the sentence an unresolvable name is refused with — in a
/// module of its own. Re-exported here, so every caller reaches it as
/// `crate::state::…` exactly like the rest of this module.
pub mod add_ons;
pub use add_ons::{
    add_on_resource_not_found, AddOnMember, ADD_ONS_LIST_MEMBER, ADD_ON_REQUEST_MEMBER,
};

/// The add-ons of `record` with the one of type `add_on_type` removed.
///
/// A stored add-on is named by its type (the modelled `AddOnType` value the
/// request that enabled it carried), and a resource holds at most one add-on of
/// a type, so this removes the one the request named and leaves every other
/// add-on alone. A type the resource has not enabled removes nothing — the
/// result depends only on which type the request named.
pub fn add_ons_without(record: &dyn AddOnRecord, add_on_type: &str) -> Vec<AddOn> {
    record
        .add_ons()
        .iter()
        .filter(|add_on| add_on.name.as_deref() != Some(add_on_type))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Resource tags
//
// Every Lightsail resource whose modelled shape carries a `tags` member holds
// those tags on its own stored record, beside the rest of what the resource is.
// The trait below is what lets one tag operation reach them all without knowing
// which kind of resource it was handed: see
// [`ComAmazonawsLightsailLightsail20161128State::with_tagged_record`].
// ---------------------------------------------------------------------------

/// What a stored record must expose for a tag operation to work on it: where it
/// is, how else it can be named, and the tags it carries.
pub trait TaggedRecord {
    /// The resource's ARN, which a tag operation may name it by as well as by
    /// its name.
    fn arn(&self) -> &str;
    /// Where the resource is, for the operation-log entry a tag change writes.
    fn location(&self) -> &StoredLocation;
    /// The tags the resource carries now.
    fn tags(&self) -> &[Tag];
    /// Replace the tags the resource carries.
    fn set_tags(&mut self, tags: Vec<Tag>);
}

/// Implement [`TaggedRecord`] for each stored record that holds `arn`,
/// `location` and `tags` — which every tag-carrying Lightsail resource's record
/// does, so the implementation is the same for all of them.
macro_rules! tagged_records {
    ($($record:ident),+ $(,)?) => {
        $(impl TaggedRecord for $record {
            fn arn(&self) -> &str {
                &self.arn
            }

            fn location(&self) -> &StoredLocation {
                &self.location
            }

            fn tags(&self) -> &[Tag] {
                &self.tags
            }

            fn set_tags(&mut self, tags: Vec<Tag>) {
                self.tags = tags;
            }
        })+
    };
}

tagged_records!(
    StoredInstance,
    StoredDisk,
    StoredDiskSnapshot,
    StoredInstanceSnapshot,
    StoredDomain,
    StoredRelationalDatabase,
    StoredRelationalDatabaseSnapshot,
    StoredAlarm,
    StoredKeyPair,
    StoredContactMethod,
    StoredDistribution,
    StoredCertificate,
    StoredBucket,
    StoredContainerService,
    StoredLoadBalancer,
    StoredLoadBalancerCertificate,
);

/// The tags of `record` with every tag whose key is in `keys` removed.
///
/// Removal is BY KEY, whatever value the tag carries, and a key the resource
/// does not carry removes nothing — so the result depends only on which keys
/// the request named, never on how many times it named one or on whether the
/// resource ever carried it.
pub fn tags_without(record: &dyn TaggedRecord, keys: &BTreeSet<String>) -> Vec<Tag> {
    record
        .tags()
        .iter()
        .filter(|tag| tag.key.as_deref().is_none_or(|key| !keys.contains(key)))
        .cloned()
        .collect()
}

/// The tags of `record` with `added` merged into them.
///
/// Merging is BY KEY, the other half of [`tags_without`]'s rule: "tag keys must
/// be unique per resource" (the modelled `TagResource` documentation), so a key
/// the resource already carries keeps its place in the set and takes the new
/// value, and a key it does not carry is appended in the order the request named
/// it. A key the request names twice is therefore held once, carrying the last
/// value that request gave it.
///
/// Answers the set rather than applying it, so a caller can refuse a request that
/// would take the resource past [`MAX_TAGS`] before anything is written.
pub fn tags_merged_with(record: &dyn TaggedRecord, added: &[Tag]) -> Vec<Tag> {
    tags_merged_into(record.tags().to_vec(), added)
}

/// The tags a resource CREATED with `requested` carries, or a refusal naming the
/// bound that list broke.
///
/// A create starts the resource from no tags at all, so the set it ends up
/// carrying is the one [`tags_merged_with`] would answer for an untagged
/// resource: merging is BY KEY, because "tag keys must be unique per resource"
/// (the modelled `TagResource` documentation) is a rule about the RESOURCE and so
/// holds however the resource came to carry its tags. A key the request names
/// twice is therefore held once, carrying the last value that request gave it,
/// leaving a created resource in exactly the states a tag change can also reach —
/// and never one where a later `TagResource` for that key could not reach every
/// entry holding it.
///
/// The LIST is bounded first, at [`MAX_TAGS`] and in the wording every operation
/// of this crate that accepts `tags` answers with, so a caller-sized list is
/// refused before a single key is compared; the bound is deliberately on the
/// list rather than on the collapsed set, exactly as the tag-change path bounds
/// the list it is given.
pub fn tags_created_from(requested: Option<&[Tag]>) -> Result<Vec<Tag>, String> {
    let requested = requested.unwrap_or_default();
    if requested.len() > MAX_TAGS {
        return Err(format!(
            "tags carries {} entries, more than the {MAX_TAGS} this service accepts",
            requested.len()
        ));
    }
    Ok(tags_merged_into(Vec::new(), requested))
}

/// `added` merged into the tags `held`, the one place this crate applies the
/// per-resource key-uniqueness rule — reached from [`tags_merged_with`] for a
/// resource that already exists and from [`tags_created_from`] for one being
/// created, so both answer the same set for the same list.
fn tags_merged_into(mut held: Vec<Tag>, added: &[Tag]) -> Vec<Tag> {
    for tag in added {
        match held.iter_mut().find(|entry| entry.key == tag.key) {
            Some(entry) => entry.value = tag.value.clone(),
            None => held.push(tag.clone()),
        }
    }
    held
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
/// the resource's own name so the answer is the same on every later read, and one
/// that DOES name a time is held to the hourly increment its documentation
/// requires (`is_hourly_time_of_day`) before it is stored.
///
/// `member` is where the caller's request carries them, so every refusal below
/// names a member of the request that was actually sent.
pub fn add_ons_from_requests(
    member: AddOnMember,
    resource_name: &str,
    requests: &[AddOnRequest],
) -> Result<Vec<AddOn>, String> {
    if requests.len() > MAX_ADD_ONS {
        return Err(format!(
            "{} carries {} entries, more than the {MAX_ADD_ONS} this service accepts",
            member.carrier,
            requests.len()
        ));
    }
    requests
        .iter()
        .map(|request| match request.add_on_type.as_str() {
            "AutoSnapshot" => {
                // The daily hour the caller asked for, if any. The modelled
                // pattern admits every wall-clock minute, so the half of the
                // constraint only the documentation states — the time is an
                // hourly increment — is applied here, before the add-on is
                // built: an hour this service would never run a snapshot at is
                // refused rather than stored and reported back as the schedule.
                let named = request
                    .auto_snapshot_add_on_request
                    .as_ref()
                    .and_then(|auto| auto.snapshot_time_of_day.clone());
                if let Some(time_of_day) = &named {
                    if !is_hourly_time_of_day(time_of_day) {
                        return Err(format!(
                            "{}.autoSnapshotAddOnRequest.snapshotTimeOfDay '{time_of_day}' \
                             must be in HH:00 format, and in an hourly increment",
                            member.entry
                        ));
                    }
                }
                Ok(AddOn {
                    name: Some(ADD_ON_TYPE_AUTO_SNAPSHOT.to_string()),
                    status: Some(ADD_ON_STATUS_ENABLED.to_string()),
                    snapshot_time_of_day: Some(
                        named.unwrap_or_else(|| default_snapshot_time_of_day(resource_name)),
                    ),
                    ..Default::default()
                })
            }
            "StopInstanceOnIdle" => Ok(AddOn {
                name: Some("StopInstanceOnIdle".to_string()),
                status: Some(ADD_ON_STATUS_ENABLED.to_string()),
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
                "{}.addOnType must be one of [{}], got '{other}'",
                member.entry,
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

/// The `AddOn$status` value an add-on a resource has enabled reports ("The
/// status of the add-on").
///
/// A disabled add-on is not reported at all — `DisableAddOn` removes it from the
/// resource's `addOns` — so this is the status every stored add-on carries, and
/// what tells the automatic snapshot schedule that the add-on is running.
pub const ADD_ON_STATUS_ENABLED: &str = "Enabled";

/// The daily hour the enabled `AutoSnapshot` add-on among `add_ons` takes a copy
/// at, and `None` when none of them is that add-on.
///
/// An add-on is named for its type and carries the status it is in, so those two
/// members are what say whether the service owes the resource a daily copy at
/// all — and `snapshotTimeOfDay` is when, "in `HH:00` format, and in Coordinated
/// Universal Time (UTC)".
fn enabled_auto_snapshot_hour(add_ons: &[AddOn]) -> Option<String> {
    add_ons
        .iter()
        .find(|add_on| {
            add_on.name.as_deref() == Some(ADD_ON_TYPE_AUTO_SNAPSHOT)
                && add_on.status.as_deref() == Some(ADD_ON_STATUS_ENABLED)
        })
        .and_then(|add_on| add_on.snapshot_time_of_day.clone())
}

/// `add_ons` as a read of the resource carrying them reports them: the enabled
/// `AutoSnapshot` add-on also says when its NEXT daily copy is due
/// (`next_auto_snapshot_time_of_day`, `None` when the store owes the resource no
/// further copy); every other add-on is reported exactly as it is stored.
///
/// The modelled `AddOn$nextSnapshotTimeOfDay` is a fact about the SCHEDULE the
/// resource's add-on runs on — the instant the next copy falls due, which the
/// clock re-arms after each one — and not about the request that enabled the
/// add-on, so it is read from that schedule at every read
/// ([`ComAmazonawsLightsailLightsail20161128State::next_auto_snapshot_time_of_day`])
/// rather than stored a second time on the record, where it could disagree with
/// the copy the clock actually takes next.
fn add_ons_view(add_ons: &[AddOn], next_auto_snapshot_time_of_day: Option<String>) -> Vec<AddOn> {
    add_ons
        .iter()
        .map(|add_on| AddOn {
            next_snapshot_time_of_day: (add_on.name.as_deref()
                == Some(ADD_ON_TYPE_AUTO_SNAPSHOT))
                .then(|| next_auto_snapshot_time_of_day.clone())
                .flatten(),
            ..add_on.clone()
        })
        .collect()
}

/// How many milliseconds one hour of Unix time counts — the unit the automatic
/// snapshot hour is reported in, since its times are hourly increments.
const MILLIS_PER_HOUR: i64 = 60 * 60 * 1_000;

/// How many milliseconds one day of Unix time counts. Unix time has no leap
/// second, so every UTC day in it is exactly this long — which is what lets the
/// daily automatic snapshot hour below be found by arithmetic rather than by a
/// calendar.
const MILLIS_PER_DAY: i64 = 24 * MILLIS_PER_HOUR;

/// The UTC hour the instant `epoch_millis` falls in, in the `HH:00` form the
/// modelled `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` admits and
/// the automatic snapshot members report ("in `HH:00` format, and in Coordinated
/// Universal Time (UTC)").
///
/// Unix time has no leap second, so the hour of a UTC day an instant falls in is
/// arithmetic on that instant rather than a calendar lookup — the same reasoning
/// [`MILLIS_PER_DAY`] rests on. The division floors, so an instant anywhere
/// inside an hour reports that hour, which is what the member's "between the
/// time shown and up to 45 minutes after" describes.
fn utc_hour_of_day(epoch_millis: i64) -> String {
    format!(
        "{:02}:00",
        epoch_millis.div_euclid(MILLIS_PER_HOUR).rem_euclid(24)
    )
}

/// The instant the daily automatic snapshot hour `hour` next comes round after
/// `after_ms`.
///
/// `hour` is the add-on's `snapshotTimeOfDay`, which that member's
/// documentation states is "in `HH:00` format, and in Coordinated Universal Time
/// (UTC)" — so the next copy is due that far into a UTC day, on the first such
/// instant still ahead of `after_ms`. An hour that is not that form (nothing
/// this store writes) is read as the start of the day.
fn next_auto_snapshot_due_ms(after_ms: i64, hour: &str) -> i64 {
    // The one `hh24:mi` parser this module has, so the hour an automatic
    // snapshot is scheduled at and the window members of a relational database
    // are read the same way.
    let into_day_ms = minute_of_day(hour).unwrap_or(0).saturating_mul(60_000);
    let day_start_ms = after_ms.div_euclid(MILLIS_PER_DAY).saturating_mul(MILLIS_PER_DAY);
    let at_ms = day_start_ms.saturating_add(into_day_ms);
    if at_ms > after_ms {
        at_ms
    } else {
        at_ms.saturating_add(MILLIS_PER_DAY)
    }
}

/// The add-ons `requests` enable on a DISK, or a refusal naming the member that
/// is wrong.
///
/// The same conversion as [`add_ons_from_requests`], narrowed to the one add-on
/// a disk can carry ([`ADD_ON_TYPE_AUTO_SNAPSHOT`]): asking a disk for the
/// idle-stop add-on is refused rather than stored as an add-on that can never
/// act. Every operation that creates a disk goes through here, so `CreateDisk`
/// and `CreateDiskFromSnapshot` accept exactly the same add-ons.
pub fn disk_add_ons_from_requests(
    member: AddOnMember,
    disk_name: &str,
    requests: &[AddOnRequest],
) -> Result<Vec<AddOn>, String> {
    if let Some(request) = requests
        .iter()
        .find(|request| request.add_on_type != ADD_ON_TYPE_AUTO_SNAPSHOT)
    {
        return Err(format!(
            "{}.addOnType '{}' cannot be enabled on a disk: a disk accepts only \
             {ADD_ON_TYPE_AUTO_SNAPSHOT}",
            member.entry, request.add_on_type
        ));
    }
    add_ons_from_requests(member, disk_name, requests)
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

/// The source members of the operations that can address an AUTOMATIC snapshot
/// — the one rule those three requests are judged by, and the sentences it
/// refuses a combination with — in a module of its own. Re-exported here, so
/// every caller reaches it as `crate::state::…` exactly like the rest of this
/// module.
pub mod snapshot_source;
pub use snapshot_source::{
    no_such_auto_snapshot, requested_snapshot_source, RequestedSnapshotSource,
    SnapshotSourceMembers, COPY_SNAPSHOT_SOURCE_MEMBERS, DISK_RESTORE_SOURCE_MEMBERS,
    INSTANCE_RESTORE_SOURCE_MEMBERS,
};

/// Whether `value` is a daily automatic-snapshot hour an `AutoSnapshot` add-on
/// can be scheduled at.
///
/// `AutoSnapshotAddOnRequest$snapshotTimeOfDay` and the `AddOn` members that
/// report it target the modelled `TimeOfDay` pattern
/// `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$`, which admits any wall-clock minute of
/// the day; their documentation narrows it to a value that "Must be in `HH:00`
/// format, and in an hourly increment". That narrowing is prose the
/// modelled-constraint walk cannot express, so — exactly as for
/// [`is_auto_snapshot_date`] — it is enforced here: an hourly increment is a
/// minute of the day that is a whole number of hours into it, which is also the
/// only form this store ever writes. The time itself is read by the one
/// `hh24:mi` parser this module has ([`minute_of_day`], which admits only two
/// ASCII digits per component and bounds both), so the answer does not depend on
/// the modelled pattern having been walked first.
fn is_hourly_time_of_day(value: &str) -> bool {
    minute_of_day(value).is_some_and(|minute| minute % 60 == 0)
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

/// The UTC calendar date of `epoch_millis`, in the `YYYY-MM-DD` form an
/// automatic snapshot is dated by: `AutoSnapshotDetails$date` is "the date of
/// the automatic snapshot in `YYYY-MM-DD` format".
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)`
/// renders the instant as RFC 3339 in UTC (`2026-10-24T01:05:05Z`), whose first
/// field is that date, so no leap-year arithmetic is re-derived here.
pub fn utc_date(epoch_millis: i64) -> String {
    timestamp(epoch_millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|rendered| {
            rendered
                .split_once('T')
                .map(|(date, _)| date.to_string())
        })
        .unwrap_or_default()
}

/// The monthly billing cycle `epoch_millis` falls in, as the UTC calendar month
/// it belongs to (`YYYY-MM`): an account is billed per calendar month, so two
/// instants of the same month are the same cycle and the month after it is the
/// next one.
///
/// The calendar conversion is [`utc_date`]'s, so no month arithmetic is
/// re-derived here.
pub fn billing_cycle(epoch_millis: i64) -> String {
    let date = utc_date(epoch_millis);
    match date.rsplit_once('-') {
        Some((cycle, _day)) => cycle.to_string(),
        None => date,
    }
}

/// Whether the one bundle change a monthly billing cycle allows is available at
/// `now_ms` to a resource whose bundle was last changed at `last_changed_at_ms`
/// (`None` on one that has never been moved).
///
/// The allowance belongs to the CYCLE the change was made in: a further request
/// in that same cycle finds it spent, and it is available again once that cycle
/// has passed, with nothing to materialise for the resource in between. A
/// bucket's and a distribution's bundle update document the one rule, so both
/// ask here instead of latching a flag of their own.
pub fn bundle_change_available(last_changed_at_ms: Option<i64>, now_ms: i64) -> bool {
    last_changed_at_ms.is_none_or(|changed| billing_cycle(changed) != billing_cycle(now_ms))
}

/// Order two modelled timestamps. A `DateTime` carries seconds plus a
/// sub-second part, so both are compared — two instants one nanosecond apart
/// are not the same instant.
pub fn timestamp_key(value: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (value.secs(), value.subsec_nanos())
}

/// Nanoseconds in one second, the divisor a `DateTime`'s sub-second part is held
/// against.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// One modelled timestamp as the seconds since the epoch a refusal reports it
/// by: the whole seconds, the sub-second part beside them, and the instant's own
/// sign.
///
/// The whole seconds alone are NOT the instant — every instant inside one second
/// shares them, and the seconds component of a pre-epoch instant is the second
/// BELOW it — so a message built from them alone states two equal numbers for a
/// pair that differs. The fraction is rendered exactly, from the nanoseconds
/// rather than through a float, and trailing zeros are dropped so a whole second
/// still reads as one number.
fn epoch_seconds_display(value: ::aws_smithy_types::DateTime) -> String {
    let (secs, subsec_nanos) = timestamp_key(value);
    if subsec_nanos == 0 {
        return secs.to_string();
    }
    // `secs` is the second at or below the instant and `subsec_nanos` the part
    // above that second, so a NEGATIVE instant's magnitude is one second less
    // its fraction: -0.5 is seconds -1 plus 500000000 nanoseconds.
    let (sign, whole, nanos) = if secs < 0 {
        (
            "-",
            secs.unsigned_abs().saturating_sub(1),
            NANOS_PER_SECOND - u64::from(subsec_nanos),
        )
    } else {
        ("", secs.unsigned_abs(), u64::from(subsec_nanos))
    };
    let fraction = format!("{nanos:09}");
    format!("{sign}{whole}.{}", fraction.trim_end_matches('0'))
}

/// The refusal a time window whose end does not follow its start earns, and
/// `None` while the pair is a window — the one sentence this service answers
/// that pair with wherever it takes one.
///
/// Both bounds are named as the INSTANTS they are, sub-second part and sign
/// included, so a window inverted inside one second — or one lying before the
/// epoch — is reported as what it is rather than as two equal whole seconds.
pub fn inverted_window_refusal(
    start: ::aws_smithy_types::DateTime,
    end: ::aws_smithy_types::DateTime,
) -> Option<String> {
    (timestamp_key(start) >= timestamp_key(end)).then(|| {
        format!(
            "startTime must be earlier than endTime, got startTime {} and endTime {} \
             (seconds since the epoch)",
            epoch_seconds_display(start),
            epoch_seconds_display(end)
        )
    })
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

/// The ARN of the SSH key pair `id` in `region`.
pub fn key_pair_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{KEY_PAIR_ARN_RESOURCE_PREFIX}{id}"),
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

/// The ARN of the contact method `id` in `region`.
pub fn contact_method_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONTACT_METHOD_ARN_RESOURCE_PREFIX}{id}"),
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

/// How a refusal words the condition "the resource is the origin of a
/// distribution", given how many of this account's distributions pull their
/// content from it, and `None` when none does.
///
/// The ONE owner of that wording: every delete that can remove a resource a
/// distribution pulls from reports the same condition, so it cannot drift
/// between the kinds of origin resource — the bucket delete, which offers a
/// force that overrides it, names it beside the other conditions that member
/// documents, and the deletes that offer none name it on its own (see
/// [`ComAmazonawsLightsailLightsail20161128State::in_use_as_distribution_origin`]).
pub fn origin_of_distributions_condition(count: usize) -> Option<String> {
    (count > 0).then(|| {
        format!(
            "it is the origin of {count} distribution{}",
            if count == 1 { "" } else { "s" }
        )
    })
}

/// How a refusal words the condition "the disk's own state does not allow this
/// call", given the operation's verb (`attach`, `detach`, `delete`) and the
/// disk's current `DiskState` value.
///
/// The ONE owner of that wording: every disk call a disk's state blocks reports
/// the condition the same way, keyed on the STATE alone — the only variable part
/// besides the verb — so attach, detach and delete cannot word one condition
/// three different ways.
pub fn disk_blocked_by_state_message(verb: &str, state: &str) -> String {
    format!("You can't {verb} this disk right now. The state of this disk is: {state}.")
}

/// How a refusal words the condition "the disk is still attached to an
/// instance", naming the instance that holds it — the only variable part, read
/// from the disk's own [`StoredDisk::attached_to`].
///
/// The ONE owner of that wording, beside [`disk_blocked_by_state_message`]: this
/// sentence is what names the instance a caller has to detach from, so a call
/// blocked by the ATTACHMENT reports the holder rather than a state word.
pub fn disk_still_attached_message(instance_name: &str) -> String {
    format!(
        "You can't perform this operation because the disk is still attached to a Lightsail \
         instance: {instance_name}."
    )
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

/// The kinds of record the restriction [`domain_region_refusal`] carries covers:
/// the DNS zone, the CDN distribution, and the SSL/TLS certificate a
/// distribution is served with — the records whose own operations all ask that
/// helper, each held under its name alone because it is addressed from the one
/// Region its API is served in (see [`DISTRIBUTION_REGION_NAME`]).
///
/// A load balancer's SSL/TLS certificate is NOT one of them: it is a record of
/// the Region its load balancer lives in, and the load balancer API is served in
/// every Region.
pub const DOMAIN_REGION_ONLY_RESOURCE_TYPES: [&str; 3] = [
    RESOURCE_TYPE_DOMAIN,
    RESOURCE_TYPE_DISTRIBUTION,
    RESOURCE_TYPE_CERTIFICATE,
];

/// The refusal for a call onto a record of `resource_type` this request cannot
/// be served by, and `None` when the kind is not one the restriction covers or
/// the request is made in the Region those APIs are served from.
///
/// For a door that admits every kind of record and so learns WHICH kind a name
/// addresses only once it has resolved it — a tag change, which reaches every
/// record whose modelled shape carries a `tags` member
/// ([`ComAmazonawsLightsailLightsail20161128State::with_tagged_record`]). It
/// asks [`domain_region_refusal`], the one owner of the rule, so such a door
/// admits exactly the Regions the record's own operations do rather than
/// re-deriving the restriction.
pub fn domain_region_refusal_for_resource_type(
    parts: &::http::request::Parts,
    resource_type: &str,
) -> Option<String> {
    if !DOMAIN_REGION_ONLY_RESOURCE_TYPES.contains(&resource_type) {
        return None;
    }
    domain_region_refusal(parts)
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
///
/// Each source is tested on its own, in that order, and the FIRST one carrying a value
/// wins: a `Host` header that names nothing (an empty value) leaves the authority the URI
/// carries still to be read, rather than standing in for it.
pub fn request_endpoint(parts: &::http::request::Parts) -> Option<String> {
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|host| host.to_str().ok()),
        parts.uri.authority().map(::http::uri::Authority::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|host| !host.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A plausible instant to create a set of credentials at, and the credentials
    /// one is created with. The username is shaped like one the credential store
    /// mints; the password stands in for the secret it mints.
    const CREATED_AT_MS: i64 = 1_700_000_000_000;
    const USERNAME: &str = "EMULATORASJ4K7QWRT2X";
    const PASSWORD: &str = "a-minted-registry-password";
    const REGION: &str = "us-east-1";

    /// One store holding one set of registry log in credentials.
    fn store_with_one_login() -> ComAmazonawsLightsailLightsail20161128State {
        let mut store = ComAmazonawsLightsailLightsail20161128State::default();
        store.insert_registry_login(
            REGION,
            StoredRegistryLogin::new(USERNAME.to_string(), PASSWORD, CREATED_AT_MS),
        );
        store
    }

    /// RULE: the CLOCK is what ends a set of container image registry log in
    /// credentials — it is recognised for the documented 12 hours after it was
    /// created and gone from the store from that instant on, whether or not
    /// anybody called in between. The integration suite asserts the `expiresAt`
    /// the caller is told; this asserts the instant itself, which no wall-clock
    /// test can reach.
    #[test]
    fn a_registry_login_is_forgotten_once_the_clock_passes_its_expiry() {
        let expiry = CREATED_AT_MS + REGISTRY_LOGIN_LIFETIME_MS;
        // (case, the instant the clock has reached, whether the set logs in)
        let cases = [
            ("the instant it was created", CREATED_AT_MS, true),
            ("one millisecond before it expires", expiry - 1, true),
            ("the instant it expires", expiry, false),
            ("an hour after it expired", expiry + 60 * 60 * 1_000, false),
        ];
        for (case, at_ms, want) in cases {
            // A fresh store per case: the clock only ever moves forward, so each
            // instant is reached from the creation rather than from the last case.
            let mut store = store_with_one_login();
            store.advance_clock(at_ms);
            let found = store.registry_login(REGION, USERNAME, PASSWORD).is_some();
            assert_eq!(
                found, want,
                "case {case}: expected the set to log in = {want}, got {found}"
            );
            assert_eq!(
                store.registry_logins.contains_key(REGION),
                want,
                "case {case}: expected an expired set to be gone from the store, got \
                 {:?}",
                store.registry_logins.get(REGION).map(Vec::len)
            );
        }
    }

    /// RULE: the store's own per-region bound keeps the credentials it holds
    /// finite: past [`MAX_REGISTRY_LOGINS_PER_REGION`] the OLDEST set is the one
    /// dropped, and every younger set still logs in.
    #[test]
    fn the_oldest_registry_login_is_the_one_the_bound_drops() {
        let mut store = ComAmazonawsLightsailLightsail20161128State::default();
        // One set past the bound, each a millisecond younger than the last so
        // "oldest" is well defined.
        let total = MAX_REGISTRY_LOGINS_PER_REGION + 1;
        let username = |index: usize| format!("{USERNAME}{index}");
        for index in 0..total {
            store.insert_registry_login(
                REGION,
                StoredRegistryLogin::new(
                    username(index),
                    PASSWORD,
                    CREATED_AT_MS + i64::try_from(index).expect("the index fits"),
                ),
            );
        }
        store.advance_clock(CREATED_AT_MS);

        assert_eq!(
            store.registry_logins.get(REGION).map(Vec::len),
            Some(MAX_REGISTRY_LOGINS_PER_REGION),
            "case the region at its bound: expected {MAX_REGISTRY_LOGINS_PER_REGION} sets"
        );
        assert!(
            store
                .registry_login(REGION, &username(0), PASSWORD)
                .is_none(),
            "case the oldest set: expected it to have been dropped"
        );
        for index in 1..total {
            assert!(
                store
                    .registry_login(REGION, &username(index), PASSWORD)
                    .is_some(),
                "case the set created {index} milliseconds in: expected it to still log in"
            );
        }
    }

    /// RULE: only the credentials a set was created with log in to it, and only at
    /// the registry of the region it was created for.
    #[test]
    fn only_the_credentials_a_registry_login_was_created_with_are_accepted() {
        let mut store = store_with_one_login();
        store.advance_clock(CREATED_AT_MS);

        // (case, region, username, password)
        let refused = [
            ("another region's registry", "us-west-2", USERNAME, PASSWORD),
            ("a username no set was created for", REGION, "AWS", PASSWORD),
            ("another password", REGION, USERNAME, "another-password"),
            ("the password of the username", REGION, USERNAME, USERNAME),
            ("no password at all", REGION, USERNAME, ""),
            ("a password one character short", REGION, USERNAME, &PASSWORD[..PASSWORD.len() - 1]),
        ];
        for (case, region, username, password) in refused {
            assert!(
                store.registry_login(region, username, password).is_none(),
                "case {case}: expected no set to be recognised"
            );
        }
        assert!(
            store
                .registry_login(REGION, USERNAME, PASSWORD)
                .is_some_and(|login| login.expires_at_ms
                    == CREATED_AT_MS + REGISTRY_LOGIN_LIFETIME_MS),
            "case the credentials it was created with: expected the set to be recognised, \
             expiring 12 hours after it was created"
        );
    }

    /// RULE: what one region's registry holds is accounted for in BYTES — every
    /// entry charged what it costs this process, content it already holds and a
    /// tag it already has costing it nothing, and a fresh tag of a manifest it
    /// already holds costing what that tag takes — so nothing a client repeats
    /// grows it without bound.
    #[test]
    fn a_registry_accounts_for_every_byte_it_holds() {
        let mut registry = StoredImageRegistry::default();
        let layer = b"a layer of image content".to_vec();
        let manifest = br#"{"schemaVersion":2,"layers":[]}"#.to_vec();
        let media_type = "application/vnd.oci.image.manifest.v1+json";
        let layer_digest = registry_digest(&layer);
        // One entry's cost: its own bytes, the string it is keyed by, and the
        // fixed overhead every entry carries.
        let blob_bytes =
            REGISTRY_ENTRY_OVERHEAD_BYTES + layer_digest.len() + layer.len();

        assert_eq!(
            registry.bytes_held(),
            0,
            "case an empty registry: expected it to hold nothing"
        );
        let stored = registry.put_blob(layer.clone(), CREATED_AT_MS);
        assert_eq!(
            (stored, registry.bytes_held()),
            (RegistryWrite::Stored(layer_digest.clone()), blob_bytes),
            "case one pushed layer: expected its own bytes, its digest key and one entry's \
             overhead to be held under its own digest"
        );
        // The SAME bytes again are the blob that is already there.
        registry.put_blob(layer.clone(), CREATED_AT_MS + 1);
        assert_eq!(
            (registry.blobs.len(), registry.bytes_held()),
            (1, blob_bytes),
            "case the same layer pushed twice: expected one blob and no growth"
        );

        let digest = registry_digest(&manifest);
        let manifest_bytes = REGISTRY_ENTRY_OVERHEAD_BYTES
            + digest.len()
            + manifest.len()
            + "app".len()
            + media_type.len();
        let tag_bytes = |tag: &str| {
            REGISTRY_ENTRY_OVERHEAD_BYTES + "app".len() + tag.len() + digest.len()
        };
        for (case, tag, want_tags, want_bytes) in [
            (
                "the manifest under its first tag",
                "latest",
                1,
                blob_bytes + manifest_bytes + tag_bytes("latest"),
            ),
            (
                "the same manifest under the same tag again",
                "latest",
                1,
                blob_bytes + manifest_bytes + tag_bytes("latest"),
            ),
            (
                "the same manifest under a second tag",
                "v1",
                2,
                blob_bytes + manifest_bytes + tag_bytes("latest") + tag_bytes("v1"),
            ),
        ] {
            let stored = registry.put_manifest(
                "app",
                Some(tag),
                manifest.clone(),
                media_type,
                CREATED_AT_MS,
            );
            assert_eq!(
                stored,
                RegistryWrite::Stored(digest.clone()),
                "case {case}: expected it to be held under the digest of its own bytes"
            );
            assert_eq!(
                (registry.manifests.len(), registry.tags.len(), registry.bytes_held()),
                (1, want_tags, want_bytes),
                "case {case}: expected one manifest, {want_tags} tags and {want_bytes} bytes"
            );
        }
    }

    /// RULE: a registry entry is charged what it costs this process, so the
    /// per-region byte bound bounds the ENTRY COUNT too — a client pushing
    /// distinct one-byte blobs is refused after at most
    /// `MAX_REGISTRY_BYTES_PER_REGION / REGISTRY_ENTRY_OVERHEAD_BYTES` of them,
    /// rather than after the hundred million the content alone would admit.
    #[test]
    fn a_registry_charges_every_entry_its_own_footprint() {
        // Distinct one-byte blobs: the cheapest content a client can push, and
        // each one a digest of its own, so none of them is the blob that is
        // already there.
        let digest_len = registry_digest(&[0]).len();
        for count in [1usize, 2, 8, 64] {
            let mut registry = StoredImageRegistry::default();
            for index in 0..count {
                let content = vec![u8::try_from(index).expect("the table stays under 256")];
                assert_eq!(
                    registry.put_blob(content.clone(), CREATED_AT_MS),
                    RegistryWrite::Stored(registry_digest(&content)),
                    "case {count} one-byte blobs: expected blob {index} to be held"
                );
            }
            let want = count * (REGISTRY_ENTRY_OVERHEAD_BYTES + digest_len + 1);
            assert_eq!(
                (registry.blobs.len(), registry.bytes_held()),
                (count, want),
                "case {count} one-byte blobs: expected each to be charged its digest key and \
                 its own overhead, not just its one byte"
            );
        }

        // The bound that follows: every entry costs at least the per-entry
        // overhead, so one region's registry holds at most this many entries
        // however small the content pushed into them is.
        let ceiling = MAX_REGISTRY_BYTES_PER_REGION / REGISTRY_ENTRY_OVERHEAD_BYTES;
        assert!(
            ceiling <= 1_048_576,
            "expected the per-region bound to admit at most a million entries whatever their \
             content, got {ceiling}"
        );
    }

    /// RULE: the store's own per-region bound keeps the blob uploads in flight
    /// finite: past [`MAX_REGISTRY_UPLOADS_PER_REGION`] the OLDEST session is
    /// the one dropped, and every younger session is still there to send to.
    #[test]
    fn the_oldest_blob_upload_is_the_one_the_bound_drops() {
        let mut registry = StoredImageRegistry::default();
        // One session past the bound, each a millisecond younger than the last
        // so "oldest" is well defined.
        let total = MAX_REGISTRY_UPLOADS_PER_REGION + 1;
        let session = |index: usize| format!("session-{index:04}");
        for index in 0..total {
            registry.start_upload(
                session(index),
                "app",
                CREATED_AT_MS + i64::try_from(index).expect("the index fits"),
            );
        }

        assert_eq!(
            registry.uploads.len(),
            MAX_REGISTRY_UPLOADS_PER_REGION,
            "case the region at its bound: expected {MAX_REGISTRY_UPLOADS_PER_REGION} sessions"
        );
        assert_eq!(
            registry.append_to_upload(&session(0), "app", b"bytes for a dropped session"),
            UploadAppend::Unknown,
            "case the oldest session: expected it to have been dropped"
        );
        for index in 1..total {
            assert_eq!(
                registry.append_to_upload(&session(index), "app", b"ab"),
                UploadAppend::Appended { held: 2 },
                "case the session started {index} milliseconds in: expected it to take bytes"
            );
        }
    }

    /// RULE: a label's version is one past the highest that label's registered
    /// images carry — 1 for the first, 2 for the next, and counted per label —
    /// which is what keeps the name of every image a service holds distinct.
    #[test]
    fn a_registered_image_takes_the_next_version_of_its_label() {
        let mut service = StoredContainerService {
            name: "svc".to_string(),
            arn: String::new(),
            created_at_ms: CREATED_AT_MS,
            location: StoredLocation {
                region_name: REGION.to_string(),
                availability_zone: Some(format!("{REGION}a")),
            },
            tags: Vec::new(),
            power: "nano".to_string(),
            power_id: "nano-1".to_string(),
            scale: 1,
            state: CONTAINER_SERVICE_READY,
            is_disabled: false,
            domain_token: "token".to_string(),
            principal_arn: String::new(),
            public_domain_names: BTreeMap::new(),
            ecr_image_puller_role_active: false,
            deployments: VecDeque::new(),
            next_deployment_version: 1,
            images: Vec::new(),
            owed: VecDeque::new(),
        };

        // (the label registered, the name the service must assign)
        let cases = [
            ("web", ":svc.web.1"),
            ("web", ":svc.web.2"),
            ("api", ":svc.api.1"),
            ("web", ":svc.web.3"),
            ("api", ":svc.api.2"),
        ];
        for (label, want) in cases {
            let registered = service.register_image(label, "sha256:abc", CREATED_AT_MS);
            assert_eq!(
                registered.name(&service.name),
                want,
                "case registering {label:?}: expected the name {want}"
            );
        }
        let held: Vec<String> = service
            .images
            .iter()
            .map(|image| image.name(&service.name))
            .collect();
        assert_eq!(
            held,
            cases.iter().map(|(_, name)| (*name).to_string()).collect::<Vec<_>>(),
            "case the whole registry: expected the images in registration order"
        );
    }

    /// RULE: a deployment that steps down when the clock activates the next one
    /// stays one that PREVIOUSLY RAN, so the containers it declared stay ones
    /// the service is running or previously ran, while a deployment that never
    /// became active — one still waiting on the clock, or one replaced before it
    /// activated — launched nothing and declares none of them. A deployment the
    /// [`MAX_DEPLOYMENTS_PER_SERVICE`] bound dropped is forgotten with its
    /// containers.
    #[test]
    fn a_superseded_deployment_is_kept_as_one_that_previously_ran() {
        let mut service = StoredContainerService {
            name: "svc".to_string(),
            arn: String::new(),
            created_at_ms: CREATED_AT_MS,
            location: StoredLocation {
                region_name: REGION.to_string(),
                availability_zone: Some(format!("{REGION}a")),
            },
            tags: Vec::new(),
            power: "nano".to_string(),
            power_id: "nano-1".to_string(),
            scale: 1,
            state: CONTAINER_SERVICE_READY,
            is_disabled: false,
            domain_token: "token".to_string(),
            principal_arn: String::new(),
            public_domain_names: BTreeMap::new(),
            ecr_image_puller_role_active: false,
            deployments: VecDeque::new(),
            next_deployment_version: 1,
            images: Vec::new(),
            owed: VecDeque::new(),
        };

        // Each deployment declares one container named after it, so which
        // deployment a container came from is unambiguous.
        let container_of = |version: i32| format!("app-{version}");
        let deployment = |version: i32| StoredDeployment {
            version,
            state: DEPLOYMENT_STATE_ACTIVATING,
            activated: false,
            containers: BTreeMap::from([(
                container_of(version),
                Container {
                    image: Some("nginx:latest".to_string()),
                    ..Default::default()
                },
            )]),
            public_endpoint: None,
            created_at_ms: CREATED_AT_MS,
        };

        // Three deployments past the bound, so what the bound drops is covered
        // as well as what it keeps.
        let total = i32::try_from(MAX_DEPLOYMENTS_PER_SERVICE + 3)
            .expect("the case count fits a deployment version");
        for version in 1..=total {
            service.record_deployment(deployment(version));
            service.owed.push_back(OwedPhase {
                at_ms: CREATED_AT_MS,
                state: CONTAINER_SERVICE_RUNNING,
                activates_next_deployment: true,
            });
            service.advance(CREATED_AT_MS);

            let ran = service.deployments_that_ran().count();
            let want = usize::try_from(version)
                .expect("the version fits a count")
                .min(MAX_DEPLOYMENTS_PER_SERVICE);
            assert_eq!(
                ran, want,
                "case {version} deployments activated: expected {want} of them still counted as \
                 having run, got {ran}"
            );
            // Every deployment still held answers for its own container,
            // whether it is the active one or one that ran before.
            for held in service
                .deployments_that_ran()
                .map(|deployment| deployment.version)
                .collect::<Vec<i32>>()
            {
                let container = container_of(held);
                assert!(
                    service.has_run_container(&container),
                    "case {version} deployments activated: expected {container:?} of the \
                     deployment {held} the store still holds to be one that ran, got the \
                     containers {:?}",
                    service.containers_that_ran()
                );
            }
        }

        assert_eq!(
            service
                .current_deployment()
                .map(|active| (active.version, active.state)),
            Some((total, DEPLOYMENT_STATE_ACTIVE)),
            "case the newest deployment: expected it active, got {:?}",
            service.current_deployment()
        );
        let not_inactive: Vec<i32> = service
            .deployments
            .iter()
            .filter(|held| held.version != total && held.state != DEPLOYMENT_STATE_INACTIVE)
            .map(|held| held.version)
            .collect();
        assert!(
            not_inactive.is_empty(),
            "case every superseded deployment: expected each marked \
             {DEPLOYMENT_STATE_INACTIVE}, got {not_inactive:?} that are not"
        );
        // The oldest deployments are the ones the bound forgot, so their
        // containers are no longer ones this service ran.
        let bound = i32::try_from(MAX_DEPLOYMENTS_PER_SERVICE)
            .expect("the bound fits a deployment version");
        for forgotten in 1..=(total - bound) {
            let container = container_of(forgotten);
            assert!(
                !service.has_run_container(&container),
                "case the deployment {forgotten} the bound dropped: expected {container:?} to \
                 be forgotten with it, got the containers {:?}",
                service.containers_that_ran()
            );
        }
        let newest = container_of(total);
        assert!(
            service.containers_that_ran().contains(&newest.as_str()),
            "case the containers that ran: expected {newest:?} among them, got {:?}",
            service.containers_that_ran()
        );

        // A deployment still WAITING on the clock, and one replaced while it was
        // still waiting, both launched nothing — asserted here, where the clock
        // is explicit and no activation can race the check.
        let replaced_while_waiting = total.saturating_add(1);
        let waiting = total.saturating_add(2);
        service.record_deployment(deployment(replaced_while_waiting));
        service.record_deployment(deployment(waiting));
        for (case, version) in [
            ("a deployment still waiting on the clock", waiting),
            (
                "a deployment replaced before it ever activated",
                replaced_while_waiting,
            ),
        ] {
            let container = container_of(version);
            assert!(
                !service.has_run_container(&container),
                "case {case}: expected {container:?} not to be one that ran, got the containers \
                 {:?}",
                service.containers_that_ran()
            );
            assert!(
                service
                    .deployments_that_ran()
                    .all(|ran| ran.version != version),
                "case {case}: expected it outside the deployments that ran, got {:?}",
                service
                    .deployments_that_ran()
                    .map(|ran| ran.version)
                    .collect::<Vec<i32>>()
            );
        }
    }

    /// RULE: the access settings this store names are the modelled
    /// `ResourceBucketAccess` enum's own members, so the generated value list
    /// stays the one source for what the wire accepts and these constants only
    /// say which member each one is.
    #[test]
    fn each_named_bucket_access_setting_is_a_member_of_the_modelled_enum() {
        let generated = crate::ops::types::RESOURCE_BUCKET_ACCESS_VALUES;
        for (case, named) in [
            ("the setting that grants access", RESOURCE_BUCKET_ACCESS_ALLOW),
            ("the setting that revokes it", RESOURCE_BUCKET_ACCESS_DENY),
        ] {
            assert!(
                generated.contains(&named),
                "case {case}: expected {named:?} to be one of the modelled values \
                 {generated:?}"
            );
        }
        assert_ne!(
            RESOURCE_BUCKET_ACCESS_ALLOW, RESOURCE_BUCKET_ACCESS_DENY,
            "case the two settings: expected them to name different members"
        );
        assert_eq!(
            generated.len(),
            2,
            "case the modelled enum: expected the two members these constants cover, got \
             {generated:?}"
        );
    }
}


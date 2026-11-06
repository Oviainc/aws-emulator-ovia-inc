// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshiftserverless-redshiftserverless service.
//!
//! The namespace + workgroup + snapshot store and the usage limits taken out over them,
//! plus the little that every handler of those resources shares: the server-derived
//! fields (id, ARN, endpoint address, creation date), the clock-owed status transition a
//! just-created or just-modified workgroup or snapshot settles through, and the opaque
//! page token the paginated lists hand out.
//!
//! A snapshot holds the namespace's DATA as well as its metadata. That data — the
//! databases and the tables in them — lives in the SQL engine the Redshift Data API runs
//! statements on, so this module also carries the seam the snapshot and restore
//! operations copy it through: [`capture_namespace_data`] and [`restore_namespace_data`].
//!
//! `src/ops/mod.rs` is generated, so a helper more than one operation reaches lives here
//! rather than in a second module under `src/ops/`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{Namespace, Snapshot, Tag, UsageLimit, Workgroup};

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`). Also its SigV4
/// signing name and the host label its endpoints carry.
pub const ARN_NAMESPACE: &str = "redshift-serverless";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential scope
/// names one. `engine::emulator_arn`'s own doc fixes this default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `CreateNamespace`'s `dbName` default — `Namespace$dbName` is "the name of the first
/// database created in the namespace", and Redshift's first database is `dev`.
pub const DEFAULT_DB_NAME: &str = "dev";

/// The admin user a namespace reports when the create names none. Redshift Serverless
/// always has an administrator for the first database (`Namespace$adminUsername` is "the
/// username of the administrator for the first database created in the namespace"), and
/// `admin` is the name it gives that account by default — the name the recorded
/// `CreateNamespace`/`GetNamespace` responses carry for a create that supplied none.
pub const DEFAULT_ADMIN_USERNAME: &str = "admin";

/// The `kmsKeyId` a namespace reports when the create names no customer managed key:
/// Redshift Serverless encrypts a namespace with an Amazon Web Services owned key unless
/// a KMS key is supplied, and reports that choice under this sentinel rather than a key
/// id (the value the recorded responses carry for a create with no `kmsKeyId`).
pub const AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// The `lakehouseRegistrationStatus` of a namespace that is not registered with Amazon
/// Redshift federated permissions — every namespace until `UpdateLakehouseConfiguration`
/// registers it. `Namespace$lakehouseRegistrationStatus` is "the status of the lakehouse
/// registration for the namespace", and the recorded responses report this value for a
/// namespace nobody registered.
pub const LAKEHOUSE_NOT_REGISTERED: &str = "NOT_REGISTERED";

/// `CreateWorkgroup`'s `baseCapacity` default, in Redshift Processing Units (RPUs).
pub const DEFAULT_BASE_CAPACITY: i32 = 128;

/// `CreateWorkgroup`'s `port` default. From the model's `Workgroup$port` doc: "Valid port
/// ranges are 5431-5455 and 8191-8215. The default is 5439."
pub const DEFAULT_PORT: i32 = 5439;

/// The port ranges a workgroup accepts, from the same `Workgroup$port` doc sentence as
/// [`DEFAULT_PORT`]: 5431-5455 and 8191-8215, both inclusive.
pub const PORT_RANGES: [(i32, i32); 2] = [(5431, 5455), (8191, 8215)];

/// The single Redshift Serverless version this emulator serves, reported as a workgroup's
/// `workgroupVersion`. One emulator build is one release, so the value is a property of
/// the build and not of any request.
pub const WORKGROUP_VERSION: &str = "1.0";

/// The page bound a list applies when the caller names none: `maxResults`'
/// `@range(min: 1, max: 100)` on `ListNamespaces` / `ListWorkgroups` / `ListSnapshots` /
/// `ListUsageLimits`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The account's quota for each resource in one Region, from AWS's "Quotas for Amazon
/// Redshift Serverless": 25 namespaces and 25 workgroups per account per Region (both
/// adjustable on request, so this is the DEFAULT quota). It bounds the store as well as
/// the API: a caller cannot accumulate records past it.
pub const MAX_NAMESPACES: usize = 25;
/// See [`MAX_NAMESPACES`].
pub const MAX_WORKGROUPS: usize = 25;

/// How many snapshots this emulator keeps for the account in one Region. Unlike
/// [`MAX_NAMESPACES`] this is NOT an AWS-documented number — it is a bound this emulator
/// chooses so a caller cannot grow the in-memory store without limit, surfaced as
/// `CreateSnapshot`'s own modelled `ServiceQuotaExceededException`. (The usage-limit store
/// needs no such bound: it is already bounded by the namespace and workgroup quotas times
/// the modelled usage types and periods.)
pub const MAX_SNAPSHOTS: usize = 100;

/// How many engine pages of DATA the snapshots of this account may hold between them.
///
/// A snapshot holds a copy of the namespace's databases ([`SnapshotRecord::data`]), so the
/// snapshot store grows with the data a caller wrote and not only with the number of
/// records [`MAX_SNAPSHOTS`] bounds. Also an emulator-chosen bound, in the page unit the
/// Data API's engine counts a database in, and surfaced the same way: `CreateSnapshot` is
/// refused with its modelled `ServiceQuotaExceededException` rather than retaining more.
/// 16,384 pages is 64 MiB at the engine's 4 KiB page.
pub const MAX_SNAPSHOT_DATA_PAGES: i64 = 16_384;

/// The `WorkgroupStatus` values this slice reports
/// (`smithy_query(query="shape", shape="WorkgroupStatus")` -> CREATING, AVAILABLE,
/// MODIFYING, DELETING).
pub const WORKGROUP_CREATING: &str = "CREATING";
/// See [`WORKGROUP_CREATING`].
pub const WORKGROUP_AVAILABLE: &str = "AVAILABLE";
/// See [`WORKGROUP_CREATING`].
pub const WORKGROUP_MODIFYING: &str = "MODIFYING";
/// See [`WORKGROUP_CREATING`].
pub const WORKGROUP_DELETING: &str = "DELETING";

/// The `SnapshotStatus` values this slice reports
/// (`smithy_query(query="shape", shape="SnapshotStatus")` -> AVAILABLE, CREATING,
/// DELETED, CANCELLED, FAILED, COPYING). A snapshot this emulator takes is reported
/// `CREATING` and settles into `AVAILABLE`; the other four belong to operations that are
/// still scaffolded.
pub const SNAPSHOT_CREATING: &str = "CREATING";
/// See [`SNAPSHOT_CREATING`].
pub const SNAPSHOT_AVAILABLE: &str = "AVAILABLE";

/// The retention period of a snapshot kept until it is deleted by hand. Redshift's manual
/// snapshot retention uses `-1` for "retained indefinitely", and
/// `Snapshot$snapshotRetentionPeriod` is "the period of time, in days, of how long the
/// snapshot is retained", which admits no 0-day snapshot — so a create that names no
/// period reports this.
pub const SNAPSHOT_RETENTION_INDEFINITE: i32 = -1;

/// The longest retention a manual snapshot accepts, in days: Redshift's manual-snapshot
/// retention period "must be either -1 or an integer between 1 and 3,653" (3,653 days is
/// ten years plus the leap days). The model carries no `@range` for it, so the handler
/// enforces it.
pub const MAX_SNAPSHOT_RETENTION_DAYS: i32 = 3653;

/// Seconds in a day, the unit `Snapshot$snapshotRemainingDays` counts in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// `CreateUsageLimit`'s `period` default. From the model's `CreateUsageLimitRequest$period`
/// doc: "The time period that the amount applies to. A weekly period begins on Sunday. The
/// default is monthly."
pub const DEFAULT_USAGE_LIMIT_PERIOD: &str = "monthly";

/// `CreateUsageLimit`'s `breachAction` default. From the model's
/// `CreateUsageLimitRequest$breachAction` doc: "The action that Amazon Redshift Serverless
/// takes when the limit is reached. The default is log."
pub const DEFAULT_USAGE_LIMIT_BREACH_ACTION: &str = "log";

/// The usage types a limit can be taken out on, from
/// `smithy_query(query="shape", shape="UsageLimitUsageType")`: compute consumed by the
/// serverless workgroup, and data transferred for cross-Region data sharing.
pub const USAGE_TYPE_SERVERLESS_COMPUTE: &str = "serverless-compute";
/// See [`USAGE_TYPE_SERVERLESS_COMPUTE`].
pub const USAGE_TYPE_CROSS_REGION_DATASHARING: &str = "cross-region-datasharing";

/// The metered dimension `amount` is counted in when the limit caps serverless compute:
/// `CreateUsageLimitRequest$amount` is "in Redshift Processing Units (RPU) consumed per
/// hour" for a time-based limit, which is the compute the workgroup used. The token is the
/// one the recorded usage-limit payloads carry for a `serverless-compute` limit.
pub const LIMIT_TYPE_COMPUTE_USED: &str = "compute-used";

/// The metered dimension `amount` is counted in when the limit caps cross-Region data
/// sharing: `CreateUsageLimitRequest$amount` is then "in terabytes (TB) of data transferred
/// between Regions in cross-account sharing", i.e. data. Redshift words that dimension the
/// same way on a provisioned usage limit, whose `LimitType` must be `data-scanned` whenever
/// the feature being limited is `cross-region-datasharing`.
pub const LIMIT_TYPE_DATA_SCANNED: &str = "data-scanned";

/// The metered dimension a limit over `usage_type` counts its `amount` in.
///
/// A usage-limit payload reports the feature being limited (which restates the request's
/// `usageType`) beside the dimension the `amount` is expressed in, so a client knows what
/// unit the number is in without looking the feature up. `None` for a usage type this
/// service knows no dimension for, which the modelled `UsageLimitUsageType` enum admits
/// none of: the boundary validator refuses any other value before a handler runs.
pub fn usage_limit_limit_type(usage_type: &str) -> Option<&'static str> {
    match usage_type {
        USAGE_TYPE_SERVERLESS_COMPUTE => Some(LIMIT_TYPE_COMPUTE_USED),
        USAGE_TYPE_CROSS_REGION_DATASHARING => Some(LIMIT_TYPE_DATA_SCANNED),
        _ => None,
    }
}

/// The `NamespaceStatus` values this slice reports
/// (`smithy_query(query="shape", shape="NamespaceStatus")` -> AVAILABLE, MODIFYING,
/// DELETING). There is no `CREATING`: a namespace holds no compute, so it is available as
/// soon as it exists.
pub const NAMESPACE_AVAILABLE: &str = "AVAILABLE";
/// See [`NAMESPACE_AVAILABLE`].
pub const NAMESPACE_MODIFYING: &str = "MODIFYING";
/// See [`NAMESPACE_AVAILABLE`].
pub const NAMESPACE_DELETING: &str = "DELETING";

/// How long after the call that starts it a status transition falls due. A span this
/// emulator chooses, not a duration the service documents: it only has to be long enough
/// that a caller acting straight after the call still observes the transient status, and
/// short enough to poll through.
const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// True when `port` lies in one of the ranges `Workgroup$port` documents.
pub fn port_in_documented_range(port: i32) -> bool {
    PORT_RANGES
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&port))
}

/// The HTTP status this service answers a resource-NAME refusal with — a name already
/// taken, or one that resolves to nothing — whatever the modelled error's `@httpError`
/// names. From the recorded `CreateNamespace` / `GetNamespace` / `GetWorkgroup` refusals,
/// which all carry the modelled error shape with this status: the `@httpError` 404 of
/// `ResourceNotFoundException` is the error's documented status, not the one this service
/// sends when the input that resolves to nothing is a resource NAME. See
/// [`namespace_not_found`] and [`workgroup_not_found`].
pub const RESOURCE_NAME_REFUSAL_STATUS: u16 = 400;

/// How this service states that a namespace name resolves to nothing.
///
/// The recorded traffic answers an unresolvable namespace NAME with
/// [`RESOURCE_NAME_REFUSAL_STATUS`] carrying the modelled `ResourceNotFoundException`
/// shape, and words the refusal from the name the request asked for. Every operation that
/// addresses a namespace by name refuses a miss this way, so the sentence lives here
/// once; the status is restated through [`crate::scaffolded::Scaffolded::restated`].
pub fn namespace_not_found(name: &str) -> String {
    format!("Serverless namespace {name} not found.")
}

/// How this service states that a workgroup name resolves to nothing: the same
/// [`RESOURCE_NAME_REFUSAL_STATUS`] carrying the same modelled `ResourceNotFoundException`
/// shape as [`namespace_not_found`], because a workgroup is addressed by NAME exactly as a
/// namespace is. Every operation that addresses a workgroup by name refuses a miss this
/// way, so the sentence lives here once.
pub fn workgroup_not_found(name: &str) -> String {
    format!("no workgroup named {name}")
}

/// How this service states that a namespace name is already taken: HTTP **400** carrying
/// the modelled `ConflictException` shape, worded from the name the request asked for. See
/// [`RESOURCE_NAME_REFUSAL_STATUS`] for the status.
pub fn namespace_already_exists(name: &str) -> String {
    format!("The namespace {name} already exists.")
}

/// How this service states that a namespace carries no compute to take a snapshot through.
///
/// The sentence the recorded `CreateSnapshot` refusal carries is entirely FIXED: it ends at
/// "namespace." and names no resource, because the resource the refusal is about travels in
/// the modelled `ResourceNotFoundException$resourceName` member ("the name of the resource
/// that could not be found") rather than inside the message. So the wording is a constant
/// here, not a template.
pub const NAMESPACE_HAS_NO_WORKGROUP: &str =
    "You can't create a snapshot because there isn't a workgroup created for the namespace.";

/// How this service states that a `resourceArn` naming the resource a usage limit is taken
/// out over resolves to nothing.
///
/// The recorded `CreateUsageLimit` refusal is HTTP **400** carrying the modelled
/// `ValidationException` shape — the ARN is the input that is wrong, not a resource the
/// call addressed — and its sentence has ONE variable slot: the id segment the submitted
/// ARN ends in, which the service read the resource's id off. Everything else is fixed.
pub fn resource_arn_does_not_resolve(arn: &str) -> String {
    format!(
        "The Resource ARN is not valid as the workgroup ID {}.",
        arn_resource_id(arn)
    )
}

/// The id segment of `arn`: what follows the last `/`, which is where every ARN this
/// service mints carries the resource's id (`namespace/<id>`, `workgroup/<id>`). A value
/// with no `/` at all carries its id nowhere else, so it stands in for its own id segment.
fn arn_resource_id(arn: &str) -> &str {
    arn.rsplit_once('/').map_or(arn, |(_, id)| id)
}

/// `arn:aws:redshift-serverless:<region>:<account>:namespace/<namespaceId>`.
pub fn namespace_arn(region: &str, namespace_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("namespace/{namespace_id}"))
}

/// `arn:aws:redshift-serverless:<region>:<account>:workgroup/<workgroupId>`.
pub fn workgroup_arn(region: &str, workgroup_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("workgroup/{workgroup_id}"))
}

/// `arn:aws:redshift-serverless:<region>:<account>:snapshot/<snapshotId>`.
pub fn snapshot_arn(region: &str, snapshot_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("snapshot/{snapshot_id}"))
}

/// `arn:aws:redshift-serverless:<region>:<account>:usagelimit/<usageLimitId>`.
///
/// The resource type is one unhyphenated word — the form the recorded `CreateUsageLimit` /
/// `GetUsageLimit` responses carry, and the only part of the ARN that is not supplied by
/// the environment (partition and service prefix aside): region, account and id come from
/// the request and from state.
pub fn usage_limit_arn(region: &str, usage_limit_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("usagelimit/{usage_limit_id}"))
}

/// How this service states that a `usageLimitId` resolves to no usage limit.
///
/// A usage limit is addressed by the id the service minted for it, so an id that names
/// nothing is a bad INPUT rather than a resource the call addressed: the recorded refusal
/// is the modelled `ValidationException` at its `@httpError` 400, worded with one variable
/// slot — the `usageLimitId` the request carried. Every operation that addresses a usage
/// limit by id refuses a miss this way, so the sentence lives here once.
pub fn usage_limit_id_does_not_exist(usage_limit_id: &str) -> String {
    format!("The usage limit ID is not valid. {usage_limit_id} does not exist")
}

/// How this service states that a usage limit's `amount` is not a positive number, the
/// rule `CreateUsageLimitRequest$amount` / `UpdateUsageLimitRequest$amount` document ("The
/// value must be a positive number") and that `Long` carries no `@range` for, so no
/// boundary validator can refuse it. Both operations refuse it the same way, so the
/// sentence lives here once.
pub fn usage_limit_amount_not_positive(amount: i64) -> String {
    format!("amount {amount} must be a positive number")
}

/// The DNS address of the endpoint a workgroup serves:
/// `<workgroupName>.<account>.<region>.redshift-serverless.amazonaws.com`.
pub fn endpoint_address(workgroup_name: &str, region: &str) -> String {
    format!(
        "{workgroup_name}.{}.{region}.{ARN_NAMESPACE}.amazonaws.com",
        ::engine::emulator_account_id()
    )
}

/// A server-minted resource id (`namespaceId` / `workgroupId`): a v4 UUID, the form AWS
/// mints them in. Random per call, so it is stored and then read back, never recomputed.
pub fn new_resource_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Now, as the `date-time` timestamp `Namespace$creationDate` / `Workgroup$creationDate`
/// are modelled with.
pub fn now_timestamp() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(std::time::SystemTime::now())
}

/// The region this request is signed for, which is the region its resources' ARNs carry.
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A status change the clock owes a resource.
///
/// `WorkgroupStatus` models `CREATING` and `MODIFYING` beside `AVAILABLE`, and
/// `NamespaceStatus` models `MODIFYING`, so a resource is reported in one of them while
/// the change it describes is being applied, and reaches `AVAILABLE` on its own a moment
/// later with nobody calling. The due instant is STORED and applied when a later call
/// observes the record; no handler ever sleeps for it.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The instant from which [`status`](Self::status) is the resource's real status.
    pub due_at: Instant,
    /// The status the resource settles into.
    pub status: &'static str,
}

impl PendingStatus {
    /// The transition into `status` that the call made just now owes, due
    /// [`SETTLE_AFTER`] from now.
    pub fn due_after_settle(status: &'static str) -> Self {
        Self {
            due_at: Instant::now() + SETTLE_AFTER,
            status,
        }
    }

    /// Take this transition when `now` has made it due, leaving it outstanding otherwise.
    fn settled_by(pending: &mut Option<Self>, now: Instant) -> Option<&'static str> {
        pending
            .take_if(|pending| now >= pending.due_at)
            .map(|pending| pending.status)
    }
}

/// A stored namespace.
#[derive(Debug, Clone)]
pub struct NamespaceRecord {
    /// The resource every reader of this namespace returns, held whole so a get, a list
    /// and a delete all answer exactly what the create stored.
    pub namespace: Namespace,
    /// Held because the service holds it, and never returned: `Namespace` models no
    /// password member, so no read can echo it.
    pub admin_user_password: Option<String>,
    /// Whether the caller asked the service to manage the admin password itself.
    pub manage_admin_password: bool,
    /// The tags the create request carried. `Namespace` models no `tags` member, so they
    /// are reached through the tagging operations rather than echoed on a read.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this namespace, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
}

impl NamespaceRecord {
    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.namespace.status = Some(transient.to_string());
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + SETTLE_AFTER,
            status: settled,
        });
    }

    /// Materialise the transition the clock has already made due, if any. See
    /// [`WorkgroupRecord::settle`] for why a reporting operation may make this one write.
    pub fn settle(&mut self, now: Instant) {
        if let Some(pending) = self.pending_status.take_if(|pending| now >= pending.due_at) {
            self.namespace.status = Some(pending.status.to_string());
        }
    }
}

/// A stored workgroup.
#[derive(Debug, Clone)]
pub struct WorkgroupRecord {
    /// The resource every reader of this workgroup returns, held whole so a get, a list,
    /// an update and a delete all answer exactly what the last write stored.
    pub workgroup: Workgroup,
    /// The tags the create request carried; see [`NamespaceRecord::tags`].
    pub tags: Vec<Tag>,
    /// The transition the clock owes this workgroup, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
}

impl WorkgroupRecord {
    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.workgroup.status = Some(transient.to_string());
        self.pending_status = Some(PendingStatus::due_after_settle(settled));
    }

    /// Materialise the transition the clock has already made due, if any.
    ///
    /// This is the one write a reporting operation may make: the change was owed at `now`
    /// whether or not anybody called, so applying it does not change what a later caller
    /// sees — it is what a later caller sees.
    pub fn settle(&mut self, now: Instant) {
        if let Some(status) = PendingStatus::settled_by(&mut self.pending_status, now) {
            self.workgroup.status = Some(status.to_string());
        }
    }
}

/// A stored snapshot.
///
/// A snapshot is taken THROUGH the compute attached to its namespace, so it is minted
/// only while a workgroup on that namespace exists; it then outlives every change to
/// that compute, which is why the record holds the namespace-derived fields it answered
/// with rather than looking the namespace up again on each read.
#[derive(Debug)]
pub struct SnapshotRecord {
    /// The resource every reader of this snapshot returns, bar the retention countdown
    /// [`reported`](Self::reported) recomputes from the clock.
    pub snapshot: Snapshot,
    /// The tags the create request carried; see [`NamespaceRecord::tags`].
    pub tags: Vec<Tag>,
    /// The transition the clock owes this snapshot, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
    /// When the snapshot was taken, the instant `snapshotRemainingDays` counts down from.
    pub created_at: Instant,
    /// The namespace's DATA as it stood when the snapshot was taken: a copy of the
    /// databases reached through the namespace's workgroups, which `RestoreFromSnapshot`
    /// writes back. Empty for a namespace no statement has ever run against. See
    /// [`capture_namespace_data`].
    ///
    /// Behind an `Arc` so a restore can copy FROM it with no state guard held: the
    /// restore clones the handle under the lock, releases the lock, and copies.
    pub data: ::std::sync::Arc<NamespaceData>,
}

impl SnapshotRecord {
    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.snapshot.status = Some(transient.to_string());
        self.pending_status = Some(PendingStatus::due_after_settle(settled));
    }

    /// Materialise the `CREATING` -> `AVAILABLE` transition the clock has already made
    /// due, if any. See [`WorkgroupRecord::settle`].
    pub fn settle(&mut self, now: Instant) {
        if let Some(status) = PendingStatus::settled_by(&mut self.pending_status, now) {
            self.snapshot.status = Some(status.to_string());
        }
    }

    /// The snapshot as a reader reports it: the stored record with the retention
    /// countdown recomputed, because `snapshotRemainingDays` is "the amount of days until
    /// the snapshot is deleted" and that shrinks with the clock, not with any call.
    pub fn reported(&self, now: Instant) -> Snapshot {
        let mut snapshot = self.snapshot.clone();
        snapshot.snapshot_remaining_days = snapshot.snapshot_retention_period.map(|retention| {
            remaining_retention_days(retention, now.saturating_duration_since(self.created_at))
        });
        snapshot
    }
}

/// The days left of a `retention`-day retention period after `elapsed`, floored at 0 —
/// or [`SNAPSHOT_RETENTION_INDEFINITE`] for a snapshot kept indefinitely, which never
/// counts down.
pub fn remaining_retention_days(retention: i32, elapsed: Duration) -> i32 {
    if retention < 0 {
        return SNAPSHOT_RETENTION_INDEFINITE;
    }
    let elapsed_days = i64::try_from(elapsed.as_secs() / SECONDS_PER_DAY).unwrap_or(i64::MAX);
    i32::try_from((i64::from(retention) - elapsed_days).max(0)).unwrap_or(0)
}

/// Per-service state for com-amazonaws-redshiftserverless-redshiftserverless. A handler reaches it through [`service_state`].
///
/// Namespaces, workgroups and snapshots are keyed by NAME, which is their identity in
/// this API (every operation addresses them by `namespaceName` / `workgroupName` /
/// `snapshotName`); a usage limit has no name, so it is keyed by the `usageLimitId` the
/// service mints for it and `GetUsageLimit` reads back. All four are `BTreeMap`s so each
/// paginated list walks them in one deterministic order that a page token can resume.
#[derive(Debug, Default)]
pub struct ComAmazonawsRedshiftserverlessRedshiftserverlessState {
    /// Namespaces by `namespaceName`.
    pub namespaces: BTreeMap<String, NamespaceRecord>,
    /// Workgroups by `workgroupName`.
    pub workgroups: BTreeMap<String, WorkgroupRecord>,
    /// Snapshots by `snapshotName`.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// Usage limits by `usageLimitId`, which is this resource's identity in the API:
    /// `GetUsageLimit`, `UpdateUsageLimit` and `DeleteUsageLimit` all address them by it.
    /// The resource is held whole, so every reader answers what the create stored.
    pub usage_limits: BTreeMap<String, UsageLimit>,
}

impl ComAmazonawsRedshiftserverlessRedshiftserverlessState {
    /// Apply every status transition the clock has already made due, to every resource.
    /// Call it before reading or writing a namespace, a workgroup or a snapshot, so no
    /// caller observes a status the service has already moved on from. See
    /// [`WorkgroupRecord::settle`].
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for record in self.namespaces.values_mut() {
            record.settle(now);
        }
        for record in self.workgroups.values_mut() {
            record.settle(now);
        }
        for record in self.snapshots.values_mut() {
            record.settle(now);
        }
    }

    /// True when `arn` is the ARN of a namespace or a workgroup this store holds — the
    /// Redshift Serverless resources a usage limit can be created for. Compared against
    /// the STORED ARNs rather than re-derived, so it accepts exactly what a create
    /// returned.
    pub fn holds_resource_arn(&self, arn: &str) -> bool {
        self.namespaces
            .values()
            .any(|record| record.namespace.namespace_arn.as_deref() == Some(arn))
            || self
                .workgroups
                .values()
                .any(|record| record.workgroup.workgroup_arn.as_deref() == Some(arn))
    }

    /// The id of the usage limit already covering (`resource_arn`, `usage_type`,
    /// `period`), if the store holds one. That triple is what a usage limit caps, so a
    /// second limit over it would be two answers to one question and is refused.
    pub fn usage_limit_covering(
        &self,
        resource_arn: &str,
        usage_type: &str,
        period: &str,
    ) -> Option<&str> {
        self.usage_limits
            .values()
            .find(|limit| {
                limit.resource_arn.as_deref() == Some(resource_arn)
                    && limit.usage_type.as_deref() == Some(usage_type)
                    && limit.period.as_deref() == Some(period)
            })
            .and_then(|limit| limit.usage_limit_id.as_deref())
    }

    /// The names of the workgroups attached to `namespace_name`, in list order. A
    /// namespace with any of these cannot be deleted, and a namespace with none of them
    /// holds no compute to take a snapshot through.
    pub fn workgroups_in_namespace(&self, namespace_name: &str) -> Vec<&str> {
        self.workgroups
            .values()
            .filter(|record| {
                record.workgroup.namespace_name.as_deref() == Some(namespace_name)
            })
            .filter_map(|record| record.workgroup.workgroup_name.as_deref())
            .collect()
    }
}

/// One page of a listing: the records it carries, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a, V> {
    /// The records of this page, in key order.
    pub items: Vec<&'a V>,
    /// Set only when at least one more record remains after this page.
    pub next_token: Option<String>,
}

/// A page token is the key the NEXT page resumes after, hex-encoded.
///
/// Hex makes the token opaque in the sense the API promises — it is not a resource name a
/// client should read or construct, and it carries no meaning a caller may depend on —
/// but it is NOT authenticated: hex is reversible, and a hand-made token that decodes to
/// UTF-8 is accepted and simply resumes after that key. That is the same answer a token
/// whose record has since been deleted gets, which is what lets a listing resume
/// correctly across inserts and deletes rather than failing on them. Nothing a token can
/// say reaches past this listing's own key order, so an unauthenticated token grants no
/// access: a forged one can only start the same listing at a different place.
fn encode_page_token(key: &str) -> String {
    hex::encode(key)
}

/// The key a token resumes after, or the reason the token cannot be one this service
/// issued (its bytes are not hex, or not UTF-8).
fn decode_page_token(token: &str) -> Result<String, String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| {
            "nextToken is not a pagination token this service issued".to_string()
        })
}

/// The page of `entries` that `next_token` and `max_results` select.
///
/// `max_results` is a page BOUND, not a count: a page holds at most that many records,
/// and `next_token` is returned only while more remain.
pub fn page<'a, V>(
    entries: &'a BTreeMap<String, V>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, V>, String> {
    page_matching(entries, |_| true, next_token, max_results)
}

/// [`page`] over the records `keep` accepts: the page bound counts MATCHING records, so a
/// filtered listing fills its pages the same way an unfiltered one does, and the token it
/// hands out resumes after the same key.
pub fn page_matching<'a, V>(
    entries: &'a BTreeMap<String, V>,
    keep: impl Fn(&V) -> bool,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, V>, String> {
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let bound = usize::try_from(bound).unwrap_or(1);
    let after = match next_token {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };

    let mut remaining = entries
        .iter()
        .filter(|(key, _)| after.as_deref().is_none_or(|from| key.as_str() > from))
        .filter(|(_, value)| keep(value));
    let selected: Vec<(&String, &V)> = remaining.by_ref().take(bound).collect();
    // A token only when a record is left over: the last page of a listing carries none.
    let next_token = remaining
        .next()
        .and_then(|_| selected.last().map(|(key, _)| encode_page_token(key)));

    Ok(Page {
        items: selected.into_iter().map(|(_, value)| value).collect(),
        next_token,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRedshiftserverlessRedshiftserverlessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRedshiftserverlessRedshiftserverlessState>)
}

// ---------------------------------------------------------------------------
// The namespace's data, which lives in the Data API's SQL engine
// ---------------------------------------------------------------------------

/// A copy of a namespace's databases, as [`capture_namespace_data`] took it.
///
/// Opaque here: the databases live in the SQL engine the Redshift Data API runs
/// statements on, so the copy is taken, held and written back through that service's own
/// seam and no SQL type crosses into this crate. See the dependency's note in
/// `Cargo.toml`.
pub type NamespaceData =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::WarehouseCapture;

/// Why a namespace's data could not be copied or written back.
pub type DataPlaneUnavailable =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::WarehouseUnavailable;

/// Why a copy of a namespace's data did not happen.
///
/// The copy runs on the blocking pool under a budget, so it can fail three ways: the
/// copy itself could not be made, the request gave up waiting for it, or the task
/// carrying it did not return.
#[derive(Debug)]
pub enum DataPlaneFailure {
    /// The databases could not be copied.
    Unavailable(DataPlaneUnavailable),
    /// The request waited [`DATA_COPY_BUDGET`] and gave up, so the copy is abandoned
    /// rather than reported as done.
    GaveUp(Duration),
    /// The blocking task carrying the copy did not return a result.
    Lost,
}

impl DataPlaneFailure {
    /// True when the failure is this emulator being at its capacity for open
    /// databases — the one of these an operation reports as a quota error rather
    /// than as a server fault.
    pub fn is_capacity(&self) -> bool {
        matches!(
            self,
            Self::Unavailable(DataPlaneUnavailable::TooManyOpen(_))
        )
    }
}

impl std::fmt::Display for DataPlaneFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(unavailable) => write!(f, "{unavailable}"),
            Self::GaveUp(budget) => write!(
                f,
                "the copy did not finish within {} seconds and was abandoned",
                budget.as_secs()
            ),
            Self::Lost => write!(f, "the copy did not return a result"),
        }
    }
}

/// How long a request waits for a copy of a namespace's data: the Data API's own
/// budget for one copy, which is longer than the deadline the copy holds itself to,
/// so what ends a copy is its own deadline and not this request's patience.
pub const DATA_COPY_BUDGET: Duration =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::COPY_REQUEST_BUDGET;

/// Copy the databases reached through `workgroups` — the compute attached to one
/// namespace — as they stand now.
///
/// This is what makes a snapshot a snapshot of DATA: `CreateSnapshot` "creates a
/// snapshot of all databases in a namespace", and a namespace's databases are the ones
/// the Data API serves through the workgroups attached to it. A namespace nothing has
/// run a statement against captures empty, which is correct: it holds no table to
/// restore.
///
/// The copy is blocking engine work, so it runs on the blocking pool under
/// [`DATA_COPY_BUDGET`] and never on the async worker the request is served on — the
/// rule the Data API's `capture_warehouse` documents and its `ExecuteStatement`
/// follows for the same engine. Call it with NO state guard held, so one caller's copy
/// serialises nothing else.
pub async fn capture_namespace_data<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    workgroups: Vec<String>,
) -> Result<NamespaceData, DataPlaneFailure> {
    let engine = ::std::sync::Arc::clone(engine);
    let joined = ::tokio::time::timeout(
        DATA_COPY_BUDGET,
        ::tokio::task::spawn_blocking(move || {
            let names: Vec<&str> = workgroups.iter().map(String::as_str).collect();
            ::service_com_amazonaws_redshiftdata_redshiftdata::state::capture_warehouse(
                &*engine, &names,
            )
        }),
    )
    .await;
    match joined {
        Ok(Ok(Ok(data))) => Ok(data),
        Ok(Ok(Err(unavailable))) => Err(DataPlaneFailure::Unavailable(unavailable)),
        Ok(Err(_panicked)) => Err(DataPlaneFailure::Lost),
        Err(_elapsed) => Err(DataPlaneFailure::GaveUp(DATA_COPY_BUDGET)),
    }
}

/// Write `data` back over the databases `workgroup` serves, and report how many
/// databases it materialised.
///
/// The workgroup then holds exactly the databases the snapshot holds, with exactly
/// their contents, so a statement submitted against it afterwards reads the restored
/// tables. Bounded and taken off the async worker exactly as
/// [`capture_namespace_data`] is, and for the same reasons.
pub async fn restore_namespace_data<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    data: ::std::sync::Arc<NamespaceData>,
    workgroup: String,
) -> Result<usize, DataPlaneFailure> {
    let engine = ::std::sync::Arc::clone(engine);
    let joined = ::tokio::time::timeout(
        DATA_COPY_BUDGET,
        ::tokio::task::spawn_blocking(move || {
            ::service_com_amazonaws_redshiftdata_redshiftdata::state::restore_warehouse(
                &*engine, &data, &workgroup,
            )
        }),
    )
    .await;
    match joined {
        Ok(Ok(Ok(restored))) => Ok(restored),
        Ok(Ok(Err(unavailable))) => Err(DataPlaneFailure::Unavailable(unavailable)),
        Ok(Err(_panicked)) => Err(DataPlaneFailure::Lost),
        Err(_elapsed) => Err(DataPlaneFailure::GaveUp(DATA_COPY_BUDGET)),
    }
}

/// The engine pages the stored snapshots' data occupies between them, the quantity
/// [`MAX_SNAPSHOT_DATA_PAGES`] bounds.
pub fn retained_snapshot_data_pages(
    state: &ComAmazonawsRedshiftserverlessRedshiftserverlessState,
) -> i64 {
    state
        .snapshots
        .values()
        .map(|record| record.data.pages())
        .sum()
}

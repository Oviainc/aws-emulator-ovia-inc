// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rds-amazonrdsv19 service.
//!
//! The DB instance and DB snapshot records the implemented handlers read and write,
//! plus the projection of a stored record onto the modelled `DBInstance` /
//! `DBSnapshot` wire shapes. These live here rather than in one operation file
//! because every implemented operation shares them: `CreateDBInstance`,
//! `DescribeDBInstances`, `ModifyDBInstance`, `CreateDBSnapshot`,
//! `DescribeDBSnapshots` and `DeleteDBInstance` all project the same record.
//!
//! RDS is a REGIONAL service, so both maps are keyed by `(region, identifier)`: the
//! region a request is signed for scopes what it can see and create, one identifier
//! can name a different instance in each region, and a describe signed for one region
//! never reports another's.
//!
//! Keying by region is also what bounds the store and every scan, which matters
//! because the region comes from the request's own SigV4 credential scope and this
//! emulator authenticates nobody. Three bounds hold together: [`check_region`] admits
//! only a value of the published AWS region-id form (so a region key is at most 32
//! bytes), [`MAX_REGIONS`] caps how many distinct regions hold records at all, and the
//! per-region quotas cap the records in each. The store therefore holds at most
//! `MAX_REGIONS * DB_INSTANCE_QUOTA_PER_REGION` instances and
//! `MAX_REGIONS * DB_SNAPSHOT_QUOTA_PER_REGION` snapshots, whatever sequence of
//! requests arrives, and every per-request scan reads one region's ordered range
//! rather than the whole map.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    DBInstance, DBParameterGroupStatus, DBSnapshot, DBSubnetGroup, DomainMembership, Endpoint, Tag,
    VpcSecurityGroupMembership,
};

/// How long after the call that starts it a status transition this service owes
/// becomes due.
///
/// RDS takes minutes to bring a DB instance up; a generated emulator picks its own
/// span, and this one is deliberately non-zero so a caller that reads straight after
/// `CreateDBInstance` still observes the `creating` status (and the state errors that
/// go with it) exactly as it would against the real service. The due instant is
/// stored on the record and materialised by the next call — nothing sleeps.
pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(300);

/// The ARN namespace and sigv4 signing name of this service, as its Smithy model's
/// `aws.api#service` trait declares them (`arnNamespace: rds`).
const ARN_NAMESPACE: &str = "rds";

/// How many DB instances one region may hold: Amazon RDS's published default quota
/// ("DB instances": 40 per Region). Reaching it is the modelled
/// `InstanceQuotaExceededFault` — "The request would result in the user exceeding the
/// allowed number of DB instances".
pub const DB_INSTANCE_QUOTA_PER_REGION: usize = 40;

/// How many manual DB snapshots one region may hold: Amazon RDS's published default
/// quota ("Manual DB snapshots": 100 per Region). Reaching it is the modelled
/// `SnapshotQuotaExceededFault`, which both `CreateDBSnapshot` and `DeleteDBInstance`
/// declare.
pub const DB_SNAPSHOT_QUOTA_PER_REGION: usize = 100;

/// How many distinct regions this store keeps records for.
///
/// The per-region quotas alone would not bound the store: the region keying them is
/// the request's own SigV4 credential-scope region, which this emulator does not
/// authenticate, so without this cap a caller could open a fresh quota bucket per
/// request. With it the store holds at most `MAX_REGIONS` buckets, and so at most
/// `MAX_REGIONS * DB_INSTANCE_QUOTA_PER_REGION` DB instances and
/// `MAX_REGIONS * DB_SNAPSHOT_QUOTA_PER_REGION` DB snapshots in total. AWS operates
/// on the order of 36 commercial regions, so this bound is this emulator's and is
/// comfortably above the real surface; reaching it answers the same modelled quota
/// fault, which is what AWS answers when a create would exceed an allowance.
pub const MAX_REGIONS: usize = 64;

/// How many filter VALUES one describe request may carry in total, across all its
/// filters.
///
/// RDS documents no bound on `Filters`, and the scan runs while this service's one
/// state lock is held, so the work must be capped on both factors: the record count
/// is capped by the quotas above and the term count by this. A request past it is
/// refused with a 400 rather than served slowly; the bound is this emulator's, and
/// generous next to the five filter names either describe operation recognises.
pub const MAX_FILTER_TERMS: usize = 256;

/// A record's key: the region the request that created it was signed for, and the
/// resource's own identifier.
pub type RegionalKey = (String, String);

/// Per-service state for com-amazonaws-rds-amazonrdsv19. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRdsAmazonrdsv19State {
    /// DB instances by `(region, DBInstanceIdentifier)` — the identifier always stored
    /// lowercased, as the model documents. Ordered, so `DescribeDBInstances` pages
    /// deterministically within a region.
    pub db_instances: BTreeMap<RegionalKey, DbInstanceRecord>,
    /// DB snapshots by `(region, DBSnapshotIdentifier)`. Ordered, for the same reason.
    pub db_snapshots: BTreeMap<RegionalKey, DbSnapshotRecord>,
    /// Monotonic counter behind every server-assigned id this service mints, so
    /// resource ids and endpoint hosts are unique and reproducible.
    pub mint_counter: u64,
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The next value of the server-side id counter.
    pub fn next_mint(&mut self) -> u64 {
        self.mint_counter += 1;
        self.mint_counter
    }

    /// Materialise the status transitions the clock has already made due IN `region`.
    ///
    /// Call it at the top of every handler, reporting ones included: this writes only
    /// changes that were owed at this instant even if nobody had called, so a read
    /// cannot move a status that was not already due. It walks one region's ordered
    /// range — bounded by that region's quota — never the whole store, so the work one
    /// request does under the state lock does not grow with what other callers stored.
    pub fn settle_due_transitions(&mut self, region: &str) {
        let now = Instant::now();
        for (key, instance) in self
            .db_instances
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            instance.settle(now);
        }
        for (key, snapshot) in self
            .db_snapshots
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            snapshot.settle(now);
        }
    }

    /// The DB instances stored in `region`, in identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// [`DB_INSTANCE_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn instances_in(&self, region: &str) -> Vec<&DbInstanceRecord> {
        range_in(&self.db_instances, region)
    }

    /// The DB snapshots stored in `region`, in identifier order.
    pub fn snapshots_in(&self, region: &str) -> Vec<&DbSnapshotRecord> {
        range_in(&self.db_snapshots, region)
    }

    /// Whether a record may be stored for `region`: either it already holds records, or
    /// the store has room for one more region ([`MAX_REGIONS`]).
    ///
    /// The count walks the keys of both maps, which the three bounds above already cap
    /// in total, so the work is bounded by that total rather than by anything a caller
    /// chooses.
    pub fn has_room_for_region(&self, region: &str) -> bool {
        let known: BTreeSet<&str> = self
            .db_instances
            .keys()
            .chain(self.db_snapshots.keys())
            .map(|(stored, _)| stored.as_str())
            .collect();
        known.contains(region) || known.len() < MAX_REGIONS
    }
}

/// The values of `map` whose key names `region`, in key order.
fn range_in<'a, T>(map: &'a BTreeMap<RegionalKey, T>, region: &str) -> Vec<&'a T> {
    map.range((region.to_string(), String::new())..)
        .take_while(|((stored, _), _)| stored == region)
        .map(|(_, record)| record)
        .collect()
}

/// A status this service owes: what the resource becomes, and when that is due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub status: String,
    pub due_at: Instant,
}

impl PendingStatus {
    /// A transition to `status`, due [`STATUS_TRANSITION_DELAY`] from now.
    pub fn in_a_moment(status: &str) -> Self {
        Self {
            status: status.to_string(),
            due_at: Instant::now() + STATUS_TRANSITION_DELAY,
        }
    }
}

/// A stored DB instance: everything the service persists about it.
///
/// The ARN, the endpoint DNS name and the hosted zone id are NOT stored as text:
/// they are functions of the stored region, identifier and minted endpoint host, and
/// [`DbInstanceRecord::to_wire`] computes them. That is what keeps them consistent
/// after `ModifyDBInstance` renames an instance — a stored ARN would still name the
/// old identifier.
#[derive(Debug, Clone)]
pub struct DbInstanceRecord {
    pub identifier: String,
    /// The region the instance was created in; the scope of its ARN and endpoint.
    pub region: String,
    pub dbi_resource_id: String,
    /// The server-assigned middle label of the endpoint DNS name, minted once.
    pub endpoint_host: String,
    pub db_instance_class: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    pub db_name: Option<String>,
    pub allocated_storage: Option<i32>,
    pub max_allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    pub endpoint_port: i32,
    pub multi_az: bool,
    pub storage_type: String,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub availability_zone: String,
    pub backup_retention_period: i32,
    pub preferred_backup_window: Option<String>,
    pub preferred_maintenance_window: Option<String>,
    pub publicly_accessible: bool,
    pub auto_minor_version_upgrade: bool,
    pub copy_tags_to_snapshot: bool,
    pub deletion_protection: bool,
    pub db_subnet_group_name: Option<String>,
    pub db_parameter_group_name: Option<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub db_cluster_identifier: Option<String>,
    pub domain: Option<String>,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl DbInstanceRecord {
    /// Apply a transition whose due instant has passed.
    fn settle(&mut self, now: Instant) {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            self.status = pending.status;
        }
    }

    /// `arn:aws:rds:<region>:<account>:db:<identifier>`, from the stored region and
    /// the instance's current identifier.
    pub fn arn(&self) -> String {
        db_instance_arn(&self.region, &self.identifier)
    }

    /// The DNS name this instance's endpoint resolves to:
    /// `<identifier>.<minted host>.<region>.rds.amazonaws.com`.
    pub fn endpoint_address(&self) -> String {
        format!(
            "{}.{}.{}.rds.amazonaws.com",
            self.identifier, self.endpoint_host, self.region
        )
    }

    /// Project the stored record onto the modelled `DBInstance` output shape.
    pub fn to_wire(&self) -> DBInstance {
        DBInstance {
            db_instance_identifier: Some(self.identifier.clone()),
            db_instance_class: Some(self.db_instance_class.clone()),
            engine: Some(self.engine.clone()),
            db_instance_status: Some(self.status.clone()),
            master_username: self.master_username.clone(),
            db_name: self.db_name.clone(),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address()),
                port: Some(self.endpoint_port),
                hosted_zone_id: hosted_zone_id(&self.region).map(str::to_string),
            }),
            allocated_storage: self.allocated_storage,
            instance_create_time: Some(self.instance_create_time),
            preferred_backup_window: self.preferred_backup_window.clone(),
            backup_retention_period: Some(self.backup_retention_period),
            vpc_security_groups: Some(
                self.vpc_security_group_ids
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some("active".to_string()),
                    })
                    .collect(),
            ),
            db_parameter_groups: self.db_parameter_group_name.clone().map(|name| {
                vec![DBParameterGroupStatus {
                    db_parameter_group_name: Some(name),
                    parameter_apply_status: Some("in-sync".to_string()),
                }]
            }),
            availability_zone: Some(self.availability_zone.clone()),
            db_subnet_group: self.db_subnet_group_name.clone().map(|name| DBSubnetGroup {
                db_subnet_group_name: Some(name),
                subnet_group_status: Some("Complete".to_string()),
                ..DBSubnetGroup::default()
            }),
            preferred_maintenance_window: self.preferred_maintenance_window.clone(),
            multi_az: Some(self.multi_az),
            engine_version: self.engine_version.clone(),
            auto_minor_version_upgrade: Some(self.auto_minor_version_upgrade),
            license_model: self.license_model.clone(),
            iops: self.iops,
            publicly_accessible: Some(self.publicly_accessible),
            storage_type: Some(self.storage_type.clone()),
            db_cluster_identifier: self.db_cluster_identifier.clone(),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            domain_memberships: self.domain.clone().map(|domain| {
                vec![DomainMembership {
                    domain: Some(domain),
                    status: Some("joined".to_string()),
                    ..DomainMembership::default()
                }]
            }),
            copy_tags_to_snapshot: Some(self.copy_tags_to_snapshot),
            db_instance_arn: Some(self.arn()),
            deletion_protection: Some(self.deletion_protection),
            max_allocated_storage: self.max_allocated_storage,
            tag_list: Some(self.tags.clone()),
            ..DBInstance::default()
        }
    }
}

/// A stored DB snapshot: the source instance's settings as they stood when the
/// snapshot was taken, which is why they are copied rather than re-read later.
#[derive(Debug, Clone)]
pub struct DbSnapshotRecord {
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    pub db_instance_identifier: String,
    pub dbi_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    pub availability_zone: String,
    pub port: i32,
    pub storage_type: String,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    pub snapshot_type: String,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl DbSnapshotRecord {
    /// Apply a transition whose due instant has passed.
    fn settle(&mut self, now: Instant) {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            self.status = pending.status;
        }
    }

    /// Snapshot the source instance's settings. `snapshot_type` is `manual` for a
    /// `CreateDBSnapshot` and for the final snapshot `DeleteDBInstance` takes —
    /// `automated` is reserved for the backups RDS itself schedules.
    pub fn from_instance(
        instance: &DbInstanceRecord,
        snapshot_identifier: &str,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: instance.region.clone(),
            db_instance_identifier: instance.identifier.clone(),
            dbi_resource_id: instance.dbi_resource_id.clone(),
            engine: instance.engine.clone(),
            engine_version: instance.engine_version.clone(),
            allocated_storage: instance.allocated_storage,
            master_username: instance.master_username.clone(),
            availability_zone: instance.availability_zone.clone(),
            port: instance.endpoint_port,
            storage_type: instance.storage_type.clone(),
            encrypted: instance.storage_encrypted,
            kms_key_id: instance.kms_key_id.clone(),
            iops: instance.iops,
            license_model: instance.license_model.clone(),
            status: "creating".to_string(),
            pending_status: Some(PendingStatus::in_a_moment("available")),
            snapshot_type: "manual".to_string(),
            instance_create_time: instance.instance_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            tags,
        }
    }

    /// `arn:aws:rds:<region>:<account>:snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Project the stored record onto the modelled `DBSnapshot` output shape.
    pub fn to_wire(&self) -> DBSnapshot {
        DBSnapshot {
            db_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_instance_identifier: Some(self.db_instance_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            allocated_storage: self.allocated_storage,
            status: Some(self.status.clone()),
            port: Some(self.port),
            availability_zone: Some(self.availability_zone.clone()),
            instance_create_time: Some(self.instance_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            iops: self.iops,
            // 0 while the snapshot is still being taken, 100 once it is available:
            // the progress of the copy this record stands for, not a stored field.
            percent_progress: Some(if self.status == "available" { 100 } else { 0 }),
            storage_type: Some(self.storage_type.clone()),
            encrypted: Some(self.encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_snapshot_arn: Some(self.arn()),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            tag_list: Some(self.tags.clone()),
            ..DBSnapshot::default()
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRdsAmazonrdsv19State>)
}

/// The region this request was signed for: every ARN and endpoint host this service
/// mints is scoped to it, and it is half of every stored record's key.
///
/// Falls back to `us-east-1`, the region a generated emulator defaults to, when the
/// request carries no SigV4 credential scope to read it from. A scope that names
/// something that is not an AWS region id is refused rather than stored: the value is
/// request data this emulator does not authenticate, and it keys the store.
pub fn request_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<String, String> {
    let region = emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string());
    check_region(&region)?;
    Ok(region)
}

/// The longest AWS region id this service accepts. The longest one AWS publishes is
/// 14 characters (`ap-southeast-4`); 32 leaves room for any it adds.
const MAX_REGION_LEN: usize = 32;

/// Refuse a credential-scope region that is not of the published AWS region-id form:
/// lowercase letters in two or more hyphen-separated groups, then a one- or
/// two-digit number — `us-east-1`, `eu-central-1`, `ap-southeast-4`, `us-gov-west-1`,
/// `cn-north-1`.
pub fn check_region(region: &str) -> Result<(), String> {
    let malformed = || {
        format!(
            "the credential scope names '{region}', which is not an AWS region id \
             (expected a form like us-east-1, at most {MAX_REGION_LEN} characters)"
        )
    };
    if region.is_empty() || region.len() > MAX_REGION_LEN {
        return Err(malformed());
    }
    let mut parts: Vec<&str> = region.split('-').collect();
    let number = parts.pop().ok_or_else(malformed)?;
    if parts.len() < 2
        || !(1..=2).contains(&number.len())
        || !number.bytes().all(|b| b.is_ascii_digit())
        || !parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase()))
    {
        return Err(malformed());
    }
    Ok(())
}

/// `arn:aws:rds:<region>:<account>:db:<identifier>`.
pub fn db_instance_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("db:{identifier}"))
}

/// The current instant as the modelled timestamp shapes carry it.
pub fn now_as_wire_timestamp() -> ::aws_smithy_types::DateTime {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
}

/// `value` in base 36 (digits, then uppercase letters), left-padded with `0` to
/// `width` characters — the alphabet RDS's server-assigned ids are drawn from.
fn base36(value: u64, width: usize) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut out = Vec::with_capacity(width);
    let mut remaining = value;
    loop {
        out.push(DIGITS[(remaining % 36) as usize]);
        remaining /= 36;
        if remaining == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// The immutable resource id RDS reports as `DbiResourceId`: `db-` plus 26
/// uppercase characters, minted once per instance and never reused.
pub fn mint_dbi_resource_id(mint: u64) -> String {
    format!("db-{}", base36(mint, 26))
}

/// The DNS label RDS assigns an instance's endpoint between the identifier and the
/// region: a server-minted, 12-character token, unique per instance.
pub fn mint_endpoint_host(mint: u64) -> String {
    base36(mint, 12).to_ascii_lowercase()
}

/// The Route 53 hosted zone id RDS reports for the endpoints in one region, when it
/// is a region this emulator can cite one for.
///
/// RDS publishes ONE zone per region, so this is a function of the region alone. The
/// published ids are values AWS ALLOCATED — nothing computes them — so the only honest
/// source is a table, and a region the table does not carry gets no `HostedZoneId` at
/// all rather than a made-up value dressed in the published form: a client that reads
/// this member to build a DNS alias would be misled by an id no zone answers to.
/// Extend the table as published ids are cited.
pub fn hosted_zone_id(region: &str) -> Option<&'static str> {
    /// The published RDS hosted zone ids this emulator carries, as
    /// `(region, hosted zone id)`.
    const PUBLISHED: &[(&str, &str)] = &[("us-east-1", "Z2R2ITUGPM61AM")];

    PUBLISHED
        .iter()
        .find(|(name, _)| *name == region)
        .map(|(_, published)| *published)
}

/// The port a DB instance accepts connections on when the request names none.
///
/// From `CreateDBInstanceMessage$Port`: "Default: RDS for Db2 - 50000, RDS for
/// MariaDB - 3306, RDS for Microsoft SQL Server - 1433, RDS for MySQL - 3306, RDS
/// for Oracle - 1521, RDS for PostgreSQL - 5432". The engine names are the prefixes
/// the `Engine` member's own value list uses (`custom-sqlserver-ee`, `oracle-se2`,
/// `aurora-postgresql`, …), so the family is matched on the prefix.
pub fn default_port(engine: &str) -> i32 {
    let engine = engine.to_ascii_lowercase();
    if engine.contains("db2") {
        50000
    } else if engine.contains("sqlserver") {
        1433
    } else if engine.contains("oracle") {
        1521
    } else if engine.contains("postgres") {
        5432
    } else {
        // mysql, mariadb, aurora-mysql and the Aurora MySQL aliases.
        3306
    }
}

/// The lowest and highest port `CreateDBInstanceMessage$Port` accepts
/// ("Valid Values: 1150-65535").
pub const PORT_RANGE: std::ops::RangeInclusive<i32> = 1150..=65535;

/// The lowest and highest `BackupRetentionPeriod`
/// ("Constraints: Must be a value from 0 to 35").
pub const BACKUP_RETENTION_RANGE: std::ops::RangeInclusive<i32> = 0..=35;

/// The lowest and highest `MaxRecords` a describe operation accepts
/// ("Constraints: Minimum 20, maximum 100").
pub const MAX_RECORDS_RANGE: std::ops::RangeInclusive<i32> = 20..=100;

/// The page size a describe operation uses when the request names none
/// (`MaxRecords` "Default: 100").
pub const DEFAULT_MAX_RECORDS: usize = 100;

/// Whether `engine` names an Aurora engine, which several documented defaults
/// branch on (`aurora-mysql`, `aurora-postgresql`).
pub fn is_aurora(engine: &str) -> bool {
    engine.to_ascii_lowercase().starts_with("aurora")
}

/// The documented default for `PubliclyAccessible` when the request omits it.
///
/// From `CreateDBInstanceMessage$PubliclyAccessible`: "If `DBSubnetGroup` isn't
/// specified, `PubliclyAccessible` defaults to `false` for Aurora instances and
/// `true` for non-Aurora instances. If `DBSubnetGroup` is specified,
/// `PubliclyAccessible` defaults to `false` unless the value of `DBSubnetGroup` is
/// `default`, in which case `PubliclyAccessible` defaults to `true`."
pub fn default_publicly_accessible(engine: &str, db_subnet_group_name: Option<&str>) -> bool {
    match db_subnet_group_name {
        None => !is_aurora(engine),
        Some(name) => name.eq_ignore_ascii_case("default"),
    }
}

/// The documented default for `StorageType` when the request omits it.
///
/// From `CreateDBInstanceMessage$StorageType`: "Default: `io1`, if the `Iops`
/// parameter is specified. Otherwise, `gp3`."
pub fn default_storage_type(iops: Option<i32>) -> &'static str {
    if iops.is_some() {
        "io1"
    } else {
        "gp3"
    }
}

/// Refuse an identifier that breaks the constraints its member documents.
///
/// `DBInstanceIdentifier` and `DBSnapshotIdentifier` share one rule, differing only
/// in the length bound (63 and 255): "Must contain from 1 to N letters, numbers, or
/// hyphens. First character must be a letter. Can't end with a hyphen or contain two
/// consecutive hyphens." Returns the message to refuse the request with, naming the
/// member and the constraint it broke.
pub fn check_identifier(member: &str, value: &str, max_len: usize) -> Result<(), String> {
    // The alphabet comes first: a value outside it is reported as the wrong alphabet
    // rather than measured against the length bound, which for a multi-byte character
    // would name a count the caller never typed.
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "{member} must contain only letters, numbers, or hyphens; got '{value}'"
        ));
    }
    let length = value.chars().count();
    if length == 0 || length > max_len {
        return Err(format!(
            "{member} must contain from 1 to {max_len} letters, numbers, or hyphens; \
             got {length} characters"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Err(format!(
            "the first character of {member} must be a letter; got '{value}'"
        ));
    }
    if value.ends_with('-') {
        return Err(format!("{member} can't end with a hyphen; got '{value}'"));
    }
    if value.contains("--") {
        return Err(format!(
            "{member} can't contain two consecutive hyphens; got '{value}'"
        ));
    }
    Ok(())
}

/// Refuse a `MaxRecords` outside the range every describe operation documents.
pub fn check_max_records(max_records: Option<i32>) -> Result<usize, String> {
    match max_records {
        None => Ok(DEFAULT_MAX_RECORDS),
        Some(value) if MAX_RECORDS_RANGE.contains(&value) => Ok(value as usize),
        Some(value) => Err(format!(
            "invalid value {value} for MaxRecords: must be a value from {} to {}",
            MAX_RECORDS_RANGE.start(),
            MAX_RECORDS_RANGE.end()
        )),
    }
}

/// Refuse a describe request carrying more filter values than [`MAX_FILTER_TERMS`].
///
/// The scan is the product of the stored record count and the filter terms, and it
/// runs with this service's one state lock held; the record count is capped by the
/// per-region quotas, and this caps the other factor.
pub fn check_filter_terms(filters: &[crate::ops::types::Filter]) -> Result<(), String> {
    let terms: usize = filters.iter().map(|filter| filter.values.len()).sum();
    if terms > MAX_FILTER_TERMS {
        return Err(format!(
            "Filters carries {terms} values: this emulator serves at most {MAX_FILTER_TERMS}"
        ));
    }
    Ok(())
}

/// One page of an ordered result set, plus the `Marker` to resume from.
///
/// The marker is the key the next page starts AT, so resuming is a single ordered
/// lookup and a page boundary cannot drift when a record is added or removed between
/// calls. A marker is returned only while records remain, which is what `@paginated`
/// callers loop on.
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_marker: Option<String>,
}

/// Take the page of `records` that starts at `marker` and holds at most
/// `page_size` items. `records` must be ordered by its key.
pub fn paginate<T>(
    records: Vec<(String, T)>,
    marker: Option<&str>,
    page_size: usize,
) -> Page<T> {
    let start = match marker {
        None => 0,
        Some(marker) => records
            .iter()
            .position(|(key, _)| key.as_str() >= marker)
            .unwrap_or(records.len()),
    };
    let end = start.saturating_add(page_size).min(records.len());
    let next_marker = records.get(end).map(|(key, _)| key.clone());
    let items = records
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(_, record)| record)
        .collect();
    Page { items, next_marker }
}

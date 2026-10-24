// SPDX-License-Identifier: CC0-1.0

//! The RDS Proxy records `CreateDBProxy`, `DescribeDBProxies`,
//! `RegisterDBProxyTargets`, `DescribeDBProxyTargets` and `DeregisterDBProxyTargets`
//! share.
//!
//! A proxy's TARGETS are stored as registrations — which DB instance or DB cluster was
//! registered, and nothing else — so everything a target reports about the database behind
//! it (its ARN, its endpoint, its port) is read from that database's own record when the
//! target is projected. A deleted database therefore cannot leave a target reporting an
//! endpoint that no longer exists.

use std::time::Instant;

use super::{PendingChange, PendingStatus, ARN_NAMESPACE};
use crate::ops::types::{DBProxy, DBProxyTarget, Tag, TargetHealth, UserAuthConfigInfo};

/// How many proxies one region holds. The cap is this emulator's, and it bounds the store
/// the same way the other per-region quotas in `src/state.rs` do.
pub const DB_PROXY_QUOTA_PER_REGION: usize = 20;

/// How many targets one proxy's target group holds. "Currently, each target group is
/// associated with exactly one RDS DB instance or Aurora DB cluster"
/// (`DBProxyTargetGroup`), so this is generous; it bounds a caller-sized list that enters
/// the store.
pub const DB_PROXY_TARGET_QUOTA_PER_PROXY: usize = 20;

/// `DBProxyStatus` — the status a just-created proxy reports while this service brings it
/// up.
pub const DB_PROXY_STATUS_CREATING: &str = "creating";

/// `DBProxyStatus` — "A status of `available` means the proxy is ready to handle
/// requests."
pub const DB_PROXY_STATUS_AVAILABLE: &str = "available";

/// The only target group name a proxy has: "Currently, this name must be `default`"
/// (`RegisterDBProxyTargets$TargetGroupName`), because each proxy has exactly one target
/// group and the service names it that.
pub const DEFAULT_DB_PROXY_TARGET_GROUP: &str = "default";

/// `CreateDBProxy$IdleClientTimeout`'s documented default: "Default: 1800 (30 minutes)".
pub const DEFAULT_IDLE_CLIENT_TIMEOUT: i32 = 1800;

/// `CreateDBProxy$DefaultAuthScheme`'s default, the `DefaultAuthScheme` value that leaves
/// authentication to the `Auth` list rather than using end-to-end IAM authentication.
pub const DEFAULT_AUTH_SCHEME_NONE: &str = "NONE";

/// `EndpointNetworkType` / `TargetConnectionNetworkType`: the IPv4 value both default to,
/// the only network type an emulated proxy endpoint serves.
pub const NETWORK_TYPE_IPV4: &str = "IPV4";

/// `TargetHealth$State` — "`AVAILABLE`", the state of a registered target whose database
/// this service still holds.
pub const TARGET_STATE_AVAILABLE: &str = "AVAILABLE";

/// `TargetHealth$State` — "`UNAVAILABLE`", the state of a registered target whose database
/// is gone.
pub const TARGET_STATE_UNAVAILABLE: &str = "UNAVAILABLE";

/// `TargetHealthReason` — "`UNREACHABLE`", why a target whose database is gone is
/// unavailable.
pub const TARGET_HEALTH_REASON_UNREACHABLE: &str = "UNREACHABLE";

/// `TargetType` — "`RDS_INSTANCE`", the kind of database a registered DB instance is.
pub const TARGET_TYPE_RDS_INSTANCE: &str = "RDS_INSTANCE";

/// `TargetType` — "`TRACKED_CLUSTER`", the kind of database a registered DB cluster is.
pub const TARGET_TYPE_TRACKED_CLUSTER: &str = "TRACKED_CLUSTER";

/// `TargetRole` — "`READ_WRITE`", the role of the writer a proxy connects to.
pub const TARGET_ROLE_READ_WRITE: &str = "READ_WRITE";

/// The ARN resource type of a proxy, as AWS writes it under the `rds` namespace.
const ARN_RESOURCE_TYPE: &str = "db-proxy";

/// The ARN resource type of a proxy's target group.
const TARGET_GROUP_ARN_RESOURCE_TYPE: &str = "target-group";

/// Refuse a `TargetGroupName` that is not the one target group a proxy has.
///
/// "Currently, this name must be `default`" — each proxy has exactly one target group and
/// the service names it that, so any other name names no target group of this proxy, which
/// is what `DBProxyTargetGroupNotFoundFault` reports. `Err` carries the message.
pub fn check_target_group_name(proxy_name: &str, requested: Option<&str>) -> Result<(), String> {
    match requested {
        None => Ok(()),
        Some(name) if name == DEFAULT_DB_PROXY_TARGET_GROUP => Ok(()),
        Some(name) => Err(format!(
            "DBProxyTargetGroup {name} not found for DB proxy {proxy_name}: a proxy has one \
             target group, named {DEFAULT_DB_PROXY_TARGET_GROUP}"
        )),
    }
}

/// One registration in a proxy's target group: which database was registered, and when.
#[derive(Debug, Clone)]
pub struct DbProxyTargetRecord {
    /// The `DBInstanceIdentifier` or `DBClusterIdentifier` that was registered, stored
    /// lowercased like the record it names.
    pub identifier: String,
    /// Whether the registration names a DB CLUSTER rather than a DB instance.
    pub is_cluster: bool,
}

/// A persisted RDS proxy.
#[derive(Debug, Clone)]
pub struct DbProxyRecord {
    /// `DBProxyName`, which is half this record's key.
    pub name: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// `EngineFamily`.
    pub engine_family: String,
    /// `DefaultAuthScheme`.
    pub default_auth_scheme: String,
    /// `Auth`, as the proxy reports it back.
    pub auth: Vec<UserAuthConfigInfo>,
    /// `RoleArn`.
    pub role_arn: String,
    /// `VpcSubnetIds`.
    pub vpc_subnet_ids: Vec<String>,
    /// `VpcSecurityGroupIds`.
    pub vpc_security_group_ids: Vec<String>,
    /// `RequireTLS`.
    pub require_tls: bool,
    /// `IdleClientTimeout`.
    pub idle_client_timeout: i32,
    /// `DebugLogging`.
    pub debug_logging: bool,
    /// `EndpointNetworkType`.
    pub endpoint_network_type: String,
    /// `TargetConnectionNetworkType`.
    pub target_connection_network_type: String,
    /// The minted DNS label the proxy's endpoint carries.
    pub endpoint_host: String,
    /// `Status`.
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `CreatedDate`.
    pub created_date: ::aws_smithy_types::DateTime,
    /// `UpdatedDate`.
    pub updated_date: ::aws_smithy_types::DateTime,
    /// The databases registered in the proxy's one target group, in registration order.
    pub targets: Vec<DbProxyTargetRecord>,
    /// The tags the create carried. The `DBProxy` shape reports none, so these are held
    /// for the tagging operations rather than projected onto the proxy.
    pub tags: Vec<Tag>,
}

impl DbProxyRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// `DBProxyArn`, from the stored region and the proxy's name.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{ARN_RESOURCE_TYPE}:{}", self.name),
        )
    }

    /// `DBProxyTargetGroup$TargetGroupArn` for this proxy's one target group.
    pub fn target_group_arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{TARGET_GROUP_ARN_RESOURCE_TYPE}:{}", self.name),
        )
    }

    /// "The endpoint that you can use to connect to the DB proxy", whose published form is
    /// `<proxy name>.proxy-<server-assigned label>.<region>.rds.amazonaws.com` (Amazon RDS
    /// User Guide, "Connecting to a database through RDS Proxy").
    pub fn endpoint(&self) -> String {
        format!(
            "{}.proxy-{}.{}.rds.amazonaws.com",
            self.name, self.endpoint_host, self.region
        )
    }

    /// Whether `identifier` is already registered in the proxy's target group.
    pub fn target_position(&self, identifier: &str, is_cluster: bool) -> Option<usize> {
        self.targets
            .iter()
            .position(|target| target.identifier == identifier && target.is_cluster == is_cluster)
    }

    /// Project the stored record onto the modelled `DBProxy` shape. `VpcId` is left out
    /// rather than invented: this service holds no VPC, so a proxy it stores belongs to
    /// none it could name.
    pub fn to_wire(&self) -> DBProxy {
        DBProxy {
            db_proxy_name: Some(self.name.clone()),
            db_proxy_arn: Some(self.arn()),
            status: Some(self.status.clone()),
            engine_family: Some(self.engine_family.clone()),
            vpc_id: None,
            vpc_security_group_ids: Some(self.vpc_security_group_ids.clone()),
            vpc_subnet_ids: Some(self.vpc_subnet_ids.clone()),
            default_auth_scheme: Some(self.default_auth_scheme.clone()),
            auth: Some(self.auth.clone()),
            role_arn: Some(self.role_arn.clone()),
            endpoint: Some(self.endpoint()),
            require_tls: Some(self.require_tls),
            idle_client_timeout: Some(self.idle_client_timeout),
            debug_logging: Some(self.debug_logging),
            created_date: Some(self.created_date),
            updated_date: Some(self.updated_date),
            endpoint_network_type: Some(self.endpoint_network_type.clone()),
            target_connection_network_type: Some(self.target_connection_network_type.clone()),
        }
    }
}

/// Project a registration onto the modelled `DBProxyTarget`, with everything about the
/// database behind it read from `database`.
///
/// `database` is `None` when the registered DB instance or DB cluster is no longer in the
/// store: the registration stands, and the target reports the health of one whose database
/// cannot be reached rather than an endpoint that answers nothing.
pub fn target_to_wire(
    target: &DbProxyTargetRecord,
    database: Option<RegisteredDatabase>,
) -> DBProxyTarget {
    let target_type = if target.is_cluster {
        TARGET_TYPE_TRACKED_CLUSTER
    } else {
        TARGET_TYPE_RDS_INSTANCE
    };
    match database {
        Some(database) => DBProxyTarget {
            target_arn: Some(database.arn),
            endpoint: Some(database.endpoint),
            tracked_cluster_id: target
                .is_cluster
                .then(|| target.identifier.clone()),
            rds_resource_id: Some(database.resource_id),
            port: Some(database.port),
            r#type: Some(target_type.to_string()),
            role: Some(TARGET_ROLE_READ_WRITE.to_string()),
            target_health: Some(TargetHealth {
                state: Some(TARGET_STATE_AVAILABLE.to_string()),
                reason: None,
                description: None,
            }),
        },
        None => DBProxyTarget {
            target_arn: None,
            endpoint: None,
            tracked_cluster_id: target
                .is_cluster
                .then(|| target.identifier.clone()),
            rds_resource_id: Some(target.identifier.clone()),
            port: None,
            r#type: Some(target_type.to_string()),
            role: Some(TARGET_ROLE_READ_WRITE.to_string()),
            target_health: Some(TargetHealth {
                state: Some(TARGET_STATE_UNAVAILABLE.to_string()),
                reason: Some(TARGET_HEALTH_REASON_UNREACHABLE.to_string()),
                description: Some(format!(
                    "{} {} is no longer available",
                    if target.is_cluster {
                        "DB cluster"
                    } else {
                        "DB instance"
                    },
                    target.identifier
                )),
            }),
        },
    }
}

/// What a proxy target reports about the database behind it, read from that database's own
/// record.
#[derive(Debug, Clone)]
pub struct RegisteredDatabase {
    /// The database's ARN.
    pub arn: String,
    /// The writer endpoint a client connects to.
    pub endpoint: String,
    /// The database's immutable resource id.
    pub resource_id: String,
    /// The port the proxy connects to the database on.
    pub port: i32,
}

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
use crate::ops::types::{
    ConnectionPoolConfiguration, ConnectionPoolConfigurationInfo, DBProxy, DBProxyTarget,
    DBProxyTargetGroup, Tag, TargetHealth, UserAuthConfigInfo,
};

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

/// `DBProxyStatus` — the status a proxy reports once a delete has been accepted for it and
/// before this service has finished taking it away.
pub const DB_PROXY_STATUS_DELETING: &str = "deleting";

/// The only target group name a proxy has: "Currently, this name must be `default`"
/// (`RegisterDBProxyTargets$TargetGroupName`), because each proxy has exactly one target
/// group and the service names it that.
pub const DEFAULT_DB_PROXY_TARGET_GROUP: &str = "default";

/// `CreateDBProxy$IdleClientTimeout`'s documented default: "Default: 1800 (30 minutes)".
pub const DEFAULT_IDLE_CLIENT_TIMEOUT: i32 = 1800;

/// The `EngineFamily` whose connection pool defaults differ from every other engine's.
pub const ENGINE_FAMILY_SQLSERVER: &str = "SQLSERVER";

/// `ConnectionPoolConfiguration$MaxConnectionsPercent`'s documented default for RDS for
/// Microsoft SQL Server: "Default: 10 for RDS for Microsoft SQL Server, and 100 for all
/// other engines".
pub const DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER: i32 = 10;

/// `ConnectionPoolConfiguration$MaxConnectionsPercent`'s documented default for every
/// other engine (same sentence as above).
pub const DEFAULT_MAX_CONNECTIONS_PERCENT: i32 = 100;

/// `ConnectionPoolConfiguration$ConnectionBorrowTimeout`'s documented default:
/// "Default: 120" seconds.
pub const DEFAULT_CONNECTION_BORROW_TIMEOUT: i32 = 120;

/// The connection pool settings a proxy's target group starts with, which are the ones
/// `ConnectionPoolConfiguration` documents as its defaults for `engine_family`:
/// `MaxConnectionsPercent` is engine-family dependent, and "the default value of
/// `MaxIdleConnectionsPercent` is half of the value of `MaxConnectionsPercent`" — which is
/// the 5 and 50 that member documents for SQL Server and for all other engines.
pub fn default_connection_pool_config(engine_family: &str) -> ConnectionPoolConfigurationInfo {
    let max_connections_percent = if engine_family == ENGINE_FAMILY_SQLSERVER {
        DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER
    } else {
        DEFAULT_MAX_CONNECTIONS_PERCENT
    };
    ConnectionPoolConfigurationInfo {
        max_connections_percent: Some(max_connections_percent),
        max_idle_connections_percent: Some(max_connections_percent / 2),
        connection_borrow_timeout: Some(DEFAULT_CONNECTION_BORROW_TIMEOUT),
        // "Each item in the list represents a class of SQL operations" exempted from
        // pinning: a target group starts with none exempted.
        session_pinning_filters: Some(Vec::new()),
        // "One or more SQL statements for the proxy to run when opening each new database
        // connection" — none until a modification sets one.
        init_query: None,
    }
}

/// `ConnectionPoolConfiguration$MaxConnectionsPercent`'s published constraint: "Must be
/// between 1 and 100."
const MAX_CONNECTIONS_PERCENT_RANGE: ::std::ops::RangeInclusive<i32> = 1..=100;

/// `ConnectionPoolConfiguration$ConnectionBorrowTimeout`'s published constraint: "Must be
/// between 0 and 300."
const CONNECTION_BORROW_TIMEOUT_RANGE: ::std::ops::RangeInclusive<i32> = 0..=300;

/// How many session pinning filters one target group holds. The cap is this emulator's: it
/// bounds a caller-sized list that enters the store, which the API publishes no limit for.
pub const MAX_SESSION_PINNING_FILTERS: usize = 20;

/// The sentence `ModifyDBProxyTargetGroup` refuses a rename of the group every proxy has
/// with: "You can't rename the default target group" (`ModifyDBProxyTargetGroup$NewName`),
/// and that group is the only one a proxy has.
pub const DEFAULT_TARGET_GROUP_RENAME_MESSAGE: &str =
    "You can't rename the default target group.";

/// The settings a target group holds once `ConnectionPoolConfiguration` has been applied
/// over them.
///
/// `ModifyDBProxyTargetGroup` carries the settings a caller wants CHANGED, so a member the
/// request leaves out keeps the value the group already holds — which is what makes the
/// call that sets an initialization query alone leave the pool sizes as they were. `Err`
/// carries the sentence a value outside the constraints that shape publishes is refused
/// with: each range is quoted on the constant that holds it, and
/// `MaxIdleConnectionsPercent` "must be between 0 and the value of
/// `MaxConnectionsPercent`" — the value in force after this call, not the one the group
/// held before it.
pub fn merged_connection_pool_config(
    held: &ConnectionPoolConfigurationInfo,
    requested: &ConnectionPoolConfiguration,
) -> Result<ConnectionPoolConfigurationInfo, String> {
    let mut merged = held.clone();
    if let Some(percent) = requested.max_connections_percent {
        check_range(
            "MaxConnectionsPercent",
            percent,
            &MAX_CONNECTIONS_PERCENT_RANGE,
        )?;
        merged.max_connections_percent = Some(percent);
    }
    if let Some(timeout) = requested.connection_borrow_timeout {
        check_range(
            "ConnectionBorrowTimeout",
            timeout,
            &CONNECTION_BORROW_TIMEOUT_RANGE,
        )?;
        merged.connection_borrow_timeout = Some(timeout);
    }
    if let Some(percent) = requested.max_idle_connections_percent {
        let ceiling = merged
            .max_connections_percent
            .unwrap_or(*MAX_CONNECTIONS_PERCENT_RANGE.end());
        check_range("MaxIdleConnectionsPercent", percent, &(0..=ceiling))?;
        merged.max_idle_connections_percent = Some(percent);
    }
    if let Some(filters) = &requested.session_pinning_filters {
        super::check_request_list_len(
            "SessionPinningFilters",
            filters.len(),
            MAX_SESSION_PINNING_FILTERS,
        )?;
        for filter in filters {
            super::check_free_form("SessionPinningFilters", filter)?;
        }
        merged.session_pinning_filters = Some(filters.clone());
    }
    if let Some(query) = &requested.init_query {
        super::check_free_form("InitQuery", query)?;
        merged.init_query = Some(query.clone());
    }
    Ok(merged)
}

/// Refuse a connection pool setting outside the range that member's own doc publishes.
fn check_range(
    member: &str,
    value: i32,
    range: &::std::ops::RangeInclusive<i32>,
) -> Result<(), String> {
    if range.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {value} for {member}. Must be between {} and {}.",
        range.start(),
        range.end()
    ))
}

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

/// The message a `DBProxyNotFoundFault` carries for a `DBProxyName` that resolves no proxy:
/// the fault reports "the DB proxy name doesn't refer to an existing DB proxy", and Amazon
/// RDS words it with the requested name in SINGLE QUOTES inside the sentence.
///
/// It lives here rather than in one handler because every operation that resolves a
/// `DBProxyName` answers the same one fault, and the quoting is part of the text a caller
/// matches the refusal against. `named` is the value the REQUEST carried, which is the
/// spelling a proxy is stored under too (`DBProxyName` is not folded).
pub fn db_proxy_not_found_message(named: &str) -> String {
    format!("DBProxy '{named}' not found.")
}

/// The sentence `RegisterDBProxyTargets` and `DeregisterDBProxyTargets` refuse a request
/// that names no database to act on with.
///
/// Both members that name one are optional, so a request can carry neither — and a
/// registration of nothing is not a registration. Amazon RDS words the refusal as a
/// statement about what is missing, naming the two KINDS of database rather than the
/// modelled member names, and interpolates nothing from the request.
pub const MISSING_DB_PROXY_TARGET_IDENTIFIER_MESSAGE: &str =
    "Missing DB instance or DB cluster identifier.";

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

/// The one target group a proxy has, as this service persists it.
///
/// "Currently, each target group is associated with exactly one RDS DB instance or Aurora
/// DB cluster" and each proxy has exactly one target group, so the group is held on the
/// proxy record rather than keyed separately; its NAME is therefore not stored (it is
/// always [`DEFAULT_DB_PROXY_TARGET_GROUP`], which is what `check_target_group_name`
/// resolves against) and its registrations live in `DbProxyRecord::targets`. What IS
/// stored is what a modification of the group changes: its connection pool settings, and
/// when the group itself was created and last updated.
#[derive(Debug, Clone)]
pub struct DbProxyTargetGroupRecord {
    /// `DBProxyTargetGroup$ConnectionPoolConfig`.
    pub connection_pool_config: ConnectionPoolConfigurationInfo,
    /// `DBProxyTargetGroup$CreatedDate`.
    pub created_date: ::aws_smithy_types::DateTime,
    /// `DBProxyTargetGroup$UpdatedDate`.
    pub updated_date: ::aws_smithy_types::DateTime,
}

impl DbProxyTargetGroupRecord {
    /// The target group a proxy of `engine_family` is created with at `now`: the
    /// documented default connection pool settings, created and last updated at the
    /// instant the proxy was.
    pub fn new(engine_family: &str, now: ::aws_smithy_types::DateTime) -> Self {
        Self {
            connection_pool_config: default_connection_pool_config(engine_family),
            created_date: now,
            updated_date: now,
        }
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
    /// The proxy's one target group, which it is created with.
    pub target_group: DbProxyTargetGroupRecord,
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

    /// Accept a delete: the proxy reports `deleting` from now on, and the clock owes its
    /// removal from the store [`crate::state::STATUS_TRANSITION_DELAY`] from now, after
    /// which every read of the name is a `DBProxyNotFoundFault`. Nothing sleeps: the due
    /// instant is stored and materialised by `settle_due_transitions`.
    pub fn begin_deletion(&mut self) {
        self.status = DB_PROXY_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
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

    /// Project the proxy's one target group onto the modelled `DBProxyTargetGroup`.
    ///
    /// `IsDefault` is `true` because "each proxy is currently associated with a single
    /// target group", and the group reports the PROXY's status: it is created with the
    /// proxy and has no provisioning of its own, so it is ready exactly when the proxy is
    /// ("Other values indicate that you must wait for the target group to be ready").
    pub fn target_group_to_wire(&self) -> DBProxyTargetGroup {
        DBProxyTargetGroup {
            db_proxy_name: Some(self.name.clone()),
            target_group_name: Some(DEFAULT_DB_PROXY_TARGET_GROUP.to_string()),
            target_group_arn: Some(self.target_group_arn()),
            is_default: Some(true),
            status: Some(self.status.clone()),
            connection_pool_config: Some(self.target_group.connection_pool_config.clone()),
            created_date: Some(self.target_group.created_date),
            updated_date: Some(self.target_group.updated_date),
        }
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

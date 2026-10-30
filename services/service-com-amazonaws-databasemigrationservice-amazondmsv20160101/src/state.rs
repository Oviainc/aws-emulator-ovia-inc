// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-databasemigrationservice-amazondmsv20160101 service.
//!
//! This module owns the replication-instance, endpoint and replication-task records
//! the implemented operations read and write, the resource-id minting the DMS ARNs
//! are built from, the identifier rules the member docs publish, and the single
//! place where a clock-owed status transition is materialised ([`settle`]).
//!
//! [`settle`]: ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State::settle

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    DmsTransferSettings, DocDbSettings, DynamoDbSettings, ElasticsearchSettings, Endpoint,
    GcpMySQLSettings, IBMDb2Settings, KafkaSettings, KinesisSettings, MicrosoftSQLServerSettings,
    MongoDbSettings, MySQLSettings, NeptuneSettings, OracleSettings, PostgreSQLSettings,
    RedisSettings, RedshiftSettings, ReplicationInstance, ReplicationTask, S3Settings,
    SybaseSettings, Tag, TimestreamSettings, VpcSecurityGroupMembership,
};

/// A replication instance `CreateReplicationInstance` has accepted but not finished
/// provisioning. One of the values the `ReplicationInstance$ReplicationInstanceStatus`
/// doc publishes.
pub const INSTANCE_CREATING: &str = "creating";
/// A replication instance that finished provisioning.
pub const INSTANCE_AVAILABLE: &str = "available";

/// The status an endpoint carries. DMS creates an endpoint ready — it provisions
/// nothing — so this is the only endpoint status this slice reports.
pub const ENDPOINT_ACTIVE: &str = "active";

/// A task `CreateReplicationTask` has accepted but not finished creating: "the task
/// is being created in response to running the CreateReplicationTask operation"
/// (`ReplicationTask$Status` doc).
pub const TASK_CREATING: &str = "creating";
/// A created task that has never run, and can be started.
pub const TASK_READY: &str = "ready";
/// A task `StartReplicationTask` has accepted.
pub const TASK_STARTING: &str = "starting";
/// A task that is migrating data.
pub const TASK_RUNNING: &str = "running";
/// A task that was stopped, and so can be started again.
pub const TASK_STOPPED: &str = "stopped";
/// A task `DeleteReplicationTask` has accepted: "the task is being deleted in
/// response to running the DeleteReplicationTask operation" (`ReplicationTask$Status`
/// doc).
pub const TASK_DELETING: &str = "deleting";

/// How long after the call that starts it a status transition this emulator owes
/// becomes due (`creating` -> `available` for an instance, `creating` -> `ready` and
/// `starting` -> `running` for a task, `deleting` -> gone). A chosen span, not an
/// AWS-documented duration: it only has to be non-zero, so a caller that acts
/// straight after a create, start or delete still observes the transient status and
/// the state errors that go with it.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(400);

/// The region used to build ARNs and name Availability Zones when a request carries
/// no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The storage a replication instance gets when the request names no
/// `AllocatedStorage`. The emulator's own default: the model publishes no default for
/// the member.
pub const DEFAULT_ALLOCATED_STORAGE: i32 = 50;

/// The engine version a replication instance gets when the request names none.
/// `CreateReplicationInstance`'s `EngineVersion` doc says "If an engine version
/// number is not specified when a replication instance is created, the default is the
/// latest engine version available", and the model publishes no version catalog, so
/// this is the version THIS emulator presents as its latest.
pub const DEFAULT_ENGINE_VERSION: &str = "3.5.4";

/// The SSL mode an endpoint gets when the request names none: the `Endpoint$SslMode`
/// doc says "The default value is none."
pub const DEFAULT_SSL_MODE: &str = "none";

/// How an engine settings structure reports authentication when the request named no
/// method.
///
/// DMS reports `password` there — authentication by the endpoint's own user name and
/// password — for an endpoint created without one, and `password` is the value each
/// engine's authentication-method enum models for exactly that
/// (`MySQLAuthenticationMethod`, `PostgreSQLAuthenticationMethod`,
/// `OracleAuthenticationMethod` and `SqlServerAuthenticationMethod`); the other values
/// each name an alternative the caller has to ask for (`iam`, `kerberos`).
pub const DEFAULT_AUTHENTICATION_METHOD: &str = "password";

/// Whether an endpoint a caller created is read-only. `Endpoint$IsReadOnly`: "When set
/// to true, this endpoint is managed by DMS as part of a zero-ETL integration and
/// cannot be modified or deleted directly" — which a caller-created endpoint is not.
pub const DEFAULT_IS_READ_ONLY: bool = false;

/// The expanded engine name each `EngineName` value carries, which is what
/// `Endpoint$EngineDisplayName` reports: "The expanded name for the engine name. For
/// example, if the EngineName parameter is \"aurora\", this value would be \"Amazon
/// Aurora MySQL\"". The keys are the values `CreateEndpoint$EngineName` publishes.
const ENGINE_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("aurora", "Amazon Aurora MySQL"),
    ("aurora-serverless", "Amazon Aurora MySQL Serverless"),
    ("aurora-postgresql", "Amazon Aurora PostgreSQL"),
    ("aurora-postgresql-serverless", "Amazon Aurora PostgreSQL Serverless"),
    ("azuredb", "Microsoft Azure SQL Database"),
    ("azure-sql-managed-instance", "Microsoft Azure SQL Managed Instance"),
    ("babelfish", "Babelfish"),
    ("db2", "IBM Db2 LUW"),
    ("db2-zos", "IBM Db2 for z/OS"),
    ("dms-transfer", "DMS Transfer"),
    ("docdb", "Amazon DocumentDB"),
    ("documentdb", "Amazon DocumentDB"),
    ("dynamodb", "Amazon DynamoDB"),
    ("elasticsearch", "Amazon OpenSearch Service"),
    ("gcp-mysql", "Google Cloud SQL for MySQL"),
    ("kafka", "Apache Kafka"),
    ("kinesis", "Amazon Kinesis Data Streams"),
    ("mariadb", "MariaDB"),
    ("mongodb", "MongoDB"),
    ("mysql", "MySQL"),
    ("neptune", "Amazon Neptune"),
    ("opensearch", "Amazon OpenSearch Service"),
    ("oracle", "Oracle"),
    ("postgres", "PostgreSQL"),
    ("redis", "Redis"),
    ("redshift", "Amazon Redshift"),
    ("redshift-serverless", "Amazon Redshift Serverless"),
    ("s3", "Amazon S3"),
    ("sqlserver", "Microsoft SQL Server"),
    ("sybase", "SAP ASE"),
    ("timestream", "Amazon Timestream"),
];

/// The expanded name for `engine_name`.
///
/// An engine name outside the published catalog is reported as given rather than
/// guessed at: the member still carries the engine the endpoint runs. Refusing an
/// engine name the catalog does not list belongs to the create path's own validation
/// rule; once that lands, this fallback is unreachable for a stored endpoint.
pub fn engine_display_name(engine_name: &str) -> String {
    ENGINE_DISPLAY_NAMES
        .iter()
        .find(|(name, _)| *name == engine_name)
        .map(|(_, display)| (*display).to_string())
        .unwrap_or_else(|| engine_name.to_string())
}

/// The key an endpoint's connection parameters are encrypted with when the request
/// named none: "If you don't specify a value for the KmsKeyId parameter, then DMS uses
/// your default encryption key. KMS creates the default encryption key for your Amazon
/// Web Services account. Your Amazon Web Services account has a different default
/// encryption key for each Amazon Web Services Region" (`Endpoint$KmsKeyId`).
///
/// Derived from the region alone, so it is one key per region, stable for the life of
/// every endpoint that reports it.
pub fn default_kms_key_arn(region: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"dms-default-encryption-key");
    hasher.update(region.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(&digest[..16]);
    let key_id = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    emulator_server::emulator_arn("kms", region, &format!("key/{key_id}"))
}

/// Whether a replication instance the request says nothing about is publicly
/// accessible: the `PubliclyAccessible` doc says "The default value is true."
pub const DEFAULT_PUBLICLY_ACCESSIBLE: bool = true;

/// Whether minor engine upgrades are applied automatically when the request says
/// nothing: the `AutoMinorVersionUpgrade` doc says "This parameter defaults to true."
pub const DEFAULT_AUTO_MINOR_VERSION_UPGRADE: bool = true;

/// The maintenance window a replication instance gets when the request names none.
/// The doc says "A 30-minute window selected at random from an 8-hour block of time
/// per Amazon Web Services Region"; this emulator picks one deterministically instead,
/// in the documented `ddd:hh24:mi-ddd:hh24:mi` format.
pub const DEFAULT_MAINTENANCE_WINDOW: &str = "sun:03:00-sun:03:30";

/// The status a VPC security group carries once it is attached to an instance.
pub const VPC_SECURITY_GROUP_ACTIVE: &str = "active";

/// Smallest `MaxRecords` a paginated DMS describe accepts, per the member doc:
/// "Constraints: Minimum 20, maximum 100."
pub const MAX_RECORDS_MIN: i32 = 20;
/// Largest `MaxRecords` a paginated DMS describe accepts ("maximum 100").
pub const MAX_RECORDS_MAX: i32 = 100;
/// The page size a describe uses when the request names no `MaxRecords`:
/// "Default: 100".
pub const DEFAULT_MAX_RECORDS: i32 = 100;

/// The most characters a `ResourceIdentifier` may carry: "The value for this
/// parameter can have up to 31 characters."
pub const RESOURCE_IDENTIFIER_MAX_LEN: usize = 31;

/// The most characters a `ReplicationInstanceIdentifier` may carry: "Must contain
/// 1-63 alphanumeric characters or hyphens."
pub const INSTANCE_IDENTIFIER_MAX_LEN: usize = 63;

/// The most characters a `ReplicationTaskIdentifier` may carry: "Must contain 1-255
/// alphanumeric characters or hyphens."
pub const TASK_IDENTIFIER_MAX_LEN: usize = 255;

/// How many characters a minted resource id (the last ARN field) carries. Inside the
/// 31 characters a caller-supplied `ResourceIdentifier` may have, so a minted id and
/// a supplied one are interchangeable.
const MINTED_RESOURCE_ID_LEN: usize = 26;

/// The alphabet a minted resource id is drawn from: uppercase ASCII letters and
/// digits, which is what the `ResourceIdentifier` doc allows ("only ASCII letters,
/// digits, and hyphen").
const RESOURCE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The most replication instances this emulator's store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// `CreateReplicationInstance` is the only thing that adds one, so the store needs a
/// maximum entry count: this is the emulator's OWN ceiling, not a modelled or
/// published DMS quota.
pub const MAX_REPLICATION_INSTANCES: usize = 100;
/// The most endpoints this emulator's store holds — again the emulator's own ceiling.
pub const MAX_ENDPOINTS: usize = 1000;
/// The most replication tasks this emulator's store holds — the emulator's own
/// ceiling.
pub const MAX_REPLICATION_TASKS: usize = 1000;

/// What the clock owes a record once [`Pending::due`] passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owed {
    /// Move the record to this status.
    ToStatus(&'static str),
    /// Drop the record (a deleted task leaves the store).
    Removal,
}

/// A transition this emulator owes a record, and the instant it comes due. The
/// instant is stored, never slept on.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    /// When the transition is due.
    pub due: SystemTime,
    /// What is owed.
    pub owed: Owed,
}

impl Pending {
    /// A transition due [`TRANSITION_DELAY`] after `now`.
    pub fn after(now: SystemTime, owed: Owed) -> Self {
        Self {
            due: now + TRANSITION_DELAY,
            owed,
        }
    }
}

/// One replication instance.
#[derive(Debug, Clone)]
pub struct ReplicationInstanceRecord {
    /// The instance's ARN, `arn:aws:dms:<region>:<account>:rep:<resource id>`.
    pub arn: String,
    /// The caller's identifier, stored lowercased as the member doc requires.
    pub identifier: String,
    /// The compute and memory capacity the caller asked for.
    pub instance_class: String,
    /// The instance's status, one of the `ReplicationInstanceStatus` values.
    pub status: String,
    /// Storage allocated to the instance, in gigabytes.
    pub allocated_storage: i32,
    /// When `CreateReplicationInstance` minted it.
    pub create_time: SystemTime,
    /// The Availability Zone the instance runs in.
    pub availability_zone: String,
    /// The standby's Availability Zone, for a Multi-AZ instance only.
    pub secondary_availability_zone: Option<String>,
    /// The instance's engine version.
    pub engine_version: String,
    /// Whether the instance is a Multi-AZ deployment.
    pub multi_az: bool,
    /// Whether the instance has a public IP address.
    pub publicly_accessible: bool,
    /// Whether minor engine upgrades are applied automatically.
    pub auto_minor_version_upgrade: bool,
    /// The weekly maintenance window.
    pub preferred_maintenance_window: String,
    /// The KMS key the caller named, if any.
    pub kms_key_id: Option<String>,
    /// The custom DNS name servers the caller named, if any.
    pub dns_name_servers: Option<String>,
    /// The IP address protocol the caller named, if any.
    pub network_type: Option<String>,
    /// The VPC security groups the caller named.
    pub vpc_security_group_ids: Vec<String>,
    /// The instance's tags.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this instance, if any.
    pub pending: Option<Pending>,
    /// Creation order, so a describe scans instances oldest first.
    pub sequence: u64,
}

impl ReplicationInstanceRecord {
    /// This record as the modelled `ReplicationInstance` shape.
    pub fn to_shape(&self) -> ReplicationInstance {
        ReplicationInstance {
            replication_instance_identifier: Some(self.identifier.clone()),
            replication_instance_class: Some(self.instance_class.clone()),
            replication_instance_status: Some(self.status.clone()),
            allocated_storage: self.allocated_storage,
            instance_create_time: Some(::aws_smithy_types::DateTime::from(self.create_time)),
            vpc_security_groups: Some(
                self.vpc_security_group_ids
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some(VPC_SECURITY_GROUP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            availability_zone: Some(self.availability_zone.clone()),
            // A subnet group is only attached by `CreateReplicationSubnetGroup`, which
            // this slice does not implement.
            replication_subnet_group: None,
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            // Only `ModifyReplicationInstance` leaves pending modifications behind.
            pending_modified_values: None,
            multi_az: self.multi_az,
            engine_version: Some(self.engine_version.clone()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            kms_key_id: self.kms_key_id.clone(),
            replication_instance_arn: Some(self.arn.clone()),
            // The emulator models no EC2, so an instance has no addresses to report;
            // both scalar members are deprecated in the model besides.
            replication_instance_public_ip_address: None,
            replication_instance_private_ip_address: None,
            replication_instance_public_ip_addresses: None,
            replication_instance_private_ip_addresses: None,
            replication_instance_ipv6_addresses: None,
            publicly_accessible: self.publicly_accessible,
            secondary_availability_zone: self.secondary_availability_zone.clone(),
            // Only an instance in the Free DMS program has an expiry.
            free_until: None,
            dns_name_servers: self.dns_name_servers.clone(),
            network_type: self.network_type.clone(),
            // Kerberos settings are only attached by a request that carries them, and
            // this slice stores none.
            kerberos_authentication_settings: None,
        }
    }
}

/// The engine-specific settings structures an endpoint holds, as the request that
/// created it supplied them.
///
/// One field per settings member `CreateEndpoint` models; `EngineName` selects the one
/// an `Endpoint` reports. Build it through [`EngineSettings::sanitized`], which is what
/// keeps a secret out of the store — and so out of every response.
#[derive(Debug, Clone, Default)]
pub struct EngineSettings {
    /// Settings for an Amazon DynamoDB target.
    pub dynamo_db: Option<DynamoDbSettings>,
    /// Settings for an Amazon S3 target.
    pub s3: Option<S3Settings>,
    /// Settings for a DMS Transfer source.
    pub dms_transfer: Option<DmsTransferSettings>,
    /// Settings for a MongoDB source.
    pub mongo_db: Option<MongoDbSettings>,
    /// Settings for an Amazon Kinesis Data Streams target.
    pub kinesis: Option<KinesisSettings>,
    /// Settings for an Apache Kafka target.
    pub kafka: Option<KafkaSettings>,
    /// Settings for an OpenSearch target.
    pub elasticsearch: Option<ElasticsearchSettings>,
    /// Settings for an Amazon Neptune target.
    pub neptune: Option<NeptuneSettings>,
    /// Settings for an Amazon Redshift endpoint.
    pub redshift: Option<RedshiftSettings>,
    /// Settings for a PostgreSQL endpoint.
    pub postgre_sql: Option<PostgreSQLSettings>,
    /// Settings for a MySQL endpoint.
    pub my_sql: Option<MySQLSettings>,
    /// Settings for an Oracle endpoint.
    pub oracle: Option<OracleSettings>,
    /// Settings for a SAP ASE endpoint.
    pub sybase: Option<SybaseSettings>,
    /// Settings for a Microsoft SQL Server endpoint.
    pub microsoft_sql_server: Option<MicrosoftSQLServerSettings>,
    /// Settings for an IBM Db2 endpoint.
    pub ibm_db2: Option<IBMDb2Settings>,
    /// Settings for a DocumentDB endpoint.
    pub doc_db: Option<DocDbSettings>,
    /// Settings for a Redis target.
    pub redis: Option<RedisSettings>,
    /// Settings for a GCP MySQL source.
    pub gcp_my_sql: Option<GcpMySQLSettings>,
    /// Settings for an Amazon Timestream target.
    pub timestream: Option<TimestreamSettings>,
}

/// Clear each named member of an optional settings structure.
macro_rules! clear_members {
    ($holder:expr, $($member:ident),+ $(,)?) => {
        if let Some(settings) = $holder.as_mut() {
            $(settings.$member = None;)+
        }
    };
}

impl EngineSettings {
    /// These settings with every secret member dropped.
    ///
    /// A password a request carried is persisted nowhere, so no response can echo one
    /// back and nothing in the store holds a credential for a real database.
    pub fn sanitized(mut self) -> Self {
        clear_members!(self.my_sql, password);
        clear_members!(self.postgre_sql, password);
        clear_members!(self.oracle, password, asm_password, security_db_encryption);
        clear_members!(self.microsoft_sql_server, password);
        clear_members!(self.sybase, password);
        clear_members!(self.ibm_db2, password);
        clear_members!(self.mongo_db, password);
        clear_members!(self.doc_db, password);
        clear_members!(self.redshift, password);
        clear_members!(self.gcp_my_sql, password);
        clear_members!(self.redis, auth_password);
        clear_members!(self.kafka, sasl_password, ssl_client_key_password);
        self
    }
}

/// Fill the connection parameters an engine settings structure shares with the
/// endpoint's own members.
///
/// What the settings structure itself carries wins, and the endpoint's top-level
/// member fills what the structure left unset: an `Endpoint` reports the connection
/// parameters the request supplied, wherever it supplied them.
macro_rules! with_connection_parameters {
    ($record:expr, $settings:expr) => {{
        let record = $record;
        let mut settings = $settings;
        if settings.server_name.is_none() {
            settings.server_name = record.server_name.clone();
        }
        if settings.port.is_none() {
            settings.port = record.port;
        }
        if settings.database_name.is_none() {
            settings.database_name = record.database_name.clone();
        }
        if settings.username.is_none() {
            settings.username = record.username.clone();
        }
        settings
    }};
}

/// Report [`DEFAULT_AUTHENTICATION_METHOD`] for a settings structure whose engine
/// models an authentication method the request did not name.
macro_rules! with_default_authentication_method {
    ($settings:expr) => {{
        let mut settings = $settings;
        if settings.authentication_method.is_none() {
            settings.authentication_method = Some(DEFAULT_AUTHENTICATION_METHOD.to_string());
        }
        settings
    }};
}

/// One endpoint.
#[derive(Debug, Clone)]
pub struct EndpointRecord {
    /// The endpoint's ARN, `arn:aws:dms:<region>:<account>:endpoint:<resource id>`.
    pub arn: String,
    /// The caller's identifier.
    pub identifier: String,
    /// `source` or `target`, as `ReplicationEndpointTypeValue` models them for a
    /// request.
    pub endpoint_type: String,
    /// The database engine name the caller named.
    pub engine_name: String,
    /// The endpoint's status.
    pub status: String,
    /// The user name used to connect, if the caller named one.
    pub username: Option<String>,
    /// The server at the endpoint, if the caller named one.
    pub server_name: Option<String>,
    /// The port used to reach the endpoint, if the caller named one.
    pub port: Option<i32>,
    /// The database at the endpoint, if the caller named one.
    pub database_name: Option<String>,
    /// Extra connection attributes, if the caller named any.
    pub extra_connection_attributes: Option<String>,
    /// The SSL mode used to connect.
    pub ssl_mode: String,
    /// The key the endpoint's connection parameters are encrypted with: the one the
    /// caller named, or the region's default DMS key ([`default_kms_key_arn`]).
    pub kms_key_id: String,
    /// The certificate used for the SSL connection, if the caller named one.
    pub certificate_arn: Option<String>,
    /// The role DMS assumes to reach the endpoint, if the caller named one.
    pub service_access_role_arn: Option<String>,
    /// The external table definition, if the caller named one.
    pub external_table_definition: Option<String>,
    /// Whether DMS manages this endpoint as part of a zero-ETL integration, which
    /// makes it read-only.
    pub is_read_only: bool,
    /// The engine-specific settings the endpoint carries.
    pub settings: EngineSettings,
    /// The endpoint's tags.
    pub tags: Vec<Tag>,
    /// Creation order, so a describe scans endpoints oldest first.
    pub sequence: u64,
}

impl EndpointRecord {
    /// This record as the modelled `Endpoint` shape — the ONE projection every
    /// operation that answers with an endpoint goes through, so the create path and
    /// the describe path report the same member set for the same endpoint.
    pub fn to_shape(&self) -> Endpoint {
        let mut endpoint = Endpoint {
            endpoint_identifier: Some(self.identifier.clone()),
            // A response carries the endpoint type uppercased; the lowercase
            // `source`/`target` of `ReplicationEndpointTypeValue` are the request form.
            endpoint_type: Some(self.endpoint_type.to_ascii_uppercase()),
            engine_name: Some(self.engine_name.clone()),
            engine_display_name: Some(engine_display_name(&self.engine_name)),
            username: self.username.clone(),
            server_name: self.server_name.clone(),
            port: self.port,
            database_name: self.database_name.clone(),
            extra_connection_attributes: self.extra_connection_attributes.clone(),
            status: Some(self.status.clone()),
            kms_key_id: Some(self.kms_key_id.clone()),
            endpoint_arn: Some(self.arn.clone()),
            certificate_arn: self.certificate_arn.clone(),
            ssl_mode: Some(self.ssl_mode.clone()),
            service_access_role_arn: self.service_access_role_arn.clone(),
            external_table_definition: self.external_table_definition.clone(),
            // A cross-account external id is minted by a cross-account create, which
            // this emulator does not model.
            external_id: None,
            is_read_only: Some(self.is_read_only),
            // A Lakehouse endpoint is one DMS creates itself for a zero-ETL
            // integration, so nothing here holds its settings.
            lakehouse_settings: None,
            ..Endpoint::default()
        };
        self.project_engine_settings(&mut endpoint);
        endpoint
    }

    /// Report the single engine settings structure this endpoint's `EngineName`
    /// selects, leaving every structure belonging to another engine absent.
    ///
    /// A structure whose members are all optional is reported for its engine even when
    /// the request carried none of it, because the endpoint's own connection
    /// parameters and the engine's documented defaults populate it. A structure the
    /// model gives required members (`ElasticsearchSettings`, `NeptuneSettings`,
    /// `RedisSettings`, `TimestreamSettings`, `DynamoDbSettings`) is reported only as
    /// the request supplied it, since nothing here can invent a required value.
    fn project_engine_settings(&self, endpoint: &mut Endpoint) {
        let settings = &self.settings;
        match self.engine_name.as_str() {
            "mysql" | "mariadb" | "aurora" | "aurora-serverless" => {
                endpoint.my_sql_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(self, settings.my_sql.clone().unwrap_or_default())
                ));
            }
            "postgres" | "aurora-postgresql" | "aurora-postgresql-serverless" | "babelfish" => {
                endpoint.postgre_sql_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(
                        self,
                        settings.postgre_sql.clone().unwrap_or_default()
                    )
                ));
            }
            "oracle" => {
                endpoint.oracle_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(self, settings.oracle.clone().unwrap_or_default())
                ));
            }
            "sqlserver" | "azuredb" | "azure-sql-managed-instance" => {
                endpoint.microsoft_sql_server_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(
                        self,
                        settings.microsoft_sql_server.clone().unwrap_or_default()
                    )
                ));
            }
            "sybase" => {
                endpoint.sybase_settings = Some(with_connection_parameters!(
                    self,
                    settings.sybase.clone().unwrap_or_default()
                ));
            }
            "db2" | "db2-zos" => {
                endpoint.ibm_db2_settings = Some(with_connection_parameters!(
                    self,
                    settings.ibm_db2.clone().unwrap_or_default()
                ));
            }
            "mongodb" => {
                endpoint.mongo_db_settings = Some(with_connection_parameters!(
                    self,
                    settings.mongo_db.clone().unwrap_or_default()
                ));
            }
            "docdb" | "documentdb" => {
                endpoint.doc_db_settings = Some(with_connection_parameters!(
                    self,
                    settings.doc_db.clone().unwrap_or_default()
                ));
            }
            "redshift" | "redshift-serverless" => {
                endpoint.redshift_settings = Some(with_connection_parameters!(
                    self,
                    settings.redshift.clone().unwrap_or_default()
                ));
            }
            "gcp-mysql" => {
                endpoint.gcp_my_sql_settings = Some(with_connection_parameters!(
                    self,
                    settings.gcp_my_sql.clone().unwrap_or_default()
                ));
            }
            "s3" => endpoint.s3_settings = settings.s3.clone(),
            "dms-transfer" => endpoint.dms_transfer_settings = settings.dms_transfer.clone(),
            "kinesis" => endpoint.kinesis_settings = settings.kinesis.clone(),
            "kafka" => endpoint.kafka_settings = settings.kafka.clone(),
            "dynamodb" => endpoint.dynamo_db_settings = settings.dynamo_db.clone(),
            "opensearch" | "elasticsearch" => {
                endpoint.elasticsearch_settings = settings.elasticsearch.clone();
            }
            "neptune" => endpoint.neptune_settings = settings.neptune.clone(),
            "redis" => endpoint.redis_settings = settings.redis.clone(),
            "timestream" => endpoint.timestream_settings = settings.timestream.clone(),
            // An engine name the model's catalog does not list selects no settings
            // structure, so reporting none is what keeps a response from carrying one
            // belonging to another engine.
            _ => {}
        }
    }
}

/// One replication task.
#[derive(Debug, Clone)]
pub struct ReplicationTaskRecord {
    /// The task's ARN, `arn:aws:dms:<region>:<account>:task:<resource id>`.
    pub arn: String,
    /// The caller's identifier.
    pub identifier: String,
    /// The ARN of the `source` endpoint the task reads.
    pub source_endpoint_arn: String,
    /// The ARN of the `target` endpoint the task writes.
    pub target_endpoint_arn: String,
    /// The ARN of the replication instance the task runs on.
    pub replication_instance_arn: String,
    /// `full-load`, `cdc` or `full-load-and-cdc`, as `MigrationTypeValue` models them.
    pub migration_type: String,
    /// The table mappings the caller supplied.
    pub table_mappings: String,
    /// The task settings the caller supplied, if any.
    pub replication_task_settings: Option<String>,
    /// Supplemental task data the caller supplied, if any.
    pub task_data: Option<String>,
    /// The task's status, one of the `ReplicationTask$Status` values.
    pub status: String,
    /// Why the task stopped, once it has.
    pub stop_reason: Option<String>,
    /// The last failure the task reported, if any.
    pub last_failure_message: Option<String>,
    /// When `CreateReplicationTask` minted it.
    pub creation_date: SystemTime,
    /// When `StartReplicationTask` last started it.
    pub start_date: Option<SystemTime>,
    /// Where a CDC operation starts, if the caller named a position.
    pub cdc_start_position: Option<String>,
    /// Where a CDC operation stops, if the caller named a position.
    pub cdc_stop_position: Option<String>,
    /// The task's tags.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this task, if any.
    pub pending: Option<Pending>,
    /// Creation order, so a describe scans tasks oldest first.
    pub sequence: u64,
}

impl ReplicationTaskRecord {
    /// This record as the modelled `ReplicationTask` shape.
    ///
    /// `without_settings` drops `ReplicationTaskSettings`, which is what
    /// `DescribeReplicationTasks`' `WithoutSettings` member asks for.
    pub fn to_shape(&self, without_settings: bool) -> ReplicationTask {
        ReplicationTask {
            replication_task_identifier: Some(self.identifier.clone()),
            source_endpoint_arn: Some(self.source_endpoint_arn.clone()),
            target_endpoint_arn: Some(self.target_endpoint_arn.clone()),
            replication_instance_arn: Some(self.replication_instance_arn.clone()),
            migration_type: Some(self.migration_type.clone()),
            table_mappings: Some(self.table_mappings.clone()),
            replication_task_settings: if without_settings {
                None
            } else {
                self.replication_task_settings.clone()
            },
            status: Some(self.status.clone()),
            last_failure_message: self.last_failure_message.clone(),
            stop_reason: self.stop_reason.clone(),
            replication_task_creation_date: Some(::aws_smithy_types::DateTime::from(
                self.creation_date,
            )),
            replication_task_start_date: self
                .start_date
                .map(::aws_smithy_types::DateTime::from),
            cdc_start_position: self.cdc_start_position.clone(),
            cdc_stop_position: self.cdc_stop_position.clone(),
            // A recovery checkpoint is produced by a CDC operation that has read
            // changes; nothing in this emulator reads a source database, so there is
            // no checkpoint to report.
            recovery_checkpoint: None,
            replication_task_arn: Some(self.arn.clone()),
            // Statistics describe a migration that ran. This emulator migrates no
            // data, so it reports none rather than synthesizing counters.
            replication_task_stats: None,
            task_data: self.task_data.clone(),
            // Only `MoveReplicationTask` sets the instance a task is moving to.
            target_replication_instance_arn: None,
        }
    }
}

/// Per-service state for com-amazonaws-databasemigrationservice-amazondmsv20160101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State {
    /// Every replication instance this emulator holds, keyed by ARN.
    pub replication_instances: BTreeMap<String, ReplicationInstanceRecord>,
    /// `ReplicationInstanceIdentifier` -> ARN, so a duplicate create is refused.
    pub instance_arn_by_identifier: BTreeMap<String, String>,
    /// Every endpoint this emulator holds, keyed by ARN.
    pub endpoints: BTreeMap<String, EndpointRecord>,
    /// `EndpointIdentifier` -> ARN, so a duplicate create is refused.
    pub endpoint_arn_by_identifier: BTreeMap<String, String>,
    /// Every replication task this emulator holds, keyed by ARN.
    pub replication_tasks: BTreeMap<String, ReplicationTaskRecord>,
    /// `ReplicationTaskIdentifier` -> ARN, so a duplicate create is refused.
    pub task_arn_by_identifier: BTreeMap<String, String>,
    /// Bumped once per created resource, so a describe can scan in creation order.
    pub next_sequence: u64,
    /// Bumped once per minted resource id, so two ids minted in the same instant
    /// differ.
    pub id_counter: u64,
}

impl ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State {
    /// Materialise every status transition the clock has already made due.
    ///
    /// Called at the top of every handler, including the reporting ones: the change
    /// was owed at `now` whether or not anybody called, so applying it here is what
    /// makes a read and a later read agree.
    pub fn settle(&mut self, now: SystemTime) {
        for instance in self.replication_instances.values_mut() {
            if let Some(pending) = instance.pending {
                if now >= pending.due {
                    instance.pending = None;
                    if let Owed::ToStatus(next) = pending.owed {
                        instance.status = next.to_string();
                    }
                }
            }
        }

        // A task whose `deleting` span has elapsed leaves the store, and its
        // identifier becomes free again.
        let removed: Vec<(String, String)> = self
            .replication_tasks
            .values()
            .filter(|task| {
                task.pending
                    .is_some_and(|pending| now >= pending.due && pending.owed == Owed::Removal)
            })
            .map(|task| (task.arn.clone(), task.identifier.clone()))
            .collect();
        for (arn, identifier) in removed {
            self.replication_tasks.remove(&arn);
            self.task_arn_by_identifier.remove(&identifier);
        }

        for task in self.replication_tasks.values_mut() {
            if let Some(pending) = task.pending {
                if now >= pending.due {
                    task.pending = None;
                    if let Owed::ToStatus(next) = pending.owed {
                        task.status = next.to_string();
                    }
                }
            }
        }
    }

    /// The next creation sequence number.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.next_sequence
    }

    /// The replication instances this emulator holds, oldest first.
    pub fn ordered_instances(&self) -> Vec<&ReplicationInstanceRecord> {
        let mut records: Vec<&ReplicationInstanceRecord> =
            self.replication_instances.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// The endpoints this emulator holds, oldest first.
    pub fn ordered_endpoints(&self) -> Vec<&EndpointRecord> {
        let mut records: Vec<&EndpointRecord> = self.endpoints.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// The replication tasks this emulator holds, oldest first.
    pub fn ordered_tasks(&self) -> Vec<&ReplicationTaskRecord> {
        let mut records: Vec<&ReplicationTaskRecord> = self.replication_tasks.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// 32 bytes derived from `domain`, a per-store counter and the wall clock, so
    /// every minted resource id is distinct from every other.
    fn digest(&mut self, domain: &str) -> [u8; 32] {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        hasher.update(self.id_counter.to_be_bytes());
        hasher.update(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes(),
        );
        hasher.finalize().into()
    }

    /// A fresh resource id for the last field of a DMS ARN: uppercase letters and
    /// digits, beginning with a letter, inside the 31 characters the
    /// `ResourceIdentifier` doc allows.
    pub fn mint_resource_id(&mut self, domain: &str) -> String {
        let digest = self.digest(domain);
        let mut id: String = digest
            .iter()
            .take(MINTED_RESOURCE_ID_LEN)
            .map(|byte| char::from(RESOURCE_ID_ALPHABET[usize::from(*byte) % RESOURCE_ID_ALPHABET.len()]))
            .collect();
        // "can only begin with a letter": fold a leading digit onto a letter rather
        // than redrawing, so the id stays a pure function of the digest.
        if id.starts_with(|c: char| c.is_ascii_digit()) {
            let rest = id.split_off(1);
            id = format!("A{rest}");
        }
        id
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State>)
}

/// The region a request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A DMS ARN for `resource_type` (`rep`, `endpoint`, `task`) and `resource_id`.
///
/// Built through `emulator_server::emulator_arn`, which is the single source of the
/// canonical account id every emitted ARN carries.
pub fn dms_arn(region: &str, resource_type: &str, resource_id: &str) -> String {
    emulator_server::emulator_arn("dms", region, &format!("{resource_type}:{resource_id}"))
}

/// The Availability Zone a replication instance is placed in when the request names
/// none: the request's region plus a zone letter.
///
/// The doc says the default is "a random, system-chosen Availability Zone in the
/// endpoint's Amazon Web Services Region, for example: us-east-1d"; the emulator
/// models no EC2, so it picks one deterministically in the same form.
pub fn availability_zone_for(region: &str, index: usize) -> String {
    let letter = char::from(b'a' + (index % 26) as u8);
    format!("{region}{letter}")
}

/// Why an identifier was refused, as a sentence that names the member and the
/// constraint it broke.
///
/// The constraints are the ones the `ReplicationInstanceIdentifier`,
/// `EndpointIdentifier` and `ReplicationTaskIdentifier` docs publish, and the ones the
/// `ResourceIdentifier` doc publishes for the ARN suffix: ASCII letters, digits and
/// hyphens only, a letter first, no trailing hyphen, no two consecutive hyphens, and
/// (where the doc gives one) a maximum length.
pub fn identifier_refusal(member: &str, value: &str, max_len: Option<usize>) -> Option<String> {
    if value.is_empty() {
        return Some(format!("{member} must not be empty"));
    }
    if let Some(max_len) = max_len {
        if value.chars().count() > max_len {
            return Some(format!(
                "{member} must contain at most {max_len} characters, and {value} has {}",
                value.chars().count()
            ));
        }
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Some(format!(
            "{member} must contain only ASCII letters, digits and hyphens: {value}"
        ));
    }
    if !value.as_bytes()[0].is_ascii_alphabetic() {
        return Some(format!(
            "{member} must begin with a letter: {value}"
        ));
    }
    if value.ends_with('-') {
        return Some(format!("{member} must not end with a hyphen: {value}"));
    }
    if value.contains("--") {
        return Some(format!(
            "{member} must not contain two consecutive hyphens: {value}"
        ));
    }
    None
}

/// The request's tags, each carrying the ARN of the resource they were attached to,
/// which is what `Tag$ResourceArn` reports.
pub fn tags_for(arn: &str, tags: Option<Vec<Tag>>) -> Vec<Tag> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| Tag {
            resource_arn: Some(arn.to_string()),
            ..tag
        })
        .collect()
}

/// A `Marker` that resumes a paged describe at `arn`.
///
/// Hex-encoded rather than returned raw so a client cannot read a resource ARN out of
/// a pagination token, and so a marker this emulator did not mint is refused rather
/// than silently treated as an ARN.
pub fn encode_marker(arn: &str) -> String {
    hex::encode(arn)
}

/// The ARN a `Marker` resumes at, or `None` when the marker was not minted by
/// [`encode_marker`].
pub fn decode_marker(marker: &str) -> Option<String> {
    let bytes = hex::decode(marker).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    (!arn.is_empty()).then_some(arn)
}

/// The page size a paginated describe uses for `max_records`, or the reason the
/// request is refused.
///
/// The member doc publishes both the default and the range: "Default: 100" and
/// "Constraints: Minimum 20, maximum 100", which the model's plain `IntegerOptional`
/// does not express.
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    let requested = max_records.unwrap_or(DEFAULT_MAX_RECORDS);
    if !(MAX_RECORDS_MIN..=MAX_RECORDS_MAX).contains(&requested) {
        return Err(format!(
            "MaxRecords {requested} is outside the supported range \
             {MAX_RECORDS_MIN}-{MAX_RECORDS_MAX}"
        ));
    }
    Ok(usize::try_from(requested).unwrap_or(MAX_RECORDS_MAX as usize))
}

/// The span of `records` one page covers when resuming at `marker`, plus the marker
/// that resumes after it.
///
/// `key` names a record's pagination key (its ARN). The marker is "resume AT this
/// key", and the final page carries none, which is what the `@paginated` contract
/// means by an absent `outputToken`.
pub fn paginate<T>(
    records: &[T],
    marker: Option<&String>,
    size: usize,
    key: impl Fn(&T) -> &str,
) -> Result<(::std::ops::Range<usize>, Option<String>), String> {
    let start = match marker {
        Some(marker) => {
            let resume_at = decode_marker(marker)
                .ok_or_else(|| "Marker was not produced by a previous response".to_string())?;
            records
                .iter()
                .position(|record| key(record) == resume_at)
                .ok_or_else(|| {
                    "Marker resumes at a resource that is no longer described".to_string()
                })?
        }
        None => 0,
    };

    let end = records.len().min(start.saturating_add(size));
    let next = (end < records.len()).then(|| encode_marker(key(&records[end])));
    Ok((start..end, next))
}

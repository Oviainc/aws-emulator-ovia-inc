// SPDX-License-Identifier: CC0-1.0

//! The option-group option catalogue: which options each DB engine publishes, and the
//! settings each of those options carries.
//!
//! Reference data, like the engine-version catalogue in
//! `crate::ops::describe_db_engine_versions`: AWS publishes one option list per engine
//! and no request changes it, so this is the list this emulator publishes. It is NOT
//! the real service's live list, which moves with every RDS release, and it does not
//! distinguish major engine versions, which AWS additionally scopes availability by.
//! What a caller can rely on is that `ModifyOptionGroup` accepts exactly the options
//! the group's engine publishes here, that every setting an option publishes is
//! reported with its published default and metadata, and that a value the caller sets
//! replaces that default while `DefaultValue` keeps reporting it.
//!
//! The rows are drawn from the per-engine option tables of the Amazon RDS User Guide
//! (its "Options for ..." chapters — MariaDB Audit Plugin, memcached, Oracle native
//! network encryption, Oracle Enterprise Manager, Statspack, SQL Server native backup
//! and restore, SQL Server audit, Transparent Data Encryption), the same published
//! tables `DescribeOptionGroupOptions` reports from. The members each row fills come
//! from the model:
//! `smithy_query(query="shape",shape="Option,OptionSetting",model_path="services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json")`.

/// `OptionSetting$ApplyType` — "The DB engine specific parameter type." A setting RDS
/// can apply to a running instance.
const DYNAMIC: &str = "DYNAMIC";

/// `OptionSetting$ApplyType` for a setting that takes effect only on restart.
const STATIC: &str = "STATIC";

/// `OptionSetting$DataType` for a textual setting.
const STRING: &str = "STRING";

/// `OptionSetting$DataType` for a numeric setting.
const INTEGER: &str = "INTEGER";

/// A setting one option publishes: everything the modelled `OptionSetting` reports
/// about it except `Value`, which is the group's own configuration and lives in
/// [`crate::state::OptionRecord`].
#[derive(Debug)]
pub struct OptionSettingFacts {
    /// `OptionSetting$Name`.
    pub name: &'static str,
    /// `OptionSetting$Description`.
    pub description: &'static str,
    /// `OptionSetting$ApplyType`: [`DYNAMIC`] or [`STATIC`].
    pub apply_type: &'static str,
    /// `OptionSetting$DataType`: [`STRING`] or [`INTEGER`].
    pub data_type: &'static str,
    /// `OptionSetting$DefaultValue` — the value the setting reports until a
    /// `ModifyOptionGroup` call sets another.
    pub default_value: &'static str,
    /// `OptionSetting$AllowedValues`, in the form the engine publishes it: a
    /// comma-separated value list or a `low-high` range. EMPTY for a free-form
    /// setting (a role ARN, a user list), which is then reported with no
    /// `AllowedValues` member at all rather than with an empty one.
    pub allowed_values: &'static str,
    /// `OptionSetting$IsModifiable` — "Indicates whether the option setting can be
    /// modified from the default." A request that sets one of these is refused.
    pub is_modifiable: bool,
    /// `OptionSetting$IsCollection` — "Indicates whether the option setting is part of
    /// a collection", which is how a setting taking several values at once reports
    /// itself.
    pub is_collection: bool,
}

/// One setting row, so the tables below read as tables: a modifiable setting taking
/// ONE value.
///
/// Columns, in order: name, description, apply type, data type, default value, allowed
/// values. [`collection_setting`] and [`fixed_setting`] are the same row with
/// `IsCollection` and `IsModifiable` respectively flipped.
const fn setting(
    name: &'static str,
    description: &'static str,
    apply_type: &'static str,
    data_type: &'static str,
    default_value: &'static str,
    allowed_values: &'static str,
) -> OptionSettingFacts {
    OptionSettingFacts {
        name,
        description,
        apply_type,
        data_type,
        default_value,
        allowed_values,
        is_modifiable: true,
        is_collection: false,
    }
}

/// A modifiable setting that takes SEVERAL values at once — `IsCollection` true.
const fn collection_setting(
    name: &'static str,
    description: &'static str,
    apply_type: &'static str,
    data_type: &'static str,
    default_value: &'static str,
    allowed_values: &'static str,
) -> OptionSettingFacts {
    OptionSettingFacts {
        name,
        description,
        apply_type,
        data_type,
        default_value,
        allowed_values,
        is_modifiable: true,
        is_collection: true,
    }
}

/// A setting a request may NOT set — `IsModifiable` false, so it always reports its
/// published default.
const fn fixed_setting(
    name: &'static str,
    description: &'static str,
    apply_type: &'static str,
    data_type: &'static str,
    default_value: &'static str,
    allowed_values: &'static str,
) -> OptionSettingFacts {
    OptionSettingFacts {
        name,
        description,
        apply_type,
        data_type,
        default_value,
        allowed_values,
        is_modifiable: false,
        is_collection: false,
    }
}

/// An option a DB engine publishes: everything the modelled `Option` reports about it
/// except the group's own configuration of it.
#[derive(Debug)]
pub struct OptionFacts {
    /// The engines that publish this option — each an entry of
    /// [`crate::state::OPTION_GROUP_ENGINE_NAMES`].
    pub engines: &'static [&'static str],
    /// `Option$OptionName`, in the catalogue's own spelling, which is what a group
    /// reports however the request spelled it.
    pub name: &'static str,
    /// `Option$OptionDescription`.
    pub description: &'static str,
    /// `Option$Permanent` — a permanent option can't be removed from the group it was
    /// added to, which `ModifyOptionGroupMessage$OptionGroupName` documents in so many
    /// words ("Permanent options, such as the TDE option for Oracle Advanced Security
    /// TDE, can't be removed from an option group").
    pub permanent: bool,
    /// `Option$Persistent` — a persistent option stays with a DB instance across a
    /// restore.
    pub persistent: bool,
    /// `Option$Port`: "If required, the port configured for this option to use." The
    /// published default port of an option that listens, and `None` for one that does
    /// not.
    pub port: Option<i32>,
    /// The settings this option publishes, in the order it publishes them.
    pub settings: &'static [OptionSettingFacts],
}

impl OptionFacts {
    /// The setting of this option named `name`, matched case-insensitively like the
    /// option name itself.
    pub fn setting(&self, name: &str) -> Option<&'static OptionSettingFacts> {
        self.settings
            .iter()
            .find(|facts| facts.name.eq_ignore_ascii_case(name))
    }
}

/// One option row.
///
/// Columns, in order: the engines publishing it, name, description, permanent,
/// persistent, port, settings.
const fn option(
    engines: &'static [&'static str],
    name: &'static str,
    description: &'static str,
    permanent: bool,
    persistent: bool,
    port: Option<i32>,
    settings: &'static [OptionSettingFacts],
) -> OptionFacts {
    OptionFacts {
        engines,
        name,
        description,
        permanent,
        persistent,
        port,
        settings,
    }
}

/// The engines that publish the MariaDB Audit Plugin: RDS offers it for MariaDB and
/// for MySQL.
const MARIADB_AND_MYSQL: &[&str] = &["mariadb", "mysql"];

/// The Oracle engines of `CreateOptionGroupMessage$EngineName`.
const ORACLE_ENGINES: &[&str] = &["oracle-ee", "oracle-ee-cdb", "oracle-se2", "oracle-se2-cdb"];

/// The SQL Server engines that publish native backup and restore: every edition.
const SQLSERVER_ENGINES: &[&str] = &[
    "sqlserver-ee",
    "sqlserver-se",
    "sqlserver-ex",
    "sqlserver-web",
];

/// The SQL Server editions that publish server audit.
const SQLSERVER_FULL_EDITIONS: &[&str] = &["sqlserver-ee", "sqlserver-se"];

/// `MARIADB_AUDIT_PLUGIN`'s settings, as the MariaDB Audit Plugin option table
/// publishes them.
const MARIADB_AUDIT_PLUGIN_SETTINGS: &[OptionSettingFacts] = &[
    collection_setting(
        "SERVER_AUDIT_EVENTS",
        "The event classes recorded in the audit log.",
        DYNAMIC,
        STRING,
        "CONNECT,QUERY",
        "CONNECT,QUERY,QUERY_DDL,QUERY_DML,QUERY_DML_NO_SELECT,QUERY_DCL,TABLE",
    ),
    collection_setting(
        "SERVER_AUDIT_EXCL_USERS",
        "The users whose activity is not recorded in the audit log.",
        DYNAMIC,
        STRING,
        "",
        "",
    ),
    collection_setting(
        "SERVER_AUDIT_INCL_USERS",
        "The users whose activity is recorded in the audit log.",
        DYNAMIC,
        STRING,
        "",
        "",
    ),
    fixed_setting(
        "SERVER_AUDIT_FILE_PATH",
        "The path of the audit log file.",
        STATIC,
        STRING,
        "RDS_AUDIT",
        "",
    ),
    setting(
        "SERVER_AUDIT_FILE_ROTATE_SIZE",
        "The size in bytes at which an audit log file is rotated.",
        DYNAMIC,
        INTEGER,
        "1000000",
        "1-1000000000",
    ),
    setting(
        "SERVER_AUDIT_FILE_ROTATIONS",
        "The number of rotated audit log files kept.",
        DYNAMIC,
        INTEGER,
        "9",
        "0-100",
    ),
    fixed_setting(
        "SERVER_AUDIT_LOGGING",
        "Whether audit logging is enabled.",
        STATIC,
        STRING,
        "ON",
        "ON,OFF",
    ),
    setting(
        "SERVER_AUDIT_QUERY_LOG_LIMIT",
        "The number of characters of a query recorded in the audit log.",
        DYNAMIC,
        INTEGER,
        "1024",
        "0-2147483647",
    ),
];

/// `MEMCACHED`'s settings, as the memcached option table publishes them.
const MEMCACHED_SETTINGS: &[OptionSettingFacts] = &[
    setting(
        "DAEMON_MEMCACHED_R_BATCH_SIZE",
        "The number of memcached read operations before a commit.",
        STATIC,
        INTEGER,
        "1",
        "1-1073741824",
    ),
    setting(
        "DAEMON_MEMCACHED_W_BATCH_SIZE",
        "The number of memcached write operations before a commit.",
        STATIC,
        INTEGER,
        "1",
        "1-1073741824",
    ),
    setting(
        "DAEMON_MEMCACHED_OPTION",
        "The memcached daemon options passed on startup.",
        STATIC,
        STRING,
        "",
        "",
    ),
    setting(
        "INNODB_API_DISABLE_ROWLOCK",
        "Whether row locks are disabled for memcached operations.",
        STATIC,
        INTEGER,
        "0",
        "0,1",
    ),
    setting(
        "INNODB_API_ENABLE_MDL",
        "Whether metadata locking is enabled for memcached operations.",
        STATIC,
        INTEGER,
        "0",
        "0,1",
    ),
    setting(
        "INNODB_API_TRX_LEVEL",
        "The transaction isolation level of memcached operations.",
        STATIC,
        INTEGER,
        "0",
        "0-3",
    ),
];

/// `NATIVE_NETWORK_ENCRYPTION`'s settings — the option the modelled `OptionSetting`
/// shape cites itself ("the NATIVE_NETWORK_ENCRYPTION option has a setting called
/// SQLNET.ENCRYPTION_SERVER that can have several different values").
const NATIVE_NETWORK_ENCRYPTION_SETTINGS: &[OptionSettingFacts] = &[
    setting(
        "SQLNET.ENCRYPTION_SERVER",
        "Whether the server requires, requests, accepts or rejects encryption.",
        DYNAMIC,
        STRING,
        "REQUESTED",
        "ACCEPTED,REJECTED,REQUESTED,REQUIRED",
    ),
    collection_setting(
        "SQLNET.ENCRYPTION_TYPES_SERVER",
        "The encryption algorithms the server accepts.",
        DYNAMIC,
        STRING,
        "AES256,AES192,AES128",
        "AES256,AES192,AES128,3DES168,3DES112,DES,DES40,RC4_256,RC4_128,RC4_56,RC4_40",
    ),
    setting(
        "SQLNET.CRYPTO_CHECKSUM_SERVER",
        "Whether the server requires, requests, accepts or rejects data integrity \
         checksums.",
        DYNAMIC,
        STRING,
        "REQUESTED",
        "ACCEPTED,REJECTED,REQUESTED,REQUIRED",
    ),
    collection_setting(
        "SQLNET.CRYPTO_CHECKSUM_TYPES_SERVER",
        "The data integrity algorithms the server accepts.",
        DYNAMIC,
        STRING,
        "SHA1",
        "SHA1,MD5",
    ),
];

/// `SQLSERVER_AUDIT`'s settings, as the SQL Server audit option table publishes them.
const SQLSERVER_AUDIT_SETTINGS: &[OptionSettingFacts] = &[
    setting(
        "IAM_ROLE_ARN",
        "The ARN of the IAM role with write access to the S3 bucket audit logs are \
         uploaded to.",
        DYNAMIC,
        STRING,
        "",
        "",
    ),
    setting(
        "S3_BUCKET_ARN",
        "The ARN of the S3 bucket audit logs are uploaded to.",
        DYNAMIC,
        STRING,
        "",
        "",
    ),
    setting(
        "ENABLE_COMPRESSION",
        "Whether audit logs are compressed before upload.",
        DYNAMIC,
        STRING,
        "true",
        "true,false",
    ),
    setting(
        "RETENTION_TIME",
        "The number of hours audit logs are kept on the DB instance.",
        DYNAMIC,
        INTEGER,
        "0",
        "0-840",
    ),
];

/// `SQLSERVER_BACKUP_RESTORE`'s one setting, as the native backup and restore option
/// table publishes it.
const SQLSERVER_BACKUP_RESTORE_SETTINGS: &[OptionSettingFacts] = &[setting(
    "IAM_ROLE_ARN",
    "The ARN of the IAM role with access to the S3 bucket native backups are written \
     to and read from.",
    DYNAMIC,
    STRING,
    "",
    "",
)];

/// An option that publishes no settings of its own.
const NO_SETTINGS: &[OptionSettingFacts] = &[];

/// The option-group options this service publishes, in catalogue order.
pub const OPTIONS: &[OptionFacts] = &[
    option(
        MARIADB_AND_MYSQL,
        "MARIADB_AUDIT_PLUGIN",
        "MariaDB Audit Plugin",
        false,
        false,
        None,
        MARIADB_AUDIT_PLUGIN_SETTINGS,
    ),
    option(
        &["mysql"],
        "MEMCACHED",
        "Memcached interface to InnoDB",
        false,
        false,
        // The port memcached listens on, which is why this option reports one at all.
        Some(11211),
        MEMCACHED_SETTINGS,
    ),
    option(
        ORACLE_ENGINES,
        "NATIVE_NETWORK_ENCRYPTION",
        "Oracle Advanced Security - Native Network Encryption",
        false,
        false,
        None,
        NATIVE_NETWORK_ENCRYPTION_SETTINGS,
    ),
    option(
        ORACLE_ENGINES,
        "OEM",
        "Oracle Enterprise Manager Database Control",
        false,
        false,
        // The Database Control HTTPS console port.
        Some(5500),
        NO_SETTINGS,
    ),
    option(
        ORACLE_ENGINES,
        "STATSPACK",
        "Oracle Statspack",
        false,
        false,
        None,
        NO_SETTINGS,
    ),
    option(
        SQLSERVER_FULL_EDITIONS,
        "SQLSERVER_AUDIT",
        "SQL Server Audit",
        false,
        false,
        None,
        SQLSERVER_AUDIT_SETTINGS,
    ),
    option(
        SQLSERVER_ENGINES,
        "SQLSERVER_BACKUP_RESTORE",
        "SQL Server Native Backup and Restore",
        false,
        false,
        None,
        SQLSERVER_BACKUP_RESTORE_SETTINGS,
    ),
    option(
        &["sqlserver-ee"],
        "TDE",
        "SQL Server - Transparent Data Encryption",
        // The permanent and persistent option `ModifyOptionGroup` names: once added it
        // cannot be removed from the group.
        true,
        true,
        None,
        NO_SETTINGS,
    ),
];

/// The option `engine_name` publishes under `option_name`, if it publishes one.
///
/// The name is matched case-insensitively — an option name is a published identifier,
/// not caller data, so the catalogue's own spelling is what a group reports — and only
/// within the engine the group was created for: an option one engine publishes is not
/// an option another does.
pub fn option_facts(engine_name: &str, option_name: &str) -> Option<&'static OptionFacts> {
    OPTIONS.iter().find(|facts| {
        facts.name.eq_ignore_ascii_case(option_name)
            && facts
                .engines
                .iter()
                .any(|engine| engine.eq_ignore_ascii_case(engine_name))
    })
}

/// Every option name `engine_name` publishes, in catalogue order — what a refusal
/// names so the caller can fix the request from the message alone.
pub fn published_option_names(engine_name: &str) -> Vec<&'static str> {
    OPTIONS
        .iter()
        .filter(|facts| {
            facts
                .engines
                .iter()
                .any(|engine| engine.eq_ignore_ascii_case(engine_name))
        })
        .map(|facts| facts.name)
        .collect()
}

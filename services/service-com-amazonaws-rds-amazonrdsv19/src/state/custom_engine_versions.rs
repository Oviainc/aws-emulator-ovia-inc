// SPDX-License-Identifier: CC0-1.0

//! The custom DB engine version (CEV) records `CreateCustomDBEngineVersion` writes.
//!
//! A CEV is the one engine version that is ACCOUNT state rather than this service's
//! published reference data: it exists only because a request created it from
//! customer-supplied installation media or from an Amazon Machine Image, and it is
//! reported only to the account and region that created it
//! (`CreateCustomDBEngineVersion$EngineVersion`: "The CEV name is unique per customer per
//! Amazon Web Services Regions"). The catalogue of AWS-PROVIDED engine versions stays
//! where it is, in `crate::ops::describe_db_engine_versions`, and that module projects a
//! record of this kind onto the `DBEngineVersion` wire shape, so one CEV reads the same
//! through the create that minted it and through the report that lists it.
//!
//! Only the record lives here: everything derived from it — its major version, its
//! parameter group family and the capability members of its engine — is computed from the
//! engine-version catalogue rather than stored twice.

use super::ARN_NAMESPACE;
use crate::ops::types::Tag;

/// How many custom engine versions one region holds. The cap is this emulator's, for the
/// reason every other per-region quota in `src/state.rs` exists: it bounds the store and
/// every scan over it. Reaching it is what the modelled
/// `CustomDBEngineVersionQuotaExceededFault` reports.
pub const CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION: usize = 20;

/// The `Status` a created CEV reports: the first value
/// `ModifyCustomDBEngineVersion$Status` publishes ("`available` — You can use this CEV to
/// create a new RDS Custom DB instance"), and the status `DescribeDBEngineVersions` lists
/// an engine version under by default. A create answers with a CEV already in it, so this
/// service owes a new CEV no status transition; `ModifyCustomDBEngineVersion` is what moves
/// a CEV to another status afterwards ("you can change any status to any status").
pub const CUSTOM_ENGINE_VERSION_STATUS_AVAILABLE: &str = "available";

/// The engines a custom engine version can be created for, as
/// `CreateCustomDBEngineVersion$Engine` publishes them: the four RDS Custom for Oracle
/// engines, the four RDS Custom for SQL Server engines, and the RDS for SQL Server
/// Bring Your Own Media engines (`sqlserver-ee`, `sqlserver-se`) with
/// `sqlserver-dev-ee`. Alphabetical, so the refusal below lists them in one stable order.
pub const CUSTOM_ENGINE_VERSION_ENGINES: [&str; 11] = [
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
    "custom-sqlserver-dev",
    "custom-sqlserver-ee",
    "custom-sqlserver-se",
    "custom-sqlserver-web",
    "sqlserver-dev-ee",
    "sqlserver-ee",
    "sqlserver-se",
];

/// The ARN resource type of a custom engine version.
const ARN_RESOURCE_TYPE: &str = "cev";

/// Refuse an `Engine` no custom engine version can be created for.
///
/// The member's pattern admits any name at all, so the SET of engines is checked here: a
/// value outside it names no engine whose versions an account mints, which is what the
/// Query API's `InvalidParameterValue` reports.
pub fn check_custom_engine(engine: &str) -> Result<(), String> {
    if CUSTOM_ENGINE_VERSION_ENGINES.contains(&engine) {
        return Ok(());
    }
    Err(format!(
        "invalid value '{engine}' for Engine: a custom DB engine version can be created \
         for {}",
        CUSTOM_ENGINE_VERSION_ENGINES.join(", ")
    ))
}

/// Whether a CEV of `engine` is created from an Amazon Machine Image rather than from
/// installation files: RDS Custom for SQL Server, whose `ImageId` documentation states
/// that "for RDS Custom for SQL Server, an AMI ID is required to create a CEV".
pub fn is_ami_backed_custom_engine(engine: &str) -> bool {
    engine.starts_with("custom-sqlserver")
}

/// Whether `engine` reports a `DBEngineMediaType`.
///
/// The member is "applicable to RDS Custom for SQL Server and to RDS for SQL Server engine
/// versions (`sqlserver-ee` and `sqlserver-se` with the bring-your-own-media license
/// model, and `sqlserver-dev-ee`)" — the SQL Server engines, and no other.
pub fn reports_engine_media_type(engine: &str) -> bool {
    engine.contains("sqlserver")
}

/// The `DBEngineMediaType` of a custom engine version: "a value of `Customer Provided`
/// indicates that the engine version was created from customer-supplied installation media
/// using `CreateCustomDBEngineVersion`" — which is what every record here was.
pub const MEDIA_TYPE_CUSTOMER_PROVIDED: &str = "Customer Provided";

/// The second half of a CEV's store key: one `(Engine, EngineVersion)` pair names one
/// custom engine version in a region, since the name "is unique per customer per Amazon
/// Web Services Regions" and two engines can carry the same version name.
pub fn custom_db_engine_version_key(engine: &str, engine_version: &str) -> String {
    format!("{engine}/{engine_version}")
}

/// A persisted custom DB engine version.
#[derive(Debug, Clone)]
pub struct CustomDbEngineVersionRecord {
    /// `Engine`, folded to the lower case the published engine names are written in, and
    /// half this record's key.
    pub engine: String,
    /// `EngineVersion` — the CEV name, the other half of the key.
    pub engine_version: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The minted, immutable id the CEV's ARN carries.
    pub cev_id: String,
    /// `MajorEngineVersion`, resolved from the CEV name when the record was written: the
    /// core attributes it is derived from cannot change, so it is resolved once.
    pub major_engine_version: String,
    /// `DBParameterGroupFamily`, resolved beside the major version.
    pub parameter_group_family: String,
    /// `Description` — reported as `DBEngineVersionDescription`.
    pub description: Option<String>,
    /// `Manifest` — reported as `CustomDBEngineVersionManifest`.
    pub manifest: Option<String>,
    /// `ImageId` — reported inside `Image`.
    pub image_id: Option<String>,
    /// `KMSKeyId`.
    pub kms_key_id: Option<String>,
    /// `DatabaseInstallationFilesS3BucketName`.
    pub database_installation_files_s3_bucket_name: Option<String>,
    /// `DatabaseInstallationFilesS3Prefix`.
    pub database_installation_files_s3_prefix: Option<String>,
    /// `DatabaseInstallationFiles`.
    pub database_installation_files: Vec<String>,
    /// `Status`.
    pub status: String,
    /// `CreateTime`.
    pub create_time: ::aws_smithy_types::DateTime,
    /// `Tags`, reported as `TagList`.
    pub tags: Vec<Tag>,
}

impl CustomDbEngineVersionRecord {
    /// This record's store key within its region.
    pub fn key(&self) -> String {
        custom_db_engine_version_key(&self.engine, &self.engine_version)
    }

    /// `DBEngineVersionArn` — "the ARN of the custom engine version": the engine and the
    /// CEV name it is known by, plus the minted id that makes one ARN name one record even
    /// after a CEV of the same name has been deleted and created again.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!(
                "{ARN_RESOURCE_TYPE}:{}/{}/{}",
                self.engine, self.engine_version, self.cev_id
            ),
        )
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-glue-awsglue service.
//!
//! The Data Catalog is the service's primary resource tree: a catalog (keyed by
//! `CatalogId`, which defaults to the request's account id) holds databases, and a
//! database holds tables. Every handler reads and writes THIS store, so a
//! `CreateTable` in one request is visible to a `GetTable` in the next. A stored table
//! carries its own version history beside its live definition
//! ([`TableEntry::archived_versions`]), so a read of one version serves the definition
//! that version was written with and not what a later write replaced it by. A table also
//! holds its PARTITIONS ([`TableEntry::partitions`]), the slices of its data a partition
//! create registers, because a partition is addressed by its values within one table.
//!
//! Beside the catalog the store holds the two account-level resources an ETL client
//! registers: JOB DEFINITIONS (`CreateJob` writes one, `GetJob` reads it back) and, inside
//! each catalog, CONNECTION definitions (`CreateConnection` / `GetConnection`). A job is
//! named per account ("The name you assign to this job definition. It must be unique in
//! your account" — `CreateJobRequest$Name`) and a connection per catalog, which is where
//! each lives here; because Glue is a regional service, the job namespace is keyed by the
//! REGION the create was served in as well, so a job is reachable only through the
//! endpoint that created it. A job definition also holds the RUNS `StartJobRun` started
//! of it ([`JobRunEntry`]), because every operation that addresses a run names its job too.
//!
//! A catalog also holds ONE security configuration of its own
//! ([`Catalog::encryption_settings`]): the encryption settings a write set for it, which
//! every later read of that catalog's settings reports. What that setting does to the
//! catalog's OBJECTS is recorded on each object: a database, table or partition written
//! while the catalog encrypts its writes under a KMS key carries the key material that
//! write was encrypted under ([`catalog_encryption::ObjectEncryption`]), because the
//! setting applies "to every catalog write thereafter" and not to what is already stored.
//!
//! Crawlers sit BESIDE that tree too, in one per-account namespace keyed by crawler name:
//! every operation that addresses a crawler does so by name alone, so a crawler's
//! `CatalogId` says where its output goes, not where the crawler itself lives.
//!
//! COLUMN STATISTICS TASK RUNS — the runs `StartColumnStatisticsTaskRun` starts over one
//! table — sit beside the tree in the same way, in a per-account list
//! ([`ComAmazonawsGlueAwsglueState::column_statistics_task_runs`]): a run is ADDRESSED by
//! the id the start mints (`GetColumnStatisticsTaskRun` takes that id and nothing else),
//! so the table a run covers is recorded on the run rather than the run on the table.
//!
//! The SETTINGS those runs are scheduled by (`CreateColumnStatisticsTaskSettings`) sit the
//! other way round — ON the table, like its partitions
//! ([`TableEntry::column_statistics_task_settings`]) — because every operation that
//! addresses one settings record names its catalog, its database and then its table and
//! nothing else, so there is at most one of them per table.
//!
//! The SCHEMA REGISTRY is the store's second resource tree and is independent of the
//! catalog: a registry (keyed by name, per account) holds schema sets, and a schema set
//! holds the versions registered against it. It is keyed by name rather than by
//! `CatalogId` because every schema-registry operation addresses its resource by a
//! `RegistryId`/`SchemaId` wrapper carrying a name or an ARN and no catalog id at all.
//!
//! CUSTOM ENTITY TYPES — the custom sensitive-data patterns `CreateCustomEntityType`
//! registers — sit in the same kind of per-account namespace, keyed by pattern name:
//! "This name must be unique per Amazon Web Services account"
//! (`CustomEntityType$Name`), and every operation that addresses one names it by name
//! alone, with no `CatalogId` member.
//!
//! SECURITY CONFIGURATIONS — the sets of encryption properties
//! `CreateSecurityConfiguration` registers — sit in such a per-account namespace too
//! ([`ComAmazonawsGlueAwsglueState::security_configurations`]), keyed by configuration
//! name, because every operation that addresses one (`GetSecurityConfiguration`,
//! `DeleteSecurityConfiguration`) names it by `Name` alone and no `CatalogId`. This is
//! a DIFFERENT resource from [`Catalog::encryption_settings`] above: those settings are
//! the catalog's own, while a security configuration is a named record a job, crawler or
//! development endpoint refers to.
//!
//! FORM TYPES — the metadata schemas `PutFormType` registers — sit in such a per-account
//! namespace too, keyed by form type name, because a put names its form type by `Name`
//! alone and no `CatalogId`. They are ADDRESSED by the identifier a client holds for one —
//! the opaque `FormTypeId` the first put under a name mints, or that name itself — which is
//! the shape every operation that reads, deletes or attaches to one takes.
//!
//! ASSET TYPES — the asset structures `PutAssetType` registers, each naming the forms an
//! asset of that type includes — sit in their own per-account namespace beside the form
//! types they reference ([`ComAmazonawsGlueAwsglueState::asset_types`]), keyed by asset
//! type name for the same reason: a put names its asset type by `Name` alone and no
//! `CatalogId`. Each of its form references carries the `FormTypeId` of a form type in
//! this same store, and the asset type is itself ADDRESSED by the opaque `AssetTypeId` the
//! first put under a name mints.
//!
//! BUSINESS GLOSSARIES — the containers for business-concept terms `CreateGlossary`
//! registers — sit beside that tree in another per-account namespace
//! ([`ComAmazonawsGlueAwsglueState::glossaries`]). They are keyed by the opaque
//! `GlossaryId` the create mints, because every operation that addresses a glossary takes
//! an `Identifier` targeting that shape and no `CatalogId`; the NAME a create carries is a
//! separate uniqueness constraint on the namespace ([`ComAmazonawsGlueAwsglueState::glossary_by_name`]),
//! which is what `CreateGlossary`'s modelled `AlreadyExistsException` refuses a second
//! create under.
//!
//! GLOSSARY TERMS — the business concepts `CreateGlossaryTerm` registers within a glossary
//! — sit BESIDE the glossaries in their own per-account namespace
//! ([`ComAmazonawsGlueAwsglueState::glossary_terms`]) keyed by the opaque `GlossaryTermId`
//! the create mints, because every operation that reads, updates or deletes a term
//! addresses it by that id alone and names no glossary; the glossary a term belongs to is
//! recorded ON the term ([`GlossaryTermEntry::glossary_id`]), and the term's NAME is a
//! uniqueness constraint within that one glossary
//! ([`ComAmazonawsGlueAwsglueState::glossary_term_by_name`]), which is what
//! `CreateGlossaryTerm`'s modelled `AlreadyExistsException` refuses a second create under.
//!
//! RESOURCE POLICIES — the IAM policy documents `PutResourcePolicy` attaches to a Glue
//! resource — sit beside the catalog tree in one per-account namespace keyed by the
//! resource's ARN ([`ComAmazonawsGlueAwsglueState::resource_policies`]). The ARN is the
//! key because every operation that addresses a policy addresses it by `ResourceArn`
//! alone, with no `CatalogId` member, and a request that names none addresses the Data
//! Catalog's own policy ("If not supplied, the Data Catalog resource policy is returned"
//! — `GetResourcePolicyRequest$ResourceArn`), which is therefore keyed by the Data
//! Catalog's own ARN ([`catalog_arn`]).
//!
//! WORKFLOWS — the containers for the dependent jobs and crawlers `CreateWorkflow`
//! registers — sit beside that tree in a per-account namespace keyed by workflow name
//! ([`ComAmazonawsGlueAwsglueState::workflows`]): "The name to be assigned to the
//! workflow. It should be unique within your account" (`CreateWorkflowRequest$Name`), and
//! every operation that addresses a workflow (`GetWorkflow`, `UpdateWorkflow`,
//! `DeleteWorkflow`, `StartWorkflowRun`) names it by `Name` alone, with no `CatalogId`
//! member. A workflow holds the RUNS `StartWorkflowRun` started of it
//! ([`WorkflowEntry::runs`]), because every operation that addresses a run names its
//! workflow too (`GetWorkflowRun` takes a `Name` beside the `RunId`).
//!
//! DATA QUALITY RULE RECOMMENDATION RUNS — the runs `StartDataQualityRuleRecommendationRun`
//! starts over one data source (a catalog table) — sit beside the catalog tree in the same
//! per-account list as the column statistics task runs
//! ([`ComAmazonawsGlueAwsglueState::data_quality_rule_recommendation_runs`]): a run is
//! ADDRESSED by the `RunId` the start mints, which
//! `GetDataQualityRuleRecommendationRun` takes and nothing else, so the data source a run
//! covers is recorded on the run rather than the run on the table.
//!
//! USAGE PROFILES — the job and session parameter sets an admin registers with
//! `CreateUsageProfile` — sit beside the catalog tree in another per-account namespace
//! keyed by profile name ([`ComAmazonawsGlueAwsglueState::usage_profiles`]), because every
//! operation that addresses a profile names it by `Name` alone and carries no `CatalogId`.
//!
//! TRIGGERS — the records `CreateTrigger` registers, each naming the jobs and crawlers it
//! initiates — sit beside the catalog tree too
//! ([`ComAmazonawsGlueAwsglueState::triggers`]), keyed by the REGION they were created in
//! and then by trigger name: every operation that addresses a trigger names it by `Name`
//! alone with no `CatalogId`, and the jobs a trigger's actions run are themselves reachable
//! only through the Region that created them, so a trigger is served by the endpoint that
//! created it exactly as a job definition is.
//!
//! MACHINE LEARNING TRANSFORMS — the transforms `CreateMLTransform` registers — sit beside
//! that tree in a per-account namespace keyed by the opaque `TransformId` the create mints
//! ([`ComAmazonawsGlueAwsglueState::ml_transforms`]), because every operation that
//! addresses one takes that id alone and no `CatalogId`, and a transform's NAME is no
//! address at all: "Names are not guaranteed unique and can be changed at any time"
//! (`MLTransform$Name`). The catalog TABLES a transform reads are recorded on the
//! transform as the create named them ([`MlTransformEntry::input_record_tables`]), which is
//! where its reported schema is taken from.
//!
//! Database and table names are folded to lowercase on the way in — the model says
//! so for both `DatabaseInput$Name` ("For Hive compatibility, this is folded to
//! lowercase when it is stored") and `TableInput$Name` — and every lookup folds the
//! requested name the same way, so the fold cannot make a resource unreachable. A JOB,
//! CONNECTION or CRAWLER name is NOT folded: the model documents the fold for the two
//! Hive-compatible catalog resources only.

pub mod catalog_encryption;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use self::catalog_encryption::ObjectEncryption;

use crate::ops::types::{
    Action, AssetTypeFormReference,
    CodeGenConfigurationNode, Column, ColumnStatisticsTaskRun, ColumnStatisticsTaskSettings,
    ConfigurationObject,
    Connection, ConnectionInput,
    ConnectionPasswordEncryption, ConnectionsList, Crawler, CrawlerTargets,
    CreateColumnStatisticsTaskSettingsInput, CreateCrawlerInput,
    CreateCustomEntityTypeInput, CreateGlossaryInput, CreateGlossaryTermInput, CreateJobInput,
    CreateMLTransformInput,
    CreateSecurityConfigurationInput, CreateTriggerInput, CreateWorkflowInput, CustomEntityType,
    DataCatalogEncryptionSettings, DataLakePrincipal,
    DataQualityRuleRecommendationRunAdditionalRunOptions,
    DataSource, Database, DatabaseIdentifier, DatabaseInput,
    Edge, EncryptionAtRest, EncryptionConfiguration, ErrorDetail, EventBatchingCondition,
    ExecutionAttempt, ExecutionProperty, FederatedDatabase,
    FederatedTable, GlueTable, Job, MLTransform,
    CreateUsageProfileInput,
    JobCommand, JobRun, JobUpdate, LakeFormationConfiguration, LineageConfiguration,
    Node, NotificationProperty, Partition, PartitionInput, Predicate, PrincipalPermissions,
    ProfileConfiguration,
    RLSProperties,
    RecrawlPolicy, RegistryId, Schedule,
    SchemaChangePolicy, SchemaColumn, SchemaId, SecurityConfiguration, SourceControlDetails,
    StartColumnStatisticsTaskRunInput,
    StartDataQualityRuleRecommendationRunInput,
    StartJobRunInput, StartWorkflowRunInput, StorageDescriptor, Table, TableIdentifier, TableInput,
    TableVersion,
    TransformEncryption, TransformFilterCriteria, TransformParameters, TransformSortCriteria,
    Trigger, TriggerNodeDetails,
    UpdateCrawlerInput, Workflow, WorkflowGraph, WorkflowRun, WorkflowRunStatistics,
};

/// Ceiling on the databases one catalog may hold.
///
/// This emulator's own capacity, NOT an AWS quota: the store is resident memory and a
/// create is reachable by any caller, so the number of records one process will hold
/// is bounded here. A create past the ceiling is refused with the operation's modelled
/// `ResourceNumberLimitExceededException` rather than inserted.
pub const MAX_DATABASES_PER_CATALOG: usize = 1_000;

/// Ceiling on the tables one database may hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`].
pub const MAX_TABLES_PER_DATABASE: usize = 1_000;

/// Ceiling on the tables the whole store may hold, across every catalog and database.
/// Bounds the product of the two per-container ceilings, which on its own would still
/// let a caller grow the store far past what one process should hold.
pub const MAX_STORED_TABLES: usize = 10_000;

/// Ceiling on the partitions ONE table may hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: a partition create is reachable by any caller, so the
/// number of partition records one process will hold is bounded here. A create past the
/// ceiling is refused — per item, for a batch create — rather than inserted.
pub const MAX_PARTITIONS_PER_TABLE: usize = 1_000;

/// Ceiling on the partitions the whole store may hold, across every catalog, database
/// and table. Bounds the PRODUCT of [`MAX_PARTITIONS_PER_TABLE`] and
/// [`MAX_STORED_TABLES`], which on its own would still let a caller grow the store far
/// past what one process should hold.
pub const MAX_STORED_PARTITIONS: usize = 100_000;

/// Ceiling on the versions of ONE table the store keeps archived, for the same reason
/// as [`MAX_DATABASES_PER_CATALOG`]: every update archives the definition it replaces,
/// and the update is reachable by any caller, so the number of archived definitions one
/// process will hold is bounded here. An update that would archive past the ceiling is
/// refused with the operation's modelled `ResourceNumberLimitExceededException` rather
/// than applied.
pub const MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE: usize = 1_000;

/// Ceiling on the archived table versions the whole store may hold, across every
/// catalog, database and table. Bounds the PRODUCT of
/// [`MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE`] and [`MAX_STORED_TABLES`], which on its own
/// would still let a caller grow the store far past what one process should hold.
pub const MAX_STORED_ARCHIVED_TABLE_VERSIONS: usize = 100_000;

/// Ceiling on the connection definitions one catalog may hold, for the same reason as
/// [`MAX_JOBS`].
pub const MAX_CONNECTIONS_PER_CATALOG: usize = 1_000;

/// Ceiling on the CATALOGS the whole store may hold.
///
/// The ceilings above bound what one container holds; this one bounds how many
/// containers there are, which is what makes the store as a whole bounded. The map is
/// keyed by `CatalogId`, a member a request may name, so without this a create that
/// reached a catalog the store does not hold yet could add containers without end — each
/// able to hold its own [`MAX_DATABASES_PER_CATALOG`] and
/// [`MAX_CONNECTIONS_PER_CATALOG`] records. It is also what keeps the store-wide walks
/// below ([`ComAmazonawsGlueAwsglueState::stored_tables`] and its siblings) a bounded
/// cost per create. A create that would add one past the ceiling is refused with the
/// operation's modelled `ResourceNumberLimitExceededException`.
pub const MAX_CATALOGS: usize = 16;

/// Ceiling on the databases the whole store may hold, across every catalog, for the same
/// reason as [`MAX_STORED_TABLES`]: the per-catalog ceiling alone, multiplied by
/// [`MAX_CATALOGS`], would still let a caller grow the store past what one process
/// should hold.
pub const MAX_STORED_DATABASES: usize = 10_000;

/// Ceiling on the connection definitions the whole store may hold, across every catalog,
/// for the same reason as [`MAX_STORED_DATABASES`].
pub const MAX_STORED_CONNECTIONS: usize = 10_000;

/// The `ConnectionProperties` key holding a connection's password: "`PASSWORD` - A
/// password, if one is used, for the user name." (`Connection$ConnectionProperties`.) It is
/// what `GetConnection`'s `HidePassword` withholds.
pub const PASSWORD_PROPERTY: &str = "PASSWORD";

/// The `ConnectionProperties` key holding the encrypted form of that password: "
/// `ENCRYPTED_PASSWORD` - When you enable connection password protection … this field
/// stores the encrypted password." Withheld with [`PASSWORD_PROPERTY`].
pub const ENCRYPTED_PASSWORD_PROPERTY: &str = "ENCRYPTED_PASSWORD";

/// `Connection$ConnectionSchemaVersion` for a connection whose properties are the version 1
/// set, and for one carrying the per-compute-environment properties: "The version of the
/// connection schema for this connection. Version 2 supports properties for specific
/// compute environments."
pub const CONNECTION_SCHEMA_VERSION_V1: i32 = 1;
/// See [`CONNECTION_SCHEMA_VERSION_V1`].
pub const CONNECTION_SCHEMA_VERSION_V2: i32 = 2;

/// The three values the model's `ComputeEnvironment` enum carries, which
/// `GetConnection$ApplyOverrideForComputeEnvironment` names one of.
pub const COMPUTE_ENVIRONMENT_SPARK: &str = "SPARK";
/// See [`COMPUTE_ENVIRONMENT_SPARK`].
pub const COMPUTE_ENVIRONMENT_ATHENA: &str = "ATHENA";
/// See [`COMPUTE_ENVIRONMENT_SPARK`].
pub const COMPUTE_ENVIRONMENT_PYTHON: &str = "PYTHON";

/// Ceiling on the crawlers this emulator will hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: resident memory bounded against a create any caller
/// can reach. A create past it is refused with `CreateCrawler`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_CRAWLERS: usize = 1_000;

/// Ceiling on the job definitions this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateJob`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_JOBS: usize = 1_000;

/// Ceiling on the triggers this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateTrigger`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_TRIGGERS: usize = 1_000;

/// Ceiling on the custom entity types this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateCustomEntityType`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_CUSTOM_ENTITY_TYPES: usize = 1_000;

/// Ceiling on the security configurations this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateSecurityConfiguration`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_SECURITY_CONFIGURATIONS: usize = 1_000;

/// Ceiling on the form types this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `PutFormType` writes a record any caller can reach, and each one
/// carries a schema of up to the modelled 100000 characters, so the number of records one
/// process will hold is bounded here. A put that would add one past the ceiling is
/// refused rather than stored; `PutFormType` models no limit error, so the refusal is
/// the closest client error it does model (`InvalidInputException`).
pub const MAX_FORM_TYPES: usize = 1_000;

/// Ceiling on the asset types this emulator will hold, for the same reason as
/// [`MAX_FORM_TYPES`]: `PutAssetType` writes a record any caller can reach, each one
/// carrying up to the modelled 100 form references, so the number of records one process
/// will hold is bounded here. A put that would add one past the ceiling is refused rather
/// than stored; `PutAssetType` models no limit error, so the refusal is the closest client
/// error it does model (`InvalidInputException`).
pub const MAX_ASSET_TYPES: usize = 1_000;

/// Ceiling on the business glossaries this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateGlossary` writes a record any caller can reach, so the number
/// of records one process will hold is bounded here. A create past the ceiling is refused
/// rather than stored; `CreateGlossary` models no limit error, so the refusal is the
/// closest client error it does model (`InvalidInputException`).
pub const MAX_GLOSSARIES: usize = 1_000;

/// Ceiling on the workflows this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateWorkflow` writes a record any caller can reach, so the number
/// of records one process will hold is bounded here. A create past the ceiling is refused
/// with `CreateWorkflow`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_WORKFLOWS: usize = 1_000;

/// Ceiling on the glossary terms this emulator will hold, for the same reason as
/// [`MAX_GLOSSARIES`]: `CreateGlossaryTerm` writes a record any caller can reach, each one
/// carrying descriptions of up to the modelled 1024 and 4096 characters, so the number of
/// records one process will hold is bounded here. A create past the ceiling is refused
/// rather than stored; `CreateGlossaryTerm` models no limit error, so the refusal is the
/// closest client error it does model (`InvalidInputException`).
pub const MAX_GLOSSARY_TERMS: usize = 1_000;

/// Ceiling on the usage profiles this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateUsageProfile` writes a record any caller can reach, each one
/// carrying two configuration maps, so the number of records one process will hold is
/// bounded here. A create past the ceiling is refused with `CreateUsageProfile`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_USAGE_PROFILES: usize = 1_000;

/// Ceiling on the machine learning transforms this emulator will hold, for the same reason
/// as [`MAX_CRAWLERS`]: `CreateMLTransform` writes a record any caller can reach, each one
/// carrying its input tables, parameters and schema, so the number of records one process
/// will hold is bounded here. A create past the ceiling is refused with
/// `CreateMLTransform`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_ML_TRANSFORMS: usize = 1_000;

/// Ceiling on the resource policies this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `PutResourcePolicy` writes a record any caller can reach, and each
/// one carries a policy document, so the number of records one process will hold is
/// bounded here. A write that would add one past the ceiling is refused rather than
/// stored; `PutResourcePolicy` models no limit error, so the refusal is the closest
/// client error it does model (`InvalidInputException`).
pub const MAX_RESOURCE_POLICIES: usize = 1_000;

/// The `ExistCondition` a `PutResourcePolicy` carries to require that a policy is already
/// attached ("A value of MUST_EXIST is used to update a policy"). One of the modelled
/// `ExistCondition` values, whose published list the generated
/// [`crate::ops::types::EXIST_CONDITION_VALUES`] carries (`MUST_EXIST`, `NONE`,
/// `NOT_EXIST`) and the request layer validates against.
pub const EXIST_CONDITION_MUST_EXIST: &str = "MUST_EXIST";

/// The `ExistCondition` that requires that NO policy is attached yet ("A value of
/// NOT_EXIST is used to create a new policy"). See [`EXIST_CONDITION_MUST_EXIST`] for the
/// published list; the remaining value (`NONE`) states no dependency on an existing
/// policy at all, which is also what an absent member means ("If a value of NONE or a
/// null value is used, the call does not depend on the existence of a policy").
pub const EXIST_CONDITION_NOT_EXIST: &str = "NOT_EXIST";

/// The `CrawlerState` of a crawler with no run in progress — the state a crawler is in
/// the moment `CreateCrawler` returns, and the only one this emulator stores until
/// `StartCrawler`/`StopCrawler` are implemented. One of the modelled `CrawlerState`
/// values (`READY`, `RUNNING`, `STOPPING`).
pub const CRAWLER_STATE_READY: &str = "READY";

/// The `CrawlerState` of a crawler whose crawl is in progress — "indicates whether the
/// crawler is running, or whether a run is pending" (`Crawler$State`). A write that
/// changes the crawler's definition is refused while it is in this state.
pub const CRAWLER_STATE_RUNNING: &str = "RUNNING";

/// The `CrawlerState` of a crawler whose crawl has been asked to stop and has not
/// finished: a run is still in progress, so a definition write is refused here too.
pub const CRAWLER_STATE_STOPPING: &str = "STOPPING";

/// The `Schedule$State` reported for a crawler that carries a cron expression. One of
/// the modelled `ScheduleState` values (`SCHEDULED`, `NOT_SCHEDULED`,
/// `TRANSITIONING`); a crawler created WITHOUT a `Schedule` has no schedule object at
/// all, so `NOT_SCHEDULED` is not reachable from a create alone.
pub const SCHEDULE_STATE_SCHEDULED: &str = "SCHEDULED";

/// The `TriggerState` of a trigger that is registered but not started: the state a
/// trigger is in the moment `CreateTrigger` returns when the create did not ask for it to
/// be started. One of the modelled `TriggerState` values, whose published list the
/// generated [`crate::ops::types::TRIGGER_STATE_VALUES`] carries.
///
/// A create answers with the trigger in this STEADY state rather than in the enum's
/// `CREATING`: the create is synchronous — it registers a definition and starts nothing —
/// so the clock owes a freshly created trigger no further transition.
pub const TRIGGER_STATE_CREATED: &str = "CREATED";

/// The `TriggerState` of a trigger that is started and waiting to fire: what
/// `CreateTrigger` leaves a trigger in when the request set `StartOnCreation` ("Set to
/// true to start SCHEDULED and CONDITIONAL triggers when created").
pub const TRIGGER_STATE_ACTIVATED: &str = "ACTIVATED";

/// The `TriggerType` of a trigger that fires only when a caller starts it: the one type
/// `StartOnCreation` cannot be set for ("True is not supported for ON_DEMAND triggers" —
/// `CreateTriggerRequest$StartOnCreation`). One of the modelled `TriggerType` values,
/// whose published list the generated [`crate::ops::types::TRIGGER_TYPE_VALUES`] carries
/// (`CONDITIONAL`, `EVENT`, `ON_DEMAND`, `SCHEDULED`).
pub const TRIGGER_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// The `TriggerType` of a trigger that fires on a cron schedule, for which the request's
/// `Schedule` is required ("This field is required when the trigger type is SCHEDULED" —
/// `CreateTriggerRequest$Schedule`).
pub const TRIGGER_TYPE_SCHEDULED: &str = "SCHEDULED";

/// The `TriggerType` of a trigger that fires when its predicate is satisfied, for which
/// the request's `Predicate` is required ("This field is required when the trigger type is
/// CONDITIONAL" — `CreateTriggerRequest$Predicate`).
pub const TRIGGER_TYPE_CONDITIONAL: &str = "CONDITIONAL";

/// The `NodeType` of a workflow-graph node that stands for a trigger. One of the modelled
/// `NodeType` values, whose published list the generated
/// [`crate::ops::types::NODE_TYPE_VALUES`] carries (`CRAWLER`, `JOB`, `TRIGGER`).
pub const NODE_TYPE_TRIGGER: &str = "TRIGGER";

/// The `NodeType` of a node that stands for a job definition. See [`NODE_TYPE_TRIGGER`].
pub const NODE_TYPE_JOB: &str = "JOB";

/// The `NodeType` of a node that stands for a crawler. See [`NODE_TYPE_TRIGGER`].
pub const NODE_TYPE_CRAWLER: &str = "CRAWLER";

/// The widest `NameString` the model accepts, which `Node$Name` and `Node$UniqueId` are
/// both of: "length 1..255" (`smithy_query shape=Node`).
const MAX_NAME_STRING_CHARS: usize = 255;

/// The `Version` a crawler carries straight after `CreateCrawler`: the first version of
/// a crawler is 1, and an update moves it on. The model's `@default` of 0 on
/// `Crawler$Version` is what an ABSENT long deserializes to, not the version of a stored
/// crawler. (A table's first version is numbered differently — see
/// [`FIRST_TABLE_VERSION_ID`] — because the catalog's table version history starts at
/// 0.)
pub const FIRST_CRAWLER_VERSION: i64 = 1;

/// The Lake Formation principal that stands for "whoever IAM already allows": the
/// `DataLakePrincipalIdentifier` of the grant a Data Catalog database carries while
/// nothing has narrowed it to Lake Formation permissions. Spelled exactly like this on
/// the wire, and the same principal
/// `services/service-com-amazonaws-lakeformation-awslakeformation/src/state.rs` holds the
/// matching database grant under.
pub const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The permission that default grant carries.
///
/// From the documented Lake Formation default, not chosen: Lake Formation's
/// `DataLakeSettings$CreateDatabaseDefaultPermissions` documents that "A value that
/// assigns ALL to IAM_ALLOWED_PRINCIPALS indicates access control by IAM permissions.
/// This is referred to as the setting \"Use only IAM access control\", and is for
/// backward compatibility with the Glue permission model", and that "The only permitted
/// values are an empty array or an array that contains a single JSON object that grants
/// ALL to IAM_ALLOWED_PRINCIPALS". `ALL` is also one of the modelled `Permission` enum's
/// values (`ALL`, `SELECT`, `ALTER`, `DROP`, `DELETE`, `INSERT`, `CREATE_DATABASE`,
/// `CREATE_TABLE`, `DATA_LOCATION_ACCESS`).
pub const PERMISSION_ALL: &str = "ALL";

/// The `CreateTableDefaultPermissions` a database gets when its create named none.
///
/// `Database$CreateTableDefaultPermissions` "creates a set of default permissions on the
/// table for principals. Used by Lake Formation": a database the caller did not govern
/// with Lake Formation is open to every IAM-allowed principal, which the catalog records
/// as the single `ALL` grant to [`IAM_ALLOWED_PRINCIPALS`] rather than as no grant at
/// all. The distinction matters to the reader: an EMPTY grant list is the opposite
/// governance decision ("nobody, until Lake Formation grants it"), so it is stored only
/// when the client sends one.
pub fn default_create_table_default_permissions() -> Vec<PrincipalPermissions> {
    vec![PrincipalPermissions {
        principal: Some(DataLakePrincipal {
            data_lake_principal_identifier: Some(IAM_ALLOWED_PRINCIPALS.to_string()),
        }),
        permissions: Some(vec![PERMISSION_ALL.to_string()]),
        // Stamped by the read projection, not stored: see
        // [`tagged_principal_permissions`].
        __type: None,
    }]
}

/// The `__type` tag the Data Catalog writes onto a nested `PrincipalPermissions`.
///
/// The Data Catalog's awsJson1.1 serializer tags the nested structures it returns with a
/// type discriminator whose value is the structure's own Smithy shape name, unqualified,
/// so a client that dispatches on the tag can identify the entry. The two structures the
/// recorded wire traffic shows tagged are `PrincipalPermissions` (inside
/// `Database$CreateTableDefaultPermissions`) and [`ERROR_DETAIL_TYPE_TAG`]'s
/// `ErrorDetail`; the tag is therefore keyed on the SHAPE, which is why each constant is
/// spelled exactly like the shape it tags.
pub const PRINCIPAL_PERMISSIONS_TYPE_TAG: &str = "PrincipalPermissions";

/// The `__type` tag the Data Catalog writes onto a nested `ErrorDetail`. See
/// [`PRINCIPAL_PERMISSIONS_TYPE_TAG`] for the rule.
pub const ERROR_DETAIL_TYPE_TAG: &str = "ErrorDetail";

/// A stored grant as the catalog REPORTS it: the stored members plus the `__type` tag.
///
/// The tag belongs to the projection, not to the record: a client may send
/// `CreateTableDefaultPermissions` with or without a tag of its own, and the catalog
/// answers with its own either way — so the tag is stamped here on the way out rather
/// than carried over from whatever the request happened to contain.
pub fn tagged_principal_permissions(stored: &PrincipalPermissions) -> PrincipalPermissions {
    PrincipalPermissions {
        __type: Some(PRINCIPAL_PERMISSIONS_TYPE_TAG.to_string()),
        ..stored.clone()
    }
}

/// A per-item `ErrorDetail` the way the Data Catalog reports one: the error's code and
/// message, plus the `__type` tag ([`ERROR_DETAIL_TYPE_TAG`]).
///
/// Every batch operation that reports a per-item failure builds its `ErrorDetail` here,
/// so the tag cannot be present on one operation's entries and absent from another's.
pub fn catalog_error_detail(error_code: &str, error_message: String) -> ErrorDetail {
    ErrorDetail {
        error_code: Some(error_code.to_string()),
        error_message: Some(error_message),
        __type: Some(ERROR_DETAIL_TYPE_TAG.to_string()),
    }
}

/// The `CatalogEncryptionMode` a catalog is in while nothing encrypts its metadata at
/// rest.
///
/// Source: the modelled `CatalogEncryptionMode` enum, whose published values the
/// generated [`crate::ops::types::CATALOG_ENCRYPTION_MODE_VALUES`] carries (`DISABLED`,
/// `SSE-KMS`, `SSE-KMS-WITH-SERVICE-ROLE`) and the request layer validates against.
/// `DISABLED` is the only one of them that names no key, so it is the mode a catalog no
/// `PutDataCatalogEncryptionSettings` has configured reports.
pub const CATALOG_ENCRYPTION_MODE_DISABLED: &str = "DISABLED";

/// The `EncryptionAtRest` a catalog reports while no setting has been stored for it.
///
/// `EncryptionAtRest$CatalogEncryptionMode` is required, so the structure is only
/// well formed with a mode in it, and the mode of a catalog nothing configured is
/// [`CATALOG_ENCRYPTION_MODE_DISABLED`]; with no mode selected there is no key and no
/// role to report either.
pub fn default_encryption_at_rest() -> EncryptionAtRest {
    EncryptionAtRest {
        catalog_encryption_mode: CATALOG_ENCRYPTION_MODE_DISABLED.to_string(),
        sse_aws_kms_key_id: None,
        catalog_encryption_service_role: None,
    }
}

/// The `ConnectionPasswordEncryption` a catalog reports while no setting has been stored
/// for it.
///
/// `ConnectionPasswordEncryption$ReturnConnectionPasswordEncrypted` is required and
/// modelled with a default of `false`, so a catalog nothing configured hands connection
/// passwords back the way it stored them, under no key of its own.
pub fn default_connection_password_encryption() -> ConnectionPasswordEncryption {
    ConnectionPasswordEncryption {
        return_connection_password_encrypted: false,
        aws_kms_key_id: None,
    }
}

/// How many database ids this process has minted.
///
/// Not catalog state — the id itself is what gets stored, on the [`DatabaseEntry`]. This
/// is only a nonce source for [`new_database_id`], so that two creates landing in the
/// same nanosecond are still given different input to mint from.
static MINTED_DATABASE_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `DatabaseId`: the opaque id the catalog assigns a database when it is
/// created.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is the shape a
/// catalog-assigned Data Catalog id has on the wire. The bits come from [`mint_hex_id`],
/// drawn against [`MINTED_DATABASE_IDS`] — so two creates in the same nanosecond cannot be
/// minted from the same input. It is minted once, at create, and stored on the
/// [`DatabaseEntry`], so
/// every later read of that database — and every table in it — reports the same id, and
/// two databases never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_database_id() -> String {
    mint_hex_id(&MINTED_DATABASE_IDS, CATALOG_ID_WORDS)
}

/// How many 64-bit words a catalog-assigned Data Catalog id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters. The width of a database id and of a
/// form type id alike — both are ids the catalog assigns a resource.
const CATALOG_ID_WORDS: usize = 2;

/// `words` 64-bit draws rendered as lowercase hexadecimal: the one place this crate mints
/// an opaque id from, so every id-minting function below agrees on where the bits come
/// from and how they are spelled.
///
/// The bits come from three sources: the OS-seeded randomness a `RandomState` draws, the
/// system clock in nanoseconds, and `minted`, the caller's own count of how many ids it
/// has handed out — so two ids minted in the same nanosecond cannot come from the same
/// input. Each caller passes its OWN counter, so one kind of id running ahead cannot skew
/// another's.
///
/// This mints a RESOURCE id, not a credential: a credential would come from the credential
/// store, never from here.
fn mint_hex_id(minted: &AtomicU64, words: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::fmt::Write;
    use std::hash::BuildHasher;

    let sequence = minted.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut id = String::with_capacity(words * 16);
    for word in 0..words {
        let draw = RandomState::new().hash_one((sequence, nanos, word));
        // Writing into the pre-sized buffer; `write!` to a `String` cannot fail.
        let _ = write!(id, "{draw:016x}");
    }
    id
}

/// Per-service state for com-amazonaws-glue-awsglue. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlueAwsglueState {
    /// Every Data Catalog this emulator holds, keyed by `CatalogId`.
    pub catalogs: HashMap<String, Catalog>,
    /// Every crawler this emulator holds, keyed by its name.
    ///
    /// Crawlers live BESIDE the catalog tree rather than inside it: `GetCrawler`,
    /// `DeleteCrawler` and `StartCrawler` address a crawler by name alone, with no
    /// `CatalogId` member, so the namespace is per account and a crawler's `CatalogId`
    /// says only where its output goes. The key is the name exactly as sent — unlike
    /// `DatabaseInput$Name`/`TableInput$Name`, the model documents no case fold for
    /// `Crawler$Name`.
    pub crawlers: HashMap<String, CrawlerEntry>,
    /// Every job definition this emulator holds, keyed by the REGION it was created in
    /// and then by its name.
    ///
    /// Jobs live BESIDE the catalog tree, like crawlers: `GetJob`, `UpdateJob`,
    /// `DeleteJob` and `StartJobRun` address a job by name alone, with no `CatalogId`
    /// member, and `CreateJob$Name` is documented as unique per account ("It must be
    /// unique in your account"). Glue is a REGIONAL service, so that namespace is per
    /// account AND per Region: a job created through one Region's endpoint is served only
    /// through that endpoint, and the same name is free in every other Region. The inner
    /// key is the name exactly as sent — the model documents no case fold for `Job$Name`.
    pub jobs: HashMap<String, HashMap<String, JobEntry>>,
    /// Every trigger this emulator holds, keyed by the REGION it was created in and then
    /// by its name, in name order.
    ///
    /// Triggers live BESIDE the catalog tree, like jobs: `GetTrigger`, `UpdateTrigger`,
    /// `StartTrigger`, `StopTrigger` and `DeleteTrigger` address a trigger by name alone,
    /// with no `CatalogId` member. The outer key is the Region for the reason a job's is:
    /// Glue is a REGIONAL service, and a trigger's actions name the jobs and crawlers of
    /// the endpoint that created it, so a trigger created through one Region's endpoint is
    /// served only through that endpoint and the same name is free in every other Region.
    /// The inner map is ordered, so `ListTriggers`/`GetTriggers` have one deterministic
    /// order to page over, and its key is the name exactly as sent — the model documents
    /// no case fold for `Trigger$Name`.
    pub triggers: HashMap<String, BTreeMap<String, TriggerEntry>>,
    /// Every schema registry this emulator holds, keyed by `RegistryName`, in name order.
    ///
    /// Registries live BESIDE the catalog tree: a schema-registry operation addresses its
    /// resource through a `RegistryId`/`SchemaId` wrapper carrying a name or an ARN and no
    /// `CatalogId`, so the namespace is per account. The key is the name exactly as sent —
    /// the model documents no case fold for `SchemaRegistryNameString`, whose pattern
    /// (`^[a-zA-Z0-9-_$#.]+$`) admits both cases and so distinguishes them.
    ///
    /// The map is NOT pre-seeded with [`DEFAULT_REGISTRY_NAME`]: that entry is materialised
    /// by the first create that reaches it, which is what `CreateSchema` documents ("this
    /// will create an entry for a "default-registry" in the registry database tables, if it
    /// is not already present").
    pub registries: BTreeMap<String, RegistryEntry>,
    /// Every custom entity type (custom sensitive-data pattern) this emulator holds,
    /// keyed by its name.
    ///
    /// Per account, like crawlers and job definitions: `GetCustomEntityType`,
    /// `DeleteCustomEntityType` and `BatchGetCustomEntityTypes` address a pattern by name
    /// alone, and `CustomEntityType$Name` is documented as unique per account. Ordered,
    /// so `ListCustomEntityTypes` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for it.
    pub custom_entity_types: BTreeMap<String, CustomEntityTypeEntry>,
    /// Every security configuration this emulator holds, keyed by its name.
    ///
    /// Per account, like crawlers and custom entity types: `GetSecurityConfiguration`,
    /// `DeleteSecurityConfiguration` and `GetSecurityConfigurations` address a security
    /// configuration by name alone, with no `CatalogId` member, so the namespace is per
    /// account and the name a create sends is the whole address. Ordered, so
    /// `GetSecurityConfigurations` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for `NameString`.
    pub security_configurations: BTreeMap<String, SecurityConfigurationEntry>,
    /// Every metadata form type this emulator holds, keyed by `FormTypeName`.
    ///
    /// Per account, like crawlers and job definitions: `PutFormType` names a form type by
    /// `Name` alone, with no `CatalogId` member, and that name is what decides whether a
    /// put creates a form type or updates the one already stored. Ordered, so
    /// `ListFormTypes` has one deterministic order to page over. The key is the name
    /// exactly as sent — the model documents no case fold for `FormTypeName`, whose
    /// pattern (`^[A-Z]\w*$`) already fixes the first character's case.
    ///
    /// A form type is ADDRESSED by an `Identifier` rather than by this key: `GetFormType`
    /// and `DeleteFormType` take one targeting `FormTypeId`, the same shape
    /// `PutFormType`'s `Id` carries, and a client holds either that minted id or the name
    /// it registered the form type under. [`Self::form_type_by_reference`] is that
    /// resolution.
    pub form_types: BTreeMap<String, FormTypeEntry>,
    /// Every asset type this emulator holds, keyed by `AssetTypeName`.
    ///
    /// Per account and beside the catalog tree, like form types: `PutAssetType` names an
    /// asset type by `Name` alone, with no `CatalogId` member, and that name is what
    /// decides whether a put creates an asset type or updates the one already stored.
    /// Ordered, so `ListAssetTypes` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for `AssetTypeName`.
    ///
    /// An asset type is ADDRESSED by an `Identifier` rather than by this key:
    /// `GetAssetType` and `DeleteAssetType` take one targeting `AssetTypeId`, the same
    /// shape `PutAssetType`'s `Id` carries, and a client holds either that minted id or
    /// the name it registered the asset type under. [`Self::asset_type_by_reference`] is
    /// that resolution.
    pub asset_types: BTreeMap<String, AssetTypeEntry>,
    /// Every business glossary this emulator holds, keyed by its `GlossaryId`.
    ///
    /// Per account, like crawlers and job definitions: `GetGlossary`, `UpdateGlossary`,
    /// `DeleteGlossary` and `CreateGlossaryTerm` address a glossary by an identifier
    /// targeting `GlossaryId` alone, with no `CatalogId` member. Keyed by that id rather
    /// than by name because the id is the whole address and a glossary's name can be
    /// changed by an update, while the map stays ordered so `ListGlossaries` has one
    /// deterministic order to page over.
    ///
    /// The NAME is a uniqueness constraint over this namespace rather than its key:
    /// [`Self::glossary_by_name`] is that resolution, and `CreateGlossary`'s modelled
    /// `AlreadyExistsException` is what a second create under a stored name gets.
    pub glossaries: BTreeMap<String, GlossaryEntry>,
    /// Every glossary term this emulator holds, keyed by its `GlossaryTermId`.
    ///
    /// BESIDE the glossaries rather than inside them, exactly as a column statistics task
    /// run sits beside the table it covers: `GetGlossaryTerm`, `UpdateGlossaryTerm` and
    /// `DeleteGlossaryTerm` address a term by a `GlossaryTermId` alone, with no glossary
    /// member at all, so the id is the whole address and the glossary a term belongs to is
    /// recorded ON the term ([`GlossaryTermEntry::glossary_id`]). That is also why deleting
    /// a glossary leaves its terms addressable: the term records are not held by the
    /// glossary record. Ordered, so `ListGlossaryTerms` has one deterministic order to page
    /// over, and bounded by [`MAX_GLOSSARY_TERMS`].
    ///
    /// A term's NAME is a uniqueness constraint within ONE glossary rather than its key:
    /// [`Self::glossary_term_by_name`] is that resolution, and `CreateGlossaryTerm`'s
    /// modelled `AlreadyExistsException` is what a second create of that name in that
    /// glossary gets.
    pub glossary_terms: BTreeMap<String, GlossaryTermEntry>,
    /// Every usage profile this emulator holds, keyed by its name, in name order.
    ///
    /// Per account, like crawlers and custom entity types: `GetUsageProfile`,
    /// `UpdateUsageProfile` and `DeleteUsageProfile` address a profile by `Name` alone,
    /// with no `CatalogId` member. Ordered, so `ListUsageProfiles` has one deterministic
    /// order to page over. The key is the name exactly as sent — the model documents no
    /// case fold for a usage profile name.
    pub usage_profiles: BTreeMap<String, UsageProfileEntry>,
    /// Every machine learning transform this emulator holds, keyed by its `TransformId`.
    ///
    /// Per account and beside the catalog tree, like crawlers and usage profiles:
    /// `GetMLTransform`, `UpdateMLTransform` and `DeleteMLTransform` address a transform by
    /// an id alone, with no `CatalogId` member. Keyed by that minted id rather than by name
    /// because the id is the whole address and a transform's name is not even unique —
    /// "Names are not guaranteed unique and can be changed at any time"
    /// (`MLTransform$Name`) — so two transforms may share one. Ordered, so the two
    /// transform listings have one deterministic order to fall back on, and bounded by
    /// [`MAX_ML_TRANSFORMS`].
    pub ml_transforms: BTreeMap<String, MlTransformEntry>,
    /// Every column statistics task run this emulator holds, oldest first.
    ///
    /// A run lives BESIDE the catalog tree rather than on the table it was started for,
    /// because `GetColumnStatisticsTaskRun` addresses one by `ColumnStatisticsTaskRunId`
    /// alone, with no catalog, database or table member: the id is the whole address, so
    /// the namespace is per account and the table a run covers is recorded ON the run
    /// ([`ColumnStatisticsTaskRunEntry`]). Kept in start order, so
    /// `ListColumnStatisticsTaskRuns` has one deterministic order to page over, and
    /// bounded by [`MAX_COLUMN_STATISTICS_TASK_RUNS`].
    pub column_statistics_task_runs: Vec<ColumnStatisticsTaskRunEntry>,
    /// Every data quality rule recommendation run this emulator holds, oldest first.
    ///
    /// A run lives BESIDE the catalog tree rather than on the table it covers, for the
    /// same reason a column statistics task run does:
    /// `GetDataQualityRuleRecommendationRun` addresses one by `RunId` alone, with no
    /// catalog, database or table member, so the namespace is per account and the data
    /// source a run covers is recorded ON the run
    /// ([`DataQualityRuleRecommendationRunEntry`]). Kept in start order and bounded by
    /// [`MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS`].
    pub data_quality_rule_recommendation_runs: Vec<DataQualityRuleRecommendationRunEntry>,
    /// Every resource policy this emulator holds, keyed by the ARN of the resource it is
    /// attached to, in ARN order.
    ///
    /// A policy lives BESIDE the catalog tree because every operation that addresses one
    /// (`GetResourcePolicy`, `PutResourcePolicy`, `DeleteResourcePolicy`) addresses it by
    /// `ResourceArn` alone, with no `CatalogId` member — so the namespace is per account
    /// and the resource a policy governs is recorded in the key. A request that names no
    /// `ResourceArn` addresses the Data Catalog's own policy, which is keyed by the Data
    /// Catalog's ARN ([`catalog_arn`]), so the default and the catalog ARN spelled out
    /// name ONE record. Ordered, so `GetResourcePolicies` has one deterministic order to
    /// page over, and bounded by [`MAX_RESOURCE_POLICIES`].
    pub resource_policies: BTreeMap<String, ResourcePolicyEntry>,
    /// Every workflow this emulator holds, keyed by its name.
    ///
    /// Per account and beside the catalog tree, like crawlers and custom entity types:
    /// `GetWorkflow`, `UpdateWorkflow`, `DeleteWorkflow` and `StartWorkflowRun` address a
    /// workflow by `Name` alone, with no `CatalogId` member, and `CreateWorkflowRequest$Name`
    /// is documented as unique per account ("It should be unique within your account").
    /// Ordered, so `ListWorkflows` has one deterministic order to page over, and bounded by
    /// [`MAX_WORKFLOWS`]. The key is the name exactly as sent — the model documents no case
    /// fold for a workflow name.
    pub workflows: BTreeMap<String, WorkflowEntry>,
}

impl ComAmazonawsGlueAwsglueState {
    /// Give every column statistics task run the state the CLOCK already owed it at
    /// `now`.
    ///
    /// The one write a reporting operation performs, exactly as [`JobEntry::settle_due_runs`]
    /// is for a job run: a run's next state was due at
    /// [`ColumnStatisticsTaskRunEntry::settles_at`] whether or not anybody called, so
    /// materialising it on read reports the state the clock had already reached.
    pub fn settle_due_column_statistics_task_runs(&mut self, now: SystemTime) {
        for run in &mut self.column_statistics_task_runs {
            run.settle_if_due(now);
        }
    }

    /// The stored column statistics task run with this id, or `None` when this account
    /// holds no such run.
    pub fn column_statistics_task_run(&self, id: &str) -> Option<&ColumnStatisticsTaskRunEntry> {
        self.column_statistics_task_runs
            .iter()
            .find(|run| run.id == id)
    }

    /// The column statistics task run of this table that has not finished, if one has
    /// not.
    ///
    /// Call [`Self::settle_due_column_statistics_task_runs`] first, or a run the clock has
    /// already finished still counts as in flight. `database_name` and `table_name` are
    /// matched folded, the way the catalog stores them.
    pub fn running_column_statistics_task_run(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&ColumnStatisticsTaskRunEntry> {
        self.column_statistics_task_runs.iter().find(|run| {
            !run.is_terminal()
                && run.catalog_id == catalog_id
                && run.database_name == fold_name(database_name)
                && run.table_name == fold_name(table_name)
        })
    }

    /// The LAST column statistics task run started for this table, or `None` when none
    /// has been.
    ///
    /// [`Self::column_statistics_task_runs`] is kept in start order, so the last match is
    /// the most recent attempt — which is what a settings read reports as its
    /// `LastExecutionAttempt`. Call
    /// [`Self::settle_due_column_statistics_task_runs`] first, or the attempt is reported
    /// in a status the clock has already moved it out of. `database_name` and `table_name`
    /// are matched folded, the way the catalog stores them.
    pub fn last_column_statistics_task_run(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&ColumnStatisticsTaskRunEntry> {
        let database = fold_name(database_name);
        let table = fold_name(table_name);
        self.column_statistics_task_runs.iter().rev().find(|run| {
            run.catalog_id == catalog_id
                && run.database_name == database
                && run.table_name == table
        })
    }

    /// Append a column statistics task run, keeping at most
    /// [`MAX_COLUMN_STATISTICS_TASK_RUNS`] of them.
    pub fn record_column_statistics_task_run(&mut self, run: ColumnStatisticsTaskRunEntry) {
        self.column_statistics_task_runs.push(run);
        if self.column_statistics_task_runs.len() > MAX_COLUMN_STATISTICS_TASK_RUNS {
            let overflow = self.column_statistics_task_runs.len() - MAX_COLUMN_STATISTICS_TASK_RUNS;
            self.column_statistics_task_runs.drain(..overflow);
        }
    }

    /// Give every data quality rule recommendation run the state the CLOCK already owed
    /// it at `now`, exactly as [`Self::settle_due_column_statistics_task_runs`] does for a
    /// statistics run.
    pub fn settle_due_data_quality_rule_recommendation_runs(&mut self, now: SystemTime) {
        // A run a cancel asked to stop settles as that stop, whatever its data source
        // holds, so those are settled first and read no catalog at all.
        for run in &mut self.data_quality_rule_recommendation_runs {
            if run.is_stopping() {
                run.settle_stop_if_due(now);
            }
        }
        // What an ANALYSING run settles into is read off the catalog — the columns the
        // covered table declares — so the outcome of every run still due is resolved first
        // and applied afterwards: one walk reads the catalog, the next writes the runs.
        let outcomes: Vec<(usize, RecommendationRunOutcome)> = self
            .data_quality_rule_recommendation_runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.is_due(now))
            .map(|(index, run)| (index, self.covered_table_outcome(run)))
            .collect();
        for (index, outcome) in outcomes {
            if let Some(run) = self.data_quality_rule_recommendation_runs.get_mut(index) {
                run.settle(outcome);
            }
        }
    }

    /// What the data source of `run` offers a recommendation run: the columns the covered
    /// table declares, or the reason there is nothing to analyse.
    fn covered_table_outcome(
        &self,
        run: &DataQualityRuleRecommendationRunEntry,
    ) -> RecommendationRunOutcome {
        let Some(database) = self.database(&run.catalog_id, &run.database_name) else {
            return RecommendationRunOutcome::NothingToAnalyse(messages::database_not_found(
                &run.database_name,
            ));
        };
        match database.tables.get(&fold_name(&run.table_name)) {
            Some(table) => RecommendationRunOutcome::Recommended(table_column_names(table)),
            None => RecommendationRunOutcome::NothingToAnalyse(messages::table_not_found(
                &run.table_name,
            )),
        }
    }

    /// The stored data quality rule recommendation run with this id, or `None` when this
    /// account holds no such run.
    pub fn data_quality_rule_recommendation_run(
        &self,
        id: &str,
    ) -> Option<&DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs
            .iter()
            .find(|run| run.id == id)
    }

    /// The stored data quality rule recommendation run with this id, mutably: what a cancel
    /// moves.
    pub fn data_quality_rule_recommendation_run_mut(
        &mut self,
        id: &str,
    ) -> Option<&mut DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs
            .iter_mut()
            .find(|run| run.id == id)
    }

    /// The run an earlier start with this `ClientToken` recorded, if one did.
    ///
    /// `StartDataQualityRuleRecommendationRunRequest$ClientToken` is "used for idempotency
    /// … to avoid creating or starting multiple instances of the same resource", so a start
    /// carrying a token already recorded is the duplicate the service refuses rather than a
    /// second run.
    pub fn data_quality_rule_recommendation_run_of_client_token(
        &self,
        client_token: &str,
    ) -> Option<&DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs
            .iter()
            .find(|run| run.client_token.as_deref() == Some(client_token))
    }

    /// The recommendation run of this data source that has not finished, if one has not.
    ///
    /// Call [`Self::settle_due_data_quality_rule_recommendation_runs`] first, or a run the
    /// clock has already finished still counts as in flight. `database_name` and
    /// `table_name` are matched folded, the way the catalog stores them.
    pub fn running_data_quality_rule_recommendation_run(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs.iter().find(|run| {
            !run.is_terminal()
                && run.catalog_id == catalog_id
                && run.database_name == fold_name(database_name)
                && run.table_name == fold_name(table_name)
        })
    }

    /// Append a data quality rule recommendation run, keeping at most
    /// [`MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS`] of them.
    pub fn record_data_quality_rule_recommendation_run(
        &mut self,
        run: DataQualityRuleRecommendationRunEntry,
    ) {
        self.data_quality_rule_recommendation_runs.push(run);
        if self.data_quality_rule_recommendation_runs.len()
            > MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS
        {
            let overflow = self.data_quality_rule_recommendation_runs.len()
                - MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS;
            self.data_quality_rule_recommendation_runs.drain(..overflow);
        }
    }

    /// The stored database, or `None` when either the catalog or the database is absent.
    pub fn database(&self, catalog_id: &str, name: &str) -> Option<&DatabaseEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.databases.get(&fold_name(name)))
    }

    /// The stored database, mutably.
    pub fn database_mut(&mut self, catalog_id: &str, name: &str) -> Option<&mut DatabaseEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.databases.get_mut(&fold_name(name)))
    }

    /// The stored connection definition `name` of `catalog_id`, or `None` when either the
    /// catalog or the connection is absent.
    pub fn connection(&self, catalog_id: &str, name: &str) -> Option<&ConnectionEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.connections.get(name))
    }

    /// The stored connection definition `name` of `catalog_id`, mutably.
    pub fn connection_mut(
        &mut self,
        catalog_id: &str,
        name: &str,
    ) -> Option<&mut ConnectionEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.connections.get_mut(name))
    }

    /// Remove the connection definition `name` from `catalog_id`, answering the record
    /// that was removed, or `None` when either the catalog or the connection is absent.
    ///
    /// The whole record goes, tag set included: a connection's tags are held ON the entry
    /// ([`Self::resource_tags`]), so the name a delete frees carries no tags into the next
    /// create under it. The removed entry is handed back rather than dropped, so a caller
    /// that has to report what it deleted reads it from here instead of looking the record
    /// up again before removing it. Matched by name exactly as sent, the way the read path
    /// matches it ([`Self::connection`]).
    pub fn remove_connection(&mut self, catalog_id: &str, name: &str) -> Option<ConnectionEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.connections.remove(name))
    }

    /// How many records the whole store holds of one catalog-scoped kind: `count` says
    /// how many ONE catalog holds, and this sums it over every catalog.
    ///
    /// The one walk the three store-wide counts below share, so a count and the ceiling
    /// checked against it can never disagree about which records they cover. Bounded by
    /// [`MAX_CATALOGS`] catalogs, so the walk itself is a bounded cost.
    fn stored_in_catalogs(&self, count: impl Fn(&Catalog) -> usize) -> usize {
        self.catalogs.values().map(count).sum()
    }

    /// How many tables the whole store holds. Bounded by the ceilings above, so this
    /// walk is over at most [`MAX_CATALOGS`] catalogs of
    /// [`MAX_DATABASES_PER_CATALOG`] entries each.
    pub fn stored_tables(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| database.tables.len())
                .sum()
        })
    }

    /// Whether the store as a whole can hold another table, i.e. is below
    /// [`MAX_STORED_TABLES`].
    pub fn can_store_table(&self) -> bool {
        self.stored_tables() < MAX_STORED_TABLES
    }

    /// How many archived table versions the whole store holds. Bounded the same way
    /// [`Self::stored_tables`] is, over at most [`MAX_STORED_TABLES`] tables.
    pub fn stored_archived_table_versions(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| {
                    database
                        .tables
                        .values()
                        .map(|table| table.archived_versions.len())
                        .sum::<usize>()
                })
                .sum()
        })
    }

    /// Whether the store as a whole can archive another table version, i.e. is below
    /// [`MAX_STORED_ARCHIVED_TABLE_VERSIONS`].
    pub fn can_archive_table_version(&self) -> bool {
        self.stored_archived_table_versions() < MAX_STORED_ARCHIVED_TABLE_VERSIONS
    }

    /// How many partitions the whole store holds. Bounded the same way
    /// [`Self::stored_tables`] is, over at most [`MAX_STORED_TABLES`] tables.
    pub fn stored_partitions(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| {
                    database
                        .tables
                        .values()
                        .map(|table| table.partitions.len())
                        .sum::<usize>()
                })
                .sum()
        })
    }

    /// Whether the store as a whole can hold another partition, i.e. is below
    /// [`MAX_STORED_PARTITIONS`].
    ///
    /// A batch create writes SEVERAL partitions under one lock, so a caller that walks a
    /// batch reads this count once and then counts the partitions it has inserted
    /// itself: re-walking the store per item would be the same answer at a cost per
    /// item.
    pub fn can_store_partition(&self) -> bool {
        self.stored_partitions() < MAX_STORED_PARTITIONS
    }

    /// How many databases the whole store holds, across every catalog. Bounded by
    /// [`MAX_CATALOGS`] times [`MAX_DATABASES_PER_CATALOG`], so this walk is bounded.
    pub fn stored_databases(&self) -> usize {
        self.stored_in_catalogs(|catalog| catalog.databases.len())
    }

    /// Whether the store as a whole can hold another database, i.e. is below
    /// [`MAX_STORED_DATABASES`].
    pub fn can_store_database(&self) -> bool {
        self.stored_databases() < MAX_STORED_DATABASES
    }

    /// How many connection definitions the whole store holds, across every catalog.
    pub fn stored_connections(&self) -> usize {
        self.stored_in_catalogs(|catalog| catalog.connections.len())
    }

    /// Whether the store as a whole can hold another connection definition, i.e. is
    /// below [`MAX_STORED_CONNECTIONS`].
    pub fn can_store_connection(&self) -> bool {
        self.stored_connections() < MAX_STORED_CONNECTIONS
    }

    /// The catalog `catalog_id`, for a create that is about to write a record into it:
    /// the one this store already holds, or a fresh empty one when there is room for
    /// another container ([`MAX_CATALOGS`]). `None` means the store holds no such catalog
    /// and cannot take one, so the create must be refused rather than served.
    ///
    /// The ONE place a catalog is brought into existence, so no create can grow the
    /// container map past the ceiling, and a create that is refused earlier leaves no
    /// empty catalog behind: a handler calls this only once every other check has passed.
    pub fn catalog_mut_for_create(&mut self, catalog_id: &str) -> Option<&mut Catalog> {
        if !self.catalogs.contains_key(catalog_id) && self.catalogs.len() >= MAX_CATALOGS {
            return None;
        }
        Some(self.catalogs.entry(catalog_id.to_string()).or_default())
    }

    /// The Data Catalog encryption settings `catalog_id` REPORTS: the record a
    /// `PutDataCatalogEncryptionSettings` stored for it, with each setting the stored
    /// record left unset filled in with that setting's default
    /// ([`default_encryption_at_rest`], [`default_connection_password_encryption`]).
    ///
    /// Both settings are therefore always reported, for a catalog nothing ever
    /// configured as much as for one a write named only half of: the catalog holds a
    /// value for each of them at all times, and a write that named one setting leaves
    /// the other at its default rather than at whatever an earlier write put there — the
    /// stored record is the whole settings structure the write carried.
    pub fn data_catalog_encryption_settings(
        &self,
        catalog_id: &str,
    ) -> DataCatalogEncryptionSettings {
        let stored = self
            .catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.encryption_settings.as_ref());
        DataCatalogEncryptionSettings {
            encryption_at_rest: Some(
                stored
                    .and_then(|settings| settings.encryption_at_rest.clone())
                    .unwrap_or_else(default_encryption_at_rest),
            ),
            connection_password_encryption: Some(
                stored
                    .and_then(|settings| settings.connection_password_encryption.clone())
                    .unwrap_or_else(default_connection_password_encryption),
            ),
        }
    }

    /// The resource policy attached to `resource_arn`, or `None` when no policy is
    /// attached to that resource.
    ///
    /// `resource_arn` is the ARN the request addressed, as
    /// [`resolve_policy_resource_arn`] resolved it, so a request that named none and one
    /// that spelled the Data Catalog's ARN out resolve to the same record.
    pub fn resource_policy(&self, resource_arn: &str) -> Option<&ResourcePolicyEntry> {
        self.resource_policies.get(resource_arn)
    }

    /// Whether the store can hold another resource policy, i.e. is below
    /// [`MAX_RESOURCE_POLICIES`].
    pub fn can_store_resource_policy(&self) -> bool {
        self.resource_policies.len() < MAX_RESOURCE_POLICIES
    }

    /// Attach `policy_in_json` to `resource_arn`, and answer the `PolicyHash` of the
    /// policy now attached.
    ///
    /// A resource that already carries a policy has that policy's DOCUMENT replaced \u2014 the
    /// put carries the whole document, so nothing is merged \u2014 while the record keeps the
    /// `CreateTime` of the policy that was first attached and takes `now` as its
    /// `UpdateTime`. `None` means the store holds no policy for this resource and cannot
    /// take another ([`MAX_RESOURCE_POLICIES`]), so the write must be refused rather than
    /// served.
    pub fn set_resource_policy(
        &mut self,
        resource_arn: &str,
        policy_in_json: String,
        now: SystemTime,
    ) -> Option<String> {
        if let Some(stored) = self.resource_policies.get_mut(resource_arn) {
            stored.replace_document(policy_in_json, now);
            return Some(stored.policy_hash.clone());
        }
        if !self.can_store_resource_policy() {
            return None;
        }
        let attached = ResourcePolicyEntry::new(policy_in_json, now);
        let policy_hash = attached.policy_hash.clone();
        self.resource_policies
            .insert(resource_arn.to_string(), attached);
        Some(policy_hash)
    }

    /// Detach the policy of `resource_arn`, answering whether one was attached.
    pub fn remove_resource_policy(&mut self, resource_arn: &str) -> bool {
        self.resource_policies.remove(resource_arn).is_some()
    }

    /// The policies attached to INDIVIDUAL resources, in the order their ARNs sort in.
    ///
    /// Every stored policy except an account-level Data Catalog policy
    /// ([`names_a_catalog`]), which is the set `GetResourcePolicies` enumerates: the
    /// catalog's own policy is reported by `GetResourcePolicy`, which addresses it
    /// directly, and is not one of the individual resources' policies that list reports.
    pub fn individual_resource_policies(&self) -> Vec<(&String, &ResourcePolicyEntry)> {
        self.resource_policies
            .iter()
            .filter(|(resource_arn, _)| !names_a_catalog(resource_arn))
            .collect()
    }

    /// Whether the store can hold another workflow, i.e. is below [`MAX_WORKFLOWS`].
    pub fn can_store_workflow(&self) -> bool {
        self.workflows.len() < MAX_WORKFLOWS
    }

    /// The stored workflow `name`, or `None` when this account holds no such workflow.
    pub fn workflow(&self, name: &str) -> Option<&WorkflowEntry> {
        self.workflows.get(name)
    }

    /// [`Self::workflow`] mutably: the one record every write to a workflow, and every
    /// write to one of its runs, lands in.
    pub fn workflow_mut(&mut self, name: &str) -> Option<&mut WorkflowEntry> {
        self.workflows.get_mut(name)
    }

    /// The trigger of the Region `region` that can BEGIN a run of the workflow
    /// `workflow_name`, or `None` when that workflow has no entry point.
    ///
    /// A trigger belongs to the workflow its `WorkflowName` names, and it can begin a run
    /// unless firing it depends on something inside the workflow having run first: a
    /// `CONDITIONAL` trigger "fires when its predicate is satisfied"
    /// (`CreateTriggerRequest$Predicate` — "required when the trigger type is
    /// CONDITIONAL"), and that predicate's conditions are the states of this workflow's own
    /// jobs and crawlers, so it can only continue a run, never start one. Every other
    /// modelled `TriggerType` fires on something outside the workflow — a caller, the
    /// clock, an event — so any of them is an entry point.
    ///
    /// The walk is over the Region's triggers, bounded by [`MAX_TRIGGERS`], and takes them
    /// in name order (the map is ordered), so the same workflow resolves to the same entry
    /// point on every call.
    pub fn starting_trigger(&self, region: &str, workflow_name: &str) -> Option<&TriggerEntry> {
        self.triggers.get(region)?.values().find(|trigger| {
            trigger.workflow_name.as_deref() == Some(workflow_name)
                && trigger.trigger_type != TRIGGER_TYPE_CONDITIONAL
        })
    }

    /// The graph of the workflow `workflow_name`: "all the Glue components present in the
    /// workflow and all the directed connections between them" (`WorkflowGraph`).
    ///
    /// A workflow holds the triggers whose `WorkflowName` names it, and each of those
    /// triggers names the components it works with: "the actions initiated by this trigger"
    /// (`Trigger$Actions`), each naming a job or a crawler, and the conditions of "the
    /// predicate of this trigger, which defines when it will fire" (`Trigger$Predicate`),
    /// each naming the job or crawler whose state it waits on. So the nodes are those
    /// triggers plus every component they name, and an edge is directed the way the work
    /// flows: from a trigger to what it initiates, and from a component to the trigger that
    /// waits on it. A component named twice is one node, and a connection named twice is
    /// one edge.
    ///
    /// `region` is the Region the read is served in, which is where a trigger lives — a
    /// trigger is served only through the endpoint that created it — while a workflow is
    /// per-account, so a workflow read in another Region reports the components attached
    /// there.
    ///
    /// The walk is over that Region's triggers, bounded by [`MAX_TRIGGERS`], and takes them
    /// in name order (the map is ordered), so two reads of an unchanged store report the
    /// same graph.
    pub fn workflow_graph(&self, region: &str, workflow_name: &str) -> WorkflowGraph {
        let mut graph = WorkflowGraphBuilder::default();
        let attached = self
            .triggers
            .get(region)
            .into_iter()
            .flat_map(BTreeMap::values)
            .filter(|trigger| trigger.workflow_name.as_deref() == Some(workflow_name));
        for trigger in attached {
            let trigger_id = graph.node(NODE_TYPE_TRIGGER, &trigger.name, Some(trigger));
            for action in &trigger.actions {
                for (node_type, name) in
                    named_components(action.job_name.as_deref(), action.crawler_name.as_deref())
                {
                    let initiated = graph.node(node_type, name, None);
                    graph.edge(&trigger_id, &initiated);
                }
            }
            for condition in trigger
                .predicate
                .as_ref()
                .and_then(|predicate| predicate.conditions.as_ref())
                .into_iter()
                .flatten()
            {
                for (node_type, name) in named_components(
                    condition.job_name.as_deref(),
                    condition.crawler_name.as_deref(),
                ) {
                    let waited_on = graph.node(node_type, name, None);
                    graph.edge(&waited_on, &trigger_id);
                }
            }
        }
        graph.build()
    }

    /// Store `entry` as this account's workflow under its own name, which is the key every
    /// read addresses it by ([`Self::workflow`]) — so the namespace's key form lives here
    /// rather than in each handler that writes to it.
    pub fn insert_workflow(&mut self, entry: WorkflowEntry) {
        self.workflows.insert(entry.name.clone(), entry);
    }

    /// Whether the store can hold another crawler, i.e. is below [`MAX_CRAWLERS`].
    pub fn can_store_crawler(&self) -> bool {
        self.crawlers.len() < MAX_CRAWLERS
    }

    /// The crawler `name`, or `None` when this account holds no crawler under that name.
    ///
    /// A crawler is addressed by name alone, in ONE per-account namespace — the rule the
    /// `crawlers` field documents — so this is where that addressing lives, as
    /// [`Self::job`] is for a job definition's Region-scoped namespace.
    pub fn crawler(&self, name: &str) -> Option<&CrawlerEntry> {
        self.crawlers.get(name)
    }

    /// Whether the store can hold another job definition, i.e. is below [`MAX_JOBS`].
    pub fn can_store_job(&self) -> bool {
        self.stored_jobs() < MAX_JOBS
    }

    /// How many job definitions this store holds across every Region. The walk is over
    /// the Regions one process has been addressed in, so it is bounded.
    pub fn stored_jobs(&self) -> usize {
        self.jobs.values().map(HashMap::len).sum()
    }

    /// The job definition `name` of the Region `region`, or `None` when that Region holds
    /// no job under that name — which is every job of every other Region, because a Glue
    /// job exists only in the Region it was created in.
    pub fn job(&self, region: &str, name: &str) -> Option<&JobEntry> {
        self.jobs.get(region).and_then(|jobs| jobs.get(name))
    }

    /// [`Self::job`] mutably: the one record every write to a job definition, and every
    /// write to one of its runs, lands in.
    pub fn job_mut(&mut self, region: &str, name: &str) -> Option<&mut JobEntry> {
        self.jobs
            .get_mut(region)
            .and_then(|jobs| jobs.get_mut(name))
    }

    /// Store `entry` as the Region `region`'s job definition under its own name.
    pub fn insert_job(&mut self, region: &str, entry: JobEntry) {
        self.jobs
            .entry(region.to_string())
            .or_default()
            .insert(entry.name.clone(), entry);
    }

    /// Every job definition the Region `region` holds, in name order.
    pub fn jobs_in(&self, region: &str) -> Vec<&JobEntry> {
        let mut found: Vec<&JobEntry> = self
            .jobs
            .get(region)
            .into_iter()
            .flat_map(HashMap::values)
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        found
    }

    /// Whether the store can hold another trigger, i.e. is below [`MAX_TRIGGERS`].
    pub fn can_store_trigger(&self) -> bool {
        self.stored_triggers() < MAX_TRIGGERS
    }

    /// How many triggers this store holds across every Region. The walk is over the
    /// Regions one process has been addressed in, so it is bounded.
    pub fn stored_triggers(&self) -> usize {
        self.triggers.values().map(BTreeMap::len).sum()
    }

    /// The trigger `name` of the Region `region`, or `None` when that Region holds no
    /// trigger under that name — which is every trigger of every other Region, because a
    /// trigger is served only through the endpoint that created it.
    pub fn trigger(&self, region: &str, name: &str) -> Option<&TriggerEntry> {
        self.triggers
            .get(region)
            .and_then(|triggers| triggers.get(name))
    }

    /// Store `entry` as the Region `region`'s trigger under its own name.
    pub fn insert_trigger(&mut self, region: &str, entry: TriggerEntry) {
        self.triggers
            .entry(region.to_string())
            .or_default()
            .insert(entry.name.clone(), entry);
    }

    /// Whether the store can hold another schema registry, i.e. is below
    /// [`MAX_REGISTRIES`].
    pub fn can_store_registry(&self) -> bool {
        self.registries.len() < MAX_REGISTRIES
    }

    /// The stored registry `name`, or `None` when this store holds no such registry.
    pub fn registry(&self, name: &str) -> Option<&RegistryEntry> {
        self.registries.get(name)
    }

    /// The stored registry `name`, mutably.
    pub fn registry_mut(&mut self, name: &str) -> Option<&mut RegistryEntry> {
        self.registries.get_mut(name)
    }

    /// The stored schema set `schema_name` of registry `registry_name`, or `None` when
    /// either the registry or the schema is absent.
    pub fn schema(&self, registry_name: &str, schema_name: &str) -> Option<&SchemaEntry> {
        self.registries
            .get(registry_name)
            .and_then(|registry| registry.schemas.get(schema_name))
    }

    /// How many schema sets the whole store holds. Bounded by [`MAX_REGISTRIES`], so this
    /// walk is over at most that many entries.
    pub fn stored_schemas(&self) -> usize {
        self.registries
            .values()
            .map(|registry| registry.schemas.len())
            .sum()
    }

    /// Whether the store as a whole can hold another schema set, i.e. is below
    /// [`MAX_STORED_SCHEMAS`].
    pub fn can_store_schema(&self) -> bool {
        self.stored_schemas() < MAX_STORED_SCHEMAS
    }

    /// Whether the store can hold another custom entity type, i.e. is below
    /// [`MAX_CUSTOM_ENTITY_TYPES`].
    pub fn can_store_custom_entity_type(&self) -> bool {
        self.custom_entity_types.len() < MAX_CUSTOM_ENTITY_TYPES
    }

    /// Whether the store can hold another security configuration, i.e. is below
    /// [`MAX_SECURITY_CONFIGURATIONS`].
    pub fn can_store_security_configuration(&self) -> bool {
        self.security_configurations.len() < MAX_SECURITY_CONFIGURATIONS
    }

    /// Whether the store can hold another usage profile, i.e. is below
    /// [`MAX_USAGE_PROFILES`].
    pub fn can_store_usage_profile(&self) -> bool {
        self.usage_profiles.len() < MAX_USAGE_PROFILES
    }

    /// Whether the store can hold another machine learning transform, i.e. is below
    /// [`MAX_ML_TRANSFORMS`].
    pub fn can_store_ml_transform(&self) -> bool {
        self.ml_transforms.len() < MAX_ML_TRANSFORMS
    }

    /// Every stored machine learning transform `filter` admits, in the order `sort` names,
    /// each paired with its ordering key — which is what a listing's continuation token
    /// carries ([`MlTransformEntry::ordering_key`]).
    ///
    /// One walk, shared by the two transform listings, so the listing that reports
    /// transforms and the one that reports their ids can never disagree about which
    /// transforms a filter admits or what order they come in. Bounded by
    /// [`MAX_ML_TRANSFORMS`].
    pub fn ml_transforms_in_order(
        &self,
        filter: Option<&TransformFilterCriteria>,
        sort: Option<&TransformSortCriteria>,
    ) -> Vec<(String, &MlTransformEntry)> {
        let column = sort.map(|criteria| criteria.column.as_str());
        let mut ordered: Vec<(String, &MlTransformEntry)> = self
            .ml_transforms
            .values()
            .filter(|transform| filter.is_none_or(|criteria| transform.matches(criteria)))
            .map(|transform| (transform.ordering_key(column), transform))
            .collect();
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        if sort.is_some_and(|criteria| criteria.sort_direction == SORT_DIRECTION_DESCENDING) {
            // The ordering key is unique per transform, so reversing the ascending order
            // is exactly the descending one.
            ordered.reverse();
        }
        ordered
    }

    /// The schema a machine learning transform over `tables` runs against: "the columns and
    /// data types that this transform can run against" (`MLTransform$Schema`).
    ///
    /// A create carries no schema, so the columns are read out of the CATALOG, from the
    /// input record tables the request addressed, in the order those tables name them. A
    /// column name already contributed by an earlier table is not repeated — the schema is
    /// a map from column to type — and a table this store does not hold contributes
    /// nothing, the way a run over an absent table recommends nothing rather than being
    /// refused. Capped at [`MAX_TRANSFORM_SCHEMA_COLUMNS`], which is the modelled length of
    /// the member.
    pub fn ml_transform_schema(&self, tables: &[GlueTable]) -> Vec<SchemaColumn> {
        let mut schema: Vec<SchemaColumn> = Vec::new();
        for table in tables {
            let catalog_id = resolve_catalog_id(table.catalog_id.as_ref());
            let Some(stored) = self
                .database(&catalog_id, &table.database_name)
                .and_then(|database| database.tables.get(&fold_name(&table.table_name)))
            else {
                continue;
            };
            for column in stored
                .storage_descriptor
                .as_ref()
                .and_then(|descriptor| descriptor.columns.as_ref())
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if schema.len() >= MAX_TRANSFORM_SCHEMA_COLUMNS {
                    return schema;
                }
                if schema
                    .iter()
                    .any(|held| held.name.as_deref() == Some(column.name.as_str()))
                {
                    continue;
                }
                schema.push(SchemaColumn {
                    name: Some(column.name.clone()),
                    data_type: column.r#type.clone(),
                });
            }
        }
        schema
    }

    /// Whether the store can hold another form type, i.e. is below [`MAX_FORM_TYPES`].
    pub fn can_store_form_type(&self) -> bool {
        self.form_types.len() < MAX_FORM_TYPES
    }

    /// The stored form type whose `Id` is `id`, or `None` when this store holds no form
    /// type under that id.
    ///
    /// Every operation that ADDRESSES a form type does so by a `FormTypeId`
    /// (`GetFormType$Identifier`, `DeleteFormType$Identifier`, `PutAttachment$FormTypeId`
    /// all target that shape), while the records are keyed by the name a put addresses
    /// them by — so the id resolution is a walk, bounded by [`MAX_FORM_TYPES`]. One copy,
    /// shared, so no two operations can disagree about which record an id names.
    pub fn form_type_by_id(&self, id: &str) -> Option<&FormTypeEntry> {
        self.form_types
            .values()
            .find(|form_type| form_type.id == id)
    }

    /// The stored form type an `AssetTypeFormReference$FormTypeIdentifier` names, or
    /// `None` when this store holds no such form type.
    ///
    /// A reference carries a form type's identifier as the CLIENT holds it, which is
    /// either the `FormTypeId` the catalog minted for it or the `FormTypeName` it was
    /// registered under: a client that put a form type by name addresses it by that name,
    /// and the catalog serves that reference, so both resolve to the one record. The
    /// minted id is tried first, so an identifier that could be read either way names the
    /// record it was minted for.
    pub fn form_type_by_reference(&self, identifier: &str) -> Option<&FormTypeEntry> {
        self.form_type_by_id(identifier)
            .or_else(|| self.form_types.get(identifier))
    }

    /// Whether the store can hold another asset type, i.e. is below [`MAX_ASSET_TYPES`].
    pub fn can_store_asset_type(&self) -> bool {
        self.asset_types.len() < MAX_ASSET_TYPES
    }

    /// The stored asset type whose `Id` is `id`, or `None` when this store holds no asset
    /// type under that id.
    ///
    /// Every operation that ADDRESSES an asset type does so by an `AssetTypeId`
    /// (`GetAssetType$Identifier`, `DeleteAssetType$Identifier` and `PutAsset$AssetTypeId`
    /// all target that shape), while the records are keyed by the name a put addresses
    /// them by — so the id resolution is a walk, bounded by [`MAX_ASSET_TYPES`]. One copy,
    /// shared, so no two operations can disagree about which record an id names.
    pub fn asset_type_by_id(&self, id: &str) -> Option<&AssetTypeEntry> {
        self.asset_types
            .values()
            .find(|asset_type| asset_type.id == id)
    }

    /// The stored asset type an `Identifier` targeting `AssetTypeId` names, or `None` when
    /// this store holds no such asset type.
    ///
    /// An identifier carries an asset type's address as the CLIENT holds it, which is
    /// either the `AssetTypeId` the catalog minted for it or the `AssetTypeName` it was
    /// registered under, exactly as a form type reference does
    /// ([`Self::form_type_by_reference`]): a client that put an asset type by name
    /// addresses it by that name, and the catalog serves that read. The minted id is tried
    /// first, so an identifier that could be read either way names the record it was
    /// minted for.
    pub fn asset_type_by_reference(&self, identifier: &str) -> Option<&AssetTypeEntry> {
        self.asset_type_by_id(identifier)
            .or_else(|| self.asset_types.get(identifier))
    }

    /// Whether the store can hold another business glossary, i.e. is below
    /// [`MAX_GLOSSARIES`].
    pub fn can_store_glossary(&self) -> bool {
        self.glossaries.len() < MAX_GLOSSARIES
    }

    /// The stored glossary whose `Id` is `id`, or `None` when this account holds no
    /// glossary under that id.
    ///
    /// Every operation that addresses a glossary does so by a `GlossaryId`, which is this
    /// map's key, so the lookup is the key lookup. One copy, shared, so no two operations
    /// can disagree about which record an identifier names.
    pub fn glossary(&self, id: &str) -> Option<&GlossaryEntry> {
        self.glossaries.get(id)
    }

    /// The stored glossary whose `Id` is `id`, mutably.
    pub fn glossary_mut(&mut self, id: &str) -> Option<&mut GlossaryEntry> {
        self.glossaries.get_mut(id)
    }

    /// The stored glossary carrying this `Name`, or `None` when the name is free.
    ///
    /// A glossary's name is unique over this per-account namespace — that is what
    /// `CreateGlossary`'s modelled `AlreadyExistsException` refuses a second create under
    /// — and the records are keyed by id, so the resolution is a walk bounded by
    /// [`MAX_GLOSSARIES`]. The name is matched exactly as sent: the model documents no
    /// case fold for `GlossaryName`.
    pub fn glossary_by_name(&self, name: &str) -> Option<&GlossaryEntry> {
        self.glossaries
            .values()
            .find(|glossary| glossary.name == name)
    }

    /// Whether the store can hold another glossary term, i.e. is below
    /// [`MAX_GLOSSARY_TERMS`].
    pub fn can_store_glossary_term(&self) -> bool {
        self.glossary_terms.len() < MAX_GLOSSARY_TERMS
    }

    /// The stored glossary term whose `Id` is `id`, or `None` when this account holds no
    /// term under that id.
    ///
    /// Every operation that addresses a term does so by a `GlossaryTermId`, which is this
    /// map's key, so the lookup is the key lookup. One copy, shared, so no two operations
    /// can disagree about which record an identifier names.
    pub fn glossary_term(&self, id: &str) -> Option<&GlossaryTermEntry> {
        self.glossary_terms.get(id)
    }

    /// The stored glossary term whose `Id` is `id`, mutably.
    pub fn glossary_term_mut(&mut self, id: &str) -> Option<&mut GlossaryTermEntry> {
        self.glossary_terms.get_mut(id)
    }

    /// The term carrying this `Name` within the glossary `glossary_id` names, or `None`
    /// when that name is free in that glossary.
    ///
    /// A term's name identifies it WITHIN its glossary — that is what
    /// `CreateGlossaryTerm`'s modelled `AlreadyExistsException` refuses a second create
    /// under — so the same name in another glossary is a different term. The records are
    /// keyed by id, so the resolution is a walk bounded by [`MAX_GLOSSARY_TERMS`]. The name
    /// is matched exactly as sent: the model documents no case fold for `GlossaryTermName`.
    pub fn glossary_term_by_name(
        &self,
        glossary_id: &str,
        name: &str,
    ) -> Option<&GlossaryTermEntry> {
        self.glossary_terms
            .values()
            .find(|term| term.glossary_id == glossary_id && term.name == name)
    }

    /// The tag set of the resource `resource` names, or `None` when this store holds no
    /// such resource.
    ///
    /// The ONE resolution every tag READ goes through, so an ARN names the same record
    /// whichever operation — this service's own `GetTags`, or the account-wide tagging
    /// API that projects this store — resolved it.
    pub fn resource_tags(&self, resource: &TaggedResource) -> Option<&HashMap<String, String>> {
        match resource.kind {
            // A database and a connection live INSIDE a catalog, so the ARN's account
            // segment is the `CatalogId` they are looked for under.
            TaggedResourceKind::Database => self
                .catalogs
                .get(&resource.account_id)
                .and_then(|catalog| catalog.databases.get(&resource.name))
                .map(|database| &database.tags),
            TaggedResourceKind::Connection => self
                .catalogs
                .get(&resource.account_id)
                .and_then(|catalog| catalog.connections.get(&resource.name))
                .map(|connection| &connection.tags),
            // A crawler and a job definition live BESIDE the catalog tree in one
            // per-account namespace, so the account segment is what decides whether this
            // emulator could hold them at all.
            TaggedResourceKind::Crawler => is_served_account(&resource.account_id)
                .then(|| self.crawlers.get(&resource.name).map(|crawler| &crawler.tags))
                .flatten(),
            TaggedResourceKind::Job => is_served_account(&resource.account_id)
                .then(|| {
                    self.job(&resource.region, &resource.name)
                        .map(|job| &job.tags)
                })
                .flatten(),
        }
    }

    /// [`Self::resource_tags`] mutably: the ONE record every tag WRITE lands in, so a tag
    /// written through this service's own `TagResource` and one written through the
    /// account-wide tagging API are the same tag.
    pub fn resource_tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut HashMap<String, String>> {
        match resource.kind {
            TaggedResourceKind::Database => self
                .catalogs
                .get_mut(&resource.account_id)
                .and_then(|catalog| catalog.databases.get_mut(&resource.name))
                .map(|database| &mut database.tags),
            TaggedResourceKind::Connection => self
                .catalogs
                .get_mut(&resource.account_id)
                .and_then(|catalog| catalog.connections.get_mut(&resource.name))
                .map(|connection| &mut connection.tags),
            TaggedResourceKind::Crawler => {
                if !is_served_account(&resource.account_id) {
                    return None;
                }
                self.crawlers
                    .get_mut(&resource.name)
                    .map(|crawler| &mut crawler.tags)
            }
            TaggedResourceKind::Job => {
                if !is_served_account(&resource.account_id) {
                    return None;
                }
                let region = resource.region.clone();
                let name = resource.name.clone();
                self.job_mut(&region, &name).map(|job| &mut job.tags)
            }
        }
    }

    /// Every resource this store holds a tag set for, as `(ARN, tags)`.
    ///
    /// This is the account's tag-store rows for this service: the tags a create recorded
    /// are the resource's own, so the account-wide tagging API projects THESE records
    /// rather than keeping a second copy that a create would have to be taught to write.
    /// Resources carrying NO tag are included — the caller decides what to do with an
    /// untagged resource — and `region` is the Region the ARNs are minted for, for the
    /// reason [`request_region`] gives.
    pub fn tagged_resources(&self, region: &str) -> Vec<(String, HashMap<String, String>)> {
        let mut found: Vec<(String, HashMap<String, String>)> = Vec::new();
        for catalog in self.catalogs.values() {
            for database in catalog.databases.values() {
                found.push((
                    TaggedResourceKind::Database.arn(region, &database.name),
                    database.tags.clone(),
                ));
            }
            for connection in catalog.connections.values() {
                found.push((
                    TaggedResourceKind::Connection.arn(region, &connection.name),
                    connection.tags.clone(),
                ));
            }
        }
        for crawler in self.crawlers.values() {
            found.push((
                TaggedResourceKind::Crawler.arn(region, &crawler.name),
                crawler.tags.clone(),
            ));
        }
        for job in self.jobs_in(region) {
            found.push((
                TaggedResourceKind::Job.arn(region, &job.name),
                job.tags.clone(),
            ));
        }
        found
    }
}

/// One Data Catalog: the databases stored under a single `CatalogId`.
#[derive(Debug, Default)]
pub struct Catalog {
    /// Databases keyed by their folded (lowercased) name.
    pub databases: HashMap<String, DatabaseEntry>,
    /// Connection definitions keyed by `ConnectionInput$Name`, in name order.
    pub connections: BTreeMap<String, ConnectionEntry>,
    /// The security configuration `PutDataCatalogEncryptionSettings` last set on this
    /// catalog, as that write carried it, or `None` while no write has set one.
    ///
    /// ONE record per catalog rather than one per resource: every operation that
    /// addresses it names only a `CatalogId`, and the write takes the whole
    /// `DataCatalogEncryptionSettings` structure, so a write REPLACES this record rather
    /// than merging into it. What a read reports from it, defaults included, is
    /// [`ComAmazonawsGlueAwsglueState::data_catalog_encryption_settings`].
    pub encryption_settings: Option<DataCatalogEncryptionSettings>,
}

impl Catalog {
    /// Whether this catalog can hold another database, i.e. is below
    /// [`MAX_DATABASES_PER_CATALOG`].
    pub fn can_store_database(&self) -> bool {
        self.databases.len() < MAX_DATABASES_PER_CATALOG
    }

    /// Whether this catalog can hold another connection, i.e. is below
    /// [`MAX_CONNECTIONS_PER_CATALOG`].
    pub fn can_store_connection(&self) -> bool {
        self.connections.len() < MAX_CONNECTIONS_PER_CATALOG
    }
}

/// One resource policy: the policy document attached to a single Glue resource, and when
/// it was attached and last written.
///
/// The record is what every read of that policy reports, so the `PolicyHash` a write
/// answered and the one a later read reports are the same string.
#[derive(Debug, Clone)]
pub struct ResourcePolicyEntry {
    /// `PolicyInJson`: "the requested policy document, in JSON format", exactly as the
    /// write carried it \u2014 a policy is reported as stored, never re-serialised.
    pub policy_in_json: String,
    /// `PolicyHash`: the hash of [`Self::policy_in_json`], from [`policy_hash`].
    pub policy_hash: String,
    /// `CreateTime`: "the date and time at which the policy was created" \u2014 the instant the
    /// FIRST write attached a policy to this resource, kept across later writes.
    pub create_time: SystemTime,
    /// `UpdateTime`: "the date and time at which the policy was last updated".
    pub update_time: SystemTime,
}

impl ResourcePolicyEntry {
    /// A policy attached at `now`, whose create and update instants are therefore the
    /// same.
    pub fn new(policy_in_json: String, now: SystemTime) -> Self {
        Self {
            policy_hash: policy_hash(&policy_in_json),
            policy_in_json,
            create_time: now,
            update_time: now,
        }
    }

    /// Replace this record's document with the one a later write carried, at `now`.
    ///
    /// The document is replaced whole and the hash recomputed from it, while
    /// [`Self::create_time`] stays where the first write put it: the policy is the same
    /// attachment, rewritten.
    pub fn replace_document(&mut self, policy_in_json: String, now: SystemTime) {
        self.policy_hash = policy_hash(&policy_in_json);
        self.policy_in_json = policy_in_json;
        self.update_time = now;
    }
}

/// The `PolicyHash` of a policy document: the SHA-256 of the document, lowercase
/// hexadecimal.
///
/// Derived from the document's CONTENT, which is what the member is for: it is "the hash
/// value returned when the previous policy was set using `PutResourcePolicy`", whose
/// "purpose is to prevent concurrent modifications of a policy"
/// (`PutResourcePolicyRequest$PolicyHashCondition`) \u2014 so the same document always hashes
/// to the same value, and a document changed in any way hashes to a different one. Glue
/// publishes no digest for it, so the algorithm is this emulator's; the 64 hexadecimal
/// characters are inside `HashString`'s `length 1..255` and match its pattern.
///
/// This is not a per-run value and not a credential: it is computed from the request's
/// own document, so two runs that attach the same policy report the same hash.
pub fn policy_hash(policy_in_json: &str) -> String {
    use ::sha2::Digest;

    ::hex::encode(::sha2::Sha256::digest(policy_in_json.as_bytes()))
}

/// The resource whose policy a resource-policy request addresses: the `ResourceArn` it
/// names, else this account's Data Catalog ("If not supplied, the Data Catalog resource
/// policy is returned" \u2014 `GetResourcePolicyRequest$ResourceArn`).
///
/// `Err` carries the sentence for a `ResourceArn` this endpoint can hold no policy for \u2014
/// one that is not a well-formed Glue ARN, or one naming another account's or another
/// Region's estate, whose policies this process does not hold.
///
/// The ONE resolution all three resource-policy operations go through, so a policy
/// written under one spelling of its resource is read back under the other.
pub fn resolve_policy_resource_arn(
    resource_arn: Option<&String>,
    region: &str,
) -> Result<String, String> {
    let Some(named) = resource_arn.map(String::as_str).filter(|arn| !arn.is_empty()) else {
        // The catalog of the account this endpoint serves, which is the one a request
        // that names no catalog addresses ([`resolve_catalog_id`]).
        return Ok(catalog_arn(region, &resolve_catalog_id(None)));
    };
    match parse_glue_arn(named) {
        Some(parsed) if parsed.is_local(region) => Ok(named.to_string()),
        _ => Err(messages::invalid_policy_resource_arn(named)),
    }
}

/// Whether `resource_arn` names a Data Catalog ITSELF rather than a resource inside one:
/// its resource portion is [`CATALOG_ARN_RESOURCE`] alone, with no name after it.
///
/// This is what separates the ACCOUNT-LEVEL Data Catalog policy — the one a
/// `PutResourcePolicy` that names no `ResourceArn` sets — from a policy attached to an
/// individual resource, which is the distinction `GetResourcePolicies` enumerates by.
pub fn names_a_catalog(resource_arn: &str) -> bool {
    parse_glue_arn(resource_arn).is_some_and(|parsed| parsed.resource == CATALOG_ARN_RESOURCE)
}

/// A stored database, plus the tables it contains.
#[derive(Debug, Clone)]
pub struct DatabaseEntry {
    /// The folded name, which is also this entry's key in [`Catalog::databases`].
    pub name: String,
    pub description: Option<String>,
    pub location_uri: Option<String>,
    pub parameters: Option<HashMap<String, String>>,
    /// The Lake Formation default grants for tables created in this database. Always a
    /// definite list: the create either sent one (an empty list included) or the
    /// catalog's default from [`default_create_table_default_permissions`] was recorded.
    pub create_table_default_permissions: Vec<PrincipalPermissions>,
    pub target_database: Option<DatabaseIdentifier>,
    pub federated_database: Option<FederatedDatabase>,
    pub catalog_id: String,
    /// The opaque id the catalog assigned this database at create
    /// ([`new_database_id`]). Server-set and written once, so every later read of this
    /// database reports the same id and the tables it holds report it too — an update
    /// that renames the database does not change it, because it identifies the
    /// database rather than naming it.
    pub database_id: String,
    /// Server-set, written once at create so every later read reports the same instant.
    pub create_time: SystemTime,
    pub tags: HashMap<String, String>,
    /// Tables keyed by their folded (lowercased) name.
    pub tables: HashMap<String, TableEntry>,
    /// The key material the write that last stored this database was encrypted under, or
    /// `None` when that write was encrypted under no key of the caller's.
    ///
    /// Recorded per OBJECT because the catalog's setting applies "to every catalog write
    /// thereafter": a later change to it leaves this database encrypted under the key its
    /// own write used. See [`catalog_encryption`].
    pub encryption: Option<ObjectEncryption>,
}

impl DatabaseEntry {
    /// Whether this database can hold another table, i.e. is below
    /// [`MAX_TABLES_PER_DATABASE`].
    pub fn can_store_table(&self) -> bool {
        self.tables.len() < MAX_TABLES_PER_DATABASE
    }

    /// Build a stored database from the client's `DatabaseInput`.
    pub fn from_input(
        input: &DatabaseInput,
        catalog_id: &str,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: fold_name(&input.name),
            description: input.description.clone(),
            location_uri: input.location_uri.clone(),
            parameters: input.parameters.clone(),
            // Recorded at create, not filled in per read, so every later read of this
            // database reports the same grants.
            create_table_default_permissions: input
                .create_table_default_permissions
                .clone()
                .unwrap_or_else(default_create_table_default_permissions),
            target_database: input.target_database.clone(),
            federated_database: input.federated_database.clone(),
            catalog_id: catalog_id.to_string(),
            database_id: new_database_id(),
            create_time: now,
            tags: tags.cloned().unwrap_or_default(),
            tables: HashMap::new(),
            // The key material is not in any `DatabaseInput`: it is what the catalog's
            // own encryption setting had KMS produce for this write, so the handler
            // records it on the entry it has just built.
            encryption: None,
        }
    }

    /// Rebuild the modelled `Database` output shape from what is stored.
    pub fn to_database(&self) -> Database {
        Database {
            name: self.name.clone(),
            description: self.description.clone(),
            location_uri: self.location_uri.clone(),
            parameters: self.parameters.clone(),
            create_time: Some(to_date_time(self.create_time)),
            create_table_default_permissions: Some(
                self.create_table_default_permissions
                    .iter()
                    .map(tagged_principal_permissions)
                    .collect(),
            ),
            target_database: self.target_database.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            federated_database: self.federated_database.clone(),
            // The catalog attributes every database it returns to the catalog that owns
            // it and to the id it assigned the database, both reported DEFINITELY: a
            // client builds an ARN or correlates the database across Lake Formation and
            // Athena from these, so an absent member would leave it with nothing to key
            // on. Both come from what is stored, so repeated reads agree.
            catalog_identifier: Some(self.catalog_id.clone()),
            database_id: Some(self.database_id.clone()),
        }
    }

    /// Overwrite this database's definition with the one the client submitted.
    ///
    /// `DatabaseInput` is the WHOLE client-settable definition of a database — the
    /// same shape a create submits — so an update writes every member it carries and
    /// clears the ones it omits; what the client sent is exactly what the next read
    /// reports. The members no `DatabaseInput` carries are left alone, because the
    /// client cannot set them: `CatalogId` and `CreateTime` belong to the server, the
    /// tags are written through the tagging operations, and the tables this database
    /// owns are their own resources.
    ///
    /// The submitted `Name` is folded and written like any other member, so a request
    /// that submits a different one has MOVED the database: its tables' reported
    /// `DatabaseName` moves with it. Relocating the entry under its new key is the
    /// caller's job, since the key lives in [`Catalog::databases`].
    pub fn apply_input(&mut self, input: &DatabaseInput) {
        let folded = fold_name(&input.name);
        for table in self.tables.values_mut() {
            table.database_name = folded.clone();
        }
        self.name = folded;
        self.description = input.description.clone();
        self.location_uri = input.location_uri.clone();
        self.parameters = input.parameters.clone();
        self.create_table_default_permissions = input
            .create_table_default_permissions
            .clone()
            // An update that names no grants records the catalog's default, exactly as
            // a create does: the stored list is always definite, and an EMPTY list is
            // the opposite governance decision, so it is stored only when sent.
            .unwrap_or_else(default_create_table_default_permissions);
        self.target_database = input.target_database.clone();
        self.federated_database = input.federated_database.clone();
    }

    /// The stored tables ordered by folded name — the deterministic order
    /// `GetTables` pages over.
    pub fn tables_ordered(&self) -> Vec<&TableEntry> {
        let mut tables: Vec<&TableEntry> = self.tables.values().collect();
        tables.sort_by(|left, right| left.name.cmp(&right.name));
        tables
    }
}

/// A stored table inside its parent database.
#[derive(Debug, Clone)]
pub struct TableEntry {
    /// The folded name, which is also this entry's key in [`DatabaseEntry::tables`].
    pub name: String,
    pub database_name: String,
    pub catalog_id: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub table_type: Option<String>,
    pub retention: i32,
    pub parameters: Option<HashMap<String, String>>,
    pub storage_descriptor: Option<StorageDescriptor>,
    pub partition_keys: Option<Vec<Column>>,
    pub view_original_text: Option<String>,
    pub view_expanded_text: Option<String>,
    pub target_table: Option<TableIdentifier>,
    pub federated_table: Option<FederatedTable>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_access_time: Option<::aws_smithy_types::DateTime>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_analyzed_time: Option<::aws_smithy_types::DateTime>,
    /// Server-set, written once at create.
    pub create_time: SystemTime,
    /// Server-set; moves with every write to this table.
    pub update_time: SystemTime,
    /// Server-set version counter, [`FIRST_TABLE_VERSION_ID`] for a freshly created
    /// table.
    pub version_id: String,
    /// Server-set: the principal that issued the create.
    pub created_by: Option<String>,
    /// The versions of this table the catalog has ARCHIVED, keyed by the counter of the
    /// `VersionId` each one was current under.
    ///
    /// "By default, `UpdateTable` always creates an archived version of the table before
    /// updating it" (`UpdateTableRequest$SkipArchive`), so a write that moves
    /// [`Self::version_id`] leaves the definition it replaced in here. The CURRENT
    /// version is NOT in this map — it is the live definition above, which is itself the
    /// version [`Self::version_id`] names — so a table that has never been updated has
    /// exactly one version and an empty archive.
    pub archived_versions: BTreeMap<u64, ArchivedTableVersion>,
    /// The PARTITIONS of this table — "a slice of table data" (`Partition`) — keyed by
    /// the `Values` that address one.
    ///
    /// A partition lives on its table rather than beside the catalog tree because every
    /// operation that addresses one names its catalog, database and table and then the
    /// partition's values (`GetPartition$PartitionValues`), so the values identify a
    /// partition only WITHIN one table. The key is the value list exactly as sent: the
    /// model documents the Hive case fold for a database and a table name and for
    /// neither a partition key nor a partition value, and a value is table DATA, which a
    /// fold would rewrite.
    ///
    /// Ordered, so a reader of a table's partitions has one deterministic order to walk
    /// — lexicographic by value list, which for the partition-key order the values are
    /// passed in ("ordered in the same order as the partition keys",
    /// `PartitionInput$Values`) groups a table's partitions by their first key.
    pub partitions: BTreeMap<Vec<String>, PartitionEntry>,
    /// The key material the write that last stored this table's definition was encrypted
    /// under, or `None` when that write was encrypted under no key of the caller's. See
    /// [`DatabaseEntry::encryption`].
    pub encryption: Option<ObjectEncryption>,
    /// The COLUMN STATISTICS TASK SETTINGS of this table — "the settings for a column
    /// statistics task" (`ColumnStatisticsTaskSettings`) — or `None` while no create has
    /// written them.
    ///
    /// On the table rather than beside the catalog tree, exactly as a partition is:
    /// every operation that addresses one settings record names its catalog, its database
    /// and then its table (`GetColumnStatisticsTaskSettings$TableName`) and nothing else,
    /// so the record is addressed only WITHIN one table and there is at most one of it per
    /// table. That is also why a second create under the same table is the duplicate
    /// [`messages::COLUMN_STATISTICS_TASK_SETTINGS_ALREADY_EXIST`] reports rather than an
    /// overwrite, and why dropping the table drops its settings with it.
    pub column_statistics_task_settings: Option<ColumnStatisticsTaskSettingsEntry>,
}

/// Why a partition could not be created on a table.
///
/// One refusal reason per way a create can fail ON the table it addresses, so the
/// single-partition create and the batch create make the SAME decision: the batch
/// reports it as a per-item [`ErrorDetail`] inside `PartitionError`, where "the errors
/// encountered when trying to create the requested partitions" belong, and a
/// single-partition create raises the matching modelled exception named by
/// [`Self::error_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionCreateRefusal {
    /// The `PartitionInput` named no partition value at all. "Although this parameter is
    /// not required by the SDK, you must specify this parameter for a valid input"
    /// (`PartitionInput$Values`), so an input without values is not a partition the
    /// catalog can address.
    ValuesRequired,
    /// The values do not line up with the table's partition keys: a partition is one
    /// value per key of the table it belongs to, passed "in the same order as the
    /// partition keys" (`PartitionInput$Values`), so a list of another length cannot
    /// name a slice of this table.
    ValuesDoNotMatchPartitionKeys {
        /// How many partition keys the addressed table defines.
        keys: usize,
        /// How many values the input carried.
        values: usize,
    },
    /// The table already holds a partition with those values.
    AlreadyExists,
    /// The store cannot hold another partition ([`MAX_PARTITIONS_PER_TABLE`],
    /// [`MAX_STORED_PARTITIONS`]).
    LimitExceeded {
        /// The ceiling that was reached.
        limit: usize,
    },
}

impl PartitionCreateRefusal {
    /// The modelled error shape this refusal is reported under: the `ErrorCode` of a
    /// batch's per-item [`ErrorDetail`], and the exception a single-partition create
    /// raises.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ValuesRequired | Self::ValuesDoNotMatchPartitionKeys { .. } => {
                "InvalidInputException"
            }
            Self::AlreadyExists => "AlreadyExistsException",
            Self::LimitExceeded { .. } => "ResourceNumberLimitExceededException",
        }
    }

    /// This refusal's wording. See [`messages`] for how this crate's unrecorded
    /// refusals are worded.
    pub fn message(&self) -> String {
        match self {
            Self::ValuesRequired => messages::PARTITION_VALUES_REQUIRED.to_string(),
            Self::ValuesDoNotMatchPartitionKeys { keys, values } => {
                messages::partition_values_do_not_match_keys(*keys, *values)
            }
            Self::AlreadyExists => messages::PARTITION_ALREADY_EXISTS.to_string(),
            Self::LimitExceeded { limit } => {
                format!("the Data Catalog already holds the maximum of {limit} partitions")
            }
        }
    }

    /// This refusal as the per-item error shape a batch reports it in.
    pub fn to_error_detail(self) -> ErrorDetail {
        catalog_error_detail(self.error_code(), self.message())
    }
}

/// A stored partition of a table: the client-settable definition plus the one member
/// only the server sets, `CreationTime`.
#[derive(Debug, Clone)]
pub struct PartitionEntry {
    /// The values that address this partition, which are also this entry's key in
    /// [`TableEntry::partitions`].
    pub values: Vec<String>,
    pub storage_descriptor: Option<StorageDescriptor>,
    pub parameters: Option<HashMap<String, String>>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_access_time: Option<::aws_smithy_types::DateTime>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_analyzed_time: Option<::aws_smithy_types::DateTime>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub creation_time: SystemTime,
    /// The key material the write that stored this partition was encrypted under, or
    /// `None` when that write was encrypted under no key of the caller's. See
    /// [`DatabaseEntry::encryption`].
    pub encryption: Option<ObjectEncryption>,
}

impl PartitionEntry {
    /// Build a stored partition from the client's `PartitionInput` plus the server-set
    /// `CreationTime`, which no `PartitionInput` carries.
    ///
    /// `values` is passed separately because it is the entry's ADDRESS: the caller has
    /// already checked the input carries a value list that matches the table's partition
    /// keys, and that same list is the map key the entry is stored under.
    pub fn from_input(values: Vec<String>, input: &PartitionInput, now: SystemTime) -> Self {
        Self {
            values,
            storage_descriptor: input.storage_descriptor.clone(),
            parameters: input.parameters.clone(),
            last_access_time: input.last_access_time,
            last_analyzed_time: input.last_analyzed_time,
            creation_time: now,
            // Recorded by the create, which is where the catalog's encryption setting is
            // read and the key material produced (see [`TableEntry::create_partition`]).
            encryption: None,
        }
    }

    /// Rebuild the modelled `Partition` output shape from what is stored.
    ///
    /// `table` is the table this partition belongs to, read by the caller rather than
    /// copied onto the partition: the catalog, database and table a partition resides in
    /// are the table's own attribution, so there is ONE place they are stored and a
    /// partition read cannot report a database or table that has drifted from the
    /// table read's.
    pub fn to_partition(&self, table: &TableEntry) -> Partition {
        Partition {
            values: Some(self.values.clone()),
            database_name: Some(table.database_name.clone()),
            table_name: Some(table.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_access_time: self.last_access_time,
            // Projected the way a table's own storage is
            // ([`to_storage_descriptor`]), so a partition and its table report a
            // descriptor the same way.
            storage_descriptor: self.storage_descriptor.as_ref().map(to_storage_descriptor),
            parameters: self.parameters.clone(),
            last_analyzed_time: self.last_analyzed_time,
            catalog_id: Some(table.catalog_id.clone()),
        }
    }
}

/// One ARCHIVED version of a table: the definition the table carried at an earlier
/// `VersionId`, kept so a version read can still serve it once the live definition has
/// moved on.
#[derive(Debug, Clone)]
pub struct ArchivedTableVersion {
    /// The client-settable definition that was current at this version, in the shape a
    /// client submits it in — so a version read rebuilds exactly what that write stored
    /// and nothing a later write changed.
    pub definition: TableInput,
    /// The instant that definition became current: the `UpdateTime` this version
    /// reports.
    pub update_time: SystemTime,
    /// The key material the write that stored THIS definition was encrypted under, kept
    /// with the definition it belongs to: a later write re-keys the live table
    /// ([`TableEntry::encryption`]) and leaves the versions it archived under the keys
    /// their own writes used. See [`catalog_encryption`].
    pub encryption: Option<ObjectEncryption>,
}

impl TableEntry {
    /// Build a stored table from the client's `TableInput` plus the server-derived
    /// members (`DatabaseName`, `CatalogId`, `CreateTime`, `UpdateTime`, `VersionId`,
    /// `CreatedBy`), none of which exist on `TableInput`.
    pub fn from_input(
        input: &TableInput,
        database_name: &str,
        catalog_id: &str,
        created_by: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: fold_name(&input.name),
            database_name: database_name.to_string(),
            catalog_id: catalog_id.to_string(),
            description: input.description.clone(),
            owner: input.owner.clone(),
            table_type: input.table_type.clone(),
            retention: input.retention,
            parameters: input.parameters.clone(),
            storage_descriptor: input.storage_descriptor.clone(),
            partition_keys: input.partition_keys.clone(),
            view_original_text: input.view_original_text.clone(),
            view_expanded_text: input.view_expanded_text.clone(),
            target_table: input.target_table.clone(),
            federated_table: input.federated_table.clone(),
            last_access_time: input.last_access_time,
            last_analyzed_time: input.last_analyzed_time,
            create_time: now,
            update_time: now,
            version_id: FIRST_TABLE_VERSION_ID.to_string(),
            created_by,
            // A create archives nothing: it replaced no definition, and the one it
            // writes is the current version.
            archived_versions: BTreeMap::new(),
            // A table is created empty of partitions: `TableInput` defines the partition
            // KEYS of a table, never a slice of its data, so the first partition arrives
            // through a partition create.
            partitions: BTreeMap::new(),
            // Not part of any `TableInput`: the handler records what the catalog's own
            // encryption setting had KMS produce for this write.
            encryption: None,
            // `TableInput` carries no column statistics task settings either: they arrive
            // through `CreateColumnStatisticsTaskSettings`.
            column_statistics_task_settings: None,
        }
    }

    /// How many partition keys this table defines: the number of values that address one
    /// of its partitions.
    pub fn partition_key_count(&self) -> usize {
        self.partition_keys.as_ref().map_or(0, Vec::len)
    }

    /// The stored partition addressed by `values`, or `None` when this table holds no
    /// such partition.
    pub fn partition(&self, values: &[String]) -> Option<&PartitionEntry> {
        self.partitions.get(values)
    }

    /// Whether this table can hold another partition, i.e. is below
    /// [`MAX_PARTITIONS_PER_TABLE`].
    pub fn can_store_partition(&self) -> bool {
        self.partitions.len() < MAX_PARTITIONS_PER_TABLE
    }

    /// Create ONE partition of this table from a client's `PartitionInput`, or say why
    /// it cannot be created.
    ///
    /// The whole create decision for a single partition, in one place, so the
    /// single-partition create and each item of a batch create accept and refuse exactly
    /// the same inputs: a partition is one value per partition key of its table, it is
    /// addressed by those values, and a value list the table already holds a partition
    /// for is a duplicate rather than an overwrite.
    ///
    /// `store_has_room` is whether the STORE as a whole can hold another partition
    /// ([`ComAmazonawsGlueAwsglueState::can_store_partition`]), which this table cannot
    /// see from here; the per-table ceiling is checked here.
    ///
    /// `encryption` is the key material this write is encrypted under, which the caller
    /// resolved from the catalog's own settings before it took the store's guard: a
    /// partition created while the catalog encrypts its writes under a KMS key is recorded
    /// under that key, and a read of it has to recover that key material first (see
    /// [`catalog_encryption`]).
    pub fn create_partition(
        &mut self,
        input: &PartitionInput,
        now: SystemTime,
        store_has_room: bool,
        encryption: Option<ObjectEncryption>,
    ) -> Result<&PartitionEntry, PartitionCreateRefusal> {
        let values = input.values.clone().unwrap_or_default();
        if values.is_empty() {
            return Err(PartitionCreateRefusal::ValuesRequired);
        }
        let keys = self.partition_key_count();
        if values.len() != keys {
            return Err(PartitionCreateRefusal::ValuesDoNotMatchPartitionKeys {
                keys,
                values: values.len(),
            });
        }
        if self.partitions.contains_key(&values) {
            return Err(PartitionCreateRefusal::AlreadyExists);
        }
        if !self.can_store_partition() {
            return Err(PartitionCreateRefusal::LimitExceeded {
                limit: MAX_PARTITIONS_PER_TABLE,
            });
        }
        if !store_has_room {
            return Err(PartitionCreateRefusal::LimitExceeded {
                limit: MAX_STORED_PARTITIONS,
            });
        }
        let mut entry = PartitionEntry::from_input(values.clone(), input, now);
        // The key material this write was encrypted under, which the caller read from the
        // catalog's settings and had KMS produce before taking the store's guard.
        entry.encryption = encryption;
        Ok(self.partitions.entry(values).or_insert(entry))
    }

    /// This table's client-settable definition, in the `TableInput` shape a client
    /// submits it in — the inverse of [`Self::from_input`], and what archiving a version
    /// keeps of the definition a write is about to replace.
    ///
    /// `ViewDefinition` is absent because no stored table carries one: `CreateTable` and
    /// `UpdateTable` refuse a view definition rather than storing it.
    pub fn to_table_input(&self) -> TableInput {
        TableInput {
            name: self.name.clone(),
            description: self.description.clone(),
            owner: self.owner.clone(),
            last_access_time: self.last_access_time,
            last_analyzed_time: self.last_analyzed_time,
            retention: self.retention,
            storage_descriptor: self.storage_descriptor.clone(),
            partition_keys: self.partition_keys.clone(),
            view_original_text: self.view_original_text.clone(),
            view_expanded_text: self.view_expanded_text.clone(),
            table_type: self.table_type.clone(),
            parameters: self.parameters.clone(),
            target_table: self.target_table.clone(),
            federated_table: self.federated_table.clone(),
            view_definition: None,
        }
    }

    /// Whether this table can archive another version, i.e. holds fewer than
    /// [`MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE`].
    pub fn can_archive_version(&self) -> bool {
        self.archived_versions.len() < MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE
    }

    /// This table as of one of its versions, as the modelled `TableVersion` shape, or
    /// `None` when the catalog holds no such version of it.
    ///
    /// `version` is the counter a request named, or `None` for a request that named no
    /// version, which addresses the table's current one. The current definition IS a
    /// version — the one [`Self::version_id`] names — so a version read of a table that
    /// has never been updated reports exactly what a plain read of the table reports.
    /// Every earlier version comes from [`Self::archived_versions`] and is projected
    /// through the same `Table` builder: the definition that was current then, with the
    /// members that belong to the table rather than to one of its versions
    /// (`CreateTime`, `CreatedBy`, `CatalogId`, `DatabaseName`) as they stand, and
    /// `UpdateTime` the instant that version became current.
    pub fn table_version(&self, version: Option<u64>, database_id: &str) -> Option<TableVersion> {
        let Some(wanted) = self.archived_version_wanted(version) else {
            return Some(TableVersion {
                table: Some(self.to_table(database_id)),
                version_id: Some(self.version_id.clone()),
            });
        };
        let archived = self.archived_versions.get(&wanted)?;
        let mut historical = Self::from_input(
            &archived.definition,
            &self.database_name,
            &self.catalog_id,
            self.created_by.clone(),
            archived.update_time,
        );
        // `from_input` dates a FRESH table, so `CreateTime` — which belongs to the
        // table and not to one of its versions — is restored from the live record, and
        // the version id is the one this archived definition is keyed by.
        historical.create_time = self.create_time;
        historical.version_id = wanted.to_string();
        Some(TableVersion {
            table: Some(historical.to_table(database_id)),
            version_id: Some(wanted.to_string()),
        })
    }

    /// Which ARCHIVED version a version read addresses, or `None` when it addresses the
    /// live definition: a request that names no version, or names the one
    /// [`Self::version_id`] is under, addresses the current version.
    ///
    /// The one selection both [`Self::table_version`] and [`Self::version_encryption`] go
    /// through, so the definition a version read answers and the key material it is
    /// answered through can never come from different versions.
    fn archived_version_wanted(&self, version: Option<u64>) -> Option<u64> {
        match version {
            None => None,
            Some(wanted) if Some(wanted) == parse_version_counter(&self.version_id) => None,
            Some(wanted) => Some(wanted),
        }
    }

    /// The key material the write that stored the definition of ONE of this table's
    /// versions was encrypted under — the live record's for the current version, and the
    /// archived record's own for an earlier one.
    ///
    /// `None` is a version whose write was encrypted under no key of the caller's, and
    /// also a version this table does not hold, which [`Self::table_version`] reports as
    /// the miss it is.
    pub fn version_encryption(&self, version: Option<u64>) -> Option<ObjectEncryption> {
        match self.archived_version_wanted(version) {
            None => self.encryption.clone(),
            Some(wanted) => self
                .archived_versions
                .get(&wanted)
                .and_then(|archived| archived.encryption.clone()),
        }
    }

    /// Overwrite this table's definition with the one the client submitted.
    ///
    /// `TableInput` is the WHOLE client-settable definition of a table — the same
    /// shape a create submits — so an update writes every member it carries and clears
    /// the ones it omits; what the client sent is exactly what the next read reports.
    /// The server-set members keep their own history: `CreateTime`, `CreatedBy`,
    /// `CatalogId` and `DatabaseName` stay as the create left them, `UpdateTime` moves
    /// to this write's instant, and `VersionId` advances to the next version, because
    /// the update produced a new version of the table.
    ///
    /// `TableInput$ViewDefinition` is NOT carried here: this slice stores no view
    /// representations, so an update that submits one is refused by the handler rather
    /// than accepted and dropped.
    ///
    /// The submitted `Name` is folded and written like any other member, but it can
    /// only ever be the name this table already has: `UpdateTable` refuses a request
    /// whose `TableInput$Name` is not the table it addresses, so the entry's key in
    /// [`DatabaseEntry::tables`] stays correct. A caller that ever applies an input
    /// under a different name must relocate the entry under its new key, the way
    /// `UpdateDatabase` does for [`DatabaseEntry::apply_input`].
    ///
    /// `archive_previous` keeps the definition this write replaces as the version it was
    /// current under ([`Self::archived_versions`]), which is what a later version read
    /// serves; the caller passes `false` for a write that asks not to archive it.
    pub fn apply_input(&mut self, input: &TableInput, now: SystemTime, archive_previous: bool) {
        if archive_previous {
            if let Some(current) = parse_version_counter(&self.version_id) {
                self.archived_versions.insert(
                    current,
                    ArchivedTableVersion {
                        definition: self.to_table_input(),
                        update_time: self.update_time,
                        // The definition being archived was encrypted by the write that
                        // stored it, so its key material is archived with it rather than
                        // left to the key this write is about to record on the table.
                        encryption: self.encryption.clone(),
                    },
                );
            }
        }
        self.name = fold_name(&input.name);
        self.description = input.description.clone();
        self.owner = input.owner.clone();
        self.table_type = input.table_type.clone();
        self.retention = input.retention;
        self.parameters = input.parameters.clone();
        self.storage_descriptor = input.storage_descriptor.clone();
        self.partition_keys = input.partition_keys.clone();
        self.view_original_text = input.view_original_text.clone();
        self.view_expanded_text = input.view_expanded_text.clone();
        self.target_table = input.target_table.clone();
        self.federated_table = input.federated_table.clone();
        self.last_access_time = input.last_access_time;
        self.last_analyzed_time = input.last_analyzed_time;
        self.update_time = now;
        self.version_id = next_version_id(&self.version_id);
    }

    /// Rebuild the modelled `Table` output shape from what is stored.
    ///
    /// `database_id` is the catalog-assigned id of the database this table belongs to,
    /// read from that [`DatabaseEntry`] by the caller rather than copied onto the table:
    /// the id belongs to the database, so there is one place it is stored and the table
    /// read cannot report an id that has drifted from the database read's.
    pub fn to_table(&self, database_id: &str) -> Table {
        // `Table$RLSProperties` is "the default row-level security policy of the table".
        // No stored table carries one, because no `TableInput` can set one — so the
        // catalog holds no row filter for any table it serves. Bound once here, because
        // `IsRowFilteringEnabled` below is derived from it: the flag says exactly whether
        // such a policy is in force, so the two can never report different things.
        let rls_properties: Option<RLSProperties> = None;
        Table {
            name: self.name.clone(),
            database_name: Some(self.database_name.clone()),
            description: self.description.clone(),
            owner: self.owner.clone(),
            create_time: Some(to_date_time(self.create_time)),
            update_time: Some(to_date_time(self.update_time)),
            last_access_time: self.last_access_time,
            last_analyzed_time: self.last_analyzed_time,
            retention: self.retention,
            storage_descriptor: self.storage_descriptor.as_ref().map(to_storage_descriptor),
            partition_keys: self.partition_keys.clone(),
            view_original_text: self.view_original_text.clone(),
            view_expanded_text: self.view_expanded_text.clone(),
            table_type: self.table_type.clone(),
            parameters: self.parameters.clone(),
            created_by: self.created_by.clone(),
            is_registered_with_lake_formation: false,
            target_table: self.target_table.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            version_id: Some(self.version_id.clone()),
            federated_table: self.federated_table.clone(),
            // `Table$ViewDefinition` is "all the information that defines the view": no
            // stored table has one, because `CreateTable` REFUSES a `ViewDefinition`
            // rather than dropping it (see `ops/create_table.rs`).
            view_definition: None,
            // `IsMultiDialectView` and `IsMaterializedView` are three-state
            // `NullableBoolean` members the catalog answers DEFINITELY on every read: a
            // table is one of them exactly when the catalog holds the matching view
            // definition for it. No stored table does (see `view_definition` above), so
            // both are `false` — reported rather than omitted, so a client branching on
            // three states never reads "unknown" for a table whose kind the catalog
            // knows.
            is_multi_dialect_view: Some(false),
            is_materialized_view: Some(false),
            iceberg_table_metadata: None,
            status: None,
            // The catalog attributes every table it returns to the catalog that owns it
            // and to the database that holds it, by the ids it assigned them — the same
            // `DatabaseId` the database's own read reports, so the two responses agree.
            catalog_identifier: Some(self.catalog_id.clone()),
            database_id: Some(database_id.to_string()),
            // Row filtering is on exactly when the catalog holds a row-level security
            // policy for the table, so the flag is derived from `rls_properties` rather
            // than restated: `false` for every stored table today, and DEFINITELY false
            // rather than omitted, so a client that distinguishes "no filter" from
            // "unknown" reads the first — which is what the catalog knows.
            is_row_filtering_enabled: Some(rls_properties.is_some()),
            rls_properties,
        }
    }
}

/// Project a stored `StorageDescriptor` the way the Data Catalog reports one.
///
/// `StorageDescriptor$SortColumns` is "a list specifying the sort order of each bucket in
/// the table" — part of the table definition the catalog holds, so a table that defines
/// no sort order has an EMPTY sort order rather than an unknown one, and the catalog
/// reports `[]`. A client that iterates it then loops zero times instead of having to
/// handle an absent member. Every other member is reported exactly as it was stored: a
/// member the client never sent is one the catalog does not determine, so it stays
/// absent.
pub fn to_storage_descriptor(stored: &StorageDescriptor) -> StorageDescriptor {
    StorageDescriptor {
        sort_columns: Some(stored.sort_columns.clone().unwrap_or_default()),
        ..stored.clone()
    }
}

/// A stored crawler.
///
/// Every member of the modelled `Crawler` shape that a create can fill is held here, so
/// a later `GetCrawler` reports what the create stored and nothing else: the members
/// only a crawl produces (`LastCrawl`, `CrawlElapsedTime`) are not stored, because no
/// crawl has run.
#[derive(Debug, Clone)]
pub struct CrawlerEntry {
    /// The crawler's name, which is also its key in [`ComAmazonawsGlueAwsglueState::crawlers`].
    pub name: String,
    pub role: String,
    pub targets: CrawlerTargets,
    pub database_name: Option<String>,
    pub description: Option<String>,
    /// The custom classifiers associated with the crawler; empty when the create named
    /// none, which is the accurate report for a crawler that has no custom classifier.
    pub classifiers: Vec<String>,
    pub table_prefix: Option<String>,
    pub schema_change_policy: Option<SchemaChangePolicy>,
    pub recrawl_policy: Option<RecrawlPolicy>,
    pub lineage_configuration: Option<LineageConfiguration>,
    pub lake_formation_configuration: Option<LakeFormationConfiguration>,
    pub configuration: Option<String>,
    pub crawler_security_configuration: Option<String>,
    /// The cron expression the create supplied, if any. Reported inside a `Schedule`.
    pub schedule_expression: Option<String>,
    /// The catalog the crawler writes its output to.
    pub catalog_id: String,
    /// One of the modelled `CrawlerState` values; [`CRAWLER_STATE_READY`] at create.
    pub state: String,
    /// Server-set version counter, [`FIRST_CRAWLER_VERSION`] for a fresh crawler.
    pub version: i64,
    /// Server-set, written once at create so every later read reports the same instant.
    pub creation_time: SystemTime,
    /// Server-set; moves with every write to this crawler.
    pub last_updated: SystemTime,
    /// The request's `Tags`, which are not part of the `Crawler` shape — they are read
    /// back through the tagging operations.
    pub tags: HashMap<String, String>,
}

impl CrawlerEntry {
    /// Build a stored crawler from a `CreateCrawler` request plus the server-derived
    /// members (`State`, `Version`, `CreationTime`, `LastUpdated`), none of which the
    /// client may set.
    pub fn from_create_input(
        input: &CreateCrawlerInput,
        catalog_id: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            name: input.name.clone(),
            role: input.role.clone(),
            targets: input.targets.clone(),
            database_name: input.database_name.clone(),
            description: input.description.clone(),
            classifiers: input.classifiers.clone().unwrap_or_default(),
            table_prefix: input.table_prefix.clone(),
            schema_change_policy: input.schema_change_policy.clone(),
            recrawl_policy: input.recrawl_policy.clone(),
            lineage_configuration: input.lineage_configuration.clone(),
            lake_formation_configuration: input.lake_formation_configuration.clone(),
            configuration: input.configuration.clone(),
            crawler_security_configuration: input.crawler_security_configuration.clone(),
            schedule_expression: input.schedule.clone(),
            catalog_id: catalog_id.to_string(),
            state: CRAWLER_STATE_READY.to_string(),
            version: FIRST_CRAWLER_VERSION,
            creation_time: now,
            last_updated: now,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Apply an `UpdateCrawler` request to this stored crawler, member by member.
    ///
    /// Every member the request CARRIES replaces the stored one; every member it OMITS
    /// is preserved. That is what the model states for the one member whose preservation
    /// it spells out — "if you omit this value, the existing value on the crawler is
    /// preserved" (`UpdateCrawlerRequest$CatalogId`) — and what the update's own
    /// `Description` requires: it targets the REMOVABLE description shape where the
    /// create targets the plain one, so sending an empty description is how a caller
    /// takes the description OFF a crawler, a distinction that exists only because
    /// omitting a member leaves it alone.
    ///
    /// `catalog_id` is the catalog the request named, already resolved and checked by the
    /// handler, or `None` when the request named none.
    ///
    /// The server-set members move with the write: `Version` advances by one (the first
    /// version of a crawler is [`FIRST_CRAWLER_VERSION`] and an update moves it on) and
    /// `LastUpdated` becomes `now`. The crawler's `Name`, `CreationTime`, `State` and
    /// tags are not this request's to change: `UpdateCrawlerRequest` has no member for
    /// any of them.
    pub fn apply_update(
        &mut self,
        input: &UpdateCrawlerInput,
        catalog_id: Option<String>,
        now: SystemTime,
    ) {
        if let Some(role) = input.role.as_ref() {
            self.role = role.clone();
        }
        if let Some(targets) = input.targets.as_ref() {
            self.targets = targets.clone();
        }
        if let Some(database_name) = input.database_name.as_ref() {
            self.database_name = Some(database_name.clone());
        }
        // The removable description: empty takes it off the crawler rather than storing
        // a description that is the empty string.
        if let Some(description) = input.description.as_ref() {
            self.description = (!description.is_empty()).then(|| description.clone());
        }
        if let Some(classifiers) = input.classifiers.as_ref() {
            self.classifiers = classifiers.clone();
        }
        if let Some(table_prefix) = input.table_prefix.as_ref() {
            self.table_prefix = Some(table_prefix.clone());
        }
        if let Some(schema_change_policy) = input.schema_change_policy.as_ref() {
            self.schema_change_policy = Some(schema_change_policy.clone());
        }
        if let Some(recrawl_policy) = input.recrawl_policy.as_ref() {
            self.recrawl_policy = Some(recrawl_policy.clone());
        }
        if let Some(lineage_configuration) = input.lineage_configuration.as_ref() {
            self.lineage_configuration = Some(lineage_configuration.clone());
        }
        if let Some(lake_formation_configuration) = input.lake_formation_configuration.as_ref() {
            self.lake_formation_configuration = Some(lake_formation_configuration.clone());
        }
        if let Some(configuration) = input.configuration.as_ref() {
            self.configuration = Some(configuration.clone());
        }
        if let Some(security_configuration) = input.crawler_security_configuration.as_ref() {
            self.crawler_security_configuration = Some(security_configuration.clone());
        }
        if let Some(schedule) = input.schedule.as_ref() {
            self.schedule_expression = Some(schedule.clone());
        }
        if let Some(catalog_id) = catalog_id {
            self.catalog_id = catalog_id;
        }
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// Apply an `UpdateCrawlerSchedule` request's schedule to this stored crawler.
    ///
    /// That request STATES the crawler's schedule rather than merging into it: the cron
    /// expression it carries becomes the crawler's schedule, and a request that carries
    /// NONE leaves the crawler with no schedule at all. `CrawlerName` only ADDRESSES the
    /// crawler, so the schedule is the one thing the request says anything about and its
    /// absence can only mean "no schedule" — which is also the one way a caller can take
    /// a schedule off a crawler, since `UpdateCrawler`'s own `Schedule` is a plain
    /// expression whose absence means "leave the crawler alone".
    ///
    /// The write moves the server-set members exactly as [`Self::apply_update`] does,
    /// because it writes the same member of the same crawler definition: `Version`
    /// advances by one and `LastUpdated` becomes `now`. Nothing else about the crawler is
    /// this request's to change.
    pub fn apply_schedule(&mut self, schedule: Option<&str>, now: SystemTime) {
        self.schedule_expression = schedule.map(str::to_string);
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// Rebuild the modelled `Crawler` output shape from what is stored.
    pub fn to_crawler(&self) -> Crawler {
        Crawler {
            name: Some(self.name.clone()),
            role: Some(self.role.clone()),
            targets: Some(self.targets.clone()),
            database_name: self.database_name.clone(),
            description: self.description.clone(),
            classifiers: Some(self.classifiers.clone()),
            recrawl_policy: self.recrawl_policy.clone(),
            schema_change_policy: self.schema_change_policy.clone(),
            lineage_configuration: self.lineage_configuration.clone(),
            state: Some(self.state.clone()),
            table_prefix: self.table_prefix.clone(),
            schedule: self.schedule_expression.as_ref().map(|expression| Schedule {
                schedule_expression: Some(expression.clone()),
                state: Some(SCHEDULE_STATE_SCHEDULED.to_string()),
            }),
            // "If the crawler is running, contains the total time elapsed since the
            // last crawl began" — nothing has run, so no time has elapsed.
            crawl_elapsed_time: 0,
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated: Some(to_date_time(self.last_updated)),
            // "Status and error information about the most recent crawl": absent until
            // a crawl has produced one.
            last_crawl: None,
            version: self.version,
            configuration: self.configuration.clone(),
            crawler_security_configuration: self.crawler_security_configuration.clone(),
            lake_formation_configuration: self.lake_formation_configuration.clone(),
            catalog_id: Some(self.catalog_id.clone()),
        }
    }
}

/// A stored trigger: the `Trigger` every read projects, plus the `Tags` the create
/// carried, which that shape does not hold.
///
/// Every member of the modelled `Trigger` shape is held here except `Id`, which the model
/// states is "Reserved for future use" and which therefore has no value to report. The one
/// member the server sets is [`Self::state`]: a client names a trigger's type, schedule,
/// actions and predicate, and whether the create should start it, and the service decides
/// the state those leave it in.
///
/// Tags are kept on the record so they are not lost by the create, as a crawler's and a
/// custom entity type's are; [`TaggedResourceKind`] carries no trigger variant yet, so
/// resolving one through the tagging operations belongs to those operations' own issues.
#[derive(Debug, Clone)]
pub struct TriggerEntry {
    /// `Trigger$Name`, which is also this entry's key in its Region's map inside
    /// [`ComAmazonawsGlueAwsglueState::triggers`].
    pub name: String,
    /// `Trigger$Type`: one of the modelled `TriggerType` values, as the create sent it.
    pub trigger_type: String,
    /// `Trigger$State`: one of the modelled `TriggerState` values, decided by the service
    /// rather than sent — [`TRIGGER_STATE_ACTIVATED`] when the create asked for the
    /// trigger to be started, else [`TRIGGER_STATE_CREATED`].
    pub state: String,
    /// `Trigger$WorkflowName`: "The name of the workflow associated with the trigger",
    /// stored as sent.
    pub workflow_name: Option<String>,
    pub description: Option<String>,
    /// `Trigger$Schedule`: the cron expression the create supplied, if any.
    pub schedule: Option<String>,
    /// `Trigger$Actions`: "The actions initiated by this trigger."
    pub actions: Vec<Action>,
    /// `Trigger$Predicate`: "The predicate of this trigger, which defines when it will
    /// fire", absent for a trigger created without one.
    pub predicate: Option<Predicate>,
    pub event_batching_condition: Option<EventBatchingCondition>,
    /// The request's `Tags`, which are not part of the `Trigger` shape.
    pub tags: HashMap<String, String>,
}

impl TriggerEntry {
    /// Build a stored trigger from a `CreateTrigger` request, in the state the create
    /// leaves it in.
    ///
    /// `state` is that state, decided by the handler from the request's `StartOnCreation`
    /// and `Type` — the one member of the record the client does not set.
    pub fn from_create_input(input: &CreateTriggerInput, state: &str) -> Self {
        Self {
            name: input.name.clone(),
            trigger_type: input.r#type.clone(),
            state: state.to_string(),
            workflow_name: input.workflow_name.clone(),
            description: input.description.clone(),
            schedule: input.schedule.clone(),
            actions: input.actions.clone(),
            predicate: input.predicate.clone(),
            event_batching_condition: input.event_batching_condition.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `Trigger` output shape from what is stored.
    pub fn to_trigger(&self) -> Trigger {
        Trigger {
            name: Some(self.name.clone()),
            workflow_name: self.workflow_name.clone(),
            // "Reserved for future use": a trigger carries no id of its own, so there is
            // none to report.
            id: None,
            r#type: Some(self.trigger_type.clone()),
            state: Some(self.state.clone()),
            description: self.description.clone(),
            schedule: self.schedule.clone(),
            actions: Some(self.actions.clone()),
            predicate: self.predicate.clone(),
            event_batching_condition: self.event_batching_condition.clone(),
        }
    }
}

/// One Glue entity a trigger's definition names: the workflow it belongs to, the job an
/// action runs or a condition waits on, or the crawler of either.
///
/// Returned by [`missing_trigger_reference`], so a refusal can say which kind of entity it
/// could not find and under which name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerReference<'a> {
    /// `Action$JobName` or `Condition$JobName`.
    Job(&'a str),
    /// `Action$CrawlerName` or `Condition$CrawlerName`.
    Crawler(&'a str),
    /// `CreateTriggerRequest$WorkflowName`.
    Workflow(&'a str),
}

/// The first workflow, job or crawler a trigger's definition names that this store holds
/// no record of, or `None` when every one of them is a stored entity.
///
/// A trigger belongs to the workflow it names and runs the actions and waits on the
/// conditions it carries, so the definition is resolvable only if those entities exist;
/// `region` is the Region the request is served in, which is where its job definitions
/// live, while a workflow and a crawler live in one per-account namespace. An
/// `Action$SecurityConfiguration` is NOT resolved here: it configures how an action runs
/// rather than naming the entity the action initiates.
///
/// The walk visits each action once and each condition once — the conditions bounded by
/// the modelled `ConditionList` length of at most 500, the actions by the request body the
/// transport accepted — and every lookup is a map probe.
pub fn missing_trigger_reference<'a>(
    store: &ComAmazonawsGlueAwsglueState,
    region: &str,
    workflow_name: Option<&'a str>,
    actions: &'a [Action],
    predicate: Option<&'a Predicate>,
) -> Option<TriggerReference<'a>> {
    if let Some(workflow_name) = workflow_name {
        if store.workflow(workflow_name).is_none() {
            return Some(TriggerReference::Workflow(workflow_name));
        }
    }
    let action_targets = actions
        .iter()
        .map(|action| (action.job_name.as_deref(), action.crawler_name.as_deref()));
    let condition_targets = predicate
        .and_then(|predicate| predicate.conditions.as_ref())
        .into_iter()
        .flatten()
        .map(|condition| {
            (
                condition.job_name.as_deref(),
                condition.crawler_name.as_deref(),
            )
        });
    for (job_name, crawler_name) in action_targets.chain(condition_targets) {
        if let Some(job_name) = job_name {
            if store.job(region, job_name).is_none() {
                return Some(TriggerReference::Job(job_name));
            }
        }
        if let Some(crawler_name) = crawler_name {
            if store.crawler(crawler_name).is_none() {
                return Some(TriggerReference::Crawler(crawler_name));
            }
        }
    }
    None
}

/// A stored custom entity type: the custom pattern `CreateCustomEntityType` registers
/// for detecting sensitive data.
///
/// Every member of the modelled `CustomEntityType` shape is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports the pattern the
/// create stored and nothing the server invented. There is no server-set member: the
/// model gives a custom entity type no id, status or timestamp.
#[derive(Debug, Clone)]
pub struct CustomEntityTypeEntry {
    /// The pattern's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::custom_entity_types`].
    pub name: String,
    /// "A regular expression string that is used for detecting sensitive data in a custom
    /// pattern", stored exactly as sent.
    pub regex_string: String,
    /// The context words the create supplied, if any. "If no context words are passed only
    /// a regular expression is checked", so a create that named none is stored with NO
    /// list rather than with an empty one: the two are different patterns, and the
    /// modelled list is 1..20 long, so an empty list is not even a value a create can send.
    pub context_words: Option<Vec<String>>,
    /// The create's `Tags`, which are not part of the `CustomEntityType` shape.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations: [`TaggedResourceKind`] carries no custom entity
    /// type variant, because the model states no ARN resource-type segment for this kind
    /// (nothing in it spells one), and the segment is what every tag lookup resolves an
    /// ARN against. So a tagging call naming a custom entity type still resolves to
    /// nothing, and these tags are stored rather than served.
    pub tags: HashMap<String, String>,
}

impl CustomEntityTypeEntry {
    /// Build a stored custom entity type from a `CreateCustomEntityType` request. Every
    /// member comes from the request: the client supplies the whole definition.
    pub fn from_create_input(input: &CreateCustomEntityTypeInput) -> Self {
        Self {
            name: input.name.clone(),
            regex_string: input.regex_string.clone(),
            context_words: input.context_words.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `CustomEntityType` shape from what is stored, so a listing
    /// reports the same pattern a single read does. The stored `Tags` are not part of
    /// that shape, so they stay out of it.
    pub fn to_custom_entity_type(&self) -> CustomEntityType {
        CustomEntityType {
            name: self.name.clone(),
            regex_string: self.regex_string.clone(),
            context_words: self.context_words.clone(),
        }
    }
}

/// A stored security configuration: the set of encryption properties
/// `CreateSecurityConfiguration` registers for Glue to encrypt data at rest with.
///
/// Every member of the modelled `SecurityConfiguration` shape is held here and nothing
/// else: the model gives a security configuration no id, status or owner. The name and the
/// encryption configuration come from the request; the creation instant is the one
/// server-set member, recorded when the create stored the record and never changed
/// afterwards, because the model exposes no update for this resource.
#[derive(Debug, Clone)]
pub struct SecurityConfigurationEntry {
    /// The configuration's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::security_configurations`].
    pub name: String,
    /// "The time at which this security configuration was created", the instant the create
    /// was served, so every later read of this record reports the same timestamp.
    pub created_at: SystemTime,
    /// "The encryption configuration associated with this security configuration", stored
    /// exactly as the create sent it: a read reports the properties the client wrote and
    /// nothing the server invented.
    pub encryption_configuration: EncryptionConfiguration,
}

impl SecurityConfigurationEntry {
    /// Build a stored security configuration from a `CreateSecurityConfiguration` request
    /// served at `now`. The definition comes from the request; the creation instant is the
    /// clock's.
    pub fn from_create_input(input: &CreateSecurityConfigurationInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            created_at: now,
            encryption_configuration: input.encryption_configuration.clone(),
        }
    }

    /// Rebuild the modelled `SecurityConfiguration` shape from what is stored, so a
    /// listing reports the same configuration a single read does.
    pub fn to_security_configuration(&self) -> SecurityConfiguration {
        SecurityConfiguration {
            name: Some(self.name.clone()),
            created_time_stamp: Some(to_date_time(self.created_at)),
            encryption_configuration: Some(self.encryption_configuration.clone()),
        }
    }
}

/// One usage profile: the job and session parameter set `CreateUsageProfile` registered
/// under a name, plus the two instants the SERVER stamps on it.
///
/// Every member of the modelled `GetUsageProfileResponse` is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports the profile the write
/// stored and nothing invented on the way out.
#[derive(Debug, Clone)]
pub struct UsageProfileEntry {
    /// The profile's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::usage_profiles`].
    pub name: String,
    /// "A description of the usage profile", or `None` when the write named none.
    pub description: Option<String>,
    /// "A `ProfileConfiguration` object specifying the job and session values for the
    /// profile", stored exactly as the write sent it.
    ///
    /// Each of its two members is a map that is stored as sent, so an EMPTY map and an
    /// ABSENT one stay different: an empty `JobConfiguration` is a profile that configures
    /// no job parameter, while an absent one is a profile that does not speak about job
    /// parameters at all, and a read reports back whichever the write carried.
    pub configuration: ProfileConfiguration,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this profile.
    pub last_modified_on: SystemTime,
    /// The create's `Tags`, which the `GetUsageProfile` response has no member for.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`]
    /// gives: [`TaggedResourceKind`] carries no usage profile variant.
    pub tags: HashMap<String, String>,
}

impl UsageProfileEntry {
    /// Build a stored usage profile from a `CreateUsageProfile` request plus the instant
    /// the create was served at, which is both its create and its last-modified instant.
    pub fn from_create_input(input: &CreateUsageProfileInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            description: input.description.clone(),
            configuration: input.configuration.clone(),
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }
}

/// Why the service refuses a submitted `ProfileConfiguration`, as the `Message` of the
/// refusal, or `None` when every parameter it configures is coherent.
///
/// A `ConfigurationObject` states one parameter's `DefaultValue` together with the
/// `MinValue`/`MaxValue` range it may take ("Specifies the values that an admin sets for
/// each job or session parameter configured in a Glue usage profile"), so those three have
/// to agree: a default outside the range, or a minimum above the maximum, describes a
/// parameter NO value satisfies, and such a write is refused rather than stored.
///
/// Only values that are NUMBERS are ordered ([`numeric_configuration_value`]): a parameter
/// whose values are names — a worker type, a boolean — carries no range to compare, and
/// what it admits is its `AllowedValues` list rather than a bound.
///
/// The walk is in a fixed order — job parameters before session ones, and by parameter
/// name within each map, whose own iteration order is not the request's — so a
/// configuration with several incoherent parameters is always refused for the same one.
/// Within one parameter the default is checked against the maximum first, then against
/// the minimum, then the two bounds against each other, which is the order the service
/// reports a parameter breaking more than one of them in.
pub fn usage_profile_configuration_defect(configuration: &ProfileConfiguration) -> Option<String> {
    for (section, parameters) in [
        ("JobConfiguration", configuration.job_configuration.as_ref()),
        (
            "SessionConfiguration",
            configuration.session_configuration.as_ref(),
        ),
    ] {
        let Some(parameters) = parameters else {
            continue;
        };
        let mut names: Vec<&String> = parameters.keys().collect();
        names.sort();
        for name in names {
            let Some(object) = parameters.get(name) else {
                continue;
            };
            if let Some(message) = configuration_object_defect(section, name, object) {
                return Some(message);
            }
        }
    }
    None
}

/// The incoherence in ONE parameter's `ConfigurationObject`, as the message it is refused
/// with. See [`usage_profile_configuration_defect`] for the rule and the order.
///
/// The bound each message carries is the TEXT the request sent, not the number it parsed
/// to, so the refusal quotes the value the client wrote.
fn configuration_object_defect(
    section: &str,
    parameter: &str,
    object: &ConfigurationObject,
) -> Option<String> {
    let default = object
        .default_value
        .as_deref()
        .and_then(numeric_configuration_value);
    let minimum = object
        .min_value
        .as_deref()
        .and_then(numeric_configuration_value)
        .zip(object.min_value.as_deref());
    let maximum = object
        .max_value
        .as_deref()
        .and_then(numeric_configuration_value)
        .zip(object.max_value.as_deref());

    if let Some((default, (maximum, text))) = default.zip(maximum) {
        if default > maximum {
            return Some(messages::configuration_default_above_max(
                parameter, section, text,
            ));
        }
    }
    if let Some((default, (minimum, text))) = default.zip(minimum) {
        if default < minimum {
            return Some(messages::configuration_default_below_min(
                parameter, section, text,
            ));
        }
    }
    if let Some(((minimum, _), (maximum, text))) = minimum.zip(maximum) {
        if minimum > maximum {
            return Some(messages::configuration_min_above_max(
                parameter, section, text,
            ));
        }
    }
    None
}

/// The number a `ConfigurationObject` bound states, or `None` when the value is not one.
///
/// `ConfigValueString` is free text under `^[a-zA-Z0-9_.-]+$`, so a bound may equally be a
/// count (`10`), a fraction (`0.0625`) or a name (`G.1X`). Only a decimal integer or
/// fraction, optionally signed, is read as a number: the float parser would otherwise also
/// accept `inf`, `NaN` and `1e9`, which are parameter NAMES under that pattern and not
/// quantities this service orders.
fn numeric_configuration_value(value: &str) -> Option<f64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let well_formed = digits.chars().any(|character| character.is_ascii_digit())
        && digits
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
        && digits.matches('.').count() <= 1;
    if !well_formed {
        return None;
    }
    value.parse::<f64>().ok()
}

/// How many machine learning transform ids this process has minted. A nonce source for
/// [`new_ml_transform_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
static MINTED_ML_TRANSFORM_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `TransformId`: the id the service assigns a machine learning transform when
/// `CreateMLTransform` stores it.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255` — the same shape and the same width as the other `HashString` id this
/// store mints ([`new_data_quality_rule_recommendation_run_id`]), because the service
/// publishes no spelling for this one. The bits come from [`mint_hex_id`], so two creates
/// landing in the same nanosecond cannot be given the same id, and the id is stored on the
/// record, so "the ID is guaranteed to be unique and does not change"
/// (`MLTransform$TransformId`) holds across every later read.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_ml_transform_id() -> String {
    mint_hex_id(&MINTED_ML_TRANSFORM_IDS, ML_TRANSFORM_ID_WORDS)
}

/// How many 64-bit words a machine learning transform id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters.
const ML_TRANSFORM_ID_WORDS: usize = 2;

/// The `TransformStatusType` a machine learning transform is stored in by a create: of the
/// modelled enum (`NOT_READY`, `READY`, `DELETING`), the one that says the transform
/// cannot be used yet.
///
/// A create registers the transform and nothing more — "Call this operation as the first
/// step in the process of using a machine learning transform" (`CreateMLTransform`) — so
/// it holds no labels and has been taught nothing. Moving on from here takes a TASK RUN
/// that teaches it, not the passage of time, so no later status is owed by the clock.
pub const TRANSFORM_STATUS_NOT_READY: &str = "NOT_READY";

/// The `SortDirectionType` that reverses a listing's order: one of the modelled values
/// (`ASCENDING`, `DESCENDING`), the one a sort criteria names to walk the order backwards.
pub const SORT_DIRECTION_DESCENDING: &str = "DESCENDING";

/// The `TransformSortColumnType` that orders transforms by their creation instant. One of
/// the modelled values the generated [`crate::ops::types::TRANSFORM_SORT_COLUMN_TYPE_VALUES`]
/// carries (`CREATED`, `LAST_MODIFIED`, `NAME`, `STATUS`, `TRANSFORM_TYPE`), which the
/// request layer validates a sort criteria against.
pub const TRANSFORM_SORT_COLUMN_CREATED: &str = "CREATED";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the instant last modified.
pub const TRANSFORM_SORT_COLUMN_LAST_MODIFIED: &str = "LAST_MODIFIED";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the transform's name.
pub const TRANSFORM_SORT_COLUMN_NAME: &str = "NAME";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the transform's status.
pub const TRANSFORM_SORT_COLUMN_STATUS: &str = "STATUS";
/// See [`TRANSFORM_SORT_COLUMN_CREATED`]: orders by the transform's algorithm type.
pub const TRANSFORM_SORT_COLUMN_TRANSFORM_TYPE: &str = "TRANSFORM_TYPE";

/// `MLTransform$GlueVersion` when the create named none: "If the value is not set, the
/// Glue compatibility defaults to Glue 0.9" (`CreateMLTransform$GlueVersion`).
pub const DEFAULT_ML_TRANSFORM_GLUE_VERSION: &str = "0.9";

/// `MLTransform$MaxCapacity` when the create named no capacity at all: "You can allocate
/// from 2 to 100 DPUs; the default is 10" (`CreateMLTransform$MaxCapacity`).
pub const DEFAULT_ML_TRANSFORM_DPUS: f64 = 10.0;

/// `MLTransform$Timeout` when the create named none: "The default is 2,880 minutes (48
/// hours)" (`CreateMLTransform$Timeout`).
pub const DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES: i32 = 2_880;

/// How many columns a transform's schema holds at most: the modelled length of
/// `MLTransform$Schema` (`TransformSchema`, length 0..100) — "Has an upper bound of 100
/// columns".
pub const MAX_TRANSFORM_SCHEMA_COLUMNS: usize = 100;

/// The separator between the parts of a transform listing's ordering key (and so of its
/// `NextToken`).
///
/// `U+0001` cannot appear in a name, a status, an algorithm type or a minted id: every
/// such value's modelled pattern admits `\u0020` and up, so the separator can never be
/// confused with a character inside one of the parts it joins — and a key whose first part
/// is a prefix of another's therefore still orders before it.
const TRANSFORM_KEY_SEPARATOR: char = '\u{1}';

/// `at` rendered so that lexicographic order over the rendering is the order of the
/// instants themselves: nanoseconds since the epoch, zero-padded, so no comparison of
/// ordering keys has to parse a number back out. Twenty digits hold any nanosecond count
/// this clock can report.
fn ordered_instant(at: SystemTime) -> String {
    let nanos = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:020}")
}

/// `at` as seconds since the epoch, the one scale a transform's instants are compared
/// against a client-supplied timestamp on.
fn secs_since_epoch(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// A stored machine learning transform: the definition `CreateMLTransform` registered,
/// plus the members the SERVER fills in.
///
/// Every member of the modelled `MLTransform` shape is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports what the create
/// stored and nothing invented on the way out. The server-set members are the minted
/// `TransformId`, the two instants, the status every create starts in
/// ([`TRANSFORM_STATUS_NOT_READY`]), the label count and the SCHEMA, which is taken from
/// the catalog tables the create named rather than from the request.
///
/// `EvaluationMetrics` is NOT held: metrics "provide an estimate of the quality of your
/// machine learning transform" (`MLTransform$EvaluationMetrics`), and nothing has trained
/// or evaluated this transform, so there are none to report.
#[derive(Debug, Clone)]
pub struct MlTransformEntry {
    /// `MLTransform$TransformId`, minted by [`new_ml_transform_id`], which is also this
    /// record's key in [`ComAmazonawsGlueAwsglueState::ml_transforms`].
    pub transform_id: String,
    /// "A user-defined name for the machine learning transform", stored exactly as sent.
    /// NOT unique over the namespace: "Names are not guaranteed unique and can be changed
    /// at any time" (`MLTransform$Name`).
    pub name: String,
    /// "A user-defined, long-form description text", or `None` when the create named none.
    pub description: Option<String>,
    /// One of the modelled `TransformStatusType` values; see
    /// [`TRANSFORM_STATUS_NOT_READY`].
    pub status: String,
    /// "The time and date that this machine learning transform was created."
    pub created_on: SystemTime,
    /// "The last point in time when this machine learning transform was modified", which
    /// is the create's own instant until a write moves it.
    pub last_modified_on: SystemTime,
    /// "A list of Glue table definitions used by the transform", as the create addressed
    /// them.
    pub input_record_tables: Vec<GlueTable>,
    /// "The algorithm-specific parameters that are associated with the machine learning
    /// transform", stored exactly as sent.
    pub parameters: TransformParameters,
    /// "A count identifier for the labeling files generated by Glue for this transform"
    /// (`MLTransform$LabelCount`): zero on a create, which generates none, and the count a
    /// labelling write would move.
    pub label_count: i32,
    /// "The columns and data types that this transform can run against"
    /// (`MLTransform$Schema`), taken from the catalog columns of
    /// [`Self::input_record_tables`] when the transform was stored — the data the transform
    /// reads is what decides them, so they are read out of the catalog rather than taken
    /// from the request, which carries no schema at all.
    pub schema: Vec<SchemaColumn>,
    /// "The name or Amazon Resource Name (ARN) of the IAM role with the required
    /// permissions", stored exactly as sent.
    pub role: String,
    /// "This value determines which version of Glue this machine learning transform is
    /// compatible with", defaulting to [`DEFAULT_ML_TRANSFORM_GLUE_VERSION`].
    pub glue_version: String,
    /// "The number of Glue data processing units (DPUs) that are allocated to task runs for
    /// this transform", as [`ml_transform_capacity`] resolved it.
    pub max_capacity: Option<f64>,
    /// "The type of predefined worker that is allocated when a task of this transform
    /// runs", or `None` when the create named none.
    pub worker_type: Option<String>,
    /// "The number of workers of a defined `workerType` that are allocated when a task of
    /// the transform runs", or `None` when the create named none.
    pub number_of_workers: Option<i32>,
    /// "The timeout in minutes of the machine learning transform", defaulting to
    /// [`DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES`].
    pub timeout: i32,
    /// "The maximum number of times to retry after an `MLTaskRun` of the machine learning
    /// transform fails", or `None` when the create named none: the model documents no
    /// default for it, so nothing is reported where the client asked for nothing.
    pub max_retries: Option<i32>,
    /// "The encryption-at-rest settings of the transform that apply to accessing user
    /// data", stored exactly as sent.
    pub transform_encryption: Option<TransformEncryption>,
    /// The create's `Tags`, which are not part of the `MLTransform` shape.
    ///
    /// Kept so the create does not discard what the client sent, and NOT reported by a
    /// read: the shape carries no tag member, so the tags are reachable only through the
    /// tagging operations.
    pub tags: HashMap<String, String>,
}

impl MlTransformEntry {
    /// Build a stored transform from a `CreateMLTransform` request served at `now`.
    ///
    /// The definition comes from the request; `transform_id` is the minted id and `schema`
    /// the columns the catalog holds for the request's input tables
    /// ([`ComAmazonawsGlueAwsglueState::ml_transform_schema`]).
    pub fn from_create_input(
        input: &CreateMLTransformInput,
        transform_id: String,
        schema: Vec<SchemaColumn>,
        now: SystemTime,
    ) -> Self {
        let (worker_type, number_of_workers, max_capacity) = ml_transform_capacity(input);
        Self {
            transform_id,
            name: input.name.clone(),
            description: input.description.clone(),
            status: TRANSFORM_STATUS_NOT_READY.to_string(),
            created_on: now,
            last_modified_on: now,
            input_record_tables: input.input_record_tables.clone(),
            parameters: input.parameters.clone(),
            // A create generates no labelling file.
            label_count: 0,
            schema,
            role: input.role.clone(),
            glue_version: input
                .glue_version
                .clone()
                .unwrap_or_else(|| DEFAULT_ML_TRANSFORM_GLUE_VERSION.to_string()),
            max_capacity,
            worker_type,
            number_of_workers,
            timeout: input
                .timeout
                .unwrap_or(DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES),
            max_retries: input.max_retries,
            transform_encryption: input.transform_encryption.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `MLTransform` shape from what is stored, so a listing reports
    /// the same transform a single read does. The stored `Tags` are not part of that shape,
    /// so they stay out of it.
    pub fn to_ml_transform(&self) -> MLTransform {
        MLTransform {
            transform_id: Some(self.transform_id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: Some(self.status.clone()),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            input_record_tables: Some(self.input_record_tables.clone()),
            parameters: Some(self.parameters.clone()),
            // Nothing has trained or evaluated this transform, so it has no metrics.
            evaluation_metrics: None,
            label_count: self.label_count,
            schema: Some(self.schema.clone()),
            role: Some(self.role.clone()),
            glue_version: Some(self.glue_version.clone()),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            timeout: Some(self.timeout),
            max_retries: self.max_retries,
            transform_encryption: self.transform_encryption.clone(),
        }
    }

    /// Whether this transform satisfies every criterion `filter` names.
    ///
    /// A `TransformFilterCriteria` is a conjunction: each member it carries narrows the
    /// listing, and a member it leaves out narrows nothing. The three enum members and the
    /// name are matched exactly, the two instant pairs bound the window the criteria
    /// describe ("the time and date before which the transforms were created", so an
    /// instant strictly inside the bound), and `Schema` selects "datasets with a specific
    /// schema", i.e. the transforms whose own schema carries every column it names.
    pub fn matches(&self, filter: &TransformFilterCriteria) -> bool {
        if filter
            .name
            .as_deref()
            .is_some_and(|name| name != self.name.as_str())
        {
            return false;
        }
        if filter
            .transform_type
            .as_deref()
            .is_some_and(|kind| kind != self.parameters.transform_type.as_str())
        {
            return false;
        }
        if filter
            .status
            .as_deref()
            .is_some_and(|status| status != self.status.as_str())
        {
            return false;
        }
        if filter
            .glue_version
            .as_deref()
            .is_some_and(|version| version != self.glue_version.as_str())
        {
            return false;
        }
        let created = secs_since_epoch(self.created_on);
        let modified = secs_since_epoch(self.last_modified_on);
        let outside_window = [
            (filter.created_before.as_ref(), created, true),
            (filter.created_after.as_ref(), created, false),
            (filter.last_modified_before.as_ref(), modified, true),
            (filter.last_modified_after.as_ref(), modified, false),
        ]
        .into_iter()
        .any(|(bound, instant, before)| {
            bound.is_some_and(|bound| {
                if before {
                    instant >= bound.as_secs_f64()
                } else {
                    instant <= bound.as_secs_f64()
                }
            })
        });
        if outside_window {
            return false;
        }
        filter.schema.as_deref().is_none_or(|wanted| {
            wanted.iter().all(|column| {
                self.schema.iter().any(|held| {
                    held.name == column.name
                        && (column.data_type.is_none() || held.data_type == column.data_type)
                })
            })
        })
    }

    /// This transform's place in a listing ordered by `column`, as the continuation token
    /// renders it: the sorted value, then the transform's own id so that two transforms
    /// sharing that value still have one total order to page over.
    ///
    /// `None` is the order a request that names no sort criteria gets, which is the same
    /// key [`TRANSFORM_SORT_COLUMN_NAME`] names: the model documents no default order, so
    /// the listing falls back on a deterministic one rather than on however the store
    /// happens to hold its keys. A column outside the modelled list cannot reach here —
    /// the request layer validates `Sort$Column` against it — and is ordered the same way.
    pub fn ordering_key(&self, column: Option<&str>) -> String {
        let sorted = match column {
            Some(TRANSFORM_SORT_COLUMN_TRANSFORM_TYPE) => self.parameters.transform_type.clone(),
            Some(TRANSFORM_SORT_COLUMN_STATUS) => self.status.clone(),
            Some(TRANSFORM_SORT_COLUMN_CREATED) => ordered_instant(self.created_on),
            Some(TRANSFORM_SORT_COLUMN_LAST_MODIFIED) => ordered_instant(self.last_modified_on),
            Some(TRANSFORM_SORT_COLUMN_NAME) | Some(_) | None => self.name.clone(),
        };
        format!(
            "{sorted}{TRANSFORM_KEY_SEPARATOR}{}",
            self.transform_id
        )
    }
}

/// The capacity a machine learning transform runs its task runs with, as `(worker_type,
/// number_of_workers, max_capacity)`.
///
/// `MaxCapacity` and the worker configuration are two spellings of one quantity, and the
/// worker configuration wins where the request names one: "When the `WorkerType` field is
/// set to a value other than `Standard`, the `MaxCapacity` field is set automatically and
/// becomes read-only" (`CreateMLTransform$WorkerType`), so the figure reported is the DPUs
/// those workers map to ([`dpu_per_worker`]). That sentence excepts `Standard`, which the
/// model gives no per-worker figure at all, so a worker configuration this emulator cannot
/// convert leaves the figure the REQUEST named — nothing the client sent is discarded, and
/// nothing is derived where the service documents no conversion. Where the request names
/// no capacity at all the documented default applies ([`DEFAULT_ML_TRANSFORM_DPUS`]).
pub fn ml_transform_capacity(
    input: &CreateMLTransformInput,
) -> (Option<String>, Option<i32>, Option<f64>) {
    match (input.worker_type.as_deref(), input.number_of_workers) {
        (Some(worker_type), workers) => (
            Some(worker_type.to_string()),
            workers,
            dpu_per_worker(worker_type)
                .zip(workers)
                .map(|(per_worker, workers)| per_worker * f64::from(workers))
                .or(input.max_capacity),
        ),
        // A worker count with no worker type names no DPU figure this emulator could
        // derive, so only what the request itself named is reported.
        (None, Some(workers)) => (None, Some(workers), input.max_capacity),
        (None, None) => (
            None,
            None,
            Some(input.max_capacity.unwrap_or(DEFAULT_ML_TRANSFORM_DPUS)),
        ),
    }
}

/// How many form type ids this process has minted. A nonce source for
/// [`new_form_type_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
static MINTED_FORM_TYPE_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `FormTypeId`: the opaque identifier the Data Catalog assigns a form type when
/// `PutFormType` first stores it under a name.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is the shape a
/// catalog-assigned Data Catalog id has and is inside `FormTypeId`'s `length 1..256`. The
/// bits come from [`mint_hex_id`], as a database id's do, so two puts landing
/// in the same nanosecond cannot be given the same id. It is minted ONCE, when the name
/// is first stored, and kept on the [`FormTypeEntry`] through every later update, so
/// every read of that form type reports the same id and two form types never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_form_type_id() -> String {
    mint_hex_id(&MINTED_FORM_TYPE_IDS, CATALOG_ID_WORDS)
}

/// A stored form type: the schema for structured metadata `PutFormType` registers in the
/// Data Catalog.
///
/// Every member of the modelled form type is held here — its id, its name and its schema
/// — and nothing else: the model gives a form type no status, owner or timestamp. The
/// name and the schema come from the request; the id is the one server-set member, minted
/// by [`new_form_type_id`] when the name is first stored.
#[derive(Debug, Clone)]
pub struct FormTypeEntry {
    /// `FormTypeId`, minted by [`new_form_type_id`] at the first put under this name and
    /// unchanged by every later one, so a caller that kept the id from an earlier put can
    /// still address the form type after it has been updated.
    pub id: String,
    /// The form type's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::form_types`].
    pub name: String,
    /// "The Smithy IDL schema definition for the form type", stored exactly as sent: the
    /// catalog reports back the schema text the client wrote, not a re-rendering of it.
    pub schema: String,
}

impl FormTypeEntry {
    /// The record a put stores for a name this store does not hold yet: the request's
    /// name and schema, under a freshly minted id.
    pub fn new(name: String, schema: String) -> Self {
        Self {
            id: new_form_type_id(),
            name,
            schema,
        }
    }
}

/// How many asset type ids this process has minted. A nonce source for
/// [`new_asset_type_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
static MINTED_ASSET_TYPE_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `AssetTypeId`: the opaque identifier the Data Catalog assigns an asset type
/// when `PutAssetType` first stores it under a name.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, the same shape a form type id has
/// and inside `AssetTypeId`'s `length 1..256`. The bits come from [`mint_hex_id`], so two
/// puts landing in the same nanosecond cannot be given the same id. It is minted ONCE,
/// when the name is first stored, and kept on the [`AssetTypeEntry`] through every later
/// update, so every read of that asset type reports the same id and two asset types never
/// share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_asset_type_id() -> String {
    mint_hex_id(&MINTED_ASSET_TYPE_IDS, CATALOG_ID_WORDS)
}

/// A stored asset type: the structure of an asset `PutAssetType` registers in the Data
/// Catalog, as the set of forms an asset of that type includes.
///
/// Every member of the modelled asset type is held here — its id, its name and its forms
/// — and nothing else: the model gives an asset type no status, owner or timestamp. The
/// name and the forms come from the request; the id is the one server-set member, minted
/// by [`new_asset_type_id`] when the name is first stored.
#[derive(Debug, Clone)]
pub struct AssetTypeEntry {
    /// `AssetTypeId`, minted by [`new_asset_type_id`] at the first put under this name and
    /// unchanged by every later one, so a caller that kept the id from an earlier put can
    /// still address the asset type after it has been updated.
    pub id: String,
    /// The asset type's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::asset_types`].
    pub name: String,
    /// "The forms that make up the asset type, keyed by form name", each value the
    /// identifier of the form type that defines that form's schema, stored exactly as the
    /// put sent it (`AssetTypeFormReference$FormTypeIdentifier`, the reference's only
    /// member) so a read reports the identifier the client wrote. It resolved against this
    /// store's form types when it was stored
    /// ([`ComAmazonawsGlueAwsglueState::form_type_by_reference`]), so it is either a
    /// minted `FormTypeId` or the `FormTypeName` the form type was registered under.
    /// Ordered by form name, so two reads of one asset type report the forms in one order.
    pub forms: BTreeMap<String, String>,
}

impl AssetTypeEntry {
    /// The record a put stores for a name this store does not hold yet: the request's
    /// name and form references, under a freshly minted id.
    pub fn new(name: String, forms: BTreeMap<String, String>) -> Self {
        Self {
            id: new_asset_type_id(),
            name,
            forms,
        }
    }

    /// This record's forms as the modelled `AssetTypeFormsMap`, which is what every
    /// operation that reports an asset type answers with.
    pub fn forms_map(&self) -> HashMap<String, AssetTypeFormReference> {
        self.forms
            .iter()
            .map(|(form_name, form_type_id)| {
                (
                    form_name.clone(),
                    AssetTypeFormReference {
                        form_type_identifier: form_type_id.clone(),
                    },
                )
            })
            .collect()
    }
}

/// How many glossary ids this process has minted. A nonce source for
/// [`new_glossary_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
static MINTED_GLOSSARY_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `GlossaryId`: the opaque identifier the Data Catalog assigns a business
/// glossary when `CreateGlossary` stores it.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is the shape a
/// catalog-assigned Data Catalog id has; `GlossaryId` carries no length or pattern
/// constraint of its own. The bits come from [`mint_hex_id`], as a database id's do, so
/// two creates landing in the same nanosecond cannot be given the same id. It is minted
/// ONCE, at create, and kept on the [`GlossaryEntry`] through every later update — a
/// rename included — so every read of that glossary reports the same id and two
/// glossaries never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_glossary_id() -> String {
    mint_hex_id(&MINTED_GLOSSARY_IDS, CATALOG_ID_WORDS)
}

/// A stored business glossary: the container for glossary terms `CreateGlossary`
/// registers.
///
/// Every member of the modelled glossary is held here — its id, its name and its
/// description — and nothing else: the model gives a glossary no status, owner or
/// timestamp. The name and the description come from the request; the id is the one
/// server-set member, minted by [`new_glossary_id`] when the glossary is stored.
#[derive(Debug, Clone)]
pub struct GlossaryEntry {
    /// `GlossaryId`, minted by [`new_glossary_id`] at create and unchanged afterwards, so
    /// a caller that kept the id from the create can still address the glossary after it
    /// has been updated.
    pub id: String,
    /// "The name of the glossary" (`CreateGlossaryRequest$Name`), unique over this
    /// account's glossaries. Stored exactly as sent — the model documents no case fold
    /// for `GlossaryName`.
    pub name: String,
    /// "The description of the glossary", absent when the create carried none: the member
    /// is optional and the model gives it no default, so a glossary created without one
    /// reports none rather than an empty string.
    pub description: Option<String>,
    /// The `ClientToken` of the request that created this glossary, when it carried one.
    ///
    /// `CreateGlossaryRequest$ClientToken` is "a unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request", so a retry of that same
    /// request is answered with this record instead of being refused as a duplicate name.
    pub client_token: Option<String>,
}

impl GlossaryEntry {
    /// The record a create stores: the name and description it carried, under a freshly
    /// minted id, beside the idempotency token a retry of that create is recognised by.
    pub fn new(name: String, description: Option<String>, client_token: Option<String>) -> Self {
        Self {
            id: new_glossary_id(),
            name,
            description,
            client_token,
        }
    }

    /// The same, read off a `CreateGlossary` request: every member comes from the request
    /// except the id, which the catalog assigns.
    pub fn from_create_input(input: &CreateGlossaryInput) -> Self {
        Self::new(
            input.name.clone(),
            input.description.clone(),
            input.client_token.clone(),
        )
    }
}

/// How many glossary term ids this process has minted. A nonce source for
/// [`new_glossary_term_id`], exactly as [`MINTED_GLOSSARY_IDS`] is for a glossary id.
static MINTED_GLOSSARY_TERM_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `GlossaryTermId`: the opaque identifier the Data Catalog assigns a glossary term
/// when `CreateGlossaryTerm` stores it.
///
/// The same shape as a glossary id ([`new_glossary_id`]) and for the same reason:
/// `GlossaryTermId` carries no length or pattern constraint of its own, and the bits come
/// from [`mint_hex_id`], so two creates landing in the same nanosecond cannot be given the
/// same id. It is minted ONCE, at create, and kept on the [`GlossaryTermEntry`] through
/// every later update — a rename included — so every read of that term reports the same id
/// and two terms never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_glossary_term_id() -> String {
    mint_hex_id(&MINTED_GLOSSARY_TERM_IDS, CATALOG_ID_WORDS)
}

/// A stored glossary term: one business concept `CreateGlossaryTerm` registers within a
/// glossary.
///
/// Every member of the modelled term is held here — its id, the glossary it belongs to, its
/// name and its two descriptions — and nothing else: the model gives a term no status,
/// owner or timestamp. The glossary, the name and the descriptions come from the request;
/// the id is the one server-set member, minted by [`new_glossary_term_id`] when the term is
/// stored.
#[derive(Debug, Clone)]
pub struct GlossaryTermEntry {
    /// `GlossaryTermId`, minted by [`new_glossary_term_id`] at create and unchanged
    /// afterwards, so a caller that kept the id from the create can still address the term
    /// after it has been updated.
    pub id: String,
    /// "The unique identifier of the glossary in which to create the term"
    /// (`CreateGlossaryTermRequest$GlossaryIdentifier`), recorded on the term because a
    /// term is addressed by its own id alone and the glossary it belongs to is reported by
    /// every read of it.
    pub glossary_id: String,
    /// "The name of the glossary term" (`CreateGlossaryTermRequest$Name`), unique within
    /// [`Self::glossary_id`]. Stored exactly as sent — the model documents no case fold for
    /// `GlossaryTermName`.
    pub name: String,
    /// "A short description of the glossary term", absent when the create carried none: the
    /// member is optional and the model gives it no default, so a term created without one
    /// reports none rather than an empty string.
    pub short_description: Option<String>,
    /// "A long description of the glossary term", absent for the same reason as
    /// [`Self::short_description`].
    pub long_description: Option<String>,
    /// The `ClientToken` of the request that created this term, when it carried one.
    ///
    /// `CreateGlossaryTermRequest$ClientToken` is "a unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request", so a retry of that same
    /// request is answered with this record instead of being refused as a duplicate name.
    pub client_token: Option<String>,
}

impl GlossaryTermEntry {
    /// The record a create stores: the glossary, name and descriptions it carried, under a
    /// freshly minted id, beside the idempotency token a retry of that create is recognised
    /// by.
    pub fn new(
        glossary_id: String,
        name: String,
        short_description: Option<String>,
        long_description: Option<String>,
        client_token: Option<String>,
    ) -> Self {
        Self {
            id: new_glossary_term_id(),
            glossary_id,
            name,
            short_description,
            long_description,
            client_token,
        }
    }

    /// The same, read off a `CreateGlossaryTerm` request: every member comes from the
    /// request except the id, which the catalog assigns.
    pub fn from_create_input(input: &CreateGlossaryTermInput) -> Self {
        Self::new(
            input.glossary_identifier.clone(),
            input.name.clone(),
            input.short_description.clone(),
            input.long_description.clone(),
            input.client_token.clone(),
        )
    }
}

/// `JobCommand$Name` values the model documents: "For an Apache Spark ETL job, this
/// must be `glueetl`. For a Python shell job, it must be `pythonshell`. For an Apache
/// Spark streaming ETL job, this must be `gluestreaming`. For a Ray job, this must be
/// `glueray`." A command naming anything else is not a job this service can run.
pub const JOB_COMMAND_NAMES: [&str; 4] = ["glueetl", "pythonshell", "gluestreaming", "glueray"];

/// The `JobMode` stored for a create that named none: "When the JobMode field is
/// missing or null, SCRIPT is assigned as the default value" (`CreateJob$JobMode`).
pub const DEFAULT_JOB_MODE: &str = "SCRIPT";

/// The `GlueVersion` stored for a create that named none: "Jobs that are created
/// without specifying a Glue version default to Glue 5.1" (`CreateJob$GlueVersion`).
pub const DEFAULT_GLUE_VERSION: &str = "5.1";

/// `ExecutionProperty$MaxConcurrentRuns` for a job whose create named no execution
/// property: "The maximum number of concurrent runs allowed for the job. The default
/// is 1." The model's `@default` of 0 is what an ABSENT integer deserializes to, not a
/// concurrency a job can run at.
pub const DEFAULT_MAX_CONCURRENT_RUNS: i32 = 1;

/// `Job$Timeout` when the create left it blank, for Glue 4.0 and earlier: "When the
/// value is left blank, the timeout is defaulted to 2,880 minutes for Glue version 4.0
/// and earlier, or 480 minutes for Glue version 5.0 and later."
pub const DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4: i32 = 2_880;

/// `Job$Timeout` when the create left it blank, for Glue 5.0 and later — the second
/// half of the sentence quoted on [`DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4`].
pub const DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5: i32 = 480;

/// The largest `Timeout` a job may carry: "Jobs must have timeout values less than 7
/// days or 10080 minutes. Otherwise, the jobs will throw an exception." (`Job$Timeout`)
pub const MAX_TIMEOUT_MINUTES: i32 = 10_080;

/// The `JobCommand$Name` of an Apache Spark ETL job, which is the only command
/// `ExecutionClass=FLEX` may be set on, and the `ExecutionClass` that restriction is
/// about: "Only jobs with Glue version 3.0 and above and command type `glueetl` will be
/// allowed to set `ExecutionClass` to `FLEX`." (`CreateJob$ExecutionClass`.)
pub const SPARK_ETL_COMMAND: &str = "glueetl";
/// See [`SPARK_ETL_COMMAND`].
pub const EXECUTION_CLASS_FLEX: &str = "FLEX";
/// The lowest `GlueVersion` that sentence admits `FLEX` on.
pub const MIN_FLEX_GLUE_VERSION: f64 = 3.0;

/// Whether a job's command and Glue version are the pair `ExecutionClass=FLEX` is allowed
/// on, by the sentence quoted on [`SPARK_ETL_COMMAND`].
///
/// A `GlueVersion` this cannot read as a number (the model's pattern `^(\w+\.)+\w+$` is
/// wider than a decimal) admits nothing: the restriction is stated over a version
/// comparison, so a version that cannot be compared does not satisfy it.
pub fn admits_flex_execution_class(command: &JobCommand, glue_version: &str) -> bool {
    let spark_etl = command.name.as_deref() == Some(SPARK_ETL_COMMAND);
    let version_admits = glue_version
        .parse::<f64>()
        .is_ok_and(|version| version >= MIN_FLEX_GLUE_VERSION);
    spark_etl && version_admits
}

/// The first Glue major version that takes [`DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5`].
const FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT: u32 = 5;

/// The capacity, in DPUs, a Spark job whose definition names none runs with: "When you
/// specify an Apache Spark ETL job (`JobCommand.Name`="glueetl") or Apache Spark
/// streaming ETL job (`JobCommand.Name`="gluestreaming"), you can allocate from 2 to 100
/// DPUs. The default is 10 DPUs." (`CreateJob$MaxCapacity`.)
pub const DEFAULT_SPARK_DPUS: f64 = 10.0;

/// The capacity, in DPUs, a Python shell job whose definition names none runs with:
/// "When you specify a Python shell job (`JobCommand.Name`="pythonshell"), you can
/// allocate either 0.0625 or 1 DPU. The default is 0.0625 DPU." (`CreateJob$MaxCapacity`.)
pub const DEFAULT_PYTHON_SHELL_DPUS: f64 = 0.0625;

/// The `WorkerType` a Spark job whose definition names no worker configuration runs
/// with, and the number of such workers.
///
/// From Glue 2.0 on, a Spark job's capacity is expressed as a worker type and a worker
/// count rather than as a DPU figure — "For Glue version 2.0 or later jobs, you cannot
/// specify a `Maximum capacity`. Instead, you should specify a `Worker type` and the
/// `Number of workers`" (`CreateJob$MaxCapacity`) — so the documented default of
/// [`DEFAULT_SPARK_DPUS`] is carried as that many `G.1X` workers, each of which "maps to
/// 1 DPU" (`CreateJob$WorkerType`).
pub const DEFAULT_SPARK_WORKER_TYPE: &str = "G.1X";
/// See [`DEFAULT_SPARK_WORKER_TYPE`]: ten one-DPU workers are the ten DPUs
/// `CreateJob$MaxCapacity` documents as a Spark job's default.
pub const DEFAULT_SPARK_WORKERS: i32 = 10;

/// The lowest `GlueVersion` whose jobs express capacity as a worker configuration, by the
/// sentence quoted on [`DEFAULT_SPARK_WORKER_TYPE`].
pub const MIN_WORKER_TYPE_GLUE_VERSION: f64 = 2.0;

/// The `JobCommand$Name` of an Apache Spark STREAMING ETL job, the second of the two
/// commands [`DEFAULT_SPARK_DPUS`] is documented for.
pub const SPARK_STREAMING_COMMAND: &str = "gluestreaming";

/// The `JobCommand$Name` of a Python shell job, the command [`DEFAULT_PYTHON_SHELL_DPUS`]
/// is documented for.
pub const PYTHON_SHELL_COMMAND: &str = "pythonshell";

/// The capacity a job definition runs with when it named none of `MaxCapacity`,
/// `WorkerType` and `NumberOfWorkers`, as `(worker_type, number_of_workers,
/// max_capacity)`.
///
/// The service fills a concrete capacity in rather than leaving a job no runner could
/// size: the figure is the one `CreateJob$MaxCapacity` documents for the job's COMMAND
/// ([`DEFAULT_SPARK_DPUS`] for a Spark batch or streaming job,
/// [`DEFAULT_PYTHON_SHELL_DPUS`] for a Python shell job), and from
/// [`MIN_WORKER_TYPE_GLUE_VERSION`] on a Spark job's figure is expressed as the worker
/// configuration that version takes ([`DEFAULT_SPARK_WORKER_TYPE`]).
///
/// A command this service documents no default capacity for — a Ray job, whose published
/// worker configuration this emulator would have to invent — is left with none, so
/// nothing is reported that the service does not state.
pub fn default_capacity(
    command: &JobCommand,
    glue_version: &str,
) -> (Option<String>, Option<i32>, Option<f64>) {
    let expresses_workers = glue_version
        .parse::<f64>()
        .is_ok_and(|version| version >= MIN_WORKER_TYPE_GLUE_VERSION);
    match command.name.as_deref() {
        Some(SPARK_ETL_COMMAND) | Some(SPARK_STREAMING_COMMAND) if expresses_workers => (
            Some(DEFAULT_SPARK_WORKER_TYPE.to_string()),
            Some(DEFAULT_SPARK_WORKERS),
            dpu_per_worker(DEFAULT_SPARK_WORKER_TYPE)
                .map(|per_worker| per_worker * f64::from(DEFAULT_SPARK_WORKERS)),
        ),
        Some(SPARK_ETL_COMMAND) | Some(SPARK_STREAMING_COMMAND) => {
            (None, None, Some(DEFAULT_SPARK_DPUS))
        }
        Some(PYTHON_SHELL_COMMAND) => (None, None, Some(DEFAULT_PYTHON_SHELL_DPUS)),
        _ => (None, None, None),
    }
}

/// The DPUs one worker of `worker_type` maps to, as `CreateJob$WorkerType` documents
/// each predefined worker ("For the `G.1X` worker type, each worker maps to 1 DPU", "…
/// `G.2X` … 2 DPU", "… `G.4X` … 4 DPU", "… `G.8X` … 8 DPU", "… `G.025X` … 0.25 DPU",
/// "… `Z.2X` … 2 M-DPU").
///
/// `None` for anything the model gives no per-worker DPU figure for — the `Standard`
/// worker type among them — so no capacity is derived where the service documents none.
pub fn dpu_per_worker(worker_type: &str) -> Option<f64> {
    match worker_type {
        "G.025X" => Some(0.25),
        "G.1X" => Some(1.0),
        "G.2X" | "Z.2X" => Some(2.0),
        "G.4X" => Some(4.0),
        "G.8X" => Some(8.0),
        _ => None,
    }
}

/// The `Timeout` a job created without one carries, which depends on its Glue version
/// (see [`DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4`]). The major version is the leading
/// integer of the `GlueVersion` string (`^(\w+\.)+\w+$`); a version whose major part is
/// not a number is treated as the current default, which is what an absent
/// `GlueVersion` resolves to.
pub fn default_timeout_minutes(glue_version: &str) -> i32 {
    let major = glue_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT);
    if major >= FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT {
        DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5
    } else {
        DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4
    }
}

/// A stored job definition, plus the runs `StartJobRun` has started of it.
///
/// Every member of the modelled `Job` shape a create can fill is held here, so a later
/// `GetJob` reports what the create stored and nothing else — the members only a RUN
/// produces live on [`JobRunEntry`], in [`JobEntry::runs`], and `ProfileName` is not a
/// `CreateJob` member, so a created job carries none.
#[derive(Debug, Clone)]
pub struct JobEntry {
    /// The job's name, which is also its key in [`ComAmazonawsGlueAwsglueState::jobs`].
    pub name: String,
    /// One of the modelled `JobMode` values; [`DEFAULT_JOB_MODE`] when the create
    /// named none.
    pub job_mode: String,
    pub job_run_queuing_enabled: Option<bool>,
    pub description: Option<String>,
    pub log_uri: Option<String>,
    pub role: String,
    /// Always stored, because the service documents a default concurrency
    /// ([`DEFAULT_MAX_CONCURRENT_RUNS`]) rather than "no execution property".
    pub execution_property: ExecutionProperty,
    pub command: JobCommand,
    pub default_arguments: Option<HashMap<String, String>>,
    pub non_overridable_arguments: Option<HashMap<String, String>>,
    pub connections: Option<ConnectionsList>,
    pub max_retries: i32,
    /// The job's capacity in DPUs: what the create named, else what its worker
    /// configuration maps to, else the documented default for its command and Glue
    /// version (see [`capacity_members`]). `None` only for a command this service
    /// publishes no default capacity for.
    pub max_capacity: Option<f64>,
    /// The job timeout in minutes: what the create named, else the documented default
    /// for its Glue version.
    pub timeout: i32,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub security_configuration: Option<String>,
    pub notification_property: Option<NotificationProperty>,
    /// [`DEFAULT_GLUE_VERSION`] when the create named none.
    pub glue_version: String,
    pub code_gen_configuration_nodes: Option<HashMap<String, CodeGenConfigurationNode>>,
    pub execution_class: Option<String>,
    pub source_control_details: Option<SourceControlDetails>,
    pub maintenance_window: Option<String>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this job.
    pub last_modified_on: SystemTime,
    /// The request's `Tags`, which are not part of the `Job` shape — they are read back
    /// through the tagging operations.
    pub tags: HashMap<String, String>,
    /// The runs of this job definition, oldest first, as `StartJobRun` appended them.
    ///
    /// They live on the job rather than in a store-wide map because every operation that
    /// addresses a run names its job as well (`GetJobRun`/`GetJobRuns`/`BatchStopJobRun`
    /// all take `JobName`), so a run is reclaimed with the definition it belongs to.
    pub runs: Vec<JobRunEntry>,
}

impl JobEntry {
    /// Build a stored job from a `CreateJob` request plus the members the SERVICE
    /// fills: the documented defaults for an absent `JobMode`, `GlueVersion`,
    /// `ExecutionProperty`, `Timeout` and worker/capacity configuration
    /// ([`capacity_members`]), and the create instant.
    pub fn from_create_input(input: &CreateJobInput, now: SystemTime) -> Self {
        let glue_version = input
            .glue_version
            .clone()
            .unwrap_or_else(|| DEFAULT_GLUE_VERSION.to_string());
        let max_concurrent_runs = max_concurrent_runs_of(input.execution_property.as_ref());
        let (worker_type, number_of_workers, max_capacity) = capacity_members(
            input.max_capacity,
            input.allocated_capacity,
            input.worker_type.as_deref(),
            input.number_of_workers,
            &input.command,
            &glue_version,
        );
        Self {
            name: input.name.clone(),
            job_mode: input
                .job_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_JOB_MODE.to_string()),
            job_run_queuing_enabled: input.job_run_queuing_enabled,
            description: input.description.clone(),
            log_uri: input.log_uri.clone(),
            role: input.role.clone(),
            execution_property: ExecutionProperty {
                max_concurrent_runs,
            },
            command: input.command.clone(),
            default_arguments: input.default_arguments.clone(),
            non_overridable_arguments: input.non_overridable_arguments.clone(),
            connections: input.connections.clone(),
            max_retries: input.max_retries,
            max_capacity,
            timeout: input
                .timeout
                .unwrap_or_else(|| default_timeout_minutes(&glue_version)),
            worker_type,
            number_of_workers,
            security_configuration: input.security_configuration.clone(),
            notification_property: input.notification_property.clone(),
            glue_version,
            code_gen_configuration_nodes: input.code_gen_configuration_nodes.clone(),
            execution_class: input.execution_class.clone(),
            source_control_details: input.source_control_details.clone(),
            maintenance_window: input.maintenance_window.clone(),
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
            // A job definition has no runs until `StartJobRun` starts one.
            runs: Vec::new(),
        }
    }

    /// Overwrite this job definition with the one an `UpdateJob` request submits.
    ///
    /// "The previous job definition is completely overwritten by this information"
    /// (`UpdateJob`), and "unspecified configuration is removed or reset to default
    /// values" (`UpdateJobRequest$JobUpdate`) — so every member of the stored definition
    /// the submitted `JobUpdate` can carry is taken from it, absent or not, and the
    /// members the SERVICE fills are re-derived from the submitted definition exactly as
    /// [`JobEntry::from_create_input`] derives them for a create. The exceptions are the
    /// four fields `JobUpdate` has no member for — the job's name, its create instant,
    /// its tags (read back through the tagging operations) and the runs `StartJobRun`
    /// already started of it — and `GlueVersion`, which "retains the value from the
    /// existing job definition" when the update names none (`JobUpdate$GlueVersion`).
    ///
    /// A submitted definition naming no `Role` or no `Command` is refused by the
    /// operation before it reaches here, since `JobUpdate` documents both as required.
    pub fn apply_update(&mut self, update: &JobUpdate, now: SystemTime) {
        let glue_version = update
            .glue_version
            .clone()
            .unwrap_or_else(|| self.glue_version.clone());
        let max_concurrent_runs = max_concurrent_runs_of(update.execution_property.as_ref());
        let command = update.command.clone().unwrap_or_default();
        let (worker_type, number_of_workers, max_capacity) = capacity_members(
            update.max_capacity,
            update.allocated_capacity,
            update.worker_type.as_deref(),
            update.number_of_workers,
            &command,
            &glue_version,
        );
        // Carried over rather than overwritten; moved out so the whole record below is
        // rebuilt, which is what makes "completely overwritten" exhaustive.
        let name = std::mem::take(&mut self.name);
        let created_on = self.created_on;
        let tags = std::mem::take(&mut self.tags);
        let runs = std::mem::take(&mut self.runs);
        *self = Self {
            name,
            job_mode: update
                .job_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_JOB_MODE.to_string()),
            job_run_queuing_enabled: update.job_run_queuing_enabled,
            description: update.description.clone(),
            log_uri: update.log_uri.clone(),
            role: update.role.clone().unwrap_or_default(),
            execution_property: ExecutionProperty {
                max_concurrent_runs,
            },
            command,
            default_arguments: update.default_arguments.clone(),
            non_overridable_arguments: update.non_overridable_arguments.clone(),
            connections: update.connections.clone(),
            max_retries: update.max_retries,
            max_capacity,
            timeout: update
                .timeout
                .unwrap_or_else(|| default_timeout_minutes(&glue_version)),
            worker_type,
            number_of_workers,
            security_configuration: update.security_configuration.clone(),
            notification_property: update.notification_property.clone(),
            glue_version,
            code_gen_configuration_nodes: update.code_gen_configuration_nodes.clone(),
            execution_class: update.execution_class.clone(),
            source_control_details: update.source_control_details.clone(),
            maintenance_window: update.maintenance_window.clone(),
            created_on,
            last_modified_on: now,
            tags,
            runs,
        };
    }

    /// The maximum number of this job's runs that may be in flight at once:
    /// `ExecutionProperty$MaxConcurrentRuns`, held at the documented minimum of one
    /// ([`DEFAULT_MAX_CONCURRENT_RUNS`]) so a stored zero cannot make the job unrunnable.
    pub fn max_concurrent_runs(&self) -> i32 {
        self.execution_property
            .max_concurrent_runs
            .max(DEFAULT_MAX_CONCURRENT_RUNS)
    }

    /// Give every run of this job the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs: a run's next state was due
    /// at [`JobRunEntry::settles_at`] whether or not anybody called, so materialising it
    /// on read reports the state the clock had already reached. It never moves a run
    /// because something was read.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// The stored run with this `Id`, or `None` when this job has no such run.
    pub fn run(&self, run_id: &str) -> Option<&JobRunEntry> {
        self.runs.iter().find(|run| run.id == run_id)
    }

    /// The same run, to write to: what an operation that moves one run of this job
    /// (`BatchStopJobRun`) addresses it through.
    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut JobRunEntry> {
        self.runs.iter_mut().find(|run| run.id == run_id)
    }

    /// How many of this job's runs have not finished.
    ///
    /// Call [`JobEntry::settle_due_runs`] first, or a run the clock has already finished
    /// still counts as in flight.
    pub fn runs_in_flight(&self) -> usize {
        self.runs.iter().filter(|run| !run.is_terminal()).count()
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_JOB`] of them.
    pub fn record_run(&mut self, run: JobRunEntry) {
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_JOB {
            let overflow = self.runs.len() - MAX_RUNS_PER_JOB;
            self.runs.drain(..overflow);
        }
    }

    /// Rebuild the modelled `Job` output shape from what is stored.
    pub fn to_job(&self) -> Job {
        Job {
            name: Some(self.name.clone()),
            job_mode: Some(self.job_mode.clone()),
            job_run_queuing_enabled: self.job_run_queuing_enabled,
            description: self.description.clone(),
            log_uri: self.log_uri.clone(),
            role: Some(self.role.clone()),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            execution_property: Some(self.execution_property.clone()),
            command: Some(self.command.clone()),
            default_arguments: self.default_arguments.clone(),
            non_overridable_arguments: self.non_overridable_arguments.clone(),
            connections: self.connections.clone(),
            max_retries: self.max_retries,
            // `AllocatedCapacity` is the same quantity as `MaxCapacity` — "This field
            // is deprecated. Use MaxCapacity instead" — so it reports the same number
            // of whole DPUs rather than a second, disagreeing figure. A job with no
            // capacity figure reports the model's own default of 0.
            allocated_capacity: self.max_capacity.map_or(0, |capacity| capacity as i32),
            timeout: Some(self.timeout),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            security_configuration: self.security_configuration.clone(),
            notification_property: self.notification_property.clone(),
            glue_version: Some(self.glue_version.clone()),
            code_gen_configuration_nodes: self.code_gen_configuration_nodes.clone(),
            execution_class: self.execution_class.clone(),
            source_control_details: self.source_control_details.clone(),
            maintenance_window: self.maintenance_window.clone(),
            // `ProfileName` names a usage profile, which `CreateJob` cannot set.
            profile_name: None,
        }
    }
}

/// `ExecutionProperty$MaxConcurrentRuns` a submitted job definition asks for: what it
/// names, else the documented default ([`DEFAULT_MAX_CONCURRENT_RUNS`]) — which is also
/// what a property naming zero concurrent runs resolves to, since that would otherwise
/// store a job no run of which could ever start.
///
/// `CreateJob` and `UpdateJob` carry the same member with the same documentation, so
/// both resolve it here.
fn max_concurrent_runs_of(property: Option<&ExecutionProperty>) -> i32 {
    property
        .map(|property| property.max_concurrent_runs)
        .filter(|runs| *runs > 0)
        .unwrap_or(DEFAULT_MAX_CONCURRENT_RUNS)
}

/// The capacity members a submitted job definition is STORED with, as
/// `(worker_type, number_of_workers, max_capacity)`.
///
/// What the definition named, read by [`capacity_from`] — else, when it named no capacity
/// at all, the documented default for its command and Glue version
/// ([`default_capacity`]), because the service fills those in rather than storing a job
/// with no capacity for a runner to size. `CreateJob` and `UpdateJob` submit the same four
/// capacity members with the same documentation, so both resolve them here.
fn capacity_members(
    max_capacity: Option<f64>,
    allocated_capacity: i32,
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
    command: &JobCommand,
    glue_version: &str,
) -> (Option<String>, Option<i32>, Option<f64>) {
    let named = capacity_from(
        max_capacity,
        allocated_capacity,
        worker_type,
        number_of_workers,
    );
    if named.is_none() && worker_type.is_none() && number_of_workers.is_none() {
        return default_capacity(command, glue_version);
    }
    (
        worker_type.map(str::to_string),
        number_of_workers,
        named,
    )
}

/// The capacity, in DPUs, a request asks for: `MaxCapacity` when it names one, else the
/// deprecated `AllocatedCapacity` ("This parameter is deprecated. Use MaxCapacity
/// instead", so the two are one quantity), else what the worker configuration maps to —
/// `NumberOfWorkers` workers of `WorkerType`, at the DPUs per worker [`dpu_per_worker`]
/// quotes from the model. `None` when nothing names a capacity and no worker
/// configuration the model gives a DPU figure for is named.
///
/// `CreateJob`, `UpdateJob` and `StartJobRun` carry the same four members and the same
/// documentation for each, so all three resolve capacity here.
fn capacity_from(
    max_capacity: Option<f64>,
    allocated_capacity: i32,
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
) -> Option<f64> {
    if let Some(max_capacity) = max_capacity {
        return Some(max_capacity);
    }
    if allocated_capacity > 0 {
        return Some(f64::from(allocated_capacity));
    }
    let workers = number_of_workers.filter(|count| *count > 0)?;
    let per_worker = dpu_per_worker(worker_type?)?;
    Some(per_worker * f64::from(workers))
}

/// The `JobRunState` a run is in the instant `StartJobRun` returns, and the one the
/// clock then owes it. Both are values of the modelled `JobRunState` enum (`STARTING`,
/// `RUNNING`, `STOPPING`, `STOPPED`, `SUCCEEDED`, `FAILED`, `TIMEOUT`, `ERROR`,
/// `WAITING`, `EXPIRED`).
pub const JOB_RUN_STATE_STARTING: &str = "STARTING";
/// See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_FAILED: &str = "FAILED";
/// The `JobRunState` a run is in once a stop has been submitted for it, before it comes
/// to rest in [`JOB_RUN_STATE_STOPPED`]. See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_STOPPING: &str = "STOPPING";
/// The `JobRunState` a submitted stop brings a run to rest in. See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_STOPPED: &str = "STOPPED";

/// The `JobRunState` values a run never moves out of again: of the modelled enum quoted
/// on [`JOB_RUN_STATE_STARTING`], the ones that name a finished run. The other four
/// (`STARTING`, `RUNNING`, `STOPPING`, `WAITING`) each still owe a further state.
pub const TERMINAL_JOB_RUN_STATES: [&str; 6] = [
    JOB_RUN_STATE_STOPPED,
    "SUCCEEDED",
    JOB_RUN_STATE_FAILED,
    "TIMEOUT",
    "ERROR",
    "EXPIRED",
];

/// How long a run stays in the state a call put it in before the clock owes it the next
/// one: [`JOB_RUN_STATE_STARTING`] before the state the run comes to rest in, and
/// [`JOB_RUN_STATE_STOPPING`] before [`JOB_RUN_STATE_STOPPED`].
///
/// CHOSEN, not documented: Glue publishes no duration for starting or for stopping a job
/// run, so this is the emulator's own span — deliberately not zero, so a caller that
/// polls `GetJobRun` straight after `StartJobRun` still observes the run in flight, and
/// the concurrency limit that goes with it, and one that polls straight after a submitted
/// stop still observes the run stopping. The due instant is stored on the run and
/// materialised by whichever later call reads it; nothing sleeps.
pub const JOB_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// `JobRun$ErrorMessage` for the terminal state the clock owes every run.
///
/// This emulator is the Glue control plane: it records a run faithfully, but it runs no
/// job script, so a run can never reach `SUCCEEDED` — it ends in the modelled failure
/// state carrying a reason that says exactly that, rather than reporting a result nothing
/// computed.
pub const JOB_RUN_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records job runs but executes no job script, so the job run ended \
     without running.";

/// `JobRun$Attempt` of a run that is not a retry of an earlier one: "The number of the
/// attempt to run this job", which for a first attempt is the model's own default of 0.
pub const FIRST_JOB_RUN_ATTEMPT: i32 = 0;

/// The prefix a Glue job run id carries, which is what makes an `IdString` recognisable
/// as a job run id rather than any other id the service mints.
pub const JOB_RUN_ID_PREFIX: &str = "jr_";

/// How many runs one job definition keeps.
///
/// CHOSEN: Glue publishes no retention for job-run records, so this is the emulator's own
/// window. It bounds [`JobEntry::runs`], and with it every per-job walk, so a job that
/// has been started many times still costs a bounded amount of work and memory. The
/// OLDEST record is dropped first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_JOB: usize = 100;

/// How many job run ids this process has minted. A nonce source for
/// [`new_job_run_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
static MINTED_JOB_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `JobRunId`: the id the service assigns a job run when it starts.
///
/// [`JOB_RUN_ID_PREFIX`] followed by 64 lowercase hexadecimal characters, i.e. 256 bits,
/// which is the shape a Glue job run id has on the wire and is inside `IdString`'s
/// `length 1..255`. The bits come from [`mint_hex_id`], as a database id's do,
/// so two runs started in the same nanosecond cannot be given the same id. It is stored
/// on the run, so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_job_run_id() -> String {
    format!(
        "{JOB_RUN_ID_PREFIX}{}",
        mint_hex_id(&MINTED_JOB_RUN_IDS, JOB_RUN_ID_WORDS)
    )
}

/// How many 64-bit words a Glue job run id carries: four, i.e. 256 bits rendered as 64
/// lowercase hexadecimal characters.
const JOB_RUN_ID_WORDS: usize = 4;

/// One run of a job definition, as `StartJobRun` recorded it.
///
/// A run carries the job's own configuration with the members the `StartJobRun` request
/// overrode applied on top — "This value overrides the timeout value set in the parent
/// job" (`StartJobRun$Timeout`) is the rule for each of them — plus the members only a
/// run has: its id, its attempt, its state and the instants it moved between them.
#[derive(Debug, Clone)]
pub struct JobRunEntry {
    /// `JobRun$Id`, minted by [`new_job_run_id`].
    pub id: String,
    /// The name of the job definition this is a run of.
    pub job_name: String,
    /// `JobRun$Attempt`; one past the retried run's attempt when this run retries one.
    pub attempt: i32,
    /// `JobRun$PreviousRunId`: "the `JobRunId` specified in the `StartJobRun` action",
    /// i.e. the run this one retries, when the request named one.
    pub previous_run_id: Option<String>,
    pub job_mode: String,
    pub job_run_queuing_enabled: Option<bool>,
    /// Server-set: the instant `StartJobRun` recorded this run.
    pub started_on: SystemTime,
    /// Server-set: moves with every state change of this run.
    pub last_modified_on: SystemTime,
    /// Server-set when the run reaches a terminal state, and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `JobRunState` values.
    pub state: String,
    /// The instant the clock owes this run its next state; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The `JobRunState` the clock owes this run at [`JobRunEntry::settles_at`]: the
    /// state a run nothing executed comes to rest in, and [`JOB_RUN_STATE_STOPPED`] once
    /// a stop has been submitted for it.
    pub settles_to: String,
    /// The job's `DefaultArguments` with this run's `Arguments` applied over them, then
    /// the job's `NonOverridableArguments`: "For this job run, they replace the default
    /// arguments set in the job definition itself" (`StartJobRun$Arguments`), except for
    /// the "arguments for this job that are not overridden when providing job arguments
    /// in a job run" (`CreateJob$NonOverridableArguments`).
    pub arguments: HashMap<String, String>,
    pub error_message: Option<String>,
    pub max_capacity: Option<f64>,
    pub timeout: i32,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub security_configuration: Option<String>,
    pub notification_property: Option<NotificationProperty>,
    pub glue_version: String,
    pub execution_class: Option<String>,
    pub maintenance_window: Option<String>,
    pub execution_role_session_policy: Option<String>,
}

impl JobRunEntry {
    /// Record a run of `job` as the `StartJobRun` request asks for it, in
    /// [`JOB_RUN_STATE_STARTING`] with its next state due [`JOB_RUN_SETTLES_AFTER`] out.
    ///
    /// `retries` is the `(id, attempt)` of the run this one retries, when the request's
    /// `JobRunId` ("The ID of a previous JobRun to retry") named one.
    pub fn start(
        job: &JobEntry,
        input: &StartJobRunInput,
        id: String,
        now: SystemTime,
        retries: Option<(String, i32)>,
    ) -> Self {
        let (previous_run_id, attempt) = match retries {
            Some((previous_id, previous_attempt)) => {
                (Some(previous_id), previous_attempt.saturating_add(1))
            }
            None => (None, FIRST_JOB_RUN_ATTEMPT),
        };
        // Each capacity member of the run is the one the request named, else the job's,
        // and the DPU figure is then derived from that pair exactly as a job's is.
        let worker_type = input.worker_type.clone().or_else(|| job.worker_type.clone());
        let number_of_workers = input.number_of_workers.or(job.number_of_workers);
        let max_capacity = capacity_from(
            input.max_capacity,
            input.allocated_capacity,
            worker_type.as_deref(),
            number_of_workers,
        )
        .or(job.max_capacity);

        let mut arguments = job.default_arguments.clone().unwrap_or_default();
        if let Some(requested) = input.arguments.as_ref() {
            arguments.extend(
                requested
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        if let Some(locked) = job.non_overridable_arguments.as_ref() {
            arguments.extend(
                locked
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }

        Self {
            id,
            job_name: job.name.clone(),
            attempt,
            previous_run_id,
            job_mode: job.job_mode.clone(),
            // "If this field does not match the value set in the job run, then the value
            // from the job run field will be used."
            job_run_queuing_enabled: input.job_run_queuing_enabled.or(job.job_run_queuing_enabled),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            state: JOB_RUN_STATE_STARTING.to_string(),
            settles_at: Some(now.checked_add(JOB_RUN_SETTLES_AFTER).unwrap_or(now)),
            // Nothing executes the job script here, so the state the clock owes a run
            // that nobody stops is the modelled failure state.
            settles_to: JOB_RUN_STATE_FAILED.to_string(),
            arguments,
            // Nothing has failed yet: a run carries an error message only once it has.
            error_message: None,
            max_capacity,
            timeout: input.timeout.unwrap_or(job.timeout),
            worker_type,
            number_of_workers,
            security_configuration: input
                .security_configuration
                .clone()
                .or_else(|| job.security_configuration.clone()),
            notification_property: input
                .notification_property
                .clone()
                .or_else(|| job.notification_property.clone()),
            // `GlueVersion` is not a `StartJobRun` member: a run uses its job's.
            glue_version: job.glue_version.clone(),
            execution_class: input
                .execution_class
                .clone()
                .or_else(|| job.execution_class.clone()),
            // Nor is `MaintenanceWindow`.
            maintenance_window: job.maintenance_window.clone(),
            execution_role_session_policy: input.execution_role_session_policy.clone(),
        }
    }

    /// Whether this run has finished, by [`TERMINAL_JOB_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_JOB_RUN_STATES.contains(&self.state.as_str())
    }

    /// Move this run into the state the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave the state a call put it in at [`JobRunEntry::settles_at`]
    /// whether or not anybody called, and the instants recorded are that DUE instant, not
    /// the instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.state = self.settles_to.clone();
        // `JobRun$ErrorMessage` is "the error message associated with this job run": the
        // run that comes to rest in the failure state carries the reason it never ran,
        // while one a submitted stop brought to rest failed at nothing and carries none.
        if self.state == JOB_RUN_STATE_FAILED {
            self.error_message = Some(JOB_RUN_NOT_EXECUTED_MESSAGE.to_string());
        }
        if self.is_terminal() {
            self.completed_on = Some(due);
        }
        self.last_modified_on = due;
    }

    /// Submit a stop for this run, as `BatchStopJobRun` asks for one, and answer whether
    /// the submission was accepted.
    ///
    /// A run that has not finished moves into [`JOB_RUN_STATE_STOPPING`] with
    /// [`JOB_RUN_STATE_STOPPED`] owed one span later, so a caller reading straight after
    /// the stop still sees the run stopping, and the slot it occupies is only freed once
    /// it has stopped. A run already stopping keeps the stop already owed it — a second
    /// submission for the same run neither postpones it nor records a different one — and
    /// a run that has already finished accepts no stop at all: there is nothing left to
    /// stop.
    pub fn submit_stop(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() {
            return false;
        }
        if self.state == JOB_RUN_STATE_STOPPING {
            return true;
        }
        self.state = JOB_RUN_STATE_STOPPING.to_string();
        self.settles_at = Some(now.checked_add(JOB_RUN_SETTLES_AFTER).unwrap_or(now));
        self.settles_to = JOB_RUN_STATE_STOPPED.to_string();
        self.last_modified_on = now;
        true
    }

    /// `JobRun$ExecutionTime`: "The amount of time (in seconds) that the job run consumed
    /// resources" — the whole seconds between the run starting and finishing, and the
    /// model's own default of 0 while it has not finished.
    fn execution_time_seconds(&self) -> i32 {
        let Some(completed_on) = self.completed_on else {
            return 0;
        };
        let elapsed = completed_on
            .duration_since(self.started_on)
            .unwrap_or_default();
        i32::try_from(elapsed.as_secs()).unwrap_or(i32::MAX)
    }

    /// Rebuild the modelled `JobRun` output shape from what is stored.
    pub fn to_job_run(&self) -> JobRun {
        JobRun {
            id: Some(self.id.clone()),
            attempt: self.attempt,
            previous_run_id: self.previous_run_id.clone(),
            // "The name of the trigger that started this job run": this run was started
            // through `StartJobRun`, so no trigger started it.
            trigger_name: None,
            job_name: Some(self.job_name.clone()),
            job_mode: Some(self.job_mode.clone()),
            job_run_queuing_enabled: self.job_run_queuing_enabled,
            started_on: Some(to_date_time(self.started_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            completed_on: self.completed_on.map(to_date_time),
            job_run_state: Some(self.state.clone()),
            arguments: Some(self.arguments.clone()),
            error_message: self.error_message.clone(),
            // "A job run that was used in the predicate of a conditional trigger that
            // triggered this job run" (`Predecessor`): a run started through
            // `StartJobRun` has none, and a run this one retries is not one of them —
            // that is `PreviousRunId`.
            predecessor_runs: None,
            // `AllocatedCapacity` is the same quantity as `MaxCapacity` — "This field is
            // deprecated. Use MaxCapacity instead" — so it reports the same number of
            // whole DPUs, and the model's own default of 0 for a run with no capacity.
            allocated_capacity: self.max_capacity.map_or(0, |capacity| capacity as i32),
            execution_time: self.execution_time_seconds(),
            timeout: Some(self.timeout),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            security_configuration: self.security_configuration.clone(),
            // "The name of the log group for secure logging": no job script runs here, so
            // nothing writes this run's output anywhere and there is no log group holding
            // it to name.
            log_group_name: None,
            notification_property: self.notification_property.clone(),
            glue_version: Some(self.glue_version.clone()),
            // "The total time each executor ran during the lifecycle of a job run in
            // seconds, multiplied by a DPU factor": no executor ran.
            dpu_seconds: None,
            execution_class: self.execution_class.clone(),
            maintenance_window: self.maintenance_window.clone(),
            // `ProfileName` names a usage profile, which neither `CreateJob` nor
            // `StartJobRun` can set.
            profile_name: None,
            // "For example, when a job run is in a WAITING state as a result of job run
            // queuing, the field has the reason why": this run was never queued, and the
            // reason it failed is its `ErrorMessage`.
            state_detail: None,
            execution_role_session_policy: self.execution_role_session_policy.clone(),
        }
    }
}

/// Whether `targets` names at least one data store to crawl, across every target list
/// `CrawlerTargets` carries. `CreateCrawler` documents that "at least one crawl target
/// must be specified", and an empty list names none.
pub fn has_crawl_target(targets: &CrawlerTargets) -> bool {
    fn any<T>(list: &Option<Vec<T>>) -> bool {
        list.as_ref().is_some_and(|entries| !entries.is_empty())
    }

    any(&targets.s3_targets)
        || any(&targets.jdbc_targets)
        || any(&targets.mongo_db_targets)
        || any(&targets.dynamo_db_targets)
        || any(&targets.catalog_targets)
        || any(&targets.delta_targets)
        || any(&targets.iceberg_targets)
        || any(&targets.hudi_targets)
}

/// Every DISTINCT Amazon SQS queue the crawl of `targets` consumes, in order: the
/// `EventQueueArn` of each S3 target that names one, beside the dead-letter queue it names
/// for the same crawl.
///
/// An S3 target naming an event queue is crawled FROM that queue — "a policy that
/// specifies whether to crawl the entire dataset again, or to crawl only folders that
/// were added since the last crawler run" (`CreateCrawler$RecrawlPolicy`) — so these are
/// the queues the crawler's role has to be able to consume. The list is DEDUPLICATED: the
/// same queue named by two targets is one queue to decide about, and the number of
/// decisions one request can ask for is what [`MAX_CHECKED_EVENT_QUEUES`] bounds.
pub fn crawl_event_queues(targets: &CrawlerTargets) -> Vec<String> {
    let mut queues: Vec<String> = targets
        .s3_targets
        .iter()
        .flatten()
        .flat_map(|target| {
            target
                .event_queue_arn
                .iter()
                .chain(target.dlq_event_queue_arn.iter())
        })
        .filter(|queue| !queue.trim().is_empty())
        .cloned()
        .collect();
    queues.sort();
    queues.dedup();
    queues
}

/// The IAM store a crawler's `Role` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so Glue reads the very records `CreateRole`,
/// `PutRolePolicy` and `AttachRolePolicy` wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live.
/// `None` means the engine carries no such store — a deployment with no IAM to ask,
/// which is evidence of nothing.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The Amazon SQS actions a crawler performs against the event queue of an S3 target it
/// crawls in event mode: it RECEIVES the change messages, reads the queue's attributes to
/// learn what is pending, and DELETES each message it has processed.
pub const EVENT_QUEUE_ACTIONS: [&str; 3] = [
    "sqs:ReceiveMessage",
    "sqs:GetQueueAttributes",
    "sqs:DeleteMessage",
];

/// The most policy documents this check reads for ONE role.
///
/// A role carries its inline policies plus the managed policies attached to it. A role
/// whose documents outnumber this is one the check does not finish reading, and an unread
/// document might be the one that carries the grant — so nothing is shown about such a
/// role and the create goes ahead. The count is taken BEFORE any document is read, so the
/// reading itself is bounded by it.
pub const MAX_ROLE_DOCUMENTS: usize = 32;

/// The most event queues one `CreateCrawler` request may have decided about.
///
/// CHOSEN, not documented: the target lists carry no modelled length, so the number of
/// distinct queues a request can name — and with it the number of policy evaluations the
/// create pays for, each over at most [`MAX_ROLE_DOCUMENTS`] documents — is bounded here
/// rather than left to the request. A create past the ceiling is refused with the
/// operation's own modelled `InvalidInputException`.
pub const MAX_CHECKED_EVENT_QUEUES: usize = 32;

/// The identity-based policy documents in force on the role a crawler's `Role` names: its
/// inline role policies plus the documents of the managed policies attached to it.
///
/// `Role` is "the IAM role or Amazon Resource Name (ARN) of an IAM role"
/// (`CreateCrawler$Role`), so a value that reads as this account's role ARN resolves to
/// the name it carries and anything else is taken as the role name itself.
///
/// `None` when nothing can be SHOWN about the role's permissions: this account holds no
/// such role, it carries more documents than [`MAX_ROLE_DOCUMENTS`] (counted before any is
/// read), or one of its attachments is an AWS-managed policy whose document IAM does not
/// carry. Called with the IAM guard held, and it CLONES what it reads, so the caller drops
/// the guard before deciding anything: nothing bounded by the request runs under it.
pub fn role_policy_documents(role: &str, iam: &IamState) -> Option<Vec<String>> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

    let name = ::service_common::iam_role::role_name(role).unwrap_or(role);
    let stored = iam.roles.get(&iam_store::name_key(name))?;
    let role_key = iam_store::name_key(&stored.role_name);
    let inline = iam.role_inline_policies.get(&role_key);
    let attached = iam.role_attached_policies.get(&role_key);
    // Counted FIRST, so a role carrying more documents than this check reads costs no
    // clone and no parse: an unread document might carry the grant, so nothing is shown
    // about such a role either way.
    let held = inline.map_or(0, |policies| policies.len())
        + attached.map_or(0, |attachments| attachments.len());
    if held > MAX_ROLE_DOCUMENTS {
        return None;
    }
    let mut documents: Vec<String> = inline
        .into_iter()
        .flat_map(|policies| policies.values())
        .map(|policy| policy.policy_document.clone())
        .collect();
    for arn in attached.into_iter().flatten() {
        match default_policy_document(iam, arn) {
            Some(document) => documents.push(document),
            // An AWS-managed policy ARN this emulator's IAM carries no document for might
            // be the one that carries the grant, and this account cannot read it: nothing
            // can be shown about this role. Which ARNs are in AWS's namespace is IAM's
            // OWN reading, called rather than re-derived.
            None if iam_store::is_aws_managed_policy_arn(arn) => return None,
            None => {}
        }
    }
    Some(documents)
}

/// Why the crawler role whose policies are `prepared` cannot consume the event queue
/// `queue_arn`, or `None` when those documents admit the whole crawl.
///
/// A crawler that crawls an S3 target from an event queue runs AS its `Role`, so the
/// policies in force on that role must admit the queue operations the crawl makes
/// ([`EVENT_QUEUE_ACTIONS`]) on that queue. The decision is made against the account's own
/// IAM records — the ones `CreateRole`/`PutRolePolicy`/`AttachRolePolicy` wrote, collected
/// by [`role_policy_documents`] — through the shared evaluator every service that decides
/// authorization uses, so no two services answer differently.
///
/// The documents are PREPARED once per request and passed in, so a request naming several
/// queues parses them once rather than once per queue, and the caller holds no IAM guard
/// while this runs.
pub fn event_queue_refusal(
    prepared: &[::service_common::iam_authorization::PreparedDocument],
    queue_arn: &str,
) -> Option<String> {
    let withheld = EVENT_QUEUE_ACTIONS.iter().any(|action| {
        let request = ::service_common::iam_authorization::Request::new(action, queue_arn, &[]);
        let stated =
            ::service_common::iam_authorization::identity_decision_prepared(prepared, &request);
        // Nothing stated is an implicit deny in IAM, and the documents in force on the
        // role ARE the authority on what it may do: a role whose policies say nothing
        // about the call may not make it.
        stated != ::service_common::iam_authorization::Decision::Allowed
    });
    withheld.then(|| messages::event_queue_inaccessible(queue_arn))
}

/// The default version's document of the managed policy `arn` names, or `None` when the
/// account holds no such policy and it is none of the AWS-managed policies IAM carries.
fn default_policy_document(iam: &IamState, arn: &str) -> Option<String> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

    // An AWS-managed policy is not account state: IAM holds its document in its own
    // catalogue, so a role's grant that lives in one is read from there.
    if let Some(managed) = iam_store::aws_managed_policy(arn) {
        return Some(managed.document.to_string());
    }
    let managed = iam.policy_by_arn(arn)?;
    managed
        .policy
        .default_version_id
        .as_deref()
        .and_then(|default| managed.versions.get(default))
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}

/// Whether an `UpdateCrawler` request names at least one member of the crawler to
/// change.
///
/// `Name` ADDRESSES the crawler rather than changing it (`UpdateCrawlerRequest` has no
/// rename member), so a request carrying nothing besides it asks for no change: there is
/// nothing to write, and the crawler's `Version` and `LastUpdated` stay where the last
/// write that did change something left them. No recording or doc states what the service
/// answers such a call, so what is NOT done here is advancing a write counter for a change
/// nobody asked for; a recording of that call would settle it. Written as an exhaustive
/// destructuring so a member added to the request shape cannot be silently left out of the
/// answer.
pub fn crawler_update_names_a_change(input: &UpdateCrawlerInput) -> bool {
    let UpdateCrawlerInput {
        name: _,
        role,
        database_name,
        description,
        targets,
        schedule,
        classifiers,
        table_prefix,
        schema_change_policy,
        recrawl_policy,
        lineage_configuration,
        lake_formation_configuration,
        configuration,
        crawler_security_configuration,
        catalog_id,
    } = input;

    role.is_some()
        || database_name.is_some()
        || description.is_some()
        || targets.is_some()
        || schedule.is_some()
        || classifiers.is_some()
        || table_prefix.is_some()
        || schema_change_policy.is_some()
        || recrawl_policy.is_some()
        || lineage_configuration.is_some()
        || lake_formation_configuration.is_some()
        || configuration.is_some()
        || crawler_security_configuration.is_some()
        || catalog_id.is_some()
}

/// Whether `expression` has the form the service documents for a crawler schedule: a
/// `cron(..)` expression of six fields ("to run something every day at 12:15 UTC, you
/// would specify: `cron(15 12 * * ? *)`" — `Schedule$ScheduleExpression`). Only the
/// form is checked here; the field values are the scheduler's business.
pub fn is_cron_expression(expression: &str) -> bool {
    let trimmed = expression.trim();
    let Some(fields) = trimmed
        .strip_prefix("cron(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    fields.split_whitespace().count() == 6
}

/// The `VersionId` a table carries straight after `CreateTable`.
///
/// The catalog's table version history starts at 0: the version a create makes is
/// version `"0"`, and each update appends the next one. So a table that has never been
/// updated reports `"0"`, and that is the id `GetTableVersion`/`DeleteTableVersion`
/// address its only version by — numbering the first version `"1"` left version `"0"`
/// unfetchable. `VersionString` is a string, so the id is formatted as one.
pub const FIRST_TABLE_VERSION_ID: &str = "0";

/// A stored connection definition: the `Connection` every read projects, plus the tags the
/// create carried.
#[derive(Debug, Clone)]
pub struct ConnectionEntry {
    /// `Name`, which is also this entry's key in [`Catalog::connections`].
    pub name: String,
    /// What a read of this connection reports, before `HidePassword` is applied.
    pub definition: Connection,
    /// `CreateConnectionRequest$Tags`.
    pub tags: HashMap<String, String>,
}

impl ConnectionEntry {
    /// Build a stored connection definition from the client's `ConnectionInput`.
    ///
    /// `CreationTime` and `LastUpdatedTime` are both the instant of the create (nothing has
    /// updated the definition yet), `LastUpdatedBy` is the principal that issued it, and
    /// `ConnectionSchemaVersion` is 2 exactly when the input carried the
    /// per-compute-environment property maps version 2 adds ("Version 2 supports properties
    /// for specific compute environments"), else 1.
    pub fn from_input(
        input: &ConnectionInput,
        last_updated_by: Option<String>,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        let compute_environment_properties = [
            input.spark_properties.as_ref(),
            input.athena_properties.as_ref(),
            input.python_properties.as_ref(),
        ]
        .iter()
        .any(|properties| properties.is_some_and(|map| !map.is_empty()));
        let at = to_date_time(now);
        Self {
            name: input.name.clone(),
            definition: Connection {
                name: Some(input.name.clone()),
                description: input.description.clone(),
                connection_type: Some(input.connection_type.clone()),
                match_criteria: input.match_criteria.clone(),
                connection_properties: Some(input.connection_properties.clone()),
                spark_properties: input.spark_properties.clone(),
                athena_properties: input.athena_properties.clone(),
                python_properties: input.python_properties.clone(),
                physical_connection_requirements: input.physical_connection_requirements.clone(),
                creation_time: Some(at),
                last_updated_time: Some(at),
                last_updated_by,
                // `Status`/`StatusReason` report the outcome of VALIDATING the connection
                // ("Can be one of: READY, IN_PROGRESS, or FAILED"), and this emulator
                // connects to nothing, so it reports no status and no validation instant
                // rather than inventing one.
                status: None,
                status_reason: None,
                last_connection_validation_time: None,
                // The authentication configuration a read reports is the one the input
                // declared, with its credentials left where they were: an
                // `AuthenticationConfigurationInput` carries secrets
                // (`BasicAuthenticationCredentials`, OAuth2 credentials) that the response
                // shape has no member for, and nothing here copies them into one.
                authentication_configuration: input.authentication_configuration.as_ref().map(
                    |configuration| crate::ops::types::AuthenticationConfiguration {
                        authentication_type: configuration.authentication_type.clone(),
                        secret_arn: configuration.secret_arn.clone(),
                        kms_key_arn: configuration.kms_key_arn.clone(),
                        o_auth2_properties: None,
                    },
                ),
                connection_schema_version: Some(if compute_environment_properties {
                    CONNECTION_SCHEMA_VERSION_V2
                } else {
                    CONNECTION_SCHEMA_VERSION_V1
                }),
                // "A list of compute environments compatible with the connection" is a
                // property of the connection TYPE, which this emulator does not model.
                compatible_compute_environments: None,
            },
            tags: tags.cloned().unwrap_or_default(),
        }
    }

    /// Overwrite this connection's definition with the one the client submitted.
    ///
    /// `ConnectionInput` is the WHOLE client-settable definition of a connection — the
    /// same shape a create submits, "used to specify a connection to create or update" —
    /// so an update writes every member it carries and clears the ones it omits, and
    /// `ConnectionSchemaVersion` is recomputed from what it carries exactly as at create.
    /// The members no `ConnectionInput` carries keep their own rules, because the client
    /// cannot set them: `CreationTime` is the instant of the CREATE and never moves,
    /// `LastUpdatedTime` and `LastUpdatedBy` become this call's instant and principal, and
    /// the tags are written through the tagging operations rather than by this update.
    ///
    /// The submitted `Name` is written like any other member; relocating the entry under
    /// a new key would be the caller's job, since the key lives in
    /// [`Catalog::connections`].
    pub fn apply_input(
        &mut self,
        input: &ConnectionInput,
        last_updated_by: Option<String>,
        now: SystemTime,
    ) {
        // One function decides what a `ConnectionInput` means, so an update projects it
        // exactly as a create does; only the create's own instant is carried over.
        let created_at = self.definition.creation_time;
        let rewritten = Self::from_input(input, last_updated_by, None, now);
        self.name = rewritten.name;
        self.definition = rewritten.definition;
        self.definition.creation_time = created_at;
    }

    /// What a read of this connection reports.
    ///
    /// `hide_password` is `GetConnectionRequest$HidePassword`: "Allows you to retrieve the
    /// connection metadata without returning the password", and
    /// `compute_environment` is `GetConnectionRequest$ApplyOverrideForComputeEnvironment`:
    /// "For connections that may be used in multiple services, specifies returning
    /// properties for the specified compute environment" — that environment's own property
    /// map overlays the connection's, key by key, so a property only that environment sets
    /// is reported and one it does not set keeps the connection's value.
    ///
    /// The overlay is applied FIRST and the password properties are withheld from EVERY
    /// property map the answer carries — the connection's own and each
    /// per-compute-environment one — because a per-environment map may itself carry a
    /// `PASSWORD`, and "without returning the password" is about the password, not about
    /// one member.
    pub fn to_connection(
        &self,
        hide_password: bool,
        compute_environment: Option<&str>,
    ) -> Connection {
        let mut connection = self.definition.clone();
        let overrides = match compute_environment {
            Some(COMPUTE_ENVIRONMENT_SPARK) => connection.spark_properties.clone(),
            Some(COMPUTE_ENVIRONMENT_ATHENA) => connection.athena_properties.clone(),
            Some(COMPUTE_ENVIRONMENT_PYTHON) => connection.python_properties.clone(),
            // The model's `ComputeEnvironment` enum carries exactly those three, and the
            // request walk refuses any other value before a handler runs.
            _ => None,
        };
        if let Some(overrides) = overrides {
            connection
                .connection_properties
                .get_or_insert_with(Default::default)
                .extend(overrides);
        }
        if hide_password {
            for properties in [
                connection.connection_properties.as_mut(),
                connection.spark_properties.as_mut(),
                connection.athena_properties.as_mut(),
                connection.python_properties.as_mut(),
            ]
            .into_iter()
            .flatten()
            {
                properties.remove(PASSWORD_PROPERTY);
                properties.remove(ENCRYPTED_PASSWORD_PROPERTY);
            }
        }
        connection
    }
}

// ---------------------------------------------------------------------------
// The schema registry
// ---------------------------------------------------------------------------

/// The registry a schema-registry call that names no registry is served from.
///
/// `CreateSchema` documents the name: "When this API is called without a `RegistryId`,
/// this will create an entry for a "default-registry" in the registry database tables, if
/// it is not already present." It is the one registry name a create may bring into
/// existence, so it resolves to the same create-if-absent whether a request defaults to it
/// or names it outright; any other name must already have been created.
pub const DEFAULT_REGISTRY_NAME: &str = "default-registry";

/// Ceiling on the schema registries this emulator will hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: resident memory bounded against a create any caller can
/// reach. A create past it is refused with the operation's own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_REGISTRIES: usize = 100;

/// Ceiling on the schema sets one registry may hold, for the same reason as
/// [`MAX_REGISTRIES`].
pub const MAX_SCHEMAS_PER_REGISTRY: usize = 1_000;

/// Ceiling on the schema sets the whole store may hold, across every registry.
///
/// Bounds the product of the two ceilings above, which on its own would still let a caller
/// grow the store far past what one process should hold: a schema set keeps every
/// `SchemaDefinition` registered against it verbatim, and the modelled
/// `SchemaDefinitionString` admits 170000 characters apiece, so the bound that matters is
/// the one on how many of those the store holds at once.
pub const MAX_STORED_SCHEMAS: usize = 2_000;

/// `RegistryStatus` for a registry this store holds. The enum carries exactly two values
/// and the other one (`DELETING`) belongs to a registry a delete is draining, so a
/// registry that is simply present is this one.
pub const REGISTRY_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `SchemaStatus` for a schema set this store holds. Nothing here is provisioned
/// asynchronously, so a schema set a create returned is readable at once rather than
/// passing through `PENDING`.
pub const SCHEMA_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `SchemaVersionStatus` for a stored schema version, for the same reason as
/// [`SCHEMA_STATUS_AVAILABLE`].
pub const SCHEMA_VERSION_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `Compatibility` mode recorded for a schema created without one.
///
/// Source: AWS's schema-registry documentation, which names BACKWARD the default
/// compatibility mode of a new schema and recommends it ("This compatibility choice is
/// recommended as it allows data receivers to read both the current and one previous
/// schema version" — `CreateSchemaInput$Compatibility`). A schema always has a mode: it is
/// what every later version registration is checked against, and what
/// [`SchemaEntry::checkpoint`] records a change of.
pub const DEFAULT_COMPATIBILITY: &str = "BACKWARD";

/// The number of the first version registered against a schema set, which is also the
/// minimum the modelled `VersionLongNumber` admits (range 1..100000).
pub const FIRST_SCHEMA_VERSION: i64 = 1;

/// The version number reported for a schema set that holds no version yet — the one before
/// [`FIRST_SCHEMA_VERSION`], i.e. the version a first registration advances FROM.
///
/// `CreateSchema$SchemaDefinition` is optional, so a schema set can exist with no version;
/// its latest version and its checkpoint are then this, and its next version is
/// [`FIRST_SCHEMA_VERSION`].
pub const NO_SCHEMA_VERSION: i64 = 0;

/// The ARN resource-type segment of a schema registry (`registry/<name>`), the way
/// `CreateSchema$RegistryId` spells it ("The ARN format for the same will be:
/// arn:aws:glue:us-east-2::registry/default-registry…").
pub const REGISTRY_ARN_SEGMENT: &str = "registry";

/// The ARN resource-type segment of a schema set. A schema lives inside a registry, so its
/// resource portion carries BOTH names: `schema/<registry>/<schema>`.
pub const SCHEMA_ARN_SEGMENT: &str = "schema";

/// The ARN of registry `registry_name` as served in `region`.
pub fn registry_arn(region: &str, registry_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{REGISTRY_ARN_SEGMENT}/{registry_name}"),
    )
}

/// The ARN of schema set `schema_name` of registry `registry_name`, as served in `region`.
pub fn schema_arn(region: &str, registry_name: &str, schema_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{SCHEMA_ARN_SEGMENT}/{registry_name}/{schema_name}"),
    )
}

/// The identity fields and resource portion of a Glue resource ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlueArn {
    /// Field 4 of the ARN: the Region whose estate the resource belongs to.
    pub region: String,
    /// Field 5: the account that owns it.
    pub account_id: String,
    /// The sixth field and everything after it, colons included.
    pub resource: String,
}

impl GlueArn {
    /// Whether this ARN names the estate this endpoint serves: its own account, and the
    /// Region the request asking is served in. An ARN of any other account or Region names
    /// a resource this process does not hold, whatever it holds under that name.
    pub fn is_local(&self, region: &str) -> bool {
        is_served_account(&self.account_id) && self.region == region
    }

    /// This ARN's resource portion with the resource-type segment `segment` and its `/`
    /// stripped — the `default-registry` of `registry/default-registry`, the
    /// `default-registry/payments` of `schema/default-registry/payments`.
    ///
    /// `None` when the resource names another type, or carries no name after the segment.
    pub fn resource_of_type(&self, segment: &str) -> Option<&str> {
        let (kind, name) = self.resource.split_once('/')?;
        (kind == segment && !name.is_empty()).then_some(name)
    }
}

/// `arn`'s identity fields and resource portion when it is a well-formed Glue resource
/// ARN, or `None` when it is not one — a value in another service's namespace, or an ARN
/// missing a field.
///
/// The ONE split this crate does of a Glue ARN, so no two operations can disagree about
/// what one is. Only the fields are read here; which resource TYPE the ARN names
/// ([`GlueArn::resource_of_type`]), whether it names this endpoint's estate
/// ([`GlueArn::is_local`]) and whether this store holds the resource are separate answers.
pub fn parse_glue_arn(arn: &str) -> Option<GlueArn> {
    // `arn:<partition>:glue:<region>:<account>:<resource>`. The resource is the LAST
    // field, so it keeps any colon and any `/` of its own.
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != ARN_NAMESPACE {
        return None;
    }
    Some(GlueArn {
        region: fields[3].to_string(),
        account_id: fields[4].to_string(),
        resource: fields[5].to_string(),
    })
}

/// What a `RegistryId` or `SchemaId` resolves to.
///
/// Both wrappers may address their resource by name or by ARN, and an ARN carries an
/// account and a Region of its own — so a well-formed reference can name an estate this
/// endpoint does not serve. That is not a bad parameter and not a local resource either:
/// it is a reference this endpoint holds nothing for, whatever it holds under that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference<T> {
    /// A reference into this endpoint's own estate, resolved to its names.
    Local(T),
    /// A well-formed ARN of another account or another Region, resolved to the names it
    /// carries so a refusal can say what was looked for.
    Elsewhere(T),
}

/// The sentence a refusal carries for a `RegistryId` that names neither of the two members
/// one has to carry. Stated by the model: "One of `RegistryArn` or `RegistryName` has to
/// be provided" (`RegistryId$RegistryName`).
pub const REGISTRY_ID_REQUIRES_ONE: &str =
    "One of RegistryArn or RegistryName has to be provided.";

/// The same for a `SchemaId`, from `SchemaId$SchemaArn`: "One of `SchemaArn` or
/// `SchemaName` has to be provided."
pub const SCHEMA_ID_REQUIRES_ONE: &str = "One of SchemaArn or SchemaName has to be provided.";

/// The registry a `RegistryId` names, as served in `region`.
///
/// `Err` is a bad parameter — neither member carried, an `RegistryArn` that is not a Glue
/// registry ARN, or two members naming different registries — and its string is the
/// refusal's message, derived from the member that is wrong.
pub fn registry_reference(
    registry_id: &RegistryId,
    region: &str,
) -> Result<Reference<String>, String> {
    let from_arn = match non_empty(registry_id.registry_arn.as_deref()) {
        Some(arn) => {
            let not_a_registry =
                || format!("RegistryArn {arn:?} is not the ARN of a Glue schema registry");
            let Some(parsed) = parse_glue_arn(arn) else {
                return Err(not_a_registry());
            };
            // A registry's resource portion is its name alone; anything deeper names
            // something else living under the segment, not a registry.
            let name = parsed
                .resource_of_type(REGISTRY_ARN_SEGMENT)
                .filter(|name| !name.contains('/'))
                .map(str::to_string);
            let Some(name) = name else {
                return Err(not_a_registry());
            };
            if !parsed.is_local(region) {
                return Ok(Reference::Elsewhere(name));
            }
            Some(name)
        }
        None => None,
    };
    match (from_arn, non_empty(registry_id.registry_name.as_deref())) {
        (None, None) => Err(REGISTRY_ID_REQUIRES_ONE.to_string()),
        (Some(name), None) => Ok(Reference::Local(name)),
        (None, Some(name)) => Ok(Reference::Local(name.to_string())),
        (Some(from_arn), Some(name)) if from_arn == name => Ok(Reference::Local(from_arn)),
        (Some(from_arn), Some(name)) => Err(format!(
            "RegistryArn names registry {from_arn} but RegistryName is {name}; both members \
             must name the same registry"
        )),
    }
}

/// The registry and schema names a `SchemaId` names, as served in `region`.
///
/// A `SchemaId` that names a schema by name and no registry addresses the registry a
/// schema-registry call defaults to ([`DEFAULT_REGISTRY_NAME`]), which is the registry
/// `CreateSchema` writes into when the create named none.
///
/// `Err` carries the refusal's message, like [`registry_reference`]'s.
pub fn schema_reference(
    schema_id: &SchemaId,
    region: &str,
) -> Result<Reference<(String, String)>, String> {
    let from_arn = match non_empty(schema_id.schema_arn.as_deref()) {
        Some(arn) => {
            let not_a_schema = || format!("SchemaArn {arn:?} is not the ARN of a Glue schema");
            let Some(parsed) = parse_glue_arn(arn) else {
                return Err(not_a_schema());
            };
            // `schema/<registry>/<schema>`: exactly two names, so a schema name carrying a
            // further `/` is not this ARN's shape.
            let names = parsed
                .resource_of_type(SCHEMA_ARN_SEGMENT)
                .and_then(|resource| resource.split_once('/'))
                .filter(|(registry, schema)| !registry.is_empty() && !schema.contains('/'))
                .map(|(registry, schema)| (registry.to_string(), schema.to_string()));
            let Some(names) = names else {
                return Err(not_a_schema());
            };
            if !parsed.is_local(region) {
                return Ok(Reference::Elsewhere(names));
            }
            Some(names)
        }
        None => None,
    };
    let by_name = non_empty(schema_id.schema_name.as_deref()).map(|schema| {
        let registry = non_empty(schema_id.registry_name.as_deref())
            .unwrap_or(DEFAULT_REGISTRY_NAME)
            .to_string();
        (registry, schema.to_string())
    });
    match (from_arn, by_name) {
        (None, None) => Err(SCHEMA_ID_REQUIRES_ONE.to_string()),
        (Some(named), None) => Ok(Reference::Local(named)),
        (None, Some(named)) => Ok(Reference::Local(named)),
        (Some(from_arn), Some(by_name)) if from_arn == by_name => Ok(Reference::Local(from_arn)),
        (Some(from_arn), Some(by_name)) => Err(format!(
            "SchemaArn names schema {}/{} but SchemaName names {}/{}; both must name the \
             same schema",
            from_arn.0, from_arn.1, by_name.0, by_name.1
        )),
    }
}

/// `value` when it carries a non-empty string. An absent member and one sent as `""` say
/// the same thing about which member a wrapper carried, and `""` satisfies no name's
/// modelled pattern.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// One schema registry: the schema sets stored under a single `RegistryName`.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// `RegistryName`, which is also this entry's key in
    /// [`ComAmazonawsGlueAwsglueState::registries`].
    pub name: String,
    /// `CreateRegistry$Description`, absent for a registry created without one and for the
    /// default registry a schema create materialises.
    pub description: Option<String>,
    /// `RegistryStatus`: [`REGISTRY_STATUS_AVAILABLE`] for every registry this store holds.
    pub status: String,
    /// When the registry was created, reported as `CreatedTime`.
    pub created_time: SystemTime,
    /// When it last changed, reported as `UpdatedTime`. Equal to [`Self::created_time`]
    /// until something updates the registry itself — a schema created inside it does not
    /// change the registry.
    pub updated_time: SystemTime,
    /// `CreateRegistry$Tags`.
    pub tags: HashMap<String, String>,
    /// The schema sets this registry holds, keyed by `SchemaName`, in name order.
    pub schemas: BTreeMap<String, SchemaEntry>,
}

impl RegistryEntry {
    /// A registry created at `now`, with the description and tags its create carried.
    pub fn new(
        name: &str,
        description: Option<String>,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            description,
            status: REGISTRY_STATUS_AVAILABLE.to_string(),
            created_time: now,
            updated_time: now,
            tags: tags.cloned().unwrap_or_default(),
            schemas: BTreeMap::new(),
        }
    }

    /// The [`DEFAULT_REGISTRY_NAME`] entry a schema create materialises when it is not
    /// already present. It carries no description and no tags: nothing asked for it, so
    /// there is nothing to record on it.
    pub fn default_registry(now: SystemTime) -> Self {
        Self::new(DEFAULT_REGISTRY_NAME, None, None, now)
    }

    /// Whether this registry can hold another schema set, i.e. is below
    /// [`MAX_SCHEMAS_PER_REGISTRY`].
    pub fn can_store_schema(&self) -> bool {
        self.schemas.len() < MAX_SCHEMAS_PER_REGISTRY
    }
}

/// One schema set: the compatibility settings a `CreateSchema` recorded, plus the versions
/// registered against it.
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    /// `SchemaName`, which is also this entry's key in [`RegistryEntry::schemas`].
    pub name: String,
    /// The name of the registry holding it, so a read of the schema alone can report the
    /// registry it belongs to.
    pub registry_name: String,
    /// `CreateSchema$Description`: "If description is not provided, there will not be any
    /// automatic default value for this", hence `Option` rather than an empty string.
    pub description: Option<String>,
    /// `DataFormat`, the format every version's definition is written in.
    pub data_format: String,
    /// `Compatibility`, the mode later version registrations are checked against
    /// ([`DEFAULT_COMPATIBILITY`] for a create that named none).
    pub compatibility: String,
    /// `SchemaStatus`: [`SCHEMA_STATUS_AVAILABLE`] for every schema set this store holds.
    pub status: String,
    /// `SchemaCheckpoint`, "the version number of the checkpoint (the last time the
    /// compatibility mode was changed)": the version in force when the mode was last set,
    /// which at create is the version the create registered — or [`NO_SCHEMA_VERSION`] when
    /// it registered none.
    pub checkpoint: i64,
    /// When the schema set was created, reported as `CreatedTime`.
    pub created_time: SystemTime,
    /// When it last changed, reported as `UpdatedTime`.
    pub updated_time: SystemTime,
    /// `CreateSchema$Tags`.
    pub tags: HashMap<String, String>,
    /// The versions registered against it, in registration order, so the last is the
    /// latest.
    pub versions: Vec<SchemaVersionEntry>,
}

impl SchemaEntry {
    /// `LatestSchemaVersion`, "the latest version of the schema associated with the
    /// returned schema definition": the number of the last version registered, or
    /// [`NO_SCHEMA_VERSION`] for a schema set that holds none.
    pub fn latest_schema_version(&self) -> i64 {
        self.versions
            .last()
            .map_or(NO_SCHEMA_VERSION, |version| version.version_number)
    }

    /// `NextSchemaVersion`: the number the next registration would take, which is one past
    /// [`Self::latest_schema_version`] — so a schema set holding no version reports
    /// [`FIRST_SCHEMA_VERSION`].
    pub fn next_schema_version(&self) -> i64 {
        self.latest_schema_version().saturating_add(1)
    }

    /// The last version registered against this schema set.
    pub fn latest_version(&self) -> Option<&SchemaVersionEntry> {
        self.versions.last()
    }
}

/// One version registered against a schema set.
#[derive(Debug, Clone)]
pub struct SchemaVersionEntry {
    /// `SchemaVersionId`, the version's own identifier: a version-4 UUID, which is the
    /// shape the modelled `SchemaVersionIdString` pins (len 36, pattern
    /// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). Minted once, at
    /// registration, so every later read of this version reports the same id.
    pub version_id: String,
    /// `VersionNumber`, counting from [`FIRST_SCHEMA_VERSION`].
    pub version_number: i64,
    /// The `SchemaDefinition` registered, stored verbatim: it is the artefact later reads
    /// return and later registrations are compared against.
    pub definition: String,
    /// `SchemaVersionStatus`.
    pub status: String,
    /// When the version was registered, reported as `CreatedTime`.
    pub created_time: SystemTime,
}

impl SchemaVersionEntry {
    /// The version `version_number` of a schema set, registered at `now` with `definition`.
    pub fn new(version_number: i64, definition: String, now: SystemTime) -> Self {
        Self {
            version_id: new_schema_version_id(),
            version_number,
            definition,
            status: SCHEMA_VERSION_STATUS_AVAILABLE.to_string(),
            created_time: now,
        }
    }
}

/// A fresh `SchemaVersionId`: a version-4 UUID, the shape the modelled
/// `SchemaVersionIdString` pins.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()` rendered
/// canonically), which this crate already depends on: nothing here is a hand-rolled
/// generator. This names a RESOURCE, not a credential.
pub fn new_schema_version_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A wall-clock instant as the schema registry reports it.
///
/// The registry's own timestamps (`CreatedTimestamp`, `UpdatedTimestamp`) are modelled as
/// STRINGS rather than as the timestamp type the catalog's shapes use, so the instant is
/// rendered as a date-time string here, once, and every schema-registry read formats it the
/// same way.
pub fn to_timestamp_string(at: SystemTime) -> String {
    to_date_time(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The counter a stored or requested `VersionId` spells, or `None` when it spells no
/// version counter.
///
/// "A `VersionId` is a string representation of an integer. Each version is incremented
/// by 1" (`GetTableVersionRequest$VersionId`), and only the server ever writes one — as
/// a decimal counter — so a value that spells no such integer names no version of any
/// table.
pub fn parse_version_counter(version_id: &str) -> Option<u64> {
    version_id.parse::<u64>().ok()
}

/// The `VersionId` that follows `current`: the stored counter plus one.
///
/// Only the server ever writes a `VersionId`, and it writes a decimal counter, so a
/// value that does not parse is treated as the version before the first one
/// ([`FIRST_TABLE_VERSION_ID`] minus one, i.e. the id a fresh table advances FROM)
/// rather than carried forward unchanged — a version must always move when the table
/// does.
pub fn next_version_id(current: &str) -> String {
    parse_version_counter(current)
        // Explicitly the version before the first one, not a general-purpose default.
        .unwrap_or(0)
        .saturating_add(1)
        .to_string()
}

/// The text the service puts in an error body's `Message` member.
///
/// Error wording here is PER OPERATION AND PER RESOURCE, never one template this
/// emulator invents and reuses: the service answers a fixed constant where its message
/// names no resource, and otherwise a template carrying only the request member the
/// refusal is about — never the name of a container the caller also sent, and never
/// emulator-side context such as the resolved `CatalogId`.
///
/// Four of these are pinned by the recorded wire corpus: `GetCrawler`'s crawler miss
/// ("Crawler entry with name `<Name>` does not exist"), `DeleteTable`'s table miss
/// ("Table `<Name>` not found."), `GetTable`'s table miss (the constant
/// "Entity Not Found", which names neither table nor database) and `CreateDatabase`'s
/// duplicate (the constant "Database already exists.", which withholds the requested
/// name). The rest are the same per-resource wording for the same refusal on the
/// sibling operations. Every handler refuses through these constructors, so one
/// operation's wording is never silently borrowed for another's refusal.
///
/// This is the TOP-LEVEL error body only. A per-item failure a batch reports inside its
/// output (`BatchDeleteTable`'s `TableError$ErrorDetail`) has its own recorded wording
/// and is built by [`super::catalog_error_detail`]. Of the job and connection slices only
/// `StartJobRun`'s missing-metadata refusal is here, because the corpus records it; the
/// rest of their messages are not requirements and are left to the issues that own them
/// rather than guessed at.
///
/// The schema-registry misses ARE here, because every operation that resolves a
/// `RegistryId` or a `SchemaId` resolves the same two references and so has to describe
/// the same two misses. No recording or doc states their wording either: what is pinned
/// is the behaviour — 400 with the operation's modelled `EntityNotFoundException` — and
/// the sentence follows the per-resource template the catalog slice's recorded misses
/// use.
pub mod messages {
    /// A crawler lookup (`GetCrawler`, and the `UpdateCrawler` that addresses one the
    /// same way) for a name no crawler in this account has. Built from the request's
    /// `Name`, which is stored and matched exactly as sent.
    pub fn crawler_not_found(name: &str) -> String {
        format!("Crawler entry with name {name} does not exist")
    }

    /// `UpdateCrawler` for a crawler whose crawl is still in progress ("if a crawler is
    /// running, you must stop it using `StopCrawler` before updating it"). NOT recorded:
    /// what is pinned is the behaviour — 400 with the operation's modelled
    /// `CrawlerRunningException` — and, like the recorded crawler miss above, a sentence
    /// carrying only the requested `Name`.
    pub fn crawler_running(name: &str) -> String {
        format!("Crawler with name {name} is running")
    }

    /// A write whose `Schedule` is not the `cron(..)` expression the service
    /// documents ([`super::is_cron_expression`]). NOT recorded: what is pinned is the
    /// behaviour — 400 with the operation's modelled `InvalidInputException` — and a
    /// sentence carrying only the requested expression plus the form it has to take.
    /// Shared by every operation that writes that member (`CreateCrawler`,
    /// `UpdateCrawler`, `UpdateCrawlerSchedule`, `CreateTrigger`), so one refusal cannot
    /// drift from another's.
    pub fn schedule_not_a_cron_expression(expression: &str) -> String {
        format!(
            "Schedule {expression} is not a cron expression of the form \
             cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)"
        )
    }

    /// `CreateCrawler` for a name already taken. NOT recorded: inferred from the
    /// recorded `CreateDatabase` duplicate, whose already-exists wording states the
    /// resource kind and withholds the name. What is pinned here is the behaviour — 400,
    /// `AlreadyExistsException`, and no disclosure of the requested name.
    pub const CRAWLER_ALREADY_EXISTS: &str = "Crawler already exists.";

    /// Any catalog operation whose `CatalogId` names a Data Catalog of ANOTHER account
    /// that no resource-based policy grants the caller
    /// ([`super::foreign_catalog_refusal`]). Recorded template: it names the calling
    /// principal, the IAM action the operation performs and the ARN of the catalog the
    /// request addressed, each read off the request and this emulator's own identity.
    pub fn not_authorized_on_catalog(action: &str, catalog_arn: &str) -> String {
        let prefix = super::IAM_ACTION_PREFIX;
        format!(
            "User: {} is not authorized to perform: {prefix}{action} on resource: \
             {catalog_arn} because no resource-based policy allows the {prefix}{action} \
             action",
            super::request_principal()
        )
    }

    /// A trigger lookup (`GetTrigger`, and every other operation that addresses a trigger
    /// by name) for a name no trigger of the request's Region has. NOT recorded: the
    /// sentence follows the per-resource template of [`database_not_found`], built from
    /// the requested name, which is the whole address of a trigger.
    pub fn trigger_not_found(name: &str) -> String {
        format!("Trigger {name} not found.")
    }

    /// `CreateTrigger` for a name the request's Region already holds a trigger under. NOT
    /// recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the requested name.
    pub const TRIGGER_ALREADY_EXISTS: &str = "Trigger already exists.";

    /// A trigger create whose `Actions` list is empty. NOT recorded: what is pinned is the
    /// behaviour — 400 with the operation's modelled `InvalidInputException` — and the
    /// sentence names the member that has to name something, the way the crawler create's
    /// own at-least-one refusal does.
    pub const TRIGGER_ACTIONS_REQUIRED: &str = "Actions must specify at least one action.";

    /// A trigger create missing the member its `Type` requires: `Schedule` for a scheduled
    /// trigger ("This field is required when the trigger type is SCHEDULED" —
    /// `CreateTriggerRequest$Schedule`) and `Predicate` for a conditional one ("This field
    /// is required when the trigger type is CONDITIONAL"). The sentence is those modelled
    /// requirements, with the member and the type the request named.
    pub fn trigger_member_required(member: &str, trigger_type: &str) -> String {
        format!("{member} is required when the trigger type is {trigger_type}")
    }

    /// A trigger create that asks for a type that cannot be started on creation ("True is
    /// not supported for ON_DEMAND triggers" — `CreateTriggerRequest$StartOnCreation`).
    /// The sentence is that modelled requirement, with the type the request named.
    pub fn start_on_creation_not_supported(trigger_type: &str) -> String {
        format!("StartOnCreation true is not supported for {trigger_type} triggers")
    }

    /// A trigger create whose `Predicate` lists several conditions without a `Logical`
    /// ("An optional field if only one condition is listed. If multiple conditions are
    /// listed, then this field is required" — `Predicate$Logical`). The sentence is that
    /// modelled requirement, with the number of conditions the request listed.
    pub fn predicate_logical_required(conditions: usize) -> String {
        format!("Logical is required when a Predicate lists {conditions} conditions")
    }

    /// Any operation whose `JobName`/`Name` names no job definition of the Region it is
    /// served in — a read or a write that addresses a job (`GetJob`, `UpdateJob`,
    /// `GetJobRun`, `BatchStopJobRun`), and the job an action of a trigger would run. The
    /// per-resource template this crate's job-miss refusals already carried inline, now in
    /// one place: it names neither the Region nor the resource that referred to the job.
    pub fn job_not_found(name: &str) -> String {
        format!("Job {name} not found")
    }

    /// `CreateCrawler` for an S3 target whose `EventQueueArn` (or `DlqEventQueueArn`)
    /// names a queue the crawler's `Role` cannot consume ([`super::event_queue_refusal`]).
    /// Recorded template: the refusal names the queue the create could not reach and
    /// states the two things that account for it — no such queue, or no access to it —
    /// without disclosing which, and names neither the role nor the action.
    pub fn event_queue_inaccessible(queue_arn: &str) -> String {
        format!(
            "SQS queue {queue_arn} does not exist or the role provided does not have \
             access to it."
        )
    }

    /// `GetJobRuns` for a `JobName` the endpoint's Region holds no job definition for.
    /// Recorded constant: the run listing's refusal names neither the job nor the Region.
    pub const JOB_RUNS_JOB_NOT_FOUND: &str = "Job not found.";

    /// `StartJobRun` for a `JobName` the endpoint's Region holds no job definition for —
    /// a name nothing created, or one created through ANOTHER Region's endpoint.
    /// Recorded constant: the start's refusal names the metadata it could not read rather
    /// than the requested name, which is why it is not [`super::messages`]'s per-resource
    /// template.
    pub const JOB_METADATA_MISSING: &str = "Failed to start job run due to missing metadata.";

    /// Any Data Catalog operation whose `DatabaseName`/`Name` names no database
    /// (`GetDatabase`, `DeleteDatabase`, `UpdateDatabase`, and the parent-database miss
    /// of every table operation: `GetTable`, `GetTables`, `CreateTable`, `UpdateTable`,
    /// `DeleteTable`, `BatchDeleteTable`). Built from the requested name alone — the
    /// catalog the emulator resolved is not part of the refusal.
    pub fn database_not_found(name: &str) -> String {
        format!("Database {name} not found.")
    }

    /// `CreateDatabase` (and `UpdateDatabase`, whose rename collides the same way) for
    /// a `DatabaseInput$Name` already taken. Recorded constant: the requested name is
    /// deliberately withheld.
    pub const DATABASE_ALREADY_EXISTS: &str = "Database already exists.";

    /// A READ that addresses ONE connection definition by name (`GetConnection`) for a
    /// name the addressed Data Catalog holds none under. Recorded template: it names the
    /// connection the request addressed and the account whose catalog was searched, both
    /// read off the request — the name exactly as sent, and the catalog id the request
    /// resolved to.
    pub fn connection_not_found(name: &str, catalog_id: &str) -> String {
        format!("Connection with name {name} in account {catalog_id} is not found")
    }

    /// A WRITE that addresses ONE connection definition by name (`UpdateConnection`, and
    /// the `DeleteConnection` that addresses one the same way) for a name the addressed
    /// Data Catalog holds none under. Recorded template of the update, and a DIFFERENT
    /// sentence from the read's above — which is why the two are not one shared string:
    /// it names the connection alone, built from the request, and no account.
    pub fn connection_not_found_for_write(name: &str) -> String {
        format!("Connection not found: {name}")
    }

    /// `GetTable` for a table the database does not hold. Recorded constant: this
    /// read's refusal names neither the table nor the database.
    pub const TABLE_ENTITY_NOT_FOUND: &str = "Entity Not Found";

    /// `DeleteTable` (and `UpdateTable`, the other write that addresses one table) for a
    /// table the database does not hold. Recorded template, built from the request's
    /// `Name` and naming no database — a different refusal from `GetTable`'s, which is
    /// why the two are not one shared string.
    pub fn table_not_found(name: &str) -> String {
        format!("Table {name} not found.")
    }

    /// `CreateTable` for a table name the database already holds. NOT recorded either:
    /// inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`].
    pub const TABLE_ALREADY_EXISTS: &str = "Table already exists.";

    /// A version read (`GetTableVersion`) for a version the addressed table's history
    /// does not hold. NOT recorded: the wording follows [`table_not_found`], the
    /// refusal for the resource one step up, and carries only the request's `VersionId`.
    pub fn table_version_not_found(version_id: &str) -> String {
        format!("Table version {version_id} not found.")
    }

    /// Any schema-registry operation whose `RegistryId` resolves to a registry this
    /// endpoint holds none of — the parent-registry miss of a schema create, and the miss
    /// of every operation that addresses a registry directly. Built from the resolved
    /// registry name alone, like [`database_not_found`].
    pub fn registry_not_found(name: &str) -> String {
        format!("Registry {name} not found.")
    }

    /// Any schema-registry operation whose `SchemaId` resolves to a schema set this
    /// endpoint holds none of (`GetSchema`, and the parent-schema miss of every version
    /// operation). Built from the resolved schema name alone — the registry the id
    /// resolved to is the container, which this crate's refusals do not name.
    pub fn schema_not_found(name: &str) -> String {
        format!("Schema {name} not found.")
    }

    /// `CreateSchema` for a `SchemaName` its registry already holds. NOT recorded:
    /// inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the requested name.
    pub const SCHEMA_ALREADY_EXISTS: &str = "Schema already exists.";

    /// A column statistics task run lookup (`GetColumnStatisticsTaskRun`) for an id no
    /// run in this account has. NOT recorded: the sentence follows the per-resource
    /// template of the recorded crawler miss above, built from the requested id, which is
    /// the whole address of a run.
    pub fn column_statistics_task_run_not_found(run_id: &str) -> String {
        format!("Column statistics task run with id {run_id} does not exist")
    }

    /// A column statistics task settings read (`GetColumnStatisticsTaskSettings`) for a
    /// table no create has written settings for. NOT recorded: what is pinned is the
    /// behaviour — 400 with the operation's modelled `EntityNotFoundException` — and the
    /// sentence names the table the request addressed, the way
    /// [`column_statistics_task_already_running`] names its table.
    pub fn column_statistics_task_settings_not_found(
        database_name: &str,
        table_name: &str,
    ) -> String {
        format!(
            "Column statistics task settings for table {database_name}.{table_name} do not \
             exist"
        )
    }

    /// `CreateColumnStatisticsTaskSettings` for a table whose settings a create already
    /// wrote. NOT recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the addressed table.
    pub const COLUMN_STATISTICS_TASK_SETTINGS_ALREADY_EXIST: &str =
        "Column statistics task settings already exist.";

    /// `StartColumnStatisticsTaskRun` for a table that already has a run in flight. NOT
    /// recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `ColumnStatisticsTaskRunningException`, "thrown when you try to start another job
    /// while running a column stats generation job" — and the sentence names the table the
    /// request addressed, the way the recorded per-resource templates name their resource.
    pub fn column_statistics_task_already_running(database_name: &str, table_name: &str) -> String {
        format!(
            "A column statistics generation job is already running for table \
             {database_name}.{table_name}"
        )
    }

    /// `CreateRegistry` for a `RegistryName` this account already holds. NOT recorded
    /// either: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`SCHEMA_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of the
    /// requested name.
    pub const REGISTRY_ALREADY_EXISTS: &str = "Registry already exists.";

    /// `CreateWorkflow` for a `Name` this account already holds a workflow under. NOT
    /// recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`REGISTRY_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the requested name.
    pub const WORKFLOW_ALREADY_EXISTS: &str = "Workflow already exists.";

    /// Any workflow operation whose `Name` names no workflow this account holds
    /// (`GetWorkflow`, and the same miss on every sibling that addresses one by name). NOT
    /// recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `EntityNotFoundException` — and the sentence follows [`database_not_found`]'s
    /// per-resource template, built from the requested name alone.
    pub fn workflow_not_found(name: &str) -> String {
        format!("Workflow {name} not found.")
    }

    /// `StartWorkflowRun` for a workflow no trigger can begin a run of. RECORDED
    /// template: the sentence is fixed and its one variable part is the workflow the
    /// request named, taken from the request's own `Name`. The start is refused with the
    /// operation's modelled `InvalidInputException` — a workflow with no entry point is
    /// not something the service can run.
    pub fn no_starting_trigger(name: &str) -> String {
        format!("Workflow {name} doesn't have any starting trigger.")
    }

    /// `StartWorkflowRun` for a workflow already running as many runs as it allows.
    /// DOCUMENTED template: `WorkflowRun$ErrorMessage` states this is the wording — "This
    /// error message describes any error that may have occurred in starting the workflow
    /// run. Currently the only error message is "Concurrent runs exceeded for workflow:
    /// `foo`."" — so the sentence is fixed and its one variable part is the workflow the
    /// request named. The start is refused with the operation's modelled
    /// `ConcurrentRunsExceededException`.
    pub fn concurrent_workflow_runs_exceeded(name: &str) -> String {
        format!("Concurrent runs exceeded for workflow: {name}.")
    }

    /// A workflow run lookup (`GetWorkflowRun`) for a `RunId` the named workflow holds no
    /// run under. NOT recorded: the sentence follows the per-resource template of
    /// [`column_statistics_task_run_not_found`], built from the ADDRESS the request sent —
    /// the workflow and the run id, which together name a workflow run.
    pub fn workflow_run_not_found(workflow_name: &str, run_id: &str) -> String {
        format!("Workflow run {run_id} not found for workflow {workflow_name}.")
    }

    /// A data quality rule recommendation run lookup
    /// (`GetDataQualityRuleRecommendationRun`) for an id no run in this account has. NOT
    /// recorded: the sentence follows the per-resource template of
    /// [`column_statistics_task_run_not_found`], built from the requested id, which is the
    /// whole address of a run.
    pub fn data_quality_rule_recommendation_run_not_found(run_id: &str) -> String {
        format!("Data quality rule recommendation run with id {run_id} does not exist")
    }

    /// `StartDataQualityRuleRecommendationRun` for a data source whose recommendation run
    /// has not finished. NOT recorded: what is pinned is the behaviour — 400 with the
    /// operation's modelled `ConflictException` — and the sentence names the table the
    /// request addressed, the way [`column_statistics_task_already_running`] names its
    /// table.
    pub fn data_quality_run_already_running(database_name: &str, table_name: &str) -> String {
        format!(
            "A data quality rule recommendation run is already in progress for table \
             {database_name}.{table_name}"
        )
    }

    /// `StartDataQualityRuleRecommendationRun` whose `ClientToken` an earlier start already
    /// used. RECORDED template: the sentence is fixed and carries the `ClientToken` the
    /// request itself sent. The start is refused with the operation's modelled
    /// `ConflictException` — a token stands for ONE start, so the service answers a second
    /// one carrying it as the duplicate it is rather than starting another run.
    pub fn idempotent_start_with_token(client_token: &str) -> String {
        format!("Idempotent start Data Quality run with token {client_token}")
    }

    /// A data quality run start whose `DataSource` carries neither kind of Glue table. NOT
    /// recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException` — and the sentence names the members the input has to carry
    /// one of, the way [`PARTITION_VALUES_REQUIRED`] names its member.
    pub const DATA_SOURCE_REQUIRES_A_TABLE: &str =
        "DataSource must specify either a GlueTable or a DataQualityGlueTable.";

    /// A partition create whose `PartitionInput` carries no `Values`. NOT recorded: what
    /// is pinned is the behaviour — `InvalidInputException`, as a per-item error for a
    /// batch create — and the sentence names the member the input has to carry, which is
    /// what the model requires of a valid input (`PartitionInput$Values`).
    pub const PARTITION_VALUES_REQUIRED: &str = "PartitionInput must specify Values.";

    /// A partition create whose value count is not the addressed table's partition-key
    /// count. NOT recorded: what is pinned is the behaviour — `InvalidInputException`,
    /// per item for a batch create — and the sentence carries the two counts the request
    /// and the table disagree on, so a client can see which it got wrong.
    pub fn partition_values_do_not_match_keys(keys: usize, values: usize) -> String {
        format!(
            "Partition values must name one value per partition key of the table; the table \
             has {keys} partition keys and the input carried {values} values"
        )
    }

    /// A partition create for a value list the addressed table already holds a partition
    /// for. NOT recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`TABLE_ALREADY_EXISTS`] — `AlreadyExistsException`, and no disclosure of the
    /// requested values.
    pub const PARTITION_ALREADY_EXISTS: &str = "Partition already exists.";

    /// A partition read (`GetPartition`) for a value list the addressed table holds no
    /// partition for. NOT recorded: the wording follows [`table_not_found`], the refusal
    /// for the resource one step up, and carries only the values the request addressed —
    /// which are the whole address of a partition within its table.
    pub fn partition_not_found(values: &[String]) -> String {
        format!("Partition [{}] not found.", values.join(", "))
    }

    /// `CreateSecurityConfiguration` for a name already taken. NOT recorded: inferred
    /// from the recorded `CreateDatabase` duplicate, like [`CRAWLER_ALREADY_EXISTS`] —
    /// 400, `AlreadyExistsException`, and no disclosure of the requested name.
    pub const SECURITY_CONFIGURATION_ALREADY_EXISTS: &str =
        "Security configuration already exists.";

    /// A resource-policy read or delete (`GetResourcePolicy`, `DeleteResourcePolicy`) for
    /// a resource that carries no policy. RECORDED constant: the refusal names neither
    /// the resource nor the ARN that was addressed.
    pub const POLICY_NOT_FOUND: &str = "Policy not found";

    /// `PutResourcePolicy` carrying `PolicyExistsCondition` `MUST_EXIST` for a resource
    /// that carries no policy yet ("A value of MUST_EXIST is used to update a policy"), and
    /// any resource-policy write or delete whose `PolicyHashCondition` is not the hash of
    /// the policy currently attached — the concurrent-modification check that member exists
    /// for ("its purpose is to prevent concurrent modifications of a policy"), which a
    /// resource carrying no policy at all fails too. RECORDED constant, and it names both
    /// cases: the policy is absent, or the hash is not the attached policy's. No stored
    /// value is disclosed.
    pub const POLICY_CONDITION_FAILED: &str = "Policy not exists or policy hash mismatch";

    /// `PutResourcePolicy` carrying `PolicyExistsCondition` `NOT_EXIST` against a resource
    /// that already carries one ("A value of NOT_EXIST is used to create a new policy").
    /// NOT recorded — the recorded condition failure above states the opposite case — so
    /// what is pinned is the behaviour, 400 with the operation's modelled
    /// `ConditionCheckFailureException`, and the sentence follows that wording and
    /// discloses nothing of the attached policy.
    pub const POLICY_ALREADY_EXISTS: &str = "Policy already exists";

    /// A resource-policy request whose `ResourceArn` this endpoint can hold no policy for.
    /// Derived from the request, like [`super::invalid_resource_arn_message`]: it names the
    /// member and the value that is not a Glue resource of this account and Region.
    pub fn invalid_policy_resource_arn(resource_arn: &str) -> String {
        format!(
            "ResourceArn must be the ARN of a Glue resource of this account and Region; got \
             {resource_arn:?}"
        )
    }

    /// `PutResourcePolicy` whose policy document is not a JSON object. NOT recorded: what
    /// is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException` — and the sentence names the member the document arrived in,
    /// which the model describes as "the policy document to set, in JSON format".
    pub const POLICY_MUST_BE_JSON: &str = "PolicyInJson must be a policy document in JSON format.";

    /// `PutResourcePolicy` for a resource this endpoint cannot take another policy for.
    /// Its own capacity, not an AWS quota, so the sentence says which ceiling was reached.
    pub fn too_many_resource_policies(ceiling: usize) -> String {
        format!(
            "this endpoint holds resource policies for {ceiling} resources, which is as many \
             as it can hold"
        )
    }

    /// `CreateUsageProfile` for a name this account already holds a profile under.
    /// RECORDED template: the sentence names the resource kind and carries the `Name` the
    /// request itself sent, which is the whole address of a usage profile. Unlike the
    /// catalog duplicates above, this one DOES echo the requested name, because that is
    /// the wording the corpus pins for this resource.
    pub fn usage_profile_already_exists(name: &str) -> String {
        format!("UsageProfile {name} already exists.")
    }

    /// A usage profile lookup (`GetUsageProfile`, and every other operation that addresses
    /// a profile by name) for a name no profile of this account has. RECORDED template,
    /// including its wording, built from the requested `Name`.
    pub fn usage_profile_not_found(name: &str) -> String {
        format!("UsageProfile {name} does not exists")
    }

    /// A usage profile write whose `ConfigurationObject` sets a `DefaultValue` above the
    /// `MaxValue` it sets for the same parameter. RECORDED template: the sentence carries
    /// the parameter key, the `ProfileConfiguration` member its map arrived under, and the
    /// `MaxValue` exactly as submitted.
    pub fn configuration_default_above_max(
        parameter: &str,
        section: &str,
        max_value: &str,
    ) -> String {
        format!(
            "default value for parameter {parameter} in {section} should be less than or \
             equal to max value {max_value}."
        )
    }

    /// The same refusal for the OTHER bound of the same parameter: a `DefaultValue` below
    /// its `MinValue`. NOT recorded on its own \u2014 what is pinned is the coherence rule and
    /// the sentence above \u2014 so this states the mirror requirement in that same wording,
    /// carrying the parameter, its section and the `MinValue` as submitted.
    pub fn configuration_default_below_min(
        parameter: &str,
        section: &str,
        min_value: &str,
    ) -> String {
        format!(
            "default value for parameter {parameter} in {section} should be greater than or \
             equal to min value {min_value}."
        )
    }

    /// The same refusal for the two bounds themselves: a `MinValue` above the `MaxValue`
    /// of the same parameter, which is a range no value lies in. NOT recorded on its own,
    /// so it follows the wording of [`configuration_default_above_max`] with the member
    /// that is too large named.
    pub fn configuration_min_above_max(parameter: &str, section: &str, max_value: &str) -> String {
        format!(
            "min value for parameter {parameter} in {section} should be less than or equal \
             to max value {max_value}."
        )
    }
}

/// Fold a database or table name the way the catalog stores it: ASCII-lowercased.
/// Applied on both write and lookup, so a name is reachable by any casing the client
/// sends. The fold is ASCII because Hive compatibility is an ASCII rule, and because
/// ASCII folding is length-stable: full Unicode lowercasing can change a name's length
/// and so its modelled `length` constraint.
pub fn fold_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Ceiling on the size of the matcher one name pattern compiles to, in bytes.
///
/// A pattern is request text and every stored name is matched against it, so the work it
/// can buy is bounded here rather than left to grow with a model's pattern length: the
/// compiled matcher runs in time linear in the name and never backtracks, and this
/// ceiling bounds how large a program a pattern may build — so this ceiling and the
/// per-database table ceiling together bound one request. It lives beside the matcher
/// because every listing that takes such a pattern needs the same bound; each refuses a
/// pattern past it with its OWN modelled client error.
pub const MAX_PATTERN_PROGRAM_BYTES: usize = 64 * 1024;

/// The matcher a name pattern selects names with, or the reason the value is no pattern
/// this catalog can match.
///
/// The pattern language is the one the Data Catalog's listings document: "A regular
/// expression pattern. If present, only those tables whose names match the pattern are
/// returned" (`GetTablesRequest$Expression`), in which `*` additionally stands for any run
/// of characters (including none) — the Hive-metastore substitution that makes `events*`
/// select `events` and `events_raw` while `ev_.*` selects `ev_one` and `ev_two`. The
/// pattern is anchored at both ends, because a listing selects the names that match it as
/// a WHOLE, and compared case-sensitively: a stored catalog name is folded to lowercase
/// ([`fold_name`]), the pattern is not, and a pattern in another case selects nothing.
///
/// A `*` that is ESCAPED (`\*`) or inside a character class (`[*]`) is the regular
/// expression's own star rather than the wildcard, so it is left as written: substituting
/// there would change `[*]` into a class of dot-or-star and `\*` into any run of dots,
/// i.e. a different pattern from the one the caller sent.
///
/// It lives here rather than in one operation's file because every listing of these
/// tables has to select them the same way — `GetTables`'s own `Expression`, and Athena's
/// `ListTableMetadata` over this same catalog — so there is ONE pattern language and two
/// listings of one set of tables can never disagree.
pub fn compile_name_pattern(pattern: &str) -> Result<::regex::Regex, ::regex::Error> {
    let mut translated = String::with_capacity(pattern.len() + 1);
    let mut escaped = false;
    let mut in_class = false;
    for character in pattern.chars() {
        let wildcard = character == '*' && !escaped && !in_class;
        if wildcard {
            translated.push_str(".*");
        } else {
            translated.push(character);
        }
        // The state for the NEXT character: a backslash escapes exactly the one after it,
        // and a class runs from an unescaped `[` to the next unescaped `]`.
        if escaped {
            escaped = false;
        } else {
            match character {
                '\\' => escaped = true,
                '[' => in_class = true,
                ']' => in_class = false,
                _ => {}
            }
        }
    }
    ::regex::RegexBuilder::new(&format!("^(?:{translated})$"))
        .size_limit(MAX_PATTERN_PROGRAM_BYTES)
        .build()
}

/// The IAM user name of the identity this emulator's long-term credentials speak for.
///
/// The emulator publishes exactly ONE long-term identity, and STS is what defines it:
/// `GetCallerIdentity` answers `arn:aws:iam::<account>:user/emulator` for every request
/// whose access key id names no session (`AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`).
const AMBIENT_USER_NAME: &str = "emulator";

/// The principal that made this request, as the ARN the catalog records in
/// `Table$CreatedBy` ("The person or entity who created the table").
///
/// Server-derived from the emulator's own identity contract, never from the request: it
/// is the ARN of the identity a request signed with this emulator's credentials speaks
/// for, which is the one [`AMBIENT_USER_NAME`] names — so Glue and STS agree on who the
/// caller is. An access key id is a credential id, not a user name, so it is never
/// rendered into an IAM ARN: that is a principal no AWS rule produces. The value is
/// therefore the same on every call and across restarts, exactly as the IAM identity
/// behind one set of credentials is.
pub fn request_principal() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{AMBIENT_USER_NAME}"))
}

/// The `CatalogId` a request addresses: the one it names, else this emulator's
/// account id, which is what the model documents as the default
/// ("If none is provided, the Amazon Web Services account ID is used by default").
pub fn resolve_catalog_id(catalog_id: Option<&String>) -> String {
    match catalog_id.map(String::as_str).filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => ::engine::emulator_account_id().to_string(),
    }
}

/// Whether `catalog_id` names a Data Catalog this emulator holds.
///
/// It holds exactly one: the one for its own account, which is the catalog the model
/// documents as the default. A catalog under any other id belongs to another account
/// (reachable on the real service only through a Data Catalog resource policy) or
/// would have to be created with `CreateCatalog`, which is not implemented — so no
/// other id can name a catalog here, and a request that names one is answered as a
/// miss rather than quietly given a new, empty catalog.
pub fn is_known_catalog(catalog_id: &str) -> bool {
    catalog_id == ::engine::emulator_account_id()
}

/// The resource portion of a Data Catalog's ARN: the catalog itself, which is the
/// resource a Data Catalog resource policy is attached to and the one an authorization
/// refusal names.
pub const CATALOG_ARN_RESOURCE: &str = "catalog";

/// The IAM action namespace every Glue action is spelled under, as an authorization
/// refusal names the action (`glue:GetTables`).
pub const IAM_ACTION_PREFIX: &str = "glue:";

/// The ARN of the Data Catalog `catalog_id` names, as addressed in `region`.
///
/// The canonical layout and partition come from the shared minter, so this ARN matches
/// every other ARN this emulator emits; only the ACCOUNT field is replaced, because a
/// request may address the catalog of another account and that account is what such an
/// ARN names.
pub fn catalog_arn(region: &str, catalog_id: &str) -> String {
    let canonical = ::engine::emulator_arn(ARN_NAMESPACE, region, CATALOG_ARN_RESOURCE);
    let mut fields: Vec<&str> = canonical.split(':').collect();
    // Field 5 of an ARN is the account; the canonical form always has it.
    if let Some(account) = fields.get_mut(4) {
        *account = catalog_id;
    }
    fields.join(":")
}

/// The refusal a request addressed to the Data Catalog `catalog_id` earns when that
/// catalog belongs to another account, or `None` when the request addresses the catalog
/// this emulator serves.
///
/// Authorization comes BEFORE the entity lookup: a `CatalogId` of another account is
/// reachable on the real service only through that catalog's own resource-based policy,
/// so a caller holding no grant on it is refused as unauthorized rather than told the
/// entity does not exist — which would also answer a cross-account read out of the
/// caller's OWN catalog whenever the two hold a same-named entity. This store holds one
/// catalog ([`is_known_catalog`]) and can hold no resource policy of a FOREIGN catalog —
/// `PutResourcePolicy` attaches a policy only to a resource of the account this endpoint
/// serves ([`resolve_policy_resource_arn`]) — so no grant on a foreign catalog can exist
/// here and every cross-account request for one is refused.
///
/// `action` is the operation's own name, which the refusal names as the IAM action
/// ([`IAM_ACTION_PREFIX`]) the caller may not perform.
pub fn foreign_catalog_refusal(catalog_id: &str, region: &str, action: &str) -> Option<String> {
    if is_known_catalog(catalog_id) {
        return None;
    }
    Some(messages::not_authorized_on_catalog(
        action,
        &catalog_arn(region, catalog_id),
    ))
}

/// Whether `account_id` — field 5 of an ARN — is the account this emulator serves, and
/// so the only account whose Glue resources this store can hold. An ARN naming any other
/// account names a resource of another account's estate, which this process does not
/// hold.
pub fn is_served_account(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] for a request that carries no scope (an unsigned call).
///
/// A Glue record carries no Region of its own — every operation that addresses a resource
/// names it by name — so the Region a resource's ARN is minted for, and compared against,
/// is the one its request is served in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request with no SigV4 credential scope is scoped to. Matches the Region
/// the emulator's own generated client config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace every Glue resource lives in, as the modelled
/// `GlueResourceArn` pattern spells it
/// (`^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:`).
pub const ARN_NAMESPACE: &str = "glue";

/// One kind of Glue resource that carries its OWN tag set, and so can be named by a
/// tagging call's `ResourceArn`.
///
/// The kinds are the ones whose create carries a `Tags` map and whose record in this
/// store therefore holds a tag set: a Data Catalog database (`CreateDatabase$Tags`), a
/// connection (`CreateConnection$Tags`), a crawler (`CreateCrawler$Tags`) and a job
/// definition (`CreateJob$Tags`). A table is NOT one of them — `CreateTable` models no
/// `Tags` member — and a Glue resource this emulator does not store yet has no tag set
/// to name either.
///
/// A schema-registry record holds a tag set too ([`SchemaEntry::tags`]), but is not a kind
/// here: its ARN's resource portion carries two path segments
/// (`schema/<registry>/<schema>`), which this one-segment table does not spell, so
/// resolving one through the tagging operations belongs to those operations' own issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// `connection/<name>`.
    Connection,
    /// `crawler/<name>`.
    Crawler,
    /// `database/<name>`.
    Database,
    /// `job/<name>`.
    Job,
}

/// The ARN resource-type segment each taggable kind carries: the catalog
/// [`parse_tagged_resource`] resolves an ARN against and [`TaggedResourceKind::arn`]
/// mints one from.
///
/// Source: the resource portion of each resource's documented Glue ARN
/// (`database/<name>`, `connection/<name>`, `crawler/<name>`, `job/<name>`). The model
/// states only the prefix every Glue ARN shares (`GlueResourceArn`'s pattern, see
/// [`ARN_NAMESPACE`]), so the segments are listed here once and every mint and every
/// lookup goes through this one table.
pub const TAGGED_RESOURCE_TYPES: [(&str, TaggedResourceKind); 4] = [
    ("connection", TaggedResourceKind::Connection),
    ("crawler", TaggedResourceKind::Crawler),
    ("database", TaggedResourceKind::Database),
    ("job", TaggedResourceKind::Job),
];

impl TaggedResourceKind {
    /// This kind's ARN resource-type segment, from [`TAGGED_RESOURCE_TYPES`].
    pub fn arn_segment(self) -> &'static str {
        TAGGED_RESOURCE_TYPES
            .iter()
            .find(|(_, kind)| *kind == self)
            .map(|(segment, _)| *segment)
            // Every variant has a row above, so this is unreachable; answering the
            // service namespace rather than panicking keeps a request path alive.
            .unwrap_or(ARN_NAMESPACE)
    }

    /// The ARN of the resource `name` of this kind, as served in `region`.
    ///
    /// The account segment is this emulator's own ([`::engine::emulator_arn`]), which is
    /// also the only `CatalogId` a catalog resource can live under here (see
    /// [`is_known_catalog`]), so a minted ARN and a stored record always agree on whose
    /// resource it is.
    pub fn arn(self, region: &str, name: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("{}/{name}", self.arn_segment()),
        )
    }
}

/// A Glue resource a tagging call named by ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    /// Which kind of resource the ARN's resource-type segment names.
    pub kind: TaggedResourceKind,
    /// Field 4 of the ARN: the Region whose estate the resource belongs to.
    pub region: String,
    /// Field 5 of the ARN: the account that owns the resource, which for a Data Catalog
    /// resource is also the `CatalogId` it lives under ("If none is provided, the Amazon
    /// Web Services account ID is used by default").
    pub account_id: String,
    /// The resource's name, folded exactly as the store keys it: lowercased for a
    /// database ([`fold_name`]), as written for the kinds the model documents no fold
    /// for.
    pub name: String,
}

/// The Glue resource `resource_arn` names, or `None` when it is not the ARN of a taggable
/// Glue resource at all — a value in another service's namespace, a malformed ARN, or a
/// Glue resource kind this store holds no tag set for.
///
/// Only the KIND and the identity fields are read here; whether this store holds the
/// resource is [`ComAmazonawsGlueAwsglueState::resource_tags`]'s answer, so an ARN of a
/// known kind that names nothing earns that kind's not-found refusal rather than being
/// called malformed.
pub fn parse_tagged_resource(resource_arn: &str) -> Option<TaggedResource> {
    let parsed = parse_glue_arn(resource_arn)?;
    let (segment, name) = parsed.resource.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    let (_, kind) = TAGGED_RESOURCE_TYPES
        .iter()
        .find(|(candidate, _)| *candidate == segment)?;
    Some(TaggedResource {
        kind: *kind,
        region: parsed.region.clone(),
        account_id: parsed.account_id.clone(),
        name: match kind {
            // The two Hive-compatible catalog resources are the ones the catalog folds;
            // a crawler, job or connection name is keyed as written.
            TaggedResourceKind::Database => fold_name(name),
            TaggedResourceKind::Connection
            | TaggedResourceKind::Crawler
            | TaggedResourceKind::Job => name.to_string(),
        },
    })
}

/// The message a refusal carries for a `ResourceArn` of a known kind that names no
/// resource this store holds.
///
/// Each sentence is the one this crate already answers wherever that kind fails to
/// resolve ([`messages`]), so "this ARN names no database" reads the same from a tagging
/// operation as from `GetDatabase`. The two kinds whose refusal wording no recording or
/// doc states take a sentence DERIVED from the request instead: it names the ARN that
/// resolved to nothing, and what is pinned is the behaviour — 400 with
/// `EntityNotFoundException`.
pub fn tagged_resource_not_found_message(resource: &TaggedResource, resource_arn: &str) -> String {
    match resource.kind {
        TaggedResourceKind::Database => messages::database_not_found(&resource.name),
        TaggedResourceKind::Crawler => messages::crawler_not_found(&resource.name),
        TaggedResourceKind::Connection | TaggedResourceKind::Job => {
            format!("Entity not found for resource {resource_arn}")
        }
    }
}

/// The message a refusal carries for a `ResourceArn` that is not the ARN of a taggable
/// Glue resource ([`parse_tagged_resource`] answers `None`).
///
/// Derived from the request, like this crate's other reference refusals: it names the
/// member, the resource types the member accepts ([`TAGGED_RESOURCE_TYPES`]) and the
/// value that is none of them.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    let kinds: Vec<&str> = TAGGED_RESOURCE_TYPES
        .iter()
        .map(|(segment, _)| *segment)
        .collect();
    format!(
        "ResourceArn must be the ARN of a taggable Glue resource naming one of {}; got \
         {resource_arn:?}",
        kinds.join(", ")
    )
}

/// A wall-clock instant as the modelled timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The `ColumnStatisticsState` a column statistics task run is recorded in by
/// `StartColumnStatisticsTaskRun`: the first of the modelled enum
/// (`STARTING | RUNNING | SUCCEEDED | FAILED | STOPPED`), because the run has been
/// accepted and nothing has begun generating statistics yet.
pub const COLUMN_STATISTICS_TASK_RUN_STATE_STARTING: &str = "STARTING";
/// See [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`].
pub const COLUMN_STATISTICS_TASK_RUN_STATE_FAILED: &str = "FAILED";

/// The `ColumnStatisticsState` values a column statistics task run never moves out of
/// again: of the modelled enum quoted on
/// [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`], the ones that name a finished run. The
/// other two (`STARTING`, `RUNNING`) each still owe a further state.
pub const TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES: [&str; 3] =
    ["SUCCEEDED", "FAILED", "STOPPED"];

/// How long a column statistics task run stays in
/// [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`] before the clock owes it its terminal
/// state.
///
/// CHOSEN, not documented: Glue publishes no duration for a column statistics task run,
/// so this is the emulator's own span — deliberately not zero, so a caller that reads
/// `GetColumnStatisticsTaskRun` straight after starting one still observes the run in
/// flight, and the refusal that goes with a run in flight. The due instant is stored on
/// the run and materialised by whichever later call reads it; nothing sleeps.
pub const COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// `ColumnStatisticsTaskRun$ErrorMessage` for the terminal state the clock owes every
/// column statistics task run.
///
/// This emulator is the Glue control plane: it records a task run faithfully, but it
/// reads none of the table's underlying data and so computes no statistics from it — the
/// run therefore ends in the modelled failure state carrying a reason that says exactly
/// that, rather than reporting statistics nothing computed.
pub const COLUMN_STATISTICS_TASK_RUN_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records column statistics task runs but reads none of the table's \
     underlying data, so the run ended without generating statistics.";

/// `ColumnStatisticsTaskRun$ComputationType` of a run `StartColumnStatisticsTaskRun`
/// started: of the modelled `ComputationType` (`FULL | INCREMENTAL`), the full
/// computation. The request carries no member that selects an incremental computation —
/// it names the table and the columns to "generate statistics" for — so a run started
/// this way computes the whole statistic set rather than an increment on an earlier one.
pub const COLUMN_STATISTICS_COMPUTATION_TYPE_FULL: &str = "FULL";

/// How many column statistics task runs this account keeps.
///
/// CHOSEN: Glue publishes no retention for these records, so this is the emulator's own
/// window, for the same reason as [`MAX_RUNS_PER_JOB`]: it bounds
/// [`ComAmazonawsGlueAwsglueState::column_statistics_task_runs`] and every walk over it.
/// The OLDEST record is dropped first, so the runs a caller is most likely to read are
/// kept.
pub const MAX_COLUMN_STATISTICS_TASK_RUNS: usize = 100;

/// How many column statistics task run ids this process has minted. A nonce source for
/// [`new_column_statistics_task_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job
/// run id.
static MINTED_COLUMN_STATISTICS_TASK_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `ColumnStatisticsTaskRunId`: the id the service assigns a column statistics
/// task run when it starts.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255`. The bits come from [`mint_hex_id`], as a job run id's do, so two runs
/// started in the same nanosecond cannot be given the same id. It is stored on the run,
/// so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_column_statistics_task_run_id() -> String {
    mint_hex_id(
        &MINTED_COLUMN_STATISTICS_TASK_RUN_IDS,
        COLUMN_STATISTICS_TASK_RUN_ID_WORDS,
    )
}

/// How many 64-bit words a column statistics task run id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters.
const COLUMN_STATISTICS_TASK_RUN_ID_WORDS: usize = 2;

/// One column statistics task run, as `StartColumnStatisticsTaskRun` recorded it.
///
/// A run carries the table it covers and the configuration the request named, plus the
/// members only a run has: its id, its status and the instants it moved between
/// statuses.
#[derive(Debug, Clone)]
pub struct ColumnStatisticsTaskRunEntry {
    /// `ColumnStatisticsTaskRun$ColumnStatisticsTaskRunId`, minted by
    /// [`new_column_statistics_task_run_id`].
    pub id: String,
    /// The catalog the covered table lives in, as [`resolve_catalog_id`] resolved it.
    pub catalog_id: String,
    /// The covered database's name, folded the way the catalog stores it.
    pub database_name: String,
    /// The covered table's name, folded the way the catalog stores it.
    pub table_name: String,
    /// The columns statistics are generated for: the ones the request listed, else every
    /// column the stored table has ("If none is supplied, all column names for the table
    /// will be used by default").
    pub column_name_list: Vec<String>,
    /// `ColumnStatisticsTaskRun$Role`: "The IAM role that the service assumes to generate
    /// statistics."
    pub role: String,
    /// `ColumnStatisticsTaskRun$SampleSize`: "The percentage of rows used to generate
    /// statistics", the request's value, whose own default of 0 means the entire table.
    pub sample_size: f64,
    pub security_configuration: Option<String>,
    /// One of the modelled `ColumnStatisticsState` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// `CreationTime`: "The time that this task was created."
    pub creation_time: SystemTime,
    /// `StartTime`: "The start time of the task."
    pub start_time: SystemTime,
    /// `LastUpdated`: "The last point in time when this task was modified."
    pub last_updated: SystemTime,
    /// `EndTime`: "The end time of the task", absent until the run has finished.
    pub end_time: Option<SystemTime>,
    /// `ErrorMessage`: "The error message for the job", absent until one has failed.
    pub error_message: Option<String>,
}

impl ColumnStatisticsTaskRunEntry {
    /// Record a run of the stored table as the `StartColumnStatisticsTaskRun` request asks
    /// for it, in [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`] with its next status due
    /// [`COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER`] out.
    ///
    /// `table` is the stored table the request resolved to, which is where the default
    /// column list comes from, and `catalog_id` the catalog it was resolved in.
    pub fn start(
        input: &StartColumnStatisticsTaskRunInput,
        table: &TableEntry,
        catalog_id: &str,
        id: String,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            catalog_id: catalog_id.to_string(),
            database_name: table.database_name.clone(),
            table_name: table.name.clone(),
            // "If none is supplied, all column names for the table will be used by
            // default": an EMPTY list supplies no column name either, so it takes the
            // same default rather than recording a run that covers nothing.
            column_name_list: input
                .column_name_list
                .clone()
                .filter(|names| !names.is_empty())
                .unwrap_or_else(|| table_column_names(table)),
            role: input.role.clone(),
            sample_size: input.sample_size,
            security_configuration: input.security_configuration.clone(),
            status: COLUMN_STATISTICS_TASK_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                now.checked_add(COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            creation_time: now,
            start_time: now,
            last_updated: now,
            end_time: None,
            // Nothing has failed yet: a run carries an error message only once it has.
            error_message: None,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES.contains(&self.status.as_str())
    }

    /// Move this run into the status the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`] at
    /// [`Self::settles_at`] whether or not anybody called, and the instants recorded are
    /// that DUE instant, not the instant somebody read it — so two reads of a settled run
    /// report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = COLUMN_STATISTICS_TASK_RUN_STATE_FAILED.to_string();
        self.error_message = Some(COLUMN_STATISTICS_TASK_RUN_NOT_EXECUTED_MESSAGE.to_string());
        self.end_time = Some(due);
        self.last_updated = due;
    }

    /// This run as the `ExecutionAttempt` a settings read reports: "a run attempt for a
    /// column statistics task run."
    ///
    /// Every member is this run's own — its id, the status it stands in, the instant it
    /// started and the reason it failed, if it did.
    pub fn to_execution_attempt(&self) -> ExecutionAttempt {
        ExecutionAttempt {
            status: Some(
                if self.status == COLUMN_STATISTICS_TASK_RUN_STATE_FAILED {
                    COLUMN_STATISTICS_EXECUTION_STATUS_FAILED.to_string()
                } else {
                    COLUMN_STATISTICS_EXECUTION_STATUS_STARTED.to_string()
                },
            ),
            column_statistics_task_run_id: Some(self.id.clone()),
            // "A timestamp when the last column statistics task run occurred": the
            // instant the attempt started.
            execution_timestamp: Some(to_date_time(self.start_time)),
            error_message: self.error_message.clone(),
        }
    }

    /// Rebuild the modelled `ColumnStatisticsTaskRun` output shape from what is stored.
    pub fn to_column_statistics_task_run(&self) -> ColumnStatisticsTaskRun {
        ColumnStatisticsTaskRun {
            // "The Amazon Web Services account ID" the run belongs to.
            customer_id: Some(::engine::emulator_account_id().to_string()),
            column_statistics_task_run_id: Some(self.id.clone()),
            database_name: Some(self.database_name.clone()),
            table_name: Some(self.table_name.clone()),
            column_name_list: Some(self.column_name_list.clone()),
            catalog_id: Some(self.catalog_id.clone()),
            role: Some(self.role.clone()),
            sample_size: self.sample_size,
            security_configuration: self.security_configuration.clone(),
            // "The number of workers used to generate column statistics" and "the
            // calculated DPU usage in seconds for all autoscaled workers": no worker ran,
            // so neither figure has anything to report and each stays at the model's own
            // default, and no `WorkerType` was used either.
            number_of_workers: 0,
            worker_type: None,
            dpu_seconds: 0.0,
            computation_type: Some(COLUMN_STATISTICS_COMPUTATION_TYPE_FULL.to_string()),
            status: Some(self.status.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated: Some(to_date_time(self.last_updated)),
            start_time: Some(to_date_time(self.start_time)),
            end_time: self.end_time.map(to_date_time),
            error_message: self.error_message.clone(),
        }
    }
}

/// Every column name the stored table has, in the order the table declares them: the
/// default column list of a column statistics task run ("If none is supplied, all column
/// names for the table will be used by default").
///
/// The names come from the table's `StorageDescriptor$Columns`, which is where "a list of
/// the `Columns` in the table" lives; a table stored without a storage descriptor
/// declares no columns and so yields none.
///
/// `PartitionKeys` are deliberately NOT part of the list. They are "a list of columns by
/// which the table is partitioned", and the statistics of such a column are addressed per
/// partition — `UpdateColumnStatisticsForPartition` and
/// `GetColumnStatisticsForPartition` take `PartitionValues` — so they are not what a
/// table-level statistics run covers.
pub fn table_column_names(table: &TableEntry) -> Vec<String> {
    table
        .storage_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.columns.as_ref())
        .map(|columns| {
            columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}

/// `ColumnStatisticsTaskSettings$ScheduleType` of settings created with a `Schedule`: of
/// the modelled `ScheduleType` (`CRON | AUTO`), the cron one, because the schedule those
/// settings carry IS a cron expression ("A schedule for running the column statistics,
/// specified in CRON syntax" — `CreateColumnStatisticsTaskSettingsRequest$Schedule`).
/// Settings created without a schedule carry no schedule type at all: nothing stated one.
pub const COLUMN_STATISTICS_SCHEDULE_TYPE_CRON: &str = "CRON";

/// `ColumnStatisticsTaskSettings$SettingSource` of settings a create wrote for one table:
/// of the modelled `SettingSource` (`CATALOG | TABLE`), the table one, because the create
/// addresses a single table (`DatabaseName` plus `TableName`, both `@required`) rather
/// than the catalog those settings would otherwise be inherited from.
pub const COLUMN_STATISTICS_SETTING_SOURCE_TABLE: &str = "TABLE";

/// `ExecutionAttempt$Status` of an attempt that ended in a column statistics task run's
/// failure state: of the modelled `ExecutionStatus` (`FAILED | STARTED`), the failed one.
pub const COLUMN_STATISTICS_EXECUTION_STATUS_FAILED: &str = "FAILED";

/// `ExecutionAttempt$Status` of any other attempt. The modelled `ExecutionStatus` carries
/// only the two values quoted on [`COLUMN_STATISTICS_EXECUTION_STATUS_FAILED`], so an
/// attempt that has not failed is reported as the run it started.
pub const COLUMN_STATISTICS_EXECUTION_STATUS_STARTED: &str = "STARTED";

/// The column statistics task settings of ONE table, as
/// `CreateColumnStatisticsTaskSettings` recorded them.
///
/// Only what the client STATED is held here. The members that address the settings —
/// their catalog, database and table — are the stored table's own and are read off it
/// when a read projects the modelled shape ([`Self::to_column_statistics_task_settings`]),
/// so one record cannot disagree with the table it hangs on.
#[derive(Debug, Clone)]
pub struct ColumnStatisticsTaskSettingsEntry {
    /// `ColumnStatisticsTaskSettings$Role`: "The role used for running the column
    /// statistics."
    pub role: String,
    /// `ColumnStatisticsTaskSettings$SampleSize`: "The percentage of data to sample", the
    /// request's value, whose own default is 0.
    pub sample_size: f64,
    /// The cron expression of `Schedule`, or `None` when the create stated no schedule.
    pub schedule_expression: Option<String>,
    /// `ColumnNameList`: "A list of column names for which to run statistics", or `None`
    /// when the create listed none — the settings record what was configured, and the
    /// default of a run that names no column is resolved by the run
    /// ([`table_column_names`]).
    pub column_name_list: Option<Vec<String>>,
    /// `SecurityConfiguration`: "Name of the security configuration that is used to
    /// encrypt CloudWatch logs."
    pub security_configuration: Option<String>,
    /// `CreateColumnStatisticsTaskSettingsRequest$Tags`: "A map of tags." Stored rather
    /// than served, for the reason [`CustomEntityTypeEntry::tags`] gives: no `ResourceArn`
    /// this service tags ([`ComAmazonawsGlueAwsglueState::resource_tags`]) addresses a
    /// settings record, so nothing reads them back yet — and dropping them would lose what
    /// the create was told.
    pub tags: HashMap<String, String>,
}

impl ColumnStatisticsTaskSettingsEntry {
    /// Record the settings a `CreateColumnStatisticsTaskSettings` request states.
    pub fn from_create_input(input: &CreateColumnStatisticsTaskSettingsInput) -> Self {
        Self {
            role: input.role.clone(),
            sample_size: input.sample_size,
            schedule_expression: input.schedule.clone(),
            // An empty list states no column either, so it is recorded as none rather
            // than as settings that cover nothing.
            column_name_list: input
                .column_name_list
                .clone()
                .filter(|names| !names.is_empty()),
            security_configuration: input.security_configuration.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `ColumnStatisticsTaskSettings` shape from what is stored.
    ///
    /// `table` is the stored table these settings hang on, which is where their catalog,
    /// database and table names come from, and `last_attempt` the last column statistics
    /// task run started for that table, if one has been: "the last `ExecutionAttempt` for
    /// the column statistics task run" is that run, so it is read off the stored runs
    /// rather than held here.
    pub fn to_column_statistics_task_settings(
        &self,
        table: &TableEntry,
        last_attempt: Option<&ColumnStatisticsTaskRunEntry>,
    ) -> ColumnStatisticsTaskSettings {
        ColumnStatisticsTaskSettings {
            database_name: Some(table.database_name.clone()),
            table_name: Some(table.name.clone()),
            catalog_id: Some(table.catalog_id.clone()),
            // A stated schedule is a schedule the service runs, so it is reported in the
            // scheduled state, exactly as a crawler's is ([`CrawlerEntry::to_crawler`]).
            schedule: self
                .schedule_expression
                .as_ref()
                .map(|expression| Schedule {
                    schedule_expression: Some(expression.clone()),
                    state: Some(SCHEDULE_STATE_SCHEDULED.to_string()),
                }),
            schedule_type: self
                .schedule_expression
                .as_ref()
                .map(|_| COLUMN_STATISTICS_SCHEDULE_TYPE_CRON.to_string()),
            setting_source: Some(COLUMN_STATISTICS_SETTING_SOURCE_TABLE.to_string()),
            column_name_list: self.column_name_list.clone(),
            role: Some(self.role.clone()),
            sample_size: self.sample_size,
            security_configuration: self.security_configuration.clone(),
            last_execution_attempt: last_attempt
                .map(ColumnStatisticsTaskRunEntry::to_execution_attempt),
        }
    }
}

/// A stored workflow: the container for the dependent jobs and crawlers
/// `CreateWorkflow` registers, and the resource an EVENT trigger attaches to.
///
/// Every member of the modelled `Workflow` shape a create can set is held here, plus the
/// create's `Tags`, which that shape does not carry. The server-set members are the
/// instants the record was written at and the runs started of it; the member only a
/// BLUEPRINT produces (`BlueprintDetails`) belongs to the operations that produce it, so a
/// workflow a create wrote carries none.
#[derive(Debug, Clone)]
pub struct WorkflowEntry {
    /// The workflow's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::workflows`].
    pub name: String,
    /// "A description of the workflow", stored exactly as sent, and absent for a create
    /// that named none.
    pub description: Option<String>,
    /// "A collection of properties to be used as part of each execution of the workflow."
    /// A create that named none stores NO properties rather than an empty map, so a read
    /// reports the absence the create expressed.
    pub default_run_properties: Option<HashMap<String, String>>,
    /// The ceiling on this workflow's concurrent runs. "If you leave this parameter
    /// blank, there is no limit to the number of concurrent workflow runs", so an absent
    /// member is stored absent rather than as a number.
    pub max_concurrent_runs: Option<i32>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this workflow.
    pub last_modified_on: SystemTime,
    /// The create's `Tags`, which are not part of the `Workflow` shape.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`]
    /// gives: [`TaggedResourceKind`] carries no workflow variant yet, and resolving one
    /// belongs to the tagging operations' own issues.
    pub tags: HashMap<String, String>,
    /// The runs `StartWorkflowRun` started of this workflow, oldest first and bounded by
    /// [`MAX_RUNS_PER_WORKFLOW`].
    ///
    /// A run lives on its workflow because every operation that addresses one names the
    /// workflow too: `GetWorkflowRun` takes a `Name` beside the `RunId`, exactly as
    /// `GetJobRun` takes a `JobName` beside its run id.
    pub runs: Vec<WorkflowRunEntry>,
}

impl WorkflowEntry {
    /// Build a stored workflow from a `CreateWorkflow` request plus the one thing the
    /// SERVICE fills: the instant the record was written at, which is both its create
    /// and its last-modified instant until a write moves the latter.
    pub fn from_create_input(input: &CreateWorkflowInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            description: input.description.clone(),
            default_run_properties: input.default_run_properties.clone(),
            max_concurrent_runs: input.max_concurrent_runs,
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
            // A workflow a create wrote has not been run.
            runs: Vec::new(),
        }
    }

    /// The maximum number of this workflow's runs that may be in flight at once, or `None`
    /// when the workflow names no ceiling: "If you leave this parameter blank, there is no
    /// limit to the number of concurrent workflow runs"
    /// (`CreateWorkflowRequest$MaxConcurrentRuns`).
    ///
    /// A named ceiling is held at one ([`DEFAULT_MAX_CONCURRENT_RUNS`]), so a stored zero
    /// cannot make the workflow unrunnable — the same floor a job's concurrency gets.
    pub fn concurrent_run_ceiling(&self) -> Option<i32> {
        self.max_concurrent_runs
            .map(|ceiling| ceiling.max(DEFAULT_MAX_CONCURRENT_RUNS))
    }

    /// Give every run of this workflow the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs, exactly as
    /// [`JobEntry::settle_due_runs`] is for a job run: a run's next status was due at
    /// [`WorkflowRunEntry::settles_at`] whether or not anybody called, so materialising it
    /// on read reports the status the clock had already reached.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// The stored run with this id, or `None` when this workflow has no such run.
    pub fn run(&self, run_id: &str) -> Option<&WorkflowRunEntry> {
        self.runs.iter().find(|run| run.id == run_id)
    }

    /// [`Self::run`] mutably: where every write to one of this workflow's runs lands.
    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut WorkflowRunEntry> {
        self.runs.iter_mut().find(|run| run.id == run_id)
    }

    /// How many of this workflow's runs have not finished.
    ///
    /// Call [`Self::settle_due_runs`] first, or a run the clock has already finished still
    /// counts as in flight.
    pub fn runs_in_flight(&self) -> usize {
        self.runs.iter().filter(|run| !run.is_terminal()).count()
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_WORKFLOW`] of them.
    pub fn record_run(&mut self, run: WorkflowRunEntry) {
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_WORKFLOW {
            let overflow = self.runs.len() - MAX_RUNS_PER_WORKFLOW;
            self.runs.drain(..overflow);
        }
    }

    /// Rebuild the modelled `Workflow` shape from what is stored, so a listing reports
    /// the same workflow a single read does.
    ///
    /// `graph` is the graph the reader asked for with its own `IncludeGraph` ("Specifies
    /// whether to include a graph when returning the workflow resource metadata") —
    /// [`ComAmazonawsGlueAwsglueState::workflow_graph`] for this workflow — and `None` when
    /// it asked for none, because an unasked-for graph is reported as no graph at all.
    pub fn to_workflow(&self, graph: Option<WorkflowGraph>) -> Workflow {
        Workflow {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            default_run_properties: self.default_run_properties.clone(),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            // "The information about the last execution of the workflow": the most
            // recently started run this workflow holds, and nothing at all for a workflow
            // no run has been started of. The nested run carries no graph of its own —
            // `IncludeGraph` asks for the WORKFLOW's graph, which is reported beside it.
            last_run: self.runs.last().map(|run| run.to_workflow_run(None)),
            graph,
            max_concurrent_runs: self.max_concurrent_runs,
            // "The details of the blueprint that this particular workflow is created
            // from": a workflow `CreateWorkflow` registered was created from none.
            blueprint_details: None,
        }
    }
}

/// The `WorkflowRunStatus` a run is in the instant `StartWorkflowRun` returns. One of the
/// modelled `WorkflowRunStatus` values (`RUNNING`, `COMPLETED`, `STOPPING`, `STOPPED`,
/// `ERROR`).
pub const WORKFLOW_RUN_STATUS_RUNNING: &str = "RUNNING";

/// The `WorkflowRunStatus` of a run that finished having errored. See
/// [`WORKFLOW_RUN_STATUS_RUNNING`].
pub const WORKFLOW_RUN_STATUS_ERROR: &str = "ERROR";

/// The `WorkflowRunStatus` values a run never moves out of again: of the modelled enum
/// quoted on [`WORKFLOW_RUN_STATUS_RUNNING`], the ones that name a finished run. The other
/// two (`RUNNING`, `STOPPING`) each still owe a further status.
pub const TERMINAL_WORKFLOW_RUN_STATUSES: [&str; 3] =
    ["COMPLETED", "STOPPED", WORKFLOW_RUN_STATUS_ERROR];

/// How long a workflow run stays in the status the call that started it put it in before
/// the clock owes it the next one.
///
/// CHOSEN, not documented: Glue publishes no duration for a workflow run, so this is the
/// emulator's own span — deliberately not zero, so a caller that polls `GetWorkflowRun`
/// straight after `StartWorkflowRun` still observes the run in flight, and the concurrency
/// ceiling that goes with it. The due instant is stored on the run and materialised by
/// whichever later call reads it; nothing sleeps.
pub const WORKFLOW_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// `WorkflowRun$ErrorMessage` for the terminal status the clock owes every run.
///
/// This emulator is the Glue control plane: it records a workflow run faithfully, but
/// nothing here runs the jobs and crawlers the workflow's triggers initiate, so a run can
/// never reach `COMPLETED` — it comes to rest in the modelled error status carrying a
/// reason that says exactly that, rather than reporting a result nothing computed.
pub const WORKFLOW_RUN_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records workflow runs but runs no job or crawler, so the workflow run \
     ended without any action being executed.";

/// The prefix a Glue workflow run id carries, which is what makes an `IdString`
/// recognisable as a workflow run id rather than any other id the service mints.
pub const WORKFLOW_RUN_ID_PREFIX: &str = "wr_";

/// How many runs one workflow keeps.
///
/// CHOSEN: Glue publishes no retention for workflow-run records, so this is the emulator's
/// own window, for the reason [`MAX_RUNS_PER_JOB`] gives. The OLDEST record is dropped
/// first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_WORKFLOW: usize = 100;

/// Ceiling on the run properties ONE workflow run may hold.
///
/// This emulator's own capacity, NOT an AWS quota, for the reason
/// [`MAX_DATABASES_PER_CATALOG`] gives: `PutWorkflowRunProperties` adds to the map a run
/// holds and is reachable by any caller, so what one process will hold for a run is bounded
/// here. A put that would take the run past the ceiling is refused with that operation's
/// modelled `ResourceNumberLimitExceededException` rather than written.
pub const MAX_RUN_PROPERTIES_PER_RUN: usize = 100;

/// How many workflow run ids this process has minted. A nonce source for
/// [`new_workflow_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job run id.
static MINTED_WORKFLOW_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words a Glue workflow run id carries: four, i.e. 256 bits rendered as
/// 64 lowercase hexadecimal characters.
const WORKFLOW_RUN_ID_WORDS: usize = 4;

/// A fresh `RunId`: the id the service assigns a workflow run when it starts.
///
/// [`WORKFLOW_RUN_ID_PREFIX`] followed by 64 lowercase hexadecimal characters, which is
/// inside `IdString`'s `length 1..255`. The bits come from [`mint_hex_id`], as a job run
/// id's do, so two runs started in the same nanosecond cannot be given the same id. It is
/// stored on the run, so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_workflow_run_id() -> String {
    format!(
        "{WORKFLOW_RUN_ID_PREFIX}{}",
        mint_hex_id(&MINTED_WORKFLOW_RUN_IDS, WORKFLOW_RUN_ID_WORDS)
    )
}

/// One run of a workflow, as `StartWorkflowRun` recorded it: "A workflow run is an
/// execution of a workflow providing all the runtime information" (`WorkflowRun`).
#[derive(Debug, Clone)]
pub struct WorkflowRunEntry {
    /// `WorkflowRun$WorkflowRunId`, minted by [`new_workflow_run_id`].
    pub id: String,
    /// `WorkflowRun$Name`: "Name of the workflow that was run."
    pub workflow_name: String,
    /// `WorkflowRun$WorkflowRunProperties`: "The workflow run properties which were set
    /// during the run" — the workflow's own `DefaultRunProperties` ("to be used as part of
    /// each execution of the workflow") with the start's `RunProperties` applied over them.
    pub run_properties: HashMap<String, String>,
    /// Server-set: the instant `StartWorkflowRun` recorded this run.
    pub started_on: SystemTime,
    /// Server-set once the run reaches a terminal status, and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `WorkflowRunStatus` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The `WorkflowRunStatus` the clock owes this run at [`Self::settles_at`].
    pub settles_to: String,
    /// `WorkflowRun$ErrorMessage`, which a run carries only once something has gone wrong
    /// with it.
    pub error_message: Option<String>,
    /// `WorkflowRunStatistics$TotalActions`: "Total number of Actions in the workflow run",
    /// which are the actions the trigger that began the run initiates.
    pub total_actions: i32,
}

impl WorkflowRunEntry {
    /// Record a run of `workflow` as the `StartWorkflowRun` request asks for it, in
    /// [`WORKFLOW_RUN_STATUS_RUNNING`] with its next status due
    /// [`WORKFLOW_RUN_SETTLES_AFTER`] out.
    ///
    /// `total_actions` is how many actions the trigger that began this run initiates, which
    /// is the run's total action count.
    pub fn start(
        workflow: &WorkflowEntry,
        input: &StartWorkflowRunInput,
        id: String,
        now: SystemTime,
        total_actions: i32,
    ) -> Self {
        // "A collection of properties to be used as part of each execution of the
        // workflow" is the base every run starts from, and "the workflow run properties
        // for the new workflow run" (`StartWorkflowRunRequest$RunProperties`) are this
        // run's own, so they win where both name a key.
        let mut run_properties = workflow.default_run_properties.clone().unwrap_or_default();
        if let Some(requested) = input.run_properties.as_ref() {
            run_properties.extend(
                requested
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }

        Self {
            id,
            workflow_name: workflow.name.clone(),
            run_properties,
            started_on: now,
            completed_on: None,
            status: WORKFLOW_RUN_STATUS_RUNNING.to_string(),
            settles_at: Some(
                now.checked_add(WORKFLOW_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            // Nothing here runs the actions the workflow's triggers initiate, so the
            // status the clock owes a run that nobody stops is the modelled error status.
            settles_to: WORKFLOW_RUN_STATUS_ERROR.to_string(),
            // Nothing has gone wrong yet: a run carries an error message only once it has.
            error_message: None,
            total_actions,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_WORKFLOW_RUN_STATUSES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_WORKFLOW_RUN_STATUSES.contains(&self.status.as_str())
    }

    /// How many properties this run would hold once `incoming` had been written over what
    /// it holds: the keys it already has, plus the ones the put brings that are new to it.
    pub fn properties_after(&self, incoming: &HashMap<String, String>) -> usize {
        self.run_properties.len()
            + incoming
                .keys()
                .filter(|property| !self.run_properties.contains_key(*property))
                .count()
    }

    /// Move this run into the status the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave the status a call put it in at [`Self::settles_at`] whether
    /// or not anybody called, and the instants recorded are that DUE instant, not the
    /// instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = self.settles_to.clone();
        // "This error message describes any error that may have occurred in starting the
        // workflow run": the run that comes to rest in the error status carries the reason
        // nothing executed, while one a stop brought to rest errored at nothing.
        if self.status == WORKFLOW_RUN_STATUS_ERROR {
            self.error_message = Some(WORKFLOW_RUN_NOT_EXECUTED_MESSAGE.to_string());
        }
        if self.is_terminal() {
            self.completed_on = Some(due);
        }
    }

    /// Rebuild the modelled `WorkflowRun` shape from what is stored.
    ///
    /// `graph` is the graph the reader asked for with its own `IncludeGraph` ("Specifies
    /// whether to include the workflow graph in response or not"), handled as a workflow
    /// read handles it ([`WorkflowEntry::to_workflow`]).
    pub fn to_workflow_run(&self, graph: Option<WorkflowGraph>) -> WorkflowRun {
        WorkflowRun {
            name: Some(self.workflow_name.clone()),
            workflow_run_id: Some(self.id.clone()),
            // "The ID of the previous workflow run": a run a start began follows no run,
            // so there is none to report.
            previous_run_id: None,
            workflow_run_properties: Some(self.run_properties.clone()),
            started_on: Some(to_date_time(self.started_on)),
            completed_on: self.completed_on.map(to_date_time),
            status: Some(self.status.clone()),
            error_message: self.error_message.clone(),
            statistics: Some(self.statistics()),
            graph,
            // "The batch condition that started the workflow run": only an EVENT trigger's
            // batch condition starts a run, and a start a caller made carries none.
            starting_event_batch_condition: None,
        }
    }

    /// `WorkflowRun$Statistics`: "Workflow run statistics provides statistics about the
    /// workflow run."
    ///
    /// Every count but the total is the model's own default of 0: nothing here executes an
    /// action, so no action of this run has succeeded, failed, timed out, stopped, errored,
    /// or is running or waiting. The total is the number of actions the trigger that began
    /// the run initiates.
    fn statistics(&self) -> WorkflowRunStatistics {
        WorkflowRunStatistics {
            total_actions: self.total_actions,
            ..WorkflowRunStatistics::default()
        }
    }
}

/// A workflow's graph under construction: the nodes and the directed connections
/// [`ComAmazonawsGlueAwsglueState::workflow_graph`] has placed so far.
///
/// Each component gets ONE node and each connection ONE edge however many of a workflow's
/// triggers name it, which is what the two sets of placed ids are for; the lists keep the
/// order the walk placed them in, so the graph is the same on every read of an unchanged
/// store.
#[derive(Debug, Default)]
struct WorkflowGraphBuilder {
    /// "A list of the the Glue components belong to the workflow represented as nodes."
    nodes: Vec<Node>,
    /// "A list of all the directed connections between the nodes belonging to the
    /// workflow."
    edges: Vec<Edge>,
    /// The `UniqueId` of every node already placed.
    placed_nodes: HashSet<String>,
    /// The `(SourceId, DestinationId)` of every edge already placed.
    placed_edges: HashSet<(String, String)>,
}

impl WorkflowGraphBuilder {
    /// Place the node for the component `name` of type `node_type` unless it is already
    /// placed, and answer its `UniqueId` either way.
    ///
    /// `trigger` is the stored trigger a TRIGGER node stands for, and `None` for the job and
    /// crawler nodes: "details of the Trigger when the node represents a Trigger"
    /// (`Node$TriggerDetails`).
    fn node(&mut self, node_type: &str, name: &str, trigger: Option<&TriggerEntry>) -> String {
        let unique_id = workflow_node_id(node_type, name);
        if self.placed_nodes.insert(unique_id.clone()) {
            self.nodes.push(Node {
                r#type: Some(node_type.to_string()),
                name: Some(name.to_string()),
                unique_id: Some(unique_id.clone()),
                trigger_details: trigger.map(|trigger| TriggerNodeDetails {
                    trigger: Some(trigger.to_trigger()),
                }),
                // "The information for the job runs represented by the job node"
                // (`JobNodeDetails$JobRuns`) and "a list of crawls represented by the crawl
                // node" (`CrawlerNodeDetails$Crawls`): nothing here runs the actions a
                // workflow's triggers initiate, so no run of that job and no crawl of that
                // crawler belongs to this workflow to report.
                job_details: None,
                crawler_details: None,
            });
        }
        unique_id
    }

    /// Place the connection from the node `source_id` to the node `destination_id` unless
    /// it is already placed.
    fn edge(&mut self, source_id: &str, destination_id: &str) {
        if self
            .placed_edges
            .insert((source_id.to_string(), destination_id.to_string()))
        {
            self.edges.push(Edge {
                source_id: Some(source_id.to_string()),
                destination_id: Some(destination_id.to_string()),
            });
        }
    }

    /// The modelled graph the walk built. A workflow nothing is attached to yields the
    /// graph it is — present, with no components and no connections — rather than no graph
    /// at all, which is what an unasked-for graph is.
    fn build(self) -> WorkflowGraph {
        WorkflowGraph {
            nodes: Some(self.nodes),
            edges: Some(self.edges),
        }
    }
}

/// The components one `Action` or `Condition` of a trigger names, as the node type and name
/// of each: both shapes carry a `JobName` and a `CrawlerName`, and which of them is set
/// says whether the component is a job or a crawler.
fn named_components<'a>(
    job_name: Option<&'a str>,
    crawler_name: Option<&'a str>,
) -> impl Iterator<Item = (&'static str, &'a str)> {
    job_name
        .map(|name| (NODE_TYPE_JOB, name))
        .into_iter()
        .chain(crawler_name.map(|name| (NODE_TYPE_CRAWLER, name)))
}

/// `Node$UniqueId`: "the unique Id assigned to the node within the workflow."
///
/// Derived from what identifies the component inside the workflow — its type and its name
/// — so every read of the graph reports the same id for it, and a job and a trigger of the
/// same name are still two nodes. `Node$UniqueId` is itself a `NameString`, so a component
/// named at that bound leaves no room for the type: such a name keeps as much of itself as
/// there IS room for, followed by a digest of the whole name, so two components whose names
/// agree only on the part that fits still get two ids.
fn workflow_node_id(node_type: &str, name: &str) -> String {
    let prefix = format!("{}_", node_type.to_ascii_lowercase());
    let room = MAX_NAME_STRING_CHARS.saturating_sub(prefix.chars().count());
    if name.chars().count() <= room {
        return format!("{prefix}{name}");
    }
    let digest = format!("_{:016x}", name_digest(name));
    let held: String = name
        .chars()
        .take(room.saturating_sub(digest.chars().count()))
        .collect();
    format!("{prefix}{held}{digest}")
}

/// A 64-bit digest of `name`, which is what keeps two long component names apart in
/// [`workflow_node_id`]. The standard library's own hasher, so nothing here hand-rolls one;
/// it answers the same value for the same name throughout a process, which is what a stable
/// id needs. NOT a cryptographic hash, and nothing here uses it as one.
fn name_digest(name: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGlueAwsglueState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGlueAwsglueState>)
}

/// The `TaskStatusType` a data quality run is recorded in by a start: of the modelled enum
/// (`STARTING | RUNNING | STOPPING | STOPPED | SUCCEEDED | FAILED | TIMEOUT`), the one that
/// says the run has been accepted and nothing has begun analysing the data source yet.
pub const DATA_QUALITY_RUN_STATE_STARTING: &str = "STARTING";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run that finished its
/// analysis.
pub const DATA_QUALITY_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run that could not finish.
pub const DATA_QUALITY_RUN_STATE_FAILED: &str = "FAILED";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run a cancel has asked to
/// stop, which still owes the clock the status it stops in.
pub const DATA_QUALITY_RUN_STATE_STOPPING: &str = "STOPPING";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run a cancel stopped.
pub const DATA_QUALITY_RUN_STATE_STOPPED: &str = "STOPPED";

/// The `TaskStatusType` values a data quality run never moves out of again: of the
/// modelled enum quoted on [`DATA_QUALITY_RUN_STATE_STARTING`], the ones that name a
/// finished run. The other three (`STARTING`, `RUNNING`, `STOPPING`) each still owe a
/// further status.
pub const TERMINAL_DATA_QUALITY_RUN_STATES: [&str; 4] =
    ["STOPPED", "SUCCEEDED", "FAILED", "TIMEOUT"];

/// How long a data quality rule recommendation run stays in
/// [`DATA_QUALITY_RUN_STATE_STARTING`] before the clock owes it its terminal status.
///
/// CHOSEN, not documented: Glue publishes no duration for a recommendation run, so this is
/// the emulator's own span — deliberately not zero, so a caller that reads
/// `GetDataQualityRuleRecommendationRun` straight after starting one still observes the run
/// in flight, and the refusal that goes with a run in flight. The due instant is stored on
/// the run and materialised by whichever later call reads it; nothing sleeps.
pub const DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER: Duration =
    Duration::from_millis(500);

/// The `NumberOfWorkers` a recommendation run uses when the start names none: "The number
/// of G.1X workers to be used in the run. The default is 5."
/// (`StartDataQualityRuleRecommendationRunRequest$NumberOfWorkers`).
pub const DEFAULT_DATA_QUALITY_RUN_WORKERS: i32 = 5;

/// The `Timeout` in minutes a recommendation run uses when the start names none: "The
/// default is 2,880 minutes (48 hours)."
/// (`StartDataQualityRuleRecommendationRunRequest$Timeout`).
pub const DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES: i32 = 2_880;

/// The `RecommendationMode` a recommendation run uses when the start names none: "The mode
/// that Glue Data Quality uses to recommend rules. The default is `BASIC`."
/// (`StartDataQualityRuleRecommendationRunRequest$RecommendationMode`).
pub const DEFAULT_RECOMMENDATION_MODE: &str = "BASIC";

/// How many data quality rule recommendation runs this account keeps.
///
/// CHOSEN: the emulator's own window, for the same reason as
/// [`MAX_COLUMN_STATISTICS_TASK_RUNS`] — it bounds
/// [`ComAmazonawsGlueAwsglueState::data_quality_rule_recommendation_runs`] and every walk
/// over it. The OLDEST record is dropped first. (Glue documents that "recommendation runs
/// are automatically deleted after 90 days", which is a retention this process never
/// reaches.)
pub const MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS: usize = 100;

/// How many data quality rule recommendation run ids this process has minted. A nonce
/// source for [`new_data_quality_rule_recommendation_run_id`], exactly as
/// [`MINTED_COLUMN_STATISTICS_TASK_RUN_IDS`] is for a statistics run id.
static MINTED_DATA_QUALITY_RULE_RECOMMENDATION_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `RunId`: the id the service assigns a data quality rule recommendation run when
/// it starts.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255`. The bits come from [`mint_hex_id`], so two runs started in the same
/// nanosecond cannot be given the same id. It is stored on the run, so every later read of
/// that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_data_quality_rule_recommendation_run_id() -> String {
    mint_hex_id(
        &MINTED_DATA_QUALITY_RULE_RECOMMENDATION_RUN_IDS,
        DATA_QUALITY_RUN_ID_WORDS,
    )
}

/// How many 64-bit words a data quality run id carries: two, i.e. 128 bits rendered as 32
/// lowercase hexadecimal characters.
const DATA_QUALITY_RUN_ID_WORDS: usize = 2;

/// What the data source of a due recommendation run offers it: either the columns the
/// covered table declares, which is what the run recommends rules over, or the reason the
/// catalog offers it nothing to analyse.
#[derive(Debug, Clone)]
pub enum RecommendationRunOutcome {
    /// The covered table is stored and declares these columns, in the order it declares
    /// them.
    Recommended(Vec<String>),
    /// The catalog holds no such database or table, so the run has nothing to analyse and
    /// finishes carrying this reason.
    NothingToAnalyse(String),
}

/// The data source (a catalog table) a data quality run covers, as the request's
/// `DataSource` named it.
///
/// `DataSource` carries either a `GlueTable` ("An Glue table") or a
/// `DataQualityGlueTable` ("An Glue table for Data Quality Operations"), both of which
/// address a table by `DatabaseName` and `TableName` (`@required` on each) inside an
/// optional `CatalogId` — so the table a run covers is read out of whichever of the two the
/// request carried.
#[derive(Debug, Clone)]
pub struct DataSourceTable {
    /// `GlueTable$CatalogId`/`DataQualityGlueTable$CatalogId`, as named.
    pub catalog_id: Option<String>,
    /// The database name as the request spelled it.
    pub database_name: String,
    /// The table name as the request spelled it.
    pub table_name: String,
}

/// The table `data_source` addresses, or `None` when it addresses neither kind of table.
///
/// A `DataSource` that carries both is read as the plain `GlueTable` it names: that member
/// is the one every data quality operation documents as "an Glue table", and the other adds
/// only data-quality processing options on top of the same address.
pub fn data_source_table(data_source: &DataSource) -> Option<DataSourceTable> {
    if let Some(table) = data_source.glue_table.as_ref() {
        return Some(DataSourceTable {
            catalog_id: table.catalog_id.clone(),
            database_name: table.database_name.clone(),
            table_name: table.table_name.clone(),
        });
    }
    data_source
        .data_quality_glue_table
        .as_ref()
        .map(|table| DataSourceTable {
            catalog_id: table.catalog_id.clone(),
            database_name: table.database_name.clone(),
            table_name: table.table_name.clone(),
        })
}

/// One data quality rule recommendation run, as `StartDataQualityRuleRecommendationRun`
/// recorded it.
///
/// A run carries the data source it covers and the configuration the request named — with
/// each member the request left unset filled in with the default the model documents for it
/// — plus the members only a run has: its id, its status, the instants it moved between
/// statuses and the ruleset it recommended.
#[derive(Debug, Clone)]
pub struct DataQualityRuleRecommendationRunEntry {
    /// `RunId`, minted by [`new_data_quality_rule_recommendation_run_id`].
    pub id: String,
    /// The `DataSource` the start named, reported back as it arrived.
    pub data_source: DataSource,
    /// The catalog the covered table was resolved in, as [`resolve_catalog_id`] resolved
    /// it.
    pub catalog_id: String,
    /// The covered database's name, folded the way the catalog stores it.
    pub database_name: String,
    /// The covered table's name, folded the way the catalog stores it.
    pub table_name: String,
    /// `Role`: "The IAM role that Glue assumes to access resources for the run."
    pub role: String,
    /// `NumberOfWorkers`, defaulting to [`DEFAULT_DATA_QUALITY_RUN_WORKERS`].
    pub number_of_workers: i32,
    /// `Timeout` in minutes, defaulting to
    /// [`DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES`].
    pub timeout: i32,
    /// `CreatedRulesetName`: "A name for the ruleset" the run creates, absent when the
    /// start named none.
    pub created_ruleset_name: Option<String>,
    /// `DataQualitySecurityConfiguration`, as the start named it.
    pub data_quality_security_configuration: Option<String>,
    /// The `ClientToken` the start carried, which is what makes a repeat of that start
    /// answer this run instead of starting another.
    pub client_token: Option<String>,
    /// `AdditionalRunOptions`, as the start named them.
    pub additional_run_options: Option<DataQualityRuleRecommendationRunAdditionalRunOptions>,
    /// `RecommendationMode`, defaulting to [`DEFAULT_RECOMMENDATION_MODE`].
    pub recommendation_mode: String,
    /// One of the modelled `TaskStatusType` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// `StartedOn`: "The date and time when this run started."
    pub started_on: SystemTime,
    /// `LastModifiedOn`: "The last point in time when this data quality rule
    /// recommendation run was modified."
    pub last_modified_on: SystemTime,
    /// `CompletedOn`: "The date and time when this run was completed", absent until the run
    /// has finished.
    pub completed_on: Option<SystemTime>,
    /// `RecommendedRuleset`: the rules the finished run recommends, in DQDL, absent until
    /// it has finished and when it recommended none.
    pub recommended_ruleset: Option<String>,
    /// `ErrorString`: "The error strings that are associated with the run", absent unless
    /// the run failed.
    pub error_string: Option<String>,
}

impl DataQualityRuleRecommendationRunEntry {
    /// Record a run of the data source the `StartDataQualityRuleRecommendationRun` request
    /// names, in [`DATA_QUALITY_RUN_STATE_STARTING`] with its next status due
    /// [`DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER`] out.
    ///
    /// `catalog_id` is the catalog the data source resolved in, and `table` the address it
    /// carried.
    pub fn start(
        input: &StartDataQualityRuleRecommendationRunInput,
        table: &DataSourceTable,
        catalog_id: &str,
        id: String,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            data_source: input.data_source.clone(),
            catalog_id: catalog_id.to_string(),
            database_name: fold_name(&table.database_name),
            table_name: fold_name(&table.table_name),
            role: input.role.clone(),
            number_of_workers: input
                .number_of_workers
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            timeout: input
                .timeout
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            created_ruleset_name: input.created_ruleset_name.clone(),
            data_quality_security_configuration: input.data_quality_security_configuration.clone(),
            client_token: input.client_token.clone(),
            additional_run_options: input.additional_run_options.clone(),
            recommendation_mode: input
                .recommendation_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_RECOMMENDATION_MODE.to_string()),
            status: DATA_QUALITY_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                now.checked_add(DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            // Nothing has been analysed yet, and nothing has failed.
            recommended_ruleset: None,
            error_string: None,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_DATA_QUALITY_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_DATA_QUALITY_RUN_STATES.contains(&self.status.as_str())
    }

    /// Whether the clock owed this run its next status at `now`.
    pub fn is_due(&self, now: SystemTime) -> bool {
        self.settles_at.is_some_and(|due| due <= now)
    }

    /// Whether a cancel has asked this run to stop, so what it settles into is that stop
    /// rather than anything its data source holds.
    pub fn is_stopping(&self) -> bool {
        self.status == DATA_QUALITY_RUN_STATE_STOPPING
    }

    /// How long the run consumed resources, in whole seconds: `ExecutionTime`, "the amount
    /// of time (in seconds) that the run consumed resources", which is the span between the
    /// instant it started and the instant it completed. A run still in flight has consumed
    /// no completed time, so it reports the member's own default of 0.
    pub fn execution_time_seconds(&self) -> i32 {
        self.completed_on
            .and_then(|completed| completed.duration_since(self.started_on).ok())
            .map_or(0, |consumed| {
                i32::try_from(consumed.as_secs()).unwrap_or(i32::MAX)
            })
    }

    /// Close this run at the DUE instant the clock set for it, and answer that instant;
    /// `None` when the clock owed it nothing, in which case nothing is written.
    ///
    /// The instants recorded are that DUE instant rather than the instant somebody read it,
    /// so two reads of a settled run report the same run.
    fn close_at_due_instant(&mut self) -> Option<SystemTime> {
        let due = self.settles_at?;
        self.settles_at = None;
        self.completed_on = Some(due);
        self.last_modified_on = due;
        Some(due)
    }

    /// Move this run into the status the clock owed it, given what its data source offers.
    ///
    /// The run was due to leave [`DATA_QUALITY_RUN_STATE_STARTING`] at
    /// [`Self::settles_at`] whether or not anybody called. A run a cancel asked to stop
    /// settles through [`Self::settle_stop_if_due`] instead, which consults no data source.
    pub fn settle(&mut self, outcome: RecommendationRunOutcome) {
        if self.close_at_due_instant().is_none() {
            return;
        }
        match outcome {
            RecommendationRunOutcome::Recommended(columns) => {
                self.status = DATA_QUALITY_RUN_STATE_SUCCEEDED.to_string();
                self.recommended_ruleset = recommended_ruleset(&columns);
            }
            RecommendationRunOutcome::NothingToAnalyse(reason) => {
                self.status = DATA_QUALITY_RUN_STATE_FAILED.to_string();
                self.error_string = Some(reason);
            }
        }
    }

    /// Settle a run a cancel asked to stop, once the stop the clock owed it is due: it
    /// finishes [`DATA_QUALITY_RUN_STATE_STOPPED`], having analysed nothing further, so its
    /// data source is never consulted and it recommends no ruleset.
    pub fn settle_stop_if_due(&mut self, now: SystemTime) {
        if !self.is_due(now) || self.close_at_due_instant().is_none() {
            return;
        }
        self.status = DATA_QUALITY_RUN_STATE_STOPPED.to_string();
    }

    /// Ask this run to stop, as `CancelDataQualityRuleRecommendationRun` does, and answer
    /// whether it was still running.
    ///
    /// A run in flight moves into [`DATA_QUALITY_RUN_STATE_STOPPING`] and the clock owes it
    /// [`DATA_QUALITY_RUN_STATE_STOPPED`] one span later, so the stop is observable while it
    /// drains. A run that has already finished is left exactly as it is.
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() {
            return false;
        }
        self.status = DATA_QUALITY_RUN_STATE_STOPPING.to_string();
        self.last_modified_on = now;
        self.settles_at = Some(
            now.checked_add(DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER)
                .unwrap_or(now),
        );
        true
    }
}

/// The most bytes a recommended ruleset may carry: the model's own `length 1..65536` on
/// `DataQualityRulesetString`, the shape
/// `GetDataQualityRuleRecommendationRunResponse$RecommendedRuleset` targets. A table wide
/// enough to overrun it is recommended as many rules as fit and no more, so the member
/// stays inside the constraint it is modelled with.
pub const MAX_DATA_QUALITY_RULESET_BYTES: usize = 65_536;

/// How a DQDL rule list opens, and how it closes.
const DQDL_RULES_PREFIX: &str = "Rules = [\n";
/// See [`DQDL_RULES_PREFIX`].
const DQDL_RULES_SUFFIX: &str = "\n]";

/// The ruleset a finished recommendation run recommends over a table that declares
/// `columns`, in DQDL — "when a start rule recommendation run completes, it creates a
/// recommended ruleset (a set of rules). This member has those rules in Data Quality
/// Definition Language (DQDL) format"
/// (`GetDataQualityRuleRecommendationRunResponse$RecommendedRuleset`).
///
/// The rules are computed from the SCHEMA the catalog holds for the covered table: one
/// `ColumnExists` rule per column the table declares, in the order it declares them. This
/// emulator holds the Glue CONTROL plane and reads none of the objects behind the table, so
/// it profiles no data and a rule whose truth depends on the DATA (a row count, a
/// completeness, a uniqueness or a value range — which is most of what a real recommendation
/// run proposes) is deliberately not recommended: what the run recommends is what the stored
/// schema actually supports. A table that declares no columns yields no rules at all, and so
/// no ruleset.
///
/// A quote inside a column name is escaped, so one name can never end the string its rule
/// sits in, and rules stop being added once the next one would carry the ruleset past
/// [`MAX_DATA_QUALITY_RULESET_BYTES`].
pub fn recommended_ruleset(columns: &[String]) -> Option<String> {
    let mut rules = String::new();
    for column in columns {
        let rule = format!("    ColumnExists \"{}\"", column.replace('"', "\\\""));
        let separator = if rules.is_empty() { "" } else { ",\n" };
        let would_carry = DQDL_RULES_PREFIX.len()
            + rules.len()
            + separator.len()
            + rule.len()
            + DQDL_RULES_SUFFIX.len();
        if would_carry > MAX_DATA_QUALITY_RULESET_BYTES {
            break;
        }
        rules.push_str(separator);
        rules.push_str(&rule);
    }
    if rules.is_empty() {
        return None;
    }
    Some(format!("{DQDL_RULES_PREFIX}{rules}{DQDL_RULES_SUFFIX}"))
}


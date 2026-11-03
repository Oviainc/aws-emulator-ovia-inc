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
//! CLASSIFIERS — the custom classifiers `CreateClassifier` registers for a crawler to
//! recognise a data format with — sit in such a per-account namespace beside the crawlers
//! that refer to them ([`ComAmazonawsGlueAwsglueState::classifiers`]), keyed by classifier
//! name, because every operation that addresses one (`GetClassifier`, `UpdateClassifier`,
//! `DeleteClassifier`) names it by `Name` alone and carries no `CatalogId`. They follow
//! the crawler's per-account convention rather than the job's per-Region one because a
//! crawler's `Classifiers` list refers to a classifier by name alone, so the two have to
//! share one namespace. The four KINDS the model declares share that namespace rather
//! than having one each: a read takes a name alone and answers with whichever kind that
//! name holds ([`ClassifierEntry`]).
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
//! the `FormTypeId` a put reports, which IS that name ([`FormTypeEntry::id`]) — which is
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
//! ASSETS — the catalog entities `PutAsset` registers, each one an instance of an asset
//! type carrying its metadata forms — sit in their own per-account namespace beside the
//! asset types they are instances of ([`ComAmazonawsGlueAwsglueState::assets`]). They are
//! keyed by the `AssetId` the PUT's caller chose rather than by a minted id, because that
//! identifier is what every operation addressing an asset carries (`PutAsset$Identifier`,
//! `GetAsset$Identifier`, `AssociateGlossaryTerms$AssetIdentifier`) and it is what decides
//! whether a put creates an asset or updates the one already stored. The asset type an
//! asset is an instance of is recorded ON the asset ([`AssetEntry::asset_type_id`]), as the
//! put addressed it.
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
//! Which terms an ASSET carries is recorded on the asset instead
//! ([`AssetEntry::glossary_terms`]), as the set of ids its associations named: the
//! association is to the id, so the two namespaces are independent and a term deleted
//! after being associated leaves its id on the asset.
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
//! INTEGRATION RESOURCE PROPERTIES — the processing properties
//! `CreateIntegrationResourceProperty` sets on the Glue connection an integration reads
//! from, or on the Glue database it writes into — sit in such a per-account namespace too,
//! keyed by that resource's ARN
//! ([`ComAmazonawsGlueAwsglueState::integration_resource_properties`]), for the reason a
//! resource policy is: every operation that addresses one record names it by `ResourceArn`
//! alone. One record per resource, so a second create for a resource that already has one
//! writes THAT record rather than adding a second.
//!
//! INTEGRATION TABLE PROPERTIES — the override properties
//! `CreateIntegrationTableProperties` sets for ONE table that an integration replicates —
//! sit in the same kind of per-account namespace
//! ([`ComAmazonawsGlueAwsglueState::integration_table_properties`]), keyed by the
//! `ResourceArn` the request named AND the `TableName` it carried, because those two
//! members are the whole address every operation that reaches one record
//! (`CreateIntegrationTableProperties`, `GetIntegrationTableProperties`,
//! `UpdateIntegrationTableProperties`, `DeleteIntegrationTableProperties`) takes. One
//! record per (resource, table) pair, so a second create for a pair that already has one
//! writes THAT record.
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
//! BLUEPRINTS — the reusable workflow templates a client registers with
//! `CreateBlueprint` — sit beside that tree in another per-account namespace keyed by
//! blueprint name ([`ComAmazonawsGlueAwsglueState::blueprints`]), because every operation
//! that addresses a blueprint (`GetBlueprint`, `UpdateBlueprint`, `DeleteBlueprint`,
//! `StartBlueprintRun`) names it by `Name` alone and carries no `CatalogId`, and
//! `ListBlueprints` "lists all the blueprint names in an account". Ordered, so that
//! listing has one deterministic order to page over.
//!
//! DATA QUALITY RULE RECOMMENDATION RUNS — the runs `StartDataQualityRuleRecommendationRun`
//! starts over one data source (a catalog table) — sit beside the catalog tree in the same
//! per-account list as the column statistics task runs
//! ([`ComAmazonawsGlueAwsglueState::data_quality_rule_recommendation_runs`]): a run is
//! ADDRESSED by the `RunId` the start mints, which
//! `GetDataQualityRuleRecommendationRun` takes and nothing else, so the data source a run
//! covers is recorded on the run rather than the run on the table.
//!
//! DATA QUALITY RULESET EVALUATION RUNS — the runs
//! `StartDataQualityRulesetEvaluationRun` starts of stored rulesets over one data source —
//! sit beside the catalog tree in their own per-account list
//! ([`ComAmazonawsGlueAwsglueState::data_quality_ruleset_evaluation_runs`]), for the same
//! reason a recommendation run does: `GetDataQualityRulesetEvaluationRun` addresses a run
//! by the `RunId` the start minted and nothing else, so the data source and the rulesets a
//! run evaluates are recorded on the run.
//!
//! DATA QUALITY RULESETS — the DQDL rule sets `CreateDataQualityRuleset` registers — sit
//! beside the catalog tree in a per-account namespace keyed by ruleset name
//! ([`ComAmazonawsGlueAwsglueState::data_quality_rulesets`]), because every operation that
//! addresses one (`GetDataQualityRuleset`, `UpdateDataQualityRuleset`,
//! `DeleteDataQualityRuleset`) names it by `Name` alone and carries no `CatalogId`. The
//! Glue table a ruleset is written against is recorded ON the ruleset, the way a
//! recommendation run records its data source, because the ruleset's name is its whole
//! address and the target table is optional.
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
//! (`MLTransform$Name`). That name is still the one the CREATE's client chooses, and a
//! create has to choose a free one ([`ComAmazonawsGlueAwsglueState::ml_transform_by_name`]).
//! The catalog TABLES a transform reads are recorded
//! on the transform as the create named them ([`MlTransformEntry::input_record_tables`]),
//! which is where its reported schema is taken from.
//!
//! INTERACTIVE SESSIONS — the remote Spark runtime environments `CreateSession` provisions
//! ([`ComAmazonawsGlueAwsglueState::sessions`]) — sit beside the catalog tree, keyed by the
//! REGION they were created in and then by the `Id` the create chose: every operation that
//! addresses a session names it by `Id` alone with no `CatalogId`, and the runtime a session
//! is is provisioned in the Region its create was served in, so a session is served by the
//! endpoint that created it exactly as a job definition is. A session's status is not final
//! when the create returns — the environment is still being provisioned — so the record
//! carries the instant the clock owes it the next status ([`SessionEntry::settles_at`]) and
//! the instant its own timeouts fall due, and whichever later call reads it materialises
//! what the clock had already reached.
//!
//! A session also holds the STATEMENTS run on it ([`SessionEntry::statements`]), in the
//! order they were submitted, because every operation that addresses a statement names its
//! session and then an `Id` the session itself mints — so a statement is a record of the
//! session it was run on and not an account-level resource. A statement that has not
//! finished is what makes its session busy, which is what a further run of that session is
//! refused by.
//!
//! Database and table names are folded to lowercase on the way in — the model says
//! so for both `DatabaseInput$Name` ("For Hive compatibility, this is folded to
//! lowercase when it is stored") and `TableInput$Name` — and every lookup folds the
//! requested name the same way, so the fold cannot make a resource unreachable. A JOB,
//! CONNECTION or CRAWLER name is NOT folded: the model documents the fold for the two
//! Hive-compatible catalog resources only.

pub mod catalog_encryption;

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use self::catalog_encryption::ObjectEncryption;

use crate::ops::types::{
    Action, AssetFormEntry, AssetTypeFormReference, CatalogInput, CatalogProperties,
    CatalogPropertiesOutput,
    Classifier,
    CodeGenConfigurationNode, Column, ColumnStatisticsTaskRun, ColumnStatisticsTaskSettings,
    ConfigurationObject,
    Condition,
    Connection, ConnectionInput,
    ConnectionPasswordEncryption, ConnectionsList, Crawler, CrawlerTargets, CreateClassifierInput,
    CreateColumnStatisticsTaskSettingsInput, CreateCrawlerInput,
    CreateCustomEntityTypeInput, CreateDataQualityRulesetInput, CreateGlossaryInput,
    CreateGlossaryTermInput,
    CreateIntegrationResourcePropertyInput, CreateIntegrationTablePropertiesInput, CreateJobInput,
    CreateIcebergTableInput, IcebergSchema, JobBookmarkEntry, TableOptimizer,
    TableOptimizerConfiguration,
    CreateMLTransformInput, CsvClassifier,
    CreateSecurityConfigurationInput, CreateSessionInput, CreateTriggerInput, CreateWorkflowInput,
    CustomEntityType,
    DataCatalogEncryptionSettings, DataLakeAccessPropertiesOutput, DataLakePrincipal,
    DataQualityRuleRecommendationRunAdditionalRunOptions, DataQualityEvaluationRunAdditionalRunOptions,
    DataQualityRulesetFilterCriteria,
    DataQualityRulesetListDetails, DataQualityTargetTable,
    DataSource, Database, DatabaseIdentifier, DatabaseInput,
    Edge, EncryptionAtRest, EncryptionConfiguration, ErrorDetail, EventBatchingCondition,
    ExecutionAttempt, ExecutionProperty, FederatedCatalog, FederatedDatabase,
    FederatedTable, GlueTable, GrokClassifier, IcebergOptimizationPropertiesOutput, Job,
    JsonClassifier, MLTransform,
    CreateUsageProfileInput,
    JobCommand, JobNodeDetails, JobRun, JobUpdate, LakeFormationConfiguration,
    LineageConfiguration,
    Node, NotificationProperty, Partition, PartitionInput, Predicate, PrincipalPermissions,
    ProfileConfiguration,
    RLSProperties,
    RecrawlPolicy, RegistryId, Schedule,
    SchemaChangePolicy, SchemaColumn, SchemaId, SecurityConfiguration, Session, SessionCommand,
    SourceControlDetails,
    SourceProcessingProperties, SourceTableConfig,
    StartColumnStatisticsTaskRunInput,
    StartDataQualityRuleRecommendationRunInput,
    StartDataQualityRulesetEvaluationRunInput,
    StartJobRunInput, StorageDescriptor, Table, TableIdentifier, TableInput,
    Statement, StatementOutput,
    TableVersion, TargetRedshiftCatalog,
    TransformEncryption, TransformFilterCriteria, TransformParameters, TransformSortCriteria,
    Blueprint, BlueprintDetails, BlueprintRun, CreateBlueprintInput, StartBlueprintRunInput,
    Trigger, TriggerNodeDetails,
    Tag, TargetProcessingProperties, TargetTableConfig,
    TriggerUpdate,
    UpdateClassifierInput, UpdateCrawlerInput, UpdateDataQualityRulesetInput, UpdateGlossaryInput,
    UpdateMLTransformInput,
    UpdateWorkflowInput,
    Workflow, WorkflowGraph,
    WorkflowRun,
    WorkflowRunStatistics, XMLClassifier,
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

/// How many names ONE request may give a batch reader of this service to look up.
///
/// This emulator's own REQUEST-SIZE bound, with no modelled counterpart: a batch reader's
/// name list bounds each element's length but says nothing about how many elements it may
/// hold, while the answer carries one whole resource projection per name the request
/// lists, repeats included. So what one request can make such an operation look up and
/// allocate is bounded here, at the figure this service's paginated readers already serve
/// in one answer (`GetTables`' page ceiling, 100 records), so a batch cannot ask for a
/// larger answer than a page. ONE figure, shared by every batch reader that needs it, so
/// two operations never enforce two different ceilings.
///
/// A batch past it is refused with the asking operation's own modelled
/// `InvalidInputException`, in a message that reports this figure.
pub const MAX_BATCH_NAMES: usize = 100;

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

/// Ceiling on the data quality rulesets this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateDataQualityRuleset` writes a record any caller can reach, and
/// each one carries up to the modelled 65536 characters of DQDL, so the number of records
/// one process will hold is bounded here. A create past it is refused with
/// `CreateDataQualityRuleset`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_DATA_QUALITY_RULESETS: usize = 1_000;

/// Ceiling on the classifiers this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. `CreateClassifier` models no limit error, so a create past it is
/// refused with the closest client error that operation does model — its
/// `InvalidInputException` — exactly as `PutFormType`'s own ceiling is.
pub const MAX_CLASSIFIERS: usize = 1_000;

/// The `Version` a classifier carries straight after `CreateClassifier`: the first
/// version of a classifier is 1, and an update moves it on, exactly as
/// [`FIRST_CRAWLER_VERSION`] does for a crawler. The model's `@default` of 0 on
/// `GrokClassifier$Version` and its three peers is what an ABSENT long deserializes to,
/// not the version of a stored classifier.
pub const FIRST_CLASSIFIER_VERSION: i64 = 1;

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

/// Ceiling on the assets this emulator will hold, for the same reason as
/// [`MAX_ASSET_TYPES`]: `PutAsset` writes a record any caller can reach, each one carrying
/// its forms and their JSON content, so the number of records one process will hold is
/// bounded here. A put that would add one past the ceiling is refused rather than stored;
/// `PutAsset` models no limit error, so the refusal is the closest client error it does
/// model (`InvalidInputException`).
pub const MAX_ASSETS: usize = 1_000;

/// How many glossary terms one asset can carry: the modelled bound on the member that
/// REPORTS them, "len 1..10" on `GlossaryTermIdList`
/// (`smithy_query operation=AssociateGlossaryTerms`), which is the shape both
/// `AssociateGlossaryTermsResponse$GlossaryTerms` and `GetAssetOutput$GlossaryTerms` target
/// — so an asset holding more than this many terms could not be reported by either reader,
/// whatever the input list of one request is allowed to carry. An association that would
/// leave an asset past it is therefore refused with the closest client error the operation
/// models (`InvalidInputException`) rather than stored.
pub const MAX_ASSET_GLOSSARY_TERMS: usize = 10;

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

/// Ceiling on the blueprints this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateBlueprint` writes a record any caller can reach, so the number
/// of records one process will hold is bounded here. A create past the ceiling is refused
/// with `CreateBlueprint`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_BLUEPRINTS: usize = 1_000;

/// Ceiling on the interactive sessions this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateSession` writes a record any caller can reach, each one carrying
/// its default arguments and connections, so the number of records one process will hold is
/// bounded here. A create past the ceiling is refused with `CreateSession`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_SESSIONS: usize = 1_000;

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

/// Ceiling on the integration resource property records this emulator will hold, for the
/// same reason as [`MAX_CRAWLERS`]: `CreateIntegrationResourceProperty` writes a record any
/// caller can reach, each one carrying the processing properties of one resource, so the
/// number of records one process will hold is bounded here. A create that would add one
/// past the ceiling is refused rather than stored; that operation models no limit error, so
/// the refusal is the closest client error it does model (`InvalidInputException`).
pub const MAX_INTEGRATION_RESOURCE_PROPERTIES: usize = 1_000;

/// Ceiling on the integration table property records this emulator will hold, for the same
/// reason as [`MAX_INTEGRATION_RESOURCE_PROPERTIES`]: `CreateIntegrationTableProperties`
/// writes a record any caller can reach, one per (resource, table) pair, so the number of
/// records one process will hold is bounded here. A create that would add one past the
/// ceiling is refused rather than stored; that operation models no limit error, so the
/// refusal is the closest client error it does model (`InvalidInputException`).
pub const MAX_INTEGRATION_TABLE_PROPERTIES: usize = 1_000;

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

/// The `Schedule$State` of a crawler whose cron expression is in force: the state every
/// write that STATES a schedule leaves it in, because a schedule a write puts on a
/// crawler is in effect the moment that write returns. One of the modelled `ScheduleState`
/// values (`SCHEDULED`, `NOT_SCHEDULED`, `TRANSITIONING`); a crawler created WITHOUT a
/// `Schedule` has no schedule object at all.
pub const SCHEDULE_STATE_SCHEDULED: &str = "SCHEDULED";

/// The `Schedule$State` of a crawler whose schedule is SUSPENDED: what
/// `StopCrawlerSchedule` leaves it in ("sets the schedule state of the specified crawler
/// to NOT_SCHEDULED"), and the state a crawler that carries no cron expression at all is
/// in. Another of the modelled `ScheduleState` values.
pub const SCHEDULE_STATE_NOT_SCHEDULED: &str = "NOT_SCHEDULED";

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
///
/// A trigger that waits on nothing but a caller is never in this state: it has no armed
/// state to be put into, so a start of one is a firing and leaves it in
/// [`TRIGGER_STATE_CREATED`]. See [`TriggerEntry::start`].
pub const TRIGGER_STATE_ACTIVATED: &str = "ACTIVATED";

/// The `TriggerState` of a trigger that has been stopped: the one modelled state of a
/// trigger that was asked to stop ("Stops a specified trigger" — `StopTrigger`), which
/// every later read of that trigger reports until a start moves it back to
/// [`TRIGGER_STATE_ACTIVATED`]. Another of the modelled `TriggerState` values.
///
/// A stop leaves the trigger in this STEADY state rather than in the enum's
/// `DEACTIVATING`, exactly as a start leaves it in [`TRIGGER_STATE_ACTIVATED`] rather than
/// in `ACTIVATING`: nothing in this emulator keeps a trigger's schedule or predicate
/// running, so the clock owes a stopped trigger no further transition.
pub const TRIGGER_STATE_DEACTIVATED: &str = "DEACTIVATED";

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

/// The `TriggerType` of a trigger that fires on the events EventBridge hands the workflow
/// it belongs to, as its `EventBatchingCondition` decides: "Number of events that must be
/// received from Amazon EventBridge before EventBridge event trigger fires"
/// (`EventBatchingCondition$BatchSize`). The one type whose firing comes from OUTSIDE this
/// service.
pub const TRIGGER_TYPE_EVENT: &str = "EVENT";

/// The batch size an EVENT trigger counts deliveries against when its
/// `EventBatchingCondition` names none at all.
///
/// `BatchSize` is `@required` INSIDE the condition but the condition itself is optional, so
/// a trigger carrying no condition batches nothing: it fires on the first event it is
/// handed, which is the lower bound of the member's `@range 1..100`.
pub const DEFAULT_EVENT_BATCH_SIZE: i32 = 1;

/// The window an EVENT trigger's open batch is given when its `EventBatchingCondition`
/// names no `BatchWindow`.
///
/// "Window of time in seconds after which EventBridge event trigger fires. Window starts
/// when first event is received" (`EventBatchingCondition$BatchWindow`, `@range 1..900`):
/// Glue documents 900 seconds — the member's upper bound — as the window an event trigger
/// batches over by default, so a condition that names only a `BatchSize` waits that long
/// for the batch to fill before firing with what it has.
pub const DEFAULT_EVENT_BATCH_WINDOW_SECONDS: u64 = 900;

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
    /// Every classifier this emulator holds, keyed by its name, in name order.
    ///
    /// Per account and beside the catalog tree, like the crawlers that refer to them:
    /// `GetClassifier`, `UpdateClassifier` and `DeleteClassifier` address a classifier by
    /// `Name` alone, with no `CatalogId` member, so the name a create sends is the whole
    /// address. Ordered, so `GetClassifiers` has one deterministic order to page over, and
    /// bounded by [`MAX_CLASSIFIERS`]. The key is the name exactly as sent — the model
    /// documents no case fold for a classifier name.
    ///
    /// NOT keyed by Region, which is the one choice this namespace makes differently from
    /// [`Self::jobs`] and [`Self::triggers`]: it follows the CRAWLER convention
    /// ([`Self::crawlers`]) because a classifier is reached only through a crawler's
    /// `Classifiers` list, which names classifiers by name alone, so a classifier has to
    /// live in the same namespace as the crawlers that may refer to it or a crawler could
    /// name a classifier its own namespace cannot resolve. A job is keyed by Region
    /// instead because its own uniqueness is documented per account and its runs are
    /// served only by the endpoint that created it; nothing of that kind holds for a
    /// classifier.
    pub classifiers: BTreeMap<String, ClassifierEntry>,
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
    /// A form type is ADDRESSED by an `Identifier` targeting `FormTypeId`, the same shape
    /// `PutFormType`'s `Id` carries, and that identifier is the form type's NAME
    /// ([`FormTypeEntry::id`]) — so this key is the address.
    /// [`Self::form_type_by_reference`] is that resolution.
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
    /// Every asset this emulator holds, keyed by its `AssetId`.
    ///
    /// Per account and beside the asset types they are instances of: `PutAsset` names an
    /// asset by `Identifier` alone, with no `CatalogId` member, and every operation that
    /// reads, updates, deletes or annotates an asset carries that same identifier. Keyed by
    /// it because the identifier is the whole address and is the CLIENT's, not minted here:
    /// "The unique identifier of the asset. If an asset with this identifier already
    /// exists, it is updated" (`PutAssetRequest$Identifier`). Ordered, so a listing of
    /// assets has one deterministic order to page over, and bounded by [`MAX_ASSETS`]. The
    /// key is the identifier exactly as sent — the model documents no case fold for
    /// `AssetId`.
    pub assets: BTreeMap<String, AssetEntry>,
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
    /// Every data quality ruleset evaluation run this emulator holds, oldest first.
    ///
    /// A run lives BESIDE the catalog tree rather than on the table it evaluates, for the
    /// same reason a rule recommendation run does:
    /// `GetDataQualityRulesetEvaluationRun` addresses one by `RunId` alone, with no
    /// catalog, database or table member, so the namespace is per account and the data
    /// source and the rulesets a run evaluates are recorded ON the run
    /// ([`DataQualityRulesetEvaluationRunEntry`]). Kept in start order and bounded by
    /// [`MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS`].
    pub data_quality_ruleset_evaluation_runs: Vec<DataQualityRulesetEvaluationRunEntry>,
    /// Every data quality ruleset this emulator holds, keyed by its name, in name order.
    ///
    /// Per account and beside the catalog tree, like crawlers and custom entity types:
    /// `GetDataQualityRuleset`, `UpdateDataQualityRuleset` and `DeleteDataQualityRuleset`
    /// address a ruleset by `Name` alone, with no `CatalogId` member, and
    /// `CreateDataQualityRulesetRequest$Name` is "a unique name for the data quality
    /// ruleset". The target table a ruleset is written against is recorded ON the ruleset
    /// ([`DataQualityRulesetEntry::target_table`]) rather than the ruleset on the table,
    /// because that name is the whole address and the `TargetTable` is optional. Ordered,
    /// so `ListDataQualityRulesets` has one deterministic order to page over, and bounded
    /// by [`MAX_DATA_QUALITY_RULESETS`]. The key is the name exactly as sent — the model
    /// documents no case fold for `NameString`.
    pub data_quality_rulesets: BTreeMap<String, DataQualityRulesetEntry>,
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
    /// Every integration resource property record this emulator holds, keyed by the ARN of
    /// the resource whose properties it carries, in ARN order.
    ///
    /// A record lives BESIDE the catalog tree, like a resource policy, because every
    /// operation that addresses one (`CreateIntegrationResourceProperty`,
    /// `GetIntegrationResourceProperty`, `UpdateIntegrationResourceProperty`,
    /// `DeleteIntegrationResourceProperty`) addresses it by `ResourceArn` alone, with no
    /// `CatalogId` member — so the namespace is per account and the resource a record
    /// belongs to is the key. The key is the ARN exactly as sent: the model documents no
    /// fold for it. Ordered, so `ListIntegrationResourceProperties` has one deterministic
    /// order to page over, and bounded by [`MAX_INTEGRATION_RESOURCE_PROPERTIES`].
    pub integration_resource_properties: BTreeMap<String, IntegrationResourcePropertyEntry>,
    /// Every integration table property record this emulator holds, keyed by the
    /// `ResourceArn` whose leg of the integration the properties belong to and then by the
    /// `TableName` they override, in that order.
    ///
    /// A record lives BESIDE the catalog tree, like an integration resource property
    /// record, because every operation that addresses one
    /// (`CreateIntegrationTableProperties`, `GetIntegrationTableProperties`,
    /// `UpdateIntegrationTableProperties`, `DeleteIntegrationTableProperties`) addresses it
    /// by `ResourceArn` and `TableName` alone, with no `CatalogId` member — so the
    /// namespace is per account and the pair a record belongs to is the key. Both halves of
    /// the key are as sent: the model documents no fold for either member, and the table
    /// named is "the table to be replicated"
    /// (`CreateIntegrationTablePropertiesRequest$TableName`), a source-side entity rather
    /// than a catalog table whose name the catalog folds. Ordered, so
    /// `ListIntegrationTableProperties` has one deterministic order to page over, and
    /// bounded by [`MAX_INTEGRATION_TABLE_PROPERTIES`].
    pub integration_table_properties: BTreeMap<(String, String), IntegrationTablePropertiesEntry>,
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
    /// Every blueprint this emulator holds, keyed by its name.
    ///
    /// Per account and beside the catalog tree, like crawlers and workflows:
    /// `GetBlueprint`, `UpdateBlueprint` and `DeleteBlueprint` address a blueprint by
    /// `Name` alone, with no `CatalogId` member, and `ListBlueprints` "lists all the
    /// blueprint names in an account". Ordered, so that listing has one deterministic
    /// order to page over. The key is the name exactly as sent — the model documents no
    /// case fold for a blueprint name, whose pattern (`^[\.\-_A-Za-z0-9]+$`) admits both
    /// cases and so distinguishes them.
    pub blueprints: BTreeMap<String, BlueprintEntry>,
    /// Every interactive session this emulator holds, keyed by the REGION it was created in
    /// and then by the `Id` its create chose, in id order.
    ///
    /// Sessions live BESIDE the catalog tree, like jobs and triggers: `GetSession`,
    /// `StopSession`, `DeleteSession` and every statement operation address a session by
    /// `Id` alone, with no `CatalogId` member. The outer key is the Region for the reason a
    /// job's is: Glue is a REGIONAL service and a session is a runtime provisioned in the
    /// Region its create was served in, so a session created through one Region's endpoint is
    /// served only through that endpoint and the same id is free in every other Region. The
    /// inner map is ordered, so `ListSessions` has one deterministic order to page over, and
    /// bounded by [`MAX_SESSIONS`]; its key is the id exactly as sent — the model documents
    /// no case fold for `NameString`.
    pub sessions: HashMap<String, BTreeMap<String, SessionEntry>>,
    /// The Glue IDENTITY CENTER CONFIGURATION of this account, or `None` while the account
    /// has none.
    ///
    /// An ACCOUNT SINGLETON: `GetGlueIdentityCenterConfiguration`,
    /// `DeleteGlueIdentityCenterConfiguration` and
    /// `UpdateGlueIdentityCenterConfiguration` take no input at all, so the account is the
    /// whole address of the configuration and there is at most one of it. A create while
    /// one is stored is therefore a duplicate rather than an overwrite, and a read or a
    /// delete while none is stored is a miss.
    pub identity_center_configuration: Option<IdentityCenterConfigurationEntry>,
}

/// The Glue Identity Center configuration of one account: the Identity Center instance the
/// account's Glue services authenticate against, plus the application Glue registered with
/// that instance.
#[derive(Debug, Clone)]
pub struct IdentityCenterConfigurationEntry {
    /// "The Amazon Resource Name (ARN) of the Identity Center application associated with
    /// the Glue configuration" — minted by the create
    /// ([`new_identity_center_application_arn`]) and stored, so every later read reports
    /// the same application.
    pub application_arn: String,
    /// "The Amazon Resource Name (ARN) of the Identity Center instance", as the create
    /// named it.
    pub instance_arn: String,
    /// "A list of Identity Center scopes that define the permissions and access levels for
    /// the Glue configuration", as the last write set them.
    pub scopes: Option<Vec<String>>,
    /// "Indicates whether users can run background sessions when using Identity Center
    /// authentication with Glue services", as the last write set it.
    pub user_background_sessions_enabled: Option<bool>,
}

impl IdentityCenterConfigurationEntry {
    /// Register the configuration a `CreateGlueIdentityCenterConfiguration` asks for,
    /// against the Identity Center instance `instance_arn` names.
    pub fn new(
        instance_arn: &str,
        scopes: Option<&Vec<String>>,
        user_background_sessions_enabled: Option<bool>,
    ) -> Self {
        Self {
            application_arn: new_identity_center_application_arn(instance_arn),
            instance_arn: instance_arn.to_string(),
            scopes: scopes.cloned(),
            user_background_sessions_enabled,
        }
    }
}

/// How many Identity Center application ids this process has minted. A nonce source for
/// [`new_identity_center_application_arn`], exactly as [`MINTED_DATABASE_IDS`] is for a
/// database id.
static MINTED_IDENTITY_CENTER_APPLICATIONS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words an Identity Center application id carries: one, i.e. 64 bits
/// rendered as 16 lowercase hexadecimal characters, which is the width Identity Center
/// spells the id part of an application id (`apl-<16 hex>`) in.
const IDENTITY_CENTER_APPLICATION_ID_WORDS: usize = 1;
/// The prefix Identity Center gives an application id, as it appears in the resource part
/// of an application ARN (`application/<instance id>/apl-<id>`).
const IDENTITY_CENTER_APPLICATION_PREFIX: &str = "apl-";
/// The service namespace an Identity Center ARN is spelled under, which is the namespace of
/// both the instance ARN a create names and the application ARN it mints.
const IDENTITY_CENTER_SERVICE: &str = "sso";
/// The resource segment an Identity Center INSTANCE ARN carries
/// (`arn:aws:sso:::instance/<instance id>`), which is what makes the ARN a create names an
/// instance rather than some other Identity Center resource.
pub const IDENTITY_CENTER_INSTANCE_SEGMENT: &str = "instance/";

/// The `ApplicationArn` of the Glue application registered with the Identity Center
/// instance `instance_arn` names.
///
/// An Identity Center application is a resource OF its instance, so its ARN carries the
/// instance's own id before the application id
/// (`arn:aws:sso::<account>:application/<instance id>/apl-<id>`) — the instance id read off
/// the ARN the create named, and the application id minted here from [`mint_hex_id`], so
/// two configurations created in the same nanosecond cannot be given the same application.
/// Minted once, at create, and stored on the entry, so every later read reports the same
/// ARN.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_identity_center_application_arn(instance_arn: &str) -> String {
    let instance_id = identity_center_instance_id(instance_arn).unwrap_or_default();
    let application_id = mint_hex_id(
        &MINTED_IDENTITY_CENTER_APPLICATIONS,
        IDENTITY_CENTER_APPLICATION_ID_WORDS,
    );
    ::engine::emulator_arn(
        IDENTITY_CENTER_SERVICE,
        "",
        &format!(
            "application/{instance_id}/{IDENTITY_CENTER_APPLICATION_PREFIX}{application_id}"
        ),
    )
}

/// The instance id inside an Identity Center INSTANCE ARN
/// (`arn:aws:sso:::instance/ssoins-…`), or `None` when the value names no such instance.
///
/// That is what a create's `InstanceArn` has to be: the ARN of the instance Glue registers
/// its application with, so a value carrying no instance resource segment names no instance
/// to register against.
pub fn identity_center_instance_id(instance_arn: &str) -> Option<&str> {
    let (prefix, instance_id) = instance_arn.split_once(IDENTITY_CENTER_INSTANCE_SEGMENT)?;
    if !prefix.starts_with("arn:") || instance_id.is_empty() || instance_id.contains('/') {
        return None;
    }
    Some(instance_id)
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
        let database = fold_name(database_name);
        let table = fold_name(table_name);
        self.column_statistics_task_runs
            .iter()
            .find(|run| run.is_running_for(catalog_id, &database, &table))
    }

    /// The column statistics task run of this table that has not finished, mutably: the
    /// one `StopColumnStatisticsTaskRun` moves into its stopped state.
    ///
    /// The lookup rule is [`Self::running_column_statistics_task_run`]'s — both ask
    /// [`ColumnStatisticsTaskRunEntry::is_running_for`] — so the run a stop ends is
    /// exactly the run a second start would have been refused for.
    pub fn running_column_statistics_task_run_mut(
        &mut self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&mut ColumnStatisticsTaskRunEntry> {
        let database = fold_name(database_name);
        let table = fold_name(table_name);
        self.column_statistics_task_runs
            .iter_mut()
            .find(|run| run.is_running_for(catalog_id, &database, &table))
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
        if let Some(reason) =
            self.catalog_table_miss(&run.catalog_id, &run.database_name, &run.table_name)
        {
            return RecommendationRunOutcome::NothingToAnalyse(reason);
        }
        let columns = self
            .database(&run.catalog_id, &run.database_name)
            .and_then(|database| database.tables.get(&fold_name(&run.table_name)))
            .map(table_column_names)
            .unwrap_or_default();
        RecommendationRunOutcome::Recommended(columns)
    }

    /// Why the catalog offers a data quality run nothing to read at this address — the
    /// database it does not hold, or the table that database does not hold — or `None` when
    /// it holds the table.
    ///
    /// Every data quality run resolves its data source against the catalog in these same
    /// terms, so the reason a run carries for a data source the catalog holds nothing for is
    /// worded here once rather than once per kind of run.
    pub fn catalog_table_miss(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<String> {
        let Some(database) = self.database(catalog_id, database_name) else {
            return Some(messages::database_not_found(database_name));
        };
        if database.tables.contains_key(&fold_name(table_name)) {
            None
        } else {
            Some(messages::table_not_found(table_name))
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

    /// Give every data quality ruleset evaluation run the state the CLOCK already owed it at
    /// `now`, exactly as [`Self::settle_due_data_quality_rule_recommendation_runs`] does for
    /// a recommendation run.
    ///
    /// What a due run settles into is read off the catalog — whether it still holds every
    /// table the run evaluates — so the outcome of every run still due is resolved first and
    /// applied afterwards: one walk reads the catalog, the next writes the runs.
    pub fn settle_due_data_quality_ruleset_evaluation_runs(&mut self, now: SystemTime) {
        let misses: Vec<(usize, Option<String>)> = self
            .data_quality_ruleset_evaluation_runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.is_due(now))
            .map(|(index, run)| (index, self.evaluated_tables_miss(run)))
            .collect();
        for (index, miss) in misses {
            if let Some(run) = self.data_quality_ruleset_evaluation_runs.get_mut(index) {
                run.settle(miss);
            }
        }
    }

    /// Why the catalog offers `run` nothing to evaluate — the first of its data sources it
    /// holds no table for, in the order the run records them, named as the start spelled it —
    /// or `None` when it holds every one of them.
    fn evaluated_tables_miss(&self, run: &DataQualityRulesetEvaluationRunEntry) -> Option<String> {
        run.evaluated_tables().find_map(|table| {
            self.catalog_table_miss(&table.catalog_id, &table.database_name, &table.table_name)
        })
    }

    /// The stored data quality ruleset evaluation run with this id, or `None` when this
    /// account holds no such run.
    pub fn data_quality_ruleset_evaluation_run(
        &self,
        id: &str,
    ) -> Option<&DataQualityRulesetEvaluationRunEntry> {
        self.data_quality_ruleset_evaluation_runs
            .iter()
            .find(|run| run.id == id)
    }

    /// The evaluation run an earlier start with this `ClientToken` recorded, if one did.
    ///
    /// `StartDataQualityRulesetEvaluationRunRequest$ClientToken` is "used for idempotency
    /// … to avoid creating or starting multiple instances of the same resource", so a start
    /// carrying a token already recorded is the duplicate the service refuses rather than a
    /// second run.
    pub fn data_quality_ruleset_evaluation_run_of_client_token(
        &self,
        client_token: &str,
    ) -> Option<&DataQualityRulesetEvaluationRunEntry> {
        self.data_quality_ruleset_evaluation_runs
            .iter()
            .find(|run| run.client_token.as_deref() == Some(client_token))
    }

    /// Append a data quality ruleset evaluation run, keeping at most
    /// [`MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS`] of them.
    pub fn record_data_quality_ruleset_evaluation_run(
        &mut self,
        run: DataQualityRulesetEvaluationRunEntry,
    ) {
        self.data_quality_ruleset_evaluation_runs.push(run);
        if self.data_quality_ruleset_evaluation_runs.len()
            > MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS
        {
            let overflow = self.data_quality_ruleset_evaluation_runs.len()
                - MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS;
            self.data_quality_ruleset_evaluation_runs.drain(..overflow);
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

    /// The catalog RESOURCE a `CreateCatalog` registered under `reference`, or `None`
    /// when this store holds no catalog of that name in that account.
    ///
    /// A catalog is keyed by its folded name ([`fold_name`]), so a read reaches it under
    /// any casing. The account's own Data Catalog is not one of these records — it is a
    /// container nothing created ([`Catalog::definition`]) — so an id naming it resolves
    /// to nothing here.
    pub fn catalog_definition(&self, reference: &CatalogReference) -> Option<&CatalogDefinition> {
        self.catalogs
            .get(&child_catalog_id(
                &reference.account_id,
                &fold_name(&reference.name),
            ))
            .and_then(|catalog| catalog.definition.as_ref())
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

    /// Whether the store can hold another blueprint, i.e. is below [`MAX_BLUEPRINTS`].
    pub fn can_store_blueprint(&self) -> bool {
        self.blueprints.len() < MAX_BLUEPRINTS
    }

    /// Register `entry` under its own name, which is the key every blueprint operation
    /// addresses a blueprint by.
    pub fn insert_blueprint(&mut self, entry: BlueprintEntry) {
        self.blueprints.insert(entry.name.clone(), entry);
    }

    /// The stored blueprint `name`, or `None` when this account holds no such blueprint.
    pub fn blueprint(&self, name: &str) -> Option<&BlueprintEntry> {
        self.blueprints.get(name)
    }

    /// [`Self::blueprint`] mutably: the one record every write to a blueprint lands in.
    pub fn blueprint_mut(&mut self, name: &str) -> Option<&mut BlueprintEntry> {
        self.blueprints.get_mut(name)
    }

    /// Remove the blueprint `name`, answering the record that was removed, or `None` when
    /// this account holds no blueprint under that name.
    ///
    /// The whole record goes: a blueprint's tags and the runs started of it are held ON the
    /// entry ([`BlueprintEntry::tags`], [`BlueprintEntry::runs`]), so the name a delete
    /// frees carries neither into the next create under it. The workflows those runs laid
    /// out are separate records of this store and are left alone, because a workflow
    /// outlives the run that created it. Matched by name exactly as the read path matches
    /// it ([`Self::blueprint`]), and the removed entry is handed back rather than dropped
    /// for symmetry with [`Self::remove_job`].
    pub fn remove_blueprint(&mut self, name: &str) -> Option<BlueprintEntry> {
        self.blueprints.remove(name)
    }

    /// Whether the store can hold another interactive session, i.e. is below
    /// [`MAX_SESSIONS`].
    pub fn can_store_session(&self) -> bool {
        self.stored_sessions() < MAX_SESSIONS
    }

    /// How many interactive sessions this store holds across every Region. The walk is over
    /// the Regions one process has been addressed in, so it is bounded.
    pub fn stored_sessions(&self) -> usize {
        self.sessions.values().map(BTreeMap::len).sum()
    }

    /// The session `id` of the Region `region`, or `None` when that Region holds no session
    /// under that id — which is every session of every other Region, because a session is
    /// served only through the endpoint that created it.
    pub fn session(&self, region: &str, id: &str) -> Option<&SessionEntry> {
        self.sessions
            .get(region)
            .and_then(|sessions| sessions.get(id))
    }

    /// [`Self::session`] mutably: the one record every write to a session lands in, so the
    /// operations that read, stop and delete a session reach the record its reads report
    /// rather than a copy of it.
    pub fn session_mut(&mut self, region: &str, id: &str) -> Option<&mut SessionEntry> {
        self.sessions
            .get_mut(region)
            .and_then(|sessions| sessions.get_mut(id))
    }

    /// Store `entry` as the Region `region`'s session under its own id.
    pub fn insert_session(&mut self, region: &str, entry: SessionEntry) {
        self.sessions
            .entry(region.to_string())
            .or_default()
            .insert(entry.id.clone(), entry);
    }

    /// Start a run of the blueprint `input` names at `now` and answer the id minted for that
    /// run, or why no run could be started.
    ///
    /// The ONE place a blueprint run is started, so a run and the workflow it produced are
    /// written together or not at all. In order: the blueprint is resolved, the registration
    /// status the CLOCK already owed it is materialised, a run is refused of a registration
    /// that has not finished, the workflow the run lays out is created, and the run of it is
    /// recorded carrying what that layout came to.
    ///
    /// "The name of a workflow that is created as a result of a successful blueprint run"
    /// (`BlueprintRun$WorkflowName`) is what a run produces, and the workflow it creates
    /// carries the run that created it ("The details of the blueprint that this particular
    /// workflow is created from", `Workflow$BlueprintDetails`). A name this account already
    /// holds a workflow under is NOT overwritten: that run creates nothing and fails, the
    /// way any blueprint run that cannot create its entities does.
    pub fn start_blueprint_run(
        &mut self,
        input: &StartBlueprintRunInput,
        now: SystemTime,
    ) -> Result<String, BlueprintRunNotStarted> {
        // Whether the store has room for the workflow the run would create, read before the
        // two maps below are borrowed apart.
        let can_store_workflow = self.can_store_workflow();
        // The blueprint and the workflows are two different maps of this store, borrowed
        // apart here so the run and the workflow it lays out are written in ONE pass rather
        // than by resolving the blueprint a second time.
        let Self {
            blueprints,
            workflows,
            ..
        } = self;
        let Some(blueprint) = blueprints.get_mut(&input.blueprint_name) else {
            return Err(BlueprintRunNotStarted::NoSuchBlueprint);
        };
        // The status the clock had already made due, materialised before it is read: the
        // registration was owed its terminal status at its due instant whether or not
        // anybody called.
        blueprint.settle_if_due(now);
        if !blueprint.is_active() {
            return Err(BlueprintRunNotStarted::NoActiveDefinition);
        }

        let workflow_name =
            blueprint_run_workflow_name(&blueprint.name, input.parameters.as_deref());
        let run_id = new_blueprint_run_id();
        let layout = if workflows.contains_key(&workflow_name) {
            BlueprintRunLayout::WorkflowAlreadyExists(workflow_name)
        } else if can_store_workflow {
            insert_workflow_into(
                workflows,
                WorkflowEntry::from_blueprint_run(&workflow_name, &blueprint.name, &run_id, now),
            );
            BlueprintRunLayout::Created(workflow_name)
        } else {
            return Err(BlueprintRunNotStarted::WorkflowCeilingReached(MAX_WORKFLOWS));
        };

        blueprint.begin_run(input, run_id.clone(), now, &layout);
        Ok(run_id)
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
    ///
    /// `run` is the workflow run the graph is reported BESIDE, whose job runs each `JOB`
    /// node reports as "the information for the job runs represented by the job node"
    /// (`JobNodeDetails$JobRuns`), and `None` for a read that reports no run at all.
    pub fn workflow_graph(
        &self,
        region: &str,
        workflow_name: &str,
        run: Option<&WorkflowRunEntry>,
    ) -> WorkflowGraph {
        let mut graph = WorkflowGraphBuilder::default();
        let attached = self
            .triggers
            .get(region)
            .into_iter()
            .flat_map(BTreeMap::values)
            .filter(|trigger| trigger.workflow_name.as_deref() == Some(workflow_name));
        for trigger in attached {
            let trigger_id = graph.node(NODE_TYPE_TRIGGER, &trigger.name, Some(trigger), &[]);
            for action in &trigger.actions {
                for (node_type, name) in
                    named_components(action.job_name.as_deref(), action.crawler_name.as_deref())
                {
                    let runs = self.component_job_runs(region, run, node_type, name);
                    let initiated = graph.node(node_type, name, None, &runs);
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
                    let runs = self.component_job_runs(region, run, node_type, name);
                    let waited_on = graph.node(node_type, name, None, &runs);
                    graph.edge(&waited_on, &trigger_id);
                }
            }
        }
        graph.build()
    }

    /// The runs of the workflow run `run` that belong to the graph node for the component
    /// `name` of type `node_type`, as the modelled `JobRun` shape.
    ///
    /// Only a `JOB` node has job runs to report, and only the runs THIS workflow run began
    /// are its: a run of the same job that another workflow run — or a caller's own
    /// `StartJobRun` — began is not part of this run. The runs are read out of the job
    /// definition they belong to ([`JobEntry::runs`]), so a node reports the state each run
    /// has reached by now rather than the state it was begun in, and they are taken in the
    /// order the actions initiated them, so two reads of an unchanged store agree.
    fn component_job_runs(
        &self,
        region: &str,
        run: Option<&WorkflowRunEntry>,
        node_type: &str,
        name: &str,
    ) -> Vec<JobRun> {
        if node_type != NODE_TYPE_JOB {
            return Vec::new();
        }
        let Some(run) = run else {
            return Vec::new();
        };
        let Some(job) = self.job(region, name) else {
            return Vec::new();
        };
        run.action_job_runs
            .iter()
            .filter(|begun| begun.job_name == name)
            .filter_map(|begun| job.run(&begun.run_id))
            .map(JobRunEntry::to_job_run)
            .collect()
    }

    /// `WorkflowRun$Statistics`: "Workflow run statistics provides statistics about the
    /// workflow run."
    ///
    /// "Total number of Actions in the workflow run" is what the entry point that began the
    /// run initiates, which the run itself records ([`WorkflowRunEntry::total_actions`]).
    /// Every other count is of the run's actions that are IN one state, so each is taken
    /// from the state the job run that action began has reached by now — the same records
    /// the graph's `JOB` nodes report — rather than from anything stored on the run, so a
    /// later read reports the counts the clock has since moved.
    ///
    /// An action whose job run the store no longer holds, and a terminal `JobRunState` the
    /// shape has no counter for, are counted in none of them.
    pub fn workflow_run_statistics(
        &self,
        region: &str,
        run: &WorkflowRunEntry,
    ) -> WorkflowRunStatistics {
        let mut statistics = WorkflowRunStatistics {
            total_actions: run.total_actions,
            ..WorkflowRunStatistics::default()
        };
        for begun in &run.action_job_runs {
            let Some(state) = self
                .job(region, &begun.job_name)
                .and_then(|job| job.run(&begun.run_id))
                .map(|job_run| job_run.state.as_str())
            else {
                continue;
            };
            let counted = match state {
                JOB_RUN_STATE_SUCCEEDED => &mut statistics.succeeded_actions,
                JOB_RUN_STATE_FAILED => &mut statistics.failed_actions,
                JOB_RUN_STATE_TIMEOUT => &mut statistics.timeout_actions,
                JOB_RUN_STATE_STOPPED => &mut statistics.stopped_actions,
                JOB_RUN_STATE_ERROR => &mut statistics.errored_actions,
                JOB_RUN_STATE_WAITING => &mut statistics.waiting_actions,
                // "Total number Actions in running state": a run that has not finished and
                // is not waiting for a slot is consuming resources.
                other if !TERMINAL_JOB_RUN_STATES.contains(&other) => {
                    &mut statistics.running_actions
                }
                _ => continue,
            };
            *counted = counted.saturating_add(1);
        }
        statistics
    }

    /// The modelled `Workflow` for the stored workflow `workflow`, read in `region`.
    ///
    /// `include_graph` is the reader's own `IncludeGraph`; the graph is reported beside
    /// "the information about the last execution of the workflow" (`Workflow$LastRun`), so
    /// that run is the one its `JOB` nodes report the job runs of.
    pub fn to_workflow(
        &self,
        region: &str,
        workflow: &WorkflowEntry,
        include_graph: bool,
    ) -> Workflow {
        let last_run = workflow.runs.last();
        let graph =
            include_graph.then(|| self.workflow_graph(region, &workflow.name, last_run));
        workflow.to_workflow(
            graph,
            last_run.map(|run| {
                run.to_workflow_run(None, self.workflow_run_statistics(region, run))
            }),
        )
    }

    /// The modelled `WorkflowRun` for the stored run `run`, read in `region`: the ONE
    /// projection every read of a workflow run serves, so the single-run read and the run
    /// listing report a run identically.
    ///
    /// `include_graph` is the reader's own `IncludeGraph`, and the graph it asks for is the
    /// graph of THIS run — its `JOB` nodes report the runs this run's actions began.
    pub fn to_workflow_run(
        &self,
        region: &str,
        run: &WorkflowRunEntry,
        include_graph: bool,
    ) -> WorkflowRun {
        let graph =
            include_graph.then(|| self.workflow_graph(region, &run.workflow_name, Some(run)));
        run.to_workflow_run(graph, self.workflow_run_statistics(region, run))
    }

    /// Give the job runs the workflow `workflow_name`'s runs began the state the CLOCK
    /// already owed them at `now`.
    ///
    /// A read of one of those workflow runs reports the state each of its actions has
    /// reached — under the `JOB` nodes of its graph and in its statistics — and that state
    /// was due at the job run's own settle instant whether or not anybody called, so the
    /// read materialises it exactly as it materialises the statuses the workflow's own runs
    /// were owed ([`WorkflowEntry::settle_due_runs`]). Nothing else about those runs moves.
    ///
    /// The walk is over the jobs this workflow's runs named, bounded by
    /// [`MAX_RUNS_PER_WORKFLOW`] runs each naming the actions of one trigger.
    pub fn settle_due_action_job_runs(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) {
        let named: BTreeSet<String> = self
            .workflow(workflow_name)
            .into_iter()
            .flat_map(|workflow| workflow.runs.iter())
            .flat_map(|run| run.action_job_runs.iter())
            .map(|begun| begun.job_name.clone())
            .collect();
        for job_name in named {
            if let Some(job) = self.job_mut(region, &job_name) {
                job.settle_due_runs(now);
            }
        }
    }

    /// Record on the run `run_id` of the workflow `workflow_name` the job runs `begun` that
    /// its actions started, so a later read of that run reports them.
    ///
    /// The association lives on the RUN because a job run is addressed by its job and its
    /// own id and carries no workflow run of its own: without it, two runs of the same
    /// workflow could not be told apart by what each initiated.
    pub fn record_workflow_run_job_runs(
        &mut self,
        workflow_name: &str,
        run_id: &str,
        begun: &[ActionJobRun],
    ) {
        if begun.is_empty() {
            return;
        }
        if let Some(run) = self
            .workflow_mut(workflow_name)
            .and_then(|workflow| workflow.run_mut(run_id))
        {
            run.action_job_runs.extend(begun.iter().cloned());
        }
    }

    /// Store `entry` as this account's workflow under its own name, which is the key every
    /// read addresses it by ([`Self::workflow`]) — so the namespace's key form lives in
    /// [`insert_workflow_into`] rather than in each handler that writes to it.
    pub fn insert_workflow(&mut self, entry: WorkflowEntry) {
        insert_workflow_into(&mut self.workflows, entry);
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

    /// Remove the job definition `name` from the Region `region`, answering the record
    /// that was removed, or `None` when that Region holds no job under that name.
    ///
    /// The whole record goes: a job's tags and the runs started of it are held ON the
    /// entry ([`JobEntry::tags`], [`JobEntry::runs`]), so the name a delete frees carries
    /// neither into the next create under it. The removed entry is handed back rather
    /// than dropped, for symmetry with [`Self::remove_connection`] — today's caller only
    /// needs to know WHETHER a record went. Matched by name and Region exactly as the
    /// read path matches them ([`Self::job`]).
    pub fn remove_job(&mut self, region: &str, name: &str) -> Option<JobEntry> {
        self.jobs
            .get_mut(region)
            .and_then(|jobs| jobs.remove(name))
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

    /// [`Self::trigger`] mutably: where every write to a stored trigger lands, so the
    /// operations that start, stop and update a trigger reach the one record its reads
    /// report rather than a copy of it.
    pub fn trigger_mut(&mut self, region: &str, name: &str) -> Option<&mut TriggerEntry> {
        self.triggers
            .get_mut(region)
            .and_then(|triggers| triggers.get_mut(name))
    }

    /// Every trigger the Region `region` holds, in name order — the triggers served
    /// through this endpoint, which is every trigger reads of this Region can address.
    ///
    /// The order is the inner map's own key order, so a listing that walks this set pages
    /// deterministically across calls.
    pub fn triggers_in(&self, region: &str) -> Vec<&TriggerEntry> {
        self.triggers
            .get(region)
            .into_iter()
            .flat_map(BTreeMap::values)
            .collect()
    }

    /// Store `entry` as the Region `region`'s trigger under its own name.
    pub fn insert_trigger(&mut self, region: &str, entry: TriggerEntry) {
        self.triggers
            .entry(region.to_string())
            .or_default()
            .insert(entry.name.clone(), entry);
    }

    /// Initiate `actions` — the actions of the trigger `trigger_name`, which has just FIRED
    /// in the Region `region` — and answer every job run they began, in action order.
    ///
    /// "Defines an action to be initiated by a trigger" (`Action`): an action naming a job
    /// begins a RUN of that job, carrying the overrides the action names over the job's own
    /// definition — "The job arguments used when this trigger fires. For this job run, they
    /// replace the default arguments set in the job definition itself" (`Action$Arguments`),
    /// and the same for its `Timeout`, `SecurityConfiguration` and `NotificationProperty` —
    /// so what a firing begins is the record [`JobRunEntry::start`] builds for a direct
    /// start of that job, named as the run of a trigger (`JobRun$TriggerName`), and a later
    /// read of the job's runs reports it.
    ///
    /// The ceiling a direct start respects bounds a firing too: a job already at its
    /// `ExecutionProperty$MaxConcurrentRuns` takes no further run, and because the firing is
    /// not the caller's own start of that job, the firing begins the runs it can rather than
    /// being refused. An action naming a job this Region no longer holds begins no run for
    /// the same reason.
    ///
    /// An action's `CrawlerName` initiates a CRAWL, which is the crawl lifecycle
    /// `StartCrawler` owns and which this crate does not hold yet ([`CrawlerEntry`] records
    /// no crawl), so such an action begins nothing here.
    pub fn initiate_trigger_actions(
        &mut self,
        region: &str,
        trigger_name: &str,
        actions: &[Action],
        now: SystemTime,
    ) -> Vec<ActionJobRun> {
        let mut begun = Vec::new();
        for action in actions {
            let Some(job_name) = action.job_name.as_deref() else {
                continue;
            };
            let request = job_run_request_for_action(action, job_name);
            let Some(job) = self.job_mut(region, job_name) else {
                continue;
            };
            // Everything the clock already owed this job's runs is materialised before they
            // are counted, so a run it has already finished does not hold a slot.
            job.settle_due_runs(now);
            let allowed = usize::try_from(job.max_concurrent_runs()).unwrap_or(usize::MAX);
            if job.runs_in_flight() >= allowed {
                continue;
            }
            let run_id = new_job_run_id();
            let run = JobRunEntry::start(
                job,
                &request,
                run_id.clone(),
                now,
                None,
                Some(trigger_name),
            );
            job.record_run(run);
            begun.push(ActionJobRun {
                job_name: job_name.to_string(),
                run_id,
            });
        }
        begun
    }

    /// The EVENT trigger of the Region `region` that the events delivered to the workflow
    /// `workflow_name` are counted against, or `None` when that workflow has none.
    ///
    /// An EVENT trigger is the one kind driven from outside this service: the events an
    /// EventBridge rule hands a workflow are counted against the `EventBatchingCondition`
    /// of the EVENT trigger that belongs to it ("number of events that must be received
    /// from Amazon EventBridge before EventBridge event trigger fires",
    /// `EventBatchingCondition$BatchSize`), so this is the trigger an ingress resolves. A
    /// trigger is served only through the endpoint that created it, so the search is over
    /// THIS Region's triggers, bounded by [`MAX_TRIGGERS`] and taken in name order (the map
    /// is ordered), so one workflow resolves to the same trigger on every call — the same
    /// way [`Self::starting_trigger`] resolves an entry point.
    pub fn event_trigger(&self, region: &str, workflow_name: &str) -> Option<&TriggerEntry> {
        self.triggers.get(region)?.values().find(|trigger| {
            trigger.workflow_name.as_deref() == Some(workflow_name)
                && trigger.trigger_type == TRIGGER_TYPE_EVENT
        })
    }

    /// Count ONE event EventBridge delivered to the workflow `workflow_name` against that
    /// workflow's EVENT trigger, and answer what became of the delivery.
    ///
    /// The ingress AWS exposes as `glue:notifyEvent`: an EventBridge rule whose target is a
    /// Glue workflow hands every matched event to that workflow, and the workflow's EVENT
    /// trigger is what turns those events into runs — "batch condition that must be met
    /// (specified number of events received or batch time window expired) before
    /// EventBridge event trigger fires" (`EventBatchingCondition`). So one delivery is
    /// counted ([`TriggerEntry::receive_event`]) and the trigger fires EXACTLY ONCE when
    /// the count reaches its `BatchSize`, beginning one run of the workflow and initiating
    /// that trigger's actions — the same firing [`Self::fire_event_trigger`] performs for a
    /// window that ran out.
    ///
    /// Before the event is counted, a batch whose window the clock has already run out
    /// fires on its own, so an event arriving after the window belongs to the next batch
    /// rather than to the one it closed.
    pub fn notify_workflow_event(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) -> WorkflowEventDelivery {
        if self.workflow(workflow_name).is_none() {
            return WorkflowEventDelivery::NoWorkflow;
        }
        self.settle_due_event_batches(region, workflow_name, now);
        let Some(trigger) = self.event_trigger(region, workflow_name) else {
            return WorkflowEventDelivery::NoEventTrigger;
        };
        if !trigger.batches_events() {
            return WorkflowEventDelivery::Deactivated;
        }
        let trigger_name = trigger.name.clone();
        let fires = self
            .trigger_mut(region, &trigger_name)
            .is_some_and(|trigger| trigger.receive_event(now));
        if !fires {
            let (received, batch_size) = self
                .trigger(region, &trigger_name)
                .map_or((0, 0), |trigger| {
                    (trigger.batched_events, trigger.event_batch_size())
                });
            return WorkflowEventDelivery::Counted {
                received,
                batch_size,
            };
        }
        self.fire_event_trigger(region, &trigger_name, now)
    }

    /// Fire the batch of the EVENT trigger of the workflow `workflow_name` that the CLOCK
    /// has already run the window out on, at `now`.
    ///
    /// "Window of time in seconds after which EventBridge event trigger fires. Window
    /// starts when first event is received" (`EventBatchingCondition$BatchWindow`): a batch
    /// that has received at least one event and whose window has run out fires with the
    /// events it holds, and it was owed that firing at the instant the window ran out
    /// whether or not anybody called — so a read of the workflow's runs materialises it
    /// exactly as it materialises the statuses those runs were owed
    /// ([`WorkflowEntry::settle_due_runs`]), and the run begins at the DUE instant rather
    /// than at the instant of the read. Nothing else moves, and a batch still inside its
    /// window fires nothing.
    pub fn settle_due_event_batches(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) {
        let Some(trigger) = self.event_trigger(region, workflow_name) else {
            return;
        };
        if !trigger.batches_events() {
            return;
        }
        let trigger_name = trigger.name.clone();
        let Some(due) = trigger
            .batch_window_due_at()
            .filter(|due| now >= *due)
        else {
            return;
        };
        if let Some(trigger) = self.trigger_mut(region, &trigger_name) {
            trigger.close_event_batch();
        }
        self.fire_event_trigger(region, &trigger_name, due);
    }

    /// Begin the one run of its workflow that the trigger `trigger_name` of `region` FIRING
    /// starts, at `now`, and answer what became of it.
    ///
    /// A firing begins a run of the workflow the trigger belongs to and initiates "the
    /// actions initiated by this trigger" (`Trigger$Actions`) inside it, which is what
    /// `StartWorkflowRun` does for the entry point a caller starts: the run is recorded
    /// through the ONE place a run of a workflow is recorded
    /// ([`WorkflowEntry::begin_run`]), so a run a trigger began is counted against the same
    /// concurrency ceiling as one a caller started and reported identically by every later
    /// read.
    fn fire_event_trigger(
        &mut self,
        region: &str,
        trigger_name: &str,
        now: SystemTime,
    ) -> WorkflowEventDelivery {
        // The workflow the firing runs and the actions it initiates, resolved before the
        // workflow is borrowed to write to.
        let Some((workflow_name, actions)) = self.trigger(region, trigger_name).and_then(|trigger| {
            trigger
                .workflow_name
                .clone()
                .map(|workflow_name| (workflow_name, trigger.actions.clone()))
        }) else {
            return WorkflowEventDelivery::NoWorkflow;
        };
        let total_actions = i32::try_from(actions.len()).unwrap_or(i32::MAX);
        let Some(workflow) = self.workflow_mut(&workflow_name) else {
            return WorkflowEventDelivery::NoWorkflow;
        };
        // A firing passes no run properties of its own, so the run carries the workflow's
        // own defaults.
        let run_id = match workflow.begin_run(None, now, total_actions) {
            Ok(run_id) => run_id,
            // "If you leave this parameter blank, there is no limit to the number of
            // concurrent workflow runs": past a ceiling the workflow named, the firing
            // begins no run, exactly as a caller's start is refused past it.
            Err(ceiling) => return WorkflowEventDelivery::ConcurrentRunsExceeded { ceiling },
        };
        let begun_job_runs = self.initiate_trigger_actions(region, trigger_name, &actions, now);
        self.record_workflow_run_job_runs(&workflow_name, &run_id, &begun_job_runs);
        ::tracing::info!(
            trigger = %trigger_name,
            workflow = %workflow_name,
            run = %run_id,
            job_runs = begun_job_runs.len(),
            "a Glue EVENT trigger's batch fired and began a workflow run"
        );
        WorkflowEventDelivery::Fired { run_id }
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

    /// The stored schema set `schema_name` of registry `registry_name`, mutably.
    pub fn schema_mut(
        &mut self,
        registry_name: &str,
        schema_name: &str,
    ) -> Option<&mut SchemaEntry> {
        self.registries
            .get_mut(registry_name)
            .and_then(|registry| registry.schemas.get_mut(schema_name))
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

    /// Whether the store can hold another data quality ruleset, i.e. is below
    /// [`MAX_DATA_QUALITY_RULESETS`].
    pub fn can_store_data_quality_ruleset(&self) -> bool {
        self.data_quality_rulesets.len() < MAX_DATA_QUALITY_RULESETS
    }

    /// The integration resource property record stored for `resource_arn`, if this account
    /// holds one.
    ///
    /// The ONE resolution every read of such a record goes through, so an ARN names the
    /// same record whichever operation resolved it. The ARN is matched exactly as sent: the
    /// model documents no fold for `ResourceArn`.
    pub fn integration_resource_property(
        &self,
        resource_arn: &str,
    ) -> Option<&IntegrationResourcePropertyEntry> {
        self.integration_resource_properties.get(resource_arn)
    }

    /// Whether the store can hold another integration resource property record, i.e. is
    /// below [`MAX_INTEGRATION_RESOURCE_PROPERTIES`].
    pub fn can_store_integration_resource_property(&self) -> bool {
        self.integration_resource_properties.len() < MAX_INTEGRATION_RESOURCE_PROPERTIES
    }

    /// The integration table property record stored for `resource_arn` and `table_name`, if
    /// this account holds one for that pair.
    ///
    /// The pair is the record's whole address, so the lookup is the key lookup. One copy,
    /// shared, so the create and every read of one record cannot disagree about which
    /// record a request names.
    pub fn integration_table_properties(
        &self,
        resource_arn: &str,
        table_name: &str,
    ) -> Option<&IntegrationTablePropertiesEntry> {
        self.integration_table_properties
            .get(&(resource_arn.to_string(), table_name.to_string()))
    }

    /// Whether the store can hold another integration table property record, i.e. is below
    /// [`MAX_INTEGRATION_TABLE_PROPERTIES`].
    pub fn can_store_integration_table_properties(&self) -> bool {
        self.integration_table_properties.len() < MAX_INTEGRATION_TABLE_PROPERTIES
    }

    /// Whether this account holds ANY integration table property record for `resource_arn`,
    /// whatever table each one overrides.
    ///
    /// This is what tells the miss of ONE pair apart from a resource that carries no
    /// integration table properties at all, which the two operations that address a record
    /// report differently ([`Self::integration_table_properties_miss`]).
    pub fn holds_integration_table_properties_of(&self, resource_arn: &str) -> bool {
        self.integration_table_properties
            .keys()
            .any(|(arn, _)| arn == resource_arn)
    }

    /// The message an operation that addresses ONE integration table property record
    /// (`GetIntegrationTableProperties`, `DeleteIntegrationTableProperties`) carries when it
    /// reaches no record for the pair it was asked about.
    ///
    /// RECORDED behaviour: a resource carrying no integration table properties at all is
    /// reported as the resource itself not being found ([`messages::target_resource_not_found`]),
    /// while a resource that carries properties for other tables answers with the miss of the
    /// pair the request addressed ([`messages::integration_table_properties_not_found`]). The
    /// ONE reading of that rule, so the read and the delete cannot disagree about which miss
    /// a request earns.
    pub fn integration_table_properties_miss(&self, resource_arn: &str, table_name: &str) -> String {
        if self.holds_integration_table_properties_of(resource_arn) {
            messages::integration_table_properties_not_found(resource_arn, table_name)
        } else {
            messages::target_resource_not_found(resource_arn)
        }
    }

    /// Whether this store holds the resource an integration table property call's
    /// `ResourceArn` named ([`integration_table_reference`]).
    ///
    /// The properties belong to that resource, so it is the resource that has to exist —
    /// for a connection or a database this is the same question a tagging call asks
    /// ([`Self::resource_tags`]), and for a table ARN it is the table itself, inside the
    /// database its ARN names.
    pub fn holds_integration_table_resource(&self, resource: &IntegrationTableResource) -> bool {
        match resource {
            IntegrationTableResource::Tagged(tagged) => self.resource_tags(tagged).is_some(),
            IntegrationTableResource::Table {
                account_id,
                database,
                table,
            } => self
                .database(account_id, database)
                .is_some_and(|database| database.tables.contains_key(table)),
        }
    }

    /// Whether this store has room for another classifier, bounded by
    /// [`MAX_CLASSIFIERS`].
    pub fn can_store_classifier(&self) -> bool {
        self.classifiers.len() < MAX_CLASSIFIERS
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

    /// The machine learning transform stored under `name`, if this account holds one.
    ///
    /// A `Name` is not a transform's ADDRESS — every operation that addresses one takes
    /// the minted `TransformId` — but it IS the name the client chooses at create time, so
    /// a create resolves it to refuse a name this account already holds a transform under.
    /// At most one transform is ever stored under one name, because such a create is
    /// refused. Bounded by [`MAX_ML_TRANSFORMS`].
    pub fn ml_transform_by_name(&self, name: &str) -> Option<&MlTransformEntry> {
        self.ml_transforms
            .values()
            .find(|transform| transform.name == name)
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

    /// The stored form type an `Identifier` targeting `FormTypeId` names — a
    /// `GetFormType$Identifier`, a `DeleteFormType$Identifier` or an
    /// `AssetTypeFormReference$FormTypeIdentifier` — or `None` when this store holds no
    /// such form type.
    ///
    /// A form type's identifier is its name ([`FormTypeEntry::id`]), so the resolution is
    /// this map's own lookup, exact as sent. One copy, shared, so no two operations can
    /// disagree about which record an identifier names.
    pub fn form_type_by_reference(&self, identifier: &str) -> Option<&FormTypeEntry> {
        self.form_types.get(identifier)
    }

    /// Remove the form type an `Identifier` targeting `FormTypeId` names, reporting the
    /// record that was removed, or `None` when this store holds no such form type.
    ///
    /// The identifier resolves exactly as a read resolves one, i.e. as this namespace's
    /// own key ([`Self::form_type_by_reference`]), so a delete reaches the record a get of
    /// the same identifier would report. Only the form type itself is removed: an asset
    /// type's form references are written by that asset type's own puts and are left as
    /// they stand.
    pub fn remove_form_type_by_reference(&mut self, identifier: &str) -> Option<FormTypeEntry> {
        self.form_types.remove(identifier)
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

    /// Remove the asset type an `Identifier` targeting `AssetTypeId` names, reporting the
    /// record that was removed, or `None` when this store holds no such asset type.
    ///
    /// The identifier resolves exactly as a read resolves one
    /// ([`Self::asset_type_by_reference`]), so a delete reaches the record a get of the
    /// same identifier would report. Only the asset type record is removed: the assets
    /// that are instances of it record the identifier their own puts sent and are left as
    /// they stand.
    pub fn remove_asset_type_by_reference(&mut self, identifier: &str) -> Option<AssetTypeEntry> {
        let name = self.asset_type_by_reference(identifier)?.name.clone();
        self.asset_types.remove(&name)
    }

    /// Whether the store can hold another asset, i.e. is below [`MAX_ASSETS`].
    pub fn can_store_asset(&self) -> bool {
        self.assets.len() < MAX_ASSETS
    }

    /// The stored asset an `AssetId` names, or `None` when this account holds no asset
    /// under that identifier.
    ///
    /// Every operation that addresses an asset does so by that identifier, which is this
    /// namespace's own key, so the resolution is this map's lookup, exact as sent. One copy,
    /// shared, so no two operations can disagree about which record an identifier names.
    pub fn asset(&self, identifier: &str) -> Option<&AssetEntry> {
        self.assets.get(identifier)
    }

    /// The stored asset an `AssetId` names, mutably.
    pub fn asset_mut(&mut self, identifier: &str) -> Option<&mut AssetEntry> {
        self.assets.get_mut(identifier)
    }

    /// Remove the asset an `AssetId` names, reporting the record that was removed, or
    /// `None` when this account holds no asset under that identifier.
    ///
    /// The identifier resolves exactly as a read resolves one ([`Self::asset`]) — it is
    /// this namespace's own key — so a delete reaches the record a get of the same
    /// identifier would report. The asset's own record is all that is removed: the asset
    /// type it was an instance of and the glossary terms its associations named are their
    /// own records in their own namespaces.
    pub fn remove_asset(&mut self, identifier: &str) -> Option<AssetEntry> {
        self.assets.remove(identifier)
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

    /// Remove the glossary whose `Id` is `id`, reporting the record removed and `None` when
    /// this account holds none under that id.
    ///
    /// The one place a glossary leaves the namespace, resolving the identifier the way
    /// [`Self::glossary`] does — the `GlossaryId` is the whole address — so the delete and
    /// the reads cannot disagree about which record an identifier names. A glossary's terms
    /// are their own records in their own namespace, so this removes none of them.
    pub fn remove_glossary(&mut self, id: &str) -> Option<GlossaryEntry> {
        self.glossaries.remove(id)
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

    /// Remove the glossary term whose `Id` is `id`, reporting the record removed and
    /// `None` when this account holds none under that id.
    ///
    /// The one place a term leaves the namespace, resolving the identifier the way
    /// [`Self::glossary_term`] does — the `GlossaryTermId` is the whole address — so the
    /// delete and the reads cannot disagree about which record an identifier names. Only
    /// the term record is removed: an asset an association ALREADY put this id on keeps it
    /// ([`AssetEntry::glossary_terms`]), because the delete rewrites no other namespace's
    /// records. A NEW association naming the removed id is a different question and is
    /// refused, because an association resolves the ids it names against this namespace
    /// first ([`Self::first_unheld_glossary_term`]).
    pub fn remove_glossary_term(&mut self, id: &str) -> Option<GlossaryTermEntry> {
        self.glossary_terms.remove(id)
    }

    /// The first of `identifiers` this store holds no glossary term under, in the order the
    /// request named them, and `None` when every one of them resolves to a live term.
    ///
    /// This is how an operation that writes a REFERENCE to a glossary term resolves the
    /// references it was given before it writes any of them: each id is resolved the one way
    /// every operation addressing a term resolves one ([`Self::glossary_term`]), so a term
    /// that was never created and a term that has since been deleted are the same miss here.
    /// The FIRST miss is reported, in request order, so a refusal that names one id names
    /// the same id however many of the request's ids miss.
    pub fn first_unheld_glossary_term<'ids>(
        &self,
        identifiers: &'ids [String],
    ) -> Option<&'ids str> {
        identifiers
            .iter()
            .map(String::as_str)
            .find(|identifier| self.glossary_term(identifier).is_none())
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
    /// The catalog RESOURCE record a `CreateCatalog` wrote for this catalog id, or
    /// `None` for a catalog that is only a container — the account's own Data Catalog,
    /// which no create brought into existence and which therefore has no name,
    /// description or federation of its own.
    ///
    /// It sits on the container rather than in a map of its own so that one catalog is
    /// ONE record: the databases and connections stored under a catalog id and the
    /// metadata of the catalog resource under that same id are read and written through
    /// the same entry.
    pub definition: Option<CatalogDefinition>,
}

impl Catalog {
    /// The stored databases ordered by folded name — the deterministic order
    /// `GetDatabases` pages over, the same order [`DatabaseEntry::tables_ordered`] gives
    /// the tables of one database.
    pub fn databases_ordered(&self) -> Vec<&DatabaseEntry> {
        let mut databases: Vec<&DatabaseEntry> = self.databases.values().collect();
        databases.sort_by(|left, right| left.name.cmp(&right.name));
        databases
    }

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

/// The catalog RESOURCE a `CreateCatalog` registered: "a logical grouping of databases in
/// the Glue Data Catalog or a federated source" (`Catalog`), stored under the catalog id
/// it answers to.
///
/// Every member of the modelled `CatalogInput` is held, so a later read reports the
/// definition the client wrote and nothing the server invented, plus the three members
/// the SERVER sets: the two instants the write stamped and the id the catalog is
/// addressed by. The create's `Tags` are held too, as `CatalogInput` does not carry them.
#[derive(Debug, Clone)]
pub struct CatalogDefinition {
    /// `Catalog$Name`: "the name of the catalog", exactly as the create spelled it. The
    /// catalog is KEYED by the folded name ([`fold_name`]), so a read reaches it under
    /// any casing ("This should be all lowercase" — `GetCatalog`), while the reported
    /// name is the one the client chose.
    pub name: String,
    /// `Catalog$CatalogId`: "the ID of the catalog" — the id every other operation
    /// addresses this catalog by, which for a catalog created inside an account's own
    /// Data Catalog is that account's id and the catalog's name
    /// ([`child_catalog_id`]).
    pub catalog_id: String,
    /// `CatalogInput$Description`, stored as sent.
    pub description: Option<String>,
    /// `CatalogInput$Parameters`: "a map array of key-value pairs that define the
    /// parameters and properties of the catalog", stored as sent — an absent map and an
    /// empty one are different definitions, so the absence is kept.
    pub parameters: Option<HashMap<String, String>>,
    /// `CatalogInput$FederatedCatalog`: the entity outside the Glue Data Catalog this
    /// catalog points at, stored as sent.
    pub federated_catalog: Option<FederatedCatalog>,
    /// `CatalogInput$TargetRedshiftCatalog`: the catalog this one links its resources
    /// from, stored as sent.
    pub target_redshift_catalog: Option<TargetRedshiftCatalog>,
    /// `CatalogInput$CatalogProperties`, stored as sent. A read reports it through the
    /// OUTPUT shape of the same properties ([`Self::to_catalog`]).
    pub catalog_properties: Option<CatalogProperties>,
    /// `CatalogInput$CreateTableDefaultPermissions`, stored as sent: an empty list is the
    /// governance decision the doc calls for ("Typically should be explicitly set as an
    /// empty list"), so it is kept apart from a create that named no list at all.
    pub create_table_default_permissions: Option<Vec<PrincipalPermissions>>,
    /// `CatalogInput$CreateDatabaseDefaultPermissions`, stored as sent, for the same
    /// reason as [`Self::create_table_default_permissions`].
    pub create_database_default_permissions: Option<Vec<PrincipalPermissions>>,
    /// `CatalogInput$AllowFullTableExternalDataAccess`, stored as sent.
    pub allow_full_table_external_data_access: Option<String>,
    /// `Catalog$CreateTime`: "the time at which the catalog was created", the instant the
    /// create was served, so every later read reports the same timestamp.
    pub create_time: SystemTime,
    /// `Catalog$UpdateTime`: "the time at which the catalog was last updated" — the
    /// create's own instant until a write changes the definition.
    pub update_time: SystemTime,
    /// The create's `Tags`, which the `Catalog` shape does not carry.
    ///
    /// Kept so the create does not discard what the client sent, but not yet reachable
    /// through the tagging operations: [`TaggedResourceKind`] carries no catalog variant,
    /// and that table is what every tag lookup resolves an ARN against.
    pub tags: HashMap<String, String>,
}

impl CatalogDefinition {
    /// Build a stored catalog from a `CreateCatalog` request served at `now`: `name` as
    /// the client spelled it, `catalog_id` the id the catalog answers to, the definition
    /// from `input`, and both instants from the clock.
    pub fn from_create_input(
        name: &str,
        catalog_id: String,
        input: &CatalogInput,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            catalog_id,
            description: input.description.clone(),
            parameters: input.parameters.clone(),
            federated_catalog: input.federated_catalog.clone(),
            target_redshift_catalog: input.target_redshift_catalog.clone(),
            catalog_properties: input.catalog_properties.clone(),
            create_table_default_permissions: input.create_table_default_permissions.clone(),
            create_database_default_permissions: input.create_database_default_permissions.clone(),
            allow_full_table_external_data_access: input
                .allow_full_table_external_data_access
                .clone(),
            create_time: now,
            update_time: now,
            tags: tags.cloned().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `Catalog` shape from what is stored, as served in `region`.
    ///
    /// `ResourceArn` is minted per request rather than stored, like every other ARN this
    /// crate reports: a Glue record carries no Region of its own, so the ARN names the
    /// Region the read was served in ([`catalog_resource_arn`]).
    pub fn to_catalog(&self, region: &str) -> crate::ops::types::Catalog {
        crate::ops::types::Catalog {
            catalog_id: Some(self.catalog_id.clone()),
            name: self.name.clone(),
            resource_arn: Some(catalog_resource_arn(
                region,
                &parse_catalog_reference(&self.catalog_id).account_id,
                &self.name,
            )),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            create_time: Some(to_date_time(self.create_time)),
            update_time: Some(to_date_time(self.update_time)),
            target_redshift_catalog: self.target_redshift_catalog.clone(),
            federated_catalog: self.federated_catalog.clone(),
            catalog_properties: self
                .catalog_properties
                .as_ref()
                .map(|properties| to_catalog_properties_output(properties, self.update_time)),
            create_table_default_permissions: self
                .create_table_default_permissions
                .as_ref()
                .map(|grants| grants.iter().map(tagged_principal_permissions).collect()),
            create_database_default_permissions: self
                .create_database_default_permissions
                .as_ref()
                .map(|grants| grants.iter().map(tagged_principal_permissions).collect()),
            allow_full_table_external_data_access: self
                .allow_full_table_external_data_access
                .clone(),
        }
    }
}

/// The stored `CatalogProperties` as a read REPORTS them, in the output shape of the same
/// properties: every member the write carried, plus the members only the server can fill.
///
/// `IcebergOptimizationPropertiesOutput$LastUpdatedTime` is one of those: it is the
/// instant the stored properties were last written, which is the catalog's own update
/// instant. The `DataLakeAccessPropertiesOutput` members that report a MANAGED Redshift
/// Serverless compute ("The managed Redshift Serverless compute name that is created for
/// your catalog resource") stay unset, because this emulator provisions no such compute —
/// reporting a name or a status for one would describe something that does not exist.
pub fn to_catalog_properties_output(
    stored: &CatalogProperties,
    updated: SystemTime,
) -> CatalogPropertiesOutput {
    CatalogPropertiesOutput {
        data_lake_access_properties: stored.data_lake_access_properties.as_ref().map(
            |properties| DataLakeAccessPropertiesOutput {
                data_lake_access: properties.data_lake_access,
                data_transfer_role: properties.data_transfer_role.clone(),
                kms_key: properties.kms_key.clone(),
                catalog_type: properties.catalog_type.clone(),
                managed_workgroup_name: None,
                managed_workgroup_status: None,
                redshift_database_name: None,
                status_message: None,
            },
        ),
        iceberg_optimization_properties: stored.iceberg_optimization_properties.as_ref().map(
            |properties| IcebergOptimizationPropertiesOutput {
                role_arn: properties.role_arn.clone(),
                compaction: properties.compaction.clone(),
                retention: properties.retention.clone(),
                orphan_file_deletion: properties.orphan_file_deletion.clone(),
                last_updated_time: Some(to_date_time(updated)),
            },
        ),
        custom_properties: stored.custom_properties.clone(),
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
    /// The TABLE OPTIMIZERS of this table — "an optimizer associated with a table"
    /// (`TableOptimizer`) — keyed by the `TableOptimizerType` each one is of.
    ///
    /// On the table rather than beside the catalog tree, exactly as its column statistics
    /// settings are: every operation that addresses an optimizer names its catalog, its
    /// database, its table and then the TYPE (`GetTableOptimizerRequest$Type`), so there is
    /// at most one optimizer of each type per table — which is why a second create of the
    /// same type is a duplicate rather than an overwrite, and why dropping the table drops
    /// its optimizers with it. Ordered by type, so a reader has one deterministic order.
    pub optimizers: BTreeMap<String, TableOptimizerEntry>,
}

/// One stored table optimizer: the configuration a create or update wrote for one
/// `TableOptimizerType` of one table.
///
/// `lastRun` is absent on every stored optimizer: a run is work an optimizer performs over
/// the table's data files, and nothing in this emulator performs it, so there is no run to
/// report ([`TableOptimizerEntry::to_table_optimizer`]).
#[derive(Debug, Clone)]
pub struct TableOptimizerEntry {
    /// The optimizer's type, which is also this entry's key in [`TableEntry::optimizers`].
    pub r#type: String,
    /// "A `TableOptimizerConfiguration` object that was specified when creating or
    /// updating a table optimizer" (`TableOptimizer$configuration`), stored exactly as
    /// sent so a read reports the configuration this write recorded.
    pub configuration: TableOptimizerConfiguration,
}

impl TableOptimizerEntry {
    /// Build a stored optimizer from the configuration a create submitted for `r#type`.
    pub fn new(r#type: &str, configuration: &TableOptimizerConfiguration) -> Self {
        Self {
            r#type: r#type.to_string(),
            configuration: configuration.clone(),
        }
    }

    /// This optimizer in the `TableOptimizer` shape every read of one answers with.
    pub fn to_table_optimizer(&self) -> TableOptimizer {
        TableOptimizer {
            r#type: Some(self.r#type.clone()),
            configuration: Some(self.configuration.clone()),
            // No run: nothing here compacts data files, expires snapshots or deletes
            // orphan files, so reporting a run would report work nothing did.
            last_run: None,
            // `configurationSource` "indicates how the table optimizer was configured and
            // which entity or service initiated the configuration"; the service publishes
            // no rule mapping a create to one of its two values, so none is reported.
            configuration_source: None,
        }
    }
}

/// The table parameter whose value marks a stored table as an open table format, and the
/// value an Iceberg table carries in it. Set by a `CreateTable` that names an
/// `OpenTableFormatInput`, and read back by every later read of the table, which is how a
/// client — and a service that consumes Glue tables — tells an Iceberg table from a
/// Hive-style one.
pub const ICEBERG_TABLE_TYPE_PARAMETER: &str = "table_type";
/// See [`ICEBERG_TABLE_TYPE_PARAMETER`]: the value an Iceberg table carries in it.
pub const ICEBERG_TABLE_TYPE: &str = "ICEBERG";
/// The table parameter carrying the location of an Iceberg table's current metadata file,
/// which is what an Iceberg client reads a table's schema and snapshots from.
pub const ICEBERG_METADATA_LOCATION_PARAMETER: &str = "metadata_location";
/// The table parameter carrying an Iceberg table's format version — the property Iceberg
/// itself names that version under, which is where the version a create asked for
/// (`IcebergInput$Version`) is observable afterwards.
pub const ICEBERG_FORMAT_VERSION_PARAMETER: &str = "format-version";
/// The Iceberg format version a create that names none asks for: "The table version for
/// the Iceberg table. Defaults to 2" (`IcebergInput$Version`).
pub const DEFAULT_ICEBERG_FORMAT_VERSION: &str = "2";
/// The directory, under an Iceberg table's own location, that its metadata files live in,
/// as Iceberg lays a table out.
const ICEBERG_METADATA_DIRECTORY: &str = "metadata";
/// How many 64-bit words the random part of an Iceberg metadata file name carries: two,
/// i.e. the 128 bits a UUID is, which is what Iceberg names a metadata file with.
const ICEBERG_METADATA_ID_WORDS: usize = 2;

/// How many Iceberg metadata file names this process has minted. A nonce source for
/// [`new_iceberg_metadata_location`], exactly as [`MINTED_DATABASE_IDS`] is for a database
/// id.
static MINTED_ICEBERG_METADATA_FILES: AtomicU64 = AtomicU64::new(0);

/// The Iceberg metadata a create of an open-table-format table mints for it, and records
/// on the stored table so every later read serves the same metadata location.
///
/// A create "mints the Iceberg metadata" for the table: the first metadata file of a table
/// whose history is one commit long, under the table's own location.
#[derive(Debug, Clone)]
pub struct IcebergTableMarker {
    /// Where the table's current metadata file lives
    /// ([`new_iceberg_metadata_location`]).
    pub metadata_location: String,
    /// The Iceberg format version the create asked for, else
    /// [`DEFAULT_ICEBERG_FORMAT_VERSION`].
    pub format_version: String,
}

impl IcebergTableMarker {
    /// Mint the metadata of an Iceberg table stored at `location`, at the format `version`
    /// the create named.
    pub fn mint(location: &str, version: Option<&str>) -> Self {
        Self {
            metadata_location: new_iceberg_metadata_location(location),
            format_version: version
                .unwrap_or(DEFAULT_ICEBERG_FORMAT_VERSION)
                .to_string(),
        }
    }

    /// Record this metadata on the `TableInput` about to be stored, so the stored table —
    /// and every read of it — carries the three parameters that make it an Iceberg table.
    ///
    /// The client's own parameters are kept: only the three the service sets are written
    /// over, because they describe what the create minted rather than what it asked for.
    pub fn apply(&self, input: &mut TableInput) {
        let parameters = input.parameters.get_or_insert_with(HashMap::new);
        parameters.insert(
            ICEBERG_TABLE_TYPE_PARAMETER.to_string(),
            ICEBERG_TABLE_TYPE.to_string(),
        );
        parameters.insert(
            ICEBERG_METADATA_LOCATION_PARAMETER.to_string(),
            self.metadata_location.clone(),
        );
        parameters.insert(
            ICEBERG_FORMAT_VERSION_PARAMETER.to_string(),
            self.format_version.clone(),
        );
    }
}

/// The location of the FIRST metadata file of an Iceberg table stored at `location`.
///
/// Iceberg lays a table's metadata out under the table's own location, in a file named by
/// the commit it records and a random id —
/// `<location>/metadata/00000-<id>.metadata.json` for the first commit, which is the one a
/// create records. The id is 128 bits from [`mint_hex_id`], spelled the way a UUID is, so
/// two tables created in the same nanosecond cannot be given the same metadata file.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_iceberg_metadata_location(location: &str) -> String {
    let base = location.trim_end_matches('/');
    let digits = mint_hex_id(&MINTED_ICEBERG_METADATA_FILES, ICEBERG_METADATA_ID_WORDS);
    // The 32 hexadecimal characters grouped 8-4-4-4-12, the way a UUID is spelled. Built
    // by walking the characters rather than by slicing, so the spelling cannot depend on
    // the id's width.
    let mut id = String::with_capacity(digits.len() + 4);
    for (position, character) in digits.chars().enumerate() {
        if matches!(position, 8 | 12 | 16 | 20) {
            id.push('-');
        }
        id.push(character);
    }
    format!("{base}/{ICEBERG_METADATA_DIRECTORY}/00000-{id}.metadata.json")
}

/// The Hive-style table definition an Iceberg create describes, for a request that carries
/// no `TableInput` of its own.
///
/// `CreateIcebergTableInput` is "the configuration parameters required to create a new
/// Iceberg table": it names the table's location, its schema and its table properties,
/// which with the request's `Name` is everything a stored table needs.
pub fn iceberg_table_definition(name: &str, create: &CreateIcebergTableInput) -> TableInput {
    TableInput {
        name: name.to_string(),
        storage_descriptor: Some(StorageDescriptor {
            columns: Some(iceberg_schema_columns(&create.schema)),
            location: Some(create.location.clone()),
            ..StorageDescriptor::default()
        }),
        parameters: create.properties.clone(),
        ..TableInput::default()
    }
}

/// One catalog `Column` per field of an Iceberg schema, in the order the schema declares
/// them: a field's `Name` is the column's name, its `Doc` the column's comment.
///
/// A field's `Type` is an Iceberg type document. A PRIMITIVE Iceberg type is spelled as a
/// string (`"long"`, `"string"`), which is the column type; a nested one is an object, and
/// this slice maps no nested Iceberg type onto a Hive column type, so such a column is
/// reported with its name and no type rather than with a type nothing produced.
fn iceberg_schema_columns(schema: &IcebergSchema) -> Vec<Column> {
    schema
        .fields
        .iter()
        .map(|field| Column {
            name: field.name.clone(),
            r#type: match &field.r#type {
                ::aws_smithy_types::Document::String(spelling) => Some(spelling.clone()),
                _ => None,
            },
            comment: field.doc.clone(),
            parameters: None,
        })
        .collect()
}

/// Where an Iceberg table being created stores its data: the location its `TableInput`
/// names, else the one its `CreateIcebergTableInput` names, and `None` when neither does.
///
/// The table's metadata is minted UNDER that location
/// ([`new_iceberg_metadata_location`]), so a create that names none has nowhere to put the
/// metadata the table is read through.
pub fn iceberg_table_location(
    definition: &TableInput,
    create: Option<&CreateIcebergTableInput>,
) -> Option<String> {
    definition
        .storage_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.location.clone())
        .or_else(|| create.map(|create| create.location.clone()))
        .filter(|location| !location.is_empty())
}

/// Why a read that has to resolve a table's metadata through the source a FEDERATED
/// database points at cannot be served, as the message of the modelled
/// `FederationSourceRetryableException` — or `None` for a database whose metadata the
/// catalog itself holds.
///
/// "A database that points to an entity outside the Glue Data Catalog"
/// (`FederatedDatabase`) keeps its tables in that entity, reached through the connection
/// its registration names, so a read of one of its tables is answered by the SOURCE and not
/// out of the catalog. This emulator connects to no such source — a registration is
/// accepted, and nothing can reach what it points at — so that read fails the way a source
/// that is there but unreachable fails: retryably, which is the code that tells a client to
/// retry rather than give up.
pub fn federated_source_refusal(database: &DatabaseEntry) -> Option<String> {
    let federated = database.federated_database.as_ref()?;
    let source = federated
        .connection_name
        .clone()
        .or_else(|| federated.identifier.clone())
        .unwrap_or_default();
    Some(messages::federated_source_unreachable(
        &database.name,
        &source,
    ))
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

/// Why a partition of a table could not be updated.
///
/// One refusal reason per way an update can fail ON the table it addresses, so the
/// single-partition update and each item of a batch update accept and refuse exactly the
/// same entries: the batch reports it as a per-item [`ErrorDetail`] inside
/// `BatchUpdatePartitionFailureEntry` — "the errors encountered when trying to update the
/// requested partitions" — and a single-partition update raises the matching modelled
/// exception named by [`Self::error_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionUpdateRefusal {
    /// The submitted `PartitionInput` named no partition value at all. "Although this
    /// parameter is not required by the SDK, you must specify this parameter for a valid
    /// input" (`PartitionInput$Values`), the same requirement a create is held to.
    ValuesRequired,
    /// The submitted `PartitionInput` named values other than the ones the entry
    /// addresses. "The Values property can't be changed. If you want to change the
    /// partition key values for a partition, delete and recreate the partition"
    /// (`UpdatePartitionRequest$PartitionInput`), so an update cannot move a partition to
    /// another address.
    ValuesCannotBeChanged,
    /// The addressed table holds no partition at those values, so there is nothing to
    /// update.
    NotFound,
}

impl PartitionUpdateRefusal {
    /// The modelled error shape this refusal is reported under: the `ErrorCode` of a
    /// batch's per-item [`ErrorDetail`], and the exception a single-partition update
    /// raises.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::ValuesRequired | Self::ValuesCannotBeChanged => "InvalidInputException",
            Self::NotFound => "EntityNotFoundException",
        }
    }

    /// This refusal's wording. See [`messages`] for how this crate's refusals are worded.
    pub fn message(self) -> String {
        match self {
            Self::ValuesRequired => messages::PARTITION_VALUES_REQUIRED.to_string(),
            Self::ValuesCannotBeChanged => {
                messages::PARTITION_VALUES_CANNOT_BE_CHANGED.to_string()
            }
            Self::NotFound => messages::PARTITION_NOT_FOUND.to_string(),
        }
    }

    /// This refusal as the per-item error shape a batch reports it in.
    pub fn to_error_detail(self) -> ErrorDetail {
        catalog_error_detail(self.error_code(), self.message())
    }
}

/// Why a partition of a table could not be deleted.
///
/// One refusal reason per way a delete can fail ON the table it addresses, so the
/// single-partition delete and each item of a batch delete accept and refuse exactly the
/// same value lists: the batch reports it as a per-item [`ErrorDetail`] inside
/// `PartitionError` — "the errors encountered when trying to delete the requested
/// partitions" — and a single-partition delete raises the matching modelled exception
/// named by [`Self::error_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionDeleteRefusal {
    /// The addressed table holds no partition at those values, so there is nothing to
    /// delete. A delete addresses a partition by the values it was created under
    /// (`PartitionValueList$Values`), matched exactly as a read matches them, so a list
    /// this table holds no partition for names no partition of it at all.
    NotFound,
}

impl PartitionDeleteRefusal {
    /// The modelled error shape this refusal is reported under: the `ErrorCode` of a
    /// batch's per-item [`ErrorDetail`], and the exception a single-partition delete
    /// raises.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::NotFound => "EntityNotFoundException",
        }
    }

    /// This refusal's wording, the one this service answers a partition miss with
    /// wherever it reports one. See [`messages`] for how this crate's refusals are
    /// worded.
    pub fn message(self) -> String {
        match self {
            Self::NotFound => messages::PARTITION_NOT_FOUND.to_string(),
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

    /// Overwrite this partition's definition with the one the client submitted.
    ///
    /// `PartitionInput` is the WHOLE client-settable definition of a partition — the same
    /// shape a create submits — so an update writes every member it carries and clears the
    /// ones it omits; what the client sent is exactly what the next read reports. The
    /// members only the server sets keep their own history: the partition stays at the
    /// address it was created under and reports the `CreationTime` the create wrote,
    /// because an update produces no new partition.
    ///
    /// `encryption` is the key material THIS write is encrypted under, which replaces the
    /// create's: "the specified encryption is applied to every catalog write thereafter"
    /// (`PutDataCatalogEncryptionSettings`), so the stored definition is read back through
    /// the key the write that stored it used (see [`catalog_encryption`]).
    pub fn apply_input(&mut self, input: &PartitionInput, encryption: Option<ObjectEncryption>) {
        self.storage_descriptor = input.storage_descriptor.clone();
        self.parameters = input.parameters.clone();
        self.last_access_time = input.last_access_time;
        self.last_analyzed_time = input.last_analyzed_time;
        self.encryption = encryption;
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
            // Nor any optimizer: an optimizer of a table arrives through
            // `CreateTableOptimizer`, which addresses a table that already exists.
            optimizers: BTreeMap::new(),
        }
    }

    /// How many partition keys this table defines: the number of values that address one
    /// of its partitions.
    pub fn partition_key_count(&self) -> usize {
        self.partition_keys.as_ref().map_or(0, Vec::len)
    }

    /// Whether this table is an open-table-format (Iceberg) table, i.e. whether its
    /// parameters carry the marker an Iceberg table is recognised by
    /// ([`ICEBERG_TABLE_TYPE_PARAMETER`] holding [`ICEBERG_TABLE_TYPE`]).
    ///
    /// That marker is what every client and every service that consumes Glue tables tells
    /// an Iceberg table from a Hive one by, so it is read off the STORED parameters
    /// whichever write put it there: the create that minted the table's Iceberg metadata
    /// ([`IcebergTableMarker::parameters`]) or a `TableInput` that carried the parameter
    /// itself. The value is compared without regard to case, because the parameter is a
    /// client-written table property.
    pub fn is_iceberg_table(&self) -> bool {
        self.parameters.as_ref().is_some_and(|parameters| {
            parameters
                .get(ICEBERG_TABLE_TYPE_PARAMETER)
                .is_some_and(|table_type| table_type.eq_ignore_ascii_case(ICEBERG_TABLE_TYPE))
        })
    }

    /// The location of this table's current Iceberg metadata file, from the parameter an
    /// Iceberg table carries it in ([`ICEBERG_METADATA_LOCATION_PARAMETER`]), or `None` for
    /// a table whose parameters name none.
    ///
    /// An empty value is no filepath: the parameter is client-writable, so a table can carry
    /// the key with nothing in it.
    pub fn iceberg_metadata_location(&self) -> Option<&str> {
        self.parameters
            .as_ref()?
            .get(ICEBERG_METADATA_LOCATION_PARAMETER)
            .map(String::as_str)
            .filter(|location| !location.is_empty())
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

    /// Update ONE partition of this table from a client's `PartitionInput`, or say why it
    /// cannot be updated.
    ///
    /// The whole update decision for a single partition, in one place, so the
    /// single-partition update and each entry of a batch update accept and refuse exactly
    /// the same entries: a partition is addressed by the values it was created under, an
    /// update replaces its definition rather than its address, and a value list this table
    /// holds no partition for is nothing to update.
    ///
    /// `values` is the ADDRESS the entry names (`PartitionValueList`) and `input` the
    /// definition to write; `encryption` is the key material this write is encrypted
    /// under, resolved from the catalog's own settings by the caller before it took the
    /// store's guard, exactly as a create's is.
    pub fn update_partition(
        &mut self,
        values: &[String],
        input: &PartitionInput,
        encryption: Option<ObjectEncryption>,
    ) -> Result<&PartitionEntry, PartitionUpdateRefusal> {
        let submitted = input.values.as_deref().unwrap_or_default();
        if submitted.is_empty() {
            return Err(PartitionUpdateRefusal::ValuesRequired);
        }
        if submitted != values {
            return Err(PartitionUpdateRefusal::ValuesCannotBeChanged);
        }
        let Some(entry) = self.partitions.get_mut(values) else {
            return Err(PartitionUpdateRefusal::NotFound);
        };
        entry.apply_input(input, encryption);
        Ok(entry)
    }

    /// Delete ONE partition of this table, or say why it cannot be deleted.
    ///
    /// The whole delete decision for a single partition, in one place, so the
    /// single-partition delete and each item of a batch delete accept and refuse exactly
    /// the same value lists: a partition is addressed by the values it was created under,
    /// matched exactly as a read matches them, and a list this table holds no partition
    /// for is nothing to delete. A deleted partition leaves the record every partition
    /// read consults ([`Self::partitions`]), so it is gone for every later reader, and the
    /// table's other partitions are untouched.
    ///
    /// Nothing of the removed partition is handed back: a delete reports the removal and
    /// no definition, and neither the single-partition `DeletePartitionResponse` nor a
    /// batch's `PartitionError` carries one.
    pub fn delete_partition(&mut self, values: &[String]) -> Result<(), PartitionDeleteRefusal> {
        if self.partitions.remove(values).is_some() {
            Ok(())
        } else {
            Err(PartitionDeleteRefusal::NotFound)
        }
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

    /// The counters of every version of this table the catalog holds, ASCENDING.
    ///
    /// A version of a table is either the live definition — the version
    /// [`Self::version_id`] names — or one the catalog archived
    /// ([`Self::archived_versions`]), so the two together are the whole history a version
    /// listing walks, in the order the counters were minted ("Each version is incremented
    /// by 1", `GetTableVersionRequest$VersionId`). Each counter is readable through
    /// [`Self::table_version`], so the list is exactly the versions that answer.
    pub fn version_counters(&self) -> Vec<u64> {
        let mut counters: Vec<u64> = self.archived_versions.keys().copied().collect();
        if let Some(current) = parse_version_counter(&self.version_id) {
            counters.push(current);
        }
        counters.sort_unstable();
        counters.dedup();
        counters
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

/// The schedule state a write that STATES a crawler's schedule leaves it in: a cron
/// expression the write puts on the crawler is in force the moment the write returns, and
/// a crawler left with no expression has no schedule running for it at all.
///
/// One place, so `CreateCrawler`, `UpdateCrawler` and `UpdateCrawlerSchedule` cannot
/// disagree about the state the schedule they wrote is in.
fn stated_schedule_state(expression: Option<&str>) -> String {
    if expression.is_some() {
        SCHEDULE_STATE_SCHEDULED.to_string()
    } else {
        SCHEDULE_STATE_NOT_SCHEDULED.to_string()
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
    /// The state that schedule is in, one of the modelled `ScheduleState` values: a cron
    /// expression a write states is in force ([`SCHEDULE_STATE_SCHEDULED`]) until the
    /// operation that suspends a schedule moves it to [`SCHEDULE_STATE_NOT_SCHEDULED`],
    /// which is also where a crawler carrying no expression sits. Reported as the
    /// `Schedule`'s `State`, so it is observable only while an expression is there to
    /// report it with.
    pub schedule_state: String,
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
            schedule_state: stated_schedule_state(input.schedule.as_deref()),
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
            self.schedule_state = stated_schedule_state(Some(schedule));
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
        self.schedule_state = stated_schedule_state(schedule);
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// Whether this crawler's schedule is RUNNING, i.e. its cron expression is in force
    /// ([`SCHEDULE_STATE_SCHEDULED`]). False both for a crawler whose schedule has been
    /// suspended and for one that carries no expression to run: neither has a scheduler
    /// running for it, which is what the operations that suspend and resume a schedule
    /// decide about.
    pub fn schedule_is_running(&self) -> bool {
        self.schedule_state == SCHEDULE_STATE_SCHEDULED
    }

    /// Suspend this crawler's schedule: the schedule state becomes
    /// [`SCHEDULE_STATE_NOT_SCHEDULED`], which is all `StopCrawlerSchedule` decides —
    /// "sets the schedule state of the specified crawler to NOT_SCHEDULED, but does not
    /// stop the crawler if it is already running".
    ///
    /// The cron expression STAYS on the crawler: suspending a schedule is not removing it
    /// (that is what a schedule write stating no expression does), so the expression is
    /// there to be put back in force. The crawler's run `State` is untouched for the
    /// reason the documentation gives, and `Version` with it: it counts the crawler's
    /// DEFINITION, which this call writes no member of. `LastUpdated` moves, because the
    /// crawler record did change.
    pub fn suspend_schedule(&mut self, now: SystemTime) {
        self.schedule_state = SCHEDULE_STATE_NOT_SCHEDULED.to_string();
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
                state: Some(self.schedule_state.clone()),
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
    /// How many events EventBridge has delivered to this trigger's workflow since its
    /// current batch opened, and `0` when no batch is open.
    ///
    /// Server-set and reported by no member of the `Trigger` shape: it is the accounting
    /// the `EventBatchingCondition` is read against — "Number of events that must be
    /// received from Amazon EventBridge before EventBridge event trigger fires"
    /// ([`EventBatchingCondition$BatchSize`]) — so what a client observes of it is the
    /// workflow run the batch starts and not the count itself. See [`Self::receive_event`].
    ///
    /// [`EventBatchingCondition$BatchSize`]: EventBatchingCondition
    pub batched_events: i32,
    /// When this trigger's current batch opened, and `None` when no batch is open:
    /// "Window starts when first event is received" (`EventBatchingCondition$BatchWindow`),
    /// so this is the instant the FIRST event of the open batch arrived and the window is
    /// measured from it.
    pub batch_window_opened_at: Option<SystemTime>,
    /// The request's `StartOnCreation`, as the create that stored this record sent it.
    ///
    /// Kept beside the definition because it is a member of the REQUEST rather than of the
    /// `Trigger` shape — no read reports it — and because [`Self::state`] is no record of
    /// it: a later start or stop moves the state, so what the create asked for would be
    /// lost. It is what the idempotency comparison reads
    /// ([`Self::matches_create_input`]).
    pub start_on_creation: bool,
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
            // A trigger a create registered has been handed no event yet, so no batch of
            // one is open.
            batched_events: 0,
            batch_window_opened_at: None,
            start_on_creation: input.start_on_creation,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Whether this stored trigger is the one `input` asks to create, member for member.
    ///
    /// A trigger is addressed by its name, so the name is what a create reuses when it
    /// submits a definition under one already taken: such a create is a RETRY of the create
    /// that stored this record only when it asks for the very same trigger, and otherwise it
    /// has associated one identifier with two different records. The comparison is over
    /// every member a create sets, the tags included, because any of them is part of the
    /// trigger the caller asked for.
    pub fn matches_create_input(&self, input: &CreateTriggerInput) -> bool {
        // The name is compared too, so the comparison is member for member and holds for
        // any pair rather than only for a lookup already keyed by the requested name.
        self.name == input.name
            && self.trigger_type == input.r#type
            // The REQUEST member, not the trigger's state, which a later start or stop
            // moves: an identical retry is still a retry of a trigger that has been
            // started since.
            && self.start_on_creation == input.start_on_creation
            && self.workflow_name == input.workflow_name
            && self.description == input.description
            && self.schedule == input.schedule
            && same_trigger_actions(&self.actions, &input.actions)
            && same_trigger_predicate(self.predicate.as_ref(), input.predicate.as_ref())
            && same_event_batching_condition(
                self.event_batching_condition.as_ref(),
                input.event_batching_condition.as_ref(),
            )
            && input
                .tags
                .as_ref()
                .map_or(self.tags.is_empty(), |tags| &self.tags == tags)
    }

    /// Whether this trigger CAN START the job `job_name`, i.e. whether firing it would
    /// begin a run of that job.
    ///
    /// A trigger initiates "the actions initiated by this trigger" (`Trigger$Actions`) and
    /// an action naming a job begins a run of it ([`ComAmazonawsGlueAwsglueState::
    /// initiate_trigger_actions`]), so the jobs a trigger can start are exactly the
    /// `JobName`s of its actions. The job a condition WAITS ON (`Condition$JobName`) is not
    /// one the trigger starts, so a predicate does not qualify a trigger here. Names are
    /// compared exactly, as every other read of a job name is.
    pub fn starts_job(&self, job_name: &str) -> bool {
        self.actions
            .iter()
            .any(|action| action.job_name.as_deref() == Some(job_name))
    }

    /// Move this trigger into the state a start leaves it in.
    ///
    /// "Starts an existing trigger" (`StartTrigger`) ARMS a trigger that waits on something
    /// outside the caller — its schedule, its predicate or the events it batches — so such a
    /// trigger is left in [`TRIGGER_STATE_ACTIVATED`], which every later read reports until
    /// a stop moves it.
    ///
    /// A trigger that waits on nothing but a caller has no armed state to be put into: it is
    /// the one type a create cannot start, "Set to true to start SCHEDULED and CONDITIONAL
    /// triggers when created. True is not supported for ON_DEMAND triggers"
    /// (`CreateTriggerRequest$StartOnCreation`), because there is nothing for it to wait for
    /// once armed. Starting one FIRES it, and what the start leaves it in is the state it
    /// waits in ([`TRIGGER_STATE_CREATED`]), ready for the next caller.
    pub fn start(&mut self) {
        self.state = if self.trigger_type == TRIGGER_TYPE_ON_DEMAND {
            TRIGGER_STATE_CREATED.to_string()
        } else {
            TRIGGER_STATE_ACTIVATED.to_string()
        };
    }

    /// Replace the stored DEFINITION with the one a `TriggerUpdate` carries: "this object
    /// updates the previous trigger definition by overwriting it completely"
    /// (`TriggerUpdate`), so a member the update omits is left with no value rather than
    /// with the one the previous definition had.
    ///
    /// Only the five definition members `TriggerUpdate` declares are touched. The record's
    /// identity and the members the shape carries no value for — [`Self::name`],
    /// [`Self::trigger_type`], [`Self::state`], [`Self::workflow_name`] and [`Self::tags`]
    /// — are left as they are; `TriggerUpdate$Name` is "Reserved for future use", so it
    /// renames nothing.
    pub fn apply_update(&mut self, update: &TriggerUpdate) {
        self.description = update.description.clone();
        self.schedule = update.schedule.clone();
        self.actions = update.actions.clone().unwrap_or_default();
        self.predicate = update.predicate.clone();
        self.event_batching_condition = update.event_batching_condition.clone();
        // The batch open under the PREVIOUS condition was counted against the batch size
        // and window that condition named, and this update overwrites both, so it is
        // closed rather than carried over: the next event delivered opens a batch under
        // the condition now in force.
        self.close_event_batch();
    }

    /// Whether EventBridge deliveries to this trigger's workflow are counted against it.
    ///
    /// Only an `EVENT` trigger batches events — every other `TriggerType` fires on a
    /// caller, the clock or a predicate — and a trigger a stop deactivated fires on
    /// nothing at all ("Stops a specified trigger", `StopTrigger`), so a delivery is
    /// counted against neither.
    pub fn batches_events(&self) -> bool {
        self.trigger_type == TRIGGER_TYPE_EVENT && self.state != TRIGGER_STATE_DEACTIVATED
    }

    /// How many events have to arrive before this trigger fires: "Number of events that
    /// must be received from Amazon EventBridge before EventBridge event trigger fires"
    /// (`EventBatchingCondition$BatchSize`), and [`DEFAULT_EVENT_BATCH_SIZE`] for a trigger
    /// carrying no condition.
    ///
    /// Held at the member's lower bound (`@range 1..100`), so a stored zero cannot make the
    /// trigger unfireable — the same floor a concurrency ceiling gets.
    pub fn event_batch_size(&self) -> i32 {
        self.event_batching_condition
            .as_ref()
            .map_or(DEFAULT_EVENT_BATCH_SIZE, |condition| {
                condition.batch_size.max(DEFAULT_EVENT_BATCH_SIZE)
            })
    }

    /// How long this trigger's open batch waits for the rest of its events: "Window of
    /// time in seconds after which EventBridge event trigger fires"
    /// (`EventBatchingCondition$BatchWindow`), and
    /// [`DEFAULT_EVENT_BATCH_WINDOW_SECONDS`] for a condition that names none.
    ///
    /// Held at the member's lower bound of one second for the reason
    /// [`Self::event_batch_size`] gives.
    pub fn event_batch_window(&self) -> Duration {
        let seconds = self
            .event_batching_condition
            .as_ref()
            .and_then(|condition| condition.batch_window)
            .and_then(|window| u64::try_from(window).ok())
            .unwrap_or(DEFAULT_EVENT_BATCH_WINDOW_SECONDS);
        Duration::from_secs(seconds.max(1))
    }

    /// Count ONE event EventBridge delivered to this trigger's workflow, and answer
    /// whether the batch is now COMPLETE — i.e. whether the trigger fires.
    ///
    /// "Batch condition that must be met (specified number of events received or batch time
    /// window expired) before EventBridge event trigger fires"
    /// (`EventBatchingCondition`): this is the count half of that condition, so the batch
    /// completes on the delivery that reaches [`Self::event_batch_size`] and the trigger
    /// fires ONCE for it, however many events the batch coalesced. A delivery that does not
    /// complete the batch fires nothing, which is what setting a batch size is FOR.
    ///
    /// The first event of a batch also opens its window, because "window starts when first
    /// event is received" ([`Self::batch_window_due_at`]); a completed batch closes, so the
    /// next event opens a new one.
    pub fn receive_event(&mut self, now: SystemTime) -> bool {
        self.batched_events = self.batched_events.saturating_add(1);
        if self.batch_window_opened_at.is_none() {
            self.batch_window_opened_at = Some(now);
        }
        if self.batched_events < self.event_batch_size() {
            return false;
        }
        self.close_event_batch();
        true
    }

    /// When the clock fires this trigger's open batch on its own, and `None` when no batch
    /// is open: the instant its window runs out, which is the window measured from the
    /// first event the batch received.
    ///
    /// The due instant is STORED (as the window's opening instant) rather than waited for:
    /// the firing it owes is materialised by whichever later call reads the workflow's runs
    /// ([`ComAmazonawsGlueAwsglueState::settle_due_event_batches`]), so nothing sleeps.
    pub fn batch_window_due_at(&self) -> Option<SystemTime> {
        if self.batched_events <= 0 {
            return None;
        }
        Some(self.batch_window_opened_at? + self.event_batch_window())
    }

    /// Whether the clock has already run this trigger's open batch window out at `now`, so
    /// the batch fires with the events it has.
    pub fn batch_window_expired(&self, now: SystemTime) -> bool {
        self.batch_window_due_at().is_some_and(|due| now >= due)
    }

    /// Close the open batch: the events it held have fired the trigger (or the window they
    /// were waiting in has), so the next delivery opens a new batch and a new window.
    fn close_event_batch(&mut self) {
        self.batched_events = 0;
        self.batch_window_opened_at = None;
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

/// What became of ONE event EventBridge delivered to a Glue workflow
/// ([`ComAmazonawsGlueAwsglueState::notify_workflow_event`]).
///
/// The delivery is the `glue:notifyEvent` ingress, so this says whether the event was
/// counted into a batch that has not filled yet, whether it completed the batch and began a
/// run, and why no batch could be counted at all — which is what lets the caller that made
/// the delivery report a handover the workflow accepted apart from one it could not make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowEventDelivery {
    /// The event was counted against the trigger's batch, which is not complete yet:
    /// `received` of the `batch_size` events the condition asks for have arrived, so
    /// nothing has fired.
    Counted {
        /// How many events the open batch now holds.
        received: i32,
        /// The `BatchSize` the batch is counted against
        /// ([`TriggerEntry::event_batch_size`]).
        batch_size: i32,
    },
    /// The event completed the batch, and the trigger fired: this run of the workflow
    /// began.
    Fired {
        /// The `WorkflowRunId` of the run the firing began.
        run_id: String,
    },
    /// The workflow's EVENT trigger is stopped, so nothing counts the event and nothing
    /// fires — the delivery itself was still accepted.
    Deactivated,
    /// This account holds no workflow of that name, so there is nothing to deliver to.
    NoWorkflow,
    /// The workflow holds no EVENT trigger in this Region, so no batch exists for the event
    /// to be counted against.
    NoEventTrigger,
    /// The batch completed, but the workflow is already running as many runs as its
    /// `MaxConcurrentRuns` allows, so the firing began none.
    ConcurrentRunsExceeded {
        /// The ceiling the workflow named.
        ceiling: i32,
    },
}

/// Whether two action lists are the same actions, in the same order. The modelled `Action`
/// derives no equality, so its members are compared here, as
/// [`same_data_quality_target_table`] compares its own shape's.
fn same_trigger_actions(stored: &[Action], requested: &[Action]) -> bool {
    stored.len() == requested.len()
        && stored
            .iter()
            .zip(requested.iter())
            .all(|(stored, requested)| {
                stored.job_name == requested.job_name
                    && stored.crawler_name == requested.crawler_name
                    && stored.arguments == requested.arguments
                    && stored.timeout == requested.timeout
                    && stored.security_configuration == requested.security_configuration
                    && same_notification_property(
                        stored.notification_property.as_ref(),
                        requested.notification_property.as_ref(),
                    )
            })
}

/// Whether two optional `NotificationProperty` values are the same delay.
fn same_notification_property(
    stored: Option<&NotificationProperty>,
    requested: Option<&NotificationProperty>,
) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.notify_delay_after == requested.notify_delay_after
        }
        _ => false,
    }
}

/// Whether two optional `Predicate` values are the same predicate: the same `Logical` over
/// the same conditions, in the same order.
fn same_trigger_predicate(stored: Option<&Predicate>, requested: Option<&Predicate>) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.logical == requested.logical
                && same_trigger_conditions(
                    stored.conditions.as_deref(),
                    requested.conditions.as_deref(),
                )
        }
        _ => false,
    }
}

/// Whether two optional condition lists are the same conditions, in the same order.
fn same_trigger_conditions(stored: Option<&[Condition]>, requested: Option<&[Condition]>) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.len() == requested.len()
                && stored
                    .iter()
                    .zip(requested.iter())
                    .all(|(stored, requested)| {
                        stored.logical_operator == requested.logical_operator
                            && stored.job_name == requested.job_name
                            && stored.state == requested.state
                            && stored.crawler_name == requested.crawler_name
                            && stored.crawl_state == requested.crawl_state
                    })
        }
        _ => false,
    }
}

/// Whether two optional `EventBatchingCondition` values are the same batch condition.
fn same_event_batching_condition(
    stored: Option<&EventBatchingCondition>,
    requested: Option<&EventBatchingCondition>,
) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.batch_size == requested.batch_size
                && stored.batch_window == requested.batch_window
        }
        _ => false,
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

impl TriggerReference<'_> {
    /// How a trigger write refuses the definition that names this missing entity: the
    /// wording the lookup of THAT kind of entity answers with wherever it is made, so a
    /// create and an update report one missing job, crawler or workflow alike.
    pub fn refusal_message(&self) -> String {
        match self {
            // The job-definition miss is the same fixed sentence wherever a job is looked
            // up, so a trigger that would run a job nothing holds is refused with it too,
            // and discloses no name.
            Self::Job(_) => messages::JOB_NOT_FOUND.to_string(),
            Self::Crawler(name) => messages::crawler_not_found(name),
            Self::Workflow(name) => messages::workflow_not_found(name),
        }
    }
}

/// The first rule the trigger DEFINITION `trigger_type`/`schedule`/`actions`/`predicate`
/// breaks, worded as the refusal to answer with, or `None` when the definition is one the
/// service could fire.
///
/// One copy of the rule, for every write that leaves a definition behind: `CreateTrigger`
/// stores the definition its request carries and `UpdateTrigger` overwrites a stored one
/// with the definition its `TriggerUpdate` carries, and both have to end up with a trigger
/// that knows what to initiate and when. Every rule here is a modelled requirement:
/// "the actions initiated by this trigger" (`Trigger$Actions`), "this field is required
/// when the trigger type is SCHEDULED" (`Schedule`) and "...is CONDITIONAL" (`Predicate`),
/// "an optional field if only one condition is listed. If multiple conditions are listed,
/// then this field is required" (`Predicate$Logical`), and "a `cron` expression used to
/// specify the schedule" (`Schedule`).
///
/// The caller lifts the sentence into its OWN modelled input error, which is the only part
/// of the refusal that differs between the operations.
pub fn trigger_definition_refusal(
    trigger_type: &str,
    schedule: Option<&str>,
    actions: &[Action],
    predicate: Option<&Predicate>,
) -> Option<String> {
    // A trigger that names no action initiates nothing when it fires, so a definition
    // without one is refused rather than stored.
    if actions.is_empty() {
        return Some(messages::trigger_member_required("Actions"));
    }

    // Without the member its type requires, the service would hold a trigger with nothing
    // to decide when it fires.
    if trigger_type == TRIGGER_TYPE_SCHEDULED && schedule.is_none() {
        return Some(messages::trigger_member_required("Schedule"));
    }
    let conditions = predicate
        .and_then(|predicate| predicate.conditions.as_ref())
        .map_or(0, Vec::len);
    if trigger_type == TRIGGER_TYPE_CONDITIONAL && conditions == 0 {
        return Some(messages::trigger_member_required("Predicate"));
    }

    // Without a `Logical`, a multi-condition predicate does not say whether all or any of
    // its conditions must hold.
    if conditions > 1 && predicate.is_none_or(|predicate| predicate.logical.is_none()) {
        return Some(messages::predicate_logical_required(conditions));
    }

    // Anything but a `cron(<six fields>)` expression is not a schedule this service can
    // run, so it is refused rather than stored and reported back.
    if let Some(schedule) = schedule {
        if !is_cron_expression(schedule) {
            return Some(messages::schedule_not_a_cron_expression(schedule));
        }
    }

    // A conditional trigger fires the actions it carries when the conditions it watches
    // are met, so a job that is BOTH what the trigger waits on and what it runs describes
    // a trigger that would fire on its own output: the definition is refused rather than
    // stored, whether or not that job exists.
    if trigger_type == TRIGGER_TYPE_CONDITIONAL && job_is_predicate_and_action(actions, predicate)
    {
        return Some(messages::JOB_IN_PREDICATE_AND_ACTION.to_string());
    }

    None
}

/// Whether one job is named both by a condition of `predicate` and by an action of
/// `actions` — the overlap a conditional trigger's definition may not carry.
///
/// A job is named exactly as the request spells it (job definitions are keyed by their
/// name, unfolded), so the comparison is over those names as sent. The condition names are
/// collected into a SET first and each action is then one probe of it, so the work is the
/// number of conditions PLUS the number of actions rather than their product: neither
/// factor, both of which the one request body carries, can multiply the other. A definition
/// that names no job in one of the two places cannot overlap at all.
fn job_is_predicate_and_action(actions: &[Action], predicate: Option<&Predicate>) -> bool {
    let condition_jobs: BTreeSet<&str> = predicate
        .and_then(|predicate| predicate.conditions.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|condition| condition.job_name.as_deref())
        .collect();
    if condition_jobs.is_empty() {
        return false;
    }
    actions
        .iter()
        .filter_map(|action| action.job_name.as_deref())
        .any(|action_job| condition_jobs.contains(action_job))
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

/// A stored classifier: the custom classifier `CreateClassifier` registers for a crawler
/// to recognise a data format with.
///
/// The four classifier KINDS the model declares live in ONE record rather than four
/// namespaces, because every operation that addresses a classifier takes a `Name` alone
/// and the read answers with whichever kind that name holds. The members a client sets
/// live on [`ClassifierDefinition`]; the name and the three server-set members
/// (`Version`, `CreationTime`, `LastUpdated`) live here, because every kind declares the
/// same three and a read of any kind reports them the same way.
#[derive(Debug, Clone)]
pub struct ClassifierEntry {
    /// The classifier's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::classifiers`].
    pub name: String,
    /// "The time that this classifier was registered": the instant the create was served,
    /// so every later read of this record reports the same timestamp.
    pub creation_time: SystemTime,
    /// "The time that this classifier was last updated": the creation instant until a
    /// write lands, and the instant of the latest write afterwards.
    pub last_updated: SystemTime,
    /// "The version of this classifier": [`FIRST_CLASSIFIER_VERSION`] for a fresh one,
    /// advanced by one on every write.
    pub version: i64,
    /// Which kind of classifier this is, and the members the client set on it.
    pub definition: ClassifierDefinition,
}

/// What kind of classifier a stored record is, with the members that kind carries.
///
/// The kind is decided by which member of the create request was present — "This can be
/// a GrokClassifier, an XMLClassifier, a JsonClassifier, or a CsvClassifier, depending on
/// which field of the request is present" (`CreateClassifier`) — so the kind and its
/// members are ONE value: a stored classifier cannot hold the members of two kinds.
#[derive(Debug, Clone)]
pub enum ClassifierDefinition {
    /// "A classifier that uses `grok`."
    Grok {
        /// "An identifier of the data format that the classifier matches."
        classification: String,
        /// "The grok pattern applied to a data store by this classifier."
        grok_pattern: String,
        /// "Optional custom grok patterns defined by this classifier."
        custom_patterns: Option<String>,
    },
    /// "A classifier for XML content."
    Xml {
        /// "An identifier of the data format that the classifier matches."
        classification: String,
        /// "The XML tag designating the element that contains each record in an XML
        /// document being parsed."
        row_tag: Option<String>,
    },
    /// "A classifier for JSON content."
    Json {
        /// "A `JsonPath` string defining the JSON data for the classifier to classify."
        json_path: String,
    },
    /// "A classifier for comma-separated values (CSV)."
    Csv(CsvClassifierDefinition),
}

/// The members a CSV classifier carries beyond the name and the server-set three.
///
/// A struct of its own rather than nine fields on the enum variant, so one write path
/// applies them and one read path projects them.
#[derive(Debug, Clone, Default)]
pub struct CsvClassifierDefinition {
    /// "A custom symbol to denote what separates each column entry in the row."
    pub delimiter: Option<String>,
    /// "A custom symbol to denote what combines content into a single column value."
    pub quote_symbol: Option<String>,
    /// "Indicates whether the CSV file contains a header": a `CsvHeaderOption` value.
    pub contains_header: Option<String>,
    /// "A list of strings representing column names."
    pub header: Option<Vec<String>>,
    /// "Specifies not to trim values before identifying the type of column values."
    pub disable_value_trimming: Option<bool>,
    /// "Enables the processing of files that contain only one column."
    pub allow_single_column: Option<bool>,
    /// "Enables the custom datatype to be configured."
    pub custom_datatype_configured: Option<bool>,
    /// "A list of custom datatypes."
    pub custom_datatypes: Option<Vec<String>>,
    /// "Sets the SerDe for processing CSV in the classifier": a `CsvSerdeOption` value.
    pub serde: Option<String>,
}

/// Whether a CSV classifier's quote symbol is the character its column delimiter already
/// is, which the model forbids: "A custom symbol to denote what combines content into a
/// single column value. It must be different from the column delimiter"
/// (`CsvClassifier$QuoteSymbol`).
///
/// Takes the EFFECTIVE pair, so one rule serves a create (the members the request
/// carries) and an update (the members it carries merged over the stored ones). A pair
/// where either side is absent collides with nothing: the classifier names only one
/// separator.
pub fn csv_separators_collide(delimiter: Option<&str>, quote_symbol: Option<&str>) -> bool {
    matches!((delimiter, quote_symbol), (Some(delimiter), Some(quote_symbol)) if delimiter == quote_symbol)
}

impl ClassifierDefinition {
    /// The definition a `CreateClassifier` request carries, or `None` when the request
    /// named no classifier at all.
    ///
    /// Only ONE member is read, because the caller has already established that exactly
    /// one is present: the kind is "which field of the request is present", so a request
    /// naming several names no one kind.
    pub fn from_create_input(input: &CreateClassifierInput) -> Option<Self> {
        if let Some(grok) = input.grok_classifier.as_ref() {
            return Some(Self::Grok {
                classification: grok.classification.clone(),
                grok_pattern: grok.grok_pattern.clone(),
                custom_patterns: grok.custom_patterns.clone(),
            });
        }
        if let Some(xml) = input.xml_classifier.as_ref() {
            return Some(Self::Xml {
                classification: xml.classification.clone(),
                row_tag: xml.row_tag.clone(),
            });
        }
        if let Some(json) = input.json_classifier.as_ref() {
            return Some(Self::Json {
                json_path: json.json_path.clone(),
            });
        }
        let csv = input.csv_classifier.as_ref()?;
        Some(Self::Csv(CsvClassifierDefinition {
            delimiter: csv.delimiter.clone(),
            quote_symbol: csv.quote_symbol.clone(),
            contains_header: csv.contains_header.clone(),
            header: csv.header.clone(),
            disable_value_trimming: csv.disable_value_trimming,
            allow_single_column: csv.allow_single_column,
            custom_datatype_configured: csv.custom_datatype_configured,
            custom_datatypes: csv.custom_datatypes.clone(),
            serde: csv.serde.clone(),
        }))
    }

    /// The name of this kind, spelled as the model's member for it — the same spelling on
    /// `CreateClassifierRequest` and on `UpdateClassifierRequest`, so a refusal that names
    /// a kind names the member a request would carry it in.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Grok { .. } => "GrokClassifier",
            Self::Xml { .. } => "XMLClassifier",
            Self::Json { .. } => "JsonClassifier",
            Self::Csv(_) => "CsvClassifier",
        }
    }
}

/// How many of the four classifier kinds an `UpdateClassifier` request names, and which
/// kind and `Name` the one it names addresses.
///
/// The count and the target are read in one walk because an update needs both: the kind
/// is "which field is present" ("Modifies an existing classifier … depending on which
/// field is present"), and the name that field carries is the record it addresses.
pub fn update_classifier_target(
    input: &UpdateClassifierInput,
) -> (usize, Option<(&'static str, String)>) {
    let named = [
        input
            .grok_classifier
            .as_ref()
            .map(|request| ("GrokClassifier", request.name.clone())),
        input
            .xml_classifier
            .as_ref()
            .map(|request| ("XMLClassifier", request.name.clone())),
        input
            .json_classifier
            .as_ref()
            .map(|request| ("JsonClassifier", request.name.clone())),
        input
            .csv_classifier
            .as_ref()
            .map(|request| ("CsvClassifier", request.name.clone())),
    ];
    let count = named.iter().filter(|target| target.is_some()).count();
    (count, named.into_iter().flatten().next())
}

/// How many of the four classifier kinds a `CreateClassifier` request names.
///
/// The kind of the classifier to create is "which field of the request is present", so
/// this count is what decides whether the request names one kind, none, or an ambiguous
/// several.
pub fn named_classifier_kinds(input: &CreateClassifierInput) -> usize {
    [
        input.grok_classifier.is_some(),
        input.xml_classifier.is_some(),
        input.json_classifier.is_some(),
        input.csv_classifier.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count()
}

impl ClassifierEntry {
    /// Build a stored classifier from a `CreateClassifier` request served at `now`, or
    /// `None` when the request names no classifier kind.
    ///
    /// The definition comes from the request; the name is the one the named kind carries,
    /// and the three server-set members are the service's own: the create is the first
    /// version and both timestamps are the instant it was served.
    pub fn from_create_input(input: &CreateClassifierInput, now: SystemTime) -> Option<Self> {
        Some(Self {
            name: create_classifier_name(input)?,
            creation_time: now,
            last_updated: now,
            version: FIRST_CLASSIFIER_VERSION,
            definition: ClassifierDefinition::from_create_input(input)?,
        })
    }

    /// Apply an `UpdateClassifier` request to this stored classifier, member by member.
    ///
    /// Every member the request CARRIES replaces the stored one; every member it OMITS is
    /// preserved — the only reading an update whose definition members are all optional
    /// admits, and the same rule [`CrawlerEntry::apply_update`] follows. The server-set
    /// members move with the write, as a crawler's do: `Version` advances by one and
    /// `LastUpdated` becomes `now`. The kind, the `Name` and the `CreationTime` are not an
    /// update's to change, so the caller establishes that the request names THIS record's
    /// kind before applying it; a request for another kind leaves the definition alone.
    pub fn apply_update(&mut self, input: &UpdateClassifierInput, now: SystemTime) {
        match &mut self.definition {
            ClassifierDefinition::Grok {
                classification,
                grok_pattern,
                custom_patterns,
            } => {
                if let Some(request) = input.grok_classifier.as_ref() {
                    if let Some(value) = request.classification.as_ref() {
                        *classification = value.clone();
                    }
                    if let Some(value) = request.grok_pattern.as_ref() {
                        *grok_pattern = value.clone();
                    }
                    if let Some(value) = request.custom_patterns.as_ref() {
                        *custom_patterns = Some(value.clone());
                    }
                }
            }
            ClassifierDefinition::Xml {
                classification,
                row_tag,
            } => {
                if let Some(request) = input.xml_classifier.as_ref() {
                    if let Some(value) = request.classification.as_ref() {
                        *classification = value.clone();
                    }
                    if let Some(value) = request.row_tag.as_ref() {
                        *row_tag = Some(value.clone());
                    }
                }
            }
            ClassifierDefinition::Json { json_path } => {
                if let Some(request) = input.json_classifier.as_ref() {
                    if let Some(value) = request.json_path.as_ref() {
                        *json_path = value.clone();
                    }
                }
            }
            ClassifierDefinition::Csv(csv) => {
                if let Some(request) = input.csv_classifier.as_ref() {
                    if let Some(value) = request.delimiter.as_ref() {
                        csv.delimiter = Some(value.clone());
                    }
                    if let Some(value) = request.quote_symbol.as_ref() {
                        csv.quote_symbol = Some(value.clone());
                    }
                    if let Some(value) = request.contains_header.as_ref() {
                        csv.contains_header = Some(value.clone());
                    }
                    if let Some(value) = request.header.as_ref() {
                        csv.header = Some(value.clone());
                    }
                    if let Some(value) = request.disable_value_trimming {
                        csv.disable_value_trimming = Some(value);
                    }
                    if let Some(value) = request.allow_single_column {
                        csv.allow_single_column = Some(value);
                    }
                    if let Some(value) = request.custom_datatype_configured {
                        csv.custom_datatype_configured = Some(value);
                    }
                    if let Some(value) = request.custom_datatypes.as_ref() {
                        csv.custom_datatypes = Some(value.clone());
                    }
                    if let Some(value) = request.serde.as_ref() {
                        csv.serde = Some(value.clone());
                    }
                }
            }
        }
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// The CSV members this classifier holds, or `None` when it is of another kind.
    ///
    /// What a CSV update's separator check reads: the rule is about the EFFECTIVE pair,
    /// which is the request's members merged over these.
    pub fn csv_definition(&self) -> Option<&CsvClassifierDefinition> {
        match &self.definition {
            ClassifierDefinition::Csv(csv) => Some(csv),
            _ => None,
        }
    }

    /// Rebuild the modelled `Classifier` shape from what is stored, so a listing reports
    /// the same classifier a single read does.
    ///
    /// Exactly one member of that shape is filled: the one for the kind this record is,
    /// which is how a client tells a grok classifier from a CSV one.
    pub fn to_classifier(&self) -> Classifier {
        let creation_time = Some(to_date_time(self.creation_time));
        let last_updated = Some(to_date_time(self.last_updated));
        match &self.definition {
            ClassifierDefinition::Grok {
                classification,
                grok_pattern,
                custom_patterns,
            } => Classifier {
                grok_classifier: Some(GrokClassifier {
                    name: self.name.clone(),
                    classification: classification.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    grok_pattern: grok_pattern.clone(),
                    custom_patterns: custom_patterns.clone(),
                }),
                ..Classifier::default()
            },
            ClassifierDefinition::Xml {
                classification,
                row_tag,
            } => Classifier {
                xml_classifier: Some(XMLClassifier {
                    name: self.name.clone(),
                    classification: classification.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    row_tag: row_tag.clone(),
                }),
                ..Classifier::default()
            },
            ClassifierDefinition::Json { json_path } => Classifier {
                json_classifier: Some(JsonClassifier {
                    name: self.name.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    json_path: json_path.clone(),
                }),
                ..Classifier::default()
            },
            ClassifierDefinition::Csv(csv) => Classifier {
                csv_classifier: Some(CsvClassifier {
                    name: self.name.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    delimiter: csv.delimiter.clone(),
                    quote_symbol: csv.quote_symbol.clone(),
                    contains_header: csv.contains_header.clone(),
                    header: csv.header.clone(),
                    disable_value_trimming: csv.disable_value_trimming,
                    allow_single_column: csv.allow_single_column,
                    custom_datatype_configured: csv.custom_datatype_configured,
                    custom_datatypes: csv.custom_datatypes.clone(),
                    serde: csv.serde.clone(),
                }),
                ..Classifier::default()
            },
        }
    }
}

/// The name of the classifier a `CreateClassifier` request asks for: the `Name` the one
/// kind it names carries, or `None` when it names no kind.
///
/// Every kind's create request carries its own required `Name`, so the name is read off
/// the named kind rather than from a member of the request itself.
pub fn create_classifier_name(input: &CreateClassifierInput) -> Option<String> {
    if let Some(grok) = input.grok_classifier.as_ref() {
        return Some(grok.name.clone());
    }
    if let Some(xml) = input.xml_classifier.as_ref() {
        return Some(xml.name.clone());
    }
    if let Some(json) = input.json_classifier.as_ref() {
        return Some(json.name.clone());
    }
    input.csv_classifier.as_ref().map(|csv| csv.name.clone())
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

/// The `SessionStatus` a session is in the instant its create returns: the remote Spark
/// environment is still being provisioned. One of the modelled `SessionStatus` values
/// (`PROVISIONING`, `READY`, `FAILED`, `TIMEOUT`, `STOPPING`, `STOPPED`, from
/// `smithy_query shape=SessionStatus`).
pub const SESSION_STATUS_PROVISIONING: &str = "PROVISIONING";

/// The `SessionStatus` of a session whose environment is up and can take statements. See
/// [`SESSION_STATUS_PROVISIONING`].
pub const SESSION_STATUS_READY: &str = "READY";

/// The `SessionStatus` of a session its own timeout ended: "the number of minutes when idle
/// before session times out" (`CreateSessionRequest$IdleTimeout`) has passed. See
/// [`SESSION_STATUS_PROVISIONING`].
pub const SESSION_STATUS_TIMEOUT: &str = "TIMEOUT";

/// How long a session stays in [`SESSION_STATUS_PROVISIONING`] before the clock owes it
/// [`SESSION_STATUS_READY`].
///
/// CHOSEN, not documented: Glue publishes no duration for provisioning a session, so this is
/// the emulator's own span, the same one a blueprint registration takes
/// ([`BLUEPRINT_SETTLES_AFTER`]) — deliberately not zero, so a caller that reads a session
/// straight after creating it still observes it being provisioned, and the refusals that go
/// with a session that is not ready. The due instant is stored on the record and
/// materialised by whichever later call reads it; nothing sleeps.
pub const SESSION_SETTLES_AFTER: Duration = BLUEPRINT_SETTLES_AFTER;

/// `CreateSessionRequest$Timeout` when the create named none: "The number of minutes before
/// session times out. Default for Spark ETL jobs is 48 hours (2880 minutes)."
pub const DEFAULT_SESSION_TIMEOUT_MINUTES: i32 = 2_880;

/// The Glue version a session has to run on, as `CreateSessionRequest$GlueVersion` states
/// the restriction: "The GlueVersion must be greater than 2.0." A create naming a version
/// that is not is refused rather than stored.
pub const MIN_SESSION_GLUE_VERSION: f64 = 2.0;

/// How many seconds one minute of a session's documented timeouts is: both `Timeout` and
/// `IdleTimeout` are stated in minutes.
const SECONDS_PER_MINUTE: u64 = 60;

/// Whether `glue_version` fails the restriction quoted on [`MIN_SESSION_GLUE_VERSION`], and
/// the refusal to state when it does.
///
/// A `GlueVersion` this cannot read as a number (the model's pattern `^(\w+\.)+\w+$` is
/// wider than a decimal) fails nothing: the restriction is stated as a version comparison,
/// so a version no comparison can be made against is not refused by it — the same reading
/// [`admits_flex_execution_class`] takes of its own version restriction.
pub fn session_glue_version_defect(glue_version: &str) -> Option<String> {
    let version = glue_version.parse::<f64>().ok()?;
    (version <= MIN_SESSION_GLUE_VERSION)
        .then(|| messages::session_glue_version_too_low(glue_version))
}

/// A stored interactive session: the remote Spark runtime environment a session create
/// provisions, "the period in which a remote Spark runtime environment is running"
/// (`Session`).
///
/// Everything a caller can observe through the modelled `Session` shape is held here — the
/// id it is addressed by, the command, role and capacity the create asked for, when it was
/// created and the status its provisioning has reached — plus the two documented timeouts
/// and the `Tags` the create carried, neither of which that shape reports.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// `Session$Id`: "The ID of the session", which is also this record's key in
    /// [`ComAmazonawsGlueAwsglueState::sessions`]. The create chooses it
    /// (`CreateSessionRequest$Id`, "The ID of the session request"), so it is not minted
    /// here.
    pub id: String,
    /// `Session$CreatedOn`: "The time and date when the session was created."
    pub created_on: SystemTime,
    /// The instant this session was last active, which is where the IDLE leg of its timeout
    /// is measured from ([`Self::times_out_at`]). A session nothing has run a statement on
    /// has been idle since it was created.
    pub last_active_on: SystemTime,
    /// One of the modelled `SessionStatus` values: "The session status."
    pub status: String,
    /// The instant the clock owes this session its next status; `None` once its
    /// provisioning has settled.
    pub settles_at: Option<SystemTime>,
    /// `Session$CompletedOn`: "The date and time that this session is completed", and
    /// `None` while it is still running.
    pub completed_on: Option<SystemTime>,
    /// `Session$Description`: "The description of the session", and `None` when the create
    /// carried none.
    pub description: Option<String>,
    /// `Session$Role`: "The name or Amazon Resource Name (ARN) of the IAM role associated
    /// with the Session", exactly as the create sent it.
    pub role: String,
    /// `Session$Command`: "The command object", as the create described it.
    pub command: SessionCommand,
    /// `Session$DefaultArguments`: "A map array of key-value pairs", and `None` when the
    /// create carried no map at all — which a read reports differently from an empty one.
    pub default_arguments: Option<HashMap<String, String>>,
    /// `Session$Connections`: "The number of connections used for the session."
    pub connections: Option<ConnectionsList>,
    /// The session's capacity in DPUs: what the create named, else what its worker
    /// configuration maps to ([`dpu_per_worker`]), resolved by the one helper every
    /// capacity-carrying request goes through.
    pub max_capacity: Option<f64>,
    /// `Session$NumberOfWorkers`: "The number of workers of a defined `WorkerType` to use
    /// for the session", as the create named it.
    pub number_of_workers: Option<i32>,
    /// `Session$WorkerType`: "The type of predefined worker that is allocated when a
    /// session runs", as the create named it.
    pub worker_type: Option<String>,
    /// `Session$SecurityConfiguration`: "The name of the SecurityConfiguration structure to
    /// be used with the session."
    pub security_configuration: Option<String>,
    /// `Session$GlueVersion`: "The Glue version determines the versions of Apache Spark and
    /// Python that Glue supports", as the create named it. `None` when it named none: the
    /// model documents no default for a session's Glue version, so none is invented here.
    pub glue_version: Option<String>,
    /// `Session$SessionType`: "The type of the session", one of the modelled `SessionType`
    /// values, as the create named it.
    pub session_type: Option<String>,
    /// "The number of minutes before session times out"
    /// (`CreateSessionRequest$Timeout`), defaulted by
    /// [`DEFAULT_SESSION_TIMEOUT_MINUTES`] when the create named none.
    ///
    /// The modelled `Session` shape carries NO `Timeout` member, so this is not reported;
    /// it is kept because it bounds how long the session runs and is the documented default
    /// for [`Self::idle_timeout`].
    pub timeout: i32,
    /// `Session$IdleTimeout`: "The number of minutes when idle before the session times
    /// out", which for a create that named none is the value of [`Self::timeout`] ("Default
    /// for Spark ETL jobs is value of Timeout").
    pub idle_timeout: i32,
    /// The tags the session was created with, which the modelled `Session` shape does not
    /// carry.
    ///
    /// Kept because `ListSessions` takes "the tags" to narrow its answer by, so they decide
    /// which sessions a filtered listing reports. NOT yet reachable through the tagging
    /// operations, for the reason [`BlueprintEntry::tags`] gives: [`TaggedResourceKind`]
    /// carries no session variant yet.
    pub tags: HashMap<String, String>,
    /// The statements run on this session, oldest first — which is submission order,
    /// because a statement is appended here by the run that recorded it.
    ///
    /// They live ON the session, as a job's runs live on the job, because every operation
    /// that addresses a statement names its `SessionId` and then the `Id` this session
    /// minted for it ([`Self::next_statement_id`]).
    pub statements: Vec<StatementEntry>,
    /// The `Id` the next statement run on this session is given: one past the last one
    /// minted, counting from [`FIRST_STATEMENT_ID`]. Kept on the session rather than
    /// derived from [`Self::statements`], so an id is never reused once the oldest records
    /// are dropped.
    pub next_statement_id: i32,
}

impl SessionEntry {
    /// Record the session a session create describes, in [`SESSION_STATUS_PROVISIONING`]
    /// with [`SESSION_STATUS_READY`] due [`SESSION_SETTLES_AFTER`] out, and every other
    /// member taken from the request or from the default the model documents for it.
    pub fn from_create_input(input: &CreateSessionInput, now: SystemTime) -> Self {
        let timeout = input.timeout.unwrap_or(DEFAULT_SESSION_TIMEOUT_MINUTES);
        Self {
            id: input.id.clone(),
            created_on: now,
            last_active_on: now,
            status: SESSION_STATUS_PROVISIONING.to_string(),
            settles_at: Some(now.checked_add(SESSION_SETTLES_AFTER).unwrap_or(now)),
            completed_on: None,
            description: input.description.clone(),
            role: input.role.clone(),
            command: input.command.clone(),
            default_arguments: input.default_arguments.clone(),
            connections: input.connections.clone(),
            // The same three members a job's capacity is read from, with the same
            // documentation, so they resolve through the same helper.
            max_capacity: capacity_from(
                input.max_capacity,
                0,
                input.worker_type.as_deref(),
                input.number_of_workers,
            ),
            number_of_workers: input.number_of_workers,
            worker_type: input.worker_type.clone(),
            security_configuration: input.security_configuration.clone(),
            glue_version: input.glue_version.clone(),
            session_type: input.session_type.clone(),
            timeout,
            // "Default for Spark ETL jobs is value of Timeout."
            idle_timeout: input.idle_timeout.unwrap_or(timeout),
            tags: input.tags.clone().unwrap_or_default(),
            // Nothing has been run on a session its create has just returned.
            statements: Vec::new(),
            next_statement_id: FIRST_STATEMENT_ID,
        }
    }

    /// The instant this session's own timeouts fall due: the earlier of the two the model
    /// documents — [`Self::timeout`] minutes after it was created ("The number of minutes
    /// before session times out") and [`Self::idle_timeout`] minutes after it was last
    /// active ("The number of minutes when idle before the session times out").
    ///
    /// `None` when neither instant is representable, which leaves the session running.
    pub fn times_out_at(&self) -> Option<SystemTime> {
        let after = |from: SystemTime, minutes: i32| {
            let minutes = u64::try_from(minutes).ok()?;
            from.checked_add(Duration::from_secs(minutes * SECONDS_PER_MINUTE))
        };
        let whole_life = after(self.created_on, self.timeout);
        let idle = after(self.last_active_on, self.idle_timeout);
        match (whole_life, idle) {
            (Some(whole_life), Some(idle)) => Some(whole_life.min(idle)),
            (due, None) | (None, due) => due,
        }
    }

    /// Move this session into the status the clock owed it at `now`, if it owed one.
    ///
    /// Two changes are due by the clock alone, so materialising them on a read reports the
    /// status the clock had already reached and two reads of a settled session agree: the
    /// provisioning that was due to finish at [`Self::settles_at`], and the timeout that
    /// was due at [`Self::times_out_at`]. A session already in a status nothing but a
    /// caller can leave is untouched.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        if self.status == SESSION_STATUS_PROVISIONING {
            if self.settles_at.is_none_or(|due| due > now) {
                return;
            }
            self.settles_at = None;
            self.status = SESSION_STATUS_READY.to_string();
        }
        if self.status != SESSION_STATUS_READY {
            return;
        }
        if let Some(due) = self.times_out_at().filter(|due| *due <= now) {
            self.status = SESSION_STATUS_TIMEOUT.to_string();
            // The session ended when the timeout fell due, not when somebody read it.
            self.completed_on = Some(due);
        }
    }

    /// `Session$ExecutionTime`: "The total time the session ran for", in seconds — the span
    /// from the instant it was created to the instant it completed, and `None` while it is
    /// still running.
    fn execution_time(&self) -> Option<f64> {
        self.completed_on?
            .duration_since(self.created_on)
            .ok()
            .map(|ran| ran.as_secs_f64())
    }

    /// Rebuild the modelled `Session` shape from what is stored, so every reader of a
    /// session reports the same record.
    pub fn to_session(&self) -> Session {
        let execution_time = self.execution_time();
        Session {
            id: Some(self.id.clone()),
            created_on: Some(to_date_time(self.created_on)),
            status: Some(self.status.clone()),
            // "The error message displayed during the session": a session this store
            // accepted has none.
            error_message: None,
            description: self.description.clone(),
            role: Some(self.role.clone()),
            command: Some(self.command.clone()),
            default_arguments: self.default_arguments.clone(),
            connections: self.connections.clone(),
            // "The code execution progress of the session", which is the model's own
            // default until a statement of this session has run code.
            progress: 0.0,
            max_capacity: self.max_capacity,
            security_configuration: self.security_configuration.clone(),
            glue_version: self.glue_version.clone(),
            number_of_workers: self.number_of_workers,
            worker_type: self.worker_type.clone(),
            completed_on: self.completed_on.map(to_date_time),
            execution_time,
            // "The DPUs consumed by the session (formula: ExecutionTime * MaxCapacity)",
            // which is reportable once both of those are.
            dpu_seconds: execution_time
                .zip(self.max_capacity)
                .map(|(ran, capacity)| ran * capacity),
            idle_timeout: Some(self.idle_timeout),
            // "The name of an Glue usage profile associated with the session": a session
            // create carries no profile member, so no session this store holds has one.
            profile_name: None,
            session_type: self.session_type.clone(),
        }
    }

    /// Move every statement of this session into the terminal state the clock owed it at
    /// `now`, if it owed one.
    ///
    /// This is the one write a reporting operation makes, for the reason
    /// [`Self::settle_if_due`] gives: a statement was due to leave the state its run put
    /// it in at [`StatementEntry::settles_at`] whether or not anybody called.
    pub fn settle_due_statements(&mut self, now: SystemTime) {
        for statement in &mut self.statements {
            statement.settle_if_due(now);
        }
    }

    /// The statement of this session under `id`, or `None` when it has none.
    ///
    /// Call [`Self::settle_due_statements`] first, or the record answered carries the
    /// state it was in before the clock moved it.
    pub fn statement(&self, id: i32) -> Option<&StatementEntry> {
        self.statements.iter().find(|statement| statement.id == id)
    }

    /// Whether a statement of this session has not finished, which is what makes the
    /// session busy: the remote runtime is "currently busy processing another request"
    /// (`SessionBusyException`) until the statement it is running comes to rest.
    ///
    /// Call [`Self::settle_due_statements`] first, or a statement the clock has already
    /// finished still counts as in flight.
    pub fn has_statement_in_flight(&self) -> bool {
        self.statements
            .iter()
            .any(|statement| !statement.is_terminal())
    }

    /// Record `code` as the next statement of this session, and answer the `Id` it was
    /// given — "the Id of the statement that was run" (`RunStatementResponse$Id`).
    ///
    /// Running a statement is what a session is used for, so it is also the activity the
    /// IDLE leg of its timeout is measured from ([`Self::times_out_at`]). At most
    /// [`MAX_STATEMENTS_PER_SESSION`] records are kept, the oldest dropped first, exactly
    /// as a job keeps its runs; the id sequence is unaffected by that drop.
    pub fn run_statement(&mut self, code: &str, now: SystemTime) -> i32 {
        let id = self.next_statement_id;
        self.next_statement_id = id.saturating_add(1);
        self.last_active_on = now;
        self.statements.push(StatementEntry::run(id, code, now));
        if self.statements.len() > MAX_STATEMENTS_PER_SESSION {
            let overflow = self.statements.len() - MAX_STATEMENTS_PER_SESSION;
            self.statements.drain(..overflow);
        }
        id
    }
}

/// The `StatementState` a statement is in the instant its run returns: the session has
/// taken it and nothing has begun actioning it yet. One of the modelled `StatementState`
/// values, whose published list the generated
/// [`crate::ops::types::STATEMENT_STATE_VALUES`] carries (`WAITING`, `RUNNING`,
/// `AVAILABLE`, `CANCELLING`, `CANCELLED`, `ERROR`).
pub const STATEMENT_STATE_WAITING: &str = "WAITING";

/// The `StatementState` of a statement whose code ran and whose output is readable. See
/// [`STATEMENT_STATE_WAITING`].
pub const STATEMENT_STATE_AVAILABLE: &str = "AVAILABLE";

/// The `StatementState` of a statement a cancel stopped before it ran. See
/// [`STATEMENT_STATE_WAITING`].
pub const STATEMENT_STATE_CANCELLED: &str = "CANCELLED";

/// The `StatementState` of a statement that ended without producing output. See
/// [`STATEMENT_STATE_WAITING`].
pub const STATEMENT_STATE_ERROR: &str = "ERROR";

/// The `StatementState` values a statement never moves out of again: of the modelled
/// enum quoted on [`STATEMENT_STATE_WAITING`], the ones that name a finished statement.
/// The other three (`WAITING`, `RUNNING`, `CANCELLING`) each still owe a further state.
pub const TERMINAL_STATEMENT_STATES: [&str; 3] = [
    STATEMENT_STATE_AVAILABLE,
    STATEMENT_STATE_CANCELLED,
    STATEMENT_STATE_ERROR,
];

/// How long a statement stays in the state its run put it in before the clock owes it its
/// terminal one.
///
/// CHOSEN, not documented: Glue publishes no duration for actioning a statement, so this
/// is the emulator's own span, the same one a session's provisioning takes
/// ([`SESSION_SETTLES_AFTER`]) — deliberately not zero, so a caller that runs a second
/// statement straight after the first still observes the session busy. The due instant is
/// stored on the record and materialised by whichever later call reads it; nothing sleeps.
pub const STATEMENT_SETTLES_AFTER: Duration = SESSION_SETTLES_AFTER;

/// `StatementOutput$ErrorValue` for the terminal state the clock owes every statement.
///
/// This emulator is the Glue control plane: it records a statement faithfully, but it runs
/// no Spark runtime, so a statement can never reach [`STATEMENT_STATE_AVAILABLE`] — it
/// ends in the modelled error state carrying a reason that says exactly that, rather than
/// reporting output nothing computed. The same reading as
/// [`JOB_RUN_NOT_EXECUTED_MESSAGE`].
pub const STATEMENT_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records statements but runs no Spark runtime, so the statement ended \
     without running.";

/// `StatementOutput$ErrorName` for that state: the kind of the error above, which is that
/// nothing was available to run the code on.
pub const STATEMENT_NOT_EXECUTED_ERROR_NAME: &str = "RuntimeUnavailable";

/// The `Id` the first statement of a session is given, which a session's ids then count
/// up from.
pub const FIRST_STATEMENT_ID: i32 = 1;

/// How many statements one session keeps.
///
/// CHOSEN: Glue publishes no retention for statement records, so this is the emulator's
/// own window, for the reason [`MAX_RUNS_PER_JOB`] gives: it bounds
/// [`SessionEntry::statements`], and with it every per-session walk. The OLDEST record is
/// dropped first, so the statements a caller is most likely to read are kept.
pub const MAX_STATEMENTS_PER_SESSION: usize = 100;

/// One statement run on a session: "the statement or request for a particular action to
/// occur in a session" (`Statement`), as the run that submitted it recorded it.
///
/// Everything a caller can observe through the modelled `Statement` shape is held here —
/// the id the session minted for it, the code it was submitted with, the state it has
/// reached and the instants it moved between.
#[derive(Debug, Clone)]
pub struct StatementEntry {
    /// `Statement$Id`: "The ID of the statement", minted by
    /// [`SessionEntry::run_statement`].
    pub id: i32,
    /// `Statement$Code`: "The execution code of the statement", exactly as the run sent
    /// it.
    pub code: String,
    /// One of the modelled `StatementState` values: "The state while request is actioned."
    pub state: String,
    /// `Statement$StartedOn`: the instant the run recorded this statement.
    pub started_on: SystemTime,
    /// `Statement$CompletedOn`: the instant it reached a terminal state, and `None` until
    /// it has.
    pub completed_on: Option<SystemTime>,
    /// The instant the clock owes this statement its next state; `None` once it has
    /// settled.
    pub settles_at: Option<SystemTime>,
    /// The `StatementState` the clock owes this statement at [`Self::settles_at`].
    pub settles_to: String,
}

impl StatementEntry {
    /// Record a statement submitted at `now` under `id`, in [`STATEMENT_STATE_WAITING`]
    /// with its terminal state due [`STATEMENT_SETTLES_AFTER`] out.
    pub fn run(id: i32, code: &str, now: SystemTime) -> Self {
        Self {
            id,
            code: code.to_string(),
            state: STATEMENT_STATE_WAITING.to_string(),
            started_on: now,
            completed_on: None,
            settles_at: Some(now.checked_add(STATEMENT_SETTLES_AFTER).unwrap_or(now)),
            // Nothing runs the code here, so the state the clock owes a statement nobody
            // cancels is the modelled error state.
            settles_to: STATEMENT_STATE_ERROR.to_string(),
        }
    }

    /// Whether this statement has finished, by [`TERMINAL_STATEMENT_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATEMENT_STATES.contains(&self.state.as_str())
    }

    /// Move this statement into the state the clock owed it at `now`, if it owed one.
    ///
    /// The instants recorded are the DUE instant, not the instant somebody read it, so
    /// two reads of a settled statement report the same record.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at.filter(|due| *due <= now) else {
            return;
        };
        self.settles_at = None;
        self.state = self.settles_to.clone();
        if self.is_terminal() {
            self.completed_on = Some(due);
        }
    }

    /// `Statement$Output`: "The output in JSON", which a statement has once it has
    /// finished — and which, for one nothing ran, is the modelled error output carrying
    /// [`STATEMENT_NOT_EXECUTED_MESSAGE`] rather than data nothing computed.
    fn output(&self) -> Option<StatementOutput> {
        if !self.is_terminal() {
            return None;
        }
        let errored = self.state == STATEMENT_STATE_ERROR;
        Some(StatementOutput {
            // "The code execution output": there is none, from a statement that ran no
            // code.
            data: None,
            // "The execution count of the output": nothing executed, so this is the
            // model's own default.
            execution_count: 0,
            // "The status of the code execution output", which is the state the statement
            // came to rest in.
            status: Some(self.state.clone()),
            error_name: errored.then(|| STATEMENT_NOT_EXECUTED_ERROR_NAME.to_string()),
            error_value: errored.then(|| STATEMENT_NOT_EXECUTED_MESSAGE.to_string()),
            // "The traceback of the output": a Python traceback, which only code that ran
            // has.
            traceback: None,
        })
    }

    /// Rebuild the modelled `Statement` shape from what is stored, so every reader of a
    /// statement reports the same record.
    pub fn to_statement(&self) -> Statement {
        Statement {
            id: self.id,
            code: Some(self.code.clone()),
            state: Some(self.state.clone()),
            output: self.output(),
            // "The code execution progress", which is the model's own default for a
            // statement whose code nothing ran.
            progress: 0.0,
            started_on: to_unix_millis(self.started_on),
            // The model's own default of 0 while the statement has not completed.
            completed_on: self.completed_on.map(to_unix_millis).unwrap_or_default(),
        }
    }
}

/// The `BlueprintStatus` a blueprint is in the instant its registration is recorded:
/// "Creating — The blueprint registration is in progress" (`Blueprint$Status`). One of the
/// modelled `BlueprintStatus` values, whose published list the generated
/// [`crate::ops::types::BLUEPRINT_STATUS_VALUES`] carries (`ACTIVE`, `CREATING`, `FAILED`,
/// `UPDATING`).
pub const BLUEPRINT_STATUS_CREATING: &str = "CREATING";

/// The `BlueprintStatus` of a blueprint whose registration has finished: "Active — The
/// blueprint has been successfully registered." See [`BLUEPRINT_STATUS_CREATING`].
pub const BLUEPRINT_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a blueprint stays in the status its registration put it in before the clock
/// owes it [`BLUEPRINT_STATUS_ACTIVE`].
///
/// CHOSEN, not documented: Glue publishes no duration for a blueprint registration, so this
/// is the emulator's own span — deliberately not zero, so a caller that reads a blueprint
/// straight after registering it still observes the registration in progress. The due
/// instant is stored on the record and materialised by whichever later call reads it;
/// nothing sleeps.
pub const BLUEPRINT_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// A stored blueprint: the reusable workflow template a blueprint create registers.
///
/// Everything a caller of this record can observe through the modelled `Blueprint` shape is
/// held here — the name it is addressed by, where its archive is published, its
/// description, when it was registered and the status of that registration — plus the
/// `Tags` a create carried, which is what `ListBlueprints` narrows its answer by.
#[derive(Debug, Clone)]
pub struct BlueprintEntry {
    /// The blueprint's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::blueprints`].
    pub name: String,
    /// `Blueprint$BlueprintLocation`: "Specifies the path in Amazon S3 where the blueprint
    /// is published", exactly as the registration sent it.
    pub blueprint_location: String,
    /// `Blueprint$Description`: "The description of the blueprint", and `None` when the
    /// registration carried none.
    pub description: Option<String>,
    /// `Blueprint$CreatedOn`: "The date and time the blueprint was registered."
    pub created_on: SystemTime,
    /// `Blueprint$LastModifiedOn`: "The date and time the blueprint was last modified",
    /// which for a blueprint nothing has updated is the instant it was registered.
    pub last_modified_on: SystemTime,
    /// One of the modelled `BlueprintStatus` values: the status of this blueprint's
    /// registration.
    pub status: String,
    /// The instant the clock owes this blueprint its next status; `None` once it has
    /// settled.
    pub settles_at: Option<SystemTime>,
    /// The tags the blueprint was registered with, which the modelled `Blueprint` shape
    /// does not carry.
    ///
    /// Kept because `ListBlueprints` "filters the list by an Amazon Web Services resource
    /// tag", so they decide which blueprints a filtered listing reports. NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`]
    /// gives: [`TaggedResourceKind`] carries no blueprint variant yet.
    pub tags: HashMap<String, String>,
    /// The runs `StartBlueprintRun` started of this blueprint, oldest first and bounded by
    /// [`MAX_RUNS_PER_BLUEPRINT`].
    ///
    /// A run lives on its blueprint because every operation that addresses one names the
    /// blueprint too: `GetBlueprintRun` takes a `BlueprintName` beside the `RunId`, exactly
    /// as `GetJobRun` takes a `JobName` beside its run id.
    pub runs: Vec<BlueprintRunEntry>,
}

impl BlueprintEntry {
    /// Record the blueprint a `CreateBlueprint` describes, in
    /// [`BLUEPRINT_STATUS_CREATING`] with [`BLUEPRINT_STATUS_ACTIVE`] due
    /// [`BLUEPRINT_SETTLES_AFTER`] out — "Creating — The blueprint registration is in
    /// progress" — and every other member taken from the request, which carries the whole
    /// registration.
    pub fn from_create_input(input: &CreateBlueprintInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            blueprint_location: input.blueprint_location.clone(),
            description: input.description.clone(),
            created_on: now,
            last_modified_on: now,
            status: BLUEPRINT_STATUS_CREATING.to_string(),
            settles_at: Some(now.checked_add(BLUEPRINT_SETTLES_AFTER).unwrap_or(now)),
            tags: input.tags.clone().unwrap_or_default(),
            // A blueprint a registration wrote has not been run.
            runs: Vec::new(),
        }
    }

    /// Move this blueprint into the status the clock owed it at `now`, if it owed one.
    ///
    /// The registration was due to finish at [`Self::settles_at`] whether or not anybody
    /// called, so materialising it on a read reports the status the clock had already
    /// reached, and two reads of a settled blueprint report the same record.
    /// [`Self::last_modified_on`] does not move with it: the registration finishing is the
    /// create completing, not a later modification of the blueprint.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = BLUEPRINT_STATUS_ACTIVE.to_string();
    }

    /// Whether this blueprint's registration has finished, i.e. is in
    /// [`BLUEPRINT_STATUS_ACTIVE`]: "Active — The blueprint has been successfully
    /// registered" (`Blueprint$Status`).
    ///
    /// Call [`Self::settle_if_due`] first, or a registration the clock has already finished
    /// still reports as being in progress.
    pub fn is_active(&self) -> bool {
        self.status == BLUEPRINT_STATUS_ACTIVE
    }

    /// Give every run of this blueprint the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs, exactly as
    /// [`WorkflowEntry::settle_due_runs`] is for a workflow run: a run's next state was due
    /// at [`BlueprintRunEntry::settles_at`] whether or not anybody called, so materialising
    /// it on read reports the state the clock had already reached.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// The stored run with this id, or `None` when this blueprint has no such run.
    pub fn run(&self, run_id: &str) -> Option<&BlueprintRunEntry> {
        self.runs.iter().find(|run| run.run_id == run_id)
    }

    /// Record the run `run_id` of this blueprint, as the start that minted that id describes
    /// it, at `now`.
    ///
    /// Everything the clock already owed this blueprint's runs is materialised first, so a
    /// read of any of them reports the state the clock had already reached; a run already in
    /// flight does not stop another being recorded, because a blueprint publishes no ceiling
    /// on its concurrent runs. The id is minted by the start that lays the run's workflow out
    /// ([`ComAmazonawsGlueAwsglueState::start_blueprint_run`]), because the workflow a run
    /// creates carries the id of the run that created it.
    pub fn begin_run(
        &mut self,
        input: &StartBlueprintRunInput,
        run_id: String,
        now: SystemTime,
        layout: &BlueprintRunLayout,
    ) {
        self.settle_due_runs(now);
        self.record_run(BlueprintRunEntry::begin(input, run_id, now, layout));
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_BLUEPRINT`] of them.
    pub fn record_run(&mut self, run: BlueprintRunEntry) {
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_BLUEPRINT {
            let overflow = self.runs.len() - MAX_RUNS_PER_BLUEPRINT;
            self.runs.drain(..overflow);
        }
    }

    /// Rebuild the modelled `Blueprint` shape from what is stored, so every reader of a
    /// blueprint reports the same registration.
    pub fn to_blueprint(&self) -> Blueprint {
        Blueprint {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            // "A JSON string that indicates the list of parameter specifications for the
            // blueprint": Glue reads that list out of the blueprint ARCHIVE published at
            // `BlueprintLocation`. Nothing here reads that archive, so there is no
            // parameter list to report rather than one invented for it.
            parameter_spec: None,
            blueprint_location: Some(self.blueprint_location.clone()),
            // "Specifies a path in Amazon S3 where the blueprint is copied when you call
            // CreateBlueprint/UpdateBlueprint to register the blueprint in Glue": nothing
            // here copies the archive, so there is no such path a caller could read.
            blueprint_service_location: None,
            status: Some(self.status.clone()),
            // "An error message": a registration this store accepted has none.
            error_message: None,
            // "When there are multiple versions of a blueprint and the latest version has
            // some errors, this attribute indicates the last successful blueprint
            // definition": the registration stored here is itself the latest and has no
            // error, so there is no earlier definition to fall back to.
            last_active_definition: None,
        }
    }
}

/// The `BlueprintRunState` a run is in the instant `StartBlueprintRun` returns: "Running —
/// The blueprint run is in progress" (`BlueprintRun$State`). One of the modelled
/// `BlueprintRunState` values (`RUNNING`, `SUCCEEDED`, `FAILED`, `ROLLING_BACK`).
pub const BLUEPRINT_RUN_STATE_RUNNING: &str = "RUNNING";

/// The `BlueprintRunState` of a run that created the workflow it lays out: "Succeeded — The
/// blueprint run completed successfully" (`BlueprintRun$State`). See
/// [`BLUEPRINT_RUN_STATE_RUNNING`].
pub const BLUEPRINT_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";

/// The `BlueprintRunState` of a run that created no workflow: "Failed — The blueprint run
/// failed and rollback is complete" (`BlueprintRun$State`). See
/// [`BLUEPRINT_RUN_STATE_RUNNING`].
pub const BLUEPRINT_RUN_STATE_FAILED: &str = "FAILED";

/// The `BlueprintRunState` values a run never moves out of again: of the modelled enum
/// quoted on [`BLUEPRINT_RUN_STATE_RUNNING`], the ones that name a finished run. The other
/// two (`RUNNING`, `ROLLING_BACK`) each still owe a further state.
pub const TERMINAL_BLUEPRINT_RUN_STATES: [&str; 2] =
    [BLUEPRINT_RUN_STATE_SUCCEEDED, BLUEPRINT_RUN_STATE_FAILED];

/// How long a blueprint run stays in the state the call that started it put it in before
/// the clock owes it the next one.
///
/// CHOSEN, not documented: Glue publishes no duration for a blueprint run, so this is the
/// emulator's own span, the same one a blueprint registration takes
/// ([`BLUEPRINT_SETTLES_AFTER`]) — deliberately not zero, so a caller that reads the run
/// straight after starting it still observes it in progress. The due instant is stored on the
/// run and materialised by whichever later call reads it; nothing sleeps.
pub const BLUEPRINT_RUN_SETTLES_AFTER: Duration = BLUEPRINT_SETTLES_AFTER;

/// The key of the `Parameters` member that names the workflow a blueprint run lays out.
///
/// "The blueprint parameters as a string. You will have to provide a value for each key that
/// is required from the parameter spec that is defined in the `Blueprint$ParameterSpec`"
/// (`BlueprintRun$Parameters`): the spec itself lives in the archive published at the
/// blueprint's location, and the one parameter Glue's blueprint programming model has every
/// blueprint take from its caller is the name to give the workflow it creates, since that
/// workflow is what the run produces (`BlueprintRun$WorkflowName`). A start whose parameters
/// do not carry it names no workflow of its own ([`blueprint_run_workflow_name`]).
pub const BLUEPRINT_WORKFLOW_NAME_PARAMETER: &str = "WorkflowName";

/// The name of the workflow a run of `blueprint_name` lays out, given the `Parameters` the
/// start carried.
///
/// The caller's own [`BLUEPRINT_WORKFLOW_NAME_PARAMETER`] when its parameters are a JSON
/// object carrying that key with a string value a workflow can be named by — `NameString`,
/// "len 1..255" — because that is the name the caller asked the run to create the workflow
/// under.
///
/// Otherwise the BLUEPRINT's own name, which is CHOSEN: the layout a run performs is defined
/// by the published archive, which nothing here reads, so when the caller names no workflow
/// the only name this store knows for what the run produces is the blueprint's. A blueprint
/// name is inside `NameString`'s bounds, so the workflow a run creates always has a name
/// that can be addressed.
pub fn blueprint_run_workflow_name(blueprint_name: &str, parameters: Option<&str>) -> String {
    parameters
        .and_then(|parameters| serde_json::from_str::<serde_json::Value>(parameters).ok())
        .and_then(|parameters| {
            parameters
                .get(BLUEPRINT_WORKFLOW_NAME_PARAMETER)
                .and_then(|named| named.as_str())
                .map(str::to_string)
        })
        .filter(|named| (1..=MAX_WORKFLOW_NAME_LENGTH).contains(&named.chars().count()))
        .unwrap_or_else(|| blueprint_name.to_string())
}

/// The longest workflow name the model admits: `NameString`, "len 1..255", which is what
/// `CreateWorkflowRequest$Name` and every workflow read address a workflow by.
const MAX_WORKFLOW_NAME_LENGTH: usize = 255;

/// What a blueprint run's layout of the workflow it produces came to.
///
/// A run exists to create a workflow — "The name of a workflow that is created as a result of
/// a successful blueprint run" (`BlueprintRun$WorkflowName`) — so what the run becomes is
/// decided by whether it could create one: it succeeds carrying the workflow it created, and
/// otherwise fails carrying the reason, because "if a blueprint run has an error, there will
/// not be a workflow created".
#[derive(Debug, Clone)]
pub enum BlueprintRunLayout {
    /// The run created the workflow of this name, which is the workflow it produced.
    Created(String),
    /// This account already held a workflow of this name, so the run created none and
    /// nothing already stored was overwritten.
    WorkflowAlreadyExists(String),
}

/// Why no run of a blueprint could be started
/// ([`ComAmazonawsGlueAwsglueState::start_blueprint_run`]).
///
/// Each variant is one of `StartBlueprintRun`'s own modelled failures, carrying what the
/// refusal has to name; the handler words them, so the store answers WHAT happened rather
/// than how it is reported.
#[derive(Debug, Clone)]
pub enum BlueprintRunNotStarted {
    /// This account holds no blueprint under the name the start addressed.
    NoSuchBlueprint,
    /// The blueprint has no ACTIVE definition to run: its registration has not finished, or
    /// finished without one.
    NoActiveDefinition,
    /// The store already holds as many workflows as it will ([`MAX_WORKFLOWS`]), so the
    /// workflow the run would create cannot be written.
    WorkflowCeilingReached(usize),
}

/// How many runs one blueprint keeps.
///
/// CHOSEN: Glue publishes no retention for blueprint-run records, so this is the emulator's
/// own window, for the reason [`MAX_RUNS_PER_JOB`] gives. The OLDEST record is dropped
/// first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_BLUEPRINT: usize = 100;

/// How many blueprint run ids this process has minted. A nonce source for
/// [`new_blueprint_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job run id.
static MINTED_BLUEPRINT_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words a blueprint run id carries: two, i.e. 128 bits rendered as 32
/// lowercase hexadecimal characters.
const BLUEPRINT_RUN_ID_WORDS: usize = 2;

/// A fresh `RunId`: the id the service assigns a blueprint run when it starts.
///
/// 32 lowercase hexadecimal characters from [`mint_hex_id`], which is inside `IdString`'s
/// `length 1..255` and satisfies its pattern. Glue documents no prefix for a blueprint run
/// id — unlike a job run's `jr_` or a workflow run's `wr_` — so this one carries none. It is
/// stored on the run, so every later read of that run reports the same id, and two runs never
/// share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_blueprint_run_id() -> String {
    mint_hex_id(&MINTED_BLUEPRINT_RUN_IDS, BLUEPRINT_RUN_ID_WORDS)
}

/// One run of a blueprint, as the `StartBlueprintRun` that began it recorded it: "The
/// details of a blueprint run" (`BlueprintRun`).
///
/// Everything the modelled `BlueprintRun` shape reports is held here: the blueprint the run
/// is of and the id that addresses it within that blueprint, the `Parameters` and `RoleArn`
/// the start carried, and the state the run is in with the instants it moved between.
#[derive(Debug, Clone)]
pub struct BlueprintRunEntry {
    /// `BlueprintRun$RunId`, minted by [`new_blueprint_run_id`].
    pub run_id: String,
    /// `BlueprintRun$BlueprintName`: the blueprint this is a run of, which is also where the
    /// record lives ([`BlueprintEntry::runs`]).
    pub blueprint_name: String,
    /// `BlueprintRun$Parameters`: "The blueprint parameters as a string", exactly as the
    /// start sent them, and `None` for a start that carried none.
    pub parameters: Option<String>,
    /// `BlueprintRun$RoleArn`: "The role ARN. This role will be assumed by the Glue service
    /// and will be used to create the workflow and other entities of a workflow", which the
    /// start is `@required` to carry.
    pub role_arn: String,
    /// `BlueprintRun$StartedOn`: "The date and time that the blueprint run started."
    pub started_on: SystemTime,
    /// `BlueprintRun$CompletedOn`: "The date and time that the blueprint run completed",
    /// server-set once the run reaches a terminal state and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `BlueprintRunState` values: the state this run is in.
    pub state: String,
    /// The instant the clock owes this run its next state; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The `BlueprintRunState` the clock owes this run at [`Self::settles_at`].
    pub settles_to: String,
    /// `BlueprintRun$ErrorMessage`: "Indicates any errors that are seen while running the
    /// blueprint", which a run carries only once something has gone wrong with it.
    pub error_message: Option<String>,
    /// The `ErrorMessage` the clock gives this run when it moves it into
    /// [`Self::settles_to`], and `None` for a run whose layout went through.
    pub settles_with_error: Option<String>,
    /// `BlueprintRun$WorkflowName`: "The name of a workflow that is created as a result of a
    /// successful blueprint run", and `None` for a run that created none — "if a blueprint
    /// run has an error, there will not be a workflow created".
    pub workflow_name: Option<String>,
}

impl BlueprintRunEntry {
    /// Record the run a `StartBlueprintRun` describes, in [`BLUEPRINT_RUN_STATE_RUNNING`]
    /// with its next state due [`BLUEPRINT_RUN_SETTLES_AFTER`] out — "Running — The
    /// blueprint run is in progress" — and the members the start carried taken from the
    /// request.
    ///
    /// `layout` is what the run's layout of the workflow it produces came to, which is what
    /// decides the state the clock owes the run: the run that created a workflow succeeds
    /// carrying it, and one that created none fails carrying why.
    pub fn begin(
        input: &StartBlueprintRunInput,
        run_id: String,
        now: SystemTime,
        layout: &BlueprintRunLayout,
    ) -> Self {
        let (settles_to, workflow_name, settles_with_error) = match layout {
            BlueprintRunLayout::Created(workflow_name) => (
                BLUEPRINT_RUN_STATE_SUCCEEDED,
                Some(workflow_name.clone()),
                None,
            ),
            BlueprintRunLayout::WorkflowAlreadyExists(workflow_name) => (
                BLUEPRINT_RUN_STATE_FAILED,
                None,
                Some(messages::blueprint_run_workflow_exists(workflow_name)),
            ),
        };
        Self {
            run_id,
            blueprint_name: input.blueprint_name.clone(),
            parameters: input.parameters.clone(),
            role_arn: input.role_arn.clone(),
            started_on: now,
            completed_on: None,
            state: BLUEPRINT_RUN_STATE_RUNNING.to_string(),
            settles_at: Some(now.checked_add(BLUEPRINT_RUN_SETTLES_AFTER).unwrap_or(now)),
            settles_to: settles_to.to_string(),
            // Nothing has gone wrong yet: a run carries an error message only once it has.
            error_message: None,
            settles_with_error,
            workflow_name,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_BLUEPRINT_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_BLUEPRINT_RUN_STATES.contains(&self.state.as_str())
    }

    /// Move this run into the state the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave the state the start put it in at [`Self::settles_at`]
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
        // "Indicates any errors that are seen while running the blueprint": the run that
        // comes to rest without having created a workflow carries the reason it did not.
        self.error_message = self.settles_with_error.clone();
        if self.is_terminal() {
            self.completed_on = Some(due);
        }
    }

    /// Rebuild the modelled `BlueprintRun` shape from what is stored, so every reader of a
    /// run reports the same run.
    pub fn to_blueprint_run(&self) -> BlueprintRun {
        BlueprintRun {
            blueprint_name: Some(self.blueprint_name.clone()),
            run_id: Some(self.run_id.clone()),
            // "The name of a workflow that is created as a result of a successful blueprint
            // run": the workflow the run laid out, which exists from the instant the run
            // created it, and nothing at all for a run that created none — "if a blueprint
            // run has an error, there will not be a workflow created".
            workflow_name: self.workflow_name.clone(),
            state: Some(self.state.clone()),
            started_on: Some(to_date_time(self.started_on)),
            completed_on: self.completed_on.map(to_date_time),
            error_message: self.error_message.clone(),
            // "If there are any errors while creating the entities of a workflow, we try to
            // roll back the created entities until that point and delete them": a run that
            // created no workflow created no entity either, so nothing was rolled back and no
            // rollback error arose.
            rollback_error_message: None,
            parameters: self.parameters.clone(),
            role_arn: Some(self.role_arn.clone()),
        }
    }
}

/// How many integration resource property ids this process has minted.
///
/// Not catalog state — the id itself is what gets stored, on the
/// [`IntegrationResourcePropertyEntry`]. This is only a nonce source for
/// [`new_integration_resource_property_id`], so that two creates landing in the same
/// nanosecond are still given different input to mint from.
static MINTED_INTEGRATION_RESOURCE_PROPERTY_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh id for an integration resource property record: the name its own ARN carries.
///
/// 32 lowercase hexadecimal characters from [`mint_hex_id`], the width every other id this
/// crate mints has. It is minted ONCE, when the first create for a resource stores its
/// record, and kept on that record — so every later read, and every later create for the
/// same resource, reports the same `ResourcePropertyArn`, and two resources never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_integration_resource_property_id() -> String {
    mint_hex_id(&MINTED_INTEGRATION_RESOURCE_PROPERTY_IDS, CATALOG_ID_WORDS)
}

/// The ARN resource-type segment of an integration resource property record, the way the
/// model spells it: "The format is something like arn:aws:glue:::integrationresourceproperty/*"
/// (`CreateIntegrationResourcePropertyResponse$ResourcePropertyArn`).
pub const INTEGRATION_RESOURCE_PROPERTY_ARN_SEGMENT: &str = "integrationresourceproperty";

/// The ARN of the integration resource property record `property_id`, as served in
/// `region`.
pub fn integration_resource_property_arn(region: &str, property_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{INTEGRATION_RESOURCE_PROPERTY_ARN_SEGMENT}/{property_id}"),
    )
}

/// The Glue resource an integration property call's `ResourceArn` names, or
/// `Err` with the message a refusal carries when the value cannot name one.
///
/// `ResourceArn` is documented as "The connection ARN of the source, or the database ARN of
/// the target", so a value that is not the ARN of a Glue connection or a Glue database
/// names neither and is a bad parameter. The ONE resolution of that member, so the create
/// and every read of a record agree on which values address a resource at all.
///
/// Shared by the integration TABLE property operations, whose `ResourceArn` names the same
/// two legs of an integration: "the same API need to be invoked with the Glue connection ARN
/// as ResourceArn with SourceTableConfig, and the Glue database ARN as ResourceArn with
/// TargetTableConfig respectively" (`CreateIntegrationTableProperties`). A catalog TABLE's
/// own ARN is one of the values that names neither leg.
///
/// Only the KIND and the identity fields are read here; whether this store holds the
/// resource is [`ComAmazonawsGlueAwsglueState::resource_tags`]'s answer, exactly as for a
/// tagging call's `ResourceArn`.
pub fn integration_resource_reference(resource_arn: &str) -> Result<TaggedResource, String> {
    match parse_tagged_resource(resource_arn) {
        Some(resource)
            if matches!(
                resource.kind,
                TaggedResourceKind::Connection | TaggedResourceKind::Database
            ) =>
        {
            Ok(resource)
        }
        _ => Err(format!(
            "ResourceArn {resource_arn:?} is not the ARN of a Glue connection or a Glue database"
        )),
    }
}

/// The processing properties one resource of an integration carries: the record
/// `CreateIntegrationResourceProperty` writes for a Glue connection (the source) or a Glue
/// database (the target).
///
/// Every member of the modelled `IntegrationResourceProperty` shape is held here, plus the
/// create's `Tags`, which that shape does not carry. The id is the one server-set member:
/// it is minted when the first create for a resource stores its record and never changes,
/// so the `ResourcePropertyArn` a client reads is stable across reads and across a later
/// create for the same resource.
#[derive(Debug, Clone)]
pub struct IntegrationResourcePropertyEntry {
    /// The ARN of the resource these properties belong to, which is also this record's key
    /// in [`ComAmazonawsGlueAwsglueState::integration_resource_properties`].
    pub resource_arn: String,
    /// The name this record's own ARN carries ([`integration_resource_property_arn`]),
    /// minted once by [`new_integration_resource_property_id`].
    pub property_id: String,
    /// "The resource properties associated with the integration source", as the last write
    /// that stated them sent them, or `None` while no write has.
    pub source_processing_properties: Option<SourceProcessingProperties>,
    /// "The resource properties associated with the integration target", held the same way.
    pub target_processing_properties: Option<TargetProcessingProperties>,
    /// The create's `Tags`, which the `IntegrationResourceProperty` shape has no member for.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`] gives:
    /// [`TaggedResourceKind`] carries no integration resource property variant.
    pub tags: Vec<Tag>,
}

impl IntegrationResourcePropertyEntry {
    /// The record a first `CreateIntegrationResourceProperty` for `resource_arn` stores:
    /// the properties the request carried, under a freshly minted id.
    pub fn from_create_input(input: &CreateIntegrationResourcePropertyInput) -> Self {
        Self {
            resource_arn: input.resource_arn.clone(),
            property_id: new_integration_resource_property_id(),
            source_processing_properties: input.source_processing_properties.clone(),
            target_processing_properties: input.target_processing_properties.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Apply a create for a resource this store ALREADY holds a record for: each member the
    /// request states replaces the stored one, and a member it does not state is left as
    /// stored.
    ///
    /// The source and the target properties of one integration are written by separate
    /// calls — "To set both source and target properties the same API needs to be invoked
    /// with the Glue connection ARN as ResourceArn with SourceProcessingProperties and the
    /// Glue database ARN as ResourceArn with TargetProcessingProperties respectively" — so a
    /// write that states one side says nothing about the other and must not erase it. The id
    /// is not touched: the record is the same resource's, so its ARN stays what the first
    /// create minted.
    pub fn apply_create(&mut self, input: &CreateIntegrationResourcePropertyInput) {
        if input.source_processing_properties.is_some() {
            self.source_processing_properties = input.source_processing_properties.clone();
        }
        if input.target_processing_properties.is_some() {
            self.target_processing_properties = input.target_processing_properties.clone();
        }
        if let Some(tags) = input.tags.clone() {
            self.tags = tags;
        }
    }

    /// This record's own ARN, as served in `region`.
    pub fn arn(&self, region: &str) -> String {
        integration_resource_property_arn(region, &self.property_id)
    }
}

/// The ARN resource-type segment of a Data Catalog table, whose resource portion carries
/// the table's database as well as its own name: `table/<database>/<table>`.
///
/// Not in [`TAGGED_RESOURCE_TYPES`] because a table carries no tag set of its own
/// (`CreateTable` models no `Tags` member), but it IS a value an integration table
/// property's `ResourceArn` may carry: "the provided ARN should be the ARN of the target
/// table in the Glue Data Catalog" (`CreateIntegrationTablePropertiesRequest$ResourceArn`).
pub const TABLE_ARN_SEGMENT: &str = "table";

/// The Glue resource an integration table property call's `ResourceArn` names.
///
/// Either the LEG of an integration whose table properties the call writes — the Glue
/// connection of the source or the Glue database of the target ("the same API need to be
/// invoked with the Glue connection ARN as ResourceArn with SourceTableConfig, and the Glue
/// database ARN as ResourceArn with TargetTableConfig respectively") — or a replicated
/// table's own ARN, which `CreateIntegrationTableProperties` also takes ("the provided ARN
/// should be the ARN of the target table in the Glue Data Catalog").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationTableResource {
    /// A Glue connection or a Glue database, resolved the way every call that names one by
    /// ARN resolves it ([`parse_tagged_resource`]).
    Tagged(TaggedResource),
    /// A Data Catalog table, whose ARN names its database as well as itself.
    Table {
        /// Field 5 of the ARN, which for a catalog resource is the `CatalogId` it lives
        /// under.
        account_id: String,
        /// The folded database name, the way the catalog keys it.
        database: String,
        /// The folded table name, the same way.
        table: String,
    },
}

impl IntegrationTableResource {
    /// Whether this resource is a LEG of an integration, which is what a read of a table
    /// property record addresses.
    ///
    /// Recorded behaviour: a create against a replicated table's own ARN is accepted, and
    /// `GetIntegrationTableProperties` for that same ARN is then refused — the read resolves
    /// its `ResourceArn` to the connection or the database an integration runs between, and a
    /// catalog table is neither.
    pub fn is_integration_leg(&self) -> bool {
        matches!(self, Self::Tagged(_))
    }
}

/// The Glue resource `resource_arn` names, or `Err` with the message a refusal carries when
/// the value can name no resource an integration table property could belong to.
///
/// The ONE resolution of that member, so the create and every read of a record agree on
/// which values address a resource at all. Only the KIND and the identity fields are read
/// here; whether this store holds the resource is
/// [`ComAmazonawsGlueAwsglueState::holds_integration_table_resource`]'s answer, exactly as a
/// tagging call's `ResourceArn` splits those two questions.
pub fn integration_table_reference(resource_arn: &str) -> Result<IntegrationTableResource, String> {
    if let Some(resource) = parse_tagged_resource(resource_arn) {
        if matches!(
            resource.kind,
            TaggedResourceKind::Connection | TaggedResourceKind::Database
        ) {
            return Ok(IntegrationTableResource::Tagged(resource));
        }
    }
    if let Some(parsed) = parse_glue_arn(resource_arn) {
        if let Some((database, table)) = parsed
            .resource_of_type(TABLE_ARN_SEGMENT)
            .and_then(|rest| rest.split_once('/'))
        {
            if !database.is_empty() && !table.is_empty() && !table.contains('/') {
                return Ok(IntegrationTableResource::Table {
                    account_id: parsed.account_id.clone(),
                    database: fold_name(database),
                    table: fold_name(table),
                });
            }
        }
    }
    Err(format!(
        "ResourceArn {resource_arn:?} is not the ARN of a Glue connection, a Glue database or a \
         Glue Data Catalog table"
    ))
}

/// The `SourceTableConfig` members the service does not process yet, named as the model
/// spells them.
///
/// Source: the three members the model documents as "Currently unsupported" — `Fields` ("A
/// list of fields used for column-level filtering. Currently unsupported."),
/// `FilterPredicate` ("A condition clause used for row-level filtering. Currently
/// unsupported.") and `RecordUpdateField` ("Incremental pull timestamp-based field.
/// Currently unsupported."). `PrimaryKey` is NOT one of them: it is "Currently supported
/// specifically for SAP EntityOf entities upon request".
pub const UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS: [&str; 3] =
    ["Fields", "FilterPredicate", "RecordUpdateField"];

/// Whether a source table configuration states any member of
/// [`UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS`], which is what makes a write of it a request
/// for processing the source leg does not do.
///
/// The ONE reading of that rule, so the create and any later write of a record refuse the
/// same configurations.
pub fn states_unsupported_source_table_properties(config: &SourceTableConfig) -> bool {
    config.fields.is_some()
        || config.filter_predicate.is_some()
        || config.record_update_field.is_some()
}

/// The override properties ONE replicated table carries on one leg of an integration: the
/// record `CreateIntegrationTableProperties` writes for a `ResourceArn` and a `TableName`.
///
/// Both modelled sides are held here — the source leg's filtering and partitioning
/// properties and the target leg's — because the two are written by separate calls against
/// separate `ResourceArn`s and read back together by one `GetIntegrationTableProperties`.
/// The record carries no server-set member: the model gives integration table properties no
/// id, ARN, status or timestamp of their own.
#[derive(Debug, Clone)]
pub struct IntegrationTablePropertiesEntry {
    /// The ARN of the integration leg these properties belong to, the first half of this
    /// record's key in
    /// [`ComAmazonawsGlueAwsglueState::integration_table_properties`].
    pub resource_arn: String,
    /// "The name of the table to be replicated", the second half of that key.
    pub table_name: String,
    /// "A structure for the source table configuration", as the last write that stated it
    /// sent it, or `None` while no write has.
    pub source_table_config: Option<SourceTableConfig>,
    /// "A structure for the target table configuration", held the same way.
    pub target_table_config: Option<TargetTableConfig>,
}

impl IntegrationTablePropertiesEntry {
    /// The record a first `CreateIntegrationTableProperties` for a pair stores: the
    /// configuration the request carried, for the resource and table it named.
    pub fn from_create_input(input: &CreateIntegrationTablePropertiesInput) -> Self {
        Self {
            resource_arn: input.resource_arn.clone(),
            table_name: input.table_name.clone(),
            source_table_config: input.source_table_config.clone(),
            target_table_config: input.target_table_config.clone(),
        }
    }

    /// Apply a create for a pair this store ALREADY holds a record for: the side the
    /// request states replaces the stored one, and a side it does not state is left as
    /// stored.
    ///
    /// The source and the target configuration of one table are written by separate calls —
    /// each against the ARN of its own leg — so a write that states one side says nothing
    /// about the other and must not erase it.
    pub fn apply_create(&mut self, input: &CreateIntegrationTablePropertiesInput) {
        if input.source_table_config.is_some() {
            self.source_table_config = input.source_table_config.clone();
        }
        if input.target_table_config.is_some() {
            self.target_table_config = input.target_table_config.clone();
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

/// The separator between the parts of a sorted listing's ordering key (and so of its
/// `NextToken`): the sorted value and then the record's own id.
///
/// ONE copy, shared by the listings that page over a value a `Sort` names (the machine
/// learning transforms and the task runs of one transform), so the token format cannot
/// drift between them. `U+0001` cannot appear in a name, a status, a type or a minted id:
/// every such value's modelled pattern admits `\u0020` and up, so the separator can never
/// be confused with a character inside one of the parts it joins — and a key whose first
/// part is a prefix of another's therefore still orders before it.
const LISTING_KEY_SEPARATOR: char = '\u{1}';

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

/// `at` as seconds since the epoch, the one scale a stored instant is compared against a
/// client-supplied timestamp on.
fn secs_since_epoch(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Whether `at` sits inside the window a filter's `…Before`/`…After` pair describes.
///
/// One reading, shared by every filtered listing in this service: a bound the criteria
/// leave out narrows nothing, and a bound they name admits only an instant STRICTLY inside
/// it — "the time and date before which the … were created" is a window the instant is
/// before, not one it may sit on the edge of.
fn instant_window_admits(
    at: SystemTime,
    before: Option<&::aws_smithy_types::DateTime>,
    after: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let instant = secs_since_epoch(at);
    before.is_none_or(|bound| instant < bound.as_secs_f64())
        && after.is_none_or(|bound| instant > bound.as_secs_f64())
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
    /// The task runs started of this transform, oldest first.
    ///
    /// Held ON the transform, the way a job definition holds its own runs
    /// ([`JobEntry::runs`]): every operation that addresses a task run
    /// (`GetMLTaskRun`, `CancelMLTaskRun`) names the parent `TransformId` and then the
    /// `TaskRunId` the start minted, so a run of one transform is unreachable through
    /// another. Kept in start order, so a listing that names no sort criteria still has
    /// one deterministic order to page over, and bounded by
    /// [`MAX_TASK_RUNS_PER_TRANSFORM`].
    pub task_runs: Vec<MlTaskRunEntry>,
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
        let (worker_type, number_of_workers, max_capacity) = ml_transform_capacity(
            input.worker_type.as_deref(),
            input.number_of_workers,
            input.max_capacity,
        );
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
            // A create starts no task run: "Call this operation as the first step in the
            // process of using a machine learning transform" (`CreateMLTransform`).
            task_runs: Vec::new(),
        }
    }

    /// Apply an `UpdateMLTransform` request to this stored transform, member by member,
    /// as served at `now`.
    ///
    /// Every member the request CARRIES replaces the stored one and every member it OMITS
    /// is preserved: the request is a patch over the definition the create registered, not
    /// a replacement for it, which is why "Call this operation to tune the algorithm
    /// parameters" (`UpdateMLTransform`) names no required member beyond the `TransformId`
    /// that addresses the transform. The three capacity members are resolved TOGETHER,
    /// through the one rule the create uses ([`ml_transform_capacity`]), from the ones the
    /// request names falling back on the ones already stored — so a request that moves the
    /// worker configuration moves the DPU figure with it, and one that names no capacity
    /// member at all leaves the stored figure where it was.
    ///
    /// The members `UpdateMLTransformRequest` has NO member for are not this request's to
    /// change: the minted `TransformId`, the creation instant, the status, the label count,
    /// the input tables and the schema they decide, the encryption settings and the tags
    /// all stay as they are. `LastModifiedOn` becomes `now`, since it is "the last point in
    /// time when this machine learning transform was modified" (`MLTransform`).
    pub fn apply_update(&mut self, input: &UpdateMLTransformInput, now: SystemTime) {
        if let Some(name) = input.name.as_ref() {
            self.name = name.clone();
        }
        if let Some(description) = input.description.as_ref() {
            self.description = Some(description.clone());
        }
        if let Some(parameters) = input.parameters.as_ref() {
            self.parameters = parameters.clone();
        }
        if let Some(role) = input.role.as_ref() {
            self.role = role.clone();
        }
        if let Some(glue_version) = input.glue_version.as_ref() {
            self.glue_version = glue_version.clone();
        }
        if let Some(timeout) = input.timeout {
            self.timeout = timeout;
        }
        if let Some(max_retries) = input.max_retries {
            self.max_retries = Some(max_retries);
        }
        let (worker_type, number_of_workers, max_capacity) = ml_transform_capacity(
            input
                .worker_type
                .as_deref()
                .or(self.worker_type.as_deref()),
            input.number_of_workers.or(self.number_of_workers),
            input.max_capacity.or(self.max_capacity),
        );
        self.worker_type = worker_type;
        self.number_of_workers = number_of_workers;
        self.max_capacity = max_capacity;
        self.last_modified_on = now;
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
        if !instant_window_admits(
            self.created_on,
            filter.created_before.as_ref(),
            filter.created_after.as_ref(),
        ) || !instant_window_admits(
            self.last_modified_on,
            filter.last_modified_before.as_ref(),
            filter.last_modified_after.as_ref(),
        ) {
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
            "{sorted}{LISTING_KEY_SEPARATOR}{}",
            self.transform_id
        )
    }

    /// Give every task run of this transform the status the CLOCK already owed it at
    /// `now`.
    ///
    /// The one write a reporting operation performs, exactly as [`JobEntry::settle_due_runs`]
    /// is for a job run: a run's next status was due at [`MlTaskRunEntry::settles_at`]
    /// whether or not anybody called, so materialising it on read reports the status the
    /// clock had already reached. It never moves a run because something was read.
    pub fn settle_due_task_runs(&mut self, now: SystemTime) {
        for run in &mut self.task_runs {
            run.settle_if_due(now);
        }
    }

    /// Append a task run of this transform, keeping at most
    /// [`MAX_TASK_RUNS_PER_TRANSFORM`] of them.
    pub fn record_task_run(&mut self, run: MlTaskRunEntry) {
        self.task_runs.push(run);
        if self.task_runs.len() > MAX_TASK_RUNS_PER_TRANSFORM {
            let overflow = self.task_runs.len() - MAX_TASK_RUNS_PER_TRANSFORM;
            self.task_runs.drain(..overflow);
        }
    }

    /// The ONE task run of this transform the id `run_id` addresses, if it holds one.
    ///
    /// A run's address is its parent `TransformId` and then the `TaskRunId` the start
    /// minted, so the search is over THIS transform's runs alone and a run of another
    /// transform is unreachable through it. The id is matched exactly, as the store's own
    /// key: the service mints it ([`new_ml_task_run_id`]) rather than taking it from a
    /// request, so no other spelling of it names a run.
    ///
    /// Call [`Self::settle_due_task_runs`] first, or the run is reported in a status the
    /// clock has already moved it out of.
    pub fn task_run(&self, run_id: &str) -> Option<&MlTaskRunEntry> {
        self.task_runs.iter().find(|run| run.id == run_id)
    }

    /// Every task run of this transform `filter` admits, in the order `sort` names, each
    /// paired with its ordering key — which is what the listing's continuation token
    /// carries ([`MlTaskRunEntry::ordering_key`]).
    ///
    /// Call [`Self::settle_due_task_runs`] first, or a run the clock has already finished
    /// is reported, and filtered on, in a status it has already left. Bounded by
    /// [`MAX_TASK_RUNS_PER_TRANSFORM`].
    pub fn task_runs_in_order(
        &self,
        filter: Option<&TaskRunFilterCriteria>,
        sort: Option<&TaskRunSortCriteria>,
    ) -> Vec<(String, &MlTaskRunEntry)> {
        let column = sort.map(|criteria| criteria.column.as_str());
        let mut ordered: Vec<(String, &MlTaskRunEntry)> = self
            .task_runs
            .iter()
            .filter(|run| filter.is_none_or(|criteria| run.matches(criteria)))
            .map(|run| (run.ordering_key(column), run))
            .collect();
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        if sort.is_some_and(|criteria| criteria.sort_direction == SORT_DIRECTION_DESCENDING) {
            // The ordering key is unique per run, so reversing the ascending order is
            // exactly the descending one.
            ordered.reverse();
        }
        ordered
    }
}

/// The capacity a machine learning transform runs its task runs with, as `(worker_type,
/// number_of_workers, max_capacity)`, resolved from the three capacity members a request
/// asks for.
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
///
/// Taken as three values rather than as one request so that the create, which resolves
/// them from the request alone, and an UPDATE, which resolves them from the members the
/// request names falling back on the ones the transform already carries
/// ([`MlTransformEntry::apply_update`]), cannot disagree about one capacity. Resolving a
/// resolved triple again answers it unchanged, so a write that names no capacity member
/// leaves the stored one where it was.
pub fn ml_transform_capacity(
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
    max_capacity: Option<f64>,
) -> (Option<String>, Option<i32>, Option<f64>) {
    match (worker_type, number_of_workers) {
        (Some(worker_type), workers) => (
            Some(worker_type.to_string()),
            workers,
            dpu_per_worker(worker_type)
                .zip(workers)
                .map(|(per_worker, workers)| per_worker * f64::from(workers))
                .or(max_capacity),
        ),
        // A worker count with no worker type names no DPU figure this emulator could
        // derive, so only what the request itself named is reported.
        (None, Some(workers)) => (None, Some(workers), max_capacity),
        (None, None) => (
            None,
            None,
            Some(max_capacity.unwrap_or(DEFAULT_ML_TRANSFORM_DPUS)),
        ),
    }
}

/// Whether an `UpdateMLTransform` request names a member to change at all.
///
/// Every member of that request other than the `TransformId` addressing the transform is
/// optional, so a request carrying none of them asks for no change: nothing is written, so
/// the transform's `LastModifiedOn` stays where the last write that did change something
/// put it — the same way an `UpdateCrawler` naming no member leaves its crawler's
/// `LastUpdated` alone ([`crawler_update_names_a_change`]).
pub fn ml_transform_update_names_a_change(input: &UpdateMLTransformInput) -> bool {
    input.name.is_some()
        || input.description.is_some()
        || input.parameters.is_some()
        || input.role.is_some()
        || input.glue_version.is_some()
        || input.max_capacity.is_some()
        || input.worker_type.is_some()
        || input.number_of_workers.is_some()
        || input.timeout.is_some()
        || input.max_retries.is_some()
}

// The shapes a machine learning TASK RUN is made of, declared beside the records and the
// selection that use them rather than in the catalog's own import list above.
use crate::ops::types::{TaskRun, TaskRunFilterCriteria, TaskRunProperties, TaskRunSortCriteria};

/// How many task runs of ONE machine learning transform this store keeps.
///
/// CHOSEN: Glue publishes no retention for a transform's task run history, so this is the
/// emulator's own window, for the reason [`MAX_RUNS_PER_JOB`] gives: it bounds
/// [`MlTransformEntry::task_runs`] and every walk over it. The OLDEST run is dropped
/// first, so the runs a caller is most likely to read are kept.
pub const MAX_TASK_RUNS_PER_TRANSFORM: usize = 100;

/// The `TaskType` of a run `StartMLLabelingSetGenerationTaskRun` starts: of the modelled
/// enum (`EVALUATION | LABELING_SET_GENERATION | IMPORT_LABELS | EXPORT_LABELS |
/// FIND_MATCHES`), the one that names generating a labelling set, which is what that
/// operation starts ("Starts the active learning workflow for your machine learning
/// transform to improve the transform's quality by generating label sets").
pub const ML_TASK_TYPE_LABELING_SET_GENERATION: &str = "LABELING_SET_GENERATION";

/// The `TaskStatusType` a task run is recorded in by the call that starts it: of the
/// modelled enum (`STARTING | RUNNING | STOPPING | STOPPED | SUCCEEDED | FAILED |
/// TIMEOUT`), the one that says the run has been accepted and no task has begun yet.
pub const ML_TASK_RUN_STATUS_STARTING: &str = "STARTING";

/// See [`ML_TASK_RUN_STATUS_STARTING`]: the status a run that did not run comes to rest
/// in.
pub const ML_TASK_RUN_STATUS_FAILED: &str = "FAILED";

/// How long a task run stays in the status the call that started it put it in before the
/// clock owes it the next one.
///
/// CHOSEN, not documented: Glue publishes no duration for starting a machine learning
/// task run, so this is the emulator's own span, the same one a job run's transitions take
/// ([`JOB_RUN_SETTLES_AFTER`]) — deliberately not zero, so a caller that lists the runs
/// straight after a start still observes the run in flight. The due instant is stored on
/// the run and materialised by whichever later call reads it; nothing sleeps.
pub const ML_TASK_RUN_SETTLES_AFTER: Duration = JOB_RUN_SETTLES_AFTER;

/// `TaskRun$ErrorString` for the terminal status the clock owes every machine learning
/// task run.
///
/// This emulator is the Glue control plane: it records a task run faithfully, but it runs
/// no machine learning task over the transform's data, so a run can never reach
/// `SUCCEEDED` — it comes to rest in the modelled failure status carrying a reason that
/// says exactly that, rather than reporting a labelling set or a metric nothing produced.
pub const ML_TASK_RUN_NOT_EXECUTED_MESSAGE: &str =
    "This emulator records machine learning task runs but runs no machine learning task, \
     so the task run ended without running.";

/// The `TaskRunSortColumnType` that orders a transform's task runs by their task type. One
/// of the modelled values the generated
/// [`crate::ops::types::TASK_RUN_SORT_COLUMN_TYPE_VALUES`] carries (`TASK_RUN_TYPE`,
/// `STATUS`, `STARTED`), which the request layer validates a sort criteria against.
pub const ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE: &str = "TASK_RUN_TYPE";
/// See [`ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE`]: orders by the status each run stands in.
pub const ML_TASK_RUN_SORT_COLUMN_STATUS: &str = "STATUS";
/// See [`ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE`]: orders by the instant each run started.
pub const ML_TASK_RUN_SORT_COLUMN_STARTED: &str = "STARTED";

/// How many machine learning task run ids this process has minted. A nonce source for
/// [`new_ml_task_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job run id.
static MINTED_ML_TASK_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words a machine learning task run id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters.
const ML_TASK_RUN_ID_WORDS: usize = 2;

/// A fresh `TaskRunId`: the id the service assigns a machine learning task run when a
/// start records it.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255` — the same shape and width as the transform id this store mints
/// ([`new_ml_transform_id`]), because the service publishes no spelling for either. The
/// bits come from [`mint_hex_id`], so two starts landing in the same nanosecond cannot be
/// given the same id, and the id is stored on the run, so every later read of that run
/// reports the same one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_ml_task_run_id() -> String {
    mint_hex_id(&MINTED_ML_TASK_RUN_IDS, ML_TASK_RUN_ID_WORDS)
}

/// One machine learning task run, as the call that started it recorded it.
///
/// A run carries the transform it was started of, the configuration properties of the task
/// that start asked for, and the members only a run has: the minted id, the status it
/// stands in and the instants it moved between statuses. The properties are the modelled
/// `TaskRunProperties` shape, which is where the TYPE of the task lives
/// ([`Self::task_type`]), so each start records the properties of its own task kind and a
/// listing reports them unchanged.
#[derive(Debug, Clone)]
pub struct MlTaskRunEntry {
    /// `TaskRun$TaskRunId`, minted by [`new_ml_task_run_id`].
    pub id: String,
    /// The `TransformId` of the transform this run was started of, which is the first half
    /// of a run's address.
    pub transform_id: String,
    /// "Specifies configuration properties associated with this task run"
    /// (`TaskRun$Properties`), as the start that recorded the run described the task: its
    /// `TaskType` and the properties of that one kind of task.
    pub properties: TaskRunProperties,
    /// One of the modelled `TaskStatusType` values; see [`ML_TASK_RUN_STATUS_STARTING`].
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// "The date and time that this task run started" (`TaskRun$StartedOn`).
    pub started_on: SystemTime,
    /// "The last point in time that the requested task run was updated"
    /// (`TaskRun$LastModifiedOn`).
    pub last_modified_on: SystemTime,
    /// "The last point in time that the requested task run was completed"
    /// (`TaskRun$CompletedOn`), absent until the run has finished.
    pub completed_on: Option<SystemTime>,
    /// "The list of error strings associated with this task run"
    /// (`TaskRun$ErrorString`), absent until the run has failed.
    pub error_string: Option<String>,
}

impl MlTaskRunEntry {
    /// Record a run of `transform_id` performing the task `properties` describes, in
    /// [`ML_TASK_RUN_STATUS_STARTING`] with its next status due
    /// [`ML_TASK_RUN_SETTLES_AFTER`] out.
    ///
    /// Taken as the modelled properties rather than as one start's request, so every
    /// operation that starts a task run of a transform records one the same way and the
    /// listing cannot tell them apart by anything but the properties themselves.
    pub fn start(
        transform_id: &str,
        id: String,
        properties: TaskRunProperties,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            transform_id: transform_id.to_string(),
            properties,
            status: ML_TASK_RUN_STATUS_STARTING.to_string(),
            settles_at: Some(now.checked_add(ML_TASK_RUN_SETTLES_AFTER).unwrap_or(now)),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            // Nothing has failed yet: a run carries an error string only once it has.
            error_string: None,
        }
    }

    /// This run's `TaskType`, the one a filter and a sort select and order on: the type
    /// recorded in its own properties, so there is one place a run's kind is held.
    pub fn task_type(&self) -> &str {
        self.properties.task_type.as_deref().unwrap_or_default()
    }

    /// Move this run into the status the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave [`ML_TASK_RUN_STATUS_STARTING`] at [`Self::settles_at`]
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
        self.status = ML_TASK_RUN_STATUS_FAILED.to_string();
        self.error_string = Some(ML_TASK_RUN_NOT_EXECUTED_MESSAGE.to_string());
        self.completed_on = Some(due);
        self.last_modified_on = due;
    }

    /// `TaskRun$ExecutionTime`: "The amount of time (in seconds) that the task run
    /// consumed resources" — the whole seconds between the run starting and finishing, and
    /// the model's own default of 0 while it has not finished.
    fn execution_time_seconds(&self) -> i32 {
        let Some(completed_on) = self.completed_on else {
            return 0;
        };
        let elapsed = completed_on
            .duration_since(self.started_on)
            .unwrap_or_default();
        i32::try_from(elapsed.as_secs()).unwrap_or(i32::MAX)
    }

    /// Whether this run satisfies every criterion `filter` names.
    ///
    /// A `TaskRunFilterCriteria` is a conjunction: each member it carries narrows the
    /// listing, and a member it leaves out narrows nothing. The two enum members are
    /// matched exactly, and the two instants bound the window the criteria describe
    /// ("filter on task runs started before this date"), so a run started exactly at a
    /// bound is outside it — the same reading the transform filter's instant windows take
    /// ([`MlTransformEntry::matches`]).
    pub fn matches(&self, filter: &TaskRunFilterCriteria) -> bool {
        if filter
            .task_run_type
            .as_deref()
            .is_some_and(|kind| kind != self.task_type())
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
        let started = secs_since_epoch(self.started_on);
        if filter
            .started_before
            .as_ref()
            .is_some_and(|bound| started >= bound.as_secs_f64())
        {
            return false;
        }
        if filter
            .started_after
            .as_ref()
            .is_some_and(|bound| started <= bound.as_secs_f64())
        {
            return false;
        }
        true
    }

    /// This run's place in a listing ordered by `column`, as the continuation token
    /// renders it: the sorted value, then the run's own id so that two runs sharing that
    /// value still have one total order to page over.
    ///
    /// `None` is the order a request that names no sort criteria gets, which is the same
    /// key [`ML_TASK_RUN_SORT_COLUMN_STARTED`] names: the operation documents no default
    /// order for "a list of historic runs" (`GetMLTaskRuns`), so the listing falls back on
    /// the order the runs were started in rather than on however the store happens to hold
    /// them. A column outside the modelled list cannot reach here — the request layer
    /// validates `Sort$Column` against it — and is ordered the same way.
    pub fn ordering_key(&self, column: Option<&str>) -> String {
        let sorted = match column {
            Some(ML_TASK_RUN_SORT_COLUMN_TASK_RUN_TYPE) => self.task_type().to_string(),
            Some(ML_TASK_RUN_SORT_COLUMN_STATUS) => self.status.clone(),
            Some(ML_TASK_RUN_SORT_COLUMN_STARTED) | Some(_) | None => {
                ordered_instant(self.started_on)
            }
        };
        format!("{sorted}{LISTING_KEY_SEPARATOR}{}", self.id)
    }

    /// Rebuild the modelled `TaskRun` output shape from what is stored, so a listing
    /// reports the same run a single-run read does.
    pub fn to_task_run(&self) -> TaskRun {
        TaskRun {
            transform_id: Some(self.transform_id.clone()),
            task_run_id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            // "The names of the log group for secure logging, associated with this task
            // run": no task runs here, so nothing writes this run's output anywhere and
            // there is no log group holding it to name.
            log_group_name: None,
            properties: Some(self.properties.clone()),
            error_string: self.error_string.clone(),
            started_on: Some(to_date_time(self.started_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            completed_on: self.completed_on.map(to_date_time),
            execution_time: self.execution_time_seconds(),
        }
    }
}

/// Whether an `UpdateWorkflow` request names a member of the workflow to change at all.
///
/// Every member of that request other than the `Name` addressing the workflow is optional,
/// so a request carrying none of them asks for no change: nothing is written, so the
/// workflow's `LastModifiedOn` stays where the last write that did change something put it
/// — the same way an `UpdateCrawler` naming no member leaves its crawler's `LastUpdated`
/// alone ([`crawler_update_names_a_change`]). Written as an exhaustive destructuring so a
/// member added to the request shape cannot be silently left out of the answer.
pub fn workflow_update_names_a_change(input: &UpdateWorkflowInput) -> bool {
    let UpdateWorkflowInput {
        name: _,
        description,
        default_run_properties,
        max_concurrent_runs,
    } = input;

    description.is_some() || default_run_properties.is_some() || max_concurrent_runs.is_some()
}

/// The message the catalog's metadata form type surface refuses an identifier it holds no
/// form type under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// This surface's refusal text is itself a JSON document carrying the sentence under a
/// `Message` key, so that text — braces and all — is what the modelled member carries, and
/// a client reading `Message` reads the document rather than a bare sentence. It is NOT an
/// envelope this emulator adds around the member: the member's own value is this text, as
/// the surface words it. `serde_json` renders it, so an identifier carrying a quote or a
/// backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a form type identifier
/// ([`ComAmazonawsGlueAwsglueState::form_type_by_reference`]), so two of them cannot
/// disagree about how the one miss is worded.
pub fn form_type_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Metadata form type {identifier} not found."))
}

/// `sentence` as the business-catalog surface puts a refusal on the wire: a JSON document
/// carrying it under a `Message` key.
///
/// One copy, shared by every message builder of that surface below, so the DOCUMENT shape
/// cannot drift between them while each keeps the sentence its own refusal words. Rendered
/// by `serde_json`, so a sentence carrying a quote or a backslash still yields a
/// well-formed document.
fn catalog_refusal_document(sentence: &str) -> String {
    serde_json::json!({ "Message": sentence }).to_string()
}

/// The message the catalog's business glossary surface refuses an identifier it holds no
/// glossary under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type one above does
/// ([`form_type_not_found_message`]): the refusal text is itself a JSON document carrying
/// the sentence under a `Message` key, so that text — braces and all — is what the modelled
/// member carries, and the resource is named as the service names it, a business glossary.
/// `serde_json` renders it, so an identifier carrying a quote or a backslash still yields a
/// well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a `GlossaryId`
/// ([`ComAmazonawsGlueAwsglueState::glossary`]) — the glossary's own reader and the term
/// create that puts a term inside one — so two of them cannot disagree about how the one
/// miss is worded.
pub fn glossary_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Business glossary {identifier} not found."))
}

/// The message every refusal of a glossary NAME this account already holds a glossary
/// under carries: the value of that refusal's `AlreadyExistsException$Message`.
///
/// Worded the way this surface words every refusal ([`glossary_not_found_message`],
/// [`form_type_not_found_message`]): the refusal text is itself a JSON document carrying
/// the sentence under a `Message` key, so that text — braces and all — is what the
/// modelled member carries. The sentence names the glossary the write asked for, in
/// single quotes, which is its one variable part; `serde_json` renders it, so a name
/// carrying a quote or a backslash still yields a well-formed document.
///
/// One copy, shared by the create that would mint a second glossary under a stored name
/// and the update that would rename one onto it, so the two cannot disagree about how the
/// one collision is worded.
pub fn glossary_already_exists_message(name: &str) -> String {
    catalog_refusal_document(&format!("A glossary with name '{name}' already exists."))
}

/// A stored form type: the schema for structured metadata `PutFormType` registers in the
/// Data Catalog.
///
/// Every member of the modelled form type is held here — its id, its name and its schema
/// — and nothing else: the model gives a form type no status, owner or timestamp. The
/// name and the schema come from the request, and the id is derived from the name (see
/// [`Self::id`]).
#[derive(Debug, Clone)]
pub struct FormTypeEntry {
    /// `FormTypeId`: the identifier a client addresses this form type by, which is the
    /// form type's own NAME.
    ///
    /// Not an opaque per-record id: the catalog can word a refusal for a form type it no
    /// longer holds with that form type's name while all it was given is the identifier,
    /// so the identifier it hands out carries the name. `FormTypeName`'s `length 1..128`
    /// sits inside `FormTypeId`'s `length 1..256`, so every name is a usable identifier,
    /// and because the name is also the namespace's key, two form types never share one
    /// identifier and an update under a name keeps the identifier an earlier put reported.
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
            id: name.clone(),
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

/// The message the catalog's asset type surface refuses an identifier it holds no asset
/// type under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type, asset and business
/// glossary ones do ([`form_type_not_found_message`]): the refusal text is itself a JSON
/// document carrying the sentence under a `Message` key, so that text — braces and all —
/// is what the modelled member carries, and a client reading `Message` reads the document
/// rather than a bare sentence. `serde_json` renders it, so an identifier carrying a quote
/// or a backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves an asset type identifier
/// ([`ComAmazonawsGlueAwsglueState::asset_type_by_reference`]), so two of them cannot
/// disagree about how the one miss is worded.
pub fn asset_type_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Asset type {identifier} not found."))
}

/// The message the catalog's asset surface refuses an identifier it holds no asset under
/// with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type and business
/// glossary ones above do ([`form_type_not_found_message`]): the refusal text is itself a
/// JSON document carrying the sentence under a `Message` key, so that text — braces and
/// all — is what the modelled member carries. This surface words the sentence the other
/// way round from those two, naming the resource kind and then the identifier that
/// resolved to nothing. `serde_json` renders it, so an identifier carrying a quote or a
/// backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves an `AssetId`
/// ([`ComAmazonawsGlueAwsglueState::asset`]) — the asset's own reader and the glossary
/// term association that addresses one — so two of them cannot disagree about how the one
/// miss is worded.
pub fn asset_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Asset not found: {identifier}"))
}

/// The message the catalog's business glossary surface refuses an identifier it holds no
/// glossary TERM under with: the value of that refusal's
/// `EntityNotFoundException$Message`.
///
/// Worded the way the glossary itself words a miss ([`glossary_not_found_message`]) — the
/// refusal text is a JSON document carrying the sentence under a `Message` key — with the
/// resource named as the service names it, a business glossary term.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a `GlossaryTermId`
/// ([`ComAmazonawsGlueAwsglueState::glossary_term`]), so two of them cannot disagree about
/// how the one miss is worded.
pub fn glossary_term_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Business glossary term {identifier} not found."))
}

/// The message the catalog refuses a request that asks to associate a glossary term this
/// store holds none of with: the value of that refusal's `InvalidInputException$Message`.
///
/// The refusal is the ASSET surface's reading of a reference it cannot resolve, so it is
/// worded the way that surface words a miss ([`asset_not_found_message`]) rather than the
/// way the term's own reader words one: the text is a JSON document carrying the sentence
/// under a `Message` key, and the sentence names the kind of thing that was looked for and
/// then the identifier that resolved to nothing.
///
/// `identifier` is the text's one variable part and is the entry of the request's
/// identifier list that resolved to nothing, taken verbatim.
pub fn unassociable_glossary_term_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Glossary term not found: {identifier}"))
}

/// A stored asset: one catalog entity `PutAsset` registers as an instance of an asset
/// type, carrying that entity's metadata in forms keyed by form name.
///
/// Every member of the modelled asset is held here — its identifier, the asset type it is
/// an instance of, its name, its description, its forms and the two instants the SERVER
/// stamps on it — so a read reports the asset the last write stored and nothing invented on
/// the way out. The identifier, the asset type, the name, the description and the forms
/// come from the request; the two instants are the clock's.
#[derive(Debug, Clone)]
pub struct AssetEntry {
    /// `AssetId`: the identifier the PUT's caller chose, which is also this record's key in
    /// [`ComAmazonawsGlueAwsglueState::assets`] and the address every later operation
    /// carries. Stored exactly as sent.
    pub id: String,
    /// "The identifier of the asset type for the asset" (`PutAssetRequest$AssetTypeId`),
    /// stored exactly as the put addressed it — either the `AssetTypeId` the catalog minted
    /// for that asset type or the `AssetTypeName` it was registered under, the two forms
    /// [`ComAmazonawsGlueAwsglueState::asset_type_by_reference`] resolves — because it
    /// resolved against this store when it was stored, so a read reports the identifier the
    /// client wrote.
    pub asset_type_id: String,
    /// "The name of the asset" (`PutAssetRequest$Name`), as the latest put sent it.
    pub name: String,
    /// "The description of the asset", or `None` when the latest put carried none: the
    /// member is optional and the model gives it no default.
    pub description: Option<String>,
    /// "The forms to set on the asset, keyed by form name", each entry the form type that
    /// defines the form's schema and the form's JSON content, stored exactly as the put
    /// sent them. A put SETS the whole map, so a form the latest put did not send is not
    /// held. Ordered by form name, so two reads of one asset report its forms in one order.
    pub forms: BTreeMap<String, AssetFormEntry>,
    /// "The timestamp at which the asset was created": the instant the put that first
    /// stored this identifier was served, kept by every later put under it so every read
    /// reports the same creation instant.
    pub created_at: SystemTime,
    /// "The timestamp at which the asset was last updated" (`GetAssetOutput$UpdatedAt`):
    /// the instant the latest put that changed this record was served, which for a record
    /// no put has updated yet is [`Self::created_at`].
    pub updated_at: SystemTime,
    /// The `ClientToken` of the put that last wrote this record, when it carried one.
    ///
    /// `PutAssetRequest$ClientToken` is "a unique, case-sensitive identifier that you
    /// provide to ensure the idempotency of the request", so a retry carrying the token of
    /// the put already stored is answered with that record rather than writing it again.
    pub client_token: Option<String>,
    /// The glossary terms associated with this asset, as the `GlossaryTermId` of each, in
    /// the order they were first associated.
    ///
    /// A SET, held in insertion order: associating a term already held changes nothing, so
    /// the same association sent twice leaves one entry, and the order two reads report is
    /// the order the associations happened in. The ids are stored exactly as the
    /// association sent them and are NOT resolved against the term namespace afterwards,
    /// because the association is to the id: a term deleted after being associated leaves
    /// its id on the asset, which is why this is not a list of references into
    /// [`ComAmazonawsGlueAwsglueState::glossary_terms`]. An asset no association has
    /// reached holds none, and the list never grows past
    /// [`MAX_ASSET_GLOSSARY_TERMS`].
    ///
    /// A put under an identifier already stored does not touch it: `PutAsset` sets an
    /// asset's forms and carries no glossary term member at all, so the association
    /// surface is the only writer.
    pub glossary_terms: Vec<String>,
}

impl AssetEntry {
    /// The record a put stores for an identifier this store does not hold yet: the
    /// request's members, stamped with the instant `now` the put was served at. A fresh
    /// asset carries no glossary term: the put models no member to send one with.
    pub fn new(
        id: String,
        asset_type_id: String,
        name: String,
        description: Option<String>,
        forms: BTreeMap<String, AssetFormEntry>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            asset_type_id,
            name,
            description,
            forms,
            created_at: now,
            updated_at: now,
            client_token,
            glossary_terms: Vec::new(),
        }
    }

    /// This record's glossary terms as the modelled `GlossaryTermIdList`, which is what
    /// every operation that reports an asset's associations answers with, or `None` when
    /// the asset carries none.
    ///
    /// An asset with no association reports no list rather than an empty one: the modelled
    /// list is "len 1..10", so an empty list is not a value this member can carry.
    pub fn glossary_terms_list(&self) -> Option<Vec<String>> {
        if self.glossary_terms.is_empty() {
            None
        } else {
            Some(self.glossary_terms.clone())
        }
    }

    /// Merge `identifiers` into this asset's glossary term set, reporting the terms the
    /// asset carries afterwards, or `None` when the merge would take it past
    /// [`MAX_ASSET_GLOSSARY_TERMS`].
    ///
    /// A union, not a replacement: an id the asset already holds keeps its place and is
    /// not repeated, and an id it does not hold is appended, so the terms an earlier
    /// association added survive a later one and an association sent twice is the same
    /// state as sending it once. The bound is checked against the UNION, so re-associating
    /// terms an asset already holds is served however full it is.
    pub fn associate_glossary_terms(&mut self, identifiers: &[String]) -> Option<Vec<String>> {
        let mut merged = self.glossary_terms.clone();
        for identifier in identifiers {
            if !merged.iter().any(|held| held == identifier) {
                merged.push(identifier.clone());
            }
        }
        if merged.len() > MAX_ASSET_GLOSSARY_TERMS {
            return None;
        }
        self.glossary_terms = merged;
        Some(self.glossary_terms.clone())
    }

    /// This record's forms as the modelled `AssetFormMap`, which is what every operation
    /// that reports an asset's forms answers with.
    pub fn forms_map(&self) -> HashMap<String, AssetFormEntry> {
        self.forms
            .iter()
            .map(|(form_name, form)| (form_name.clone(), form.clone()))
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

    /// Apply an `UpdateGlossary` request to this record.
    ///
    /// Every updatable member is optional on the request ("The updated name of the
    /// glossary", "The updated description of the glossary"), so the ones the request
    /// NAMES replace what is stored and the ones it omits are left where the last write
    /// that named them put them: an update naming one member changes that one alone.
    ///
    /// [`Self::id`] is not a member of the request and so cannot move, and
    /// [`Self::client_token`] keeps the CREATE's token, which is what a retry of that
    /// create is recognised by and not something an update of the glossary restates.
    pub fn apply_update(&mut self, input: &UpdateGlossaryInput) {
        if let Some(name) = input.name.clone() {
            self.name = name;
        }
        if let Some(description) = input.description.clone() {
            self.description = Some(description);
        }
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
    /// This job's BOOKMARK — "a point that a job can resume processing"
    /// (`JobBookmarkEntry`) — or `None` while the job has none.
    ///
    /// A bookmark is state a RUN leaves behind, so a job definition nothing has run yet
    /// has none and every operation that addresses one (`GetJobBookmark`,
    /// `ResetJobBookmark`) answers that job with a miss. It lives on the job rather than
    /// beside it because both operations address it by `JobName` alone, so the bookmark is
    /// reclaimed with the definition it belongs to.
    pub bookmark: Option<JobBookmarkState>,
}

/// The bookmark of one job: the point a further run of it would resume processing from.
///
/// The members are `JobBookmarkEntry`'s own, minus the job name, which is the key this is
/// stored under ([`JobEntry::bookmark`]).
#[derive(Debug, Clone, Default)]
pub struct JobBookmarkState {
    /// "The version of the job" the bookmark is of: 1 for the bookmark a job's first run
    /// wrote, advanced by each reset, which starts a new version of the bookmark the next
    /// run continues.
    pub version: i32,
    /// "The run ID number": how many runs of the job have written into this version of the
    /// bookmark.
    pub run: i32,
    /// "The attempt ID number" of the run that last wrote the bookmark.
    pub attempt: i32,
    /// "The unique run identifier associated with the previous job run".
    pub previous_run_id: Option<String>,
    /// "The run ID number" of the run that last wrote the bookmark, as that run's `Id`.
    pub run_id: Option<String>,
    /// "The bookmark itself" — the position a resumed run reads from — and `None` for a
    /// bookmark no run has written a position into, which is the state a reset leaves.
    pub job_bookmark: Option<String>,
}

impl JobBookmarkState {
    /// The bookmark version a job's first run writes. Advanced by each reset, so a reset
    /// leaves the next run continuing a version of its own.
    pub const FIRST_VERSION: i32 = 1;

    /// Record that `run_id` ran: the bookmark advances to that run, keeping the run it
    /// followed as its `PreviousRunId`.
    ///
    /// Called by the only thing that writes a bookmark, the START of a run — whether a
    /// caller started it or the job's retry policy owed it — so a bookmark never moves
    /// because something was read, only because a further run of the job began.
    pub fn record_run(&mut self, run_id: &str, attempt: i32) {
        self.previous_run_id = self.run_id.take();
        self.run_id = Some(run_id.to_string());
        self.attempt = attempt;
        self.run = self.run.saturating_add(1);
        if self.version < Self::FIRST_VERSION {
            self.version = Self::FIRST_VERSION;
        }
    }

    /// Reset this bookmark: "Resets a bookmark entry" (`ResetJobBookmark`), so the
    /// position a further run would resume from is dropped and the run counter starts
    /// again, under the next version of the bookmark.
    ///
    /// NOT recorded: what is pinned is that the reset clears the position the next run
    /// would have resumed from and that the entry stays readable afterwards, which is what
    /// a client resets a bookmark to achieve.
    pub fn reset(&mut self) {
        self.version = self.version.saturating_add(1);
        self.run = 0;
        self.attempt = 0;
        self.previous_run_id = None;
        self.run_id = None;
        self.job_bookmark = None;
    }

    /// This bookmark in the `JobBookmarkEntry` shape every operation that answers one
    /// reports it in, attributed to the job it belongs to.
    pub fn to_job_bookmark_entry(&self, job_name: &str) -> JobBookmarkEntry {
        JobBookmarkEntry {
            job_name: Some(job_name.to_string()),
            version: self.version,
            run: self.run,
            attempt: self.attempt,
            previous_run_id: self.previous_run_id.clone(),
            run_id: self.run_id.clone(),
            job_bookmark: self.job_bookmark.clone(),
        }
    }
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
            // And no bookmark until one of those runs writes it.
            bookmark: None,
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
        // Carried over for the same reason as the runs: `JobUpdate` has no member for the
        // bookmark, and an update of the definition does not undo the runs that wrote it.
        let bookmark = self.bookmark.take();
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
            bookmark,
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

    /// Give every run of this job the state the CLOCK already owed it at `now`, and record
    /// the retries those states left owed.
    ///
    /// This is the one write a reporting operation performs: a run's next state was due
    /// at [`JobRunEntry::settles_at`] whether or not anybody called, so materialising it
    /// on read reports the state the clock had already reached. It never moves a run
    /// because something was read.
    ///
    /// A retry is owed on exactly the same terms. "The maximum number of times to retry
    /// this job after a JobRun fails" (`Job$MaxRetries`) is a policy of the JOB, so the
    /// SERVICE — not the caller — runs the next attempt of a run that ended in the failure
    /// state while the job still has retry budget: a further run of the same job under the
    /// same configuration, carrying the next `Attempt` and naming the attempt it retries in
    /// its `PreviousRunId` ([`JobRunEntry::retry`]). That attempt was due the instant the
    /// failure was, so it is materialised here rather than slept for, and it is recorded
    /// once: a run another attempt already retries is retried by nothing further, so two
    /// reads of the same chain report the same runs.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        // A retry can itself fail and be retried, so what the clock owes is materialised in
        // passes: each settles the states that fell due and records at most one further
        // attempt. The passes are bounded by the runs one job can hold
        // ([`MAX_RUNS_PER_JOB`]), each pass but the last adding one.
        for _ in 0..=MAX_RUNS_PER_JOB {
            for run in &mut self.runs {
                run.settle_if_due(now);
            }
            let Some(retry) = self.owed_retry() else {
                return;
            };
            self.record_run(retry);
        }
    }

    /// The next attempt this job's stored runs leave owed, or `None` when they leave none.
    ///
    /// The run it retries is one that ended in the failure state — a run ended any other
    /// way (a submitted stop) failed at nothing and is retried by nothing — whose `Attempt`
    /// is still below the job's `MaxRetries`, so the chain one start begins is one original
    /// attempt plus at most that many retries, and which no other run already names as the
    /// attempt it retries, so a chain never forks.
    ///
    /// The attempt it answers is not in flight yet: it takes the slot the failed attempt
    /// released, so materialising it can never put more runs in flight than
    /// `ExecutionProperty$MaxConcurrentRuns` admitted starts.
    fn owed_retry(&self) -> Option<JobRunEntry> {
        if self.max_retries <= 0 {
            return None;
        }
        let retried: BTreeSet<&str> = self
            .runs
            .iter()
            .filter_map(|run| run.previous_run_id.as_deref())
            .collect();
        let failed = self.runs.iter().find(|run| {
            run.state == JOB_RUN_STATE_FAILED
                && run.attempt < self.max_retries
                && !retried.contains(run.id.as_str())
        })?;
        Some(failed.retry(new_job_run_id()))
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
    ///
    /// This is also where the job's BOOKMARK is written: a bookmark is the point a job
    /// resumes processing at, which a run is what establishes — so the first run of a job
    /// gives it one ([`JobBookmarkState::record_run`]) and every later run advances it,
    /// including an attempt the job's retry policy owed ([`JobEntry::owed_retry`]), which is
    /// a further run of the job and not a re-read of an earlier one. Nothing else writes it
    /// except a reset, so a bookmark never moves because something was read.
    pub fn record_run(&mut self, run: JobRunEntry) {
        self.bookmark
            .get_or_insert_with(JobBookmarkState::default)
            .record_run(&run.id, run.attempt);
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_JOB {
            let overflow = self.runs.len() - MAX_RUNS_PER_JOB;
            self.runs.drain(..overflow);
        }
    }

    /// Whether this stored job definition is the one `input` asks to create, member for
    /// member.
    ///
    /// "It must be unique in your account" (`CreateJobRequest$Name`), so the name is what
    /// a create reuses when it submits a definition under one already taken: such a create
    /// is a RETRY of the create that stored this record only when it asks for the very
    /// same job, and otherwise it has associated one identifier with two different
    /// records — which is what the operation's `IdempotentParameterMismatchException`
    /// ("the same unique identifier was associated with two different records") states.
    ///
    /// The comparison is over the whole stored definition, the tags included, because any
    /// member of it is part of the job the caller asked for. It is made on the modelled
    /// `Job` shape rendered by this service's own JSON codec rather than member by member:
    /// the definition carries the `CodeGenConfigurationNodes` ETL graph, whose modelled
    /// shapes derive no equality and hold `HashMap` members whose iteration order comes
    /// from a per-process hash seed, so neither `Debug` nor the rendered bytes of two equal
    /// graphs match while `serde_json::Value`'s own equality compares objects member by
    /// member whichever map backs them. The two SERVER-set instants are not part of the
    /// submitted definition, so the candidate is built carrying this record's own.
    ///
    /// A definition the codec cannot render is not shown to be this one, so such a create
    /// is answered as the mismatch it cannot be told apart from rather than as a retry
    /// that would drop what it sent behind a refusal.
    pub fn matches_create_input(&self, input: &CreateJobInput) -> bool {
        let mut candidate = Self::from_create_input(input, self.created_on);
        candidate.last_modified_on = self.last_modified_on;
        if self.tags != candidate.tags {
            return false;
        }
        match (
            rendered_shape(&Job::SCHEMA, &self.to_job()),
            rendered_shape(&Job::SCHEMA, &candidate.to_job()),
        ) {
            (Some(stored), Some(requested)) => stored == requested,
            _ => false,
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

/// One modelled shape as the JSON value this service's own codec renders for it.
///
/// Rendering through the codec is what makes two shapes COMPARABLE at all: the modelled
/// structures derive no equality, and several of them hold `std::collections::HashMap`
/// members (a job's `DefaultArguments`, its `CodeGenConfigurationNodes` and the options
/// inside them) whose iteration order comes from a per-process hash seed, so neither
/// `Debug` nor the rendered bytes of two equal values match. `serde_json::Value` then
/// supplies the equality: its objects compare by member, not by order, whichever map
/// backs them, and its lists keep their order, which is right for the modelled sequences.
///
/// A value the codec cannot render answers `None`, so the comparison is impossible rather
/// than false and the caller decides what to do with that.
fn rendered_shape(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<::serde_json::Value> {
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonShapeSerializer::new();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored shape, so two definitions cannot be compared"
        );
        return None;
    }
    match ::serde_json::from_str::<::serde_json::Value>(&serializer.into_string()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            ::tracing::warn!(
                %error,
                "the JSON this service renders for a stored shape did not parse back"
            );
            None
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
/// documentation for each, and a session create carries three of them under that same
/// documentation, so all of them resolve capacity here.
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
/// The `JobRunState` of a run whose script finished without failing. See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";
/// The `JobRunState` of a run that "is terminated and enters `TIMEOUT` status"
/// (`JobRun$Timeout`). See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_TIMEOUT: &str = "TIMEOUT";
/// The `JobRunState` of a run that ended on a service error rather than on its script.
/// See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_ERROR: &str = "ERROR";
/// The `JobRunState` of a run held back by job-run queuing: "when a job run is in a
/// WAITING state as a result of job run queuing" (`JobRun$StateDetail`). See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_WAITING: &str = "WAITING";
/// The `JobRunState` of a queued run that waited past its queuing window. See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_EXPIRED: &str = "EXPIRED";

/// The `JobRunState` values a run never moves out of again: of the modelled enum quoted
/// on [`JOB_RUN_STATE_STARTING`], the ones that name a finished run. The other four
/// (`STARTING`, `RUNNING`, `STOPPING`, `WAITING`) each still owe a further state.
pub const TERMINAL_JOB_RUN_STATES: [&str; 6] = [
    JOB_RUN_STATE_STOPPED,
    JOB_RUN_STATE_SUCCEEDED,
    JOB_RUN_STATE_FAILED,
    JOB_RUN_STATE_TIMEOUT,
    JOB_RUN_STATE_ERROR,
    JOB_RUN_STATE_EXPIRED,
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

/// The job-run request an `Action` of a fired trigger amounts to: the run of `job_name`
/// that action initiates, carrying every override the two shapes share — `Arguments`,
/// `Timeout`, `SecurityConfiguration` and `NotificationProperty`, each documented on the
/// action in the words `StartJobRunRequest` documents it in — and nothing else, because an
/// action names no capacity, no execution class and no previous run to retry. Reading the
/// run off this is what makes a run a firing began the same record a direct start records
/// ([`JobRunEntry::start`]).
fn job_run_request_for_action(action: &Action, job_name: &str) -> StartJobRunInput {
    StartJobRunInput {
        job_name: job_name.to_string(),
        arguments: action.arguments.clone(),
        timeout: action.timeout,
        security_configuration: action.security_configuration.clone(),
        notification_property: action.notification_property.clone(),
        ..StartJobRunInput::default()
    }
}

/// One run of a job definition, as a start of that job or a trigger's firing recorded it.
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
    /// `JobRun$TriggerName`: "The name of the trigger that started this job run", and
    /// `None` for a run a caller started directly, which no trigger started.
    pub trigger_name: Option<String>,
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
    ///
    /// `triggered_by` is the trigger whose firing initiated this run (`JobRun$TriggerName`,
    /// "The name of the trigger that started this job run"), and `None` for a run a caller
    /// started directly.
    pub fn start(
        job: &JobEntry,
        input: &StartJobRunInput,
        id: String,
        now: SystemTime,
        retries: Option<(String, i32)>,
        triggered_by: Option<&str>,
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
            trigger_name: triggered_by.map(str::to_string),
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

    /// The RETRY of this run: the next attempt of the same job, under the configuration
    /// this attempt ran with, carrying one past this run's `Attempt` ("The number of the
    /// attempt to run this job") and naming this run as the `PreviousRunId` it retries
    /// ("The ID of the previous run of this job").
    ///
    /// It begins the instant this attempt FINISHED, because that is the instant the retry
    /// became owed — taking it from the stored instant rather than from when somebody read
    /// the chain is what makes two reads of the chain report the same runs — and like any
    /// other run it owes its own terminal state one span later
    /// ([`JOB_RUN_SETTLES_AFTER`]), so a caller walking the chain observes each attempt in
    /// flight before the next one exists. Nothing has failed in the new attempt yet, so it
    /// carries no `ErrorMessage` of its own and no completion instant.
    ///
    /// Scheduled by [`JobEntry::settle_due_runs`], which decides WHETHER an attempt is owed;
    /// this builds the record of one. A retry of a run a TRIGGER began keeps that run's
    /// `TriggerName`, because the attempt is of the work that firing initiated; the workflow
    /// run that firing belongs to still reports the attempt IT recorded, since its actions
    /// name the run ids they began.
    fn retry(&self, id: String) -> Self {
        let started_on = self.completed_on.unwrap_or(self.last_modified_on);
        Self {
            id,
            attempt: self.attempt.saturating_add(1),
            previous_run_id: Some(self.id.clone()),
            started_on,
            last_modified_on: started_on,
            completed_on: None,
            state: JOB_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                started_on
                    .checked_add(JOB_RUN_SETTLES_AFTER)
                    .unwrap_or(started_on),
            ),
            // Nothing executes the job script for this attempt either, so the state the
            // clock owes it is the same modelled failure state.
            settles_to: JOB_RUN_STATE_FAILED.to_string(),
            error_message: None,
            ..self.clone()
        }
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
            // "The name of the trigger that started this job run": the trigger whose
            // firing initiated this run, and nothing for a run a caller started directly.
            trigger_name: self.trigger_name.clone(),
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

/// The service principal a crawler's `Role` has to trust for this service to run the crawl
/// as it. A crawl is performed BY Glue acting as that role, so the role's trust policy is
/// what lets Glue assume it at all.
pub const CRAWLER_SERVICE_PRINCIPAL: &str = "glue.amazonaws.com";

/// The ONE reading of the form a crawler's `Role` may take: its role NAME and its role ARN.
///
/// `Role` is "the IAM role or Amazon Resource Name (ARN) of an IAM role used by the new
/// crawler to access customer resources" (`CreateCrawler$Role`), so a value that reads as an
/// ARN carries the name inside it ([`::service_common::iam_role::role_name`], the tree's one
/// reading of an IAM ARN) and any other value IS the name. Stated once here so the two
/// questions this crate asks about that member — the trust document
/// ([`crawler_role_is_unassumable`]) and the identity policies in force on the role
/// ([`role_policy_documents`]) — cannot read the member's form two different ways.
pub fn crawler_role_reference(role: &str) -> CrawlerRole<'_> {
    match ::service_common::iam_role::role_name(role) {
        // A value that reads as this account's role ARN: the name is the one IAM holds it
        // under, and the ARN is the value as sent.
        Some(name) => CrawlerRole {
            name,
            arn: role.to_string(),
        },
        // Not this account's role ARN. Either it is a bare role name — in which case the
        // ARN is the one this emulator mints for a role of that name — or it is an ARN of
        // another account, which names no role this emulator's IAM can be asked about and
        // so resolves to no local name either.
        None if ::service_common::arn::parse(role).is_some() => CrawlerRole {
            name: role,
            arn: role.to_string(),
        },
        None => CrawlerRole {
            name: role,
            // An IAM ARN is partition-global, so it carries no Region.
            arn: ::engine::emulator_arn(IAM_ARN_NAMESPACE, "", &format!("{ROLE_ARN_SEGMENT}/{role}")),
        },
    }
}

/// What a crawler's `Role` resolves to: the name IAM would hold the role under, and the ARN
/// that addresses it. See [`crawler_role_reference`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlerRole<'a> {
    /// The role name, for a lookup in the IAM store's own records.
    pub name: &'a str,
    /// The role ARN, for a reading that addresses the role by ARN.
    pub arn: String,
}

/// The ARN vendor field of an IAM entity.
const IAM_ARN_NAMESPACE: &str = "iam";

/// The ARN resource-type segment of an IAM role.
const ROLE_ARN_SEGMENT: &str = "role";

/// Whether this emulator's own IAM ANSWERED that `role` is a role Glue cannot assume: this
/// account holds no such role, or the trust policy the role carries does not admit
/// [`CRAWLER_SERVICE_PRINCIPAL`].
///
/// A crawler RUNS AS its `Role`, so a role Glue cannot assume backs no crawl at all, and
/// the service settles that before it stores anything. The whole question — which role of
/// this account the value names, what IAM answers about it, and whether the trust document
/// it carries admits a service principal — is the ONE reading every emulated service that
/// resolves a caller-supplied execution role shares
/// (`service_common::iam_role::role_is_unassumable`), so this crate states only WHICH
/// principal has to be admitted and WHICH operations ask.
///
/// IAM is reached the way any emulated service reaches another: the AWS SDK over the
/// emulator's OWN in-process callback ([`smithy_server_runtime::OperationContext::callback`]),
/// so nothing here opens a socket or reads an ambient credential. `false` — do not refuse —
/// for every case where nothing is KNOWN about the role, including an emulator with no IAM
/// mounted to ask: a refusal has to be grounded in what IAM answered, never assumed. Awaited
/// with NO store guard held.
pub async fn crawler_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role: &str,
) -> bool {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        &crawler_role_reference(role).arn,
        CRAWLER_SERVICE_PRINCIPAL,
    )
    .await
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
/// (`CreateCrawler$Role`), whose form is read HERE by the crate's one reading of that
/// member ([`crawler_role_reference`]), so this lookup and the trust-document reading
/// beside it resolve the same value to the same role.
///
/// `None` when nothing can be SHOWN about the role's permissions: this account holds no
/// such role, it carries more documents than [`MAX_ROLE_DOCUMENTS`] (counted before any is
/// read), or one of its attachments is an AWS-managed policy whose document IAM does not
/// carry. Called with the IAM guard held, and it CLONES what it reads, so the caller drops
/// the guard before deciding anything: nothing bounded by the request runs under it.
pub fn role_policy_documents(role: &str, iam: &IamState) -> Option<Vec<String>> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

    let name = crawler_role_reference(role).name;
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

/// `RegistryStatus` for a registry a delete is draining, the other of the two values the
/// enum carries. A listing leaves a registry in this status out of its results, so the
/// value is named here rather than spelled in each reader.
pub const REGISTRY_STATUS_DELETING: &str = "DELETING";

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

/// The inclusive span of schema version numbers a `Versions` string names, lowest first,
/// or `None` when the value names no version at all.
///
/// Stated by the member: "A version range may be supplied which may be of the format: a
/// single version number, 5 a range, 5-8 : deletes versions 5, 6, 7, 8"
/// (`DeleteSchemaVersionsInput$Versions`), whose modelled form is a decimal counter with no
/// leading zero, alone or as the two ends of a `<low>-<high>` range. A value outside that
/// form names no version, and so does one whose ends do not spell a number this service can
/// count with.
///
/// The span is NOT narrowed to the versions a schema set holds: which of the versions a
/// request named exist is the caller's own schema set to answer, not this reading's.
pub fn parse_version_span(versions: &str) -> Option<(i64, i64)> {
    /// One end of the span: the modelled `[1-9][0-9]*`, i.e. a positive decimal counter
    /// carrying no sign, no separator and no leading zero.
    fn counter(text: &str) -> Option<i64> {
        let mut characters = text.chars();
        if !matches!(characters.next(), Some('1'..='9')) {
            return None;
        }
        if !characters.all(|character| character.is_ascii_digit()) {
            return None;
        }
        text.parse::<i64>().ok()
    }

    match versions.split_once('-') {
        Some((low, high)) => Some((counter(low)?, counter(high)?)),
        None => {
            let one = counter(versions)?;
            Some((one, one))
        }
    }
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
/// name). A fifth and a sixth are the classifier's own refusals
/// ([`messages::classifier_not_found`], [`messages::classifier_already_exists`]), whose
/// recorded wordings state the miss and the collision TWICE ("Classifier `<Name>` not
/// found. not found.") because the service composes its per-resource sentence with its
/// generic one; they are enumerated here so the doubling reads as the requirement it is
/// rather than as a typo to tidy. The classifier DELETE words its own miss differently
/// again ([`messages::classifier_not_found_for_delete`]), which is why the read's
/// sentence and the delete's are two strings and not one.
/// The rest are the same per-resource wording for the same refusal on the
/// sibling operations. Every handler refuses through these constructors, so one
/// operation's wording is never silently borrowed for another's refusal.
///
/// This is the TOP-LEVEL error body only. A per-item failure a batch reports inside its
/// output (`BatchDeleteTable`'s `TableError$ErrorDetail`) has its own recorded wording
/// and is built by [`super::catalog_error_detail`]. Of the job and connection slices the
/// job-definition miss ([`messages::JOB_NOT_FOUND`]) and `StartJobRun`'s
/// missing-metadata refusal are here, because the corpus records both; the rest of their
/// messages are not requirements and are left to the issues that own them rather than
/// guessed at.
///
/// The schema-registry misses ARE here, because every operation that resolves a
/// `RegistryId` or a `SchemaId` resolves the same two references and so has to describe
/// the same two misses. The SCHEMA miss is RECORDED: it is the fixed constant
/// [`messages::SCHEMA_NOT_FOUND`], which names neither the schema the request addressed
/// nor the registry it was addressed in. The registry miss is not recorded: what is pinned
/// there is the behaviour — 400 with the operation's modelled `EntityNotFoundException` —
/// and its sentence follows the per-resource template the catalog slice's recorded misses
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

    /// A request to suspend the schedule of a crawler that has no schedule running —
    /// one whose schedule is already suspended, or that carries no cron expression at
    /// all. NOT recorded: what is pinned is the behaviour — 400 with the operation's
    /// modelled `SchedulerNotRunningException` — and, like the crawler-running refusal
    /// above, a sentence carrying only the requested `CrawlerName`.
    pub fn scheduler_not_running(name: &str) -> String {
        format!("Scheduler for crawler with name {name} is not running")
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

    /// `CreateCrawler` (and the `UpdateCrawler` that replaces a crawler's role) for a
    /// `Role` this service cannot assume ([`super::crawler_role_is_unassumable`]).
    ///
    /// RECORDED constant, and the whole message: it is the service's fixed template for
    /// this check, naming neither the role the request sent, the principal that has to be
    /// admitted, nor the operation — so a client recognises the refusal by its wording
    /// whichever crawler write reported it. No trailing period, as recorded.
    pub const ROLE_NOT_ASSUMABLE: &str =
        "Service is unable to assume provided role. Please verify role's TrustPolicy";

    /// Any operation refusing a request member whose value is outside the form that member
    /// admits — the `Versions` of `DeleteSchemaVersions` that spells no version number or
    /// range of them ([`super::parse_version_span`]).
    ///
    /// RECORDED template: fixed text plus ONE variable part, the MEMBER's name as the model
    /// spells it. The value the caller sent is withheld, so the refusal discloses nothing of
    /// the request back to it.
    pub fn parameter_value_not_valid(parameter: &str) -> String {
        format!(
            "The parameter value contains one or more characters that are not valid. \
             Parameter Name: {parameter}"
        )
    }

    /// `DeleteSchemaVersions` whose `Versions` names the schema set's check-pointed
    /// version. Stated by the operation: "When the range of version numbers contain check
    /// pointed version, the API will … not proceed with the deletion. You have to remove
    /// the checkpoint first using the DeleteSchemaCheckpoint API before using this API."
    ///
    /// NOT recorded: what is pinned is the behaviour — the operation's modelled
    /// `ConcurrentModificationException`, with nothing deleted — and a sentence stating that
    /// documented requirement, carrying the check-pointed version the request collided with.
    pub fn schema_version_is_checkpointed(version_number: i64) -> String {
        format!(
            "Schema version {version_number} is the checkpoint of this schema and cannot be \
             deleted. Remove the checkpoint with DeleteSchemaCheckpoint first."
        )
    }

    /// `DeleteSchemaVersions` whose `Versions` names the first version of the schema set.
    /// Stated by the operation: "You cannot use the DeleteSchemaVersions API to delete the
    /// first schema version in the schema set. The first schema version can only be deleted
    /// by the DeleteSchema API."
    ///
    /// NOT recorded: what is pinned is the behaviour — the operation's modelled
    /// `InvalidInputException`, with nothing deleted — and a sentence stating that
    /// documented requirement, carrying the version the request named.
    pub fn first_schema_version_not_deletable(version_number: i64) -> String {
        format!(
            "Schema version {version_number} is the first version of this schema and can only \
             be deleted by the DeleteSchema API."
        )
    }

    /// A classifier READ (`GetClassifier`, and the `UpdateClassifier` that addresses one
    /// the same way) for a name no classifier in this account has.
    ///
    /// Recorded template, reproduced as the service words this error class and NOT
    /// shortened: the sentence names the classifier the request addressed — the only
    /// variable part, read off the request — and then states the miss a second time,
    /// because the service composes its per-resource sentence with its generic one. A
    /// client matching on the refusal sees that doubled wording, so dropping the second
    /// half would be a divergence rather than a tidy-up.
    pub fn classifier_not_found(name: &str) -> String {
        format!("Classifier {name} not found. not found.")
    }

    /// The DELETE of a classifier this account holds none of (`DeleteClassifier`).
    ///
    /// Recorded template, and a DIFFERENT sentence from the read's above — which is why
    /// the two are not one shared string, exactly as the connection read and write keep
    /// their own ([`connection_not_found`], [`connection_not_found_for_write`]): the
    /// delete states the miss ONCE, over the whole address it resolved — the account whose
    /// namespace was searched and the requested name, joined by a colon, because a
    /// classifier name is unique per account. The name is read off the request, exactly as
    /// sent; the account is the one this emulator serves, as
    /// [`custom_entity_type_not_found`] reads it.
    pub fn classifier_not_found_for_delete(name: &str) -> String {
        format!(
            "Classifier {}:{name} not found.",
            ::engine::emulator_account_id()
        )
    }

    /// `CreateClassifier` for a name already taken.
    ///
    /// Recorded template, reproduced as the service words this error class and NOT
    /// shortened: the sentence names the classifier the create asked for — the only
    /// variable part, read off the request — and then states the collision a second time,
    /// for the same reason the read's miss above states it twice.
    pub fn classifier_already_exists(name: &str) -> String {
        format!("Classifier {name} already exists. already exists")
    }

    /// `CreateCustomEntityType` for a `Name` this account already holds a custom pattern
    /// under. Recorded constant, and the whole message: the service states the collision
    /// over the resource kind and the namespace a custom pattern's name is unique in, and
    /// echoes back neither the name the create asked for nor the account.
    pub const CUSTOM_ENTITY_TYPE_ALREADY_EXISTS: &str =
        "A Custom Entity Type with the same name exist in this account";

    /// `CreateClassifier` naming none of the four classifier members. The kind to create
    /// is "which field of the request is present" (`CreateClassifier`), so a request
    /// naming no field asks for no classifier at all; the sentence states that
    /// requirement over the four members, the way the schema-registry
    /// [`super::REGISTRY_ID_REQUIRES_ONE`] states its own.
    pub const CLASSIFIER_REQUIRES_ONE: &str = "One of GrokClassifier, XMLClassifier, \
         JsonClassifier or CsvClassifier has to be provided.";

    /// `CreateClassifier` naming SEVERAL of them, which names no one kind either: the
    /// kind is decided by which field is present, so two present fields describe two
    /// different classifiers under one name.
    pub const CLASSIFIER_ALLOWS_ONLY_ONE: &str = "Only one of GrokClassifier, \
         XMLClassifier, JsonClassifier or CsvClassifier may be provided.";

    /// A CSV classifier write whose `QuoteSymbol` is the character its `Delimiter`
    /// already is. Stated by the model: "A custom symbol to denote what combines content
    /// into a single column value. It must be different from the column delimiter"
    /// (`CsvClassifier$QuoteSymbol`).
    pub const CSV_QUOTE_SYMBOL_MUST_DIFFER: &str =
        "QuoteSymbol must be different from the column delimiter.";

    /// `UpdateClassifier` naming a kind the stored classifier of that name is not: the
    /// kind a request is about is the field it carries ("Modifies an existing classifier
    /// … depending on which field is present"), and a stored classifier's kind is fixed
    /// at create, so such a request describes a classifier that does not exist. NOT
    /// recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException` — and a sentence built from the request: the name it
    /// addressed and the two kinds, each spelled as the model's member for it.
    pub fn classifier_kind_mismatch(name: &str, stored: &str, requested: &str) -> String {
        format!("Classifier {name} is a {stored} and cannot be updated as a {requested}.")
    }

    /// A custom entity type lookup for a name this account holds no custom pattern
    /// under — the miss of every operation that addresses one by `Name`
    /// (`GetCustomEntityType`, `DeleteCustomEntityType`).
    ///
    /// Recorded template: the sentence names the pattern the request addressed and the
    /// account whose namespace was searched, because a custom pattern's name "must be
    /// unique per Amazon Web Services account" (`CustomEntityType$Name`) and so the
    /// account is part of the address that resolved to nothing. The name is read off the
    /// request, exactly as sent; the account is the one this emulator serves, not a value
    /// the request carries.
    pub fn custom_entity_type_not_found(name: &str) -> String {
        format!(
            "An existing Custom Entity Type: {name} cannot be found for Account Id {}",
            ::engine::emulator_account_id()
        )
    }

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

    /// `CreateTrigger` for a name the request's Region already holds a trigger under, where
    /// the definition submitted is the stored one. NOT
    /// recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the requested name.
    pub const TRIGGER_ALREADY_EXISTS: &str = "Trigger already exists.";

    /// A create for a resource name already taken whose request asks for a DIFFERENT
    /// resource than the stored one, on an operation that models
    /// `IdempotentParameterMismatchException` ("The same unique identifier was associated
    /// with two different records") and so treats that name as the create's idempotency
    /// identifier.
    ///
    /// RECORDED template: fixed text with ONE variable part, the RESOURCE kind as the
    /// service names it. It discloses neither the name the request addressed nor which
    /// member of the definition differs.
    pub fn already_submitted_with_different_configuration(resource: &str) -> String {
        format!("{resource} already submitted with different configuration.")
    }

    /// A trigger definition whose `Actions` list is absent or empty, on the create that
    /// would store it or the update that would overwrite one with it, and every other
    /// member a trigger's `Type` requires it to carry: a `Schedule` for a scheduled
    /// trigger ("This field is required when the trigger type is SCHEDULED" —
    /// `CreateTriggerRequest$Schedule`) and a `Predicate` for a conditional one ("This
    /// field is required when the trigger type is CONDITIONAL").
    ///
    /// RECORDED template, with no trailing period: the refusal names the MEMBER that has
    /// to carry something and says that neither no value nor an empty one will do, and it
    /// discloses neither the trigger nor the operation nor the type the request named — so
    /// one sentence serves every definition member this service judges, whichever write
    /// submitted it.
    pub fn trigger_member_required(member: &str) -> String {
        format!("{member} cannot be null or empty")
    }

    /// A CONDITIONAL trigger definition naming one job both in a condition of its
    /// `Predicate` and in one of its `Actions` ([`super::job_is_predicate_and_action`]).
    ///
    /// RECORDED constant, and the whole message: it states the rule the definition breaks
    /// and names neither the job it was broken over nor the trigger, so a client sees the
    /// same sentence whichever job it doubled up. It is a refusal of the DEFINITION, so it
    /// is answered whether or not that job exists.
    pub const JOB_IN_PREDICATE_AND_ACTION: &str =
        "Job cannot be a Predicate and Action for Conditional Trigger.";

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
    /// `GetJobRun`, `GetJobRuns`, `BatchStopJobRun`), and the job an action of a trigger
    /// would run.
    ///
    /// Recorded constant: the job-definition miss is FIXED text, trailing period
    /// included, and has no variable part — the refusal names neither the requested job,
    /// nor the Region, nor the resource that referred to the job. One constant, shared by
    /// every job-definition lookup, so a client matching on the sentence recognises the
    /// miss whichever operation reported it.
    pub const JOB_NOT_FOUND: &str = "Job not found.";

    /// `CreateJob` for a `Name` the request's Region already holds a job definition
    /// under, where the definition submitted is the stored one — so the create is a
    /// retry of the one that stored it rather than a second, different job under one
    /// name ([`already_submitted_with_different_configuration`] words that one).
    ///
    /// NOT recorded, because what the corpus records for this name collision is the
    /// idempotency mismatch: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the requested name, which is also how the recorded job-definition miss above
    /// withholds it.
    pub const JOB_ALREADY_EXISTS: &str = "Job already exists.";

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

    /// A job run addressed by id that the job it was addressed through does not hold —
    /// the read (`GetJobRun`), the retry a start names (`StartJobRun$JobRunId`) and each
    /// entry of a stop the job holds no such run for (`BatchStopJobRun`).
    ///
    /// Recorded CONSTANT, and the whole message: the service states the pair it could not
    /// resolve a run for — the job name and the run id — in fixed words and echoes back
    /// neither of them, so the refusal a client matches on is the same whatever was
    /// asked. Shared by every job-run lookup, so one operation's miss cannot drift from
    /// another's.
    pub const JOB_RUN_NOT_FOUND: &str = "Job run not found for the given job name and runId.";

    /// A bookmark operation (`GetJobBookmark`, `ResetJobBookmark`) addressing a job that
    /// has no bookmark: one no run of it has written yet, or one whose run the request's
    /// `RunId` names no run of.
    ///
    /// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `EntityNotFoundException` — and a sentence following the per-resource template of
    /// [`database_not_found`], built from the job the request addressed, which with the
    /// account and Region is the whole address of a bookmark.
    pub fn job_bookmark_not_found(job_name: &str) -> String {
        format!("Job bookmark for job {job_name} not found.")
    }

    /// `GetDashboardUrl` for a `ResourceType` the operation does not serve a dashboard
    /// for. Recorded template: the refusal names the operation and the resource type the
    /// request asked for, which is the member it is refused over.
    pub fn dashboard_url_not_supported(resource_type: &str) -> String {
        format!("GetDashboardUrl operation is not supported for {resource_type}")
    }

    /// Any operation whose `ResourceId`/`Id` names no session of this account
    /// (`GetDashboardUrl` for a `ResourceType` of `SESSION`). NOT recorded: the sentence
    /// follows the per-resource template of [`database_not_found`], built from the id the
    /// request addressed, which is the whole address of a session.
    pub fn session_not_found(id: &str) -> String {
        format!("Session {id} not found.")
    }

    /// A read of the Glue Identity Center configuration of an account that has none
    /// (`GetGlueIdentityCenterConfiguration`), and the delete of the same absent
    /// singleton.
    ///
    /// Recorded constant: the refusal names the Identity Center application Glue could not
    /// find and states what has to happen first; it has no variable part, because the
    /// account is the whole address of the configuration.
    pub const IDENTITY_CENTER_CONFIGURATION_NOT_FOUND: &str =
        "Idc application not found. Need to create glue application first.";

    /// `CreateGlueIdentityCenterConfiguration` for an account that already has one. NOT
    /// recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and a sentence naming
    /// the resource kind alone, which here is the whole address of the singleton.
    pub const IDENTITY_CENTER_CONFIGURATION_ALREADY_EXISTS: &str =
        "Glue Identity Center configuration already exists.";

    /// `CreateGlueIdentityCenterConfiguration` whose `InstanceArn` is not the ARN of an
    /// Identity Center instance ([`super::identity_center_instance_id`]). NOT recorded:
    /// what is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException` — and a sentence carrying the value the request sent plus
    /// the form the member has to take, as [`malformed_version_id`] does for its own.
    pub fn malformed_identity_center_instance_arn(instance_arn: &str) -> String {
        let segment = super::IDENTITY_CENTER_INSTANCE_SEGMENT;
        format!(
            "InstanceArn {instance_arn} is not the ARN of an Identity Center instance, \
             which names its instance as arn:aws:sso:::{segment}<instance id>"
        )
    }

    /// A table-optimizer READ (`GetTableOptimizer`) for a type the addressed table has no
    /// optimizer of.
    ///
    /// RECORDED template: the sentence names the optimizer type and then the table the
    /// request addressed, which together are the whole address of an optimizer, and it is
    /// followed by the diagnostic clause this service appends to such a refusal
    /// ([`with_diagnostic`]) — stating the 404 the catalog raised the miss with, even
    /// though the call itself is answered with the `EntityNotFoundException` shape's own
    /// status.
    pub fn table_optimizer_not_found(table_name: &str, optimizer_type: &str) -> String {
        with_diagnostic(
            &format!("No optimizer of type {optimizer_type} found for the table: {table_name}"),
            DIAGNOSTIC_SERVICE_GLUE,
            DIAGNOSTIC_STATUS_NOT_FOUND,
            DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
        )
    }

    /// The DELETE (`DeleteTableOptimizer`) for the same miss. Recorded template, and a
    /// DIFFERENT sentence from the read's above — which is why the two are not one shared
    /// string: it states that no optimizer of that type exists ON the table, carrying the
    /// same two variable parts and the same diagnostic clause.
    pub fn table_optimizer_not_found_for_delete(table_name: &str, optimizer_type: &str) -> String {
        with_diagnostic(
            &format!("An optimizer of type {optimizer_type} does not exist on table {table_name}"),
            DIAGNOSTIC_SERVICE_GLUE,
            DIAGNOSTIC_STATUS_NOT_FOUND,
            DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
        )
    }

    /// Any table-optimizer operation whose `TableName` names a stored table whose type
    /// cannot be optimized — one that is no open-table-format table
    /// ([`super::TableEntry::is_iceberg_table`]).
    ///
    /// RECORDED template: the fixed validation sentence plus the diagnostic clause
    /// ([`with_diagnostic`]) stating the status and the code the validation was raised
    /// with. It is a refusal of the REQUEST rather than a miss, so it names neither the
    /// table nor the type the request asked for.
    pub fn unsupported_table_type() -> String {
        with_diagnostic(
            "Request Validation Failed: Unsupported Table type",
            DIAGNOSTIC_SERVICE_GLUE,
            DIAGNOSTIC_STATUS_VALIDATION,
            DIAGNOSTIC_CODE_INVALID_INPUT,
        )
    }

    /// `CreateTableOptimizer` for a table whose parameters name no Iceberg metadata file
    /// ([`super::TableEntry::iceberg_metadata_location`]).
    ///
    /// Registering an optimizer reads the table's current metadata file, because that is
    /// what names the data files the optimizer would work on — so a table that carries no
    /// filepath is refused whatever its declared type is.
    ///
    /// RECORDED template: the fixed sentence plus the diagnostic clause
    /// ([`with_diagnostic`]), naming neither the table nor the parameter it was read from.
    pub fn table_metadata_filepath_missing() -> String {
        with_diagnostic(
            "Glue table metadata filepath is null",
            DIAGNOSTIC_SERVICE_GLUE,
            DIAGNOSTIC_STATUS_VALIDATION,
            DIAGNOSTIC_CODE_INVALID_INPUT,
        )
    }

    /// Any table-optimizer operation whose `DatabaseName`/`TableName` resolves to no table
    /// of the addressed Data Catalog.
    ///
    /// RECORDED template: this surface resolves the table through the Data Catalog, so what
    /// a client is shown is the CATALOG's own refusal of that read — the same constant the
    /// catalog's table read answers ([`TABLE_ENTITY_NOT_FOUND`]), carrying the catalog
    /// component's diagnostic clause — wrapped in this service's clause, each with its own
    /// request id. It names neither the table nor the database, because the catalog's read
    /// miss names neither.
    pub fn table_optimizer_entity_not_found() -> String {
        with_diagnostic(
            &with_diagnostic(
                TABLE_ENTITY_NOT_FOUND,
                DIAGNOSTIC_SERVICE_DATA_CATALOG,
                DIAGNOSTIC_STATUS_VALIDATION,
                DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
            ),
            DIAGNOSTIC_SERVICE_GLUE,
            DIAGNOSTIC_STATUS_NOT_FOUND,
            DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
        )
    }

    /// The component a diagnostic clause names for a refusal THIS service raised.
    const DIAGNOSTIC_SERVICE_GLUE: &str = "glue";

    /// The component a diagnostic clause names for a refusal the DATA CATALOG raised, which
    /// this service surfaces wrapped in its own
    /// ([`table_optimizer_entity_not_found`]).
    const DIAGNOSTIC_SERVICE_DATA_CATALOG: &str = "AmazonDataCatalog";

    /// The `Error Code` a clause states for a reference that resolved to nothing.
    const DIAGNOSTIC_CODE_ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

    /// The `Error Code` a clause states for a request the service would not accept.
    const DIAGNOSTIC_CODE_INVALID_INPUT: &str = "InvalidInputException";

    /// The `Status Code` a clause states for a reference that resolved to nothing: the
    /// status the component RAISED the miss with, which is not the status the endpoint
    /// answers the call with — the awsJson client-error status the modelled shape carries.
    /// As recorded, including for the clause the catalog's own refusal is wrapped in.
    const DIAGNOSTIC_STATUS_NOT_FOUND: u16 = 404;

    /// The `Status Code` a clause states for a refused request, and the one the CATALOG
    /// component states for the miss it raises: as recorded.
    const DIAGNOSTIC_STATUS_VALIDATION: u16 = 400;

    /// `sentence` followed by the diagnostic clause this service appends to a refusal its
    /// framework raised:
    /// ` (Service: <component>; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`.
    ///
    /// The request id is the clause's only per-run field: nothing in the request carries
    /// one, so it is minted per call from the emulator's one request-id mint
    /// (`service_common::query_error_envelope::new_request_id`, a fresh v4 UUID) and is
    /// indistinguishable from the one the framework's own renderers report. A clause
    /// wrapping another's text states its own component, status and code, so one refusal
    /// can carry the inner component's clause and this service's both.
    fn with_diagnostic(
        sentence: &str,
        service: &str,
        status_code: u16,
        error_code: &str,
    ) -> String {
        format!(
            "{sentence} (Service: {service}; Status Code: {status_code}; Error Code: \
             {error_code}; Request ID: {}; Proxy: null)",
            ::service_common::query_error_envelope::new_request_id()
        )
    }

    /// `CreateTableOptimizer` for a type the addressed table already has an optimizer of.
    /// NOT recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// what the request addressed.
    pub const TABLE_OPTIMIZER_ALREADY_EXISTS: &str = "Table optimizer already exists.";

    /// A read that has to resolve a table's metadata through the source a FEDERATED
    /// database points at ([`super::federated_source_refusal`]).
    ///
    /// NOT recorded as a sentence: what is pinned is the behaviour — 400 with the modelled
    /// `FederationSourceRetryableException`, the code that tells a client to retry rather
    /// than give up — and a sentence naming the database the request addressed and the
    /// connection its registration points the source at, both read off the stored
    /// registration.
    pub fn federated_source_unreachable(database_name: &str, source: &str) -> String {
        format!(
            "Federated database {database_name} could not be reached through its source \
             {source}."
        )
    }

    /// A `CreateTable` whose `OpenTableFormatInput` names an open table format with no
    /// input of its own. The member carries exactly one format
    /// (`OpenTableFormatInput$IcebergInput`), so a value carrying none asks for no table.
    /// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException` — and a sentence naming the member that has to be there.
    pub const OPEN_TABLE_FORMAT_REQUIRES_ICEBERG: &str =
        "OpenTableFormatInput must specify IcebergInput.";

    /// A `CreateTable` of an open-table-format table that names no location for it. An
    /// Iceberg table's metadata is written under the table's own location, so a create
    /// that names none has nowhere to mint it. NOT recorded: what is pinned is the
    /// behaviour — 400 with the operation's modelled `InvalidInputException` — and a
    /// sentence naming the two members the location may come from.
    pub const ICEBERG_TABLE_REQUIRES_LOCATION: &str =
        "An Iceberg table requires a location, in TableInput.StorageDescriptor.Location \
         or in IcebergInput.CreateIcebergTableInput.Location.";

    /// A `CreateTable` of an open-table-format table that carries neither a `TableInput`
    /// nor the Iceberg input that would define one. NOT recorded: what is pinned is the
    /// behaviour — 400 with the operation's modelled `InvalidInputException` — and a
    /// sentence naming the members the table definition may come from.
    pub const ICEBERG_TABLE_REQUIRES_DEFINITION: &str =
        "An Iceberg table requires a TableInput, or an \
         IcebergInput.CreateIcebergTableInput defining the table.";

    /// `UpdateTable` whose `VersionId` is not the version the table is currently under —
    /// one a later write has replaced, or one the table never had.
    ///
    /// Recorded constant: the refusal states that the update lost a race and names neither
    /// the version the request carried nor the one the table is under, so a client sees the
    /// same sentence whichever stale version it sent.
    pub const TABLE_UPDATE_CONCURRENT_MODIFICATION: &str =
        "Update table failed due to concurrent modifications.";

    /// `StartJobRun` for a `JobName` the endpoint's Region holds no job definition for —
    /// a name nothing created, or one created through ANOTHER Region's endpoint.
    /// Recorded constant: the start's refusal names the metadata it could not read rather
    /// than the miss every other job lookup reports ([`JOB_NOT_FOUND`]), which is why the
    /// start does not share that sentence.
    pub const JOB_METADATA_MISSING: &str = "Failed to start job run due to missing metadata.";

    /// A tag write whose `ResourceArn` names a Region other than the one the endpoint
    /// serving the request is in (`TagResource`). Recorded constant: the refusal states
    /// only that the ARN belongs elsewhere and names neither the ARN nor either Region,
    /// and it is a client-INPUT refusal rather than a miss — a Glue ARN of another Region
    /// is a bad parameter, whether or not that Region holds the resource.
    ///
    /// This is the WRITE's wording and not yet this crate's answer everywhere: the tag
    /// READ (`GetTags`) still answers that condition with the kind's miss, which is what
    /// the cross-service tag projection's own suite pins it to. Only a recording of the
    /// read's refusal settles whether the two are one sentence.
    pub const ARN_WRONG_REGION: &str = "Arn passed doesn't belong to this region.";

    /// A listing whose `MaxResults` asks for a page smaller than one entry
    /// (`crate::ops::get_tables`). NOT recorded: what is pinned is the behaviour — 400
    /// with the operation's modelled `InvalidInputException` rather than the cross-service
    /// `ValidationException` — and a sentence carrying the value the request sent and the
    /// smallest page the member admits, which the model declares for it.
    pub fn page_size_below_minimum(requested: i32, minimum: i32) -> String {
        format!("MaxResults {requested} is below the minimum page size of {minimum}")
    }

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

    /// A read that addresses ONE catalog resource by its `CatalogId` (`GetCatalog`) for a
    /// name the addressed account's Data Catalog holds no catalog under. Recorded
    /// constant: the whole sentence is fixed text, and the id the request addressed is
    /// deliberately withheld — like [`DATABASE_ALREADY_EXISTS`] and unlike
    /// [`database_not_found`], whose recorded wording does name its resource.
    pub const CATALOG_NOT_FOUND: &str = "Catalog not found.";

    /// `CreateCatalog` for a `CatalogInput` that names no source outside the Glue Data
    /// Catalog. Recorded constant: the whole sentence is fixed text, naming neither the
    /// catalog the request asked for nor any member of its input.
    pub const CREATE_NATIVE_CATALOG_UNSUPPORTED: &str =
        "Create glue native catalog is not supported.";

    /// `CreateCatalog` for a federated catalog whose data source this account holds no
    /// registration for ([`super::is_registered_federation_source`]). Recorded template:
    /// the sentence names, in brackets, the data source the input pointed at and nothing
    /// else of the request.
    pub fn federation_source_not_registered(source: &str) -> String {
        format!("The data source [{source}] is not registered.")
    }

    /// `CreateCatalog` for a `Name` the account already holds a catalog under. NOT
    /// recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the requested name.
    pub const CATALOG_ALREADY_EXISTS: &str = "Catalog already exists.";

    /// `CreateCatalog` whose `Name` is the account's own id. Stated by the model: the
    /// catalog's name "Cannot be the same as the account ID" (`Catalog$Name`), because
    /// that id already addresses the account's own Data Catalog. The sentence is that
    /// requirement; what is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException`.
    pub const CATALOG_NAME_IS_ACCOUNT_ID: &str =
        "Catalog name cannot be the same as the account ID.";

    /// `CreateCatalog` whose `Name` is outside the `@pattern` its modelled type declares
    /// ([`super::is_catalog_name`]).
    ///
    /// A value outside a MODELLED constraint is refused at request validation, before the
    /// operation runs, in the clause shape that renderer uses (`N validation error(s)
    /// detected: Value '…' at '<path>' failed to satisfy constraint: <requirement>` —
    /// [`::aws_smithy_schema_server::Violation::message`]). This pattern reaches the
    /// handler only because the framework's regex engine cannot compile a negative
    /// look-ahead, so the refusal states the same two facts: the member path and the
    /// pattern the model declares.
    pub fn malformed_catalog_name(name: &str) -> String {
        let path = ::aws_smithy_schema_server::constraints::path::member("Name");
        format!(
            "1 validation error detected: Value '{name}' at '{path}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {}",
            super::CATALOG_NAME_PATTERN
        )
    }

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

    /// `CreateConnection` for a `ConnectionInput$Name` the addressed Data Catalog already
    /// holds a connection under. Recorded constant, and the whole message: the service
    /// states the resource kind and withholds the name the create asked for, as the
    /// recorded `CreateDatabase` duplicate does for its own resource.
    pub const CONNECTION_ALREADY_EXISTS: &str = "Connection already exists.";

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
    /// does not hold, and the version DELETE (`DeleteTableVersion`) for the same miss.
    /// Recorded constant: the refusal names neither the version the request addressed nor
    /// the table it was addressed in.
    pub const TABLE_VERSION_NOT_FOUND: &str = "Table version not found.";

    /// The per-item failure of the BATCH version delete (`BatchDeleteTableVersion`) for a
    /// version the addressed table's history does not hold. Recorded constant, and a
    /// SHORTER sentence than the single-version delete's [`TABLE_VERSION_NOT_FOUND`] above
    /// — which is why the two are not one shared string: an entry of `Errors` already names
    /// the table and the version it is about in its own members, so the message says only
    /// what went wrong with that item.
    pub const BATCH_TABLE_VERSION_NOT_FOUND: &str = "Version not found.";

    /// Any table-version operation whose `VersionId` spells no version counter
    /// ([`super::parse_version_counter`]) — the version read's and the version delete's
    /// one refusal for an id that is not a version id at all. NOT recorded: the sentence
    /// restates the requirement the model declares for the member ("A VersionID is a
    /// string representation of an integer", `GetTableVersionRequest$VersionId`) over the
    /// value the request sent; what is pinned is the behaviour — 400 with the operation's
    /// modelled `InvalidInputException`.
    pub fn malformed_version_id(version_id: &str) -> String {
        format!("VersionId {version_id} is not a string representation of an integer")
    }

    /// `DeleteTableVersion` (and the per-version failure of `BatchDeleteTableVersion`) for
    /// the version a table is CURRENTLY under. Recorded constant: the newest version of a
    /// table is its live definition, which only `DeleteTable` removes, so the refusal
    /// names no version and no table.
    pub const TABLE_VERSION_IS_LATEST: &str = "VersionId cannot be equal to the latest.";

    /// Any schema-registry operation whose `RegistryId` resolves to a registry this
    /// endpoint holds none of — the parent-registry miss of a schema create, and the miss
    /// of every operation that addresses a registry directly. Built from the resolved
    /// registry name alone, like [`database_not_found`].
    pub fn registry_not_found(name: &str) -> String {
        format!("Registry {name} not found.")
    }

    /// An operation of the integration API family that addresses ONE record by the ARN of
    /// the resource it belongs to, for a `ResourceArn` this account holds no such record
    /// for — the integration resource property read
    /// (`GetIntegrationResourceProperty`), the resource miss of an integration table
    /// properties write, and the miss of a read or a delete of integration table properties
    /// whose resource carries none at all
    /// ([`ComAmazonawsGlueAwsglueState::integration_table_properties_miss`]). RECORDED template: the sentence is fixed around its one variable
    /// part, the ARN the request addressed, taken verbatim from `ResourceArn`; what is
    /// pinned is the behaviour — `EntityNotFoundException`.
    pub fn target_resource_not_found(resource_arn: &str) -> String {
        format!("The target resource {resource_arn} is not found.")
    }

    /// `CreateIntegrationResourceProperty` whose request states NEITHER
    /// `SourceProcessingProperties` nor `TargetProcessingProperties`. Both members are
    /// optional on their own, so this is the service's own requirement rather than a
    /// modelled constraint. RECORDED constant: the whole sentence is fixed — it names the
    /// two sides in prose and interpolates neither the addressed resource nor a member
    /// path — and the refusal is the operation's modelled `ValidationException`.
    pub const INTEGRATION_PROCESSING_PROPERTIES_REQUIRED: &str =
        "Either source processing properties or target processing properties must be provided \
         in the request.";

    /// An operation that addresses ONE integration table property record
    /// (`GetIntegrationTableProperties`, `DeleteIntegrationTableProperties`) for a
    /// `TableName` whose `ResourceArn` carries integration table properties of OTHER tables
    /// but none of that pair; a resource carrying none at all earns
    /// [`target_resource_not_found`] instead
    /// ([`ComAmazonawsGlueAwsglueState::integration_table_properties_miss`]). Recorded
    /// template: the fixed text around the two
    /// members the request addressed the record by, which the service names `sourceArn` and
    /// `tableName`, and no trailing period.
    pub fn integration_table_properties_not_found(resource_arn: &str, table_name: &str) -> String {
        format!(
            "Failed to find the sourceTableProperties with sourceArn {resource_arn} and tableName \
             {table_name}"
        )
    }

    /// A `CreateIntegrationTableProperties` (or its update) that states BOTH table
    /// configurations in one call. Recorded constant, entirely fixed text: the model names
    /// both members optional, and the service's rule is that one call carries the
    /// configuration of the ONE leg its `ResourceArn` names — "the same API need to be
    /// invoked with the Glue connection ARN as ResourceArn with SourceTableConfig, and the
    /// Glue database ARN as ResourceArn with TargetTableConfig respectively".
    pub const INTEGRATION_TABLE_CONFIG_SIDES_EXCLUSIVE: &str =
        "Source and Target config can't both be set.";

    /// A write of a `SourceTableConfig` that states a member the service does not process
    /// yet ([`super::UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS`]). Recorded constant,
    /// entirely fixed text: it names neither the member nor the value that was refused.
    pub const UNSUPPORTED_SOURCE_TABLE_PROPERTIES: &str =
        "Unsupported source table properties are provided.";

    /// Any schema-registry operation whose `SchemaId` resolves to a schema set this
    /// endpoint holds none of (`GetSchema`, `DeleteSchemaVersions`, and the parent-schema
    /// miss of every version operation).
    ///
    /// RECORDED constant, and the whole message: the service states the miss in FIXED
    /// words and echoes back neither the schema the request addressed nor the registry it
    /// was addressed in — the same shape as the recorded `GetTable` miss. Shared by every
    /// schema lookup, so one operation's miss cannot drift from another's.
    pub const SCHEMA_NOT_FOUND: &str = "Schema is not found.";

    /// `CreateSchema` for a `SchemaName` its registry already holds.
    ///
    /// Recorded template: the fixed collision sentence followed by the MEMBER the create
    /// addressed the schema set by, spelled as the model spells it, and the value it sent
    /// — the only variable part. The registry the name was taken in is not disclosed.
    pub fn schema_already_exists(schema_name: &str) -> String {
        format!("Schema already exists. SchemaName: {schema_name}")
    }

    /// A column statistics task run lookup (`GetColumnStatisticsTaskRun`) for an id no
    /// run in this account has. NOT recorded: the sentence follows the per-resource
    /// template of the recorded crawler miss above, built from the requested id, which is
    /// the whole address of a run.
    pub fn column_statistics_task_run_not_found(run_id: &str) -> String {
        format!("Column statistics task run with id {run_id} does not exist")
    }

    /// A column statistics task settings read (`GetColumnStatisticsTaskSettings`) for a
    /// table no create has written settings for. RECORDED constant: the whole sentence is
    /// fixed — it names the MEMBERS the request addressed the entry by rather than the
    /// values it sent, so neither the database nor the table name appears in it.
    pub const COLUMN_STATISTICS_TASK_SETTINGS_NOT_FOUND: &str =
        "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist";

    /// `CreateColumnStatisticsTaskSettings` addressed at a `TableName` the addressed
    /// database holds no table under. RECORDED constant: the whole sentence is fixed — it
    /// names the MEMBERS the request addressed the table by rather than the values it
    /// sent, like this operation's other recorded refusals, so the requested table name is
    /// withheld. A SEPARATE string from the catalog's own [`table_not_found`] template,
    /// which is the recorded wording of the table writes (`DeleteTable`, `UpdateTable`).
    pub const COLUMN_STATISTICS_TASK_SETTINGS_TABLE_NOT_FOUND: &str =
        "Table passed as input within input database not found.";

    /// `CreateColumnStatisticsTaskSettings` for a table whose settings a create already
    /// wrote. NOT recorded: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
    /// the addressed table.
    pub const COLUMN_STATISTICS_TASK_SETTINGS_ALREADY_EXIST: &str =
        "Column statistics task settings already exist.";

    /// `StartColumnStatisticsTaskRun` for a table that already has a run in flight — the
    /// operation's modelled `ColumnStatisticsTaskRunningException`, "thrown when you try
    /// to start another job while running a column stats generation job". RECORDED
    /// constant: the whole sentence is fixed, naming neither the database, the table nor
    /// the in-flight run, so a client can recognise the transient conflict by the text
    /// alone.
    pub const COLUMN_STATISTICS_TASK_RUN_ALREADY_STARTED: &str =
        "Column Statistics task for input table has already started";

    /// `CreateColumnStatisticsTaskSettings` while a statistics run of the addressed table
    /// is in flight — the same modelled `ColumnStatisticsTaskRunningException`. RECORDED
    /// constant, and a SEPARATE string from the start's above: the two recorded sentences
    /// differ in the case of "statistics", so each operation answers with its own rather
    /// than sharing one.
    pub const COLUMN_STATISTICS_TASK_SETTINGS_RUN_ALREADY_STARTED: &str =
        "Column statistics task for input table has already started";

    /// `StopColumnStatisticsTaskRun` for a table with no statistics run in flight — the
    /// operation's modelled `ColumnStatisticsTaskNotRunningException`, "thrown when you
    /// try to stop a task run when there is no task running". NOT recorded: what is
    /// pinned is the behaviour, and the sentence is fixed like the in-flight refusals
    /// above, naming neither the database nor the table.
    pub const COLUMN_STATISTICS_TASK_NOT_RUNNING: &str =
        "Column Statistics task for input table is not running";

    /// `CreateRegistry` for a `RegistryName` this account already holds. NOT recorded
    /// either: inferred from the recorded `CreateDatabase` duplicate, like
    /// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of the
    /// requested name.
    pub const REGISTRY_ALREADY_EXISTS: &str = "Registry already exists.";

    /// `CreateWorkflow` for a `Name` this account already holds a workflow under.
    ///
    /// Recorded constant, and the whole message: the service states the collision over
    /// the MEMBER the create named the workflow in rather than the value, so the
    /// requested name is withheld — reproduced as recorded, its capital `E` and its
    /// missing trailing period included, because a client recognises the refusal by its
    /// wording.
    pub const WORKFLOW_ALREADY_EXISTS: &str = "Workflow with given name already Exists";

    /// `CreateBlueprint` for a `Name` this account already holds a blueprint under.
    ///
    /// Recorded constant, and the whole message: like [`WORKFLOW_ALREADY_EXISTS`] it
    /// names the member the create addressed the blueprint by and withholds the value,
    /// with no trailing period, as recorded.
    pub const BLUEPRINT_ALREADY_EXISTS: &str = "Blueprint with given name already exists";

    /// Any blueprint operation whose `Name` names no blueprint this account holds
    /// (`GetBlueprint`, and the same miss on every sibling that addresses one by name).
    /// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `EntityNotFoundException` — and the sentence follows [`database_not_found`]'s
    /// per-resource template, built from the requested name alone.
    pub fn blueprint_not_found(name: &str) -> String {
        format!("Blueprint {name} not found.")
    }

    /// A blueprint operation whose blueprint has no ACTIVE definition to work from — a run
    /// started of a registration that has not finished, or of one that finished without a
    /// definition ([`super::BlueprintEntry::is_active`]).
    ///
    /// RECORDED template: the sentence is fixed and its one variable part is the blueprint
    /// the request named, taken from the request's own `BlueprintName`. The start is refused
    /// with the operation's modelled `IllegalBlueprintStateException`, "the blueprint is in
    /// an invalid state to perform a requested operation".
    pub fn blueprint_has_no_active_definition(name: &str) -> String {
        format!("No active definition found for Blueprint: {name}")
    }

    /// A blueprint run that could not create the workflow it lays out because this account
    /// already holds a workflow of that name: `BlueprintRun$ErrorMessage`, "indicates any
    /// errors that are seen while running the blueprint". NOT recorded: what is pinned is the
    /// behaviour — the run reaches the modelled failed state having created nothing — and the
    /// sentence names the workflow the run would have created, which is the one thing a
    /// caller needs in order to act on it.
    pub fn blueprint_run_workflow_exists(workflow_name: &str) -> String {
        format!(
            "The blueprint run could not create workflow {workflow_name}: a workflow of that \
             name already exists."
        )
    }

    /// A blueprint run lookup (`GetBlueprintRun`) for a `RunId` the named blueprint holds no
    /// run under. NOT recorded: the sentence follows the per-resource template of
    /// [`workflow_run_not_found`], built from the ADDRESS the request sent — the blueprint
    /// and the run id, which together name a blueprint run.
    pub fn blueprint_run_not_found(blueprint_name: &str, run_id: &str) -> String {
        format!("Blueprint run {run_id} not found for blueprint {blueprint_name}.")
    }

    /// Any workflow operation whose `Name` names no workflow this account holds
    /// (`GetWorkflow`, and the same miss on every sibling that addresses one by name). NOT
    /// recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `EntityNotFoundException` — and the sentence follows [`database_not_found`]'s
    /// per-resource template, built from the requested name alone.
    pub fn workflow_not_found(name: &str) -> String {
        format!("Workflow {name} not found.")
    }

    /// A run-history read (`GetWorkflowRuns`) of a workflow this account DOES hold whose
    /// history has no run to report. `GetWorkflowRunsResponse$Runs` is `length 1..1000`, so
    /// a success carrying nothing is unrepresentable and the read is refused with the
    /// operation's modelled `EntityNotFoundException` instead. NOT recorded: the sentence is
    /// the fixed text Glue answers a run-history read with when there is no run history —
    /// it names no resource, because the entity that is missing is the run history itself
    /// and not the workflow, which was found.
    pub fn no_workflow_runs_found() -> String {
        "No workflow runs were found.".to_string()
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

    /// A machine learning transform lookup (`GetMLTransform`, `UpdateMLTransform`,
    /// `DeleteMLTransform`, or a walk over one transform's task runs) for a `TransformId` no
    /// transform in this account has.
    ///
    /// RECORDED template: the sentence is fixed and its two variable parts are the account
    /// whose namespace holds no such transform and the `TransformId` the request itself
    /// sent, which is the whole address of a transform — a `Name` is not, because "Names are
    /// not guaranteed unique and can be changed at any time" (`MLTransform$Name`). Every
    /// operation that misses a transform refuses it in THESE words, so one miss cannot be
    /// worded one way by a read and another by a write.
    pub fn ml_transform_not_found(transform_id: &str) -> String {
        format!(
            "Cannot find MLTransform in account {} with handle {transform_id}",
            ::engine::emulator_account_id()
        )
    }

    /// A machine learning task run lookup (`GetMLTaskRun`) for a `TaskRunId` the addressed
    /// transform holds no run under.
    ///
    /// NOT recorded: the sentence follows the template this same machine learning surface
    /// words its recorded parent miss in ([`ml_transform_not_found`]) — the account whose
    /// namespace the lookup was served in and the id the request itself sent — because the
    /// run and its transform are missed by the same surface, so one miss cannot be worded
    /// one way for the parent and another for the child. The `TaskRunId` is the half of a
    /// run's address the transform could not resolve, so it is what the sentence names.
    pub fn ml_task_run_not_found(task_run_id: &str) -> String {
        format!(
            "Cannot find MLTaskRun in account {} with handle {task_run_id}",
            ::engine::emulator_account_id()
        )
    }

    /// `CreateMLTransform` for a `Name` this account already holds a transform under.
    ///
    /// RECORDED template: the sentence is fixed and its one variable part is the `Name` the
    /// request itself sent, which is the name the create asked to take. The create is
    /// refused with the operation's modelled `AlreadyExistsException`, "a resource to be
    /// created or added already exists".
    pub fn ml_transform_already_exists(name: &str) -> String {
        format!("Transform with name {name} already exists")
    }

    /// `StartDataQualityRuleRecommendationRun` for a data source whose recommendation run
    /// has not finished. NOT recorded: what is pinned is the behaviour — 400 with the
    /// operation's modelled `ConflictException` — and the sentence names the table the
    /// request addressed, the way the recorded per-resource templates name their
    /// resource.
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

    /// A data quality ruleset evaluation run lookup
    /// (`GetDataQualityRulesetEvaluationRun`) for an id no run in this account has. NOT
    /// recorded: the sentence follows the per-resource template of
    /// [`data_quality_rule_recommendation_run_not_found`], built from the requested id,
    /// which is the whole address of a run.
    pub fn data_quality_ruleset_evaluation_run_not_found(run_id: &str) -> String {
        format!("Data quality ruleset evaluation run with id {run_id} does not exist")
    }

    /// A data quality run start whose `DataSource` carries neither kind of Glue table. NOT
    /// recorded: what is pinned is the behaviour — 400 with the operation's modelled
    /// `InvalidInputException` — and the sentence names the members the input has to carry
    /// one of, the way [`PARTITION_VALUES_REQUIRED`] names its member.
    pub const DATA_SOURCE_REQUIRES_A_TABLE: &str =
        "DataSource must specify either a GlueTable or a DataQualityGlueTable.";

    /// A data quality ruleset lookup for a name this account holds no ruleset under — the
    /// miss of every operation that addresses one by `Name` (`GetDataQualityRuleset`, and
    /// the update and delete that address a ruleset the same way), and of the evaluation
    /// start that names the rulesets to evaluate in its `RulesetNames`.
    ///
    /// Recorded template: the sentence names the account whose namespace was searched and
    /// then the ruleset the request addressed, because a ruleset's name is "a unique name
    /// for the data quality ruleset" per account and so the account is part of the address
    /// that resolved to nothing. The name is read off the request, exactly as sent; the
    /// account is the one this emulator serves, not a value the request carries.
    pub fn data_quality_ruleset_not_found(name: &str) -> String {
        format!(
            "Cannot find Data Quality Ruleset in account {} with name {name}",
            ::engine::emulator_account_id()
        )
    }

    /// `CreateDataQualityRuleset` for a name this account already holds a ruleset under.
    ///
    /// Recorded template: the fixed collision sentence plus the `Name` the create asked
    /// for, its one variable part, read off the request exactly as sent.
    pub fn data_quality_ruleset_already_exists(name: &str) -> String {
        format!("Another ruleset with the same name already exists: {name}")
    }

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

    /// A partition update (`UpdatePartition`, one entry of `BatchUpdatePartition`) whose
    /// `PartitionInput` names values other than the partition the request addresses. NOT
    /// recorded: what is pinned is the behaviour — `InvalidInputException`, per entry for a
    /// batch update — and the sentence states the documented rule the entry broke, that an
    /// update writes a partition's definition and never moves it to another address
    /// (`UpdatePartitionRequest$PartitionInput`). No stored value is disclosed.
    pub const PARTITION_VALUES_CANNOT_BE_CHANGED: &str =
        "PartitionInput must specify the values of the partition being updated; the Values \
         property can't be changed.";

    /// A partition read (`GetPartition`) for a value list the addressed table holds no
    /// partition for, an update (`UpdatePartition`, one entry of `BatchUpdatePartition`)
    /// of such a value list, which has no partition to write to either, and a delete (one
    /// item of `BatchDeletePartition`) of one, which has none to remove. RECORDED
    /// constant: the whole sentence is fixed — it names neither the values the request
    /// addressed nor the table they were looked for in.
    pub const PARTITION_NOT_FOUND: &str = "Cannot find partition.";

    /// A partition-metadata read of the Lake Formation application-integration surface
    /// (`GetUnfilteredPartitionMetadata`, `GetUnfilteredPartitionsMetadata`) made with
    /// ordinary credentials rather than through a vended query session. RECORDED: the
    /// refusal carries NO wording at all — an empty message — so it discloses nothing
    /// about the catalog, and in particular nothing about whether the addressed partition
    /// exists.
    pub const UNFILTERED_PARTITION_METADATA_ACCESS_DENIED: &str = "";

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

    /// A session create for an `Id` this Region already holds a session under. NOT
    /// recorded: the sentence follows the per-resource template of [`session_not_found`],
    /// built from the id the request sent, which is the whole address of a session, and the
    /// behaviour it goes with is 400 with the create's modelled `AlreadyExistsException`.
    pub fn session_already_exists(id: &str) -> String {
        format!("Session {id} already exists.")
    }

    /// A session create whose `GlueVersion` fails the restriction the model states for it
    /// ([`super::session_glue_version_defect`]). NOT recorded: what is pinned is the
    /// behaviour — 400 with the create's modelled `InvalidInputException` — and a sentence
    /// carrying the value the request sent plus the bound it has to clear, as
    /// [`malformed_identity_center_instance_arn`] does for its own member.
    pub fn session_glue_version_too_low(glue_version: &str) -> String {
        let minimum = super::MIN_SESSION_GLUE_VERSION;
        format!(
            "GlueVersion {glue_version} is not supported for sessions, whose GlueVersion \
             must be greater than {minimum:.1}"
        )
    }

    /// A session create this endpoint cannot take another session for. Its own capacity,
    /// not an AWS quota, so the sentence says which ceiling was reached.
    pub fn too_many_sessions(ceiling: usize) -> String {
        format!("this endpoint holds {ceiling} sessions, which is as many as it can hold")
    }

    /// A statement run on a session whose `SessionStatus` is not
    /// [`super::SESSION_STATUS_READY`], which is the only status a remote Spark runtime
    /// can take code in. NOT recorded word for word: what is pinned is the behaviour —
    /// 400 with the run's modelled `IllegalSessionStateException` — so the sentence names
    /// the session the request addressed, the status it is actually in and the status it
    /// would have to be in, every part of it read off the request and the store.
    pub fn session_not_ready(id: &str, status: &str) -> String {
        let ready = super::SESSION_STATUS_READY;
        format!("Session {id} is in {status} status, and a statement can only be run on a session in {ready} status.")
    }

    /// A statement run on a session that is still actioning one: the runtime is busy, so
    /// it cannot take the code. RECORDED: what is pinned is the behaviour — 400 with the
    /// run's modelled `InvalidInputException`, which is the refusal this service answers a
    /// session already running a statement with, rather than the `SessionBusyException`
    /// the model also declares for the operation. The sentence names the session and the
    /// statement it is still running, which is what a caller waits on.
    pub fn session_busy_with_statement(id: &str, statement_id: i32) -> String {
        format!(
            "Session {id} is still running statement {statement_id}, so it cannot take \
             another statement yet."
        )
    }

    /// A statement read (`GetStatement`, and every other operation that addresses a
    /// statement by its session and `Id`) for an id that session never minted. Follows
    /// the per-resource template of [`session_not_found`], built from the two members that
    /// together are the whole address of a statement.
    pub fn statement_not_found(session_id: &str, id: i32) -> String {
        format!("Statement {id} not found in session {session_id}.")
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

/// The `@pattern` a catalog's `Name` must satisfy: `^(?!(.*[.\/\\]|aws:)).*$`
/// (`CatalogNameString`, `smithy_query shape=CatalogNameString`) — a name carrying none
/// of the three path separators anywhere in it, and not opening with the `aws:` prefix
/// AWS reserves for its own resources.
pub const CATALOG_NAME_PATTERN: &str = r"^(?!(.*[.\/\\]|aws:)).*$";

/// The prefix AWS reserves for its own resources: a catalog's own name may not open with
/// it ([`CATALOG_NAME_PATTERN`]), and the connections of AWS's own managed federation
/// integrations are named under it ([`is_managed_federation_connection`]).
pub const RESERVED_NAME_PREFIX: &str = "aws:";

/// Whether `name` satisfies [`CATALOG_NAME_PATTERN`].
///
/// Checked here rather than left to the boundary validator because that pattern uses a
/// negative look-ahead, which the framework's regex engine cannot compile, and a
/// `@pattern` it cannot compile is treated as matching every value — which would let a
/// name the model forbids through to the store. The `@length(1, 64)` half of the
/// constraint IS enforced upstream, so it is not repeated here.
pub fn is_catalog_name(name: &str) -> bool {
    !name.contains(['.', '/', '\\']) && !name.starts_with(RESERVED_NAME_PREFIX)
}

/// Whether the connection `name` is one of the federation connections AWS's OWN managed
/// integrations use, rather than one an account creates for itself.
///
/// Such a connection is named under the prefix AWS reserves for its own resources
/// ([`RESERVED_NAME_PREFIX`]) — the prefix a tenant's catalog name may not take either —
/// and no `CreateConnection` of this account can hold it.
pub fn is_managed_federation_connection(name: &str) -> bool {
    name.starts_with(RESERVED_NAME_PREFIX)
}

/// Whether the data source `federated` points at is REGISTERED for the account this
/// endpoint serves, and so one a catalog of that account may be federated to.
///
/// The sources an account holds a registration for without having registered anything are
/// the ones AWS's own managed integrations register for it, and an account reaches those
/// through the integration's own federation connection
/// ([`is_managed_federation_connection`]). Every other source is one the account has to
/// register with the data lake itself before a catalog may be federated to it: the Glue
/// connection a federation names is a connection TO the source
/// (`FederatedCatalog$ConnectionName`: "The name of the connection to an external data
/// source, for example a Redshift-federated catalog"), not a registration OF it, so
/// holding one does not make its source available — and a federation naming no connection
/// at all names nothing through which a source could be reached.
///
/// This emulator records no registration of its own, so a source outside AWS's managed
/// integrations is unregistered here, which is what a `CreateCatalog` naming one is
/// refused for ([`messages::federation_source_not_registered`]).
pub fn is_registered_federation_source(federated: &FederatedCatalog) -> bool {
    federated
        .connection_name
        .as_deref()
        .is_some_and(is_managed_federation_connection)
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

/// Whether `catalog_id` names the ACCOUNT-level Data Catalog this emulator holds.
///
/// It holds exactly one of those: the one for its own account, which is the catalog the
/// model documents as the default. A catalog id of any other account belongs to that
/// account's estate, reachable on the real service only through a Data Catalog resource
/// policy, so a request that names one is refused rather than quietly given a new, empty
/// catalog. The catalog RESOURCES a `CreateCatalog` registers inside this account carry
/// their own ids ([`child_catalog_id`]) and are addressed through the catalog operations
/// that take one ([`ComAmazonawsGlueAwsglueState::catalog_definition`]); the databases of
/// such a catalog are not served through the account-level operations yet.
pub fn is_known_catalog(catalog_id: &str) -> bool {
    catalog_id == ::engine::emulator_account_id()
}

/// The account's own Data Catalog as the modelled `Catalog` shape, as addressed in
/// `region`.
///
/// Every account HAS this catalog: it is the default a request that names no `CatalogId`
/// resolves to ([`resolve_catalog_id`]) and the container the account's databases and its
/// created catalogs live in, so an operation that addresses it by the account's own id
/// reports it instead of searching the records a `CreateCatalog` wrote
/// ([`ComAmazonawsGlueAwsglueState::catalog_definition`]).
///
/// Its name and id are the account's own number, and its ARN is the Data Catalog's own
/// ([`catalog_arn`]) rather than one naming a catalog resource inside it. It carries no
/// definition members and no instants: no `CreateCatalog` wrote it, so there is no
/// `CatalogInput` behind it and no write to timestamp.
pub fn account_data_catalog(region: &str) -> crate::ops::types::Catalog {
    let account_id = ::engine::emulator_account_id().to_string();
    crate::ops::types::Catalog {
        catalog_id: Some(account_id.clone()),
        name: account_id.clone(),
        resource_arn: Some(catalog_arn(region, &account_id)),
        ..crate::ops::types::Catalog::default()
    }
}

/// The `CatalogId` of the catalog `name` inside the Data Catalog of `account_id`: the
/// account the catalog was created in, then the catalog's own name.
///
/// A `CreateCatalog` carries no parent id — it creates the catalog in the Data Catalog of
/// the account it is served for — and the catalog it created is then addressed by this
/// id, which is what [`parse_catalog_reference`] reads back apart.
pub fn child_catalog_id(account_id: &str, name: &str) -> String {
    format!("{account_id}:{name}")
}

/// A `CatalogId` naming one catalog RESOURCE, split into the account whose Data Catalog
/// holds it and the catalog's own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogReference {
    /// The account the catalog was created in: the part before the separator, or this
    /// emulator's own account for an id that is a bare catalog name, which names a
    /// catalog of the account the request is served for.
    pub account_id: String,
    /// The catalog's name, as the request spelled it.
    pub name: String,
}

/// The catalog resource `catalog_id` names ([`child_catalog_id`] read back apart), for
/// every operation that addresses one.
///
/// A bare name is a catalog of the account this endpoint serves, which is the default the
/// model documents for a `CatalogId` ("If none is provided, the Amazon Web Services
/// Account Number is used by default") — so `GetCatalog` reaches a catalog of its own
/// account by name alone, and a catalog of another account only by naming that account.
///
/// The account prefix is told apart from a catalog name that itself carries a colon by
/// what an account prefix IS: an Amazon Web Services account number
/// ([`ACCOUNT_ID_DIGITS`]). An id that is such a number and nothing else names that
/// account's own Data Catalog, which HOLDS catalogs but is not one of them, so it is
/// resolved into that account and a name no catalog can carry — the account's id, which
/// `Catalog$Name` cannot be.
pub fn parse_catalog_reference(catalog_id: &str) -> CatalogReference {
    if let Some((account_id, name)) = catalog_id
        .split_once(':')
        .filter(|(prefix, _)| is_account_id(prefix))
    {
        return CatalogReference {
            account_id: account_id.to_string(),
            name: name.to_string(),
        };
    }
    if is_account_id(catalog_id) {
        return CatalogReference {
            account_id: catalog_id.to_string(),
            name: catalog_id.to_string(),
        };
    }
    CatalogReference {
        account_id: ::engine::emulator_account_id().to_string(),
        name: catalog_id.to_string(),
    }
}

/// How many digits an Amazon Web Services account number carries. It is what tells a
/// `CatalogId`'s optional account prefix from a catalog name, and so from a name that
/// carries a colon of its own (`CatalogNameString`'s pattern forbids the three path
/// separators and the `aws:` prefix, not a colon).
const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is spelled as an Amazon Web Services account number: twelve decimal
/// digits ([`ACCOUNT_ID_DIGITS`]).
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
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
    glue_arn_for_account(region, catalog_id, CATALOG_ARN_RESOURCE)
}

/// `Catalog$ResourceArn`: "the Amazon Resource Name (ARN) assigned to the catalog
/// resource" — the ARN of the catalog `name` created in the Data Catalog of `account_id`,
/// as addressed in `region`.
///
/// The catalog itself is the resource TYPE ([`CATALOG_ARN_RESOURCE`]) and the catalog's
/// name the resource, which is the layout every other Glue resource ARN follows
/// ([`TaggedResourceKind::arn`]).
pub fn catalog_resource_arn(region: &str, account_id: &str, name: &str) -> String {
    glue_arn_for_account(region, account_id, &format!("{CATALOG_ARN_RESOURCE}/{name}"))
}

/// A Glue ARN for `resource` in the estate of `account_id`, as addressed in `region`.
///
/// The canonical layout and partition come from the shared minter, so this ARN matches
/// every other ARN this emulator emits; only the ACCOUNT field is replaced, because a
/// request may address a catalog of another account and that account is what such an ARN
/// names.
fn glue_arn_for_account(region: &str, account_id: &str, resource: &str) -> String {
    let canonical = ::engine::emulator_arn(ARN_NAMESPACE, region, resource);
    let mut fields: Vec<&str> = canonical.split(':').collect();
    // Field 5 of an ARN is the account; the canonical form always has it.
    if let Some(account) = fields.get_mut(4) {
        *account = account_id;
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

/// How many tags one resource's tag set may hold at once.
///
/// Source: the modelled `TagsMap` bound (len 0..50), which is the shape BOTH a tag
/// write's `TagsToAdd` and `GetTags`'s `Tags` target — so a tag set past this many
/// entries could not be reported by the read that serves it. A write that would carry a
/// resource past it is refused instead of stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

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

/// The message a refusal carries for a tag write that would leave the resource holding
/// more tags than one tag set may have ([`MAX_TAGS_PER_RESOURCE`]).
///
/// Derived from the request, like this crate's other refusals whose wording no recording
/// or doc states: it names how many tags the write would leave and the bound it passes.
/// What is pinned is the behaviour — 400 with `InvalidInputException`, and nothing
/// stored.
pub fn tag_limit_exceeded_message(would_hold: usize) -> String {
    format!(
        "TagsToAdd would leave the resource holding {would_hold} tags, more than the \
         {MAX_TAGS_PER_RESOURCE} allowed per resource"
    )
}

/// A wall-clock instant as the modelled timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The same instant as the `LongValue` members that carry "the unix time and date"
/// (`Statement$StartedOn`) report it: milliseconds since the epoch, which is the scale a
/// Glue statement's instants are stated in.
pub fn to_unix_millis(at: SystemTime) -> i64 {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// How many digits the instant part of a continuation token is padded to, so the token
/// orders the same way as a string as the instant it names orders as a number. Twenty
/// digits hold any nanosecond count this clock can report.
const CURSOR_INSTANT_DIGITS: usize = 20;

/// One record's place in a time-ordered listing, as that listing's continuation token
/// renders it: the instant the listing orders by, zero-padded so the string orders as the
/// number does, then the record's own id to separate two records stamped in the same
/// nanosecond.
///
/// ONE copy, shared by every listing of this crate that pages over a run history
/// (`GetJobRuns`, `GetColumnStatisticsTaskRuns`), so the token format cannot drift between
/// them. Comparing a token against this key is what resumes a page, which is why any
/// string a request sends names a well-defined place in the order.
pub fn history_page_cursor(ordered_by: SystemTime, id: &str) -> String {
    let at = ordered_by
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{at:0CURSOR_INSTANT_DIGITS$}:{id}")
}

/// The `ColumnStatisticsState` a column statistics task run is recorded in by
/// `StartColumnStatisticsTaskRun`: the first of the modelled enum
/// (`STARTING | RUNNING | SUCCEEDED | FAILED | STOPPED`), because the run has been
/// accepted and nothing has begun generating statistics yet.
pub const COLUMN_STATISTICS_TASK_RUN_STATE_STARTING: &str = "STARTING";
/// See [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`].
pub const COLUMN_STATISTICS_TASK_RUN_STATE_FAILED: &str = "FAILED";

/// See [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`]: the state
/// `StopColumnStatisticsTaskRun` leaves the run it stopped in.
pub const COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED: &str = "STOPPED";

/// The `ColumnStatisticsState` values a column statistics task run never moves out of
/// again: of the modelled enum quoted on
/// [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`], the ones that name a finished run. The
/// other two (`STARTING`, `RUNNING`) each still owe a further state.
pub const TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES: [&str; 3] = [
    "SUCCEEDED",
    COLUMN_STATISTICS_TASK_RUN_STATE_FAILED,
    COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED,
];

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

    /// Whether this run is the one in flight for that catalog, database and table: it has
    /// not finished and it covers exactly that table.
    ///
    /// ONE predicate, so the run a start is refused for, the run a settings create is
    /// refused for and the run a stop ends cannot drift apart. `database` and `table` are
    /// already folded the way the catalog stores them ([`fold_name`]).
    pub fn is_running_for(&self, catalog_id: &str, database: &str, table: &str) -> bool {
        !self.is_terminal()
            && self.catalog_id == catalog_id
            && self.database_name == database
            && self.table_name == table
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

    /// Move this run into the state a stop leaves it in.
    ///
    /// Of the modelled `ColumnStatisticsState` values the one that names a run somebody
    /// stopped, which is terminal — so the run owes the clock no further state, its end
    /// instant is the stop, and it carries no failure reason: nothing failed.
    pub fn stop(&mut self, now: SystemTime) {
        self.settles_at = None;
        self.status = COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED.to_string();
        self.end_time = Some(now);
        self.last_updated = now;
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
            // scheduled state, the same state a stated crawler schedule is written in
            // ([`stated_schedule_state`]). No operation suspends a column-statistics
            // schedule, so this record never leaves that state.
            schedule: self
                .schedule_expression
                .as_ref()
                .map(|expression| Schedule {
                    schedule_expression: Some(expression.clone()),
                    state: Some(stated_schedule_state(Some(expression))),
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
    /// The blueprint run that created this workflow, and `None` for a workflow a create
    /// registered directly.
    ///
    /// "The details of the blueprint that this particular workflow is created from"
    /// (`Workflow$BlueprintDetails`) is what a read reports it as, so a workflow a blueprint
    /// run laid out names the run it came from and one `CreateWorkflow` wrote names none.
    pub blueprint_run: Option<WorkflowBlueprintRun>,
}

/// Store `entry` in `workflows` under its own name, the key every workflow read addresses it
/// by.
///
/// The ONE place this namespace's key form lives, so a write that reaches the map through a
/// split borrow of the store ([`ComAmazonawsGlueAwsglueState::start_blueprint_run`]) keys it
/// exactly as [`ComAmazonawsGlueAwsglueState::insert_workflow`] does.
fn insert_workflow_into(workflows: &mut BTreeMap<String, WorkflowEntry>, entry: WorkflowEntry) {
    workflows.insert(entry.name.clone(), entry);
}

/// The blueprint run a workflow was created by: "The details of a blueprint"
/// (`BlueprintDetails`), which is the blueprint and the run of it that laid the workflow out.
#[derive(Debug, Clone)]
pub struct WorkflowBlueprintRun {
    /// `BlueprintDetails$BlueprintName`: "The name of the blueprint."
    pub blueprint_name: String,
    /// `BlueprintDetails$RunId`: "The run ID for this blueprint."
    pub run_id: String,
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
            // "The details of the blueprint that this particular workflow is created from":
            // a workflow `CreateWorkflow` registered was created from none.
            blueprint_run: None,
        }
    }

    /// Build the workflow a blueprint run lays out: the workflow that run produces, carrying
    /// the blueprint run it was created from.
    ///
    /// The members a `CreateWorkflow` sets — a description, default run properties, a
    /// concurrency ceiling, tags — are defined by the blueprint's published archive, which
    /// nothing here reads, so the workflow is registered with none of them rather than with
    /// values invented for it; the same goes for the jobs, crawlers and triggers that archive
    /// lays out INSIDE the workflow, so none of those is created either.
    pub fn from_blueprint_run(
        name: &str,
        blueprint_name: &str,
        run_id: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            default_run_properties: None,
            max_concurrent_runs: None,
            created_on: now,
            last_modified_on: now,
            tags: HashMap::new(),
            // A workflow the run has just laid out has not been run.
            runs: Vec::new(),
            blueprint_run: Some(WorkflowBlueprintRun {
                blueprint_name: blueprint_name.to_string(),
                run_id: run_id.to_string(),
            }),
        }
    }

    /// Apply an `UpdateWorkflow` request to this stored workflow, member by member, as
    /// served at `now`.
    ///
    /// Every member the request CARRIES replaces the stored one and every member it OMITS
    /// is preserved: the request is a patch over the definition the create registered, the
    /// same way an `UpdateCrawler` ([`CrawlerEntry::apply_update`]) or an
    /// `UpdateMLTransform` ([`MlTransformEntry::apply_update`]) is, and unlike `UpdateJob`,
    /// whose submitted definition is documented to overwrite the stored one completely.
    /// `Name` ADDRESSES the workflow rather than changing it (`UpdateWorkflowRequest` has no
    /// rename member), and the request has no member for the create instant, the tags or the
    /// runs already started of the workflow, so none of those is this request's to change.
    ///
    /// A member whose value is a WHOLE collection is replaced whole rather than merged key
    /// by key: `DefaultRunProperties` is "a collection of properties to be used as part of
    /// each execution of the workflow", so a request naming that collection sets it, which is
    /// what makes merging the business of `PutWorkflowRunProperties` — the operation that
    /// writes individual properties, and onto one RUN rather than onto the workflow.
    ///
    /// The new defaults apply to the runs STARTED AFTER this write: a run already begun
    /// holds the properties it began with ([`WorkflowRunEntry::run_properties`]).
    /// `LastModifiedOn` becomes `now`, since it "moves with every write to this workflow".
    pub fn apply_update(&mut self, input: &UpdateWorkflowInput, now: SystemTime) {
        if let Some(description) = input.description.as_ref() {
            self.description = Some(description.clone());
        }
        if let Some(default_run_properties) = input.default_run_properties.as_ref() {
            self.default_run_properties = Some(default_run_properties.clone());
        }
        if let Some(max_concurrent_runs) = input.max_concurrent_runs {
            self.max_concurrent_runs = Some(max_concurrent_runs);
        }
        self.last_modified_on = now;
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

    /// Begin a run of this workflow at `now` and answer the id minted for it, or the
    /// concurrency ceiling the run would have exceeded.
    ///
    /// The ONE place a run of a workflow is recorded, whatever began it: a caller's
    /// `StartWorkflowRun`, or a trigger of this workflow that fired. Everything the clock
    /// already owed this workflow's runs is materialised first, so a run the clock has
    /// already finished does not count as one in flight, and the ceiling the workflow named
    /// is then what decides whether another run may begin.
    ///
    /// `requested_properties` are the run properties the caller asked this run to carry,
    /// and `None` for a run nobody passed any for — a trigger firing passes none, so such a
    /// run carries the workflow's own defaults. `total_actions` is how many actions the
    /// entry point that began the run initiates.
    pub fn begin_run(
        &mut self,
        requested_properties: Option<&HashMap<String, String>>,
        now: SystemTime,
        total_actions: i32,
    ) -> Result<String, i32> {
        self.settle_due_runs(now);
        // "You can use this parameter to prevent unwanted multiple updates to data … If you
        // leave this parameter blank, there is no limit to the number of concurrent workflow
        // runs": past a ceiling the workflow named, no further run begins.
        if let Some(ceiling) = self.concurrent_run_ceiling() {
            if self.runs_in_flight() >= usize::try_from(ceiling).unwrap_or(usize::MAX) {
                return Err(ceiling);
            }
        }
        let run_id = new_workflow_run_id();
        let run = WorkflowRunEntry::begin(
            self,
            requested_properties,
            run_id.clone(),
            now,
            total_actions,
        );
        self.record_run(run);
        Ok(run_id)
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
    /// `last_run` is "the information about the last execution of the workflow" projected
    /// by the STORE, which holds the runs that execution's actions began
    /// ([`ComAmazonawsGlueAwsglueState::to_workflow`]).
    pub fn to_workflow(
        &self,
        graph: Option<WorkflowGraph>,
        last_run: Option<WorkflowRun>,
    ) -> Workflow {
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
            last_run,
            graph,
            max_concurrent_runs: self.max_concurrent_runs,
            // "The details of the blueprint that this particular workflow is created
            // from": the blueprint run that laid this workflow out, and nothing at all for
            // a workflow a create registered directly.
            blueprint_details: self.blueprint_run.as_ref().map(|origin| BlueprintDetails {
                blueprint_name: Some(origin.blueprint_name.clone()),
                run_id: Some(origin.run_id.clone()),
            }),
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

/// One job run an action of a workflow run began: the job definition it is a run of and
/// the id that addresses it inside that definition ([`JobEntry::run`]).
///
/// A job run carries the trigger that started it (`JobRun$TriggerName`) but no workflow
/// run, so this pair is what ties the run an action began back to the workflow run that
/// initiated the action — which is what a `JOB` node of a run's graph reports and what the
/// run's statistics count.
#[derive(Debug, Clone)]
pub struct ActionJobRun {
    /// The name of the job definition the run belongs to.
    pub job_name: String,
    /// `JobRun$Id` of the run.
    pub run_id: String,
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
    /// The job runs this run's actions began, in the order they were initiated.
    ///
    /// Only the IDENTITY of each run is held here: its state, and everything else a read
    /// reports about it, is read out of the job definition the run belongs to, so a read of
    /// this workflow run reports what the clock has since made of each of its actions.
    pub action_job_runs: Vec<ActionJobRun>,
}

impl WorkflowRunEntry {
    /// Record a run of `workflow` whatever began it — a caller's `StartWorkflowRun`, or a
    /// trigger of the workflow that fired — in [`WORKFLOW_RUN_STATUS_RUNNING`] with its
    /// next status due [`WORKFLOW_RUN_SETTLES_AFTER`] out.
    ///
    /// `requested_properties` are the run properties asked for this run, and `None` when
    /// nothing asked for any. `total_actions` is how many actions the entry point that
    /// began this run initiates, which is the run's total action count.
    pub fn begin(
        workflow: &WorkflowEntry,
        requested_properties: Option<&HashMap<String, String>>,
        id: String,
        now: SystemTime,
        total_actions: i32,
    ) -> Self {
        // "A collection of properties to be used as part of each execution of the
        // workflow" is the base every run starts from, and "the workflow run properties
        // for the new workflow run" (`StartWorkflowRunRequest$RunProperties`) are this
        // run's own, so they win where both name a key.
        let mut run_properties = workflow.default_run_properties.clone().unwrap_or_default();
        if let Some(requested) = requested_properties {
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
            // The actions are initiated by the call that began this run, which records what
            // they began ([`ComAmazonawsGlueAwsglueState::record_workflow_run_job_runs`]).
            action_job_runs: Vec::new(),
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
    /// read handles it ([`WorkflowEntry::to_workflow`]). `statistics` are this run's
    /// statistics, which count the runs its actions began and so are computed by the STORE
    /// that holds those runs
    /// ([`ComAmazonawsGlueAwsglueState::workflow_run_statistics`]) rather than here.
    pub fn to_workflow_run(
        &self,
        graph: Option<WorkflowGraph>,
        statistics: WorkflowRunStatistics,
    ) -> WorkflowRun {
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
            statistics: Some(statistics),
            graph,
            // "The batch condition that started the workflow run": only an EVENT trigger's
            // batch condition starts a run, and a start a caller made carries none.
            starting_event_batch_condition: None,
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
    /// (`Node$TriggerDetails`). `job_runs` are "the information for the job runs represented
    /// by the job node" (`JobNodeDetails$JobRuns`), which only a JOB node has and only for a
    /// graph reported beside a workflow run.
    fn node(
        &mut self,
        node_type: &str,
        name: &str,
        trigger: Option<&TriggerEntry>,
        job_runs: &[JobRun],
    ) -> String {
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
                // (`JobNodeDetails$JobRuns`): the runs the workflow run this graph is
                // reported beside began of this job, and no details at all for a node with
                // none — a job no run of this workflow has begun, or a graph reported beside
                // no run.
                job_details: (!job_runs.is_empty()).then(|| JobNodeDetails {
                    job_runs: Some(job_runs.to_vec()),
                }),
                // "A list of crawls represented by the crawl node"
                // (`CrawlerNodeDetails$Crawls`): nothing here records a crawl of a crawler
                // ([`CrawlerEntry`] holds none), so there is no crawl of this workflow to
                // report.
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
    /// of time (in seconds) that the run consumed resources".
    pub fn execution_time_seconds(&self) -> i32 {
        data_quality_run_execution_time_seconds(self.started_on, self.completed_on)
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

/// How long a data quality run that started at `started_on` consumed resources, in whole
/// seconds, given the instant it completed: `ExecutionTime`, "the amount of time (in
/// seconds) that the run consumed resources", which is the span between the instant it
/// started and the instant it completed. A run still in flight has consumed no completed
/// time, so it reports the member's own default of 0.
///
/// Shared by every kind of data quality run, which all model that member the same way.
fn data_quality_run_execution_time_seconds(
    started_on: SystemTime,
    completed_on: Option<SystemTime>,
) -> i32 {
    completed_on
        .and_then(|completed| completed.duration_since(started_on).ok())
        .map_or(0, |consumed| {
            i32::try_from(consumed.as_secs()).unwrap_or(i32::MAX)
        })
}

/// How long a data quality ruleset evaluation run stays in
/// [`DATA_QUALITY_RUN_STATE_STARTING`] before the clock owes it its terminal status.
///
/// The span a rule recommendation run is given
/// ([`DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER`]): both are data quality runs this
/// emulator accepts and finishes on its own clock, and Glue publishes a duration for
/// neither, so one chosen span serves both — deliberately not zero, so a caller that reads
/// `GetDataQualityRulesetEvaluationRun` straight after starting a run still observes it in
/// flight, and the refusal that goes with a run in flight.
pub const DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER: Duration =
    DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER;

/// How many data quality ruleset evaluation runs this account keeps, for the same reason
/// and with the same window as [`MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS`]. The OLDEST
/// record is dropped first.
pub const MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS: usize =
    MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS;

/// How many data quality ruleset evaluation run ids this process has minted. A nonce source
/// for [`new_data_quality_ruleset_evaluation_run_id`], exactly as
/// [`MINTED_DATA_QUALITY_RULE_RECOMMENDATION_RUN_IDS`] is for a recommendation run id.
static MINTED_DATA_QUALITY_RULESET_EVALUATION_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `RunId`: the id the service assigns a data quality ruleset evaluation run when it
/// starts.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255`, exactly as [`new_data_quality_rule_recommendation_run_id`] mints a
/// recommendation run's. It is stored on the run, so every later read of that run reports
/// the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_data_quality_ruleset_evaluation_run_id() -> String {
    mint_hex_id(
        &MINTED_DATA_QUALITY_RULESET_EVALUATION_RUN_IDS,
        DATA_QUALITY_RUN_ID_WORDS,
    )
}

/// One catalog table a data quality ruleset evaluation run evaluates: the run reads every
/// one of them, so each is kept as an address the catalog can be asked about rather than as
/// the `DataSource` it arrived in.
///
/// The two names are kept AS THE REQUEST SPELLED THEM, because they are what a refusal and a
/// failed run's `ErrorString` name the table by; every lookup goes through
/// [`ComAmazonawsGlueAwsglueState::catalog_table_miss`], which folds them the way the catalog
/// stores a name, so another casing of a stored table still resolves to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedTable {
    /// The catalog the table was resolved in, as [`resolve_catalog_id`] resolved it.
    pub catalog_id: String,
    /// The database's name, as the request spelled it.
    pub database_name: String,
    /// The table's name, as the request spelled it.
    pub table_name: String,
}

impl EvaluatedTable {
    /// The address `table` names inside `catalog_id`.
    pub fn of(table: &DataSourceTable, catalog_id: &str) -> Self {
        Self {
            catalog_id: catalog_id.to_string(),
            database_name: table.database_name.clone(),
            table_name: table.table_name.clone(),
        }
    }
}

/// One data quality ruleset evaluation run, as `StartDataQualityRulesetEvaluationRun`
/// recorded it.
///
/// A run carries the data sources it evaluates and the rulesets it evaluates over them, the
/// configuration the request named — with each member the request left unset filled in with
/// the default the model documents for it — plus the members only a run has: its id, its
/// status and the instants it moved between statuses.
#[derive(Debug, Clone)]
pub struct DataQualityRulesetEvaluationRunEntry {
    /// `RunId`, minted by [`new_data_quality_ruleset_evaluation_run_id`].
    pub id: String,
    /// The `DataSource` the start named, reported back as it arrived.
    pub data_source: DataSource,
    /// The table that `DataSource` addresses, as the catalog stores it.
    pub primary_table: EvaluatedTable,
    /// `AdditionalDataSources`: "a map of reference strings to additional data sources you
    /// can specify for an evaluation run", reported back as they arrived.
    pub additional_data_sources: Option<HashMap<String, DataSource>>,
    /// The tables those additional data sources address, in the order of the reference
    /// strings they arrived under, so the run reads them in one deterministic order.
    pub additional_tables: Vec<EvaluatedTable>,
    /// `Role`: "an IAM role supplied to encrypt the results of the run."
    pub role: String,
    /// `NumberOfWorkers`, defaulting to [`DEFAULT_DATA_QUALITY_RUN_WORKERS`].
    pub number_of_workers: i32,
    /// `Timeout` in minutes, defaulting to [`DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES`].
    pub timeout: i32,
    /// The `ClientToken` the start carried, which is what makes a repeat of that start the
    /// duplicate the service refuses instead of a second run.
    pub client_token: Option<String>,
    /// `AdditionalRunOptions`, as the start named them.
    pub additional_run_options: Option<DataQualityEvaluationRunAdditionalRunOptions>,
    /// `RulesetNames`: the rulesets this run evaluates, as the start listed them.
    pub ruleset_names: Vec<String>,
    /// One of the modelled `TaskStatusType` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// `StartedOn`: "the date and time when this run started."
    pub started_on: SystemTime,
    /// `LastModifiedOn`: "the date and time when this run was modified."
    pub last_modified_on: SystemTime,
    /// `CompletedOn`: "the date and time when this run was completed", absent until the run
    /// has finished.
    pub completed_on: Option<SystemTime>,
    /// `ErrorString`: "the error strings that are associated with the run", absent unless
    /// the run failed.
    pub error_string: Option<String>,
    /// `ResultIds`: "a list of result IDs for the data quality results for the run" — one id
    /// per ruleset the run evaluated, minted when the run finishes evaluating them, so a run
    /// still in flight and a run that could not evaluate anything carry none.
    pub result_ids: Vec<String>,
}

impl DataQualityRulesetEvaluationRunEntry {
    /// Record a run of the rulesets a `StartDataQualityRulesetEvaluationRun` request lists
    /// over the data sources it names, in [`DATA_QUALITY_RUN_STATE_STARTING`] with its
    /// terminal status due [`DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER`] out.
    ///
    /// `primary_table` is the table the request's `DataSource` addresses and
    /// `additional_tables` the ones its `AdditionalDataSources` address, each already
    /// resolved into the catalog that serves it.
    pub fn start(
        input: &StartDataQualityRulesetEvaluationRunInput,
        primary_table: EvaluatedTable,
        additional_tables: Vec<EvaluatedTable>,
        id: String,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            data_source: input.data_source.clone(),
            primary_table,
            additional_data_sources: input.additional_data_sources.clone(),
            additional_tables,
            role: input.role.clone(),
            number_of_workers: input
                .number_of_workers
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            timeout: input
                .timeout
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            client_token: input.client_token.clone(),
            additional_run_options: input.additional_run_options.clone(),
            ruleset_names: input.ruleset_names.clone(),
            status: DATA_QUALITY_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                now.checked_add(DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            // Nothing has been evaluated yet, and nothing has failed.
            error_string: None,
            result_ids: Vec::new(),
        }
    }

    /// Every table this run evaluates: the one its `DataSource` addresses, then the ones its
    /// `AdditionalDataSources` do.
    pub fn evaluated_tables(&self) -> impl Iterator<Item = &EvaluatedTable> {
        ::std::iter::once(&self.primary_table).chain(self.additional_tables.iter())
    }

    /// Whether the clock owed this run its next status at `now`.
    pub fn is_due(&self, now: SystemTime) -> bool {
        self.settles_at.is_some_and(|due| due <= now)
    }

    /// How long the run consumed resources, in whole seconds: `ExecutionTime`, "the amount
    /// of time (in seconds) that the run consumed resources".
    pub fn execution_time_seconds(&self) -> i32 {
        data_quality_run_execution_time_seconds(self.started_on, self.completed_on)
    }

    /// Move this run into the status the clock owed it: it was due to leave
    /// [`DATA_QUALITY_RUN_STATE_STARTING`] at [`Self::settles_at`] whether or not anybody
    /// called.
    ///
    /// A run the catalog still holds every evaluated table for finishes
    /// [`DATA_QUALITY_RUN_STATE_SUCCEEDED`], carrying one freshly minted result id per
    /// ruleset it evaluated: "the evaluation computes results which you can retrieve with the
    /// GetDataQualityResult API", and a result is what one ruleset's evaluation produced. One
    /// whose table the catalog holds no longer cannot evaluate anything over it, so it
    /// finishes [`DATA_QUALITY_RUN_STATE_FAILED`] carrying `miss`, the catalog's own reason,
    /// as its `ErrorString` and no result at all.
    ///
    /// The instants recorded are the DUE instant rather than the instant somebody read the
    /// run, so two reads of a settled run report the same run.
    pub fn settle(&mut self, miss: Option<String>) {
        let Some(due) = self.settles_at else {
            return;
        };
        self.settles_at = None;
        self.completed_on = Some(due);
        self.last_modified_on = due;
        match miss {
            Some(reason) => {
                self.status = DATA_QUALITY_RUN_STATE_FAILED.to_string();
                self.error_string = Some(reason);
            }
            None => {
                self.status = DATA_QUALITY_RUN_STATE_SUCCEEDED.to_string();
                self.result_ids = self
                    .ruleset_names
                    .iter()
                    .map(|_| new_data_quality_result_id())
                    .collect();
            }
        }
    }
}

/// How many data quality result ids this process has minted. A nonce source for
/// [`new_data_quality_result_id`], exactly as
/// [`MINTED_DATA_QUALITY_RULESET_EVALUATION_RUN_IDS`] is for an evaluation run id.
static MINTED_DATA_QUALITY_RESULT_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh data quality result id: what a finished evaluation run reports in its
/// `ResultIds`, and the id the data quality result surface addresses one result by.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside the
/// `DataQualityResultIdList` member type's `length 1..255`. It is stored on the run that
/// minted it, so every later read of that run reports the same ids.
pub fn new_data_quality_result_id() -> String {
    mint_hex_id(&MINTED_DATA_QUALITY_RESULT_IDS, DATA_QUALITY_RUN_ID_WORDS)
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

/// The keyword that opens the rule list of a DQDL ruleset.
const DQDL_RULES_KEYWORD: &str = "Rules";

/// What every character of a DQDL string literal reads as while the rule list is walked:
/// a word character, so the literal is still a rule's text but punctuates nothing.
const DQDL_LITERAL_PLACEHOLDER: char = 'x';

/// The most characters of one ruleset [`dqdl_rule_count`] reads. A ruleset longer than
/// this cannot have been stored through the modelled operations, whose `Ruleset` member is
/// `DataQualityRulesetString` (`length 1..65536`, a bound on the string's characters), so
/// the bound is that member's own.
///
/// It reuses the constant the recommended-ruleset builder states that same limit in, which
/// is spelled in bytes because that builder measures the text it BUILDS in bytes. The
/// number is the member's own in either unit, and bounding the character count bounds the
/// walk, so the reuse is deliberate rather than a unit confusion.
const MAX_DQDL_CHARS_READ: usize = MAX_DATA_QUALITY_RULESET_BYTES;

/// The deepest bracket nesting [`dqdl_rule_count`] walks. Nesting is what a rule's own
/// operands are written with, so a handful of levels is all a rule uses; past this the walk
/// stops rather than following text a caller chose to any depth.
const MAX_DQDL_NESTING: usize = 32;

/// How many rules the DQDL text `ruleset` declares — "the number of rules in the ruleset"
/// (`DataQualityRulesetListDetails$RuleCount`).
///
/// A DQDL ruleset declares its rules as one comma-separated list introduced by
/// `Rules = [ … ]`, so the count is the number of entries in THAT list: the commas that
/// separate a rule from the next one sit at the list's own nesting level, while a comma
/// inside a rule's own brackets, parentheses or quoted string belongs to that rule. Text
/// declaring no rule list at all holds no rules and counts 0; a trailing separator adds
/// no rule.
///
/// A comment runs from `#` to the end of its line and declares nothing, so neither the
/// rules it describes nor a separator inside it is counted.
///
/// DQDL is proprietary to this service and no published crate parses it, so the list is
/// walked here rather than through a dependency. Counting its entries needs none of the
/// language beyond its nesting and its string literals, so no rule GRAMMAR is assumed:
/// whatever a rule's own text is, it is one entry. The walk reads at most
/// [`MAX_DQDL_CHARS_READ`] characters in ONE pass, holds no copy of the text, nests no
/// deeper than [`MAX_DQDL_NESTING`] and never recurses, so text a caller chose bounds
/// neither the memory nor the stack this takes. Text no rule list can be read out of
/// declares no rules and counts 0: no list at all, a list left unterminated by a truncated
/// text, an unterminated string or comment, or one nested past the limit.
pub fn dqdl_rule_count(ruleset: &str) -> i32 {
    let keyword: Vec<char> = DQDL_RULES_KEYWORD.chars().collect();
    let mut phase = DqdlPhase::SeekKeyword(0);
    // Whether the character before this one could be part of a word, which is what decides
    // whether the keyword stands on its own or is the tail of a longer one.
    let mut preceded_by_word = false;
    let mut in_string = false;
    let mut in_comment = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut count = 0usize;
    let mut has_content = false;
    let mut closed = false;

    for raw in ruleset.chars().take(MAX_DQDL_CHARS_READ) {
        // A quoted string is where a rule names a column and a comment declares nothing, so
        // neither carries structure: a literal's characters read as one word character each,
        // which still counts as a rule's text while punctuating nothing, and a comment's
        // read as blanks up to the line break that ends it. A backslash escapes the
        // character after it, so an escaped quote does not end the string it sits in.
        let ch = if in_comment {
            in_comment = raw != '\n';
            if in_comment {
                ' '
            } else {
                raw
            }
        } else if in_string {
            if escaped {
                escaped = false;
            } else if raw == '\\' {
                escaped = true;
            } else if raw == '"' {
                in_string = false;
            }
            DQDL_LITERAL_PLACEHOLDER
        } else if raw == '#' {
            in_comment = true;
            ' '
        } else if raw == '"' {
            in_string = true;
            DQDL_LITERAL_PLACEHOLDER
        } else {
            raw
        };

        match phase {
            // The keyword opens a list only where it stands on its own, so a match that a
            // word character precedes is the tail of a longer word and no opener at all.
            DqdlPhase::SeekKeyword(matched) => {
                phase = if keyword.get(matched) == Some(&ch)
                    && (matched > 0 || !preceded_by_word)
                {
                    if matched + 1 == keyword.len() {
                        DqdlPhase::SeekEquals
                    } else {
                        DqdlPhase::SeekKeyword(matched + 1)
                    }
                } else {
                    // A failed match restarts the search WITHOUT re-testing the character
                    // that failed it, which loses no opener: that character is either not
                    // the keyword's first, or it is and the match it broke means a word
                    // character precedes it — and the keyword that a word character
                    // precedes is part of a longer word, which opens no list.
                    DqdlPhase::SeekKeyword(0)
                };
            }
            DqdlPhase::SeekEquals => {
                phase = match ch {
                    '=' => DqdlPhase::SeekOpen,
                    _ if ch.is_whitespace() => DqdlPhase::SeekEquals,
                    _ => DqdlPhase::SeekKeyword(0),
                };
            }
            DqdlPhase::SeekOpen => {
                phase = match ch {
                    '[' => DqdlPhase::Counting,
                    _ if ch.is_whitespace() => DqdlPhase::SeekOpen,
                    _ => DqdlPhase::SeekKeyword(0),
                };
            }
            DqdlPhase::Counting => match ch {
                // The list's own closing bracket: the entry before it is the last rule, and
                // whatever follows the list declares no rule of this list's.
                ']' if depth == 0 => {
                    if has_content {
                        count += 1;
                    }
                    closed = true;
                    break;
                }
                '[' | '(' | '{' => {
                    has_content = true;
                    depth += 1;
                    if depth > MAX_DQDL_NESTING {
                        break;
                    }
                }
                ']' | ')' | '}' => {
                    has_content = true;
                    depth = depth.saturating_sub(1);
                }
                ',' if depth == 0 => {
                    if has_content {
                        count += 1;
                    }
                    has_content = false;
                }
                _ => has_content |= !ch.is_whitespace(),
            },
        }
        preceded_by_word = is_dqdl_word_char(ch);
    }

    if !closed {
        return 0;
    }
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Where [`dqdl_rule_count`]'s single pass over a ruleset has got to: the three steps that
/// find the rule list, then the walk over its entries.
#[derive(Debug, Clone, Copy)]
enum DqdlPhase {
    /// Matching the rule-list keyword, with this many of its characters matched so far.
    SeekKeyword(usize),
    /// The keyword is spelled; awaiting the `=` that assigns the list.
    SeekEquals,
    /// The `=` is seen; awaiting the `[` that opens the list.
    SeekOpen,
    /// Inside the list, counting its entries.
    Counting,
}

/// Whether `ch` can be part of a DQDL word, and so whether the rule-list keyword touching
/// it is a word of its own.
fn is_dqdl_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// One stored data quality ruleset: the DQDL rules `CreateDataQualityRuleset` registers
/// under a name, plus the members only the server sets.
///
/// Every member of the modelled `GetDataQualityRulesetResponse` is held here, so a read
/// serves what the write stored and invents nothing, plus the create's `Tags` and
/// `ClientToken`, which that response does not carry.
#[derive(Debug, Clone)]
pub struct DataQualityRulesetEntry {
    /// "A unique name for the data quality ruleset", which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::data_quality_rulesets`].
    pub name: String,
    /// "A description of the data quality ruleset", absent when the write named none.
    pub description: Option<String>,
    /// "A Data Quality Definition Language (DQDL) ruleset", stored exactly as sent: this
    /// emulator holds the Glue control plane, so the rules are kept as the text the client
    /// authored rather than re-rendered.
    pub ruleset: String,
    /// "A target table associated with the data quality ruleset", as the write named it.
    pub target_table: Option<DataQualityTargetTable>,
    /// "The name of the security configuration created with the data quality encryption
    /// option", as the write named it.
    pub data_quality_security_configuration: Option<String>,
    /// "The date and time the data quality ruleset was created", set by the create that
    /// stored it and never moved afterwards.
    pub created_on: SystemTime,
    /// "The date and time the data quality ruleset was last modified": the instant of the
    /// most recent write to this record, which for a ruleset nothing has updated is the
    /// instant it was created.
    pub last_modified_on: SystemTime,
    /// "When a ruleset was created from a recommendation run, this run ID is generated to
    /// link the two together" (`DataQualityRulesetListDetails$RecommendationRunId`) — so a
    /// ruleset a client authored directly, which is what `CreateDataQualityRuleset` stores,
    /// carries NO run id: there is no run to link it to.
    pub recommendation_run_id: Option<String>,
    /// The `ClientToken` the create carried, which is what makes a RETRY of that create
    /// answer the stored ruleset instead of the duplicate-name refusal.
    pub client_token: Option<String>,
    /// The create's `Tags`, which no reader of a ruleset models.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, exactly as a custom entity type's are
    /// ([`CustomEntityTypeEntry::tags`]): [`TaggedResourceKind`] carries no data quality
    /// ruleset variant, because the model states no ARN resource-type segment for this kind.
    pub tags: HashMap<String, String>,
}

impl DataQualityRulesetEntry {
    /// Build a stored ruleset from a `CreateDataQualityRuleset` request served at `now`.
    ///
    /// Every member but the two timestamps comes from the request: the client authors the
    /// whole ruleset, and the model gives a created ruleset no id or status of its own.
    pub fn from_create_input(input: &CreateDataQualityRulesetInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            description: input.description.clone(),
            ruleset: input.ruleset.clone(),
            target_table: input.target_table.clone(),
            data_quality_security_configuration: input.data_quality_security_configuration.clone(),
            created_on: now,
            last_modified_on: now,
            recommendation_run_id: None,
            client_token: input.client_token.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Whether this stored ruleset is the one `input` asks to create, member for member.
    ///
    /// What makes a RETRY of a create idempotent is that it asks for the same resource, so
    /// a request that reuses a stored ruleset's `ClientToken` while asking for a different
    /// definition is not that retry: it earns the duplicate-name refusal rather than having
    /// the definition it sent silently dropped behind a 200.
    pub fn matches_create_input(&self, input: &CreateDataQualityRulesetInput) -> bool {
        // The name is compared too, so the comparison is member for member and holds for
        // any pair rather than only for a lookup already keyed by the requested name.
        self.name == input.name
            && self.description == input.description
            && self.ruleset == input.ruleset
            && self.data_quality_security_configuration == input.data_quality_security_configuration
            && same_data_quality_target_table(self.target_table.as_ref(), input.target_table.as_ref())
            && input
                .tags
                .as_ref()
                .map_or(self.tags.is_empty(), |tags| &self.tags == tags)
    }

    /// Apply an `UpdateDataQualityRuleset` request to this stored ruleset, member by
    /// member, as served at `now`.
    ///
    /// Every member the request CARRIES replaces the stored one and every member it OMITS
    /// is preserved: "updates the specified data quality ruleset"
    /// (`UpdateDataQualityRuleset`) names no required member beyond the `Name` that
    /// addresses the ruleset, so the request is a patch over the definition the create
    /// registered rather than a replacement for it.
    ///
    /// The members `UpdateDataQualityRulesetRequest` has NO member for are not this
    /// request's to change: the target table, the security configuration, the creation
    /// instant, the recommendation run id, the create's client token and its tags all stay
    /// as they are. `LastModifiedOn` becomes `now`, since it is "the last point in time
    /// when this data quality ruleset was modified"
    /// (`GetDataQualityRulesetResponse$LastModifiedOn`).
    pub fn apply_update(&mut self, input: &UpdateDataQualityRulesetInput, now: SystemTime) {
        if let Some(description) = input.description.as_ref() {
            self.description = Some(description.clone());
        }
        if let Some(ruleset) = input.ruleset.as_ref() {
            self.ruleset = ruleset.clone();
        }
        self.last_modified_on = now;
    }

    /// This stored ruleset as one entry of a listing — "describes a data quality ruleset
    /// returned by `GetDataQualityRuleset`" (`DataQualityRulesetListDetails`).
    ///
    /// Every member but `RuleCount` is the one the single-ruleset read serves, so the two
    /// can never disagree about a ruleset; `RuleCount`, which that read does not carry, is
    /// counted from the DQDL text this record holds ([`dqdl_rule_count`]). The DQDL text
    /// itself is not a member of this shape, so a listing reports how many rules a ruleset
    /// holds without serving them.
    pub fn to_list_details(&self) -> DataQualityRulesetListDetails {
        DataQualityRulesetListDetails {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            target_table: self.target_table.clone(),
            recommendation_run_id: self.recommendation_run_id.clone(),
            rule_count: Some(dqdl_rule_count(&self.ruleset)),
        }
    }

    /// Whether this ruleset satisfies every criterion `filter` names.
    ///
    /// A `DataQualityRulesetFilterCriteria` is "the criteria used to filter data quality
    /// rulesets": a conjunction in which each member it carries narrows the listing and a
    /// member it leaves out narrows nothing. `Name` and the description are matched whole —
    /// the model documents no matching mode for either, so they are compared the way this
    /// service's other filtered listing compares the name it is given — the target table is
    /// matched on the table it addresses, and the two instant pairs bound the window the
    /// criteria describe ([`instant_window_admits`], the one reading this service's filtered
    /// listings share).
    pub fn matches(&self, filter: &DataQualityRulesetFilterCriteria) -> bool {
        if filter
            .name
            .as_deref()
            .is_some_and(|name| name != self.name.as_str())
        {
            return false;
        }
        if filter.description.as_deref().is_some_and(|description| {
            self.description.as_deref() != Some(description)
        }) {
            return false;
        }
        // "The name and database name of the target table": a ruleset qualifies when the
        // table it targets is the one named, so a ruleset targeting no table at all is
        // never admitted by criteria that name one. The catalog is compared only where the
        // criteria name one, since the member documents the table and its database as what
        // it addresses.
        if let Some(wanted) = filter.target_table.as_ref() {
            let Some(targeted) = self.target_table.as_ref() else {
                return false;
            };
            if targeted.table_name != wanted.table_name
                || targeted.database_name != wanted.database_name
                || wanted
                    .catalog_id
                    .as_ref()
                    .is_some_and(|catalog| Some(catalog) != targeted.catalog_id.as_ref())
            {
                return false;
            }
        }
        instant_window_admits(
            self.created_on,
            filter.created_before.as_ref(),
            filter.created_after.as_ref(),
        ) && instant_window_admits(
            self.last_modified_on,
            filter.last_modified_before.as_ref(),
            filter.last_modified_after.as_ref(),
        )
    }
}

/// Whether an `UpdateDataQualityRuleset` request names a member to change at all.
///
/// `Description` and `Ruleset` are the only members of that request other than the `Name`
/// addressing the ruleset, and both are optional, so a request carrying neither asks for no
/// change: nothing is written, so the ruleset's `LastModifiedOn` stays where the last write
/// that did change something put it — the same way an `UpdateMLTransform` naming no member
/// leaves its transform's ([`ml_transform_update_names_a_change`]).
pub fn data_quality_ruleset_update_names_a_change(input: &UpdateDataQualityRulesetInput) -> bool {
    input.description.is_some() || input.ruleset.is_some()
}

/// Whether two optional `DataQualityTargetTable` values address the same Glue table. The
/// modelled structure derives no equality, so its three members are compared here.
fn same_data_quality_target_table(
    stored: Option<&DataQualityTargetTable>,
    requested: Option<&DataQualityTargetTable>,
) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.table_name == requested.table_name
                && stored.database_name == requested.database_name
                && stored.catalog_id == requested.catalog_id
        }
        _ => false,
    }
}


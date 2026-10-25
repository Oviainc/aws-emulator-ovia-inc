// SPDX-License-Identifier: CC0-1.0

//! What the analysis of a portfolio produces, and the preferences it is computed from.
//!
//! `StartAssessment` "Starts the assessment of an on-premises environment", and an
//! assessment that reaches its terminal `COMPLETE` status has ANALYSED the portfolio it
//! covered. The analysis is observable on the RESOURCES, not only on the assessment
//! record: `ServerDetail$recommendationSet` is "a set of recommendations",
//! `ServerDetail$lastAnalyzedTimestamp` is "the timestamp of when the server was
//! assessed", and `ApplicationComponentDetail` carries the same two members — so once a
//! completion is materialised ([`super::RegionState::analyze_portfolio`]) every read of a
//! server or an application component reports the recommendation the analysis stored for
//! it.
//!
//! WHICH recommendation it stores is what the account's saved preferences decide:
//! `PutPortfolioPreferences` "Saves the specified migration and modernization
//! preferences", `ApplicationPreferences$managementPreference` is the "application
//! preferences that you specify to prefer managed environment" and
//! `DatabasePreferences` is "preferences on managing your databases on AWS". So this
//! module is one function of the saved preferences and the resource's own stored members:
//!
//! * the preference names the TARGET DESTINATION — exactly one, since every variant's
//!   list is `@length(1, 1)` (`Homogeneous`'s is `(0, 1)`);
//! * the destination implies the STRATEGY: moving a workload onto Amazon EC2 keeps it as
//!   it is (`Rehost`), moving it onto a managed container or application platform or onto
//!   a managed database changes the platform underneath it (`Replatform`), and a
//!   preference that names no destination at all (`TargetDestination`'s own
//!   "None specified") selects no migration, so the resource stays where it is
//!   (`Retain`);
//! * the strategy and the resource name the TRANSFORMATION TOOL AWS documents for that
//!   move, from `TransformationToolName`'s catalog, and no tool at all where the catalog
//!   names none that applies.
//!
//! Nothing here looks at a request: the inputs are the stored [`super::PortfolioPreferences`]
//! and the stored [`super::ServerRecord`] / [`super::ApplicationComponentRecord`], which
//! is why a second assessment over an unchanged portfolio and unchanged preferences
//! recomputes the same recommendation.

use crate::ops::types::{
    ApplicationPreferences, DatabaseMigrationPreference, DatabasePreferences, ManagementPreference,
};

use super::{ApplicationComponentRecord, PortfolioPreferences, ServerRecord};

/// `Strategy`'s value for moving a workload to AWS unchanged.
pub const STRATEGY_REHOST: &str = "Rehost";

/// `Strategy`'s value for moving a workload onto a different platform — a managed
/// container service, a managed application platform, a managed database engine.
pub const STRATEGY_REPLATFORM: &str = "Replatform";

/// `Strategy`'s value for leaving a workload where it is, which is what an analysis
/// recommends when no saved preference selects a destination to move it to.
pub const STRATEGY_RETAIN: &str = "Retain";

/// The value every destination enumeration of this service declares for "no destination
/// chosen" — `TargetDestination`, `AwsManagedTargetDestination`,
/// `SelfManageTargetDestination`, `NoPreferenceTargetDestination` and
/// `TargetDatabaseEngine` all carry it.
pub const NOT_SPECIFIED: &str = "None specified";

/// The `TargetDestination` values a `ManagementPreference` can name, spelled out because
/// each one maps to its own strategy and tool below.
const DESTINATION_EC2: &str = "Amazon Elastic Cloud Compute (EC2)";
const DESTINATION_ECS: &str = "Amazon Elastic Container Service (ECS)";
const DESTINATION_EKS: &str = "Amazon Elastic Kubernetes Service (EKS)";
const DESTINATION_FARGATE: &str = "AWS Fargate";
const DESTINATION_BEANSTALK: &str = "AWS Elastic BeanStalk";

/// The `TargetDestination` values a `TargetDatabaseEngine` maps to.
const DESTINATION_RDS: &str = "Amazon Relational Database Service";
const DESTINATION_RDS_MYSQL: &str = "Amazon Relational Database Service on MySQL";
const DESTINATION_RDS_POSTGRESQL: &str = "Amazon Relational Database Service on PostgreSQL";
const DESTINATION_AURORA_MYSQL: &str = "Aurora MySQL";
const DESTINATION_AURORA_POSTGRESQL: &str = "Aurora PostgreSQL";
const DESTINATION_BABELFISH: &str = "Babelfish for Aurora PostgreSQL";
const DESTINATION_DOCUMENTDB: &str = "Amazon DocumentDB";

/// `TransformationToolName`'s entry for a lift-and-shift onto Amazon EC2 — AWS Application
/// Migration Service is the service that performs it.
const TOOL_MIGRATION_SERVICE: &str = "Application Migration Service";

/// `TransformationToolName`'s entry for containerising a workload — AWS App2Container is
/// documented for containerising an application and deploying it to Amazon ECS, Amazon EKS
/// and AWS Fargate.
const TOOL_APP2CONTAINER: &str = "App2Container";

/// `TransformationToolName`'s entry for moving a Windows IIS web application to AWS
/// Elastic Beanstalk — the Windows Web Application Migration Assistant is documented for
/// exactly that move, which is why it is named only for a workload running on Windows.
const TOOL_WINDOWS_WEB_ASSISTANT: &str = "Windows Web Application Migration Assistant";

/// `TransformationToolName`'s entry for a database move that keeps the engine: AWS
/// Database Migration Service migrates the data.
const TOOL_DATABASE_MIGRATION: &str = "Database Migration Service";

/// `TransformationToolName`'s entry for a database move that CHANGES the engine: the
/// schema has to be converted first, which is what the AWS Schema Conversion Tool does.
const TOOL_SCHEMA_CONVERSION: &str = "Schema Conversion Tool";

/// `DatabaseManagementPreference`'s value for running the database engine yourself, so the
/// database moves to AWS as it is rather than onto a managed engine.
const DATABASE_SELF_MANAGE: &str = "Self-manage";

/// `OSType`'s Windows value, the one the Beanstalk tool above applies to.
const OS_WINDOWS: &str = "WINDOWS";

/// The `AppType` values that are a Windows IIS / .NET web stack, for the same tool.
/// From `smithy_query(query="shape",shape="AppType")`.
const WINDOWS_WEB_APP_TYPES: &[&str] = &["IIS", "DotNetFramework", "Dotnet", "DotnetCore"];

/// The `AppType` value that says the service did not recognise the component's type; a
/// component carrying no app type at all is unknown in the same way.
const APP_TYPE_UNKNOWN: &str = "Unknown";

/// `ApplicationMode`'s values — "the classification for application component types" the
/// analysis recommends for.
const MODE_KNOWN: &str = "KNOWN";
const MODE_UNKNOWN: &str = "UNKNOWN";

/// `InclusionStatus`'s value for a component kept out of the assessment, which therefore
/// gets no recommendation and counts towards no server's.
pub const EXCLUDE_FROM_ASSESSMENT: &str = "excludeFromAssessment";

/// `ResourceSubType`'s values that make a component a DATABASE, so the saved DATABASE
/// preferences are the ones that select its destination rather than the application ones.
const DATABASE_SUB_TYPES: &[&str] = &["Database", "DatabaseProcess"];

/// One recommendation the analysis computed for one resource: the three members
/// `RecommendationSet` declares, as the resource records hold them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recommendation {
    /// `RecommendationSet$strategy` — one of `Strategy`'s values.
    pub strategy: Option<String>,
    /// `RecommendationSet$targetDestination` — one of `TargetDestination`'s values.
    pub target_destination: Option<String>,
    /// `TransformationTool$name` — one of `TransformationToolName`'s values, or `None`
    /// where the catalog names no tool for this move.
    pub transformation_tool: Option<String>,
}

impl Recommendation {
    /// The recommendation that leaves a resource where it is: what an analysis produces
    /// when no saved preference selects a destination to move it to.
    fn retain() -> Self {
        Self {
            strategy: Some(STRATEGY_RETAIN.to_string()),
            target_destination: Some(NOT_SPECIFIED.to_string()),
            transformation_tool: None,
        }
    }

    fn new(strategy: &str, destination: &str, tool: Option<&str>) -> Self {
        Self {
            strategy: Some(strategy.to_string()),
            target_destination: Some(destination.to_string()),
            transformation_tool: tool.map(str::to_string),
        }
    }
}

/// The single target destination an `applicationPreferences` names, whichever
/// `ManagementPreference` variant it chose, or `None` when it names none.
///
/// Every variant carries its own destination enumeration and its own `@length`-bounded
/// list of ONE entry, so the variant decides which values are admitted and not how many:
/// the destination is the one entry the chosen variant holds.
fn preferred_destination(preferences: Option<&ApplicationPreferences>) -> Option<&str> {
    let destinations = match preferences?.management_preference.as_ref()? {
        ManagementPreference::awsManagedResources(resources) => &resources.target_destination,
        ManagementPreference::selfManageResources(resources) => &resources.target_destination,
        ManagementPreference::noPreference(resources) => &resources.target_destination,
    };
    destinations.first().map(String::as_str)
}

/// The recommendation a destination amounts to for an application workload — a server, or
/// an application component that is not a database.
///
/// The destination decides the strategy: AWS's own managed container and application
/// platforms put the workload on a different platform (`Replatform`), Amazon EC2 runs it
/// as it is (`Rehost`), and the enumeration's own "no destination chosen" value selects no
/// migration (`Retain`). `windows` is whether the workload runs on Windows, which is the
/// one thing the tool for the Elastic Beanstalk destination depends on.
fn application_recommendation(destination: Option<&str>, windows: bool) -> Recommendation {
    match destination {
        None | Some(NOT_SPECIFIED) => Recommendation::retain(),
        Some(DESTINATION_EC2) => Recommendation::new(
            STRATEGY_REHOST,
            DESTINATION_EC2,
            Some(TOOL_MIGRATION_SERVICE),
        ),
        Some(destination @ (DESTINATION_ECS | DESTINATION_EKS | DESTINATION_FARGATE)) => {
            Recommendation::new(STRATEGY_REPLATFORM, destination, Some(TOOL_APP2CONTAINER))
        }
        Some(DESTINATION_BEANSTALK) => Recommendation::new(
            STRATEGY_REPLATFORM,
            DESTINATION_BEANSTALK,
            // The catalog's Beanstalk tool is the Windows IIS web application assistant,
            // so it is named for a Windows workload and nothing is named for any other:
            // a tool that does not perform this move would be a tool nothing picked.
            windows.then_some(TOOL_WINDOWS_WEB_ASSISTANT),
        ),
        // A destination outside the enumerations a `ManagementPreference` admits selects
        // no application move, so nothing is recommended beyond retaining the workload.
        // `PutPortfolioPreferences` refuses an undeclared value, so no stored preference
        // reaches here.
        Some(_) => Recommendation::retain(),
    }
}

/// The `TargetDestination` the AWS-managed database engine `engine` names, as the engine
/// the component runs today (`app_type`) disambiguates it.
///
/// The mapping is each engine onto the AWS managed service that RUNS that engine: MySQL
/// and PostgreSQL onto Amazon RDS for that engine, MongoDB onto Amazon DocumentDB (which
/// speaks the MongoDB API), and every other engine the enumeration declares onto Amazon
/// RDS, which runs it. `Amazon Aurora` is the one engine value that names a family rather
/// than an engine, so the component's own `AppType` picks the compatible edition —
/// Babelfish being the Aurora PostgreSQL edition AWS documents for a Microsoft SQL Server
/// source.
fn database_destination(engine: &str, app_type: Option<&str>) -> &'static str {
    match engine {
        "MySQL" => DESTINATION_RDS_MYSQL,
        "AWS PostgreSQL" => DESTINATION_RDS_POSTGRESQL,
        "MongoDB" => DESTINATION_DOCUMENTDB,
        "Amazon Aurora" => match app_type {
            Some("MySQL" | "Maria DB") => DESTINATION_AURORA_MYSQL,
            Some("SQLServer") => DESTINATION_BABELFISH,
            _ => DESTINATION_AURORA_POSTGRESQL,
        },
        // "Microsoft SQL Server", "Oracle Database", "MariaDB", "SAP", "Db2 LUW" — and
        // "None specified", which chooses no engine, so the managed relational service
        // itself is the destination.
        _ => DESTINATION_RDS,
    }
}

/// The `TargetDestination` a move that KEEPS the engine the component runs today lands
/// on — the AWS managed service that runs that engine.
///
/// `Homogeneous` is documented "moving to the same type of database into AWS. For example,
/// from SQL Server in your environment to SQL Server on AWS", so the engine to look at is
/// the component's own (`AppType`) rather than one the preference names: the model gives
/// `HomogeneousTargetDatabaseEngine` the single value "None specified" precisely because
/// there is no other engine to name. `TargetDestination` names Amazon RDS for MySQL and
/// for PostgreSQL specifically; every other engine it runs is named by the managed
/// relational service itself.
fn homogeneous_destination(app_type: Option<&str>) -> &'static str {
    match app_type {
        Some("MySQL") => DESTINATION_RDS_MYSQL,
        Some("PostgreSQLServer") => DESTINATION_RDS_POSTGRESQL,
        _ => DESTINATION_RDS,
    }
}

/// The recommendation the saved DATABASE preferences amount to for one database
/// component.
///
/// `databaseManagementPreference` decides whether the engine stays the account's to run:
/// self-managing it means the database moves to AWS as it is, onto Amazon EC2
/// (`Rehost`). Otherwise it moves onto a managed engine (`Replatform`), and the migration
/// preference decides which one and with what tool: a HETEROGENEOUS move changes the
/// engine, so the destination is the one the engine it names runs on and the schema is
/// converted first, while a homogeneous move keeps the engine the database runs today
/// ([`homogeneous_destination`]) and only migrates the data. A `databasePreferences` that
/// names neither half selects nothing.
fn database_recommendation(
    preferences: Option<&DatabasePreferences>,
    app_type: Option<&str>,
) -> Recommendation {
    let Some(preferences) = preferences else {
        return Recommendation::retain();
    };
    if preferences.database_management_preference.as_deref() == Some(DATABASE_SELF_MANAGE) {
        return Recommendation::new(
            STRATEGY_REHOST,
            DESTINATION_EC2,
            Some(TOOL_MIGRATION_SERVICE),
        );
    }
    let (destination, tool) = match preferences.database_migration_preference.as_ref() {
        // Neither half of the preference selects anything to move the database to.
        None if preferences.database_management_preference.is_none() => {
            return Recommendation::retain()
        }
        // A management preference alone names no other engine, so it is the same engine,
        // managed by AWS.
        None => (homogeneous_destination(app_type), TOOL_DATABASE_MIGRATION),
        Some(DatabaseMigrationPreference::Heterogeneous(target)) => (
            database_destination(
                target
                    .target_database_engine
                    .first()
                    .map(String::as_str)
                    .unwrap_or(NOT_SPECIFIED),
                app_type,
            ),
            TOOL_SCHEMA_CONVERSION,
        ),
        Some(DatabaseMigrationPreference::Homogeneous(_)) => {
            (homogeneous_destination(app_type), TOOL_DATABASE_MIGRATION)
        }
        Some(DatabaseMigrationPreference::noPreference(target)) => (
            database_destination(
                target
                    .target_database_engine
                    .first()
                    .map(String::as_str)
                    .unwrap_or(NOT_SPECIFIED),
                app_type,
            ),
            TOOL_DATABASE_MIGRATION,
        ),
    };
    Recommendation::new(STRATEGY_REPLATFORM, destination, Some(tool))
}

/// Whether an application component runs on Windows, which is the one thing the tool for
/// the AWS Elastic Beanstalk destination depends on.
///
/// The model gives a component a free-text `osVersion` and no `OSType`, so the two places
/// its Windows-ness can come from are that version text and an `AppType` that is a Windows
/// IIS / .NET stack. One rule, in one place, for every component.
fn runs_windows(os_version: Option<&str>, app_type: Option<&str>) -> bool {
    os_version.is_some_and(|version| version.to_ascii_lowercase().contains("windows"))
        || app_type.is_some_and(|app_type| WINDOWS_WEB_APP_TYPES.contains(&app_type))
}

/// The recommendation the analysis computes for one server of the portfolio.
///
/// A server is an application workload, so the application half of the saved preferences
/// is what selects its destination; its stored `OSType` is what decides the tool for the
/// one destination whose tool depends on it.
pub fn server_recommendation(
    preferences: Option<&PortfolioPreferences>,
    server: &ServerRecord,
) -> Recommendation {
    let application = preferences.and_then(|saved| saved.application_preferences.as_ref());
    application_recommendation(
        preferred_destination(application),
        server.os_type.as_deref() == Some(OS_WINDOWS),
    )
}

/// The recommendation the analysis computes for one application component, or `None` when
/// the component is not one the analysis covers.
///
/// Two stored members keep a component out of it: an `inclusionStatus` of
/// `excludeFromAssessment` — the member "indicates whether the application component has
/// been included for server recommendation or not" — and an `applicationMode`, "the
/// classification for application component types", that selects the other
/// classification: `KNOWN` covers the components whose `AppType` the service recognises
/// and `UNKNOWN` the ones it does not, while an absent mode and `ALL` cover both.
///
/// A component the analysis does cover is recommended from the DATABASE half of the saved
/// preferences when its `ResourceSubType` makes it a database, and from the application
/// half otherwise.
pub fn component_recommendation(
    preferences: Option<&PortfolioPreferences>,
    component: &ApplicationComponentRecord,
) -> Option<Recommendation> {
    if component.inclusion_status.as_deref() == Some(EXCLUDE_FROM_ASSESSMENT) {
        return None;
    }
    let app_type = component.app_type.as_deref();
    let known = app_type.is_some_and(|app_type| app_type != APP_TYPE_UNKNOWN);
    match preferences.and_then(|saved| saved.application_mode.as_deref()) {
        Some(MODE_KNOWN) if !known => return None,
        Some(MODE_UNKNOWN) if known => return None,
        _ => {}
    }
    let is_database = component
        .resource_sub_type
        .as_deref()
        .is_some_and(|sub_type| DATABASE_SUB_TYPES.contains(&sub_type));
    Some(if is_database {
        database_recommendation(
            preferences.and_then(|saved| saved.database_preferences.as_ref()),
            app_type,
        )
    } else {
        application_recommendation(
            preferred_destination(
                preferences.and_then(|saved| saved.application_preferences.as_ref()),
            ),
            runs_windows(component.os_version.as_deref(), app_type),
        )
    })
}

// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmsap-ssmsap service.
//!
//! The registered SAP application is this service's primary resource: every
//! application-scoped operation (`RegisterApplication`, `GetApplication`,
//! `ListApplications`, `StartApplication`, `StopApplication`,
//! `DeregisterApplication`) resolves the SAME [`StoredApplication`] out of
//! [`RegionState::applications`], and every call that changes one files a
//! [`StoredOperation`] in [`RegionState::operations`] that `GetOperation` reads back.
//! So a register is observable by every later read, a deregister by every later call,
//! and an `OperationId` handed to a caller always names a record that exists.
//!
//! Those records are held PER REGION: the service is regional, so the store is a
//! [`RegionState`] per Region ([`ComAmazonawsSsmsapSsmsapState::region`]) and a handler
//! only ever reaches the partition of the Region its request was signed for
//! ([`request_region`]).
//!
//! A configuration check is the service's OTHER asynchronous unit of work:
//! `StartConfigurationChecks` files one [`StoredConfigurationCheckOperation`] per
//! requested check in [`RegionState::configuration_check_operations`], which
//! `GetConfigurationCheckOperation` reads back by its id and
//! `ListConfigurationCheckOperations` reports per application
//! ([`RegionState::configuration_check_operations_for`]) — in that Region alone, like
//! every other record.
//!
//! The record is the stored half; the wire half (`Application`, `ApplicationSummary`,
//! `Operation`, `ConfigurationCheckOperation`) is PROJECTED from it by
//! [`StoredApplication::view`] / [`StoredApplication::summary`] /
//! [`StoredOperation::view`] / [`StoredConfigurationCheckOperation::view`], so no
//! handler assembles a response member by hand.
//!
//! An application's SAP HANA databases are DERIVED from that same record rather than
//! held beside it: the emulator discovers nothing on the registered instances, so the
//! databases an application has are the ones its registration named — by `DatabaseArn`
//! or by a credential's `DatabaseName`
//! ([`StoredApplication::databases`], projected onto the wire `DatabaseSummary` by
//! [`ApplicationDatabase::summary`]) — which is what `ListDatabases` reports, so a
//! database cannot outlive the registration it came from.
//!
//! Resource permissions work the same way: `PutResourcePermission` grants and
//! `GetResourcePermission` reads the SAME triples in [`RegionState::resource_permissions`],
//! and the `Policy` document both report is PROJECTED from them by
//! [`RegionState::resource_policy`] — so a grant is observable by every later read in
//! that Region and neither handler writes a policy document of its own.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Application, ApplicationCredential, ApplicationSummary, ConfigurationCheckOperation,
    ConflictException, DatabaseSummary, Filter, InternalServerException, Operation,
    ResourceNotFoundException, ValidationException, CONFIGURATION_CHECK_TYPE_VALUES,
};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own generated config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, as the generated
/// `OPERATIONS.md` reports it — the `service` segment of every application ARN.
pub const ARN_SERVICE: &str = "ssm-sap";

/// `ApplicationStatus` while registration is still settling. One of the model's
/// `ApplicationStatus` values (see [`crate::ops::types::APPLICATION_STATUS_VALUES`]).
pub const APPLICATION_STATUS_REGISTERING: &str = "REGISTERING";

/// `ApplicationStatus` for an application that is registered and running.
pub const APPLICATION_STATUS_ACTIVATED: &str = "ACTIVATED";

/// `ApplicationStatus` between `StartApplication` and the application running.
pub const APPLICATION_STATUS_STARTING: &str = "STARTING";

/// `ApplicationStatus` between `StopApplication` and the application being down.
pub const APPLICATION_STATUS_STOPPING: &str = "STOPPING";

/// `ApplicationStatus` for an application that has been stopped.
pub const APPLICATION_STATUS_STOPPED: &str = "STOPPED";

/// `ApplicationDiscoveryStatus` while the registration's discovery is still running.
/// One of the model's `ApplicationDiscoveryStatus` values (see
/// [`crate::ops::types::APPLICATION_DISCOVERY_STATUS_VALUES`]).
pub const DISCOVERY_STATUS_REGISTERING: &str = "REGISTERING";

/// `ApplicationDiscoveryStatus` once discovery has completed.
pub const DISCOVERY_STATUS_SUCCESS: &str = "SUCCESS";

/// `OperationStatus` for an operation that has not reached its terminal status yet.
/// One of the model's `OperationStatus` values (see
/// [`crate::ops::types::OPERATION_STATUS_VALUES`]).
pub const OPERATION_STATUS_INPROGRESS: &str = "INPROGRESS";

/// `OperationStatus` for an operation that completed.
pub const OPERATION_STATUS_SUCCESS: &str = "SUCCESS";

/// `OperationStatus` for an operation that ended without doing its work. One of the
/// model's `OperationStatus` values (see [`crate::ops::types::OPERATION_STATUS_VALUES`]).
pub const OPERATION_STATUS_ERROR: &str = "ERROR";

/// `Operation$Type` for the operation `RegisterApplication` files.
///
/// `OperationType` is an unconstrained string in the model (no enum), so the value is
/// the name of the API call that produced the record — which is what makes a record
/// readable next to the call a client made.
pub const OPERATION_TYPE_REGISTER_APPLICATION: &str = "REGISTER_APPLICATION";

/// `Operation$Type` for the operation `StartApplication` files.
pub const OPERATION_TYPE_START_APPLICATION: &str = "START_APPLICATION";

/// `Operation$Type` for the operation `StopApplication` files.
pub const OPERATION_TYPE_STOP_APPLICATION: &str = "STOP_APPLICATION";

/// `Operation$ResourceType`: the kind of resource an operation acted on.
///
/// `ResourceType` is an unconstrained string of length 1..64 in the model, so this
/// names the resource kind every operation in this slice targets — the registered
/// application.
pub const APPLICATION_RESOURCE_TYPE: &str = "APPLICATION";

/// `ComponentType` of the SAP HANA database component of an application. One of the
/// model's `ComponentType` values (see [`crate::ops::types::COMPONENT_TYPE_VALUES`]).
pub const COMPONENT_TYPE_HANA: &str = "HANA";

/// The name SAP HANA gives the system database of a multitenant database system; every
/// other database of that system is a tenant database. It is what tells a `DatabaseType`
/// of `SYSTEM` from one of `TENANT` — see [`database_type_for`].
pub const HANA_SYSTEM_DATABASE_NAME: &str = "SYSTEMDB";

/// `DatabaseType` of the system database of a HANA system. One of the model's
/// `DatabaseType` values (see [`crate::ops::types::DATABASE_TYPE_VALUES`]).
pub const DATABASE_TYPE_SYSTEM: &str = "SYSTEM";

/// `DatabaseType` of a tenant database of a HANA system.
pub const DATABASE_TYPE_TENANT: &str = "TENANT";

/// The longest `DatabaseId` the model admits
/// (`smithy_query(query="shape", shape="DatabaseSummary")`: `DatabaseId` length 1..300),
/// which is what [`database_id_from_name`] cuts a derived id to.
pub const MAX_DATABASE_ID_LENGTH: usize = 300;

/// How long after the call that starts it a status change the clock owes falls due.
///
/// Registering, starting and stopping an SAP application are asynchronous in the real
/// service: the call returns an `OperationId` and the application reaches its next
/// status later. Nothing AWS documents fixes how long that takes, so this is the
/// emulator's own span — long enough that a caller acting straight after the call still
/// observes the transient `REGISTERING`/`STARTING`/`STOPPING` status and the
/// `INPROGRESS` operation, short enough that a test need not wait long. The due instant
/// is STORED on the record and materialised by the first request at or after it
/// ([`StoredApplication::settle`] / [`StoredOperation::settle`]); nothing ever sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// How many applications this emulator will hold PER REGION.
///
/// The real service caps registered applications per account per Region with a service
/// quota, so the ceiling is per Region here too. This number is NOT a fidelity claim
/// about the published quota: it is the emulator's own ceiling, generous enough that no
/// realistic workflow meets it, so a caller looping on `RegisterApplication` cannot grow
/// this process's memory without bound. It is reported with `ConflictException`, which
/// `RegisterApplication` models.
pub const MAX_APPLICATIONS: usize = 1_000;

/// How many Region partitions this emulator will hold.
///
/// The store is partitioned per Region (see [`RegionState`]) and the Region comes from
/// the caller's own SigV4 credential scope, so without a ceiling a caller could mint a
/// partition per made-up Region. Comfortably more than the Regions AWS operates, so no
/// real multi-Region workflow meets it; with [`MAX_APPLICATIONS`] and
/// [`MAX_RETAINED_OPERATIONS`] it is what bounds the whole store. Like the application
/// ceiling it is reported with `ConflictException`, which `RegisterApplication` models,
/// and a partition left empty is dropped
/// ([`ComAmazonawsSsmsapSsmsapState::forget_region_if_empty`]) so only Regions that
/// hold something count against it.
pub const MAX_REGIONS: usize = 64;

/// How many operation records are retained per Region, per kind of record (application
/// operations, configuration check operations). Beyond this the oldest record that has
/// already reached a terminal status is dropped first, so a caller polling a fresh
/// operation can still resolve it.
pub const MAX_RETAINED_OPERATIONS: usize = 10_000;

/// The page size `ListApplications` uses when the caller names no `MaxResults`, and
/// the ceiling it clamps a requested page to: the model's own `MaxResults` range is
/// 1..50, so 50 is both the largest page the API allows and the default page.
pub const MAX_PAGE_SIZE: i32 = 50;

/// The `Version` of a rendered resource policy document: the current IAM policy
/// language version, which is what every AWS resource policy document carries.
pub const POLICY_VERSION: &str = "2012-10-17";

/// The `Effect` of a statement a resource permission grant renders to. A grant adds
/// permission, so the statement it renders to allows.
pub const POLICY_EFFECT_ALLOW: &str = "Allow";

/// How many distinct target resources one Region will hold permissions for, and how
/// many source resources may be granted one action type on one target.
///
/// The real service caps a resource policy by document size rather than by grant
/// count, so neither number is a fidelity claim about a published quota: together they
/// are the emulator's own ceiling, generous enough that no realistic workflow meets
/// them, so a caller looping on `PutResourcePermission` cannot grow this process's
/// memory without bound. Both are checked in constant time and reported with
/// `ValidationException`, which `PutResourcePermission` models.
pub const MAX_PERMISSIONED_RESOURCES: usize = 1_000;

/// How many source resources may hold one action type on one target. See
/// [`MAX_PERMISSIONED_RESOURCES`].
pub const MAX_PERMISSION_SOURCES_PER_RESOURCE: usize = 100;

/// The grants held for ONE resource: `PermissionActionType` -> the source resource
/// ARNs permitted to take that action on it.
///
/// `BTreeMap`/`BTreeSet` rather than the hashed containers because the rendered policy
/// document's statement order has to be the same for every read of an unchanged
/// resource, and because a repeated grant of the same triple must be the same single
/// statement (the set is what makes `PutResourcePermission` idempotent).
pub type ResourceGrants = BTreeMap<String, BTreeSet<String>>;

/// The `Action` a statement names for `action_type`: this service's ARN namespace
/// (its IAM action prefix, [`ARN_SERVICE`]) and the `PermissionActionType` the grant
/// was made with, which is how an IAM action is spelled.
pub fn permission_policy_action(action_type: &str) -> String {
    format!("{ARN_SERVICE}:{action_type}")
}

/// A status change the clock owes an application: at `due_at` the application takes
/// `status`/`discovery_status`, whether or not anybody called.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// When the change falls due. See [`TRANSITION_SPAN`].
    pub due_at: SystemTime,
    /// The `ApplicationStatus` the application takes at `due_at`.
    pub status: String,
    /// The `ApplicationDiscoveryStatus` it takes at `due_at`.
    pub discovery_status: String,
}

/// A registered SAP application: the state `Application` and `ApplicationSummary` are
/// rebuilt from.
#[derive(Debug, Clone)]
pub struct StoredApplication {
    pub id: String,
    /// An `ApplicationType` value (`HANA` | `SAP_ABAP`), exactly as registered.
    pub app_type: String,
    pub arn: String,
    /// The AWS Service Catalog AppRegistry ARN this application is registered under.
    /// `RegisterApplication` takes no AppRegistry application, so an application
    /// registered through this slice has none — but `GetApplication` resolves by this
    /// member, so it is stored rather than assumed.
    pub app_registry_arn: Option<String>,
    /// An `ApplicationStatus` value; see the `APPLICATION_STATUS_*` constants.
    pub status: String,
    /// An `ApplicationDiscoveryStatus` value; see the `DISCOVERY_STATUS_*` constants.
    pub discovery_status: String,
    pub status_message: Option<String>,
    /// The component ids of this application, derived from the `ComponentsInfo` the
    /// registration carried. The emulator discovers nothing on its own, so an
    /// application registered without `ComponentsInfo` has no components.
    pub components: Vec<String>,
    pub associated_application_arns: Vec<String>,
    pub last_updated: SystemTime,
    pub sid: Option<String>,
    pub sap_instance_number: Option<String>,
    /// The EC2 instances the registration named.
    pub instances: Vec<String>,
    pub credentials: Vec<ApplicationCredential>,
    /// The ARN of the SAP HANA database the registration named, when it named one: one
    /// of the databases of this application ([`StoredApplication::databases`]).
    pub database_arn: Option<String>,
    /// The change the clock owes this application, if one is outstanding.
    pub pending: Option<PendingTransition>,
}

impl StoredApplication {
    /// Materialise the status change the clock already owes this application.
    ///
    /// The change was due at [`PendingTransition::due_at`] whether or not anybody
    /// called, so applying it on a read reports what the resource already is rather
    /// than changing it — and `LastUpdated` is stamped with the due instant, not with
    /// the time of the call that happened to observe it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        let pending = self.pending.take().expect("checked present just above");
        self.status = pending.status;
        self.discovery_status = pending.discovery_status;
        self.last_updated = pending.due_at.max(self.last_updated);
    }

    /// Project the stored application onto the modelled `Application`.
    pub fn view(&self) -> Application {
        Application {
            id: Some(self.id.clone()),
            r#type: Some(self.app_type.clone()),
            arn: Some(self.arn.clone()),
            app_registry_arn: self.app_registry_arn.clone(),
            status: Some(self.status.clone()),
            discovery_status: Some(self.discovery_status.clone()),
            components: Some(self.components.clone()),
            last_updated: Some(::aws_smithy_types::DateTime::from(self.last_updated)),
            status_message: self.status_message.clone(),
            associated_application_arns: if self.associated_application_arns.is_empty() {
                None
            } else {
                Some(self.associated_application_arns.clone())
            },
        }
    }

    /// Project the stored application onto the modelled `ApplicationSummary`, with the
    /// tags held for its ARN.
    pub fn summary(&self, tags: Option<HashMap<String, String>>) -> ApplicationSummary {
        ApplicationSummary {
            id: Some(self.id.clone()),
            discovery_status: Some(self.discovery_status.clone()),
            r#type: Some(self.app_type.clone()),
            arn: Some(self.arn.clone()),
            tags,
        }
    }

    /// The value a `ListApplications` filter whose `Name` is `name` compares against,
    /// or `None` when this application has no such attribute.
    ///
    /// The names are the members the model gives `ApplicationSummary` — `Id`,
    /// `DiscoveryStatus`, `Type`, `Arn` — which is the shape `ListApplications`
    /// returns, so a filter names something the response itself reports.
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Id" => Some(self.id.as_str()),
            "DiscoveryStatus" => Some(self.discovery_status.as_str()),
            "Type" => Some(self.app_type.as_str()),
            "Arn" => Some(self.arn.as_str()),
            _ => None,
        }
    }

    /// The SAP HANA databases of this application, in `DatabaseId` order.
    ///
    /// The emulator discovers nothing on the registered instances, so the databases an
    /// application has are the ones its registration named — and a registration names a
    /// database in EITHER of two places, so the set is the union of both:
    ///
    /// * `DatabaseArn` names the application's SAP HANA database by ARN, whose last
    ///   segment is the database's own name ([`database_name_from_arn`]); the ARN the
    ///   registration gave it is that database's ARN, so it is the ARN reported for it
    ///   rather than one minted here.
    /// * every `ApplicationCredential` names, in `DatabaseName`, the SAP HANA database
    ///   its secret reaches; a database no ARN named is a sub-resource of this
    ///   application, so its ARN is minted from the two ids ([`database_arn`]).
    ///
    /// A database is identified by its name ([`database_id_from_name`]), so two of those
    /// mentions naming the same database are ONE database — and an application whose
    /// registration named none, by ARN or by credential, has none.
    ///
    /// `region` is the Region the application is registered in, which is the Region the
    /// ARNs minted here carry.
    pub fn databases(&self, region: &str) -> Vec<ApplicationDatabase> {
        // The component a HANA database belongs to is the application's HANA component,
        // identified the way this service identifies every component: by the System ID
        // and the component type. An application whose registration named no System ID
        // has nothing to identify that component by.
        let component_id = self
            .sid
            .as_deref()
            .map(|sid| component_id(sid, COMPONENT_TYPE_HANA));
        // Keyed by `DatabaseId`, which both deduplicates and puts the databases in id
        // order, so a page taken over them is stable between calls.
        let mut databases: BTreeMap<String, ApplicationDatabase> = BTreeMap::new();
        // The database the registration named by ARN goes in FIRST, so that when a
        // credential names that same database the ARN the registration gave it is the
        // one reported for it.
        let named_by_arn = self
            .database_arn
            .as_deref()
            .and_then(|arn| database_name_from_arn(arn).map(|name| (arn, name)));
        if let Some((arn, database_name)) = named_by_arn {
            let database_id = database_id_from_name(database_name);
            databases.insert(
                database_id.clone(),
                ApplicationDatabase {
                    application_id: self.id.clone(),
                    component_id: component_id.clone(),
                    arn: arn.to_string(),
                    database_id,
                    database_name: database_name.to_string(),
                    database_type: database_type_for(database_name).to_string(),
                },
            );
        }
        for credential in &self.credentials {
            let database_id = database_id_from_name(&credential.database_name);
            databases
                .entry(database_id.clone())
                .or_insert_with(|| ApplicationDatabase {
                    application_id: self.id.clone(),
                    component_id: component_id.clone(),
                    arn: database_arn(region, &self.id, &database_id),
                    database_id,
                    database_name: credential.database_name.clone(),
                    database_type: database_type_for(&credential.database_name).to_string(),
                });
        }
        databases.into_values().collect()
    }
}

/// A SAP HANA database of a registered application: the record `DatabaseSummary` is
/// projected from.
///
/// DERIVED from the registration by [`StoredApplication::databases`] rather than stored
/// beside it, so the registration stays the single source of truth for its databases and
/// no call can report one the application does not have.
#[derive(Debug, Clone)]
pub struct ApplicationDatabase {
    /// The `ApplicationId` of the application this database belongs to.
    pub application_id: String,
    /// The `ComponentId` of the application's HANA component, absent when the
    /// registration named no System ID to identify that component by.
    pub component_id: Option<String>,
    /// The `DatabaseId`, derived from the name by [`database_id_from_name`].
    pub database_id: String,
    /// The `DatabaseName` the registration named this database by: a credential's
    /// `DatabaseName`, or the name the registration's `DatabaseArn` ends with.
    pub database_name: String,
    /// A `DatabaseType` value; see [`database_type_for`].
    pub database_type: String,
    /// The ARN of the database: the one the registration's `DatabaseArn` gave it when it
    /// named this database by ARN, and otherwise the sub-resource ARN minted from the
    /// application and database ids ([`database_arn`]).
    pub arn: String,
}

impl ApplicationDatabase {
    /// Project the database onto the modelled `DatabaseSummary`, with the tags held for
    /// its ARN.
    pub fn summary(&self, tags: Option<HashMap<String, String>>) -> DatabaseSummary {
        DatabaseSummary {
            application_id: Some(self.application_id.clone()),
            component_id: self.component_id.clone(),
            database_id: Some(self.database_id.clone()),
            database_type: Some(self.database_type.clone()),
            arn: Some(self.arn.clone()),
            tags,
        }
    }

    /// The key a database listing is ordered and paged by: this database's application
    /// id and its own id, in that order.
    ///
    /// The two are joined by a byte lower than any the modelled `ApplicationId`
    /// (`^[\w\d\.-]+$`) and `DatabaseId` (`[\w\d]+$`) patterns admit, so ordering these
    /// keys as strings is the same as ordering by (application id, database id) — which
    /// is what makes a `NextToken` resume at the database the previous page stopped at.
    pub fn page_key(&self) -> String {
        format!(
            "{}{DATABASE_PAGE_KEY_SEPARATOR}{}",
            self.application_id, self.database_id
        )
    }
}

/// Joins the two parts of a database listing key; see [`ApplicationDatabase::page_key`].
const DATABASE_PAGE_KEY_SEPARATOR: char = '\u{1}';

/// A persisted asynchronous operation: the record `GetOperation` reads.
#[derive(Debug, Clone)]
pub struct StoredOperation {
    pub id: String,
    /// An `OperationType` value; see the `OPERATION_TYPE_*` constants.
    pub op_type: String,
    /// An `OperationStatus` value; see the `OPERATION_STATUS_*` constants.
    pub status: String,
    pub status_message: Option<String>,
    /// The parameters of the call that filed this operation, if it carried any.
    pub properties: HashMap<String, String>,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_arn: String,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub last_updated_time: SystemTime,
    /// When this operation's terminal status falls due. See [`TRANSITION_SPAN`].
    pub success_due_at: SystemTime,
}

impl StoredOperation {
    /// How this record ranks for eviction when the store is full: see [`evict_oldest`].
    /// The record with the smallest rank goes first, so a finished record is dropped
    /// before any that is still running, oldest first.
    fn eviction_rank(&self) -> (bool, SystemTime) {
        (self.status == OPERATION_STATUS_INPROGRESS, self.start_time)
    }
    /// Materialise the terminal status the clock already owes this operation: it was
    /// due to succeed at `success_due_at` whether or not anybody asked.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != OPERATION_STATUS_INPROGRESS || now < self.success_due_at {
            return;
        }
        self.status = OPERATION_STATUS_SUCCESS.to_string();
        self.end_time = Some(self.success_due_at);
        self.last_updated_time = self.success_due_at.max(self.last_updated_time);
    }

    /// Project the stored operation onto the modelled `Operation`.
    pub fn view(&self) -> Operation {
        Operation {
            id: Some(self.id.clone()),
            r#type: Some(self.op_type.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            properties: if self.properties.is_empty() {
                None
            } else {
                Some(self.properties.clone())
            },
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            last_updated_time: Some(::aws_smithy_types::DateTime::from(self.last_updated_time)),
        }
    }
}

/// What a configuration check operation reports once the clock has brought it to its
/// terminal status.
///
/// A configuration check evaluates AWS's own catalogue of SAP rules against a live SAP
/// system. This emulator holds the service's CONTROL plane — the registered
/// application, the operation record, its id, its status and its timestamps — but it
/// runs no SAP system and the model carries no rule catalogue
/// (`smithy_query(query="shape", shape="ConfigurationCheckType")` is three opaque
/// identifiers with no prose), so there is nothing here that can evaluate a rule.
/// Rather than synthesise results that would look computed, the run ends in the
/// model's own failure status (`ERROR`, see
/// [`crate::ops::types::OPERATION_STATUS_VALUES`]) with a `StatusMessage` that says
/// why, and reports NO `RuleStatusCounts`: no rule ran, so there is nothing to count.
pub const CONFIGURATION_CHECK_NOT_EVALUATED_MESSAGE: &str =
    "This emulator holds no SAP system to evaluate configuration check rules against, \
     so no rule results were produced for this check.";

/// A configuration check operation: one requested check, running against one
/// registered application. The record `GetConfigurationCheckOperation` reads.
#[derive(Debug, Clone)]
pub struct StoredConfigurationCheckOperation {
    pub id: String,
    /// The `ApplicationId` of the application the check was started against.
    pub application_id: String,
    /// A `ConfigurationCheckType` value (see
    /// [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]).
    pub check_id: String,
    /// An `OperationStatus` value; see the `OPERATION_STATUS_*` constants.
    pub status: String,
    pub status_message: Option<String>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    /// When this check's terminal status falls due. See [`TRANSITION_SPAN`].
    pub terminal_due_at: SystemTime,
}

impl StoredConfigurationCheckOperation {
    /// How this record ranks for eviction when the store is full; see
    /// [`StoredOperation::eviction_rank`], which the two records share the policy of.
    fn eviction_rank(&self) -> (bool, SystemTime) {
        (self.status == OPERATION_STATUS_INPROGRESS, self.start_time)
    }
    /// Materialise the terminal status the clock already owes this check: it was due to
    /// end at `terminal_due_at` whether or not anybody asked.
    ///
    /// See [`CONFIGURATION_CHECK_NOT_EVALUATED_MESSAGE`] for why the terminal status is
    /// the model's failure status rather than `SUCCESS`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != OPERATION_STATUS_INPROGRESS || now < self.terminal_due_at {
            return;
        }
        self.status = OPERATION_STATUS_ERROR.to_string();
        self.status_message = Some(CONFIGURATION_CHECK_NOT_EVALUATED_MESSAGE.to_string());
        self.end_time = Some(self.terminal_due_at);
    }

    /// Project the stored check onto the modelled `ConfigurationCheckOperation`.
    ///
    /// `ConfigurationCheckName` / `ConfigurationCheckDescription` are the catalogue's
    /// prose for the check. Neither the model nor this emulator holds that text, so
    /// they are reported ABSENT rather than invented; `RuleStatusCounts` is absent for
    /// the reason [`CONFIGURATION_CHECK_NOT_EVALUATED_MESSAGE`] states.
    pub fn view(&self) -> ConfigurationCheckOperation {
        ConfigurationCheckOperation {
            id: Some(self.id.clone()),
            application_id: Some(self.application_id.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            configuration_check_id: Some(self.check_id.clone()),
            configuration_check_name: None,
            configuration_check_description: None,
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            rule_status_counts: None,
        }
    }

    /// The value a `ListConfigurationCheckOperations` filter whose `Name` is `name`
    /// compares against, or `None` when this record has no such attribute.
    ///
    /// The names are the string-valued members the model gives
    /// `ConfigurationCheckOperation` — `Id`, `ApplicationId`, `Status`,
    /// `ConfigurationCheckId` — which is the shape that listing returns, so a filter
    /// names something the response itself reports. See
    /// [`CONFIGURATION_CHECK_FILTER_NAMES`].
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Id" => Some(self.id.as_str()),
            "ApplicationId" => Some(self.application_id.as_str()),
            "Status" => Some(self.status.as_str()),
            "ConfigurationCheckId" => Some(self.check_id.as_str()),
            _ => None,
        }
    }
}

/// The configuration checks one `StartConfigurationChecks` call runs, in the order
/// their operations are reported.
///
/// `ConfigurationCheckIds` names "the list of configuration checks to perform", so a
/// request that names some runs exactly those; one that names none runs the service's
/// whole catalogue of checks, which is the model's `ConfigurationCheckType` enum (see
/// [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]) — a check added to the
/// model is therefore started here without this function changing.
///
/// A check named twice in one request is one check to perform, so the result holds each
/// requested check once, keeping the position of its first mention.
pub fn configuration_checks_to_run(requested: Option<&[String]>) -> Vec<String> {
    let named = requested.unwrap_or(&[]);
    if named.is_empty() {
        return CONFIGURATION_CHECK_TYPE_VALUES
            .iter()
            .map(|check| (*check).to_string())
            .collect();
    }
    let mut checks: Vec<String> = Vec::with_capacity(named.len());
    for check in named {
        if !checks.iter().any(|seen| seen == check) {
            checks.push(check.clone());
        }
    }
    checks
}

/// Everything Systems Manager for SAP holds in ONE Region: the applications registered
/// there, the operation records filed there, their tags, and the resource permissions
/// granted there.
///
/// The service is regional, so a registration lives in the account AND the Region it was
/// made in: that is why every application ARN embeds a Region
/// ([`application_arn`]) and why this crate's refusals state the account and Region the
/// lookup ran in. The store is therefore partitioned per Region and a handler reaches
/// ONLY the partition of the Region its request was signed for
/// ([`ComAmazonawsSsmsapSsmsapState::region`] /
/// [`ComAmazonawsSsmsapSsmsapState::region_entry`]): nothing registered at one regional
/// endpoint is listed, read, controlled or deregistered through another, and an
/// `ApplicationId` in use in one Region is free in every other.
///
/// `applications` and `operations` are `BTreeMap`s rather than `HashMap`s because
/// `ListApplications` pages over the first: iteration order has to be the same for the
/// call that hands out a `NextToken` and the call that resumes from it, and id order is
/// that stable order.
#[derive(Debug, Default)]
pub struct RegionState {
    /// Every application registered in this Region, keyed by `ApplicationId`.
    pub applications: BTreeMap<String, StoredApplication>,
    /// Every operation record filed in this Region, keyed by `OperationId`. Bounded by
    /// [`MAX_RETAINED_OPERATIONS`].
    pub operations: BTreeMap<String, StoredOperation>,
    /// Every configuration check operation, keyed by its own `OperationId` — written by
    /// `StartConfigurationChecks` and read by `GetConfigurationCheckOperation`. A
    /// separate map from `operations` because the two are separate resources in the
    /// model: one is read as an `Operation`, the other as a
    /// `ConfigurationCheckOperation`. Bounded by [`MAX_RETAINED_OPERATIONS`] too.
    pub configuration_check_operations: BTreeMap<String, StoredConfigurationCheckOperation>,
    /// Tags by resource ARN — written by `RegisterApplication`'s `Tags` and read by
    /// `GetApplication`/`ListApplications`, so the later tagging APIs
    /// (`TagResource`/`UntagResource`/`ListTagsForResource`) have one place to join.
    /// An application ARN embeds its Region, so these keys are this Region's ARNs.
    pub tags_by_arn: HashMap<String, HashMap<String, String>>,
    /// The resource permission grants held for each target resource ARN, written by
    /// `PutResourcePermission` and read by `GetResourcePermission`, so both report one
    /// policy for a resource rather than two copies of it.
    ///
    /// A `BTreeMap` for the same reason as [`ResourceGrants`]: the policy document a
    /// read renders has to be byte-identical between reads of an unchanged resource.
    pub resource_permissions: BTreeMap<String, ResourceGrants>,
}

impl RegionState {
    /// Materialise every status change the clock already owes, on applications and on
    /// operations alike.
    ///
    /// Every handler calls this first, so each one reports the same thing about a
    /// resource at the same instant.
    pub fn settle(&mut self, now: SystemTime) {
        for application in self.applications.values_mut() {
            application.settle(now);
        }
        for operation in self.operations.values_mut() {
            operation.settle(now);
        }
        for check in self.configuration_check_operations.values_mut() {
            check.settle(now);
        }
    }

    /// Whether this Region holds nothing at all — the state an unused partition is in,
    /// and the one a partition returns to when its last application is deregistered.
    pub fn is_empty(&self) -> bool {
        self.applications.is_empty()
            && self.operations.is_empty()
            && self.configuration_check_operations.is_empty()
            && self.tags_by_arn.is_empty()
            && self.resource_permissions.is_empty()
    }

    /// Resolve the application a request names by its id, its ARN, or its AppRegistry
    /// ARN — the three members `GetApplication` accepts — to an `ApplicationId` stored
    /// IN THIS REGION.
    ///
    /// So a reference to an application of another Region resolves to nothing here,
    /// including an ARN: an application ARN embeds the Region it was registered in, so
    /// it cannot match a record of this one.
    pub fn resolve_application_id(
        &self,
        application_id: Option<&str>,
        application_arn: Option<&str>,
        app_registry_arn: Option<&str>,
    ) -> Option<String> {
        if let Some(id) = application_id {
            return self.applications.get(id).map(|app| app.id.clone());
        }
        if let Some(arn) = application_arn {
            return self
                .applications
                .values()
                .find(|app| app.arn == arn)
                .map(|app| app.id.clone());
        }
        if let Some(arn) = app_registry_arn {
            // Nothing in this slice registers an application through AppRegistry, so no
            // stored application carries an `AppRegistryArn` to match — the lookup is
            // here so adding one later needs no new resolution path.
            return self
                .applications
                .values()
                .find(|app| app.app_registry_arn.as_deref() == Some(arn))
                .map(|app| app.id.clone());
        }
        None
    }

    /// The tags held for `arn`, absent when none are.
    pub fn tags_for(&self, arn: &str) -> Option<HashMap<String, String>> {
        self.tags_by_arn
            .get(arn)
            .filter(|tags| !tags.is_empty())
            .cloned()
    }

    /// Whether `arn` names a resource THIS Region holds.
    ///
    /// The resources AWS Systems Manager for SAP holds are the registered SAP
    /// application and the SAP HANA databases of that application, so an ARN resolves
    /// here when it is a registered application's own ARN
    /// ([`StoredApplication::arn`]) or the ARN of one of the databases derived from
    /// that registration ([`StoredApplication::databases`]) — and nothing else, since
    /// the registration is the only thing this store holds.
    ///
    /// `region` is the Region this partition holds, which is the Region those ARNs
    /// embed: an ARN of another Region therefore resolves to nothing here, exactly as
    /// an application of another Region does ([`Self::resolve_application_id`]).
    pub fn holds_resource(&self, region: &str, arn: &str) -> bool {
        // An application is resolved from its ARN the ONE way this store resolves one,
        // and that pass costs a string comparison per application; only when it finds
        // nothing is the database set DERIVED, which is the one place that derivation
        // lives ([`StoredApplication::databases`]).
        if self.resolve_application_id(None, Some(arn), None).is_some() {
            return true;
        }
        self.applications.values().any(|application| {
            application
                .databases(region)
                .iter()
                .any(|database| database.arn == arn)
        })
    }

    /// Permit `source_resource_arn` to take `action_type` on `resource_arn`.
    ///
    /// Adding a grant that is already held changes nothing — the grant is the
    /// (resource, action type, source) triple, so a repeated `PutResourcePermission`
    /// leaves one statement rather than a duplicate — and `Ok(false)` reports that
    /// nothing was added. Refuses with the modelled `ValidationException` at the
    /// store ceilings ([`MAX_PERMISSIONED_RESOURCES`],
    /// [`MAX_PERMISSION_SOURCES_PER_RESOURCE`]), and writes nothing when it refuses.
    pub fn grant_resource_permission(
        &mut self,
        resource_arn: &str,
        action_type: &str,
        source_resource_arn: &str,
    ) -> Result<bool, ValidationException> {
        let held_for_resource = self.resource_permissions.get(resource_arn);
        let held_sources = held_for_resource.and_then(|grants| grants.get(action_type));
        if held_sources.is_some_and(|sources| sources.contains(source_resource_arn)) {
            return Ok(false);
        }
        if held_for_resource.is_none()
            && self.resource_permissions.len() >= MAX_PERMISSIONED_RESOURCES
        {
            return Err(validation(format!(
                "this emulator holds permissions for at most {MAX_PERMISSIONED_RESOURCES} resources per Region"
            )));
        }
        if held_sources.map_or(0, BTreeSet::len) >= MAX_PERMISSION_SOURCES_PER_RESOURCE {
            return Err(validation(format!(
                "this emulator holds at most {MAX_PERMISSION_SOURCES_PER_RESOURCE} {action_type} permissions on one resource"
            )));
        }
        self.resource_permissions
            .entry(resource_arn.to_string())
            .or_default()
            .entry(action_type.to_string())
            .or_default()
            .insert(source_resource_arn.to_string());
        Ok(true)
    }

    /// The resource policy document held for `resource_arn`, narrowed to
    /// `action_type` when the caller names one, and `None` when no grant matches —
    /// a resource nobody has granted anything on carries no policy, and nothing is
    /// fabricated in its place.
    ///
    /// The document is PROJECTED from the stored grants, one `Allow` statement per
    /// (action type, source resource) pair in the stable order the containers hold
    /// them, so two reads of an unchanged resource render the same bytes and the
    /// statement a grant added disappears exactly when the grant does.
    pub fn resource_policy(&self, resource_arn: &str, action_type: Option<&str>) -> Option<String> {
        let grants = self.resource_permissions.get(resource_arn)?;
        let statements: Vec<::serde_json::Value> = grants
            .iter()
            .filter(|(action, _)| action_type.is_none_or(|wanted| wanted == action.as_str()))
            .flat_map(|(action, sources)| {
                let action = permission_policy_action(action);
                sources.iter().map(move |source| {
                    ::serde_json::json!({
                        "Effect": POLICY_EFFECT_ALLOW,
                        // A resource policy names WHO is permitted; for a resource
                        // permission that is the source resource the caller granted.
                        "Principal": { "AWS": source },
                        "Action": action,
                        "Resource": resource_arn,
                    })
                })
            })
            .collect();
        if statements.is_empty() {
            return None;
        }
        Some(
            ::serde_json::json!({
                "Version": POLICY_VERSION,
                "Statement": statements,
            })
            .to_string(),
        )
    }

    /// File an operation record against `application` and return its `OperationId`.
    ///
    /// The record starts `INPROGRESS` with its success due [`TRANSITION_SPAN`] later;
    /// [`StoredOperation::settle`] materialises the transition once the clock reaches
    /// it. The id is a fresh UUID, which is what the model's `OperationId` pattern
    /// describes.
    pub fn record_operation(
        &mut self,
        op_type: &str,
        application: &StoredApplication,
        properties: HashMap<String, String>,
        now: SystemTime,
    ) -> String {
        let id = new_operation_id();
        self.operations.insert(
            id.clone(),
            StoredOperation {
                id: id.clone(),
                op_type: op_type.to_string(),
                status: OPERATION_STATUS_INPROGRESS.to_string(),
                // `StatusMessage` explains a non-success status; an operation that is
                // still running has nothing to explain.
                status_message: None,
                properties,
                resource_type: APPLICATION_RESOURCE_TYPE.to_string(),
                resource_id: application.id.clone(),
                resource_arn: application.arn.clone(),
                start_time: now,
                // An operation that has not finished has no end time yet.
                end_time: None,
                last_updated_time: now,
                success_due_at: now + TRANSITION_SPAN,
            },
        );
        evict_oldest(&mut self.operations, StoredOperation::eviction_rank);
        id
    }

    /// File a configuration check operation for `check_id` against `application_id` and
    /// return the record, so the caller reports the SAME thing a later
    /// `GetConfigurationCheckOperation` reads.
    ///
    /// The record starts `INPROGRESS` with its terminal status due [`TRANSITION_SPAN`]
    /// later — a configuration check is asynchronous, so a caller acting straight after
    /// the start call still observes it running — and
    /// [`StoredConfigurationCheckOperation::settle`] materialises that transition once
    /// the clock reaches it. The id is a fresh UUID, which is what the model's
    /// `OperationId` pattern describes.
    pub fn record_configuration_check(
        &mut self,
        application_id: &str,
        check_id: &str,
        now: SystemTime,
    ) -> StoredConfigurationCheckOperation {
        let record = StoredConfigurationCheckOperation {
            id: new_operation_id(),
            application_id: application_id.to_string(),
            check_id: check_id.to_string(),
            status: OPERATION_STATUS_INPROGRESS.to_string(),
            // `StatusMessage` explains a non-success status; a check that is still
            // running has nothing to explain.
            status_message: None,
            start_time: now,
            // A check that has not finished has no end time yet.
            end_time: None,
            terminal_due_at: now + TRANSITION_SPAN,
        };
        self.configuration_check_operations
            .insert(record.id.clone(), record.clone());
        evict_oldest(
            &mut self.configuration_check_operations,
            StoredConfigurationCheckOperation::eviction_rank,
        );
        record
    }

    /// The configuration check operation for `check_id` that is still running against
    /// `application_id`, if one is.
    ///
    /// Call [`Self::settle`] first, so a check whose terminal status the clock already
    /// owes does not read as running.
    pub fn configuration_check_in_progress(
        &self,
        application_id: &str,
        check_id: &str,
    ) -> Option<&StoredConfigurationCheckOperation> {
        self.configuration_check_operations.values().find(|record| {
            record.application_id == application_id
                && record.check_id == check_id
                && record.status == OPERATION_STATUS_INPROGRESS
        })
    }

    /// The configuration check operations filed against `application_id`, in the order
    /// a listing reports and pages them: by their own `OperationId`, which is the order
    /// [`Self::configuration_check_operations`] is keyed in, so the keys a page is taken
    /// over are already ascending and a `NextToken` resumes where the previous page
    /// stopped.
    ///
    /// `latest_per_check` is the `LATEST_PER_CHECK` listing mode: only the most recently
    /// STARTED record of each `ConfigurationCheckType` survives, so the listing holds one
    /// row per check the application has ever had run rather than every run of it. Two
    /// records started in the same instant are ordered by their ids, so the choice is
    /// deterministic. `false` is `ALL_OPERATIONS`: every record, nothing collapsed.
    ///
    /// Call [`Self::settle`] first, so a record whose terminal status the clock already
    /// owes is reported in it.
    pub fn configuration_check_operations_for(
        &self,
        application_id: &str,
        latest_per_check: bool,
    ) -> Vec<&StoredConfigurationCheckOperation> {
        let mine = self
            .configuration_check_operations
            .values()
            .filter(|record| record.application_id == application_id);
        if !latest_per_check {
            return mine.collect();
        }
        let mut latest: BTreeMap<&str, &StoredConfigurationCheckOperation> = BTreeMap::new();
        for record in mine {
            latest
                .entry(record.check_id.as_str())
                .and_modify(|held| {
                    if (record.start_time, record.id.as_str())
                        > (held.start_time, held.id.as_str())
                    {
                        *held = record;
                    }
                })
                .or_insert(record);
        }
        // Back into the order the whole listing is paged in: by operation id.
        let mut kept: Vec<&StoredConfigurationCheckOperation> =
            latest.into_values().collect();
        kept.sort_by(|left, right| left.id.cmp(&right.id));
        kept
    }

    /// Drop every operation record filed against `application_id`, configuration check
    /// operations included — used when the application is deregistered, since nothing
    /// else would ever reclaim them and the resource they describe no longer exists.
    pub fn forget_operations_for(&mut self, application_id: &str) {
        self.operations
            .retain(|_, record| record.resource_id != application_id);
        self.configuration_check_operations
            .retain(|_, record| record.application_id != application_id);
    }
}

/// Keep `records` within [`MAX_RETAINED_OPERATIONS`], dropping the oldest record that
/// has already reached a terminal status first so a caller polling a fresh record can
/// still resolve it.
///
/// `rank` orders the candidates for eviction: the record with the smallest rank goes
/// first, so a rank of `(still running, start time)` evicts the oldest finished record
/// before any running one.
fn evict_oldest<T, F, K>(records: &mut BTreeMap<String, T>, rank: F)
where
    F: Fn(&T) -> K,
    K: Ord,
{
    while records.len() > MAX_RETAINED_OPERATIONS {
        let victim = records
            .iter()
            .min_by_key(|(_, record)| rank(record))
            .map(|(id, _)| id.clone());
        match victim {
            Some(id) => {
                records.remove(&id);
            }
            None => break,
        }
    }
}

/// Per-service state for com-amazonaws-ssmsap-ssmsap. A handler reaches it through [`service_state`].
///
/// Systems Manager for SAP is a REGIONAL service, so the store holds one
/// [`RegionState`] per Region rather than one flat set of applications: every handler
/// resolves the Region its request was signed for ([`request_region`]) and then reaches
/// that Region's partition alone. The partitions are a `BTreeMap` so the store has one
/// stable order whatever order Regions were first written in.
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmsapSsmsapState {
    /// One partition per Region that holds something. Bounded by [`MAX_REGIONS`];
    /// written only through [`Self::region_entry`], so a READ never mints one.
    regions: BTreeMap<String, RegionState>,
}

impl ComAmazonawsSsmsapSsmsapState {
    /// Materialise every status change the clock already owes, in every Region.
    ///
    /// Every handler calls this first. It is not restricted to the request's Region
    /// because the change was due at its own instant everywhere, whether or not
    /// anybody called at that endpoint.
    pub fn settle(&mut self, now: SystemTime) {
        for region in self.regions.values_mut() {
            region.settle(now);
        }
    }

    /// What a request signed for `region` may observe: that Region's partition, and
    /// nothing else.
    ///
    /// A Region nothing has ever been stored in reads as an EMPTY partition, so a read
    /// answers honestly for any Region a caller names WITHOUT minting a partition for
    /// it — which is what keeps [`MAX_REGIONS`] a bound on what was actually written.
    pub fn region(&self, region: &str) -> &RegionState {
        static EMPTY: ::std::sync::OnceLock<RegionState> = ::std::sync::OnceLock::new();
        self.regions
            .get(region)
            .unwrap_or_else(|| EMPTY.get_or_init(RegionState::default))
    }

    /// The partition of `region` for WRITING, created when this is the first thing
    /// stored there. A caller that has already found a resource in `region` through
    /// [`Self::region`] creates nothing by reaching for it.
    pub fn region_entry(&mut self, region: &str) -> &mut RegionState {
        self.regions.entry(region.to_string()).or_default()
    }

    /// Whether the store already holds a partition for `region`.
    pub fn holds_region(&self, region: &str) -> bool {
        self.regions.contains_key(region)
    }

    /// How many Region partitions the store holds, which [`MAX_REGIONS`] bounds.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Drop `region`'s partition once nothing is left in it, so a Region that held an
    /// application and no longer does stops counting against [`MAX_REGIONS`]. Dropping
    /// an EMPTY partition changes nothing a caller can observe: an absent partition and
    /// an empty one read the same ([`Self::region`]).
    pub fn forget_region_if_empty(&mut self, region: &str) {
        if self.regions.get(region).is_some_and(RegionState::is_empty) {
            self.regions.remove(region);
        }
    }
}

/// A fresh `OperationId`: a v4 UUID, the form the model's `OperationId` pattern
/// (`^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`) describes.
pub fn new_operation_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The ARN of the application `application_id` in `region`:
/// `arn:aws:ssm-sap:<region>:<account>:application/<ApplicationId>`.
///
/// The account segment comes from the core's single source of truth, never a literal.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("application/{application_id}"),
    )
}

/// The id of the component a registration declared with `sid` and `component_type`.
///
/// A component is identified by the System ID and the type it was declared with, so two
/// entries describing the same component are one component — and the HANA component a
/// database belongs to is named the same way as one the registration listed.
pub fn component_id(sid: &str, component_type: &str) -> String {
    format!("{sid}-{component_type}")
}

/// The ARN of the database `database_id` of the application `application_id` in
/// `region`: `arn:aws:ssm-sap:<region>:<account>:database/<ApplicationId>/<DatabaseId>`.
///
/// A database is a sub-resource of its application, so its ARN names both ids; the
/// account segment comes from the core's single source of truth, never a literal.
pub fn database_arn(region: &str, application_id: &str, database_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("database/{application_id}/{database_id}"),
    )
}

/// The `DatabaseId` of the SAP HANA database named `database_name`.
///
/// A database is identified by its name, so its id is a function of that name: every
/// character the model's `DatabaseId` pattern (`[\w\d]+$`) does not admit is replaced by
/// `_`, which that pattern does admit, and the result is cut to the modelled
/// `DatabaseId` length bound (1..300). So a name of any form yields an id of the
/// modelled form and the same name always yields the same id. A `DatabaseName` is at
/// most 100 characters, so only a name taken off a longer ARN segment
/// ([`database_name_from_arn`]) can reach that cut.
pub fn database_id_from_name(database_name: &str) -> String {
    database_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .take(MAX_DATABASE_ID_LENGTH)
        .collect()
}

/// The name the SAP HANA database ARN `database_arn` names its database by: the last
/// non-empty segment of the ARN, the `:`- and `/`-separated part that carries the name
/// of the resource an ARN names (a database ARN minted by [`database_arn`] ends with the
/// `DatabaseId`).
///
/// `None` when nothing past the ARN's separators is non-empty, so the ARN names no
/// database this service can identify and no database is derived from it.
pub fn database_name_from_arn(database_arn: &str) -> Option<&str> {
    database_arn
        .split([':', '/'])
        .rfind(|segment| !segment.is_empty())
}

/// The `DatabaseType` of the SAP HANA database named `database_name`.
///
/// The system database of a HANA system is the one named [`HANA_SYSTEM_DATABASE_NAME`]
/// (HANA database names are case-insensitive); every other database of the system is a
/// tenant database.
pub fn database_type_for(database_name: &str) -> &'static str {
    if database_name.eq_ignore_ascii_case(HANA_SYSTEM_DATABASE_NAME) {
        DATABASE_TYPE_SYSTEM
    } else {
        DATABASE_TYPE_TENANT
    }
}

/// The region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope (an unsigned call).
///
/// Deliberately a per-crate copy of a helper other service crates also hold: the
/// function that owns the parsing (`emulator_server::sigv4_credential_scope`) lives in
/// the vendored, read-only framework.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled server error every handler answers with when [`service_state`] finds
/// no store (this service mounted on an engine that holds none). The payload is shared;
/// each operation wraps it in its OWN generated error variant.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("the ssm-sap store is unavailable".to_string()),
    }
}

/// This service's modelled `ValidationException` (HTTP 400) carrying `message`.
pub fn validation(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(message.into()),
    }
}

/// A `RegisterApplication` input member the model leaves optional but the service
/// requires for SOME application types.
///
/// The model cannot express "required when `ApplicationType` is X"
/// (`smithy_query(query="operation", operation="RegisterApplication")` marks only
/// `ApplicationId`, `ApplicationType` and `Instances` `@required`), so the requirement
/// is conditional on the type the same request carries and belongs to the service.
/// Each variant knows the input member it names and the words a refusal names it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalMember {
    /// `RegisterApplication$Sid` — the application's System ID.
    Sid,
    /// `RegisterApplication$SapInstanceNumber` — the application's instance number.
    SapInstanceNumber,
    /// `RegisterApplication$Credentials` — the Secrets Manager credentials that reach
    /// the application's database. An EMPTY list specifies none, which is what the
    /// member's modelled `[]` default leaves behind when a request omits it.
    Credentials,
}

impl ConditionalMember {
    /// The input member this names, spelled as the model spells it.
    pub const fn member_name(self) -> &'static str {
        match self {
            Self::Sid => "Sid",
            Self::SapInstanceNumber => "SapInstanceNumber",
            Self::Credentials => "Credentials",
        }
    }

    /// How a refusal words the member — the service states the requirement in SAP's
    /// own vocabulary rather than with the wire member name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sid => "SID",
            Self::SapInstanceNumber => "instance number",
            Self::Credentials => "credentials",
        }
    }
}

/// The members `RegisterApplication` requires IN ADDITION to the modelled `@required`
/// ones, per `ApplicationType`.
///
/// A HANA application IS its database: AWS Systems Manager for SAP cannot manage one
/// without the System ID and instance number that name the HANA instance and the
/// credentials that reach it — which is why every member of `ApplicationCredential`
/// (`DatabaseName`, `CredentialType`, `SecretId`) is documented in terms of "the SAP
/// HANA database". A type whose row is EMPTY requires nothing beyond the modelled
/// members, so a registration of that type is accepted on those alone.
///
/// One row per value of the model's `ApplicationType` enum (see
/// [`crate::ops::types::APPLICATION_TYPE_VALUES`]); a type this table does not name
/// requires nothing conditionally.
pub const CONDITIONALLY_REQUIRED_MEMBERS: &[(&str, &[ConditionalMember])] = &[
    (
        "HANA",
        &[
            ConditionalMember::Sid,
            ConditionalMember::SapInstanceNumber,
            ConditionalMember::Credentials,
        ],
    ),
    ("SAP_ABAP", &[]),
];

/// The members an application of `application_type` must carry beyond the modelled
/// `@required` ones. Empty when the type requires none.
pub fn conditionally_required_members(application_type: &str) -> &'static [ConditionalMember] {
    CONDITIONALLY_REQUIRED_MEMBERS
        .iter()
        .find(|(name, _)| *name == application_type)
        .map(|(_, members)| *members)
        .unwrap_or(&[])
}

/// Refuse a registration that omits a member its `ApplicationType` conditionally
/// requires ([`CONDITIONALLY_REQUIRED_MEMBERS`]).
///
/// `Ok(())` when the type requires nothing, or when every member it requires is
/// carried; otherwise the modelled `ValidationException` (HTTP 400) stating the whole
/// requirement for that type — the requirement is one condition, so it is reported as
/// one sentence rather than per omitted member. The caller must apply this BEFORE
/// writing anything, so a refused registration leaves no application behind.
pub fn conditional_requirements_met(
    application_type: &str,
    sid: Option<&str>,
    sap_instance_number: Option<&str>,
    credentials: &[ApplicationCredential],
) -> Result<(), ValidationException> {
    let required = conditionally_required_members(application_type);
    let carried = |member: ConditionalMember| match member {
        ConditionalMember::Sid => sid.is_some(),
        ConditionalMember::SapInstanceNumber => sap_instance_number.is_some(),
        ConditionalMember::Credentials => !credentials.is_empty(),
    };
    if required.iter().copied().all(carried) {
        return Ok(());
    }
    Err(validation(format!(
        "{} of {application_type} instance must be specified for SAP applications with type {application_type}.",
        join_labels(required)
    )))
}

/// The labels of `members`, joined as a sentence lists them: `A`, `A and B`,
/// `A, B, and C`.
fn join_labels(members: &[ConditionalMember]) -> String {
    let labels: Vec<&str> = members.iter().map(|member| member.label()).collect();
    match labels.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, [first])) => format!("{first} and {last}"),
        Some((last, head)) => format!("{}, and {last}", head.join(", ")),
    }
}

/// The modelled `ValidationException` an operation that models no
/// `ResourceNotFoundException` is refused with when no registered application answers
/// the reference it named (`GetApplication`, `DeregisterApplication`).
///
/// A registration is scoped to an account and Region, so what the refusal states is
/// that THIS account holds no such application: it names the account the lookup ran in
/// and the reference the request carried. `GetApplication` models no
/// `ResourceNotFoundException` (`smithy_query(query="operation",
/// operation="GetApplication")`: `ValidationException` 400 and
/// `InternalServerException` 500 only), so an absent application is a 400 here rather
/// than a 404. The account comes from the core's single source of truth.
pub fn no_registered_application(reference: &str) -> ValidationException {
    validation(format!(
        "No registered application found for account: {}, and application: {reference}",
        ::engine::emulator_account_id()
    ))
}

/// This service's modelled `ResourceNotFoundException` (HTTP 404) carrying `message`.
pub fn not_found(message: impl Into<String>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(message.into()),
    }
}

/// The message a call reports when the application it names is not registered in this
/// account and Region.
///
/// Built from the request: the `ApplicationId` the caller sent is the only variable part,
/// so every handler that acts on or reads under the application a request names reports
/// the SAME template around it — which is what a client surfacing or matching on the
/// service's not-found wording reads. The template is AWS's, as the `ssm-sap`
/// application-control recordings show it, and it goes with the 404
/// `ResourceNotFoundException` those operations model. A call that models NO not-found
/// error answers the same condition with [`no_registered_application`] (400) instead.
pub fn application_not_found_message(reference: &str) -> String {
    format!("Application {reference} doesn't exist.")
}

/// The message a call that names a resource by ARN reports when this account and Region
/// hold no such resource.
///
/// Built from the request: the ARN the caller sent is the only variable part, so every
/// call that resolves a resource by its ARN reports the SAME template around it — the
/// same construction [`application_not_found_message`] uses for an application named by
/// its id. It goes with the 404 `ResourceNotFoundException` those operations model.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} doesn't exist.")
}

/// The message `GetApplication` reports when the request carries NONE of the identifier
/// members that could name an application.
///
/// A fixed diagnostic for a fixed condition — the request carries nothing to build it
/// from — so it is AWS's own wording for this refusal verbatim, as the `ssm-sap`
/// application-lookup recording shows it. The identifier members this operation accepts
/// are still the three the model gives it (`smithy_query(query="operation",
/// operation="GetApplication")`); the message names the two an SAP application is
/// ordinarily referenced by.
pub const MISSING_APPLICATION_IDENTIFIER_MESSAGE: &str =
    "applicationId or applicationArn need to be specified";

/// This service's modelled `ConflictException` (HTTP 409) carrying `message`.
pub fn conflict(message: impl Into<String>) -> ConflictException {
    ConflictException {
        message: Some(message.into()),
    }
}

/// The `Name` values a `ListApplications` filter may carry: the members the model
/// gives `ApplicationSummary`, which is the shape the listing returns. Filter names
/// are case-sensitive, as `Filter$Name`'s documentation states.
pub const FILTER_NAMES: &[&str] = &["Id", "DiscoveryStatus", "Type", "Arn"];

/// The `Name` values a `ListConfigurationCheckOperations` filter may carry: the
/// string-valued members the model gives `ConfigurationCheckOperation`, which is the
/// shape that listing returns. Filter names are case-sensitive, as `Filter$Name`'s
/// documentation states. See [`StoredConfigurationCheckOperation::filter_attribute`].
pub const CONFIGURATION_CHECK_FILTER_NAMES: &[&str] =
    &["Id", "ApplicationId", "Status", "ConfigurationCheckId"];

/// `FilterOperator` for exact equality; one of the model's `FilterOperator` values
/// (see [`crate::ops::types::FILTER_OPERATOR_VALUES`]).
pub const FILTER_OPERATOR_EQUALS: &str = "Equals";

/// `FilterOperator` for "at or after" the given value.
pub const FILTER_OPERATOR_GREATER_THAN_OR_EQUALS: &str = "GreaterThanOrEquals";

/// `FilterOperator` for "at or before" the given value.
pub const FILTER_OPERATOR_LESS_THAN_OR_EQUALS: &str = "LessThanOrEquals";

/// The longest `Filter$Value` a `ListApplications` request may carry.
///
/// The model gives `FilterValue` `@length(min: 1, max: 64)`
/// (`smithy_query(query="shape", shape="FilterValue")`), so the boundary validator
/// already refuses a longer value; [`filters_within_bounds`] re-states the bound HERE
/// so the work filtering does is bounded by something this crate can point at rather
/// than by a check that lives elsewhere.
pub const FILTER_VALUE_MAX_LENGTH: usize = 64;

/// The longest `Filter$Name` a `ListApplications` request may carry: the model gives
/// `FilterName` `@length(min: 1, max: 32)`.
pub const FILTER_NAME_MAX_LENGTH: usize = 32;

/// How many filters one `ListApplications` request may carry: the model gives
/// `FilterList` `@length(min: 1, max: 10)`.
pub const MAX_FILTERS: usize = 10;

/// The most pieces splitting one `Filter$Value` on `,` can yield, derived from
/// [`FILTER_VALUE_MAX_LENGTH`]: a value of `n` characters splits into at most `n + 1`
/// pieces. With [`MAX_FILTERS`] filters and at most [`MAX_APPLICATIONS`] applications,
/// the work [`filter_matches`] does for one request is bounded by a constant,
/// independent of the request body's size.
///
/// It is a CEILING on the split, never a truncation of the caller's list: a value the
/// model accepts cannot hold more pieces than this, so no value a caller sends is ever
/// dropped.
pub const MAX_FILTER_VALUES_PER_FILTER: usize = FILTER_VALUE_MAX_LENGTH + 1;

/// Refuse a filter list that exceeds the bounds the model declares.
///
/// `Ok(())` when every filter is within them. The boundary validator enforces the same
/// bounds from the model, so this cannot normally fire; it is here so the cost of
/// [`filter_matches`] over the whole store is bounded by a check in this crate.
pub fn filters_within_bounds(filters: &[Filter]) -> Result<(), ValidationException> {
    if filters.len() > MAX_FILTERS {
        return Err(validation(format!(
            "Filters holds at most {MAX_FILTERS} filters, but {} were given",
            filters.len()
        )));
    }
    for filter in filters {
        if filter.name.len() > FILTER_NAME_MAX_LENGTH {
            return Err(validation(format!(
                "Filter Name is at most {FILTER_NAME_MAX_LENGTH} characters, but {} were given",
                filter.name.len()
            )));
        }
        if filter.value.len() > FILTER_VALUE_MAX_LENGTH {
            return Err(validation(format!(
                "Filter Value is at most {FILTER_VALUE_MAX_LENGTH} characters, but {} were given",
                filter.value.len()
            )));
        }
    }
    Ok(())
}

/// Whether `application` satisfies one `ListApplications` filter.
///
/// Every filterable attribute is a string in the model, so `Equals` is exact
/// (case-sensitive) string equality and the two ordering operators compare the
/// attribute lexicographically. `Filter$Value`'s documentation says that "if you
/// specify multiple values for a filter, the values are joined with an OR, and the
/// request returns all results that match any of the specified values", so a
/// comma-separated `Value` matches when ANY of its values does. Several filters narrow
/// together.
///
/// The number of values compared is bounded: `FilterValue` is at most
/// [`FILTER_VALUE_MAX_LENGTH`] characters — enforced by the boundary validator from the
/// model AND re-stated by [`filters_within_bounds`] before any filter is applied — so
/// splitting it yields at most [`MAX_FILTER_VALUES_PER_FILTER`] pieces, whatever the
/// request body's size. The `take` makes that ceiling explicit at the only place the
/// caller's value is iterated; because it is the ceiling the length bound already
/// implies, it can never drop a value the caller sent.
pub fn filter_matches(application: &StoredApplication, filter: &Filter) -> bool {
    filter_matches_attribute(application.filter_attribute(&filter.name), filter)
}

/// Whether the attribute one filter names satisfies it, where `actual` is the value the
/// record reports for `filter.name` and `None` means the record has no such attribute
/// (which nothing matches).
///
/// This is the comparison every filtered listing of this service shares — see
/// [`filter_matches`] for `ListApplications` and
/// [`StoredConfigurationCheckOperation::filter_attribute`] for the configuration check
/// listing — so one rule decides what a `FilterOperator` means whatever is being listed.
pub fn filter_matches_attribute(actual: Option<&str>, filter: &Filter) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    filter
        .value
        .split(',')
        .take(MAX_FILTER_VALUES_PER_FILTER)
        .any(|wanted| match filter.operator.as_str() {
            FILTER_OPERATOR_EQUALS => actual == wanted,
            FILTER_OPERATOR_GREATER_THAN_OR_EQUALS => actual >= wanted,
            FILTER_OPERATOR_LESS_THAN_OR_EQUALS => actual <= wanted,
            // The caller's operator is checked against the model's own catalogue
            // before any filter is applied, so no other value reaches here.
            _ => false,
        })
}

/// One page of a listing: the items on it, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Why a listing refused its paging arguments.
#[derive(Debug)]
pub enum PageError {
    /// `NextToken` was not a token this service issued.
    InvalidToken(String),
}

/// Take one page out of an id-ordered listing.
///
/// `all` is every item that matches the request, paired with the key it is ordered by.
/// The page holds at most `max_results` items starting at `next_token`, and carries a
/// token only when further items remain — so a caller that follows tokens to exhaustion
/// sees every item exactly once and the paging terminates.
pub fn page<T>(
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, PageError> {
    let start = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(token).ok_or_else(|| PageError::InvalidToken(token.to_string()))?,
        ),
    };
    // `MaxResults` carries `@range(1..50)`, so the boundary validator has already
    // refused anything outside it; an absent `MaxResults` takes the largest page the
    // range allows, and the `clamp` keeps that true however the range is read.
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;

    let mut rest = all
        .into_iter()
        .skip_while(|(key, _)| start.as_deref().is_some_and(|from| key.as_str() < from));
    let items: Vec<T> = rest.by_ref().take(limit).map(|(_, item)| item).collect();
    // The token names the first item NOT on this page, so the next call resumes at it.
    let next_token = rest.next().map(|(key, _)| encode_page_token(&key));
    Ok(Page { items, next_token })
}

/// Render a paging refusal as this service's modelled `ValidationException`, naming
/// the member that was wrong so the caller can fix the request from the text alone.
pub fn page_validation_exception(error: &PageError) -> ValidationException {
    match error {
        PageError::InvalidToken(token) => {
            validation(format!("NextToken {token} is not a valid pagination token"))
        }
    }
}

/// Marks a continuation token as this service's, and at which version it was issued.
///
/// Long enough on its own that the hex rendering of any token always satisfies the
/// model's `NextToken` pattern (`^.{16,2048}$`, so at least 16 characters).
const PAGE_TOKEN_PREFIX: &str = "ssmsap:applications:v1:";

/// Encode a listing key as the opaque `NextToken` this service hands out.
///
/// The key is prefixed with a version marker before hex-encoding, so the token is
/// opaque to a client (it is not the resource id it was built from) AND a value this
/// service did not issue is refused rather than resumed from. It stays a pure function
/// of the key, so the token is stable and carries no server-side cursor state.
fn encode_page_token(key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_PREFIX}{key}"))
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did not issue.
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmsapSsmsapState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmsapSsmsapState>)
}

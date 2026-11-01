// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mgn-applicationmigrationservice service.
//!
//! mgn is account-scoped: `InitializeService` initializes the account a request acts
//! on, and every other operation refuses with `UninitializedAccountException` (HTTP
//! 400) until it has. That flag and the application records live in ONE store here, so
//! every handler reads and writes the same state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ops::types::{
    ApplicationAggregatedStatus, CidrMapping, ConflictException, CreateApplicationOutput,
    CreateNetworkMigrationDefinitionOutput, DataReplicationInfo, InternalServerException,
    LaunchedInstance, LifeCycle, NetworkMigrationExecution as NetworkMigrationExecutionShape,
    ResourceNotFoundException, ServiceQuotaExceededException, SourceConfiguration,
    SourceProperties, SourceServerConnectorAction, TargetNetwork, TargetS3Configuration,
    UninitializedAccountException, UpdateSourceServerOutput, ValidationException,
    ValidationExceptionField,
};

/// The region an ARN carries when the request names none: the emulator's default.
const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / sigv4 name of this service, as the service's Smithy model
/// records it (`smithy_query(query="service_info")` -> `awsService.arnNamespace`).
const ARN_NAMESPACE: &str = "mgn";

/// The `applicationID` prefix the model's `^app-[0-9a-zA-Z]{17}$` pattern requires.
const APPLICATION_ID_PREFIX: &str = "app-";

/// How many `[0-9a-zA-Z]` characters follow the prefix of an `ApplicationID`
/// (model pattern `^app-[0-9a-zA-Z]{17}$`, length 21..21).
const APPLICATION_ID_BODY_LEN: usize = 17;

/// The `sourceServerID` prefix the model's `^s-[0-9a-zA-Z]{17}$` pattern requires.
const SOURCE_SERVER_ID_PREFIX: &str = "s-";

/// The `networkMigrationDefinitionID` prefix the model's `^nmd-[0-9a-zA-Z]{17}$`
/// pattern requires.
const NETWORK_MIGRATION_DEFINITION_ID_PREFIX: &str = "nmd-";

/// How many `[0-9a-zA-Z]` characters follow the prefix of a
/// `NetworkMigrationDefinitionID` (model pattern `^nmd-[0-9a-zA-Z]{17}$`, length
/// 21..21).
const NETWORK_MIGRATION_DEFINITION_ID_BODY_LEN: usize = 17;

/// The `VpcProvisioningStrategy` value that migrates into VPCs that already exist —
/// the only one a network migration definition may carry CIDR mappings under, as the
/// model documents `cidrMappings` ("CIDR mappings can be provided only when
/// `vpcProvisioningStrategy` is set to `USE_EXISTING`").
pub const VPC_PROVISIONING_USE_EXISTING: &str = "USE_EXISTING";

/// How many `[0-9a-zA-Z]` characters follow the prefix of a `SourceServerID`
/// (model pattern `^s-[0-9a-zA-Z]{17}$`, length 19..19).
const SOURCE_SERVER_ID_BODY_LEN: usize = 17;

/// The most applications this store holds at a time — per account AND in total.
///
/// This is the emulator's own bound on a caller-driven growth site, not a claim about
/// any AWS quota: without it a caller looping `CreateApplication` grows the process
/// heap without end. `CreateApplication` checks the account's count and the whole
/// store's count against it and refuses past either with the modelled
/// `ServiceQuotaExceededException`, so the record count cannot be multiplied by
/// varying the request's `accountID`. `DeleteApplication` releases records, so a
/// maximum count needs no eviction policy behind it.
pub const APPLICATION_QUOTA: usize = 1_000;

/// The most source servers this store holds at a time — per account AND in total.
///
/// The emulator's own bound on the source-server growth site, not a claim about any
/// AWS quota: the same reasoning as [`APPLICATION_QUOTA`]. Whatever registers a source
/// server checks [`ComAmazonawsMgnApplicationmigrationserviceState::source_server_store_is_full`]
/// before minting anything, so the record count cannot be grown past the bound by
/// varying the request's `accountID`.
pub const SOURCE_SERVER_QUOTA: usize = 1_000;

/// The most accounts this store tracks as initialized at a time.
///
/// The emulator's own bound on the other caller-driven growth site: the account
/// keyspace. `InitializeService` initializes only the caller's own account (it models
/// no `accountID` member), so the set holds one entry in practice — this bound makes
/// that structural rather than incidental, and `InitializeService` refuses past it
/// with the modelled `ValidationException`.
pub const ACCOUNT_QUOTA: usize = 16;

/// The most network migration definitions this store holds at a time — per account
/// AND in total.
///
/// The emulator's own bound on the network-migration-definition growth site, not a
/// claim about any AWS quota: the same reasoning as [`APPLICATION_QUOTA`].
/// `CreateNetworkMigrationDefinition` checks
/// [`ComAmazonawsMgnApplicationmigrationserviceState::network_migration_definition_store_is_full`]
/// before minting anything and refuses past it with the modelled
/// `ServiceQuotaExceededException`, so the record count cannot be grown past the bound.
pub const NETWORK_MIGRATION_DEFINITION_QUOTA: usize = 1_000;

/// The `ExecutionStatus` value an execution carries before anything has started it:
/// the status a definition's staged execution waits in (the model's `ExecutionStatus`
/// values are `PENDING`, `STARTED`, `SUCCEEDED`, `FAILED`).
pub const EXECUTION_STATUS_PENDING: &str = "PENDING";

/// The `ExecutionStage` value a staged execution waits at: the first stage of the
/// network migration pipeline, which a definition's execution is staged for (the
/// model's `ExecutionStage` values are `MAPPING`, `CODE_GENERATION`, `DEPLOY`,
/// `DEPLOYED_STACKS_DELETION`, `ANALYZE`).
pub const EXECUTION_STAGE_MAPPING: &str = "MAPPING";

/// The most network migration executions this store holds at a time — per account AND
/// in total.
///
/// The emulator's own bound on the execution growth site, not a claim about any AWS
/// quota: the same reasoning as [`APPLICATION_QUOTA`]. Whatever records an execution
/// goes through
/// [`ComAmazonawsMgnApplicationmigrationserviceState::insert_network_migration_execution`],
/// which refuses past the bound rather than growing the heap without end.
pub const NETWORK_MIGRATION_EXECUTION_QUOTA: usize = 1_000;

/// Per-service state for com-amazonaws-mgn-applicationmigrationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMgnApplicationmigrationserviceState {
    /// The accounts `InitializeService` has run for. Bounded by [`ACCOUNT_QUOTA`].
    pub initialized_accounts: HashSet<String>,
    /// Every application, by `applicationID`.
    pub applications: HashMap<String, Application>,
    /// The `applicationID`s in creation order, so `ListApplications` pages in a
    /// stable order and a `nextToken` resumes where the previous page stopped.
    pub application_order: Vec<String>,
    /// Every source server, by `sourceServerID`.
    pub source_servers: HashMap<String, SourceServer>,
    /// The `sourceServerID`s in registration order, so `DescribeSourceServers` pages
    /// in a stable order and a `nextToken` resumes where the previous page stopped.
    pub source_server_order: Vec<String>,
    /// Every network migration definition, by `networkMigrationDefinitionID`.
    pub network_migration_definitions: HashMap<String, NetworkMigrationDefinition>,
    /// The `networkMigrationDefinitionID`s in creation order, so a listing pages in a
    /// stable order and a `nextToken` resumes where the previous page stopped.
    pub network_migration_definition_order: Vec<String>,
    /// Every network migration execution, by `networkMigrationExecutionID`.
    pub network_migration_executions: HashMap<String, NetworkMigrationExecution>,
    /// The `networkMigrationExecutionID`s in creation order, so a listing pages in a
    /// stable order and a `nextToken` resumes where the previous page stopped.
    pub network_migration_execution_order: Vec<String>,
}

impl ComAmazonawsMgnApplicationmigrationserviceState {
    /// Whether `InitializeService` has run for `account_id`.
    pub fn is_initialized(&self, account_id: &str) -> bool {
        self.initialized_accounts.contains(account_id)
    }

    /// Every application of `account_id`, in creation order.
    pub fn applications_of(&self, account_id: &str) -> Vec<&Application> {
        self.application_order
            .iter()
            .filter_map(|id| self.applications.get(id))
            .filter(|app| app.account_id == account_id)
            .collect()
    }

    /// The application `application_id` names within `account_id`, or `None` — an
    /// application of another account is not visible to this one.
    pub fn application_of(&self, account_id: &str, application_id: &str) -> Option<&Application> {
        self.applications
            .get(application_id)
            .filter(|app| app.account_id == account_id)
    }

    /// Mutable access to the application `application_id` names within `account_id`.
    pub fn application_of_mut(
        &mut self,
        account_id: &str,
        application_id: &str,
    ) -> Option<&mut Application> {
        self.applications
            .get_mut(application_id)
            .filter(|app| app.account_id == account_id)
    }

    /// A fresh `applicationID` no stored application holds.
    pub fn mint_application_id(&self) -> String {
        loop {
            let candidate = mint_id(APPLICATION_ID_PREFIX, APPLICATION_ID_BODY_LEN);
            if !self.applications.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the store is full: either the account already holds
    /// [`APPLICATION_QUOTA`] applications, or the whole store does. Checked before a
    /// create mints anything, so neither the per-account count nor the total can be
    /// grown past the bound by varying the request's `accountID`.
    pub fn application_store_is_full(&self, account_id: &str) -> bool {
        self.applications.len() >= APPLICATION_QUOTA
            || self.applications_of(account_id).len() >= APPLICATION_QUOTA
    }

    /// Whether another account can be marked initialized: the set is bounded by
    /// [`ACCOUNT_QUOTA`], and an account already in it always can.
    pub fn can_initialize(&self, account_id: &str) -> bool {
        self.initialized_accounts.contains(account_id)
            || self.initialized_accounts.len() < ACCOUNT_QUOTA
    }

    /// Store a newly created application, remembering its place in creation order.
    ///
    /// Answers whether it was stored: callers check
    /// [`Self::application_store_is_full`] first, and a store already at
    /// [`APPLICATION_QUOTA`] refuses rather than growing past the bound, so a dropped
    /// write is never silent. Re-storing an id the map already holds keeps its place
    /// in the creation order rather than duplicating it.
    pub fn insert_application(&mut self, application: Application) -> bool {
        let known = self.applications.contains_key(&application.application_id);
        if !known && self.applications.len() >= APPLICATION_QUOTA {
            return false;
        }
        if !known {
            self.application_order
                .push(application.application_id.clone());
        }
        self.applications
            .insert(application.application_id.clone(), application);
        true
    }

    /// Drop one of `account_id`'s applications from the store and from the creation
    /// order, answering the removed record.
    ///
    /// The account key is REQUIRED, exactly as it is on every read accessor above: the
    /// `applications` map is keyed by id alone, so a removal that took only the
    /// request-controlled `applicationID` could destroy another account's record.
    /// `None` (nothing removed) when the id names no application of this account.
    pub fn remove_application(
        &mut self,
        account_id: &str,
        application_id: &str,
    ) -> Option<Application> {
        self.application_of(account_id, application_id)?;
        self.application_order.retain(|id| id != application_id);
        self.applications.remove(application_id)
    }

    /// Every source server of `account_id`, in registration order.
    pub fn source_servers_of(&self, account_id: &str) -> Vec<&SourceServer> {
        self.source_server_order
            .iter()
            .filter_map(|id| self.source_servers.get(id))
            .filter(|server| server.account_id == account_id)
            .collect()
    }

    /// The source server `source_server_id` names within `account_id`, or `None` — a
    /// source server of another account is not visible to this one.
    pub fn source_server_of(
        &self,
        account_id: &str,
        source_server_id: &str,
    ) -> Option<&SourceServer> {
        self.source_servers
            .get(source_server_id)
            .filter(|server| server.account_id == account_id)
    }

    /// A fresh `sourceServerID` no stored source server holds.
    pub fn mint_source_server_id(&self) -> String {
        loop {
            let candidate = mint_id(SOURCE_SERVER_ID_PREFIX, SOURCE_SERVER_ID_BODY_LEN);
            if !self.source_servers.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the source-server store is full: either the account already holds
    /// [`SOURCE_SERVER_QUOTA`] source servers, or the whole store does.
    pub fn source_server_store_is_full(&self, account_id: &str) -> bool {
        self.source_servers.len() >= SOURCE_SERVER_QUOTA
            || self.source_servers_of(account_id).len() >= SOURCE_SERVER_QUOTA
    }

    /// Store a source server, remembering its place in registration order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does:
    /// a store already at [`SOURCE_SERVER_QUOTA`] refuses rather than growing past the
    /// bound, and re-storing a known id keeps its place in the registration order
    /// rather than duplicating it.
    pub fn insert_source_server(&mut self, server: SourceServer) -> bool {
        let known = self.source_servers.contains_key(&server.source_server_id);
        if !known && self.source_servers.len() >= SOURCE_SERVER_QUOTA {
            return false;
        }
        if !known {
            self.source_server_order
                .push(server.source_server_id.clone());
        }
        self.source_servers
            .insert(server.source_server_id.clone(), server);
        true
    }

    /// Every network migration definition of `account_id`, in creation order.
    pub fn network_migration_definitions_of(
        &self,
        account_id: &str,
    ) -> Vec<&NetworkMigrationDefinition> {
        self.network_migration_definition_order
            .iter()
            .filter_map(|id| self.network_migration_definitions.get(id))
            .filter(|definition| definition.account_id == account_id)
            .collect()
    }

    /// The network migration definition `definition_id` names within `account_id`, or
    /// `None` — a definition of another account is not visible to this one.
    pub fn network_migration_definition_of(
        &self,
        account_id: &str,
        definition_id: &str,
    ) -> Option<&NetworkMigrationDefinition> {
        self.network_migration_definitions
            .get(definition_id)
            .filter(|definition| definition.account_id == account_id)
    }

    /// [`Self::network_migration_definition_of`], for a write: the record of this
    /// account, so what an update stores is what every reader of the same store reports
    /// afterwards. A definition of another account stays invisible here too.
    pub fn network_migration_definition_of_mut(
        &mut self,
        account_id: &str,
        definition_id: &str,
    ) -> Option<&mut NetworkMigrationDefinition> {
        self.network_migration_definitions
            .get_mut(definition_id)
            .filter(|definition| definition.account_id == account_id)
    }

    /// A fresh `networkMigrationDefinitionID` no stored definition holds.
    pub fn mint_network_migration_definition_id(&self) -> String {
        loop {
            let candidate = mint_id(
                NETWORK_MIGRATION_DEFINITION_ID_PREFIX,
                NETWORK_MIGRATION_DEFINITION_ID_BODY_LEN,
            );
            if !self.network_migration_definitions.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the network-migration-definition store is full: either the account
    /// already holds [`NETWORK_MIGRATION_DEFINITION_QUOTA`] definitions, or the whole
    /// store does. Checked before a create mints anything, so neither the per-account
    /// count nor the total can be grown past the bound.
    pub fn network_migration_definition_store_is_full(&self, account_id: &str) -> bool {
        self.network_migration_definitions.len() >= NETWORK_MIGRATION_DEFINITION_QUOTA
            || self.network_migration_definitions_of(account_id).len()
                >= NETWORK_MIGRATION_DEFINITION_QUOTA
    }

    /// Store a network migration definition, remembering its place in creation order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`NETWORK_MIGRATION_DEFINITION_QUOTA`] refuses rather than
    /// growing past the bound, and re-storing a known id keeps its place in the
    /// creation order rather than duplicating it.
    pub fn insert_network_migration_definition(
        &mut self,
        definition: NetworkMigrationDefinition,
    ) -> bool {
        let known = self
            .network_migration_definitions
            .contains_key(&definition.network_migration_definition_id);
        if !known && self.network_migration_definitions.len() >= NETWORK_MIGRATION_DEFINITION_QUOTA
        {
            return false;
        }
        if !known {
            self.network_migration_definition_order
                .push(definition.network_migration_definition_id.clone());
        }
        self.network_migration_definitions.insert(
            definition.network_migration_definition_id.clone(),
            definition,
        );
        true
    }

    /// Every execution recorded under the definition `definition_id` names within
    /// `account_id`, in creation order. An execution of another account, or of another
    /// definition, is not part of this selection.
    pub fn network_migration_executions_of(
        &self,
        account_id: &str,
        definition_id: &str,
    ) -> Vec<&NetworkMigrationExecution> {
        self.network_migration_execution_order
            .iter()
            .filter_map(|id| self.network_migration_executions.get(id))
            .filter(|execution| {
                execution.account_id == account_id
                    && execution.network_migration_definition_id == definition_id
            })
            .collect()
    }

    /// A fresh `networkMigrationExecutionID` no stored execution holds: 32 `[0-9a-f]`
    /// characters grouped 8-4-4-4-12, as the model's `NetworkMigrationExecutionID`
    /// pattern requires.
    pub fn mint_network_migration_execution_id(&self) -> String {
        loop {
            let candidate = mint_execution_id();
            if !self.network_migration_executions.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the network-migration-execution store is full: either the account
    /// already holds [`NETWORK_MIGRATION_EXECUTION_QUOTA`] executions, or the whole
    /// store does. Checked before anything mints an execution, so neither the
    /// per-account count nor the total can be grown past the bound.
    pub fn network_migration_execution_store_is_full(&self, account_id: &str) -> bool {
        self.network_migration_executions.len() >= NETWORK_MIGRATION_EXECUTION_QUOTA
            || self
                .network_migration_executions
                .values()
                .filter(|execution| execution.account_id == account_id)
                .count()
                >= NETWORK_MIGRATION_EXECUTION_QUOTA
    }

    /// Store a network migration execution, remembering its place in creation order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`NETWORK_MIGRATION_EXECUTION_QUOTA`] refuses rather than
    /// growing past the bound, and re-storing a known id keeps its place in the creation
    /// order rather than duplicating it.
    pub fn insert_network_migration_execution(
        &mut self,
        execution: NetworkMigrationExecution,
    ) -> bool {
        let known = self
            .network_migration_executions
            .contains_key(&execution.network_migration_execution_id);
        if !known && self.network_migration_executions.len() >= NETWORK_MIGRATION_EXECUTION_QUOTA {
            return false;
        }
        if !known {
            self.network_migration_execution_order
                .push(execution.network_migration_execution_id.clone());
        }
        self.network_migration_executions.insert(
            execution.network_migration_execution_id.clone(),
            execution,
        );
        true
    }

    /// Which record of `account_id` carries `resource_arn` as its own ARN, or `None`
    /// when no record of this account does.
    ///
    /// The tag operations address a resource by the ARN it was created with rather than
    /// by a typed id, so the resolution is over the ARN every taggable record stores:
    /// applications, source servers and network migration definitions — the kinds this
    /// store mints an ARN for. An ARN is compared whole, so one naming another Region,
    /// another vendor or another resource kind matches nothing, and a record of another
    /// account is not this one's to read or write.
    fn taggable_resource(&self, account_id: &str, resource_arn: &str) -> Option<TaggableResource> {
        if let Some(application) = self
            .applications
            .values()
            .find(|app| app.account_id == account_id && app.arn == resource_arn)
        {
            return Some(TaggableResource::Application(
                application.application_id.clone(),
            ));
        }
        if let Some(server) = self
            .source_servers
            .values()
            .find(|server| server.account_id == account_id && server.arn == resource_arn)
        {
            return Some(TaggableResource::SourceServer(
                server.source_server_id.clone(),
            ));
        }
        self.network_migration_definitions
            .values()
            .find(|definition| {
                definition.account_id == account_id && definition.arn == resource_arn
            })
            .map(|definition| {
                TaggableResource::NetworkMigrationDefinition(
                    definition.network_migration_definition_id.clone(),
                )
            })
    }

    /// The tags the resource `resource_arn` names within `account_id` carries, or
    /// `None` when the account holds no resource under that ARN.
    ///
    /// One map per resource: the same one the resource's own reader reports, so a tag
    /// write is visible through both.
    pub fn resource_tags(
        &self,
        account_id: &str,
        resource_arn: &str,
    ) -> Option<&BTreeMap<String, String>> {
        match self.taggable_resource(account_id, resource_arn)? {
            TaggableResource::Application(id) => self.applications.get(&id).map(|app| &app.tags),
            TaggableResource::SourceServer(id) => {
                self.source_servers.get(&id).map(|server| &server.tags)
            }
            TaggableResource::NetworkMigrationDefinition(id) => self
                .network_migration_definitions
                .get(&id)
                .map(|definition| &definition.configuration.tags),
        }
    }

    /// [`Self::resource_tags`], for a tag write: the resource's own map, so an added or
    /// removed pair is the one its reader reports afterwards.
    pub fn resource_tags_mut(
        &mut self,
        account_id: &str,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match self.taggable_resource(account_id, resource_arn)? {
            TaggableResource::Application(id) => {
                self.applications.get_mut(&id).map(|app| &mut app.tags)
            }
            TaggableResource::SourceServer(id) => self
                .source_servers
                .get_mut(&id)
                .map(|server| &mut server.tags),
            TaggableResource::NetworkMigrationDefinition(id) => self
                .network_migration_definitions
                .get_mut(&id)
                .map(|definition| &mut definition.configuration.tags),
        }
    }
}

/// Which record an ARN a tag operation named resolves to: the kind, and the id that
/// kind's map is keyed by.
///
/// The id rather than a borrow, so the same resolution serves a read and a write of the
/// record's tag map.
#[derive(Debug)]
enum TaggableResource {
    Application(String),
    SourceServer(String),
    NetworkMigrationDefinition(String),
}

/// One stored network migration execution: the run of a network migration definition
/// through the mapping, code-generation, deployment and analysis stages, as whatever
/// started it recorded it. `ListNetworkMigrationExecutions` reports these members and
/// NOTHING it computes per read, so two reads of an unchanged execution answer the same
/// record.
#[derive(Debug, Clone, Default)]
pub struct NetworkMigrationExecution {
    pub network_migration_execution_id: String,
    /// The definition this execution runs: the `networkMigrationDefinitionID` a listing
    /// selects executions by.
    pub network_migration_definition_id: String,
    /// The account the execution lives in, so one account's executions are never
    /// another's to read.
    pub account_id: String,
    /// One of the model's `ExecutionStatus` values (`PENDING`, `STARTED`, `SUCCEEDED`,
    /// `FAILED`), or absent while nothing has set one.
    pub status: Option<String>,
    /// One of the model's `ExecutionStage` values, or absent while nothing has set one.
    pub stage: Option<String>,
    /// One of the model's `ExecutionStageActivity` values, or absent while nothing has
    /// set one.
    pub activity: Option<String>,
    pub created_at: Option<::std::time::SystemTime>,
    pub updated_at: Option<::std::time::SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl NetworkMigrationExecution {
    /// The execution a definition is staged with: minted under `definition_id` in
    /// `account_id`, waiting in [`EXECUTION_STATUS_PENDING`] at the pipeline's first
    /// stage ([`EXECUTION_STAGE_MAPPING`]) with no activity, because nothing has run
    /// for it yet.
    pub fn staged(
        execution_id: String,
        definition_id: &str,
        account_id: &str,
        at: ::std::time::SystemTime,
    ) -> Self {
        Self {
            network_migration_execution_id: execution_id,
            network_migration_definition_id: definition_id.to_string(),
            account_id: account_id.to_string(),
            status: Some(EXECUTION_STATUS_PENDING.to_string()),
            stage: Some(EXECUTION_STAGE_MAPPING.to_string()),
            activity: None,
            // One instant for both: nothing has moved the execution on since it was
            // staged.
            created_at: Some(at),
            updated_at: Some(at),
            tags: BTreeMap::new(),
        }
    }

    /// The `tags` member: the stored tags, or absent when the execution carries none.
    pub fn tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        map_shape(&self.tags)
    }

    /// This execution as the model's `NetworkMigrationExecution` structure — the shape
    /// `ListNetworkMigrationExecutions` lists.
    pub fn to_shape(&self) -> NetworkMigrationExecutionShape {
        NetworkMigrationExecutionShape {
            network_migration_definition_id: Some(self.network_migration_definition_id.clone()),
            network_migration_execution_id: Some(self.network_migration_execution_id.clone()),
            status: self.status.clone(),
            stage: self.stage.clone(),
            activity: self.activity.clone(),
            created_at: self.created_at.map(to_date_time),
            updated_at: self.updated_at.map(to_date_time),
            tags: self.tags_shape(),
        }
    }
}

/// One stored source server: the modelled `SourceServer` record, as whatever
/// registered the server (an agent installation, a vCenter discovery, an import)
/// stored it. `DescribeSourceServers` reports these members and NOTHING it computes
/// per read, so two reads of an unchanged server answer the same record.
#[derive(Debug, Clone, Default)]
pub struct SourceServer {
    pub source_server_id: String,
    pub arn: String,
    /// The account the source server lives in (the registering request's `accountID`,
    /// or the emulator's own account when it named none).
    pub account_id: String,
    pub is_archived: bool,
    pub tags: BTreeMap<String, String>,
    /// One of the model's `ReplicationType` values (`AGENT_BASED`,
    /// `SNAPSHOT_SHIPPING`), or absent while nothing has set one.
    pub replication_type: Option<String>,
    /// The server's `LifeCycle`, whose `state` is one of the model's `LifeCycleState`
    /// values (`STOPPED`, `NOT_READY`, `READY_FOR_TEST`, `TESTING`,
    /// `READY_FOR_CUTOVER`, `CUTTING_OVER`, `CUTOVER`, `DISCONNECTED`, `DISCOVERED`,
    /// `PENDING_INSTALLATION`).
    pub life_cycle: Option<LifeCycle>,
    pub data_replication_info: Option<DataReplicationInfo>,
    pub source_properties: Option<SourceProperties>,
    pub launched_instance: Option<LaunchedInstance>,
    pub connector_action: Option<SourceServerConnectorAction>,
    /// The application this server is associated with, once something associated it.
    pub application_id: Option<String>,
    pub user_provided_id: Option<String>,
    pub fqdn_for_action_framework: Option<String>,
    /// The vCenter client that discovered the server, when one did.
    pub vcenter_client_id: Option<String>,
}

impl SourceServer {
    /// The server's `lifeCycle.state`, or `None` while it carries no life cycle — the
    /// value the `lifeCycleStates` filter selects on.
    pub fn life_cycle_state(&self) -> Option<&str> {
        self.life_cycle
            .as_ref()
            .and_then(|life_cycle| life_cycle.state.as_deref())
    }

    /// The `tags` member: the stored tags, or absent when the server carries none.
    pub fn tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }
    }

    /// This server as the model's `SourceServer` structure — the shape
    /// `DescribeSourceServers` lists.
    pub fn to_shape(&self) -> UpdateSourceServerOutput {
        UpdateSourceServerOutput {
            source_server_id: Some(self.source_server_id.clone()),
            arn: Some(self.arn.clone()),
            is_archived: Some(self.is_archived),
            tags: self.tags_shape(),
            launched_instance: self.launched_instance.clone(),
            data_replication_info: self.data_replication_info.clone(),
            life_cycle: self.life_cycle.clone(),
            source_properties: self.source_properties.clone(),
            replication_type: self.replication_type.clone(),
            vcenter_client_id: self.vcenter_client_id.clone(),
            application_id: self.application_id.clone(),
            user_provided_id: self.user_provided_id.clone(),
            fqdn_for_action_framework: self.fqdn_for_action_framework.clone(),
            connector_action: self.connector_action.clone(),
        }
    }
}

/// One stored application: everything `CreateApplication`, `ListApplications` and
/// `UpdateApplication` report, so a read never computes a value a write did not store.
#[derive(Debug, Clone, Default)]
pub struct Application {
    pub application_id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub is_archived: bool,
    pub aggregated_status: AggregatedStatus,
    pub creation_date_time: String,
    pub last_modified_date_time: String,
    pub tags: BTreeMap<String, String>,
    /// The wave this application belongs to, once `AssociateApplications` exists.
    pub wave_id: Option<String>,
    /// The account the application lives in (the request's `accountID`, or the
    /// emulator's own account when the request names none).
    pub account_id: String,
    /// The source servers associated with the application; its size is the
    /// `totalSourceServers` the aggregated status reports.
    pub source_server_ids: BTreeSet<String>,
}

impl Application {
    /// This application's `ApplicationAggregatedStatus`. `totalSourceServers` is the
    /// size of [`Application::source_server_ids`] rather than a second stored counter,
    /// so the count and the set can never disagree.
    pub fn aggregated_status_shape(&self) -> ApplicationAggregatedStatus {
        ApplicationAggregatedStatus {
            last_update_date_time: Some(self.aggregated_status.last_update_date_time.clone()),
            health_status: self.aggregated_status.health_status.clone(),
            progress_status: self.aggregated_status.progress_status.clone(),
            total_source_servers: self.source_server_ids.len() as i64,
        }
    }

    /// The `tags` member: the stored tags, or absent when the application carries none.
    pub fn tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(
                self.tags
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        }
    }

    /// This application as the model's `Application` structure — the output shape
    /// `CreateApplication` returns and `ListApplications` lists.
    pub fn to_shape(&self) -> CreateApplicationOutput {
        CreateApplicationOutput {
            application_id: Some(self.application_id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            is_archived: Some(self.is_archived),
            application_aggregated_status: Some(self.aggregated_status_shape()),
            creation_date_time: Some(self.creation_date_time.clone()),
            last_modified_date_time: Some(self.last_modified_date_time.clone()),
            tags: self.tags_shape(),
            wave_id: self.wave_id.clone(),
        }
    }
}

/// The stored half of `ApplicationAggregatedStatus`. `totalSourceServers` is NOT
/// stored — see [`Application::aggregated_status_shape`].
#[derive(Debug, Clone, Default)]
pub struct AggregatedStatus {
    pub last_update_date_time: String,
    /// One of the model's `ApplicationHealthStatus` values (`HEALTHY`, `LAGGING`,
    /// `ERROR`), or absent while the application has no source server to report on.
    pub health_status: Option<String>,
    /// One of the model's `ApplicationProgressStatus` values (`NOT_STARTED`,
    /// `IN_PROGRESS`, `COMPLETED`).
    pub progress_status: Option<String>,
}

/// The `ApplicationProgressStatus` of an application nothing has started migrating —
/// the state every application is created in.
pub const PROGRESS_NOT_STARTED: &str = "NOT_STARTED";

/// Everything a caller supplies about a network migration definition: the members
/// `CreateNetworkMigrationDefinition` stores and an update replaces.
///
/// Held apart from the minted id, the ARN and the timestamps so a create can tell an
/// `@idempotent` replay — a request whose every member equals a stored definition's —
/// from a request for a new definition. The operation models no idempotency token, so
/// the request's own members are the only thing a replay can be recognised by.
#[derive(Debug, Clone, Default)]
pub struct NetworkMigrationDefinitionConfiguration {
    pub name: String,
    pub description: Option<String>,
    pub source_configurations: Vec<SourceConfiguration>,
    pub target_s3_configuration: TargetS3Configuration,
    pub target_network: TargetNetwork,
    /// One of the model's `TargetDeployment` values (`SINGLE_ACCOUNT`,
    /// `MULTI_ACCOUNT`), or absent while the caller named none.
    pub target_deployment: Option<String>,
    /// One of the model's `VpcProvisioningStrategy` values (`CREATE_NEW`,
    /// `USE_EXISTING`), or absent while the caller named none.
    pub vpc_provisioning_strategy: Option<String>,
    pub cidr_mappings: Vec<CidrMapping>,
    /// Stored sorted, so the tags a read reports are in one stable order.
    pub tags: BTreeMap<String, String>,
    /// Stored sorted, for the same reason as [`Self::tags`].
    pub scope_tags: BTreeMap<String, String>,
}

impl NetworkMigrationDefinitionConfiguration {
    /// Whether two configurations carry the same members, member by member.
    ///
    /// The modelled shapes are generated without `PartialEq`, so the comparison is
    /// spelled out here rather than derived; it covers every member of the shape, so a
    /// configuration that differs anywhere is not a match.
    pub fn matches(&self, other: &Self) -> bool {
        self.name == other.name
            && self.description == other.description
            && self.target_deployment == other.target_deployment
            && self.vpc_provisioning_strategy == other.vpc_provisioning_strategy
            && self.tags == other.tags
            && self.scope_tags == other.scope_tags
            && same_target_s3_configuration(
                &self.target_s3_configuration,
                &other.target_s3_configuration,
            )
            && same_target_network(&self.target_network, &other.target_network)
            && self.source_configurations.len() == other.source_configurations.len()
            && self
                .source_configurations
                .iter()
                .zip(other.source_configurations.iter())
                .all(|(left, right)| same_source_configuration(left, right))
            && self.cidr_mappings.len() == other.cidr_mappings.len()
            && self
                .cidr_mappings
                .iter()
                .zip(other.cidr_mappings.iter())
                .all(|(left, right)| {
                    left.original_cidr == right.original_cidr
                        && left.updated_cidr == right.updated_cidr
                })
    }
}

fn same_target_s3_configuration(
    left: &TargetS3Configuration,
    right: &TargetS3Configuration,
) -> bool {
    left.s3_bucket == right.s3_bucket && left.s3_bucket_owner == right.s3_bucket_owner
}

fn same_target_network(left: &TargetNetwork, right: &TargetNetwork) -> bool {
    left.topology == right.topology
        && left.inbound_cidr == right.inbound_cidr
        && left.outbound_cidr == right.outbound_cidr
        && left.inspection_cidr == right.inspection_cidr
}

fn same_source_configuration(left: &SourceConfiguration, right: &SourceConfiguration) -> bool {
    left.source_environment == right.source_environment
        && left.source_s3_configuration.s3_bucket == right.source_s3_configuration.s3_bucket
        && left.source_s3_configuration.s3_bucket_owner
            == right.source_s3_configuration.s3_bucket_owner
        && left.source_s3_configuration.s3_key == right.source_s3_configuration.s3_key
}

/// One stored network migration definition: the minted id and ARN, the account it
/// lives in, when it was created and last updated, and the caller-supplied
/// configuration. `GetNetworkMigrationDefinition` reports exactly these members and
/// NOTHING it computes per read, so two reads of an unchanged definition answer the
/// same record.
#[derive(Debug, Clone)]
pub struct NetworkMigrationDefinition {
    pub network_migration_definition_id: String,
    pub arn: String,
    /// The account the definition lives in (the emulator's own account: the operation
    /// models no `accountID` member).
    pub account_id: String,
    pub created_at: ::std::time::SystemTime,
    pub updated_at: ::std::time::SystemTime,
    pub configuration: NetworkMigrationDefinitionConfiguration,
}

impl NetworkMigrationDefinition {
    /// The `tags` member: the stored tags, or absent when the definition carries none.
    pub fn tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        map_shape(&self.configuration.tags)
    }

    /// The `scopeTags` member: the stored scope tags, or absent when it carries none.
    pub fn scope_tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        map_shape(&self.configuration.scope_tags)
    }

    /// This definition as the model's `NetworkMigrationDefinition` structure — the
    /// output shape every network-migration-definition operation answers with.
    pub fn to_shape(&self) -> CreateNetworkMigrationDefinitionOutput {
        let configuration = &self.configuration;
        CreateNetworkMigrationDefinitionOutput {
            arn: Some(self.arn.clone()),
            network_migration_definition_id: Some(self.network_migration_definition_id.clone()),
            name: Some(configuration.name.clone()),
            description: configuration.description.clone(),
            source_configurations: list_shape(&configuration.source_configurations),
            target_s3_configuration: Some(configuration.target_s3_configuration.clone()),
            target_network: Some(configuration.target_network.clone()),
            target_deployment: configuration.target_deployment.clone(),
            vpc_provisioning_strategy: configuration.vpc_provisioning_strategy.clone(),
            cidr_mappings: list_shape(&configuration.cidr_mappings),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
            tags: self.tags_shape(),
            scope_tags: self.scope_tags_shape(),
        }
    }
}

/// A stored map as its wire member: the entries, or absent when there are none.
fn map_shape(
    stored: &BTreeMap<String, String>,
) -> Option<std::collections::HashMap<String, String>> {
    if stored.is_empty() {
        None
    } else {
        Some(
            stored
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// A stored list as its wire member: the entries, or absent when there are none.
fn list_shape<T: Clone>(stored: &[T]) -> Option<Vec<T>> {
    if stored.is_empty() {
        None
    } else {
        Some(stored.to_vec())
    }
}

/// A stored instant as the model's `Timestamp`, rendered by the SDK crate this
/// service already serialises its shapes with.
pub fn to_date_time(at: ::std::time::SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The account an operation acts on: the request's `accountID` member when it carries
/// one in the modelled shape (exactly 12 ASCII digits), else the emulator's own
/// canonical account.
///
/// The shape check keeps anything else from becoming a store key: the modelled
/// constraint is refused at the boundary before a handler runs, and nothing else may
/// widen the keyspace this state is partitioned by.
pub fn account_key(account_id: Option<&str>) -> String {
    account_id
        .filter(|id| id.len() == 12 && id.bytes().all(|b| b.is_ascii_digit()))
        .map(ToString::to_string)
        .unwrap_or_else(|| ::engine::emulator_account_id().to_string())
}

/// The region a request signed for, which its ARNs carry; [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an application: `arn:aws:mgn:<region>:<account>:application/<applicationID>`.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("application/{application_id}"),
    )
}

/// The ARN of a source server:
/// `arn:aws:mgn:<region>:<account>:source-server/<sourceServerID>`.
pub fn source_server_arn(region: &str, source_server_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("source-server/{source_server_id}"),
    )
}

/// The ARN of a network migration definition:
/// `arn:aws:mgn:<region>:<account>:network-migration-definition/<networkMigrationDefinitionID>`.
pub fn network_migration_definition_arn(region: &str, definition_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("network-migration-definition/{definition_id}"),
    )
}

/// Now, as the model's `ISO8601DatetimeString`: `YYYY-MM-DDTHH:MM:SS.fffffffffZ`.
///
/// 30 characters, inside the modelled length 19..32 and matching the modelled
/// pattern. The calendar rendering is `aws_smithy_types::DateTime`'s own RFC-3339
/// formatter (the SDK crate this service already depends on, so no date arithmetic is
/// written here); only the fraction is padded, to a fixed nine digits, so two of these
/// strings order the same way the instants they name do.
pub fn now_iso8601() -> String {
    iso8601(::aws_smithy_types::DateTime::from(
        ::std::time::SystemTime::now(),
    ))
}

fn iso8601(at: ::aws_smithy_types::DateTime) -> String {
    let rendered = at
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    // `Format::DateTime` renders `YYYY-MM-DDTHH:MM:SS[.fraction]Z`, omitting the
    // fraction when it is zero and trimming its trailing zeroes otherwise.
    let body = rendered.trim_end_matches('Z');
    let (seconds, fraction) = body.split_once('.').unwrap_or((body, ""));
    format!("{seconds}.{fraction:0<9}Z")
}

/// A fresh resource id: `prefix` followed by `body_len` `[0-9a-f]` characters — inside
/// the modelled `[0-9a-zA-Z]` alphabet — derived from the clock and a process-wide
/// counter, so two ids minted in the same instant still differ. Callers re-mint on the
/// vanishing chance the store already holds the result.
fn mint_id(prefix: &str, body_len: usize) -> String {
    use ::std::hash::{Hash, Hasher};

    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let sequence = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    (nanos, sequence).hash(&mut hasher);
    // 32 hex characters, so no `body_len` this service uses can index past the end.
    let mut body = format!("{:016x}{:016x}", hasher.finish(), sequence);
    // ASCII hex only, so the truncation is on a character boundary.
    body.truncate(body_len);
    format!("{prefix}{body}")
}

/// A fresh `NetworkMigrationExecutionID`: [`mint_id`]'s hex body, grouped 8-4-4-4-12 to
/// match the model's pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
fn mint_execution_id() -> String {
    let hex = mint_id("", 32);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The modelled refusal every operation but `InitializeService` answers until that
/// operation has run for the account.
///
/// The message names no request value: this refusal is about the account the request
/// is signed for, and the service answers one fixed sentence for it
/// (`Account not initialized`) rather than echoing the caller's `accountID` back. One
/// text for one condition, so a client matching the refusal sees the same message
/// whichever operation it called.
pub fn uninitialized_account() -> UninitializedAccountException {
    UninitializedAccountException {
        message: Some(UNINITIALIZED_ACCOUNT_MESSAGE.to_string()),
        code: None,
    }
}

/// The message [`uninitialized_account`] carries, as Application Migration Service
/// answers it for an account `InitializeService` has not completed for.
const UNINITIALIZED_ACCOUNT_MESSAGE: &str = "Account not initialized";

/// The modelled refusal for an `applicationID` the account holds no record of.
pub fn application_not_found(application_id: &str) -> ResourceNotFoundException {
    resource_not_found(APPLICATION_RESOURCE_TYPE, application_id)
}

/// The modelled `ResourceNotFoundException` for an id of `resource_type` that the
/// account holds no record of — the ONE place this service words a missing resource an
/// operation named by its TYPED ID (an `applicationID`, a
/// `networkMigrationDefinitionID`). An operation that names a resource by ARN instead
/// knows no type for a value that resolves to nothing, and words its refusal in
/// [`tagged_resource_not_found`].
///
/// `resourceType` names the resource KIND the way the API spells it: the modelled
/// resource shape's own name (`Application`, `NetworkMigrationDefinition`), which is
/// what a client branching on the member compares against. The ARN path segment
/// (`application/…`, `network-migration-definition/…`) is a different spelling of the
/// same kind and belongs in an ARN, not here. The message is composed from that type
/// name and the id the request asked for, so it names what was missing and which one.
fn resource_not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{resource_type} with Id: '{resource_id}' was not found"
        )),
        code: None,
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The `resourceType` an application refusal names: the modelled resource shape
/// `com.amazonaws.mgn#ApplicationResource` without the `Resource` suffix, as the API
/// spells the kind.
const APPLICATION_RESOURCE_TYPE: &str = "Application";

/// The modelled refusal for a `name` another of the account's applications already
/// carries: within one account an application name identifies the application a caller
/// asked for, so a second one under the same name conflicts with it.
pub fn application_name_in_use(name: &str, application_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Application {application_id} already uses the name {name}."
        )),
        code: None,
        resource_id: Some(application_id.to_string()),
        resource_type: Some(APPLICATION_RESOURCE_TYPE.to_string()),
        errors: None,
    }
}

/// The modelled refusal for an edit to an archived application: the request conflicts
/// with the resource's current state.
pub fn application_archived(application_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Application {application_id} is archived and cannot be modified."
        )),
        code: None,
        resource_id: Some(application_id.to_string()),
        resource_type: Some(APPLICATION_RESOURCE_TYPE.to_string()),
        errors: None,
    }
}

/// The modelled refusal when the emulator's own [`ACCOUNT_QUOTA`] is reached: the
/// request cannot be served because this store tracks no further accounts.
pub fn account_quota_exceeded() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "This emulator tracks at most {ACCOUNT_QUOTA} initialized accounts."
        )),
        code: None,
        reason: None,
        field_list: None,
    }
}

/// The modelled refusal for an account already holding [`APPLICATION_QUOTA`]
/// applications.
pub fn application_quota_exceeded(account_id: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Account {account_id} already holds the maximum of {APPLICATION_QUOTA} applications."
        )),
        code: None,
        resource_id: None,
        resource_type: Some(APPLICATION_RESOURCE_TYPE.to_string()),
        service_code: Some(ARN_NAMESPACE.to_string()),
        quota_code: None,
        quota_value: i32::try_from(APPLICATION_QUOTA).ok(),
    }
}

/// The modelled refusal for a `networkMigrationDefinitionID` the account holds no
/// record of.
pub fn network_migration_definition_not_found(definition_id: &str) -> ResourceNotFoundException {
    resource_not_found(NETWORK_MIGRATION_DEFINITION_RESOURCE_TYPE, definition_id)
}

/// The modelled refusal for a `resourceArn` naming no resource the account holds — the
/// ONE wording the tag operations state a missing resource with.
///
/// The request named an ARN and no typed id, and an ARN that resolves to nothing names
/// no resource KIND either, so the sentence names neither: the service answers one
/// fixed text for the condition ([`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`]) and reports the
/// value the caller submitted in `resourceId`, which is the only thing known about what
/// was missing.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        code: None,
        resource_id: Some(resource_arn.to_string()),
        resource_type: None,
    }
}

/// The message [`tagged_resource_not_found`] carries, as Application Migration Service
/// words a not-found refusal of a resource addressed by ARN.
const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The most tags one resource carries, as `TagResource` documents it ("Each resource
/// can have a maximum of 50 tags") and the model's `TagsMap` bounds it (`@length`
/// 0..50). One request's map is held to that bound at the boundary; the MERGED set a
/// write would leave on the resource is held to it by the handler.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The modelled refusal for a tag write that would leave `resulting` tags on one
/// resource, past [`MAX_TAGS_PER_RESOURCE`].
///
/// A field validation, named after the member the pairs arrived in, the way this
/// service's other field refusals are.
pub fn resource_tag_limit_exceeded(resulting: usize) -> ValidationException {
    let message = format!(
        "A resource can have a maximum of {MAX_TAGS_PER_RESOURCE} tags, and these tags \
         would give it {resulting}."
    );
    ValidationException {
        message: Some(message.clone()),
        code: None,
        reason: Some(VALIDATION_FIELD_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: Some("tags".to_string()),
            message: Some(message),
        }]),
    }
}

/// The modelled refusal for an account already holding
/// [`NETWORK_MIGRATION_DEFINITION_QUOTA`] network migration definitions.
pub fn network_migration_definition_quota_exceeded(
    account_id: &str,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Account {account_id} already holds the maximum of \
             {NETWORK_MIGRATION_DEFINITION_QUOTA} network migration definitions."
        )),
        code: None,
        resource_id: None,
        resource_type: Some(NETWORK_MIGRATION_DEFINITION_RESOURCE_TYPE.to_string()),
        service_code: Some(ARN_NAMESPACE.to_string()),
        quota_code: None,
        quota_value: i32::try_from(NETWORK_MIGRATION_DEFINITION_QUOTA).ok(),
    }
}

/// The `resourceType` a network-migration-definition refusal names: the modelled
/// resource shape `com.amazonaws.mgn#NetworkMigrationDefinitionResource` without the
/// `Resource` suffix, as the API spells the kind.
const NETWORK_MIGRATION_DEFINITION_RESOURCE_TYPE: &str = "NetworkMigrationDefinition";

/// The modelled refusal for an account already holding
/// [`NETWORK_MIGRATION_EXECUTION_QUOTA`] network migration executions, so no further
/// definition can be staged one.
pub fn network_migration_execution_quota_exceeded(
    account_id: &str,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Account {account_id} already holds the maximum of \
             {NETWORK_MIGRATION_EXECUTION_QUOTA} network migration executions."
        )),
        code: None,
        resource_id: None,
        resource_type: Some(NETWORK_MIGRATION_DEFINITION_RESOURCE_TYPE.to_string()),
        service_code: Some(ARN_NAMESPACE.to_string()),
        quota_code: None,
        quota_value: i32::try_from(NETWORK_MIGRATION_EXECUTION_QUOTA).ok(),
    }
}

/// The modelled refusal for `cidrMappings` on a definition that is not migrating into
/// VPCs that already exist.
///
/// The model documents the constraint on the member itself — "CIDR mappings can be
/// provided only when `vpcProvisioningStrategy` is set to `USE_EXISTING`" — and cannot
/// express it, so it is refused here: a field validation, named after the field it is
/// about.
pub fn cidr_mappings_need_existing_vpcs() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "cidrMappings can be provided only when vpcProvisioningStrategy is \
             {VPC_PROVISIONING_USE_EXISTING}."
        )),
        code: None,
        reason: Some(VALIDATION_FIELD_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: Some("cidrMappings".to_string()),
            message: Some(format!(
                "cidrMappings is allowed only with vpcProvisioningStrategy \
                 {VPC_PROVISIONING_USE_EXISTING}."
            )),
        }]),
    }
}

/// The `ValidationExceptionReason` for a request a member of which is not acceptable.
///
/// Taken from the model's own generated value list rather than retyped, so the spelling
/// cannot drift from the model: [`validation_exception_reason`] fails the build if the
/// model stops declaring it. The ONE definition of this value for the service — every
/// handler that words a field validation reaches it here.
pub const VALIDATION_FIELD_FAILED: &str = validation_exception_reason("fieldValidationFailed");

/// The `ValidationExceptionReason` value the model spells `wanted`, resolved at compile
/// time against `ops::types::VALIDATION_EXCEPTION_REASON_VALUES` (generated from the
/// Smithy model). A name the model does not declare is a compile error, not a wire value
/// no client would recognise.
const fn validation_exception_reason(wanted: &str) -> &str {
    let declared = crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
    let mut index = 0;
    while index < declared.len() {
        if same_ascii(declared[index], wanted) {
            return declared[index];
        }
        index += 1;
    }
    panic!("the model declares no such ValidationExceptionReason")
}

/// Whether two strings are the same bytes — `==` is not available in a `const fn`.
const fn same_ascii(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// The modelled server fault for an engine this service has no store in — the only
/// way [`service_state`] answers `None`. Never answered with a fresh engine: that
/// would hand the request a private store dropped when it returns.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the Application Migration Service store is unavailable".to_string(),
        retry_after_seconds: 0,
    }
}

/// The audit record a state-changing operation owes: which operation ran, on whose
/// account and resource, for which presented access key id, and how it ended.
///
/// Records only identifiers: the presented AKID names the caller the way CloudTrail
/// does, and no credential, header or payload member is logged.
pub fn audit(
    parts: &http::request::Parts,
    operation: &str,
    account_id: &str,
    resource: &str,
    outcome: &str,
) {
    ::tracing::info!(
        target: "emulator::audit",
        service = ARN_NAMESPACE,
        operation,
        account_id,
        resource,
        outcome,
        akid = %::emulator_server::extract_akid(&parts.headers),
        "state-changing operation"
    );
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMgnApplicationmigrationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMgnApplicationmigrationserviceState>)
}

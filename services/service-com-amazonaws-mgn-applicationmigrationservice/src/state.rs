// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mgn-applicationmigrationservice service.
//!
//! mgn is account-scoped: `InitializeService` initializes the account a request acts
//! on, and every other operation refuses with `UninitializedAccountException` (HTTP
//! 400) until it has. That flag and the application records live in ONE store here, so
//! every handler reads and writes the same state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ops::types::{
    ApplicationAggregatedStatus, ConflictException, CreateApplicationOutput, DataReplicationInfo,
    InternalServerException, LaunchedInstance, LifeCycle, ResourceNotFoundException,
    ServiceQuotaExceededException, SourceProperties, SourceServerConnectorAction,
    UninitializedAccountException, UpdateSourceServerOutput, ValidationException,
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
    ResourceNotFoundException {
        message: Some(format!("Application {application_id} does not exist.")),
        code: None,
        resource_id: Some(application_id.to_string()),
        resource_type: Some("application".to_string()),
    }
}

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
        resource_type: Some("application".to_string()),
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
        resource_type: Some("application".to_string()),
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
        resource_type: Some("application".to_string()),
        service_code: Some(ARN_NAMESPACE.to_string()),
        quota_code: None,
        quota_value: i32::try_from(APPLICATION_QUOTA).ok(),
    }
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

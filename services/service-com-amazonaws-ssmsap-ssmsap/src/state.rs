// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmsap-ssmsap service.
//!
//! The registered SAP application is this service's primary resource: every
//! application-scoped operation (`RegisterApplication`, `GetApplication`,
//! `ListApplications`, `StartApplication`, `StopApplication`,
//! `DeregisterApplication`) resolves the SAME [`StoredApplication`] out of
//! [`ComAmazonawsSsmsapSsmsapState::applications`], and every call that changes one
//! files a [`StoredOperation`] in [`ComAmazonawsSsmsapSsmsapState::operations`] that
//! `GetOperation` reads back. So a register is observable by every later read, a
//! deregister by every later call, and an `OperationId` handed to a caller always
//! names a record that exists.
//!
//! The record is the stored half; the wire half (`Application`, `ApplicationSummary`,
//! `Operation`) is PROJECTED from it by [`StoredApplication::view`] /
//! [`StoredApplication::summary`] / [`StoredOperation::view`], so no handler assembles
//! a response member by hand.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Application, ApplicationCredential, ApplicationSummary, ConflictException, Filter,
    InternalServerException, Operation, ResourceNotFoundException, ValidationException,
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

/// How many applications this emulator will hold.
///
/// The real service caps registered applications per account per Region with a service
/// quota. This number is NOT a fidelity claim about the published quota: it is the
/// emulator's own ceiling, generous enough that no realistic workflow meets it, so a
/// caller looping on `RegisterApplication` cannot grow this process's memory without
/// bound. It is reported with `ConflictException`, which `RegisterApplication` models.
pub const MAX_APPLICATIONS: usize = 1_000;

/// How many operation records are retained. Beyond this the oldest record that has
/// already reached a terminal status is dropped first, so a caller polling a fresh
/// operation can still resolve it.
pub const MAX_RETAINED_OPERATIONS: usize = 10_000;

/// The page size `ListApplications` uses when the caller names no `MaxResults`, and
/// the ceiling it clamps a requested page to: the model's own `MaxResults` range is
/// 1..50, so 50 is both the largest page the API allows and the default page.
pub const MAX_PAGE_SIZE: i32 = 50;

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
}

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

/// Per-service state for com-amazonaws-ssmsap-ssmsap. A handler reaches it through [`service_state`].
///
/// `applications` and `operations` are `BTreeMap`s rather than `HashMap`s because
/// `ListApplications` pages over the first: iteration order has to be the same for the
/// call that hands out a `NextToken` and the call that resumes from it, and id order is
/// that stable order.
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmsapSsmsapState {
    /// Every registered application, keyed by `ApplicationId`.
    pub applications: BTreeMap<String, StoredApplication>,
    /// Every operation record, keyed by `OperationId`. Bounded by
    /// [`MAX_RETAINED_OPERATIONS`].
    pub operations: BTreeMap<String, StoredOperation>,
    /// Tags by resource ARN — written by `RegisterApplication`'s `Tags` and read by
    /// `GetApplication`/`ListApplications`, so the later tagging APIs
    /// (`TagResource`/`UntagResource`/`ListTagsForResource`) have one place to join.
    pub tags_by_arn: HashMap<String, HashMap<String, String>>,
}

impl ComAmazonawsSsmsapSsmsapState {
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
    }

    /// Resolve the application a request names by its id, its ARN, or its AppRegistry
    /// ARN — the three members `GetApplication` accepts — to a stored `ApplicationId`.
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
        self.evict_oldest_operations();
        id
    }

    /// Drop every operation record filed against `application_id` — used when the
    /// application is deregistered, since nothing else would ever reclaim them.
    pub fn forget_operations_for(&mut self, application_id: &str) {
        self.operations
            .retain(|_, record| record.resource_id != application_id);
    }

    /// Keep [`Self::operations`] within [`MAX_RETAINED_OPERATIONS`], dropping the
    /// oldest record that has already reached a terminal status first so a caller
    /// polling a fresh operation can still resolve it.
    fn evict_oldest_operations(&mut self) {
        while self.operations.len() > MAX_RETAINED_OPERATIONS {
            let victim = self
                .operations
                .values()
                .min_by_key(|record| {
                    (
                        record.status == OPERATION_STATUS_INPROGRESS,
                        record.start_time,
                    )
                })
                .map(|record| record.id.clone());
            match victim {
                Some(id) => {
                    self.operations.remove(&id);
                }
                None => break,
            }
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

/// The modelled `ValidationException` a lookup is refused with when no registered
/// application answers the reference it named.
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
    let Some(actual) = application.filter_attribute(&filter.name) else {
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

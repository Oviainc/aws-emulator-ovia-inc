// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda service.
//!
//! The collaboration/membership lifecycle is persisted here and every response is
//! computed from it. The two stored records are deliberately split:
//!
//! * [`CollaborationRecord`] holds the collaboration as the CREATOR declared it —
//!   including the creator's own abilities and the immutable `members` list. The
//!   modelled `Collaboration` shape is NOT stored, because two of its members
//!   (`memberStatus`, `membershipId`/`membershipArn`) describe the CALLER's relation
//!   to the collaboration rather than the collaboration itself, so they are derived
//!   per request by [`CleanRoomsState::collaboration_view`].
//! * the modelled [`Membership`] shape IS stored as-is: every one of its members is
//!   absolute, so there is nothing to derive.
//!
//! * the modelled [`ConfiguredTable`] is likewise stored as-is: a configured table is an
//!   account-level resource, so none of its members describes the caller.
//!
//! Insertion order is kept in `collaboration_order` / `membership_order` /
//! `configured_table_order` so the list operations page deterministically and a
//! pagination token stays valid across calls.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Collaboration, CollaborationSummary, ComputeConfiguration, ConfiguredTable,
    DataEncryptionMetadata, MLMemberAbilities, MemberSpecification, Membership,
    MembershipJobComputePaymentConfig, MembershipMLPaymentConfig,
    MembershipModelInferencePaymentConfig, MembershipModelTrainingPaymentConfig,
    MembershipPaymentConfiguration, MembershipProtectedJobResultConfiguration,
    MembershipProtectedQueryOutputConfiguration, MembershipProtectedQueryResultConfiguration,
    MembershipQueryComputePaymentConfig, MembershipSummary,
    MembershipSyntheticDataGenerationPaymentConfig, PaymentConfiguration, ProtectedQuery,
    ProtectedQueryError, ProtectedQueryOutputConfiguration, ProtectedQueryResultConfiguration,
    TableReference, WorkerComputeConfiguration,
};

/// The ARN service segment and the SigV4 signing name for Clean Rooms, from
/// `OPERATIONS.md` (`sigv4 name cleanrooms`) and the `CollaborationArn` /
/// `MembershipArn` patterns in the model.
pub const ARN_SERVICE: &str = "cleanrooms";

/// The region used when the request carries no usable one. The model's
/// `CollaborationArn`/`MembershipArn` patterns pin the ARN's region segment to
/// `[\w]{2}-[\w]{4,9}-[\d]`, so a scope region that cannot appear in one of those ARNs
/// is replaced by this emulator default rather than emitted.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `MemberStatus` values this slice produces (model shape `MemberStatus`:
/// INVITED | ACTIVE | LEFT | REMOVED).
pub const MEMBER_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`MEMBER_STATUS_ACTIVE`].
pub const MEMBER_STATUS_INVITED: &str = "INVITED";

/// The `isMetricsEnabled` a create stores when the request omits it.
///
/// Documented on both `CreateCollaborationInput$isMetricsEnabled` and
/// `CreateMembershipInput$isMetricsEnabled`: "The default value is `false`." The
/// service, not the caller, decides it, so the created resource reports it rather than
/// leaving the member absent.
pub const DEFAULT_IS_METRICS_ENABLED: bool = false;

/// The `analyticsEngine` a collaboration created without one runs on.
///
/// The model's `AnalyticsEngine` enum declares `SPARK | CLEAN_ROOMS_SQL`, and
/// `CreateCollaborationInput$analyticsEngine` notes "After July 16, 2025, the
/// `CLEAN_ROOMS_SQL` parameter will no longer be available" — so `SPARK` is the engine
/// a collaboration gets when the caller selects none.
pub const DEFAULT_ANALYTICS_ENGINE: &str = "SPARK";

/// Whether a membership accepts a machine-learning cost the collaboration creator
/// assigned to nobody.
///
/// `QueryComputePaymentConfig$isResponsible` makes "the member who can query" the
/// default payer for query compute, and the same rule gives job compute to the member
/// with `CAN_RUN_JOB`; no ability in the `MemberAbility` catalog (CAN_QUERY |
/// CAN_RECEIVE_RESULTS | CAN_RUN_JOB | CAN_EXPORT_QUERY_ANALYSIS_LOG) nor in the
/// `CustomMLMemberAbility` catalog (CAN_RECEIVE_MODEL_OUTPUT |
/// CAN_RECEIVE_INFERENCE_OUTPUT) runs model training, model inference or synthetic data
/// generation, so nothing makes a member the default payer for an ML cost: an ML cost
/// the creator assigned nobody is accepted as not-responsible.
pub const DEFAULT_ML_IS_RESPONSIBLE: bool = false;

/// `ProtectedQueryStatus` values this slice produces (model shape
/// `ProtectedQueryStatus`: SUBMITTED | STARTED | CANCELLED | CANCELLING | FAILED |
/// SUCCESS | TIMED_OUT). A started query is SUBMITTED until the clock settles it, and
/// FAILED is the only terminal status this emulator can reach — see
/// [`PROTECTED_QUERY_FAILURE_CODE`].
pub const PROTECTED_QUERY_STATUS_SUBMITTED: &str = "SUBMITTED";
/// See [`PROTECTED_QUERY_STATUS_SUBMITTED`].
pub const PROTECTED_QUERY_STATUS_FAILED: &str = "FAILED";

/// How long after `StartProtectedQuery` the clock owes the query its terminal status.
///
/// AWS publishes no duration for a protected query's run, so this span is CHOSEN: 500
/// ms, inside this project's 250-750 ms band for a transition the clock owes. It is
/// deliberately non-zero, so a caller that reads the query straight after starting it
/// still sees it SUBMITTED, and the due instant is stored rather than slept on.
pub const PROTECTED_QUERY_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The `ProtectedQueryError.code` a settled query carries.
///
/// Clean Rooms executes a protected query against the configured tables a
/// collaboration associates; this emulator serves the Clean Rooms CONTROL plane only
/// and has no analytics data plane to run SQL on, so no protected query can be
/// executed. The run is therefore not performed and the query ends in the modelled
/// failure state ([`PROTECTED_QUERY_STATUS_FAILED`]) with an error that says exactly
/// that — never in a fabricated SUCCESS with results nothing computed.
pub const PROTECTED_QUERY_FAILURE_CODE: &str = "QUERY_EXECUTION_UNAVAILABLE";

/// The `ProtectedQueryError.message` that goes with [`PROTECTED_QUERY_FAILURE_CODE`].
pub const PROTECTED_QUERY_FAILURE_MESSAGE: &str =
    "this emulator serves the Clean Rooms control plane only: it has no analytics data \
     plane, so the protected query was accepted and recorded but never executed";

/// `MembershipStatus` values this slice produces (model shape `MembershipStatus`:
/// ACTIVE | REMOVED | COLLABORATION_DELETED).
pub const MEMBERSHIP_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`MEMBERSHIP_STATUS_ACTIVE`].
pub const MEMBERSHIP_STATUS_COLLABORATION_DELETED: &str = "COLLABORATION_DELETED";

/// `MemberAbility` members the payment defaults below key on (model shape
/// `MemberAbility`: CAN_QUERY | CAN_RECEIVE_RESULTS | CAN_RUN_JOB |
/// CAN_EXPORT_QUERY_ANALYSIS_LOG).
pub const ABILITY_CAN_QUERY: &str = "CAN_QUERY";
/// See [`ABILITY_CAN_QUERY`].
pub const ABILITY_CAN_RUN_JOB: &str = "CAN_RUN_JOB";

/// `ResourceType` values this slice reports on a modelled error (model shape
/// `ResourceType`: CONFIGURED_TABLE | COLLABORATION | MEMBERSHIP |
/// CONFIGURED_TABLE_ASSOCIATION).
pub const RESOURCE_TYPE_COLLABORATION: &str = "COLLABORATION";
/// See [`RESOURCE_TYPE_COLLABORATION`].
pub const RESOURCE_TYPE_MEMBERSHIP: &str = "MEMBERSHIP";
/// See [`RESOURCE_TYPE_COLLABORATION`].
pub const RESOURCE_TYPE_CONFIGURED_TABLE: &str = "CONFIGURED_TABLE";

/// `TableReferenceType` values (model shape `TableReferenceType`: GLUE | SNOWFLAKE |
/// ATHENA | DATABRICKS) — the `tableType` a configured table reports, one per variant
/// of the `TableReference` union.
pub const TABLE_TYPE_GLUE: &str = "GLUE";
/// See [`TABLE_TYPE_GLUE`].
pub const TABLE_TYPE_SNOWFLAKE: &str = "SNOWFLAKE";
/// See [`TABLE_TYPE_GLUE`].
pub const TABLE_TYPE_ATHENA: &str = "ATHENA";
/// See [`TABLE_TYPE_GLUE`].
pub const TABLE_TYPE_DATABRICKS: &str = "DATABRICKS";

/// The smallest `selectedAnalysisMethods` a request may configure.
///
/// Documented on `CreateConfiguredTableInput$selectedAnalysisMethods` and
/// `UpdateConfiguredTableInput$selectedAnalysisMethods`: "The analysis methods to enable
/// for the configured table. When configured, you must specify at least two analysis
/// methods." The model cannot express it (`SelectedAnalysisMethods` carries no length
/// constraint), so the handler enforces it with the `ValidationException` the operation
/// declares.
pub const MIN_SELECTED_ANALYSIS_METHODS: usize = 2;

/// `ConflictExceptionReason` for a resource that already exists (model shape
/// `ConflictExceptionReason`: ALREADY_EXISTS | SUBRESOURCES_EXIST | INVALID_STATE).
pub const CONFLICT_REASON_ALREADY_EXISTS: &str = "ALREADY_EXISTS";

/// The only `AccessDeniedExceptionReason` the model declares (model shape
/// `AccessDeniedExceptionReason`: INSUFFICIENT_PERMISSIONS).
pub const ACCESS_DENIED_REASON_INSUFFICIENT_PERMISSIONS: &str = "INSUFFICIENT_PERMISSIONS";

/// `ValidationExceptionReason` for a member value that failed validation (model shape
/// `ValidationExceptionReason`).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `ValidationExceptionReason` for SQL parameters that name no query to run (model
/// shape `ValidationExceptionReason`: FIELD_VALIDATION_FAILED | INVALID_CONFIGURATION |
/// INVALID_QUERY | IAM_SYNCHRONIZATION_DELAY).
pub const VALIDATION_REASON_INVALID_QUERY: &str = "INVALID_QUERY";

/// The largest `members` list `CreateCollaboration` accepts.
///
/// The model declares `MemberList` as `len 0..` — no upper bound — so without a cap
/// the only limit on the list one request may carry is the transport body size, and
/// the per-request work the handler does would be inherited from that rather than
/// bounded by the service. This is an EMULATOR-ENFORCED resource bound, not a
/// documented AWS quota: it is set far above any realistic collaboration so it refuses
/// only requests that exist to make the handler do unbounded work, and it is reported
/// through the `ServiceQuotaExceededException` (402) `CreateCollaboration` declares.
pub const MAX_COLLABORATION_MEMBERS: usize = 1000;

/// The largest number of collaborations, and of memberships, the store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// a create is the only thing that adds one, so each store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because
/// `GetCollaboration` and `GetMembership` must still answer what their creates
/// created. The two operations that grow these stores refuse past the caps with the
/// `ServiceQuotaExceededException` (HTTP 402) the model declares on exactly those two
/// operations and on no read. The numbers are the EMULATOR's own bound, not a
/// published AWS default — AWS publishes its Clean Rooms quotas as adjustable, so no
/// single value is the contract — and they are set high enough that no ordinary
/// workflow meets one.
pub const MAX_COLLABORATIONS: usize = 5_000;

/// See [`MAX_COLLABORATIONS`].
pub const MAX_MEMBERSHIPS: usize = 5_000;

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the configured-table
/// store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreateConfiguredTable` declares.
pub const MAX_CONFIGURED_TABLES: usize = 5_000;

/// The largest number of protected queries the store holds, for the same reason as
/// [`MAX_COLLABORATIONS`]: `StartProtectedQuery` is the only operation that adds one,
/// every one it adds lives for the life of the process so `GetProtectedQuery` can still
/// answer for it, and past the cap the service answers the
/// `ServiceQuotaExceededException` (HTTP 402) `StartProtectedQuery` declares. The
/// number is the EMULATOR's own bound, not a published AWS default.
pub const MAX_PROTECTED_QUERIES: usize = 5_000;

/// A collaboration exactly as its creator declared it.
#[derive(Debug, Clone)]
pub struct CollaborationRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub creator_account_id: String,
    pub creator_display_name: String,
    pub creator_member_abilities: Vec<String>,
    pub creator_ml_member_abilities: Option<MLMemberAbilities>,
    pub creator_payment_configuration: Option<PaymentConfiguration>,
    pub create_time: DateTime,
    pub update_time: DateTime,
    pub data_encryption_metadata: Option<DataEncryptionMetadata>,
    pub query_log_status: String,
    pub job_log_status: Option<String>,
    /// The engine in force — the one the caller selected, else
    /// [`DEFAULT_ANALYTICS_ENGINE`]. Not optional: the service always has one.
    pub analytics_engine: String,
    pub auto_approved_change_types: Option<Vec<String>>,
    pub allowed_result_regions: Option<Vec<String>>,
    /// The metrics setting in force — the one the caller sent, else
    /// [`DEFAULT_IS_METRICS_ENABLED`]. Not optional, for the same reason.
    pub is_metrics_enabled: bool,
    /// The initial members, not including the creator. Immutable, per the
    /// `CreateCollaboration` `members` doc.
    pub members: Vec<MemberSpecification>,
}

/// What one account was granted in a collaboration: its abilities and the payment
/// responsibilities the creator set for it.
#[derive(Debug, Clone)]
pub struct MemberGrant {
    pub display_name: String,
    pub member_abilities: Vec<String>,
    pub ml_member_abilities: Option<MLMemberAbilities>,
    pub payment_configuration: Option<PaymentConfiguration>,
}

impl CollaborationRecord {
    /// The grant `account_id` holds in this collaboration, or `None` when that account
    /// is neither the creator nor one of the declared members — such an account is not
    /// part of the collaboration at all, so it can neither see nor join it.
    pub fn grant_for(&self, account_id: &str) -> Option<MemberGrant> {
        if account_id == self.creator_account_id {
            return Some(MemberGrant {
                display_name: self.creator_display_name.clone(),
                member_abilities: self.creator_member_abilities.clone(),
                ml_member_abilities: self.creator_ml_member_abilities.clone(),
                payment_configuration: self.creator_payment_configuration.clone(),
            });
        }
        self.members
            .iter()
            .find(|m| m.account_id == account_id)
            .map(|m| MemberGrant {
                display_name: m.display_name.clone(),
                member_abilities: m.member_abilities.clone(),
                ml_member_abilities: m.ml_member_abilities.clone(),
                payment_configuration: m.payment_configuration.clone(),
            })
    }
}

/// A protected query as `StartProtectedQuery` recorded it, plus the terminal status the
/// clock owes it.
#[derive(Debug, Clone)]
pub struct ProtectedQueryRecord {
    /// The modelled shape, stored as-is: every one of its members is absolute, so a
    /// read reports it without deriving anything.
    pub query: ProtectedQuery,
    /// When the clock owes this query its terminal status, or `None` once that
    /// transition has been materialized. See [`PROTECTED_QUERY_SETTLE_DELAY`].
    pub terminal_due: Option<SystemTime>,
}

/// Per-service state for com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState {
    /// Collaborations by `Collaboration.id`.
    pub collaborations: HashMap<String, CollaborationRecord>,
    /// Collaboration ids in creation order — the order the list operation pages in.
    pub collaboration_order: Vec<String>,
    /// Memberships by `Membership.id`.
    pub memberships: HashMap<String, Membership>,
    /// Membership ids in creation order — the order the list operation pages in.
    pub membership_order: Vec<String>,
    /// `(collaboration id, account id) -> membership id`: the index that makes the
    /// duplicate-membership conflict and a `Collaboration`'s `membershipId` computable
    /// instead of guessed.
    pub membership_by_collaboration: HashMap<(String, String), String>,
    /// Configured tables by `ConfiguredTable.id`.
    ///
    /// The modelled `ConfiguredTable` is stored as-is: every one of its members is
    /// absolute (nothing about it depends on who is asking), so there is nothing to
    /// derive per request. `analysisRuleTypes` is the one member another operation
    /// maintains — a configured table is created with no analysis rules, and the
    /// analysis-rule operations add to and remove from this list.
    pub configured_tables: HashMap<String, ConfiguredTable>,
    /// Configured table ids in creation order — the order the list operation pages in.
    pub configured_table_order: Vec<String>,
    /// Tags by resource ARN, shared by collaboration and membership ARNs so a later
    /// TagResource/ListTagsForResource slice has one place to read and write.
    pub tags: HashMap<String, HashMap<String, String>>,
    /// Protected queries by `ProtectedQuery.id`.
    pub protected_queries: HashMap<String, ProtectedQueryRecord>,
    /// Protected query ids in start order — the order a listing pages in.
    pub protected_query_order: Vec<String>,
}

/// Shorthand for this service's state struct.
pub type CleanRoomsState = ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState;

impl ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState {
    /// The membership `account_id` holds in `collaboration_id`, if any.
    pub fn membership_of(&self, collaboration_id: &str, account_id: &str) -> Option<&Membership> {
        let id = self
            .membership_by_collaboration
            .get(&(collaboration_id.to_string(), account_id.to_string()))?;
        self.memberships.get(id)
    }

    /// The caller's status as a member of `record`: ACTIVE once it holds a membership in
    /// the collaboration, INVITED while it is part of the collaboration — as the creator
    /// or as a declared member — without one. `None` when the caller is not part of the
    /// collaboration at all.
    ///
    /// The creator is no exception: `CreateMembership` is the documented way to join a
    /// collaboration, and a collaboration's creator holds no membership until it calls
    /// it, so it is INVITED to the collaboration it just created and becomes ACTIVE only
    /// once that membership exists. A client that polls this member to learn whether it
    /// still has to join therefore sees the join it owes.
    pub fn member_status_for(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<&'static str> {
        let part_of_collaboration = account_id == record.creator_account_id
            || record.members.iter().any(|m| m.account_id == account_id);
        if !part_of_collaboration {
            return None;
        }
        Some(if self.membership_of(&record.id, account_id).is_some() {
            MEMBER_STATUS_ACTIVE
        } else {
            MEMBER_STATUS_INVITED
        })
    }

    /// The modelled `Collaboration` as `account_id` sees it, or `None` when that
    /// account is not part of the collaboration.
    pub fn collaboration_view(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<Collaboration> {
        let member_status = self.member_status_for(record, account_id)?;
        let membership = self.membership_of(&record.id, account_id);
        Some(Collaboration {
            id: record.id.clone(),
            arn: record.arn.clone(),
            name: record.name.clone(),
            description: record.description.clone(),
            creator_account_id: record.creator_account_id.clone(),
            creator_display_name: record.creator_display_name.clone(),
            create_time: record.create_time,
            update_time: record.update_time,
            member_status: member_status.to_string(),
            membership_id: membership.map(|m| m.id.clone()),
            membership_arn: membership.map(|m| m.arn.clone()),
            data_encryption_metadata: record.data_encryption_metadata.clone(),
            query_log_status: record.query_log_status.clone(),
            job_log_status: record.job_log_status.clone(),
            analytics_engine: Some(record.analytics_engine.clone()),
            auto_approved_change_types: record.auto_approved_change_types.clone(),
            allowed_result_regions: record.allowed_result_regions.clone(),
            is_metrics_enabled: Some(record.is_metrics_enabled),
        })
    }

    /// The modelled `Collaboration` for a stored id, as `account_id` sees it.
    pub fn collaboration_view_by_id(
        &self,
        collaboration_id: &str,
        account_id: &str,
    ) -> Option<Collaboration> {
        let record = self.collaborations.get(collaboration_id)?;
        self.collaboration_view(record, account_id)
    }

    /// The modelled `CollaborationSummary` as `account_id` sees it — the same derived
    /// members as [`Self::collaboration_view`], projected onto the summary shape.
    pub fn collaboration_summary(
        &self,
        record: &CollaborationRecord,
        account_id: &str,
    ) -> Option<CollaborationSummary> {
        let member_status = self.member_status_for(record, account_id)?;
        let membership = self.membership_of(&record.id, account_id);
        Some(CollaborationSummary {
            id: record.id.clone(),
            arn: record.arn.clone(),
            name: record.name.clone(),
            creator_account_id: record.creator_account_id.clone(),
            creator_display_name: record.creator_display_name.clone(),
            create_time: record.create_time,
            update_time: record.update_time,
            member_status: member_status.to_string(),
            membership_id: membership.map(|m| m.id.clone()),
            membership_arn: membership.map(|m| m.arn.clone()),
            analytics_engine: Some(record.analytics_engine.clone()),
        })
    }

    /// Materialize every protected-query transition the CLOCK already owes at `now`.
    ///
    /// A query's terminal status falls due [`PROTECTED_QUERY_SETTLE_DELAY`] after the
    /// call that started it, whether or not anybody calls again, so a reporting
    /// operation applies the ones already owed before it answers. It changes nothing
    /// that is not yet due: a caller reading a query inside that span sees it
    /// SUBMITTED.
    pub fn settle_protected_queries(&mut self, now: SystemTime) {
        for record in self.protected_queries.values_mut() {
            if record.terminal_due.is_none_or(|due| due > now) {
                continue;
            }
            record.terminal_due = None;
            record.query.status = PROTECTED_QUERY_STATUS_FAILED.to_string();
            // The run was never performed, so the query carries the failure's reason
            // and NO statistics or result: there is no duration to report for work
            // nothing did.
            record.query.error = Some(ProtectedQueryError {
                code: PROTECTED_QUERY_FAILURE_CODE.to_string(),
                message: PROTECTED_QUERY_FAILURE_MESSAGE.to_string(),
            });
        }
    }

    /// The protected query `query_id`, but only when the membership that started it is
    /// `membership_id`: a query is addressed under its own membership, so one
    /// membership never reads another's query.
    pub fn protected_query(
        &self,
        membership_id: &str,
        query_id: &str,
    ) -> Option<&ProtectedQueryRecord> {
        self.protected_queries
            .get(query_id)
            .filter(|record| record.query.membership_id == membership_id)
    }
}

/// The modelled `MembershipSummary` projection of a stored `Membership`.
pub fn membership_summary(membership: &Membership) -> MembershipSummary {
    MembershipSummary {
        id: membership.id.clone(),
        arn: membership.arn.clone(),
        collaboration_arn: membership.collaboration_arn.clone(),
        collaboration_id: membership.collaboration_id.clone(),
        collaboration_creator_account_id: membership.collaboration_creator_account_id.clone(),
        collaboration_creator_display_name: membership.collaboration_creator_display_name.clone(),
        collaboration_name: membership.collaboration_name.clone(),
        create_time: membership.create_time,
        update_time: membership.update_time,
        status: membership.status.clone(),
        member_abilities: membership.member_abilities.clone(),
        ml_member_abilities: membership.ml_member_abilities.clone(),
        payment_configuration: membership.payment_configuration.clone(),
    }
}

/// The account id every emulated caller presents, from the vendored runtime's single
/// source of truth. Never a literal.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The `isMetricsEnabled` a created resource carries: what the caller asked for, else
/// the documented [`DEFAULT_IS_METRICS_ENABLED`].
pub fn effective_is_metrics_enabled(requested: Option<bool>) -> bool {
    requested.unwrap_or(DEFAULT_IS_METRICS_ENABLED)
}

/// The `analyticsEngine` a created collaboration carries: what the caller selected, else
/// the [`DEFAULT_ANALYTICS_ENGINE`].
pub fn effective_analytics_engine(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_ANALYTICS_ENGINE.to_string())
}

/// Now, as the modelled `Timestamp` type.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// A fresh resource id in the `UUID` shape's form: lowercase hex, 36 characters,
/// matching `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
pub fn new_uuid() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// `arn:aws:cleanrooms:<region>:<account>:collaboration/<id>`, the form the model's
/// `CollaborationArn` pattern declares.
pub fn collaboration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("collaboration/{id}"))
}

/// `arn:aws:cleanrooms:<region>:<account>:membership/<id>`, the form the model's
/// `MembershipArn` pattern declares.
pub fn membership_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("membership/{id}"))
}

/// `arn:aws:cleanrooms:<region>:<account>:configuredtable/<id>`, the form the model's
/// `ConfiguredTableArn` pattern declares
/// (`^arn:aws:[\w]+:[\w]{2}-[\w]{4,9}-[\d]:[\d]{12}:configuredtable/[\d\w-]+$`).
pub fn configured_table_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("configuredtable/{id}"))
}

/// The `tableType` a configured table reports: the `TableReferenceType` of the
/// `TableReference` union variant the table actually points at, so the member is
/// computed from the stored reference rather than taken from the request.
pub fn table_reference_type(reference: &TableReference) -> &'static str {
    match reference {
        TableReference::Glue(_) => TABLE_TYPE_GLUE,
        TableReference::Snowflake(_) => TABLE_TYPE_SNOWFLAKE,
        TableReference::Athena(_) => TABLE_TYPE_ATHENA,
        TableReference::Databricks(_) => TABLE_TYPE_DATABRICKS,
    }
}

/// How many DISTINCT analysis methods a `selectedAnalysisMethods` list configures.
///
/// The list is a set of enum values, so a method repeated in the request still
/// configures one method — see [`MIN_SELECTED_ANALYSIS_METHODS`].
pub fn distinct_selected_analysis_methods(methods: &[String]) -> usize {
    let mut seen: Vec<&str> = Vec::with_capacity(methods.len());
    for method in methods {
        if !seen.contains(&method.as_str()) {
            seen.push(method.as_str());
        }
    }
    seen.len()
}

/// Whether `region` can appear in the ARN region segment the model's
/// `CollaborationArn`/`MembershipArn` patterns declare: `[\w]{2}-[\w]{4,9}-[\d]`.
fn region_fits_arn_pattern(region: &str) -> bool {
    let mut parts = region.split('-');
    let (Some(a), Some(b), Some(c), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let word = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    a.len() == 2
        && word(a)
        && (4..=9).contains(&b.len())
        && word(b)
        && c.len() == 1
        && c.chars().all(|c| c.is_ascii_digit())
}

/// The region this request is signed for, used for the ARNs the response carries.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_fits_arn_pattern(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Map a collaboration-side `PaymentConfiguration` (what the creator SET for a member)
/// onto the membership-side `MembershipPaymentConfiguration` (what that member
/// ACCEPTED) — the two shapes carry the same `isResponsible` flags.
pub fn accept_payment_configuration(
    config: &PaymentConfiguration,
) -> MembershipPaymentConfiguration {
    let machine_learning = config.machine_learning.as_ref().map(|ml| {
        MembershipMLPaymentConfig {
            model_training: ml.model_training.as_ref().map(|c| {
                MembershipModelTrainingPaymentConfig {
                    is_responsible: c.is_responsible,
                }
            }),
            model_inference: ml.model_inference.as_ref().map(|c| {
                MembershipModelInferencePaymentConfig {
                    is_responsible: c.is_responsible,
                }
            }),
            synthetic_data_generation: ml.synthetic_data_generation.as_ref().map(|c| {
                MembershipSyntheticDataGenerationPaymentConfig {
                    is_responsible: c.is_responsible,
                }
            }),
        }
    });
    let job_compute = config.job_compute.as_ref().map(|c| {
        MembershipJobComputePaymentConfig {
            is_responsible: c.is_responsible,
        }
    });
    MembershipPaymentConfiguration {
        query_compute: MembershipQueryComputePaymentConfig {
            is_responsible: config.query_compute.is_responsible,
        },
        machine_learning,
        job_compute,
    }
}

/// The payment responsibilities a new membership carries.
///
/// Precedence: what the joining member accepted in the request, else what the
/// collaboration creator set for that member, else the documented default —
/// `QueryComputePaymentConfig.isResponsible`: "If the collaboration creator hasn't
/// specified anyone as the member paying for query compute costs, then the member who
/// can query is the default payer." The same rule is applied to job compute, whose
/// paying ability is `CAN_RUN_JOB`.
///
/// Every OPTIONAL sub-config of the shape is then materialized, whichever of those three
/// sources supplied the rest: a membership reports the responsibility it accepted for
/// each cost the model names, so a cost nobody was assigned is reported as
/// not-responsible instead of being left absent (see [`DEFAULT_ML_IS_RESPONSIBLE`]).
pub fn membership_payment_configuration(
    requested: Option<&MembershipPaymentConfiguration>,
    grant: &MemberGrant,
) -> MembershipPaymentConfiguration {
    let has = |ability: &str| grant.member_abilities.iter().any(|a| a == ability);
    let accepted = requested.cloned().or_else(|| {
        grant
            .payment_configuration
            .as_ref()
            .map(accept_payment_configuration)
    });
    let (query_compute, machine_learning, job_compute) = match accepted {
        Some(accepted) => (
            accepted.query_compute,
            accepted.machine_learning,
            accepted.job_compute,
        ),
        None => (
            MembershipQueryComputePaymentConfig {
                is_responsible: has(ABILITY_CAN_QUERY),
            },
            None,
            None,
        ),
    };
    MembershipPaymentConfiguration {
        query_compute,
        machine_learning: Some(materialize_ml_payment_config(machine_learning)),
        job_compute: Some(job_compute.unwrap_or(MembershipJobComputePaymentConfig {
            is_responsible: has(ABILITY_CAN_RUN_JOB),
        })),
    }
}

/// The machine-learning block a membership reports, with one accepted sub-config per ML
/// cost `MembershipMLPaymentConfig` names (`modelTraining`, `modelInference`,
/// `syntheticDataGeneration`): whatever was accepted for that cost, else
/// [`DEFAULT_ML_IS_RESPONSIBLE`].
fn materialize_ml_payment_config(
    accepted: Option<MembershipMLPaymentConfig>,
) -> MembershipMLPaymentConfig {
    let accepted = accepted.unwrap_or_default();
    MembershipMLPaymentConfig {
        model_training: Some(accepted.model_training.unwrap_or(
            MembershipModelTrainingPaymentConfig {
                is_responsible: DEFAULT_ML_IS_RESPONSIBLE,
            },
        )),
        model_inference: Some(accepted.model_inference.unwrap_or(
            MembershipModelInferencePaymentConfig {
                is_responsible: DEFAULT_ML_IS_RESPONSIBLE,
            },
        )),
        synthetic_data_generation: Some(accepted.synthetic_data_generation.unwrap_or(
            MembershipSyntheticDataGenerationPaymentConfig {
                is_responsible: DEFAULT_ML_IS_RESPONSIBLE,
            },
        )),
    }
}

/// The account that pays for a collaboration's query compute costs.
///
/// `QueryComputePaymentConfig$isResponsible` documents the rule: the creator configures
/// payer candidates ("One or more members can be configured as payer candidates for
/// query compute costs"), and "if the collaboration creator hasn't specified anyone as
/// the member paying for query compute costs, then the member who can query is the
/// default payer". So the payer is the first account the creator configured as
/// responsible for query compute, else the first account granted
/// [`ABILITY_CAN_QUERY`], in declaration order (the creator, then the `members` list)
/// — and `None` when the collaboration configured neither, which leaves the optional
/// member absent rather than guessed.
pub fn query_compute_payer(record: &CollaborationRecord) -> Option<String> {
    let accounts: Vec<String> = ::std::iter::once(record.creator_account_id.clone())
        .chain(record.members.iter().map(|m| m.account_id.clone()))
        .collect();
    let configured = accounts.iter().find(|account| {
        record.grant_for(account).is_some_and(|grant| {
            grant
                .payment_configuration
                .is_some_and(|payment| payment.query_compute.is_responsible)
        })
    });
    if let Some(account) = configured {
        return Some(account.clone());
    }
    accounts.into_iter().find(|account| {
        record.grant_for(account).is_some_and(|grant| {
            grant
                .member_abilities
                .iter()
                .any(|ability| ability == ABILITY_CAN_QUERY)
        })
    })
}

/// The `WorkerComputeConfiguration$type` a worker compute configuration runs at when
/// the caller names none: the model declares `default: "CR.1X"` on that member (an
/// `addedDefault`, so the client may omit it), out of the `WorkerComputeType` catalog
/// CR.1X | CR.4X | CR.8X.
pub const DEFAULT_WORKER_COMPUTE_TYPE: &str = "CR.1X";

/// The `WorkerComputeConfiguration$number` of workers a configuration runs with when
/// the caller names none: the model declares `default: 16` on that member, inside its
/// `range 2..1024`.
pub const DEFAULT_WORKER_COMPUTE_NUMBER: i32 = 16;

/// The compute configuration a protected query runs with.
///
/// A caller that named no compute configuration selected none, and the member stays
/// absent. A caller that named a `worker` configuration gets the one in force: each of
/// its optional members is materialized at the default the model declares for it
/// ([`DEFAULT_WORKER_COMPUTE_TYPE`], [`DEFAULT_WORKER_COMPUTE_NUMBER`]), so a later
/// read reports the compute the query runs on rather than an absent member.
pub fn effective_compute_configuration(
    requested: Option<ComputeConfiguration>,
) -> Option<ComputeConfiguration> {
    let ComputeConfiguration::Worker(worker) = requested?;
    Some(ComputeConfiguration::Worker(WorkerComputeConfiguration {
        r#type: Some(
            worker
                .r#type
                .unwrap_or_else(|| DEFAULT_WORKER_COMPUTE_TYPE.to_string()),
        ),
        number: Some(worker.number.unwrap_or(DEFAULT_WORKER_COMPUTE_NUMBER)),
        properties: worker.properties,
    }))
}

/// Where a protected query's results are written: the configuration the request
/// carried, else the membership's stored `defaultResultConfiguration` — documented on
/// `CreateMembershipInput$defaultResultConfiguration` as "the default protected query
/// result configuration" — projected onto the query-side shape. `None` when the request
/// named none and the membership holds no default.
pub fn protected_query_result_configuration(
    requested: Option<ProtectedQueryResultConfiguration>,
    membership: &Membership,
) -> Option<ProtectedQueryResultConfiguration> {
    requested.or_else(|| {
        let default = membership.default_result_configuration.as_ref()?;
        // `MembershipProtectedQueryOutputConfiguration` declares one member, `s3`, and
        // it is the same shape the query-side union's `s3` member targets.
        let output_configuration = match &default.output_configuration {
            MembershipProtectedQueryOutputConfiguration::S3(s3) => {
                ProtectedQueryOutputConfiguration::S3(s3.clone())
            }
        };
        Some(ProtectedQueryResultConfiguration {
            output_configuration,
        })
    })
}

/// Everything a new `Membership` is built from: the ids minted for it, the stored
/// collaboration it joins, the grant the joining account holds there, and the
/// membership-only settings the request carried.
pub struct NewMembership<'a> {
    pub id: String,
    pub arn: String,
    pub record: &'a CollaborationRecord,
    pub grant: MemberGrant,
    pub query_log_status: String,
    pub job_log_status: Option<String>,
    pub default_result_configuration: Option<MembershipProtectedQueryResultConfiguration>,
    pub default_job_result_configuration: Option<MembershipProtectedJobResultConfiguration>,
    pub payment_configuration: MembershipPaymentConfiguration,
    /// The metrics setting in force — the one the caller sent, else
    /// [`DEFAULT_IS_METRICS_ENABLED`]. Not optional: the service always has one.
    pub is_metrics_enabled: bool,
    pub created_at: DateTime,
}

impl NewMembership<'_> {
    /// The stored `Membership`. Every collaboration-derived member is copied FROM the
    /// stored collaboration, never from the request.
    pub fn build(self) -> Membership {
        Membership {
            id: self.id,
            arn: self.arn,
            collaboration_arn: self.record.arn.clone(),
            collaboration_id: self.record.id.clone(),
            collaboration_creator_account_id: self.record.creator_account_id.clone(),
            collaboration_creator_display_name: self.record.creator_display_name.clone(),
            collaboration_name: self.record.name.clone(),
            create_time: self.created_at,
            update_time: self.created_at,
            status: MEMBERSHIP_STATUS_ACTIVE.to_string(),
            member_abilities: self.grant.member_abilities,
            ml_member_abilities: self.grant.ml_member_abilities,
            query_log_status: self.query_log_status,
            job_log_status: self.job_log_status,
            default_result_configuration: self.default_result_configuration,
            default_job_result_configuration: self.default_job_result_configuration,
            payment_configuration: self.payment_configuration,
            is_metrics_enabled: Some(self.is_metrics_enabled),
        }
    }
}

/// Why a page request could not be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    /// `nextToken` does not decode, or names a row this listing does not hold.
    UnknownToken,
    /// `maxResults` is not a page size. Unreachable through the wire: the model's
    /// `MaxResults` range 1..100 is refused at the boundary before any handler runs.
    InvalidMaxResults,
}

/// Encode an opaque pagination token that resumes at `id`.
///
/// Opaque to the caller (hex, so it is not a readable id) but resolved back to a
/// position by LOOKUP, not by index: an item deleted between two pages shifts every
/// index after it, which would silently skip or repeat a row.
pub fn encode_page_token(id: &str) -> String {
    ::hex::encode(id.as_bytes())
}

/// The id an opaque pagination token resumes at.
pub fn decode_page_token(token: &str) -> Result<String, PageError> {
    let bytes = ::hex::decode(token).map_err(|_| PageError::UnknownToken)?;
    String::from_utf8(bytes).map_err(|_| PageError::UnknownToken)
}

/// One page of `items`, plus the token for the page after it.
///
/// `items` is already filtered and in insertion order; `key` names each item's id.
/// `max_results` is the caller's page size (the model bounds it to 1..100); absent, the
/// whole remainder is returned. A token naming an id that is not in `items` is refused
/// rather than silently restarting at the beginning.
pub fn page<T>(
    items: Vec<T>,
    key: impl Fn(&T) -> &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), PageError> {
    let start = match next_token {
        Some(token) => {
            let resume_at = decode_page_token(token)?;
            items
                .iter()
                .position(|item| key(item) == resume_at)
                .ok_or(PageError::UnknownToken)?
        }
        None => 0,
    };
    // The model's `MaxResults` range 1..100 is refused at the boundary before any
    // handler runs, so a non-positive page size cannot reach here; this states the
    // invariant instead of a clamp that would quietly absorb a future bypass.
    let limit = match max_results {
        Some(max) if max >= 1 => max as usize,
        Some(_) => return Err(PageError::InvalidMaxResults),
        None => items.len(),
    };
    let end = start.saturating_add(limit).min(items.len());
    let token = items.get(end).map(|item| encode_page_token(key(item)));
    let page: Vec<T> = items.into_iter().skip(start).take(end - start).collect();
    Ok((page, token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState>)
}

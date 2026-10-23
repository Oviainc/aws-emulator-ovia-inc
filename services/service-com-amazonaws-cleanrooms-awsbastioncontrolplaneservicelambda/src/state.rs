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
//!
//! The privacy budget template a collaboration holds is stored the same way the
//! modelled [`Membership`] is — as the modelled `PrivacyBudgetTemplate`, because every
//! one of its members is absolute — plus the
//! `privacy_budget_template_by_collaboration` index that makes "each collaboration can
//! have only one privacy budget template" a lookup rather than a scan.
//!
//! A collaboration change request is likewise stored as the modelled
//! [`CollaborationChangeRequest`]; what the service COMPUTES about one — the change
//! types a specification asks for, whether the collaboration auto-approves them, and the
//! parties whose approval it needs — is derived from the stored collaboration by
//! [`change_types_for`], [`change_request_is_auto_approved`] and
//! [`CleanRoomsState::change_request_approvals`], and a committed change is written back
//! onto the collaboration by [`CleanRoomsState::apply_collaboration_change`]. The status
//! a request's recorded votes put it in is derived the same way, by
//! [`change_request_status_from_approvals`], and [`change_request_is_terminal`] says
//! which statuses admit no further decision.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccessBudgetsPrivacyTemplateParametersOutput, ApprovalStatusDetails, Change, ChangeSpecification,
    Collaboration,
    CollaborationChangeRequest, CollaborationSummary, ComputeConfiguration, ConfiguredTable,
    DataEncryptionMetadata, DifferentialPrivacyTemplateParametersOutput, MLMemberAbilities,
    MemberChangeSpecification, MemberSpecification, Membership, MembershipJobComputePaymentConfig,
    MembershipMLPaymentConfig, MembershipModelInferencePaymentConfig,
    MembershipModelTrainingPaymentConfig, MembershipPaymentConfiguration,
    MembershipProtectedJobResultConfiguration, MembershipProtectedQueryOutputConfiguration,
    MembershipProtectedQueryResultConfiguration, MembershipQueryComputePaymentConfig,
    MembershipSummary, MembershipSyntheticDataGenerationPaymentConfig, PaymentConfiguration,
    PrivacyBudgetTemplate, PrivacyBudgetTemplateParametersInput,
    PrivacyBudgetTemplateParametersOutput, ProtectedQuery, ProtectedQueryError,
    ProtectedQueryOutputConfiguration, ProtectedQueryResultConfiguration, TableReference,
    WorkerComputeConfiguration,
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

/// `ChangeRequestStatus` values this slice produces (model shape `ChangeRequestStatus`:
/// PENDING | APPROVED | CANCELLED | DENIED | COMMITTED).
pub const CHANGE_REQUEST_STATUS_PENDING: &str = "PENDING";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_COMMITTED: &str = "COMMITTED";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_APPROVED: &str = "APPROVED";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_DENIED: &str = "DENIED";
/// See [`CHANGE_REQUEST_STATUS_PENDING`].
pub const CHANGE_REQUEST_STATUS_CANCELLED: &str = "CANCELLED";

/// `ApprovalStatus` values this slice produces (model shape `ApprovalStatus`:
/// APPROVED | DENIED | PENDING).
pub const APPROVAL_STATUS_PENDING: &str = "PENDING";
/// See [`APPROVAL_STATUS_PENDING`].
pub const APPROVAL_STATUS_APPROVED: &str = "APPROVED";
/// See [`APPROVAL_STATUS_PENDING`].
pub const APPROVAL_STATUS_DENIED: &str = "DENIED";

/// `ChangeRequestAction` values (model shape `ChangeRequestAction`: APPROVE | DENY |
/// CANCEL | COMMIT) — the actions `UpdateCollaborationChangeRequest` performs on a
/// change request.
pub const CHANGE_REQUEST_ACTION_APPROVE: &str = "APPROVE";
/// See [`CHANGE_REQUEST_ACTION_APPROVE`].
pub const CHANGE_REQUEST_ACTION_DENY: &str = "DENY";
/// See [`CHANGE_REQUEST_ACTION_APPROVE`].
pub const CHANGE_REQUEST_ACTION_CANCEL: &str = "CANCEL";
/// See [`CHANGE_REQUEST_ACTION_APPROVE`].
pub const CHANGE_REQUEST_ACTION_COMMIT: &str = "COMMIT";

/// `ChangeSpecificationType` values (model shape `ChangeSpecificationType`: MEMBER |
/// COLLABORATION) — one per member of the `ChangeSpecification` union, which is what
/// makes a `specificationType` that names the other member detectable.
pub const CHANGE_SPECIFICATION_TYPE_MEMBER: &str = "MEMBER";
/// See [`CHANGE_SPECIFICATION_TYPE_MEMBER`].
pub const CHANGE_SPECIFICATION_TYPE_COLLABORATION: &str = "COLLABORATION";

/// The `ChangeType` a specification naming an account that is not yet part of the
/// collaboration asks for (model shape `ChangeType`).
pub const CHANGE_TYPE_ADD_MEMBER: &str = "ADD_MEMBER";
/// The `ChangeType` a `CollaborationChangeSpecification` that moves the collaboration's
/// `autoApprovedChangeTypes` asks for (model shape `ChangeType`).
pub const CHANGE_TYPE_EDIT_AUTO_APPROVED_CHANGE_TYPES: &str = "EDIT_AUTO_APPROVED_CHANGE_TYPES";
/// The `ChangeType` for a member that becomes responsible for a cost it was not
/// responsible for (model shape `ChangeType`).
pub const CHANGE_TYPE_ADD_PAYER_CANDIDATE: &str = "ADD_PAYER_CANDIDATE";
/// The `ChangeType` for a member that stops being responsible for every cost (model
/// shape `ChangeType`).
pub const CHANGE_TYPE_REMOVE_PAYER_CANDIDATE: &str = "REMOVE_PAYER_CANDIDATE";

/// Each `MemberAbility` the `ChangeType` catalog can grant or revoke, as
/// `(ability, grant type, revoke type)`.
///
/// The `ChangeType` catalog (ADD_MEMBER | GRANT_RECEIVE_RESULTS_ABILITY |
/// REVOKE_RECEIVE_RESULTS_ABILITY | EDIT_AUTO_APPROVED_CHANGE_TYPES |
/// ADD_PAYER_CANDIDATE | REMOVE_PAYER_CANDIDATE | GRANT_CAN_RECEIVE_MODEL_OUTPUT |
/// GRANT_CAN_RECEIVE_INFERENCE_OUTPUT | REVOKE_CAN_RECEIVE_MODEL_OUTPUT |
/// REVOKE_CAN_RECEIVE_INFERENCE_OUTPUT | GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY |
/// REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY) names a grant/revoke pair for exactly two
/// of the four `MemberAbility` values, so a change to CAN_QUERY or CAN_RUN_JOB has no
/// change type to report and cannot be asked for through a change request.
pub const MEMBER_ABILITY_CHANGE_TYPES: [(&str, &str, &str); 2] = [
    (
        "CAN_RECEIVE_RESULTS",
        "GRANT_RECEIVE_RESULTS_ABILITY",
        "REVOKE_RECEIVE_RESULTS_ABILITY",
    ),
    (
        "CAN_EXPORT_QUERY_ANALYSIS_LOG",
        "GRANT_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
        "REVOKE_EXPORT_QUERY_ANALYSIS_LOG_ABILITY",
    ),
];

/// Each `CustomMLMemberAbility` the `ChangeType` catalog can grant or revoke, as
/// `(ability, grant type, revoke type)`. The catalog
/// (CAN_RECEIVE_MODEL_OUTPUT | CAN_RECEIVE_INFERENCE_OUTPUT) has a grant and a revoke
/// change type for both of its values. See [`MEMBER_ABILITY_CHANGE_TYPES`].
pub const ML_MEMBER_ABILITY_CHANGE_TYPES: [(&str, &str, &str); 2] = [
    (
        "CAN_RECEIVE_MODEL_OUTPUT",
        "GRANT_CAN_RECEIVE_MODEL_OUTPUT",
        "REVOKE_CAN_RECEIVE_MODEL_OUTPUT",
    ),
    (
        "CAN_RECEIVE_INFERENCE_OUTPUT",
        "GRANT_CAN_RECEIVE_INFERENCE_OUTPUT",
        "REVOKE_CAN_RECEIVE_INFERENCE_OUTPUT",
    ),
];

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

/// `PrivacyBudgetType` values this slice keys on (model shape `PrivacyBudgetType`:
/// DIFFERENTIAL_PRIVACY | ACCESS_BUDGET). Each value names exactly one member of the
/// `PrivacyBudgetTemplateParametersInput` union, which is what makes a type/parameters
/// mismatch detectable.
pub const PRIVACY_BUDGET_TYPE_DIFFERENTIAL_PRIVACY: &str = "DIFFERENTIAL_PRIVACY";
/// See [`PRIVACY_BUDGET_TYPE_DIFFERENTIAL_PRIVACY`].
pub const PRIVACY_BUDGET_TYPE_ACCESS_BUDGET: &str = "ACCESS_BUDGET";

/// How often a privacy budget template refreshes when the create request names no
/// cadence.
///
/// `PrivacyBudgetTemplate$autoRefresh` is REQUIRED on the output while
/// `CreatePrivacyBudgetTemplateInput$autoRefresh` is optional, so the service decides
/// the value a template without one carries. The `PrivacyBudgetTemplateAutoRefresh`
/// catalog is CALENDAR_MONTH | NONE, and CALENDAR_MONTH is documented as an opt-in
/// ("you can use CALENDAR_MONTH to automatically get a new privacy budget … Avoid
/// choosing this if the same rows will be repeatedly queried"), so a template nobody
/// asked to refresh does not refresh. This is the emulator's choice of the value the
/// service supplies, not a published AWS default.
pub const DEFAULT_PRIVACY_BUDGET_TEMPLATE_AUTO_REFRESH: &str = "NONE";

/// `ConflictExceptionReason` for a resource whose current status does not admit the
/// requested change — a membership whose collaboration is gone, say (model shape
/// `ConflictExceptionReason`: ALREADY_EXISTS | SUBRESOURCES_EXIST | INVALID_STATE).
pub const CONFLICT_REASON_INVALID_STATE: &str = "INVALID_STATE";

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

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the privacy budget
/// template store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreatePrivacyBudgetTemplate` declares.
///
/// A per-collaboration limit is not a bound on this store on its own: a collaboration
/// can be deleted and recreated, which frees the slot [`MAX_COLLABORATIONS`] counts, so
/// the template store needs its own maximum entry count like every other store here.
pub const MAX_PRIVACY_BUDGET_TEMPLATES: usize = 5_000;

/// See [`MAX_COLLABORATIONS`]: the same EMULATOR-enforced bound on the collaboration
/// change request store, reported through the `ServiceQuotaExceededException` (HTTP 402)
/// `CreateCollaborationChangeRequest` declares.
pub const MAX_COLLABORATION_CHANGE_REQUESTS: usize = 5_000;

/// The largest `approvals` map a change request can carry, from the MODEL: the
/// `ApprovalStatuses` map is declared `len 1..50`. A change request records one approval
/// entry per party to the collaboration (see
/// [`CleanRoomsState::change_request_approvals`]), so a collaboration with more parties
/// than this has no representable approvals map and the create refuses with the
/// `ServiceQuotaExceededException` it declares.
pub const MAX_CHANGE_REQUEST_APPROVALS: usize = 50;


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
    /// Privacy budget templates by `PrivacyBudgetTemplate.id`. The modelled shape is
    /// stored as-is: every one of its members is absolute, so there is nothing to
    /// derive per request.
    pub privacy_budget_templates: HashMap<String, PrivacyBudgetTemplate>,
    /// Privacy budget template ids in creation order — the order a list operation
    /// pages in.
    pub privacy_budget_template_order: Vec<String>,
    /// `collaboration id -> privacy budget template id`: the index that makes
    /// "each collaboration can have only one privacy budget template" (the
    /// `CreatePrivacyBudgetTemplate` doc) computable instead of guessed.
    pub privacy_budget_template_by_collaboration: HashMap<String, String>,
    /// Protected queries by `ProtectedQuery.id`.
    pub protected_queries: HashMap<String, ProtectedQueryRecord>,
    /// Protected query ids in start order — the order a listing pages in.
    pub protected_query_order: Vec<String>,
    /// Collaboration change requests by `CollaborationChangeRequest.id`.
    ///
    /// The modelled shape is stored as-is, like [`Membership`]: every one of its members
    /// is absolute (its `changes`, `status`, `approvals` and `isAutoApproved` describe
    /// the request, not who is asking), so a read reports it without deriving anything.
    /// The owning collaboration is on the record itself (`collaborationId`), which is
    /// what scopes a lookup to one collaboration.
    pub collaboration_change_requests: HashMap<String, CollaborationChangeRequest>,
    /// Change request ids in creation order — the order a listing pages in.
    pub collaboration_change_request_order: Vec<String>,
}

/// Shorthand for this service's state struct.
pub type CleanRoomsState = ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState;

impl ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState {
    /// The stored collaboration `collaboration_id` names together with the grant
    /// `account_id` holds in it, or `None` when this account has no authorization to
    /// that collaboration at all.
    ///
    /// `None` covers all three indistinguishable cases: no collaboration is stored
    /// under the id (it never existed, or `DeleteCollaboration` removed it), or one is
    /// stored but the account is neither its creator nor one of its declared members.
    /// A caller's authorization to a collaboration lives in the collaboration itself,
    /// so it disappears with it — which is why these cases are one answer and not
    /// three. Every operation that takes a `collaborationIdentifier` resolves it here,
    /// so the refusal it reports is the same whichever case holds and no operation
    /// leaks whether the id exists.
    pub fn collaboration_grant(
        &self,
        collaboration_id: &str,
        account_id: &str,
    ) -> Option<(&CollaborationRecord, MemberGrant)> {
        let record = self.collaborations.get(collaboration_id)?;
        let grant = record.grant_for(account_id)?;
        Some((record, grant))
    }

    /// The stored collaboration `account_id` is authorized to see under
    /// `collaboration_id`. See [`Self::collaboration_grant`] for what `None` means.
    pub fn visible_collaboration(
        &self,
        collaboration_id: &str,
        account_id: &str,
    ) -> Option<&CollaborationRecord> {
        self.collaboration_grant(collaboration_id, account_id)
            .map(|(record, _)| record)
    }

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
        let record = self.visible_collaboration(collaboration_id, account_id)?;
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

    /// The privacy budget template `collaboration_id` holds, if any.
    ///
    /// `CreatePrivacyBudgetTemplate` documents "Each collaboration can have only one
    /// privacy budget template", so this is the record a second create collides with.
    pub fn privacy_budget_template_of_collaboration(
        &self,
        collaboration_id: &str,
    ) -> Option<&PrivacyBudgetTemplate> {
        let id = self
            .privacy_budget_template_by_collaboration
            .get(collaboration_id)?;
        self.privacy_budget_templates.get(id)
    }

    /// The stored template `template_id`, but only when it belongs to
    /// `collaboration_id`.
    ///
    /// A privacy budget template is a resource of the COLLABORATION, and the read
    /// operations address it through a membership ("the privacy budget template is
    /// retrieved from the collaboration that this membership belongs to"), so a
    /// template of another collaboration is not visible through this membership.
    pub fn privacy_budget_template_in_collaboration(
        &self,
        collaboration_id: &str,
        template_id: &str,
    ) -> Option<&PrivacyBudgetTemplate> {
        self.privacy_budget_templates
            .get(template_id)
            .filter(|template| template.collaboration_id == collaboration_id)
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

    /// The change request `change_request_id`, but only when it was made against
    /// `collaboration_id`: a change request is addressed under its collaboration
    /// (`/collaborations/{collaborationIdentifier}/changeRequests/{changeRequestIdentifier}`),
    /// so one collaboration never reads another's change request.
    pub fn collaboration_change_request_in_collaboration(
        &self,
        collaboration_id: &str,
        change_request_id: &str,
    ) -> Option<&CollaborationChangeRequest> {
        self.collaboration_change_requests
            .get(change_request_id)
            .filter(|request| request.collaboration_id == collaboration_id)
    }

    /// Every account whose approval a change request against `record` needs, in a
    /// deterministic order: the creator first, then the declared members in the order
    /// the collaboration was created with.
    ///
    /// `CollaborationChangeRequest$approvals` is documented as "approval details from
    /// collaboration members, including approval status and multi-party approval
    /// workflow information" and is keyed by `AccountId`. The model names no finer rule
    /// for WHICH accounts those are, so this slice takes the parties to the
    /// collaboration — the accounts whose grants the collaboration holds — as the
    /// parties to a change of it. The account a change ADDS is not among them: it is the
    /// subject of the change, not yet a party to the collaboration.
    pub fn change_request_approvals(&self, record: &CollaborationRecord) -> Vec<String> {
        let mut accounts = Vec::with_capacity(record.members.len() + 1);
        accounts.push(record.creator_account_id.clone());
        for member in &record.members {
            if member.account_id != record.creator_account_id {
                accounts.push(member.account_id.clone());
            }
        }
        accounts
    }

    /// Apply one COMMITTED change to the collaboration it was made against.
    ///
    /// A `MemberChangeSpecification` carries the member's whole intended grant
    /// (`memberAbilities` is required on it), so applying it REPLACES that account's
    /// stored grant — creating it when the account was not a member yet — and applying a
    /// `CollaborationChangeSpecification` replaces the collaboration's
    /// `autoApprovedChangeTypes`. The collaboration's `updateTime` is restamped, and
    /// when the affected account already holds a membership of this collaboration its
    /// `memberAbilities`/`mlMemberAbilities` are restated too, because a membership
    /// reports the same grant from the member's side.
    pub fn apply_collaboration_change(
        &mut self,
        collaboration_id: &str,
        change: &Change,
        at: DateTime,
    ) {
        let Some(record) = self.collaborations.get_mut(collaboration_id) else {
            return;
        };
        let member_spec = match &change.specification {
            ChangeSpecification::Collaboration(spec) => {
                record.auto_approved_change_types = spec.auto_approved_change_types.clone();
                record.update_time = at;
                return;
            }
            ChangeSpecification::Member(spec) => spec,
        };

        if member_spec.account_id == record.creator_account_id {
            record.creator_member_abilities = member_spec.member_abilities.clone();
            record.creator_ml_member_abilities = member_spec.ml_member_abilities.clone();
            if member_spec.payment_configuration.is_some() {
                record.creator_payment_configuration = member_spec.payment_configuration.clone();
            }
            if let Some(display_name) = &member_spec.display_name {
                record.creator_display_name = display_name.clone();
            }
        } else if let Some(member) = record
            .members
            .iter_mut()
            .find(|member| member.account_id == member_spec.account_id)
        {
            member.member_abilities = member_spec.member_abilities.clone();
            member.ml_member_abilities = member_spec.ml_member_abilities.clone();
            if member_spec.payment_configuration.is_some() {
                member.payment_configuration = member_spec.payment_configuration.clone();
            }
            if let Some(display_name) = &member_spec.display_name {
                member.display_name = display_name.clone();
            }
        } else if let Some(display_name) = &member_spec.display_name {
            // `MemberSpecification$displayName` is required, so a member can only be
            // materialized with the display name the specification carried;
            // `CreateCollaborationChangeRequest` refuses an add without one, so the
            // `else` case here is unreachable rather than silently guessed at.
            record.members.push(MemberSpecification {
                account_id: member_spec.account_id.clone(),
                member_abilities: member_spec.member_abilities.clone(),
                ml_member_abilities: member_spec.ml_member_abilities.clone(),
                display_name: display_name.clone(),
                payment_configuration: member_spec.payment_configuration.clone(),
            });
        }
        record.update_time = at;

        let membership_id = self
            .membership_by_collaboration
            .get(&(
                collaboration_id.to_string(),
                member_spec.account_id.clone(),
            ))
            .cloned();
        if let Some(membership) = membership_id.and_then(|id| self.memberships.get_mut(&id)) {
            membership.member_abilities = member_spec.member_abilities.clone();
            membership.ml_member_abilities = member_spec.ml_member_abilities.clone();
            membership.update_time = at;
        }
    }
}

/// The change types one specification asks of the collaboration as stored — the
/// `Change$types` the service reports back ("the list of change types that were
/// applied").
///
/// EMPTY means the specification asks for nothing the collaboration does not already
/// have, or asks for something the `ChangeType` catalog cannot name (a CAN_QUERY or
/// CAN_RUN_JOB ability change, say): `ChangeTypeList` is `len 1..`, so such a change is
/// not representable and the create refuses it.
pub fn change_types_for(
    record: &CollaborationRecord,
    specification: &ChangeSpecification,
) -> Vec<String> {
    match specification {
        ChangeSpecification::Member(spec) => match record.grant_for(&spec.account_id) {
            // An account the collaboration holds no grant for is being added.
            None => vec![CHANGE_TYPE_ADD_MEMBER.to_string()],
            Some(current) => member_change_types(&current, spec),
        },
        ChangeSpecification::Collaboration(spec) => {
            let requested = spec.auto_approved_change_types.as_deref().unwrap_or(&[]);
            let current = record.auto_approved_change_types.as_deref().unwrap_or(&[]);
            if same_change_type_set(requested, current) {
                Vec::new()
            } else {
                vec![CHANGE_TYPE_EDIT_AUTO_APPROVED_CHANGE_TYPES.to_string()]
            }
        }
    }
}

/// The change types a specification asks for an account that is ALREADY a party to the
/// collaboration: the difference between the grant it holds and the one the
/// specification states, named from the `ChangeType` catalog. See
/// [`MEMBER_ABILITY_CHANGE_TYPES`] and [`ML_MEMBER_ABILITY_CHANGE_TYPES`].
fn member_change_types(current: &MemberGrant, requested: &MemberChangeSpecification) -> Vec<String> {
    let mut types = Vec::new();
    for (ability, grant, revoke) in MEMBER_ABILITY_CHANGE_TYPES {
        let held = current.member_abilities.iter().any(|held| held == ability);
        let wanted = requested
            .member_abilities
            .iter()
            .any(|wanted| wanted == ability);
        match (held, wanted) {
            (false, true) => types.push(grant.to_string()),
            (true, false) => types.push(revoke.to_string()),
            _ => {}
        }
    }
    for (ability, grant, revoke) in ML_MEMBER_ABILITY_CHANGE_TYPES {
        let held = holds_ml_ability(current.ml_member_abilities.as_ref(), ability);
        let wanted = holds_ml_ability(requested.ml_member_abilities.as_ref(), ability);
        match (held, wanted) {
            (false, true) => types.push(grant.to_string()),
            (true, false) => types.push(revoke.to_string()),
            _ => {}
        }
    }
    // A member is a payer candidate while its payment configuration makes it
    // responsible for any cost; the catalog's ADD_PAYER_CANDIDATE /
    // REMOVE_PAYER_CANDIDATE name entering and leaving that set.
    let was_payer = is_payer_candidate(current.payment_configuration.as_ref());
    let is_payer = is_payer_candidate(requested.payment_configuration.as_ref());
    match (was_payer, is_payer) {
        (false, true) => types.push(CHANGE_TYPE_ADD_PAYER_CANDIDATE.to_string()),
        (true, false) => types.push(CHANGE_TYPE_REMOVE_PAYER_CANDIDATE.to_string()),
        _ => {}
    }
    types
}

/// Whether `abilities` carries the `CustomMLMemberAbility` `ability`.
fn holds_ml_ability(abilities: Option<&MLMemberAbilities>, ability: &str) -> bool {
    abilities.is_some_and(|abilities| {
        abilities
            .custom_ml_member_abilities
            .iter()
            .any(|held| held == ability)
    })
}

/// Whether a payment configuration makes its member responsible for any cost: query
/// compute, job compute, or one of the machine-learning costs.
fn is_payer_candidate(configuration: Option<&PaymentConfiguration>) -> bool {
    let Some(configuration) = configuration else {
        return false;
    };
    let ml = configuration.machine_learning.as_ref();
    configuration.query_compute.is_responsible
        || configuration
            .job_compute
            .as_ref()
            .is_some_and(|compute| compute.is_responsible)
        || ml.is_some_and(|ml| {
            ml.model_training
                .as_ref()
                .is_some_and(|cost| cost.is_responsible)
                || ml
                    .model_inference
                    .as_ref()
                    .is_some_and(|cost| cost.is_responsible)
                || ml
                    .synthetic_data_generation
                    .as_ref()
                    .is_some_and(|cost| cost.is_responsible)
        })
}

/// Whether two `autoApprovedChangeTypes` lists name the same set — the lists are
/// `uniqueItems`, so set equality is length plus containment, and ORDER is not a change.
fn same_change_type_set(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().all(|value| right.contains(value))
}

/// Whether the collaboration's auto-approval settings cover every change type a request
/// asks for — the `isAutoApproved` the created change request reports ("whether the
/// change request was automatically approved based on the collaboration's auto-approval
/// settings").
///
/// `AutoApprovedChangeType` is a 5-value subset of the 12-value `ChangeType` catalog, so
/// a change type outside that subset can never be covered and such a request is never
/// auto-approved.
pub fn change_request_is_auto_approved(record: &CollaborationRecord, changes: &[Change]) -> bool {
    let auto_approved = record.auto_approved_change_types.as_deref().unwrap_or(&[]);
    !changes.is_empty()
        && changes.iter().all(|change| {
            change
                .types
                .iter()
                .all(|change_type| auto_approved.contains(change_type))
        })
}

/// Whether a change request's status admits no further action: a COMMITTED, DENIED or
/// CANCELLED request has reached the end of the `ChangeRequestStatus` lifecycle, so
/// there is no decision left to take on it.
pub fn change_request_is_terminal(status: &str) -> bool {
    status == CHANGE_REQUEST_STATUS_COMMITTED
        || status == CHANGE_REQUEST_STATUS_DENIED
        || status == CHANGE_REQUEST_STATUS_CANCELLED
}

/// The `ChangeRequestStatus` a change request's recorded votes put it in.
///
/// `ApprovalStatusDetails$status` is "the approval status of a member's vote on the
/// change request … PENDING (if they haven't voted), APPROVED, or DENIED", and
/// `UpdateCollaborationChangeRequest` documents DENY as rejecting the change, so one
/// DENIED vote settles the request as DENIED, a request every party has APPROVED is
/// APPROVED (ready for the owner to COMMIT), and one still holding a PENDING vote stays
/// PENDING. An EMPTY tally is PENDING: there is no vote to conclude anything from.
pub fn change_request_status_from_approvals(
    approvals: &HashMap<String, ApprovalStatusDetails>,
) -> &'static str {
    if approvals
        .values()
        .any(|details| details.status == APPROVAL_STATUS_DENIED)
    {
        return CHANGE_REQUEST_STATUS_DENIED;
    }
    if !approvals.is_empty()
        && approvals
            .values()
            .all(|details| details.status == APPROVAL_STATUS_APPROVED)
    {
        return CHANGE_REQUEST_STATUS_APPROVED;
    }
    CHANGE_REQUEST_STATUS_PENDING
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

/// The message an operation reports when the caller has no authorization to the
/// `collaborationIdentifier` it sent — see
/// [`CleanRoomsState::collaboration_grant`] for the cases that reach it.
///
/// Composed from the request's collaboration identifier and the resolved caller
/// account: those two are what the authorization decision was made on, and naming them
/// is what tells a caller the collaboration is not reachable from THIS account rather
/// than that the id is malformed. It deliberately does not say whether a collaboration
/// is stored under the id.
pub fn collaboration_access_denied_message(collaboration_id: &str, account_id: &str) -> String {
    format!(
        "Account {account_id} is not authorized to access collaboration: {collaboration_id}"
    )
}

/// The message a membership lookup reports when the id names no membership of the
/// caller's.
///
/// Composed from the request's `membershipIdentifier` and the resolved caller account:
/// membership ids are account-scoped, so the account is what says the lookup failed in
/// THIS account's namespace rather than that the id exists nowhere.
pub fn membership_not_found_message(membership_id: &str, account_id: &str) -> String {
    format!("No such membership: {membership_id} for account {account_id}")
}

/// The message `CreateMembership` reports when the caller already holds a membership in
/// the collaboration it asked to join.
///
/// Composed from the resolved caller account and the request's collaboration
/// identifier: one account holds at most one membership per collaboration, so the
/// account the membership would be created for is the value that fails validation.
pub fn duplicate_membership_message(account_id: &str, collaboration_id: &str) -> String {
    format!(
        "Cannot create membership for account: {account_id}. \
         The account is already a member of collaboration: {collaboration_id}"
    )
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

/// `arn:aws:cleanrooms:<region>:<account>:privacybudgettemplate/<id>`, the form the
/// model's `PrivacyBudgetTemplateArn` pattern declares.
pub fn privacy_budget_template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("privacybudgettemplate/{id}"),
    )
}

/// The `privacyBudgetType` the parameters union itself declares: each member of
/// `PrivacyBudgetTemplateParametersInput` carries the parameters of exactly one
/// `PrivacyBudgetType`, so a request whose `privacyBudgetType` names the other one
/// cannot be stored coherently.
pub fn privacy_budget_type_of_parameters(
    parameters: &PrivacyBudgetTemplateParametersInput,
) -> &'static str {
    match parameters {
        PrivacyBudgetTemplateParametersInput::differentialPrivacy(_) => {
            PRIVACY_BUDGET_TYPE_DIFFERENTIAL_PRIVACY
        }
        PrivacyBudgetTemplateParametersInput::accessBudget(_) => PRIVACY_BUDGET_TYPE_ACCESS_BUDGET,
    }
}

/// The stored `PrivacyBudgetTemplateParametersOutput` for the parameters a create
/// carried. The input and output unions declare the same members with the same
/// members inside them, so this is a projection, not a reinterpretation.
pub fn privacy_budget_template_parameters_output(
    parameters: &PrivacyBudgetTemplateParametersInput,
) -> PrivacyBudgetTemplateParametersOutput {
    match parameters {
        PrivacyBudgetTemplateParametersInput::differentialPrivacy(input) => {
            PrivacyBudgetTemplateParametersOutput::differentialPrivacy(
                DifferentialPrivacyTemplateParametersOutput {
                    epsilon: input.epsilon,
                    users_noise_per_query: input.users_noise_per_query,
                },
            )
        }
        PrivacyBudgetTemplateParametersInput::accessBudget(input) => {
            PrivacyBudgetTemplateParametersOutput::accessBudget(
                AccessBudgetsPrivacyTemplateParametersOutput {
                    budget_parameters: input.budget_parameters.clone(),
                    resource_arn: input.resource_arn.clone(),
                },
            )
        }
    }
}

/// The refresh cadence a created privacy budget template carries: the one the caller
/// asked for, else [`DEFAULT_PRIVACY_BUDGET_TEMPLATE_AUTO_REFRESH`].
pub fn effective_template_auto_refresh(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_PRIVACY_BUDGET_TEMPLATE_AUTO_REFRESH.to_string())
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
/// configures one method — see [`MIN_SELECTED_ANALYSIS_METHODS`]. The count is taken
/// through a hash set, so it is LINEAR in the list the request carries: a membership
/// scan would make the per-request work quadratic in a list the model does not bound
/// above.
pub fn distinct_selected_analysis_methods(methods: &[String]) -> usize {
    methods
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<&str>>()
        .len()
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

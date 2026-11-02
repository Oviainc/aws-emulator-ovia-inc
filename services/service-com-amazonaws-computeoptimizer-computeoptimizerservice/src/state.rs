// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-computeoptimizer-computeoptimizerservice service.
//!
//! Compute Optimizer's writable surface is the account's ENROLLMENT record plus its
//! RECOMMENDATION PREFERENCES; everything else the service returns is analysis output it
//! derives from observed workloads. Both records live here, together with the rules more
//! than one operation applies to them — scope well-formedness, the preference upsert, the
//! scope precedence `GetEffectiveRecommendationPreferences` resolves with, and the opaque
//! page token `GetRecommendationPreferences` round-trips — so no two handlers can drift
//! from each other's view of the same record. The two rules EVERY paginated reader of this
//! service applies live here for the same reason: how large a page a request asks for
//! ([`page_size`]) and which items that page carries, with the token that resumes the walk
//! after it ([`offset_page`]). The ORGANIZATION's member accounts live here
//! too ([`OrganizationMemberStore`]): AWS Organizations records which accounts an
//! organization holds, so no operation of this API creates one, while an
//! `UpdateEnrollmentStatus` call that includes them writes their enrollment — which
//! `GetEnrollmentStatusesForOrganization` reports and `GetEnrollmentStatus` counts over.
//!
//! The analysis output this service REPORTS lives here too. The FINDINGS CATALOG holds one
//! row per resource Compute Optimizer has analysed for the account (its finding, its reason
//! codes, its performance risk, what it costs and what the recommendation would cost, and —
//! for a resource analysed as an Amazon Aurora or RDS database — the database configuration
//! the analysis observed), part of the store's initial value so every findings reader
//! aggregates the same rows without any of them writing to the store. A row records a
//! resource's REGION-FREE identity, because Compute Optimizer is regional: the ARN a reader
//! reports names the region of the endpoint the request addressed
//! ([`AnalysedResource::arn_in`], [`request_region`]). Idle resource recommendations have their own store
//! ([`IdleRecommendationStore`]): no operation of this API creates one — Compute Optimizer's
//! own analysis of an observed workload does — so that analysis is part of the store's INITIAL
//! VALUE ([`seed_idle_resource_analyses`]), exactly as the volume, group, function and license
//! observations are, and `GetIdleRecommendations` filters, orders and pages over it. Every
//! member of a reported recommendation is DERIVED from the stored analysis
//! ([`IdleResourceAnalysis::to_recommendation`]), and the rules a reader applies to it (the
//! filter vocabulary, the sort dimensions, the ARN -> resource-type catalog) are stated once,
//! here. The Lambda functions the service observes are their own inventory too
//! ([`ObservedFunction`]), from which `GetLambdaFunctionRecommendations` DERIVES every
//! recommendation it reports, and so are the licensed Amazon EC2 instances
//! ([`ObservedLicensedInstance`]), from which `GetLicenseRecommendations` DERIVES every
//! license recommendation it reports, and the Amazon EC2 instances it analyses
//! ([`ObservedInstance`]), from which `GetEC2InstanceRecommendations` DERIVES every instance
//! recommendation it reports — its finding, its utilization, the options its candidate types
//! offer and what each of them saves — and whose observed utilization
//! `GetEC2RecommendationProjectedMetrics` projects onto each instance type the analysis
//! recommends. The Amazon ECS service analysis has its inputs and its sizing rule here too
//! ([`parse_ecs_service_arn`], [`ecs_service_config`], [`ecs_observed_utilization`],
//! [`ecs_recommended_task_size`], [`ecs_projected_utilization_metrics`]), computed over the
//! ECS inventory and the utilization published for a service, so the recommendation reader
//! and the reader of that recommendation's projected metrics cannot disagree about either.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

pub mod export;

use ::aws_smithy_types::DateTime;
use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use ::service_com_amazonaws_ecs_amazonec2containerservicev20141113 as ecs;
use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    AccountEnrollmentStatus, AutoScalingGroupConfiguration, ContainerConfiguration,
    CustomizableMetricParameters,
    DBStorageConfiguration, Filter,
    EBSEffectiveRecommendationPreferences, EBSEstimatedMonthlySavings, EBSFilter,
    EBSSavingsEstimationMode,
    EBSSavingsOpportunityAfterDiscounts, EBSUtilizationMetric,
    ECSServiceProjectedUtilizationMetric, ECSServiceRecommendationFilter,
    EffectivePreferredResource,
    EffectiveRecommendationPreferences, EnrollmentFilter, EstimatedMonthlySavings,
    ExportDestination, ExternalMetricStatus, ExternalMetricsPreference,
    IdleEstimatedMonthlySavings, IdleRecommendation,
    IdleRecommendationFilter, IdleSavingsOpportunity, IdleSavingsOpportunityAfterDiscounts,
    IdleUtilizationMetric, InstanceSavingsEstimationMode, JobFilter,
    LambdaEffectiveRecommendationPreferences, LambdaEstimatedMonthlySavings,
    LambdaFunctionMemoryProjectedMetric, LambdaFunctionMemoryRecommendationOption,
    LambdaFunctionRecommendation, LambdaFunctionRecommendationFilter,
    LambdaFunctionUtilizationMetric, LambdaSavingsEstimationMode,
    LambdaSavingsOpportunityAfterDiscounts, LicenseConfiguration, LicenseRecommendation,
    LicenseRecommendationFilter, LicenseRecommendationOption, MemorySizeConfiguration,
    MetricSource, PreferredResource,
    PutRecommendationPreferencesInput, RDSDBInstanceRecommendationOption, RDSDBRecommendation,
    RDSDBRecommendationFilter, RDSDBStorageRecommendationOption, RDSDBUtilizationMetric,
    RDSEffectiveRecommendationPreferences, RDSInstanceEstimatedMonthlySavings,
    RDSInstanceSavingsOpportunityAfterDiscounts, RDSSavingsEstimationMode,
    RDSStorageEstimatedMonthlySavings, RDSStorageSavingsOpportunityAfterDiscounts,
    RecommendationExportJob, RecommendationPreferencesDetail,
    S3Destination, SavingsOpportunity, Scope, Tag, UtilizationMetric, UtilizationPreference,
    VolumeConfiguration, VolumeRecommendation, VolumeRecommendationOption,
    ENROLLMENT_FILTER_NAME_VALUES, EXTERNAL_METRICS_SOURCE_VALUES,
    EXTERNAL_METRIC_STATUS_CODE_VALUES, FILTER_NAME_VALUES,
    FINDING_REASON_CODE_VALUES,
    FINDING_VALUES,
    IDLE_FINDING_VALUES,
    IDLE_RECOMMENDATION_FILTER_NAME_VALUES, IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES,
    IDLE_VALUES, INFERRED_WORKLOAD_TYPE_VALUES,
    INSTANCE_RECOMMENDATION_FINDING_REASON_CODE_VALUES, JOB_FILTER_NAME_VALUES,
    JOB_STATUS_VALUES, E_B_S_FILTER_NAME_VALUES, E_B_S_FINDING_VALUES,
    E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES,
    E_C_S_SERVICE_RECOMMENDATION_FINDING_REASON_CODE_VALUES,
    E_C_S_SERVICE_RECOMMENDATION_FINDING_VALUES,
    LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES,
    LAMBDA_FUNCTION_RECOMMENDATION_FINDING_REASON_CODE_VALUES,
    LAMBDA_FUNCTION_RECOMMENDATION_FINDING_VALUES, LICENSE_FINDING_REASON_CODE_VALUES,
    LICENSE_FINDING_VALUES, LICENSE_NAME_VALUES, LICENSE_RECOMMENDATION_FILTER_NAME_VALUES,
    RECOMMENDATION_SOURCE_TYPE_VALUES,
    R_D_S_CURRENT_INSTANCE_PERFORMANCE_RISK_VALUES,
    R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES, R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES,
    R_D_S_INSTANCE_FINDING_VALUES, R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
    R_D_S_STORAGE_FINDING_VALUES, RESOURCE_TYPE_VALUES, STATUS_VALUES,
};

/// The look-back period in force when no preference sets one. From the model's own
/// `PutRecommendationPreferences$lookBackPeriod` documentation: "When this preference
/// isn't specified, we use the default value `DAYS_14`".
pub const DEFAULT_LOOK_BACK_PERIOD: &str = "DAYS_14";

/// The most preference records this store holds. A `PutRecommendationPreferences` key is
/// built from caller-supplied `resourceType` + `scope`, and a `ResourceArn` scope admits
/// any EC2 instance or Auto Scaling group ARN, so without this bound repeated calls could
/// mint unlimited distinct keys and grow one emulator process without limit. A new key
/// past this count is REFUSED ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::can_hold`]);
/// updating a key the store already holds stays allowed, because it allocates no record.
pub const MAX_PREFERENCE_RECORDS: usize = 1_000;

/// The largest page a paginated reader in this service will build, whatever `maxResults`
/// asks for: the ceiling the model itself puts on `maxResults` (`MaxResults` range
/// 0..1000). Applied in the handler, so no number a request carries can size a response or
/// the allocation that builds it, whatever any other layer does or does not refuse.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// The page size a paginated reader uses when the request names none (or names the
/// range's lower bound `0`, which the service documents no meaning for). The service
/// documents no default page size, so this is the emulator's own: a request that asks for
/// no particular size gets one bounded page plus a `nextToken` to walk the rest, rather
/// than the whole store serialised into one response.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most resource identifiers one request may name in a reader's ARN list, whatever the
/// model declares no bound for: a request naming more is REFUSED with that operation's
/// `InvalidParameterValueException` rather than answered.
///
/// The identifiers a request carries drive the selection and the per-identifier `errors` list
/// of a reader, so without a ceiling the work one request causes, and the response it gets
/// back, would both grow with the size of the list it sent. The bound is the page ceiling
/// [`MAX_PAGE_SIZE`]: a request cannot usefully ask about more resources than the largest page
/// this service will report.
pub const MAX_REQUESTED_IDENTIFIERS: usize = MAX_PAGE_SIZE;

/// Why an ARN list a request carries is too long to answer ([`MAX_REQUESTED_IDENTIFIERS`]),
/// naming the member and the bound, or `None` when it is within it. Stated once so no two
/// readers of this service bound their identifier list differently.
pub fn too_many_identifiers(member: &str, named: usize) -> Option<String> {
    (named > MAX_REQUESTED_IDENTIFIERS).then(|| {
        format!(
            "The value supplied for parameter {member} is not valid: at most \
             {MAX_REQUESTED_IDENTIFIERS} resources can be named per request, not {named}."
        )
    })
}

/// The `ScopeName` enum values (`crate::ops::types::SCOPE_NAME_VALUES`), named so the
/// precedence and validation rules below read as the model's own vocabulary.
pub const SCOPE_ORGANIZATION: &str = "Organization";
/// See [`SCOPE_ORGANIZATION`].
pub const SCOPE_ACCOUNT_ID: &str = "AccountId";
/// See [`SCOPE_ORGANIZATION`].
pub const SCOPE_RESOURCE_ARN: &str = "ResourceArn";

/// The value an `Organization` scope must carry, per the model's `Scope$value` doc.
pub const ORGANIZATION_SCOPE_VALUE: &str = "ALL_ACCOUNTS";

/// The `Status` enum values this service can be left in by a client call.
pub const STATUS_ACTIVE: &str = "Active";
/// See [`STATUS_ACTIVE`].
pub const STATUS_INACTIVE: &str = "Inactive";

/// The `ResourceType` values whose ARNs `Scope`/`GetEffectiveRecommendationPreferences`
/// accept: "Only EC2 instance and Auto Scaling group ARNs are currently supported."
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "Ec2Instance";
/// See [`RESOURCE_TYPE_EC2_INSTANCE`].
pub const RESOURCE_TYPE_AUTO_SCALING_GROUP: &str = "AutoScalingGroup";

/// The service namespace an Auto Scaling group's ARN carries
/// (`arn:aws:autoscaling:<region>:<account>:autoScalingGroup:…`), stated once so the ARN a
/// reader builds and the ARN form the preference API accepts cannot drift apart.
pub const AUTO_SCALING_ARN_SERVICE: &str = "autoscaling";

/// The `ResourceType` an Amazon ECS service is analysed as (`RESOURCE_TYPE_VALUES`). A
/// preference written for it is what `GetECSServiceRecommendations` resolves its effective
/// look-back period from.
///
/// Not to be confused with [`SOURCE_TYPE_ECS_SERVICE`], which spells the same word in the
/// `RecommendationSourceType` domain a findings row carries: one is what a PREFERENCE is
/// written for, the other what an analysed resource IS. Anything naming an ECS service in
/// either domain uses one of these two constants rather than the literal.
pub const RESOURCE_TYPE_ECS_SERVICE: &str = "EcsService";

/// The `ResourceType` an Amazon RDS DB instance is analysed as (`RESOURCE_TYPE_VALUES`).
///
/// Not to be confused with [`SOURCE_TYPE_RDS_DB_INSTANCE`], which spells the same word in
/// the `RecommendationSourceType` domain a findings row carries.
pub const RESOURCE_TYPE_RDS_DB_INSTANCE: &str = "RdsDBInstance";

/// The `ResourceType` Aurora DB cluster storage is analysed as (`RESOURCE_TYPE_VALUES`).
pub const RESOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE: &str = "AuroraDBClusterStorage";

/// The account's Compute Optimizer enrollment: one record per account, as the service
/// models it (`GetEnrollmentStatus` takes no input and returns exactly one).
#[derive(Debug, Clone)]
pub struct Enrollment {
    /// The persisted `Status`. An account that never called `UpdateEnrollmentStatus` is
    /// not opted in, so this starts `Inactive`.
    pub status: String,
    /// Why the account is in that status.
    pub status_reason: Option<String>,
    /// Whether the last enrollment call included the organization's member accounts.
    pub member_accounts_enrolled: bool,
    /// When the enrollment record was last written; `None` until the first write.
    pub last_updated_timestamp: Option<DateTime>,
}

impl Default for Enrollment {
    fn default() -> Self {
        Self {
            status: STATUS_INACTIVE.to_string(),
            status_reason: None,
            member_accounts_enrolled: false,
            last_updated_timestamp: None,
        }
    }
}

/// The current instant, in seconds since the Unix epoch, on the same scale as every
/// timestamp this service stores and reports (`::aws_smithy_types::DateTime`).
///
/// Every clock-driven rule in this module reads the time through here — the enrollment
/// analysis that becomes due, the export job whose status moves on — so no two of them can
/// disagree about what "now" is or about the units they compare in.
pub fn now_secs() -> f64 {
    DateTime::from(::std::time::SystemTime::now()).as_secs_f64()
}

impl Enrollment {
    /// Whether the account is opted in. The operations that model
    /// `OptInRequiredException` ("The account is not opted in to Compute Optimizer")
    /// answer it whenever this is false.
    pub fn is_opted_in(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    /// Whether the analysis this enrollment started is due, i.e. whether
    /// [`ANALYSIS_DELAY`] has elapsed since the enrollment record was last written.
    /// An account that never enrolled has no analysis owed at all.
    ///
    /// The due instant is DERIVED from the stored `lastUpdatedTimestamp` and compared
    /// against the clock; nothing waits for it and no reader writes it.
    pub fn analysis_due(&self) -> bool {
        let now = now_secs();
        self.last_updated_timestamp
            .as_ref()
            .is_some_and(|written| now - written.as_secs_f64() >= ANALYSIS_DELAY.as_secs_f64())
    }

    /// Whether the analysis this enrollment started can be reported yet: the account is
    /// opted in AND that analysis is due.
    ///
    /// The two halves are ONE rule — Compute Optimizer reports recommendations for an
    /// ENROLLED account only, and only once the analysis that enrollment starts has been
    /// produced ([`ANALYSIS_DELAY`]) — so it is stated here once rather than conjoined again
    /// at each surface that serves the analysis. Every such surface asks this, so a reader, a
    /// summary and an export of one analysis cannot apply different halves of the rule and
    /// disagree about whether that analysis exists yet.
    pub fn analysis_visible(&self) -> bool {
        self.is_opted_in() && self.analysis_due()
    }

    /// When the analysis this enrollment started was generated: one analysis span
    /// ([`ANALYSIS_DELAY`]) after the enrollment record was written, and `None` for an account
    /// that never enrolled, which has no analysis to report a refresh for.
    ///
    /// This is the instant every recommendation reader of this service reports as its
    /// `lastRefreshTimestamp` and every export file states in its `LastRefreshTimestamp`
    /// column. Derived from the store rather than read off the clock, so every read of one
    /// analysis reports the same instant, and stated ONCE here so a reader and an export of
    /// the same analysis cannot disagree about it.
    pub fn last_analysis_refresh(&self) -> Option<DateTime> {
        let written = self.last_updated_timestamp.as_ref()?.as_secs_f64();
        Some(DateTime::from_secs_f64(
            written + ANALYSIS_DELAY.as_secs_f64(),
        ))
    }
}

/// Why an account is in the status its enrollment record holds — the `statusReason` the
/// account's own reader and every member account's row report. Derived from the stored
/// status, so no two of them can disagree about one record.
pub fn enrollment_status_reason(status: &str) -> String {
    if status == STATUS_ACTIVE {
        "The account is opted in to Compute Optimizer.".to_string()
    } else {
        "The account is opted out of Compute Optimizer.".to_string()
    }
}

// ---------------------------------------------------------------------------
// The organization's member accounts
//
// `GetEnrollmentStatusesForOrganization` "Returns the Compute Optimizer enrollment (opt-in)
// status of organization member accounts", so what it reports is one row per member account
// of the organization — never the account's own record, which `GetEnrollmentStatus` reports.
// Which accounts an organization HOLDS is recorded by AWS Organizations, so no operation of
// THIS API creates one ([`OrganizationMemberStore`]); what this API writes is their
// ENROLLMENT, through the `includeMemberAccounts` of an `UpdateEnrollmentStatus` call.
// ---------------------------------------------------------------------------

/// The one `EnrollmentFilterName` the model declares
/// (`crate::ops::types::ENROLLMENT_FILTER_NAME_VALUES`): "Specify `Status` to return
/// accounts with a specific enrollment status".
pub const ENROLLMENT_FILTER_STATUS: &str = "Status";

/// The `Status` a member account reports while the enrollment the management account asked
/// for is still reaching it (`crate::ops::types::STATUS_VALUES`).
pub const STATUS_PENDING: &str = "Pending";

/// How long a member account the management account has just enrolled reports
/// [`STATUS_PENDING`] for, before it reports the status that call asked for.
///
/// A member account's enrollment does not take effect at the instant the management
/// account's call returns — "an account might show a status of `Pending` because member
/// accounts of an organization require more time to be enrolled in the service"
/// (`AccountEnrollmentStatus$statusReason`) — and AWS documents no duration for it, so this
/// is the emulator's own span. It is deliberately NON-ZERO, so a caller that reads the
/// organization straight after enrolling it still sees the transient `Pending` rows, and
/// short enough that a caller can poll them through to the status it asked for. Derived
/// from the instant the enrollment was written and never waited for
/// ([`MemberAccountEnrollment::status_at`]).
pub const MEMBER_ENROLLMENT_DELAY: Duration = Duration::from_millis(500);

/// The reason a member account is `Pending`, in the service's own words: its enrollment is
/// still reaching it ([`MEMBER_ENROLLMENT_DELAY`]).
pub const MEMBER_ENROLLMENT_PENDING_REASON: &str =
    "Member accounts of the organization require more time to be enrolled in the service.";

/// One organization member account's Compute Optimizer enrollment, as
/// `GetEnrollmentStatusesForOrganization` reports it.
///
/// The status it REPORTS is not stored on its own: while the enrollment the management
/// account asked for is still reaching the member account the row reports
/// [`STATUS_PENDING`], which the clock makes due rather than any call
/// ([`Self::status_at`]) — so a reader reports the status the clock has made due without
/// writing to the store, and two readers of one member account can never disagree about
/// where its enrollment has got to.
#[derive(Debug, Clone)]
pub struct MemberAccountEnrollment {
    /// The member account's `AccountId` — "a 12-digit Amazon Web Services account ID".
    pub account_id: String,
    /// The `Status` its enrollment was last written to.
    pub status: String,
    /// Why it is in that status; `None` while nothing has been written for it.
    pub status_reason: Option<String>,
    /// When its enrollment was last written; `None` until the first write.
    pub last_updated_timestamp: Option<DateTime>,
    /// When the enrollment last written for it finishes reaching it, in seconds since the
    /// epoch ([`now_secs`]); `None` for a member account whose enrollment is settled.
    pub pending_until: Option<f64>,
}

impl MemberAccountEnrollment {
    /// A member account of the organization that nobody has enrolled: not opted in, with no
    /// reason and no last-updated timestamp, exactly as the account's own record starts.
    pub fn new(account_id: &str) -> Self {
        let enrollment = Enrollment::default();
        Self {
            account_id: account_id.to_string(),
            status: enrollment.status,
            status_reason: enrollment.status_reason,
            last_updated_timestamp: enrollment.last_updated_timestamp,
            pending_until: None,
        }
    }

    /// Record the enrollment the management account asked for at `now`: the status it asked
    /// for, reached once [`MEMBER_ENROLLMENT_DELAY`] has elapsed.
    pub fn enroll(&mut self, status: &str, now: f64) {
        self.status = status.to_string();
        self.status_reason = Some(enrollment_status_reason(status));
        self.last_updated_timestamp = Some(DateTime::from_secs_f64(now));
        self.pending_until = Some(now + MEMBER_ENROLLMENT_DELAY.as_secs_f64());
    }

    /// Whether the enrollment written for this member account is still reaching it at `now`.
    pub fn is_pending_at(&self, now: f64) -> bool {
        self.pending_until.is_some_and(|due| now < due)
    }

    /// The `Status` this member account reports at `now`.
    pub fn status_at(&self, now: f64) -> &str {
        if self.is_pending_at(now) {
            STATUS_PENDING
        } else {
            &self.status
        }
    }

    /// Whether this member account is opted in at `now` — what
    /// `GetEnrollmentStatus$numberOfMemberAccountsOptedIn` counts.
    pub fn is_opted_in_at(&self, now: f64) -> bool {
        self.status_at(now) == STATUS_ACTIVE
    }

    /// The member account as `GetEnrollmentStatusesForOrganization` reports it at `now`.
    pub fn to_status(&self, now: f64) -> AccountEnrollmentStatus {
        AccountEnrollmentStatus {
            account_id: Some(self.account_id.clone()),
            status: Some(self.status_at(now).to_string()),
            status_reason: if self.is_pending_at(now) {
                Some(MEMBER_ENROLLMENT_PENDING_REASON.to_string())
            } else {
                self.status_reason.clone()
            },
            last_updated_timestamp: self.last_updated_timestamp,
        }
    }
}

/// The organization's member accounts, keyed by account id, which is also the order a page
/// walks them in — so a page boundary a reader hands out stays where it was.
///
/// AWS Organizations is what records an account's membership of an organization, so NO
/// operation of this API creates a member account: this store is what
/// `GetEnrollmentStatusesForOrganization` filters and pages over, and an emulator that
/// records no organization holds none, which is the empty list that reader answers. What
/// this API does write is a stored member account's ENROLLMENT ([`Self::enroll_all`]).
#[derive(Debug, Default)]
pub struct OrganizationMemberStore {
    members: BTreeMap<String, MemberAccountEnrollment>,
}

impl OrganizationMemberStore {
    /// Record `member` as an account of the organization. Recording the same account id
    /// again replaces the stored record.
    pub fn insert(&mut self, member: MemberAccountEnrollment) {
        self.members.insert(member.account_id.clone(), member);
    }

    /// The record held for `account_id`, or `None` when the organization holds no such
    /// member account.
    pub fn get(&self, account_id: &str) -> Option<&MemberAccountEnrollment> {
        self.members.get(account_id)
    }

    /// Every member account, in account-id order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &MemberAccountEnrollment> {
        self.members.values()
    }

    /// How many member accounts the organization holds.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the organization holds no member account at all — an emulator that records
    /// no organization.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Enroll every member account at `status`, as of `now`: what an
    /// `UpdateEnrollmentStatus` call that "include[s] member accounts of the organization"
    /// asks for.
    pub fn enroll_all(&mut self, status: &str, now: f64) {
        for member in self.members.values_mut() {
            member.enroll(status, now);
        }
    }
}

/// The most account enrollment statuses one page carries, whatever `maxResults` asks for:
/// "You can specify up to 100 statuses to return with each request"
/// (`GetEnrollmentStatusesForOrganizationRequest$maxResults`, whose modelled range is the
/// wider 0..1000). Applied in the handler, so no number a request carries can size a
/// response or the allocation that builds it, whatever any other layer does or does not
/// refuse; it is also the page a request naming no size gets, plus a `nextToken` to walk
/// the rest.
pub const ENROLLMENT_STATUSES_MAX_PAGE_SIZE: usize = 100;

/// Check a `GetEnrollmentStatusesForOrganization` filter set against the model's own
/// vocabulary, returning the reason it is not valid.
///
/// `name` is an `EnrollmentFilterName` and the values a filter carries are the value set of
/// the member that name selects on: `Status` selects on `Status`, whose "valid values are
/// `Active`, `Inactive`, `Pending`, and `Failed`" (`EnrollmentFilter$values`). A name or a
/// value outside those sets is refused rather than silently matching nothing.
pub fn enrollment_filter_rejection(filters: &[EnrollmentFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                ENROLLMENT_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let vocabulary: &[&str] = match name {
            ENROLLMENT_FILTER_STATUS => STATUS_VALUES,
            _ => {
                return Some(format!(
                    "The value supplied for parameter filters is not valid: {name} is not a \
                     filter name, which must be one of [{}].",
                    ENROLLMENT_FILTER_NAME_VALUES.join(", ")
                ))
            }
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value of \
                 the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Does `row` satisfy every `GetEnrollmentStatusesForOrganization` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values;
/// several filters are ANDed, and a filter carrying no values constrains nothing. An
/// unrecognised name never reaches here — [`enrollment_filter_rejection`] refuses the
/// request first.
pub fn account_enrollment_matches_filters(
    row: &AccountEnrollmentStatus,
    filters: &[EnrollmentFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let member = match filter.name.as_deref() {
            Some(ENROLLMENT_FILTER_STATUS) => row.status.as_deref(),
            _ => None,
        };
        member.is_some_and(|member| values.iter().any(|value| value == member))
    })
}

/// The identity of a stored preference record: a resource type plus the scope it was
/// written at. A scope-less record is the account-level default, and sorts first because
/// `None` orders before `Some`.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PreferenceKey {
    /// The `ResourceType` the preference applies to.
    pub resource_type: String,
    /// The `ScopeName`, or `None` for the account-level default.
    pub scope_name: Option<String>,
    /// The `Scope` value, or `None` for the account-level default.
    pub scope_value: Option<String>,
}

impl PreferenceKey {
    /// The key `resource_type` + `scope` addresses. An empty `Scope` object (neither
    /// member set) addresses the same record as no scope at all.
    pub fn new(resource_type: &str, scope: Option<&Scope>) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            scope_name: scope.and_then(|s| s.name.clone()),
            scope_value: scope.and_then(|s| s.value.clone()),
        }
    }
}

/// One stored recommendation preference record: every preference field a
/// `PutRecommendationPreferences` call can set for one (resourceType, scope) key.
#[derive(Debug, Clone, Default)]
pub struct PreferenceRecord {
    /// The scope exactly as written, so a read reports it back unchanged.
    pub scope: Option<Scope>,
    /// The `ResourceType` this record was written for.
    pub resource_type: String,
    /// `EnhancedInfrastructureMetrics`.
    pub enhanced_infrastructure_metrics: Option<String>,
    /// `InferredWorkloadTypes`.
    pub inferred_workload_types: Option<String>,
    /// `ExternalMetricsPreference`.
    pub external_metrics_preference: Option<ExternalMetricsPreference>,
    /// `LookBackPeriodPreference`.
    pub look_back_period: Option<String>,
    /// `UtilizationPreferences`.
    pub utilization_preferences: Vec<UtilizationPreference>,
    /// `PreferredResources`.
    pub preferred_resources: Vec<PreferredResource>,
    /// `SavingsEstimationMode`.
    pub savings_estimation_mode: Option<String>,
}

impl PreferenceRecord {
    /// A record for a key that has nothing written to it yet.
    pub fn for_key(resource_type: &str, scope: Option<&Scope>) -> Self {
        Self {
            scope: scope.cloned(),
            resource_type: resource_type.to_string(),
            ..Self::default()
        }
    }

    /// Apply a `PutRecommendationPreferences` request: "Creates a new recommendation
    /// preference or updates an existing recommendation preference" — so a supplied
    /// field replaces the stored one and a field the request omits is left alone.
    pub fn apply(&mut self, input: &PutRecommendationPreferencesInput) {
        if input.enhanced_infrastructure_metrics.is_some() {
            self.enhanced_infrastructure_metrics = input.enhanced_infrastructure_metrics.clone();
        }
        if input.inferred_workload_types.is_some() {
            self.inferred_workload_types = input.inferred_workload_types.clone();
        }
        if input.external_metrics_preference.is_some() {
            self.external_metrics_preference = input.external_metrics_preference.clone();
        }
        if input.look_back_period.is_some() {
            self.look_back_period = input.look_back_period.clone();
        }
        if let Some(preferences) = &input.utilization_preferences {
            self.utilization_preferences = preferences.clone();
        }
        if let Some(resources) = &input.preferred_resources {
            self.preferred_resources = resources.clone();
        }
        if input.savings_estimation_mode.is_some() {
            self.savings_estimation_mode = input.savings_estimation_mode.clone();
        }
    }

    /// Clear the one preference `name` (a `RecommendationPreferenceName`) names.
    /// `false` when the name addresses no field on this record, which
    /// [`is_recommendation_preference_name`] refuses before anything is written.
    pub fn clear_preference(&mut self, name: &str) -> bool {
        match name {
            "EnhancedInfrastructureMetrics" => self.enhanced_infrastructure_metrics = None,
            "InferredWorkloadTypes" => self.inferred_workload_types = None,
            "ExternalMetricsPreference" => self.external_metrics_preference = None,
            "LookBackPeriodPreference" => self.look_back_period = None,
            "PreferredResources" => self.preferred_resources.clear(),
            "UtilizationPreferences" => self.utilization_preferences.clear(),
            _ => return false,
        }
        true
    }

    /// Whether any preference field is still set. A record with none left is dropped, so
    /// a later read neither reports it nor inherits from it.
    pub fn holds_any_preference(&self) -> bool {
        self.enhanced_infrastructure_metrics.is_some()
            || self.inferred_workload_types.is_some()
            || self.external_metrics_preference.is_some()
            || self.look_back_period.is_some()
            || self.savings_estimation_mode.is_some()
            || !self.utilization_preferences.is_empty()
            || !self.preferred_resources.is_empty()
    }

    /// This record as `GetRecommendationPreferences` reports it.
    pub fn to_detail(&self) -> RecommendationPreferencesDetail {
        RecommendationPreferencesDetail {
            scope: self.scope.clone(),
            resource_type: Some(self.resource_type.clone()),
            enhanced_infrastructure_metrics: self.enhanced_infrastructure_metrics.clone(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            external_metrics_preference: self.external_metrics_preference.clone(),
            look_back_period: self.look_back_period.clone(),
            utilization_preferences: (!self.utilization_preferences.is_empty())
                .then(|| self.utilization_preferences.clone()),
            preferred_resources: (!self.preferred_resources.is_empty()).then(|| {
                self.preferred_resources
                    .iter()
                    .map(effective_preferred_resource)
                    .collect()
            }),
            savings_estimation_mode: self.savings_estimation_mode.clone(),
        }
    }
}

/// Whether `name` is a `RecommendationPreferenceName`, i.e. one of the preference fields
/// [`PreferenceRecord::clear_preference`] can clear. Checked before any deletion, so a
/// request naming something else cannot have cleared the names that came before it.
pub fn is_recommendation_preference_name(name: &str) -> bool {
    PreferenceRecord::default().clear_preference(name)
}

/// A `PreferredResource` as the service reports it back: the stored include and exclude
/// lists plus `effectiveIncludeList`, "the expanded version of your preferred resource's
/// include list" — the preference "resolves any wildcard expressions and returns the
/// effective list of candidate resource type values".
///
/// A value that is no wildcard expands to itself, and so does the catch-all
/// [`INCLUDE_EVERY_RESOURCE_TYPE`], which already names every candidate. Expanding a
/// NARROWER wildcard (`m5*`) needs the EC2 instance-type catalog, which this emulator does
/// not hold, so the effective list is omitted rather than invented for one of those.
pub fn effective_preferred_resource(resource: &PreferredResource) -> EffectivePreferredResource {
    let include_list = resource.include_list.clone();
    let effective_include_list = include_list
        .as_ref()
        .filter(|values| {
            values
                .iter()
                .all(|value| value == INCLUDE_EVERY_RESOURCE_TYPE || !value.contains('*'))
        })
        .cloned();
    EffectivePreferredResource {
        name: resource.name.clone(),
        include_list,
        effective_include_list,
        exclude_list: resource.exclude_list.clone(),
    }
}

/// The only `PreferredResourceName` the service supports: "Compute Optimizer only supports
/// the customization of `Ec2InstanceTypes`" (`PreferredResource$name`).
pub const PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES: &str = "Ec2InstanceTypes";

/// The wildcard expression that includes every candidate resource type, i.e. the state
/// `PreferredResource$excludeList` describes when no preference narrows the candidates: "if
/// this isn't specified, all supported resources are included by default".
pub const INCLUDE_EVERY_RESOURCE_TYPE: &str = "*";

/// The `CustomizableMetricName` values, from the model's enum of the same name.
pub const METRIC_CPU_UTILIZATION: &str = "CpuUtilization";
/// See [`METRIC_CPU_UTILIZATION`].
pub const METRIC_MEMORY_UTILIZATION: &str = "MemoryUtilization";

/// The utilization preferences in force when no preference sets them, from the model's own
/// `PutRecommendationPreferences$utilizationPreferences` documentation: "When this
/// preference isn't specified, we use the following default values. CPU utilization:
/// `P99_5` for threshold, `PERCENT_20` for headroom. Memory utilization: `PERCENT_20` for
/// headroom." The threshold setting "isn't available for memory utilization", so the memory
/// entry carries headroom only.
pub fn default_utilization_preferences() -> Vec<UtilizationPreference> {
    vec![
        UtilizationPreference {
            metric_name: Some(METRIC_CPU_UTILIZATION.to_string()),
            metric_parameters: Some(CustomizableMetricParameters {
                threshold: Some("P99_5".to_string()),
                headroom: Some("PERCENT_20".to_string()),
            }),
        },
        UtilizationPreference {
            metric_name: Some(METRIC_MEMORY_UTILIZATION.to_string()),
            metric_parameters: Some(CustomizableMetricParameters {
                threshold: None,
                headroom: Some("PERCENT_20".to_string()),
            }),
        },
    ]
}

/// The preferred resources in force when no preference sets them: the one list name the
/// service supports, including every candidate resource type and excluding none, because
/// `PreferredResource$excludeList` documents that "if this isn't specified, all supported
/// resources are included by default". Projected through
/// [`effective_preferred_resource`], so the reported `effectiveIncludeList` is the
/// expansion of that include list and not a second statement of it.
pub fn default_preferred_resources() -> Vec<EffectivePreferredResource> {
    vec![effective_preferred_resource(&PreferredResource {
        name: Some(PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES.to_string()),
        include_list: Some(vec![INCLUDE_EVERY_RESOURCE_TYPE.to_string()]),
        exclude_list: Some(Vec::new()),
    })]
}

/// An ARN split into the fields the ARN grammar defines:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`. The resource part keeps
/// its own `:` and `/` separators, because every service words it its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedArn<'a> {
    /// The partition field (`aws`, `aws-cn`, …).
    pub partition: &'a str,
    /// The service namespace the resource belongs to (`ec2`, `rds`, …).
    pub service: &'a str,
    /// The region field, empty for a global resource.
    pub region: &'a str,
    /// The account that owns the resource.
    pub account_id: &'a str,
    /// Everything after the account field, separators included.
    pub resource: &'a str,
}

impl<'a> ParsedArn<'a> {
    /// The leading segment of the resource part — the resource KIND (`instance`,
    /// `volume`, `autoScalingGroup`, …), which is what names the resource's type.
    pub fn resource_kind(&self) -> &'a str {
        resource_kind_of(self.resource)
    }
}

/// The leading segment of an ARN's resource part — the resource KIND (`instance`, `volume`,
/// `natgateway`, …), which is what names the resource's type. The one rule for reading a
/// kind, so a record that keeps a resource part on its own reads it exactly as a record that
/// parses it out of an ARN does.
pub fn resource_kind_of(arn_resource: &str) -> &str {
    arn_resource.split(['/', ':']).next().unwrap_or_default()
}

/// A resource's ARN as the endpoint in `region` reports it, built from the REGION-FREE
/// identity a record keeps: the resource's service namespace, the account that owns it, and
/// everything after the account field of its ARN.
///
/// Compute Optimizer is a REGIONAL service: each regional endpoint analyses and reports the
/// resources of its own region, so a resource this service holds is a resource of the region
/// whose endpoint a request addressed, and every reader builds the ARN for the region its
/// request was signed for ([`request_region`]) rather than storing one.
pub fn resource_arn_in(
    arn_service: &str,
    region: &str,
    account_id: &str,
    arn_resource: &str,
) -> String {
    if account_id == ::engine::emulator_account_id() {
        ::engine::emulator_arn(arn_service, region, arn_resource)
    } else {
        // `emulator_arn` always names the emulator's own account; a record kept for another
        // account keeps that account in its ARN.
        format!("arn:aws:{arn_service}:{region}:{account_id}:{arn_resource}")
    }
}

/// A resource's REGION-FREE identity: the ARN fields every regional endpoint that reports it
/// agrees on. The ARNs of one resource at two regional endpoints share one identity, which is
/// what lets a record be held once and reported wherever it is asked about.
pub fn resource_identity(arn_service: &str, account_id: &str, arn_resource: &str) -> String {
    format!("{arn_service}:{account_id}:{arn_resource}")
}

/// The [`resource_identity`] of the resource `arn` names, or `None` when `arn` is not an ARN.
pub fn identity_of_arn(arn: &str) -> Option<String> {
    let parsed = parse_arn(arn)?;
    Some(resource_identity(
        parsed.service,
        parsed.account_id,
        parsed.resource,
    ))
}

/// Split `arn` into its six ARN fields, or `None` when it is not an ARN at all. The one
/// place this service decides what an ARN looks like, so every operation that takes one
/// accepts and refuses the same strings.
pub fn parse_arn(arn: &str) -> Option<ParsedArn<'_>> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || partition.is_empty() || service.is_empty() || resource.is_empty() {
        return None;
    }
    Some(ParsedArn {
        partition,
        service,
        region,
        account_id,
        resource,
    })
}

/// Whether `value` is "a 12-digit Amazon Web Services account ID" — the account-id shape
/// the service documents on `Scope$value`, and the one every operation of this service
/// that takes an account id checks against.
pub fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// What a reader of this service answers when a request asks about an account other than
/// the one this service answers for: the real service's constant refusal, which names
/// neither the member nor the account — it states the permission that is missing and how
/// an organization account gets it. ONE spelling for the whole crate, so no two readers
/// can drift apart on the same refusal.
pub const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If \
     this is an organization account, request the management account to enable service \
     access.";

/// What the recommendation-preference API answers when the ARN a request names is not one it
/// can resolve a resource from — "Only EC2 instance and Auto Scaling group ARNs are currently
/// supported" (`GetEffectiveRecommendationPreferences$resourceArn`, `Scope$value`).
///
/// The service's own constant refusal for the condition: it states that the ARN is not valid
/// and what to do about it, and names neither the member it arrived on nor the rejected value,
/// so caller-supplied input is never reflected back in the error body. ONE spelling for the
/// preference surface, so its reader and its writers cannot word the same refusal differently.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid Amazon Resource Name (ARN) specified. \
     Please check the resource name and try again.";

/// What `PutRecommendationPreferences` answers a preference for an Auto Scaling group asked
/// for at the organization or account level: "You cannot create recommendation preferences for
/// Auto Scaling groups at the organization and account levels. You can create recommendation
/// preferences for Auto Scaling groups only at the resource level by specifying a scope name
/// of ResourceArn and a scope value of the Auto Scaling group Amazon Resource Name (ARN)"
/// (`PutRecommendationPreferences$scope`).
///
/// The service's own constant refusal for the condition — it names the scope names that cannot
/// be used and the remediation, and takes nothing from the request.
pub const AUTO_SCALING_GROUP_SCOPE_MESSAGE: &str = "You cannot create a recommendation \
     preference for an Auto Scaling Group with the \"Organization\" or \"AccountId\" scope \
     name. Specify \"ResourceArn\" as the scope name, the Auto Scaling group ARN ID as the \
     scope value, and try again.";

/// What `UpdateEnrollmentStatus` answers a `status` a client cannot set: "The Pending and
/// Failed options cannot be used to update the enrollment status of an account"
/// (`UpdateEnrollmentStatus$status`).
///
/// The service's own constant refusal for the condition — it names the two statuses a client
/// may supply, and neither the member nor the rejected value.
pub const INVALID_ENROLLMENT_STATUS_MESSAGE: &str = "Invalid status, should be 'Active' or \
     'Inactive'.";

/// What this service answers a caller that asks for `includeMemberAccounts` — the
/// organization-wide switch — while it manages no organization.
///
/// That member is conditioned on the caller's role: it indicates whether to include the
/// resources of "all member accounts of the organization", and only an organization's
/// management account or delegated administrator has any. A caller that has none is refused
/// rather than answered as though an organization-wide call had been honoured; the refusal
/// names neither the member nor the account, it states the two reasons the caller cannot ask
/// for member accounts. ONE spelling for the whole crate, so no two operations that take the
/// member word the same refusal differently.
pub const INCLUDE_MEMBER_ACCOUNTS_MESSAGE: &str = "Cannot include member accounts. This could \
     be because you are not an AWS Organization user or you are not the master account in the \
     organization.";

/// Whether `account_id` — an id a request named, or the account field of an ARN it carried
/// — names an account OTHER than the one this service answers for.
///
/// This emulator presents exactly one standalone account ([`engine::emulator_account_id`])
/// and records no organization, so a resource owned by any other account is one no request
/// can be answered about: a reader refuses it with [`CROSS_ACCOUNT_MESSAGE`] rather than
/// resolving an answer under that other account. ONE test for the whole crate, so an
/// account id and the account field of an ARN are judged by the same rule.
pub fn is_foreign_account(account_id: &str) -> bool {
    account_id != ::engine::emulator_account_id()
}

/// The account whose analysis a read request asks about — the one id `accountIds` carries,
/// or the caller's own account when it carries none — or, as `Err`, why the request cannot
/// be answered at all.
///
/// `accountIds` is the MANAGEMENT-account mechanism for naming a member account: "If your
/// account is the management account of an organization, use this parameter to specify the
/// member account for which you want to return \<resource\> recommendations. Only one
/// account ID can be specified per request", and an account id is "a 12-digit Amazon Web
/// Services account ID" (`Scope$value`). This emulator presents exactly one standalone
/// account ([`engine::emulator_account_id`]) and records no organization, so a well-formed
/// id naming any OTHER account asks about an account this service cannot report on: it is
/// refused with [`CROSS_ACCOUNT_MESSAGE`], never used as a filter key and answered with an
/// empty selection a client would read as "that account has no recommendations".
///
/// Shared by every reader that takes `accountIds` so one cannot accept what another
/// refuses.
pub fn requested_account_id(account_ids: Option<&[String]>) -> Result<String, String> {
    let account_ids = account_ids.unwrap_or_default();
    if account_ids.len() > 1 {
        return Err(format!(
            "The value supplied for parameter accountIds is not valid: only one account ID can \
             be specified per request, not {}.",
            account_ids.len(),
        ));
    }
    let Some(account_id) = account_ids.first() else {
        return Ok(::engine::emulator_account_id().to_string());
    };
    if !is_account_id(account_id) {
        return Err(format!(
            "The value supplied for parameter accountIds is not valid: {account_id} is not a \
             12-digit Amazon Web Services account ID."
        ));
    }
    if is_foreign_account(account_id) {
        return Err(CROSS_ACCOUNT_MESSAGE.to_string());
    }
    Ok(account_id.clone())
}

/// The region an ARN names when the request carries no SigV4 credential scope to read one
/// from: the region the emulator's own shared AWS config names, so an unsigned call and a
/// default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped and Compute Optimizer is a regional service, so the
/// region in a request's SigV4 credential scope is the region whose resources it is asking
/// about — and the one the ARNs in its answer name.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// An EC2 instance or Auto Scaling group ARN, split into the parts the preference rules
/// read: the account that owns the resource and the `ResourceType` it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedResourceArn<'a> {
    /// The ARN's account-id field: the account that owns the resource. A reader compares it
    /// with the account this service answers for ([`is_foreign_account`]) and refuses an
    /// ARN naming any other account's resource; it is never what an `AccountId`-scoped
    /// preference is matched against, because only this account's resources are resolved.
    pub account_id: &'a str,
    /// The `ResourceType` the ARN names.
    pub resource_type: &'static str,
}

/// Parse an ARN the preference API accepts, or `None` for anything else: "Only EC2
/// instance and Auto Scaling group ARNs are currently supported" (`Scope$value`,
/// `GetEffectiveRecommendationPreferences$resourceArn`).
pub fn parse_resource_arn(arn: &str) -> Option<ParsedResourceArn<'_>> {
    let parsed = parse_arn(arn)?;
    let resource_type = match (parsed.service, parsed.resource_kind()) {
        ("ec2", "instance") => RESOURCE_TYPE_EC2_INSTANCE,
        ("autoscaling", "autoScalingGroup") => RESOURCE_TYPE_AUTO_SCALING_GROUP,
        _ => return None,
    };
    Some(ParsedResourceArn {
        account_id: parsed.account_id,
        resource_type,
    })
}

/// Check a `Scope` against the rules the model's `Scope` shape documents, returning the
/// reason it is not valid. Shared by every operation that takes a scope so one cannot
/// accept a scope another refuses.
///
/// * `name` and `value` are set together — a scope is a name/value pair.
/// * `Organization` — "The `value` must be `ALL_ACCOUNTS`".
/// * `AccountId` — "The `value` must be a 12-digit Amazon Web Services account ID".
/// * `ResourceArn` — "The `value` must be the Amazon Resource Name (ARN) of an EC2
///   instance or an Auto Scaling group".
pub fn scope_rejection(scope: Option<&Scope>) -> Option<String> {
    let scope = scope?;
    match (scope.name.as_deref(), scope.value.as_deref()) {
        (None, None) => None,
        (Some(name), None) => Some(format!(
            "The value supplied for parameter scope is not valid: a scope value must be \
             supplied with the scope name {name}."
        )),
        (None, Some(value)) => Some(format!(
            "The value supplied for parameter scope is not valid: a scope name must be \
             supplied with the scope value {value}."
        )),
        (Some(SCOPE_ORGANIZATION), Some(value)) if value != ORGANIZATION_SCOPE_VALUE => {
            Some(format!(
                "The value supplied for parameter scope is not valid: the value of an \
                 {SCOPE_ORGANIZATION} scope must be {ORGANIZATION_SCOPE_VALUE}, not {value}."
            ))
        }
        (Some(SCOPE_ACCOUNT_ID), Some(value)) if !is_account_id(value) => Some(format!(
            "The value supplied for parameter scope is not valid: the value of an \
             {SCOPE_ACCOUNT_ID} scope must be a 12-digit Amazon Web Services account ID, \
             not {value}."
        )),
        (Some(SCOPE_RESOURCE_ARN), Some(value)) if parse_resource_arn(value).is_none() => {
            // The same ARN the preference reader resolves a resource from, so the same
            // refusal it answers an ARN it cannot resolve with
            // ([`INVALID_RESOURCE_ARN_MESSAGE`]).
            Some(INVALID_RESOURCE_ARN_MESSAGE.to_string())
        }
        (Some(_), Some(_)) => None,
    }
}

/// How specifically a stored key applies to one resource, smallest first: a resource-level
/// preference beats an account-level one, which beats the account default, which beats the
/// organization-level one. `None` when the key does not apply to this resource at all.
///
/// This is the precedence `GetEffectiveRecommendationPreferences` resolves with —
/// "Considers all applicable preferences that you might have set at the resource, account,
/// and organization level".
fn scope_precedence(key: &PreferenceKey, arn: &str, account_id: &str) -> Option<u8> {
    match (key.scope_name.as_deref(), key.scope_value.as_deref()) {
        (Some(SCOPE_RESOURCE_ARN), Some(value)) if value == arn => Some(0),
        _ => account_scope_precedence(key, account_id),
    }
}

/// [`scope_precedence`] for a resource no `ResourceArn` scope can name — a `ResourceArn`
/// scope's "value must be the Amazon Resource Name (ARN) of an EC2 instance or an Auto
/// Scaling group" (`Scope$value`), so for any other resource type only the account-level
/// and organization-level records apply, in the same order they do above.
fn account_scope_precedence(key: &PreferenceKey, account_id: &str) -> Option<u8> {
    match (key.scope_name.as_deref(), key.scope_value.as_deref()) {
        (Some(SCOPE_ACCOUNT_ID), Some(value)) if value == account_id => Some(1),
        (None, _) => Some(2),
        (Some(SCOPE_ORGANIZATION), _) => Some(3),
        _ => None,
    }
}

/// The opaque `nextToken` `GetRecommendationPreferences` hands out: the key the next page
/// starts at, hex-encoded so a client cannot read or construct one.
pub fn encode_page_token(key: &PreferenceKey) -> String {
    let encoded =
        ::serde_json::json!([key.resource_type, key.scope_name, key.scope_value]).to_string();
    ::hex::encode(encoded)
}

/// The key a `nextToken` this service issued stands for, or `None` for a token it did not.
pub fn decode_page_token(token: &str) -> Option<PreferenceKey> {
    let bytes = ::hex::decode(token).ok()?;
    let (resource_type, scope_name, scope_value) =
        ::serde_json::from_slice::<(String, Option<String>, Option<String>)>(&bytes).ok()?;
    Some(PreferenceKey {
        resource_type,
        scope_name,
        scope_value,
    })
}

// ---------------------------------------------------------------------------
// Amazon EBS volumes: the inventory Compute Optimizer observes, and the volume
// recommendations it derives from it.
// ---------------------------------------------------------------------------

/// The `ResourceType` an EBS volume preference is written for, beside
/// [`RESOURCE_TYPE_EC2_INSTANCE`]. The `RecommendationSourceType` of the same spelling,
/// which the findings catalog records, is [`SOURCE_TYPE_EBS_VOLUME`].
pub const RESOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";

/// The one `EBSFilterName` value the model declares, and so the one filter name a volume
/// recommendation request can carry (`smithy_query query="shape" shape="EBSFilterName"`;
/// the request boundary refuses every other name against that closed enum, including the
/// `tag:key` and `tag-key` names `EBSFilter$name`'s prose mentions).
pub const EBS_FILTER_NAME_FINDING: &str = "Finding";

/// Check an `EBSFilters` list against the vocabulary the model declares for it, returning
/// the reason it is not valid.
///
/// `name` is an `EBSFilterName` and the values a filter carries are the value set of the
/// member that name selects on — `Finding` selects on `EBSFinding`. A name or a value
/// outside those sets is refused rather than silently matching nothing, and so is a filter
/// that names no member to select on at all: the same rule [`rds_filter_rejection`] applies
/// to the other recommendation filters of this service. Stated once here, so the volume
/// recommendations READER and the volume recommendations EXPORT admit exactly the same
/// filter set.
pub fn ebs_filter_rejection(filters: &[EBSFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                E_B_S_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = ebs_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                E_B_S_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set an `EBSFilterName` selects on — `Finding` selects on `EBSFinding` — or
/// `None` for a name the model does not declare, which is what makes
/// [`ebs_filter_rejection`] refuse it: the name check and the value set are then one
/// decision, as they are in [`rds_filter_vocabulary`].
fn ebs_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        EBS_FILTER_NAME_FINDING => Some(E_B_S_FINDING_VALUES),
        _ => None,
    }
}

/// The `EBSMetricName` values, in the order
/// [`ObservedVolume::utilization_metrics`] reports them
/// (`smithy_query query="shape" shape="EBSMetricName"`).
pub const EBS_METRIC_READ_OPS_PER_SECOND: &str = "VolumeReadOpsPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_WRITE_OPS_PER_SECOND: &str = "VolumeWriteOpsPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_READ_BYTES_PER_SECOND: &str = "VolumeReadBytesPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_WRITE_BYTES_PER_SECOND: &str = "VolumeWriteBytesPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_IOPS_EXCEEDED: &str = "VolumeIOPSExceeded";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_THROUGHPUT_EXCEEDED: &str = "VolumeThroughputExceeded";

/// The volume type every EBS volume recommendation option this service builds is of:
/// gp3 is the current-generation general purpose SSD type, whose performance is
/// provisioned independently of size, so one option expresses both a performance change
/// and a cost change.
pub const VOLUME_TYPE_GP3: &str = "gp3";

/// gp3's included performance, from the Amazon EBS volume types documentation: a gp3
/// volume includes 3,000 IOPS and 125 MiB/s of throughput at every size, and can be
/// provisioned up to 16,000 IOPS and 1,000 MiB/s, at most 500 IOPS per GiB and at most
/// 0.25 MiB/s of throughput per provisioned IOPS.
pub const GP3_INCLUDED_IOPS: i32 = 3_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_INCLUDED_THROUGHPUT_MIB: i32 = 125;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_IOPS: i32 = 16_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_THROUGHPUT_MIB: i32 = 1_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_IOPS_PER_GIB: i32 = 500;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_THROUGHPUT_MIB_PER_IOPS: f64 = 0.25;

/// The headroom a recommendation option is sized with: the option has to carry the
/// observed peak with room to spare, or adopting it would trade cost for a performance
/// risk. The service documents no headroom for EBS volumes, so this is the emulator's
/// own, taken from the only headroom the service does document — "`PERCENT_20` for
/// headroom" (`PutRecommendationPreferences$utilizationPreferences`).
pub const RECOMMENDATION_HEADROOM: f64 = 0.20;

/// Bytes in a MiB: EBS reports volume throughput in MiB/s and CloudWatch reports the
/// `VolumeRead/WriteBytesPerSecond` metrics in bytes per second.
pub const BYTES_PER_MIB: f64 = 1_048_576.0;

/// One observation of a volume's demand: the per-second read and write rates measured
/// over one interval of the period Compute Optimizer analyses.
#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeSample {
    /// How many days before the recommendation's last refresh this sample was taken, so
    /// the look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// `VolumeReadOpsPerSecond`.
    pub read_ops_per_second: f64,
    /// `VolumeWriteOpsPerSecond`.
    pub write_ops_per_second: f64,
    /// `VolumeReadBytesPerSecond`.
    pub read_bytes_per_second: f64,
    /// `VolumeWriteBytesPerSecond`.
    pub write_bytes_per_second: f64,
}

impl VolumeSample {
    /// The I/O operations per second this sample demanded, read plus write — what a
    /// volume's IOPS capacity has to carry.
    pub fn iops(&self) -> f64 {
        self.read_ops_per_second + self.write_ops_per_second
    }

    /// The throughput this sample demanded in MiB/s, read plus write — what a volume's
    /// throughput capacity has to carry.
    pub fn throughput_mib_per_second(&self) -> f64 {
        (self.read_bytes_per_second + self.write_bytes_per_second) / BYTES_PER_MIB
    }
}

/// One Amazon EBS volume Compute Optimizer observes: its configuration, its tags, and
/// the metric samples a recommendation for it is computed from. This is the OBSERVATION;
/// everything a `VolumeRecommendation` reports is derived from it, so no two readers of
/// the same volume can disagree.
#[derive(Debug, Clone)]
pub struct ObservedVolume {
    /// The volume's EC2 volume id, which is its key in the inventory. The ARN a reader
    /// reports is built from it and the region the request was signed for
    /// ([`ObservedVolume::volume_arn`]), because Compute Optimizer is a REGIONAL service:
    /// the volumes it reports are the ones in the Region the caller addressed.
    pub volume_id: String,
    /// The account that owns the volume.
    pub account_id: String,
    /// The volume's configuration as EC2 reports it: type, size, and the baseline and
    /// burst IOPS and throughput that type and size deliver.
    pub configuration: VolumeConfiguration,
    /// The volume's tags, as key/value pairs, in the order EC2 reports them.
    pub tags: Vec<(String, String)>,
    /// How many days Compute Optimizer has been observing this volume. A look-back
    /// preference longer than this cannot be honoured, so the analysed period is the
    /// shorter of the two.
    pub days_observed: f64,
    /// The metric samples observed, most recent first.
    pub samples: Vec<VolumeSample>,
}

impl ObservedVolume {
    /// This volume's ARN in `region`: `arn:aws:ec2:<region>:<account>:volume/<id>`.
    pub fn volume_arn(&self, region: &str) -> String {
        ::engine::emulator_arn("ec2", region, &format!("volume/{}", self.volume_id))
    }

    /// The samples the look-back period in force analyses: those taken within
    /// `look_back_days` of the last refresh.
    fn analysed_samples(&self, look_back_days: f64) -> impl Iterator<Item = &VolumeSample> {
        self.samples
            .iter()
            .filter(move |sample| sample.days_ago <= look_back_days)
    }

    /// The peak of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    fn peak(&self, look_back_days: f64, of: fn(&VolumeSample) -> f64) -> f64 {
        self.analysed_samples(look_back_days)
            .map(of)
            .fold(0.0_f64, f64::max)
    }

    /// How many analysed samples `exceeded` answers true for.
    fn samples_exceeding(
        &self,
        look_back_days: f64,
        exceeded: impl Fn(&VolumeSample) -> bool,
    ) -> f64 {
        self.analysed_samples(look_back_days)
            .filter(|sample| exceeded(sample))
            .count() as f64
    }

    /// "The number of days for which utilization metrics were analyzed for the volume"
    /// (`VolumeRecommendation$lookBackPeriodInDays`): the preferred look-back period
    /// ([`look_back_period_in_days`]), or however long this volume has been observed when
    /// that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// The metrics `VolumeRecommendation$utilizationMetrics` reports: every
    /// `EBSMetricName` the model declares, each the peak of the analysed samples — and,
    /// for the two "exceeded" metrics, the number of analysed samples whose demand went
    /// past the capacity the volume's configuration delivers.
    pub fn utilization_metrics(&self, look_back_days: f64) -> Vec<EBSUtilizationMetric> {
        let iops_capacity = configured_iops_capacity(&self.configuration);
        let throughput_capacity = configured_throughput_capacity(&self.configuration);
        let metric = |name: &str, value: f64| EBSUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
            value,
        };
        vec![
            metric(
                EBS_METRIC_READ_OPS_PER_SECOND,
                self.peak(look_back_days, |sample| sample.read_ops_per_second),
            ),
            metric(
                EBS_METRIC_WRITE_OPS_PER_SECOND,
                self.peak(look_back_days, |sample| sample.write_ops_per_second),
            ),
            metric(
                EBS_METRIC_READ_BYTES_PER_SECOND,
                self.peak(look_back_days, |sample| sample.read_bytes_per_second),
            ),
            metric(
                EBS_METRIC_WRITE_BYTES_PER_SECOND,
                self.peak(look_back_days, |sample| sample.write_bytes_per_second),
            ),
            metric(
                EBS_METRIC_IOPS_EXCEEDED,
                self.samples_exceeding(look_back_days, |sample| sample.iops() > iops_capacity),
            ),
            metric(
                EBS_METRIC_THROUGHPUT_EXCEEDED,
                self.samples_exceeding(look_back_days, |sample| {
                    sample.throughput_mib_per_second() > throughput_capacity
                }),
            ),
        ]
    }

    /// The configuration this service recommends for the volume: gp3 at the same size,
    /// with IOPS and throughput provisioned to carry the analysed peak plus
    /// [`RECOMMENDATION_HEADROOM`], inside gp3's documented limits. A volume whose peak
    /// already fits gp3's included performance gets gp3's included performance.
    pub fn recommended_configuration(&self, look_back_days: f64) -> VolumeConfiguration {
        let size = self.configuration.volume_size.max(0);
        let headroom = 1.0 + RECOMMENDATION_HEADROOM;

        let peak_iops = self.peak(look_back_days, VolumeSample::iops);
        let iops_ceiling = GP3_MAX_IOPS
            .min(size.saturating_mul(GP3_MAX_IOPS_PER_GIB))
            .max(GP3_INCLUDED_IOPS);
        let iops = ((peak_iops * headroom).ceil() as i32).clamp(GP3_INCLUDED_IOPS, iops_ceiling);

        let peak_throughput = self.peak(look_back_days, VolumeSample::throughput_mib_per_second);
        let throughput_ceiling = GP3_MAX_THROUGHPUT_MIB
            .min((f64::from(iops) * GP3_MAX_THROUGHPUT_MIB_PER_IOPS) as i32)
            .max(GP3_INCLUDED_THROUGHPUT_MIB);
        let throughput = ((peak_throughput * headroom).ceil() as i32)
            .clamp(GP3_INCLUDED_THROUGHPUT_MIB, throughput_ceiling);

        VolumeConfiguration {
            volume_type: Some(VOLUME_TYPE_GP3.to_string()),
            volume_size: size,
            volume_baseline_iops: iops,
            // gp3 performance is provisioned rather than bursted — a gp3 volume uses no
            // burst credits — so the option delivers its baseline and nothing above it.
            volume_burst_iops: 0,
            volume_baseline_throughput: throughput,
            volume_burst_throughput: 0,
            // A recommendation changes a volume's type and provisioned performance; the
            // volume stays attached where it was, so whether it is a root volume is a
            // property of the volume, not of the option.
            root_volume: self.configuration.root_volume,
        }
    }

    /// How close the analysed peak came to what `configuration` sustains, as a
    /// `PerformanceRisk` on the model's 0..4 scale: 0 while the peak stays under a
    /// quarter of the sustained capacity, 4 once it reaches 125% of it. "The higher the
    /// risk, the more likely the current EBS volume doesn't have sufficient capacity"
    /// (`VolumeRecommendation$currentPerformanceRisk`), so the score rises with that
    /// ratio and is measured against the BASELINE: burst capacity is not sustained.
    pub fn performance_risk(
        &self,
        configuration: &VolumeConfiguration,
        look_back_days: f64,
    ) -> f64 {
        let baseline_iops = f64::from(configuration.volume_baseline_iops.max(0));
        let baseline_throughput = f64::from(configuration.volume_baseline_throughput.max(0));
        let iops_ratio = demand_ratio(self.peak(look_back_days, VolumeSample::iops), baseline_iops);
        let throughput_ratio = demand_ratio(
            self.peak(look_back_days, VolumeSample::throughput_mib_per_second),
            baseline_throughput,
        );
        performance_risk_score_of_ratio(iops_ratio.max(throughput_ratio))
    }

    /// The volume's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        modelled_tags(&self.tags)
    }

    /// The volume recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every figure is computed here from the stored configuration and samples: the
    /// metrics are the analysed peaks, the option is the configuration those peaks ask
    /// for, each cost comes from [`volume_monthly_cost`], and the finding is
    /// `NotOptimized` exactly when the option would either carry a peak the current
    /// configuration does not sustain or cost less than it — "a volume is considered not
    /// optimized when Compute Optimizer identifies a recommendation that can provide
    /// better performance for your workload", and `Optimized` otherwise, where "Compute
    /// Optimizer might [still] recommend a new generation volume type"
    /// (`VolumeRecommendation$finding`).
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> VolumeRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let current = self.configuration.clone();
        let recommended = self.recommended_configuration(look_back_days);
        let current_risk = self.performance_risk(&current, look_back_days);
        let option_risk = self.performance_risk(&recommended, look_back_days);
        let savings = self.savings(look_back_days);

        VolumeRecommendation {
            volume_arn: Some(self.volume_arn(region)),
            account_id: Some(self.account_id.clone()),
            current_configuration: Some(current),
            finding: Some(self.finding(look_back_days).to_string()),
            utilization_metrics: Some(self.utilization_metrics(look_back_days)),
            look_back_period_in_days: look_back_days,
            volume_recommendation_options: Some(vec![VolumeRecommendationOption {
                configuration: Some(recommended),
                performance_risk: option_risk,
                // One option, so it is the first-ranked one. "Rank 1 is considered the
                // best" (`VolumeRecommendationOption$rank`).
                rank: 1,
                savings_opportunity: savings.map(|(percentage, value)| SavingsOpportunity {
                    savings_opportunity_percentage: percentage,
                    estimated_monthly_savings: Some(EstimatedMonthlySavings {
                        currency: Some(CURRENCY_USD.to_string()),
                        value,
                    }),
                }),
                // This emulator prices a volume from the public price table in
                // [`volume_monthly_cost`] and records no negotiated discount for storage,
                // so the after-discount figures are the before-discount ones — which is
                // what the reported [`SAVINGS_ESTIMATION_SOURCE`] says.
                savings_opportunity_after_discounts: savings.map(|(percentage, value)| {
                    EBSSavingsOpportunityAfterDiscounts {
                        savings_opportunity_percentage: percentage,
                        estimated_monthly_savings: Some(EBSEstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value,
                        }),
                    }
                }),
            }]),
            last_refresh_timestamp,
            current_performance_risk: Some(performance_risk_band(current_risk).to_string()),
            effective_recommendation_preferences: Some(EBSEffectiveRecommendationPreferences {
                savings_estimation_mode: Some(EBSSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
                look_back_period: Some(look_back_period.to_string()),
            }),
            tags: self.tag_list(),
        }
    }

    /// The volume's `EBSFinding` over the analysed period: `NotOptimized` exactly when the
    /// recommended configuration would either carry a peak the current one does not
    /// sustain or cost less than it, `Optimized` otherwise. `EBSFinding`'s two values are
    /// [`FINDING_NOT_OPTIMIZED`] and [`FINDING_OPTIMIZED`].
    pub fn finding(&self, look_back_days: f64) -> &'static str {
        let recommended = self.recommended_configuration(look_back_days);
        let safer = self.performance_risk(&recommended, look_back_days)
            < self.performance_risk(&self.configuration, look_back_days);
        let cheaper = match (
            volume_monthly_cost(&self.configuration),
            volume_monthly_cost(&recommended),
        ) {
            (Some(current), Some(option)) => option < current,
            _ => false,
        };
        if safer || cheaper {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// What adopting the recommended configuration saves per month and that saving as a
    /// percentage of what the volume costs today, or `None` for a volume type this
    /// emulator holds no price for ([`volume_monthly_cost`]) — whose recommendation then
    /// carries no savings figure rather than an invented one.
    pub fn savings(&self, look_back_days: f64) -> Option<(f64, f64)> {
        let current = volume_monthly_cost(&self.configuration)?;
        let option = volume_monthly_cost(&self.recommended_configuration(look_back_days))?;
        let saved = round_hundredths((current - option).max(0.0));
        Some((percentage_of(saved, current), saved))
    }

    /// This volume as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it, under the look-back period in force.
    ///
    /// The projection is what keeps one volume's analysis in ONE place: the summary reader
    /// and `GetEBSVolumeRecommendations` both answer from this observation, so they cannot
    /// disagree about a volume's finding, its performance risk or what adopting its
    /// recommendation saves. A volume recommendation reports no finding reason codes
    /// (`VolumeRecommendation` declares no such member) and infers no workload type, so
    /// the row carries neither.
    pub fn to_analysed_resource(&self, look_back_period: &str) -> AnalysedResource {
        let look_back_days = self.analysed_days(look_back_period);
        let current_monthly_cost = volume_monthly_cost(&self.configuration).unwrap_or_default();
        let recommended_monthly_cost =
            volume_monthly_cost(&self.recommended_configuration(look_back_days))
                .unwrap_or(current_monthly_cost);
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_EBS_VOLUME.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a volume is its EC2 volume id.
            arn_service: "ec2".to_string(),
            arn_resource: format!("volume/{}", self.volume_id),
            finding: self.finding(look_back_days).to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: performance_risk_band(
                self.performance_risk(&self.configuration, look_back_days),
            )
            .to_string(),
            current_monthly_cost,
            recommended_monthly_cost,
            inferred_workload_types: Vec::new(),
            // A volume is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}

/// The opaque `nextToken` a recommendation reader hands out: the ARN of the resource the
/// next page starts at, hex-encoded so a client can neither read nor construct one.
pub fn encode_resource_page_token(resource_arn: &str) -> String {
    ::hex::encode(::serde_json::json!({ "startAt": resource_arn }).to_string())
}

/// The resource ARN a `nextToken` this service issued stands for, or `None` for a token
/// it did not — including a token issued for a different page sequence, whose encoded
/// shape is not this one.
pub fn decode_resource_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok()?;
    decoded
        .get("startAt")?
        .as_str()
        .map(::std::string::ToString::to_string)
}

// ===========================================================================
// The Auto Scaling group analysis the recommendation readers report
// ===========================================================================

/// How long after an account opts in the analysis of its resources is due.
///
/// Compute Optimizer derives recommendations from the utilization metrics it observes
/// AFTER an account enrolls — "after you opt in, it can take up to 24 hours for
/// Compute Optimizer to deliver your recommendations" — so an account that has just
/// enrolled has none yet. The emulator owes that transition on an emulator-scale span
/// rather than a real day (chosen, not documented), so a caller that reads straight
/// after opting in still observes the pre-analysis state. The instant is DERIVED from
/// the enrollment record's `lastUpdatedTimestamp` and never waited for.
pub const ANALYSIS_DELAY: Duration = Duration::from_millis(500);

/// Hours in a month, the factor that turns an hourly on-demand price into the monthly
/// figures Compute Optimizer reports (`estimatedMonthlySavings`). 730 = 365 × 24 / 12,
/// the convention Amazon Web Services' own pricing pages use.
pub const HOURS_PER_MONTH: f64 = 730.0;

/// The `MetricName` values an Auto Scaling group recommendation reports. "The `Cpu` and
/// `Memory` metrics are the only projected utilization metrics returned"
/// (`AutoScalingGroupRecommendationOption$projectedUtilizationMetrics`), and both are
/// percentages ("Units: Percent", `UtilizationMetric$name`).
pub const METRIC_CPU: &str = "Cpu";
/// See [`METRIC_CPU`].
pub const METRIC_MEMORY: &str = "Memory";

/// The one `MetricStatistic` this API reports: "The Compute Optimizer API, Command Line
/// Interface (CLI), and SDKs return utilization metrics using only the `Maximum`
/// statistic" (`UtilizationMetric$statistic`).
pub const METRIC_STATISTIC_MAXIMUM: &str = "Maximum";

/// The `AsgType` values, from the model's enum of the same name: "whether the EC2 Auto
/// Scaling group has a single instance type or a mixed instance type configuration".
pub const ASG_TYPE_SINGLE_INSTANCE_TYPE: &str = "SingleInstanceType";
/// See [`ASG_TYPE_SINGLE_INSTANCE_TYPE`].
pub const ASG_TYPE_MIXED_INSTANCE_TYPES: &str = "MixedInstanceTypes";

/// The `CpuVendorArchitecture` values (`shared_types::CPU_VENDOR_ARCHITECTURE_VALUES`).
/// `CURRENT` is a candidate of the same CPU vendor and architecture as the group runs
/// today; `AWS_ARM64` is a Graviton candidate — "when you specify `AWS_ARM64` with a
/// `GetAutoScalingGroupRecommendations` request, Compute Optimizer returns
/// recommendations that consist of Graviton instance types only".
pub const CPU_ARCHITECTURE_CURRENT: &str = "CURRENT";
/// See [`CPU_ARCHITECTURE_CURRENT`].
pub const CPU_ARCHITECTURE_AWS_ARM64: &str = "AWS_ARM64";

/// Whether an option of `architecture` is one a request asking for `requested` CPU vendor
/// architectures admits: "when you specify `AWS_ARM64` with a … request, Compute Optimizer
/// returns [or exports] recommendations that consist of Graviton instance types only"
/// (`RecommendationPreferences$cpuVendorArchitectures`). A request that asks for none
/// restricts nothing.
///
/// The ONE rule in this service, so a recommendation READER and an EXPORT of the same
/// recommendations cannot admit different options for the same preference.
pub fn architecture_admitted(requested: &[String], architecture: &str) -> bool {
    requested.is_empty() || requested.iter().any(|value| value == architecture)
}

/// The `cpuVendorArchitectures` an answer states for a resource whose admitted options are
/// `options`: "describes the CPU vendor and architecture for … recommendations" — the
/// architectures the reported options actually consist of, each once, in option order.
///
/// Stated here once for the same reason [`architecture_admitted`] is: a reader and an export of
/// one resource's recommendation report the same architectures.
pub fn stated_architectures<'a>(options: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut architectures: Vec<String> = Vec::new();
    for architecture in options {
        if !architectures.iter().any(|held| held == architecture) {
            architectures.push(architecture.to_string());
        }
    }
    architectures
}

/// The `SavingsEstimationMode` preference value that activates reporting savings net of
/// Savings Plans and Reserved Instances discounts: "Specify the `AfterDiscounts` status
/// to activate the preference, or specify `BeforeDiscounts` to deactivate" it.
pub const SAVINGS_ESTIMATION_AFTER_DISCOUNTS: &str = "AfterDiscounts";

/// The `InstanceSavingsEstimationModeSource` the emulator's figures come from: every
/// money figure a recommendation reports is computed from the public on-demand prices
/// stored with the analysis ([`GroupConfiguration::hourly_price`]), not from an
/// account's own Cost Explorer or Cost Optimization Hub data.
pub const SAVINGS_ESTIMATION_SOURCE: &str = "PublicPricing";

/// The `InferredWorkloadTypesPreference` value that leaves the inference ON: "The
/// inferred workload type feature is active by default. To deactivate it, create a
/// recommendation preference" (`PutRecommendationPreferences$inferredWorkloadTypes`).
pub const INFERRED_WORKLOAD_TYPES_ACTIVE: &str = "Active";

/// One configuration of an Auto Scaling group: the configuration it runs today, or one
/// the analysis found as a candidate for it. Both sides are the same shape because the
/// service reports them through the same `AutoScalingGroupConfiguration`, and because
/// every derived figure (cost, saving, projected utilization, migration effort) is a
/// comparison BETWEEN two of them.
#[derive(Debug, Clone)]
pub struct GroupConfiguration {
    /// The instance type(s) of the configuration: one for a `SingleInstanceType` group,
    /// several for a `MixedInstanceTypes` one.
    pub instance_types: Vec<String>,
    /// The `AllocationStrategy`, which "is only available for EC2 Auto Scaling groups
    /// with mixed instance types", so a single-instance-type configuration has none.
    pub allocation_strategy: Option<String>,
    /// `desiredCapacity`: the number of instances the configuration runs.
    pub desired_capacity: i32,
    /// `minSize`.
    pub min_size: i32,
    /// `maxSize`.
    pub max_size: i32,
    /// vCPUs of ONE instance of this configuration.
    pub vcpus: f64,
    /// Memory, in GiB, of ONE instance of this configuration.
    pub memory_gib: f64,
    /// The public on-demand price of ONE instance-hour, in [`CURRENCY_CODE`].
    pub hourly_price: f64,
    /// The configuration's `CpuVendorArchitecture`: [`CPU_ARCHITECTURE_CURRENT`] or
    /// [`CPU_ARCHITECTURE_AWS_ARM64`].
    pub architecture: String,
    /// `PerformanceRisk` (0-4) of running the analysed workload on this configuration:
    /// "the likelihood of the recommended instance type not meeting the resource needs
    /// of your workload", 0 meaning it "is predicted to always provide enough hardware
    /// capability".
    pub performance_risk: f64,
}

impl GroupConfiguration {
    /// Whether this configuration mixes instance types, which is what
    /// `AutoScalingGroupConfiguration$type` reports.
    pub fn is_mixed(&self) -> bool {
        self.instance_types.len() > 1
    }

    /// The `AsgType` of this configuration.
    pub fn asg_type(&self) -> &'static str {
        if self.is_mixed() {
            ASG_TYPE_MIXED_INSTANCE_TYPES
        } else {
            ASG_TYPE_SINGLE_INSTANCE_TYPE
        }
    }

    /// The estimated monthly on-demand cost of running this configuration: the price of
    /// one instance-hour × the instances it runs × [`HOURS_PER_MONTH`].
    pub fn monthly_cost(&self) -> f64 {
        round_hundredths(self.hourly_price * f64::from(self.desired_capacity) * HOURS_PER_MONTH)
    }

    /// The vCPUs the whole group runs on this configuration.
    pub fn total_vcpus(&self) -> f64 {
        self.vcpus * f64::from(self.desired_capacity)
    }

    /// The memory, in GiB, the whole group runs on this configuration.
    pub fn total_memory_gib(&self) -> f64 {
        self.memory_gib * f64::from(self.desired_capacity)
    }

    /// Whether `other` is a different configuration of the same group — a different set
    /// of instance types or a different capacity. A candidate that differs from the
    /// group's own configuration IS a recommendation, which is what makes the group
    /// `NotOptimized` ([`AnalyzedAutoScalingGroup::finding`]).
    pub fn differs_from(&self, other: &Self) -> bool {
        self.instance_types != other.instance_types
            || self.desired_capacity != other.desired_capacity
            || self.min_size != other.min_size
            || self.max_size != other.max_size
    }

    /// This configuration as the modelled `AutoScalingGroupConfiguration`.
    ///
    /// `estimatedInstanceHourReductionPercentage` is left unreported: it "is only
    /// available for EC2 Auto Scaling groups with scaling policies", and this emulator
    /// records no scaling policy for a group, so there is no reduction to project.
    pub fn to_wire(&self) -> AutoScalingGroupConfiguration {
        AutoScalingGroupConfiguration {
            desired_capacity: self.desired_capacity,
            min_size: self.min_size,
            max_size: self.max_size,
            instance_type: (!self.is_mixed())
                .then(|| self.instance_types.first().cloned())
                .flatten(),
            allocation_strategy: self.allocation_strategy.clone(),
            estimated_instance_hour_reduction_percentage: None,
            r#type: Some(self.asg_type().to_string()),
            mixed_instance_types: self.is_mixed().then(|| self.instance_types.clone()),
        }
    }
}

/// How much of `capacity` a peak demand of `demand` takes. A configuration that
/// sustains nothing is saturated by any demand at all, and idle under none.
fn demand_ratio(demand: f64, capacity: f64) -> f64 {
    if capacity > 0.0 {
        demand / capacity
    } else if demand > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

/// How close a peak demand of `demand` came to the capacity that has to carry it, as a
/// `PerformanceRisk` on the model's 0..4 scale: 0 while the peak stays under a quarter of
/// the capacity, 4 once it reaches 125% of it. "The higher the risk, the more likely the
/// current [configuration] doesn't have sufficient capacity", so the score rises with that
/// ratio.
///
/// The ONE risk scale in this service, so a volume's risk and a function's risk mean the
/// same thing at the same number and both land in the band [`performance_risk_band`] names.
pub fn performance_risk_score(demand: f64, capacity: f64) -> f64 {
    performance_risk_score_of_ratio(demand_ratio(demand, capacity))
}

/// [`performance_risk_score`] for a demand-to-capacity ratio already computed.
fn performance_risk_score_of_ratio(ratio: f64) -> f64 {
    ((ratio - 0.25) / 0.25).clamp(0.0, 4.0)
}

/// The `CurrentPerformanceRisk` band a `PerformanceRisk` score falls in: the model's
/// four bands over the score's 0..4 range, lowest risk first, so the reported band and
/// the option's numeric risk always say the same thing.
pub fn performance_risk_band(score: f64) -> &'static str {
    const BANDS: [&str; 4] = [
        PERFORMANCE_RISK_VERY_LOW,
        PERFORMANCE_RISK_LOW,
        PERFORMANCE_RISK_MEDIUM,
        PERFORMANCE_RISK_HIGH,
    ];
    let band = (score.max(0.0) as usize).min(BANDS.len() - 1);
    BANDS[band]
}

/// The IOPS a configuration can deliver, burst included.
pub fn configured_iops_capacity(configuration: &VolumeConfiguration) -> f64 {
    f64::from(
        configuration
            .volume_baseline_iops
            .max(configuration.volume_burst_iops)
            .max(0),
    )
}

/// The throughput in MiB/s a configuration can deliver, burst included.
pub fn configured_throughput_capacity(configuration: &VolumeConfiguration) -> f64 {
    f64::from(
        configuration
            .volume_baseline_throughput
            .max(configuration.volume_burst_throughput)
            .max(0),
    )
}

/// What one month of a volume with this configuration costs in USD, or `None` for a
/// volume type this emulator holds no price for — whose recommendation then carries no
/// savings figure rather than an invented one.
///
/// Amazon EBS public pricing in the `us-east-1` Region, as the EBS pricing page lists
/// it: `gp3` $0.08 per GiB-month plus $0.005 per provisioned IOPS-month above the 3,000
/// included and $0.040 per provisioned MiB/s-month above the 125 included; `gp2` $0.10
/// per GiB-month; `io1` $0.125 per GiB-month plus $0.065 per provisioned IOPS-month;
/// `io2` $0.125 per GiB-month plus provisioned IOPS tiered at $0.065 for the first
/// 32,000, $0.046 up to 64,000 and $0.032 above; `st1` $0.045, `sc1` $0.015 and
/// `standard` $0.05 per GiB-month. Every cost and saving this service reports is
/// computed from this one table, so the figures of a recommendation agree with each
/// other.
pub fn volume_monthly_cost(configuration: &VolumeConfiguration) -> Option<f64> {
    let volume_type = configuration.volume_type.as_deref()?;
    let size = f64::from(configuration.volume_size.max(0));
    let provisioned_iops = configuration.volume_baseline_iops.max(0);
    let provisioned_throughput = configuration.volume_baseline_throughput.max(0);
    let cost = match volume_type {
        "gp3" => {
            0.08 * size
                + 0.005 * f64::from((provisioned_iops - GP3_INCLUDED_IOPS).max(0))
                + 0.040 * f64::from((provisioned_throughput - GP3_INCLUDED_THROUGHPUT_MIB).max(0))
        }
        "gp2" => 0.10 * size,
        "io1" => 0.125 * size + 0.065 * f64::from(provisioned_iops),
        "io2" => 0.125 * size + io2_provisioned_iops_cost(provisioned_iops),
        "st1" => 0.045 * size,
        "sc1" => 0.015 * size,
        "standard" => 0.05 * size,
        _ => return None,
    };
    Some(round_hundredths(cost))
}

/// The tiered provisioned-IOPS charge of an `io2` volume, per [`volume_monthly_cost`].
fn io2_provisioned_iops_cost(provisioned_iops: i32) -> f64 {
    let first_tier = provisioned_iops.min(32_000);
    let second_tier = (provisioned_iops - 32_000).clamp(0, 32_000);
    let third_tier = (provisioned_iops - 64_000).max(0);
    0.065 * f64::from(first_tier) + 0.046 * f64::from(second_tier) + 0.032 * f64::from(third_tier)
}

/// An Amazon EBS volume ARN — `arn:<partition>:ec2:<region>:<account>:volume/<id>` —
/// split into the parts the readers need, or `None` for anything that is not one.
/// `GetEBSVolumeRecommendations$volumeArns` takes "the Amazon Resource Name (ARN) of the
/// volumes for which to return recommendations", so anything else is not a volume ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedVolumeArn<'a> {
    /// The ARN's account-id field: the account that owns the volume.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the volume is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The EC2 volume id, which is the inventory's key.
    pub volume_id: &'a str,
}

/// Parse an Amazon EBS volume ARN, or `None` for anything that is not one. See
/// [`ParsedVolumeArn`].
pub fn parse_volume_arn(arn: &str) -> Option<ParsedVolumeArn<'_>> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, _partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || *service != "ec2" {
        return None;
    }
    let (resource_kind, volume_id) = resource.split_once('/')?;
    if resource_kind != "volume" || volume_id.is_empty() {
        return None;
    }
    Some(ParsedVolumeArn {
        account_id,
        region,
        volume_id,
    })
}

/// One Auto Scaling group Compute Optimizer has analysed: the group as it is configured,
/// the utilization it observed, and the candidate configurations the analysis found for
/// it. Every value a recommendation REPORTS is either one of these stored observations
/// or a figure computed from them, so two readers of the same row cannot disagree.
#[derive(Debug, Clone)]
pub struct AnalyzedAutoScalingGroup {
    /// The account that owns the group.
    pub account_id: String,
    /// The group's own id — the identifier segment of its ARN. A row keeps the group's
    /// REGION-FREE identity, because Compute Optimizer is regional: the ARN a reader reports
    /// names the region of the endpoint the request addressed, which [`Self::arn_in`] builds
    /// from this id and the group's name.
    pub group_id: String,
    /// `autoScalingGroupName`.
    pub name: String,
    /// The group's current configuration.
    pub current: GroupConfiguration,
    /// The utilization the analysis observed, as (`MetricName`, value) pairs at the
    /// [`METRIC_STATISTIC_MAXIMUM`] statistic — the only statistic this API reports.
    pub utilization: Vec<(String, f64)>,
    /// `currentPerformanceRisk`: "the risk of the current Auto Scaling group not meeting
    /// the performance needs of its workloads", a `CurrentPerformanceRisk` value.
    pub current_performance_risk: String,
    /// The workload types the analysis inferred for the instances in the group
    /// (`InferredWorkloadType` values), empty where it inferred none.
    pub inferred_workload_types: Vec<String>,
    /// The candidate configurations the analysis found, best first — so the reported
    /// `rank` is a candidate's position in this list ("the top recommendation option is
    /// ranked as 1").
    pub candidates: Vec<GroupConfiguration>,
    /// The Savings Plans / Reserved Instances discount rate that applies to this
    /// group's compute spend, as a fraction of the on-demand price, or `0.0` where none
    /// does. It is what separates `savingsOpportunityAfterDiscounts` from the on-demand
    /// `savingsOpportunity`.
    pub savings_plans_discount_rate: f64,
}

impl AnalyzedAutoScalingGroup {
    /// This group's identity WITHOUT the region — the resource part of its ARN.
    pub fn arn_resource(&self) -> String {
        format!(
            "autoScalingGroup:{}:autoScalingGroupName/{}",
            self.group_id, self.name
        )
    }

    /// This group's ARN as the endpoint in `region` reports it — the form
    /// `AutoScalingGroupRecommendation$autoScalingGroupArn` carries ([`resource_arn_in`]).
    pub fn arn_in(&self, region: &str) -> String {
        resource_arn_in(
            AUTO_SCALING_ARN_SERVICE,
            region,
            &self.account_id,
            &self.arn_resource(),
        )
    }

    /// The group's `Finding`.
    ///
    /// "An Auto Scaling group is considered not optimized when Compute Optimizer
    /// identifies a recommendation that can provide better performance for your
    /// workload", and "optimized when Compute Optimizer determines that the group is
    /// correctly provisioned to run your workload based on the chosen instance type" —
    /// so the group is `NotOptimized` exactly when the analysis found a candidate that
    /// is not the group's own configuration, and `Optimized` when its own configuration
    /// is the best the analysis found.
    pub fn finding(&self) -> &'static str {
        if self
            .candidates
            .iter()
            .any(|candidate| candidate.differs_from(&self.current))
        {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// The observed utilization as the modelled `UtilizationMetrics`.
    pub fn utilization_metrics(&self) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .map(|(name, value)| UtilizationMetric {
                name: Some(name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: round_hundredths(*value),
            })
            .collect()
    }

    /// The utilization the observed workload is projected to reach on `candidate`.
    ///
    /// The same work runs on whatever capacity the configuration provides, so each
    /// percentage scales with the ratio of the capacity it was observed on to the
    /// capacity it is projected onto — CPU by the group's total vCPUs, memory by the
    /// group's total memory — through the one projection [`project_percentage`] states.
    pub fn projected_utilization_metrics(
        &self,
        candidate: &GroupConfiguration,
    ) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .map(|(name, observed)| {
                let (from, to) = if name == METRIC_MEMORY {
                    (self.current.total_memory_gib(), candidate.total_memory_gib())
                } else {
                    (self.current.total_vcpus(), candidate.total_vcpus())
                };
                UtilizationMetric {
                    name: Some(name.clone()),
                    statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                    value: project_percentage(*observed, from, to),
                }
            })
            .collect()
    }

    /// The `MigrationEffort` of moving this group onto `candidate`, by the rule the
    /// model states: "The migration effort is `VeryLow` if both the current and
    /// recommended instance types are of the same CPU architecture", `Low` "if Amazon
    /// EMR is the inferred workload type and an Amazon Web Services Graviton instance
    /// type is recommended", and `Medium` "if a workload type couldn't be inferred but
    /// an Amazon Web Services Graviton instance type is recommended".
    ///
    /// The model names Amazon EMR as the example of an inferred workload and states no
    /// rule for any other, so the emulator generalises its `Low` case to ANY workload type
    /// the analysis inferred: what the two documented Graviton cases differ on is whether
    /// a workload type was inferred at all, and naming one of the nine
    /// `InferredWorkloadType` values would make the effort of a Kafka or a Redis group
    /// depend on a value the rule says nothing about.
    ///
    /// `infers_workload_types` is whether the account leaves the workload-type inference
    /// active: with the inference deactivated no workload type is inferred for the
    /// group, which is the `Medium` case.
    ///
    /// The rule itself is the crate's ([`migration_effort`]), shared with the Amazon EC2
    /// instance analysis, so two recommendations of the same move cannot report different
    /// effort.
    pub fn migration_effort(
        &self,
        candidate: &GroupConfiguration,
        infers_workload_types: bool,
    ) -> &'static str {
        migration_effort(
            &candidate.architecture,
            infers_workload_types && !self.inferred_workload_types.is_empty(),
        )
    }

    /// The saving `candidate` offers against the group's own configuration, on the
    /// public on-demand prices stored with the analysis: the monthly cost it removes,
    /// and that saving as a percentage of the group's current monthly cost.
    ///
    /// `discount_rate` nets both sides of the comparison down by a Savings Plans /
    /// Reserved Instances discount, which is how `savingsOpportunityAfterDiscounts` is
    /// computed; pass `0.0` for the on-demand `savingsOpportunity`.
    pub fn savings(&self, candidate: &GroupConfiguration, discount_rate: f64) -> (f64, f64) {
        let retained = 1.0 - discount_rate;
        let current = round_hundredths(self.current.monthly_cost() * retained);
        let projected = round_hundredths(candidate.monthly_cost() * retained);
        let saving = round_hundredths((current - projected).max(0.0));
        (saving, percentage_of(saving, current))
    }

    /// This group as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it: the finding and the costs the analysis reached, with the RECOMMENDED
    /// cost being the top-ranked candidate's — "the top recommendation option is ranked as
    /// 1" — or the group's own cost where the analysis found no other candidate.
    ///
    /// The projection is what keeps one group's analysis in ONE place: the summary reader
    /// and `GetAutoScalingGroupRecommendations` both answer from this row's source, so
    /// they cannot disagree about a group's finding, its performance risk or what adopting
    /// its recommendation saves. An Auto Scaling group recommendation reports no finding
    /// reason codes (`AutoScalingGroupRecommendation` declares no such member), so the
    /// row carries none.
    pub fn to_analysed_resource(&self) -> AnalysedResource {
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_AUTO_SCALING_GROUP.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a group is the id and name its ARN
            // names it by.
            arn_service: AUTO_SCALING_ARN_SERVICE.to_string(),
            arn_resource: self.arn_resource(),
            finding: self.finding().to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: self.current_performance_risk.clone(),
            current_monthly_cost: self.current.monthly_cost(),
            recommended_monthly_cost: self
                .candidates
                .first()
                .unwrap_or(&self.current)
                .monthly_cost(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            // A group is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}

/// `part` as a percentage of `whole`, to two decimal places; zero when `whole` is zero,
/// because no percentage of nothing is defined.
pub fn percentage_of(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        return 0.0;
    }
    round_hundredths(part / whole * 100.0)
}

/// Round a derived figure to two decimal places, so a money amount lands on whole cents
/// and a percentage does not report binary-float noise.
///
/// A figure that rounds to zero is reported as `0`, never as the negative zero binary
/// floating point also has: a response states what a resource costs or saves, and no such
/// amount is a negative zero.
pub fn round_hundredths(value: f64) -> f64 {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// A resource's tags as the modelled `Tags` list, or `None` where it carries none. Every
/// record of this service reports its tags through this one projection, so no two readers can
/// word an empty tag set differently.
pub fn modelled_tags(tags: &[(String, String)]) -> Option<Vec<Tag>> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| Tag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            })
            .collect()
    })
}

/// The number of days a `LookBackPeriodPreference` names
/// (`LOOK_BACK_PERIOD_PREFERENCE_VALUES` are `DAYS_14` | `DAYS_32` | `DAYS_93`), which is
/// what `lookBackPeriodInDays` reports: "the number of days for which utilization metrics
/// were analyzed". Falls back to the service default's length for a value outside that
/// vocabulary, which no stored preference can be.
pub fn look_back_period_in_days(preference: &str) -> f64 {
    preference
        .strip_prefix("DAYS_")
        .and_then(|days| days.parse::<f64>().ok())
        .unwrap_or(14.0)
}

/// The preferences in force for one resource: what the account stored for it, with the
/// service's documented default for every preference it never set.
///
/// This is the same resolution `GetEffectiveRecommendationPreferences` reports, applied
/// to the resource a recommendation is about, so the preferences a recommendation says it
/// was generated under are the preferences a read of that resource reports.
#[derive(Debug, Clone)]
pub struct EffectivePreferences {
    /// `EnhancedInfrastructureMetrics`, reported only where a record sets it.
    pub enhanced_infrastructure_metrics: Option<String>,
    /// `InferredWorkloadTypesPreference`, which defaults to
    /// [`INFERRED_WORKLOAD_TYPES_ACTIVE`] because "the inferred workload type feature is
    /// active by default".
    pub inferred_workload_types: String,
    /// `ExternalMetricsPreference`, reported only where a record sets it.
    pub external_metrics_preference: Option<ExternalMetricsPreference>,
    /// `LookBackPeriodPreference`, which defaults to [`DEFAULT_LOOK_BACK_PERIOD`].
    pub look_back_period: String,
    /// `UtilizationPreferences`: the stored preference, or the service's documented
    /// default for a resource type that HAS one — "you can only set CPU and memory
    /// utilization preferences for the Amazon EC2 instance resource type", so the list is
    /// empty for every other resource type.
    pub utilization_preferences: Vec<UtilizationPreference>,
    /// `PreferredResources`, which default to [`default_preferred_resources`].
    pub preferred_resources: Vec<EffectivePreferredResource>,
    /// `SavingsEstimationMode`, reported only where a record sets it: the preference is
    /// what "activates" savings net of discounts.
    pub savings_estimation_mode: Option<String>,
}

impl EffectivePreferences {
    /// Whether savings are reported net of Savings Plans and Reserved Instances
    /// discounts, i.e. whether the account activated the preference.
    pub fn after_discounts(&self) -> bool {
        self.savings_estimation_mode.as_deref() == Some(SAVINGS_ESTIMATION_AFTER_DISCOUNTS)
    }

    /// Whether Compute Optimizer infers workload types for the resource.
    pub fn infers_workload_types(&self) -> bool {
        self.inferred_workload_types == INFERRED_WORKLOAD_TYPES_ACTIVE
    }

    /// These preferences as the modelled `EffectiveRecommendationPreferences` a
    /// recommendation carries, under the CPU vendor architectures the request asked for.
    ///
    /// `utilizationPreferences` is reported only where the resource type has one at all
    /// (see the field's doc), so an Auto Scaling group's recommendation carries none.
    /// `savingsEstimationMode` names where the money figures came from
    /// ([`SAVINGS_ESTIMATION_SOURCE`]).
    pub fn to_wire(
        &self,
        cpu_vendor_architectures: Vec<String>,
    ) -> EffectiveRecommendationPreferences {
        EffectiveRecommendationPreferences {
            cpu_vendor_architectures: Some(cpu_vendor_architectures),
            enhanced_infrastructure_metrics: self.enhanced_infrastructure_metrics.clone(),
            inferred_workload_types: Some(self.inferred_workload_types.clone()),
            external_metrics_preference: self.external_metrics_preference.clone(),
            look_back_period: Some(self.look_back_period.clone()),
            utilization_preferences: (!self.utilization_preferences.is_empty())
                .then(|| self.utilization_preferences.clone()),
            preferred_resources: Some(self.preferred_resources.clone()),
            savings_estimation_mode: Some(InstanceSavingsEstimationMode {
                source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
            }),
        }
    }
}

/// The Auto Scaling group analysis this emulator holds, built once with the store.
///
/// A row keeps the group's REGION-FREE identity (its id and its name), because Compute
/// Optimizer is regional: every reader builds the ARN for the region the request addressed
/// ([`AnalyzedAutoScalingGroup::arn_in`]), so the same groups are reported at every regional
/// endpoint, each under that endpoint's own ARN.
///
/// The emulator observes no real workload, so WHICH groups are analysed is a fixed set
/// rather than something derived from traffic; every figure a recommendation reports is
/// nevertheless computed from the observations stored here (cost, saving, percentage,
/// projected utilization, finding, rank and migration effort all follow from these rows),
/// and a reader of a row never writes to it.
///
/// The rows span the shapes a client must handle: a single-instance-type group that is
/// over-provisioned (two candidates, one of them Graviton), a group that is correctly
/// provisioned (its own configuration is the only candidate, so no saving), and a
/// mixed-instance-type group with a Savings Plans discount against its spend.
fn seed_auto_scaling_group_analyses() -> Vec<AnalyzedAutoScalingGroup> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        AnalyzedAutoScalingGroup {
            account_id: account_id.clone(),
            group_id: "6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e51".to_string(),
            name: "web-tier-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec!["m5.2xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 4,
                min_size: 2,
                max_size: 8,
                vcpus: 8.0,
                memory_gib: 32.0,
                hourly_price: 0.384,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 0.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 12.4),
                (METRIC_MEMORY.to_string(), 28.6),
            ],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["Nginx".to_string()],
            candidates: vec![
                GroupConfiguration {
                    instance_types: vec!["m5.xlarge".to_string()],
                    allocation_strategy: None,
                    desired_capacity: 4,
                    min_size: 2,
                    max_size: 8,
                    vcpus: 4.0,
                    memory_gib: 16.0,
                    hourly_price: 0.192,
                    architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                    performance_risk: 1.0,
                },
                GroupConfiguration {
                    instance_types: vec!["m6g.xlarge".to_string()],
                    allocation_strategy: None,
                    desired_capacity: 4,
                    min_size: 2,
                    max_size: 8,
                    vcpus: 4.0,
                    memory_gib: 16.0,
                    hourly_price: 0.154,
                    architecture: CPU_ARCHITECTURE_AWS_ARM64.to_string(),
                    performance_risk: 1.0,
                },
            ],
            savings_plans_discount_rate: 0.0,
        },
        AnalyzedAutoScalingGroup {
            account_id: account_id.clone(),
            group_id: "6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e52".to_string(),
            name: "batch-worker-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec!["c5.4xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 6,
                min_size: 6,
                max_size: 12,
                vcpus: 16.0,
                memory_gib: 32.0,
                hourly_price: 0.68,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 1.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 86.5),
                (METRIC_MEMORY.to_string(), 71.2),
            ],
            current_performance_risk: PERFORMANCE_RISK_LOW.to_string(),
            inferred_workload_types: Vec::new(),
            // The group's own configuration is the only candidate the analysis found,
            // which is what makes it `Optimized`: there is no saving to report.
            candidates: vec![GroupConfiguration {
                instance_types: vec!["c5.4xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 6,
                min_size: 6,
                max_size: 12,
                vcpus: 16.0,
                memory_gib: 32.0,
                hourly_price: 0.68,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 1.0,
            }],
            savings_plans_discount_rate: 0.0,
        },
        AnalyzedAutoScalingGroup {
            account_id,
            group_id: "6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e53".to_string(),
            name: "mixed-fleet-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec![
                    "m5.large".to_string(),
                    "m5a.large".to_string(),
                    "m6i.large".to_string(),
                ],
                allocation_strategy: Some("LowestPrice".to_string()),
                desired_capacity: 10,
                min_size: 4,
                max_size: 20,
                vcpus: 2.0,
                memory_gib: 8.0,
                hourly_price: 0.096,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 0.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 9.1),
                (METRIC_MEMORY.to_string(), 22.3),
            ],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["AmazonEmr".to_string()],
            candidates: vec![
                GroupConfiguration {
                    instance_types: vec!["m6g.large".to_string(), "m6gd.large".to_string()],
                    allocation_strategy: Some("LowestPrice".to_string()),
                    desired_capacity: 6,
                    min_size: 4,
                    max_size: 20,
                    vcpus: 2.0,
                    memory_gib: 8.0,
                    hourly_price: 0.077,
                    architecture: CPU_ARCHITECTURE_AWS_ARM64.to_string(),
                    performance_risk: 1.0,
                },
                GroupConfiguration {
                    instance_types: vec![
                        "m5.large".to_string(),
                        "m5a.large".to_string(),
                        "m6i.large".to_string(),
                    ],
                    allocation_strategy: Some("LowestPrice".to_string()),
                    desired_capacity: 6,
                    min_size: 4,
                    max_size: 20,
                    vcpus: 2.0,
                    memory_gib: 8.0,
                    hourly_price: 0.096,
                    architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                    performance_risk: 1.0,
                },
            ],
            savings_plans_discount_rate: 0.28,
        },
    ]
}

/// The currency every monetary figure in the findings catalog is denominated in, one of
/// the model's `Currency` values (`crate::ops::types::CURRENCY_VALUES`).
pub const CURRENCY_USD: &str = "USD";

/// The `CurrentPerformanceRisk` values (`crate::ops::types::CURRENT_PERFORMANCE_RISK_VALUES`),
/// named so the catalog and the rating counts read as the model's own vocabulary.
pub const PERFORMANCE_RISK_VERY_LOW: &str = "VeryLow";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_LOW: &str = "Low";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_MEDIUM: &str = "Medium";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_HIGH: &str = "High";

/// The `Finding` values (`crate::ops::types::FINDING_VALUES`).
pub const FINDING_UNDERPROVISIONED: &str = "Underprovisioned";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_OVERPROVISIONED: &str = "Overprovisioned";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_OPTIMIZED: &str = "Optimized";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_NOT_OPTIMIZED: &str = "NotOptimized";

/// The `Finding` values AS A RECOMMENDATION SUMMARY SPELLS THEM: the EC2 instance
/// surface's own upper-snake spelling of the same four classifications
/// ([`FINDING_UNDERPROVISIONED`] and the three below it), which is what
/// `GetRecommendationSummaries` names a count under for a resource type counted at the
/// `Finding` vocabulary itself.
///
/// A resource type whose recommendations carry their OWN finding vocabulary — an Amazon EBS
/// volume (`EBSFinding`), a Lambda function (`LambdaFunctionRecommendationFinding`), an
/// Amazon ECS service (`ECSServiceRecommendationFinding`), a license (`LicenseFinding`), a
/// database (`RDSInstanceFinding`, `RDSStorageFinding`) — is counted at THAT vocabulary and
/// in its spelling instead; [`findings_reported_for`] is the one table of which is which.
pub const SUMMARY_FINDING_UNDERPROVISIONED: &str = "UNDER_PROVISIONED";
/// See [`SUMMARY_FINDING_UNDERPROVISIONED`].
pub const SUMMARY_FINDING_OVERPROVISIONED: &str = "OVER_PROVISIONED";
/// See [`SUMMARY_FINDING_UNDERPROVISIONED`].
pub const SUMMARY_FINDING_OPTIMIZED: &str = "OPTIMIZED";
/// See [`SUMMARY_FINDING_UNDERPROVISIONED`].
pub const SUMMARY_FINDING_NOT_OPTIMIZED: &str = "NOT_OPTIMIZED";

/// The `RecommendationSourceType` values the findings catalog holds resources of
/// (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`).
pub const SOURCE_TYPE_EC2_INSTANCE: &str = "Ec2Instance";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_AUTO_SCALING_GROUP: &str = "AutoScalingGroup";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_LAMBDA_FUNCTION: &str = "LambdaFunction";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_ECS_SERVICE: &str = "EcsService";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_RDS_DB_INSTANCE: &str = "RdsDBInstance";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_LICENSE: &str = "License";

/// The `RecommendationSourceType` values this service holds resources of ONLY as idle
/// resources (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`): a resource kind
/// Compute Optimizer reports an idle recommendation for and no rightsizing analysis, so a
/// summary of one counts what was found idle beside a rightsizing classification every one
/// of whose counts is zero ([`findings_reported_for`]).
pub const SOURCE_TYPE_NAT_GATEWAY: &str = "NatGateway";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_DYNAMODB_TABLE: &str = "DynamoDBTable";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_ELASTICACHE_CLUSTER: &str = "ElastiCacheCluster";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_MEMORYDB_CLUSTER: &str = "MemoryDBCluster";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_DOCUMENTDB_CLUSTER: &str = "DocumentDBCluster";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_WORKSPACES: &str = "WorkSpaces";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_SAGEMAKER_ENDPOINT: &str = "SageMakerEndpoint";

/// The two `RecommendationSourceType` values naming a database's STORAGE rather than the
/// database itself (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`): groups this
/// service tracks and holds no analysis of, so a summary of one counts nothing.
pub const SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE: &str = "RdsDBInstanceStorage";
/// See [`SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE`].
pub const SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE: &str = "AuroraDBClusterStorage";

/// Every `RecommendationSourceType` this service TRACKS — the whole vocabulary
/// `RecommendationSummary$recommendationResourceType` names — in the order the model
/// declares it, which is the order `GetRecommendationSummaries` reports one summary per
/// group in.
///
/// A recommendation summary is a COUNT ROLL-UP per resource type — `Summary$value` is a
/// count whose default is `0` — so the summaries an enrolled account is answered with are
/// this whole vocabulary rather than the subset the analysis happens to hold a row of: a
/// resource type nothing of which has been analysed is summarised with counts of zero, which
/// is how a caller reads an enrolled account's zero-finding baseline instead of being unable
/// to tell it from no summary surface at all. What a summary COUNTS is the analysis a caller
/// can read
/// ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::visible_analysed_resources`]),
/// so the analysis an enrollment starts ([`ANALYSIS_DELAY`]) decides what the counts ARE and
/// never whether the summary exists.
///
/// `crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES` holds the same values SORTED,
/// which is the order a value LOOKUP wants and not the order the groups are reported in;
/// `tests/all/tracked_resource_type_summaries.rs` pins this list against it, so a value the
/// model adds or renames cannot be missed here.
pub const TRACKED_SOURCE_TYPES: &[&str] = &[
    SOURCE_TYPE_EC2_INSTANCE,
    SOURCE_TYPE_AUTO_SCALING_GROUP,
    SOURCE_TYPE_EBS_VOLUME,
    SOURCE_TYPE_LAMBDA_FUNCTION,
    SOURCE_TYPE_ECS_SERVICE,
    SOURCE_TYPE_LICENSE,
    SOURCE_TYPE_RDS_DB_INSTANCE,
    SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE,
    SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE,
    SOURCE_TYPE_NAT_GATEWAY,
    SOURCE_TYPE_DYNAMODB_TABLE,
    SOURCE_TYPE_ELASTICACHE_CLUSTER,
    SOURCE_TYPE_MEMORYDB_CLUSTER,
    SOURCE_TYPE_DOCUMENTDB_CLUSTER,
    SOURCE_TYPE_WORKSPACES,
    SOURCE_TYPE_SAGEMAKER_ENDPOINT,
];

/// The findings a summary of `resource_type` reports a count at, each as the `Finding` a
/// catalog row at it STORES paired with the NAME the summary reports that count under.
///
/// Which findings, from the `GetRecommendationSummaries` documentation: "It returns the
/// number of: Amazon EC2 instances in an account that are Underprovisioned,
/// Overprovisioned, or Optimized. EC2 Auto Scaling groups in an account that are
/// NotOptimized, or Optimized. Amazon EBS volumes in an account that are NotOptimized, or
/// Optimized. Lambda functions in an account that are NotOptimized, or Optimized. Amazon
/// ECS services in an account that are Underprovisioned, Overprovisioned, or Optimized.
/// Commercial software licenses in an account that are InsufficientMetrics, NotOptimized or
/// Optimized. Amazon Aurora and Amazon RDS databases in an account that are
/// Underprovisioned, Overprovisioned, Optimized, or NotOptimized."
///
/// Under WHICH NAME: the vocabulary of the resource type's OWN recommendations. A resource
/// type the model gives a finding enum of its own is counted at that enum and in its
/// spelling — `EBSFinding` for a volume, `LambdaFunctionRecommendationFinding` for a
/// function, `ECSServiceRecommendationFinding` for an Amazon ECS service,
/// `RDSInstanceFinding` for a database and `RDSStorageFinding` for its storage. Every other
/// tracked resource type is counted at the `Finding` vocabulary itself, which the instance
/// surface spells in upper snake case ([`SUMMARY_FINDING_UNDERPROVISIONED`]) — so an EC2
/// instance count is reported under `OPTIMIZED` while an EBS volume count is reported under
/// `Optimized`, each as the surface that produced the finding names it.
///
/// Each vocabulary is narrowed to the values `Summary$name` can carry, the model's `Finding`
/// enum: a license's documented `InsufficientMetrics` is a `LicenseFinding` value outside
/// it, so a license whose metrics the analysis could not read is counted at no finding at all
/// — and is no catalog row either ([`ObservedLicensedInstance::to_analysed_resource`]), so a
/// count and the rows behind it still agree.
///
/// An empty slice for a `RecommendationSourceType` this service does not track
/// ([`TRACKED_SOURCE_TYPES`]) and for any other string, so a caller reports no counts for
/// it rather than an invented vocabulary.
pub fn findings_reported_for(resource_type: &str) -> &'static [(&'static str, &'static str)] {
    match resource_type {
        // The `Finding` vocabulary, in the instance surface's own spelling.
        SOURCE_TYPE_EC2_INSTANCE => &[
            (FINDING_UNDERPROVISIONED, SUMMARY_FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, SUMMARY_FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, SUMMARY_FINDING_OPTIMIZED),
        ],
        SOURCE_TYPE_AUTO_SCALING_GROUP
        | SOURCE_TYPE_NAT_GATEWAY
        | SOURCE_TYPE_DYNAMODB_TABLE
        | SOURCE_TYPE_ELASTICACHE_CLUSTER
        | SOURCE_TYPE_MEMORYDB_CLUSTER
        | SOURCE_TYPE_DOCUMENTDB_CLUSTER
        | SOURCE_TYPE_WORKSPACES
        | SOURCE_TYPE_SAGEMAKER_ENDPOINT => &[
            (FINDING_NOT_OPTIMIZED, SUMMARY_FINDING_NOT_OPTIMIZED),
            (FINDING_OPTIMIZED, SUMMARY_FINDING_OPTIMIZED),
        ],
        // `ECSServiceRecommendationFinding`.
        SOURCE_TYPE_ECS_SERVICE => &[
            (FINDING_UNDERPROVISIONED, FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `EBSFinding` and `LambdaFunctionRecommendationFinding`.
        SOURCE_TYPE_EBS_VOLUME | SOURCE_TYPE_LAMBDA_FUNCTION => &[
            (FINDING_NOT_OPTIMIZED, FINDING_NOT_OPTIMIZED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `LicenseFinding`.
        SOURCE_TYPE_LICENSE => &[
            (FINDING_NOT_OPTIMIZED, FINDING_NOT_OPTIMIZED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `RDSInstanceFinding`: a database's own finding vocabulary, which names no
        // `NotOptimized` — that is its STORAGE's classification (`RDSStorageFinding`), and
        // the documentation's four findings for "Amazon Aurora and Amazon RDS databases"
        // are the two vocabularies together.
        SOURCE_TYPE_RDS_DB_INSTANCE => &[
            (FINDING_UNDERPROVISIONED, FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `RDSStorageFinding`.
        SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE | SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE => &[
            (FINDING_UNDERPROVISIONED, FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
            (FINDING_NOT_OPTIMIZED, FINDING_NOT_OPTIMIZED),
        ],
        _ => &[],
    }
}

/// The `FindingReasonCode` values a summary reports a count at, beside the finding they
/// explain: the whole vocabulary `ReasonCodeSummary$name` admits
/// (`crate::ops::types::FINDING_REASON_CODE_VALUES`, the memory pair), whatever the resource
/// type — the reason codes a resource type's OWN recommendations carry are spelled by their
/// own enums (`InstanceRecommendationFindingReasonCode`, `LicenseFindingReasonCode`, …),
/// which this shape cannot hold.
pub fn finding_reason_codes_reported_for() -> &'static [&'static str] {
    FINDING_REASON_CODE_VALUES
}

/// One resource Compute Optimizer has analysed for an account: the optimization finding
/// it carries, what it costs today, what the recommended configuration would cost, and
/// the risk its current configuration runs. This is the row the findings readers report
/// and `GetRecommendationSummaries` aggregates, so every figure a summary carries is
/// computed from these and not stated twice.
#[derive(Debug, Clone)]
pub struct AnalysedResource {
    /// The account that owns the resource.
    pub account_id: String,
    /// The `RecommendationSourceType` the resource is analysed as.
    pub resource_type: String,
    /// The service namespace the resource belongs to — the `ec2`, `rds`, … field of its ARN.
    pub arn_service: String,
    /// Everything after the account field of the resource's ARN (`db:orders-primary`),
    /// separators included: the resource's identity WITHOUT the region, which
    /// [`Self::arn_in`] builds the ARN for one regional endpoint from.
    pub arn_resource: String,
    /// The `Finding` the analysis reached for it.
    pub finding: String,
    /// The `FindingReasonCode`s behind that finding; empty where the finding has none.
    pub finding_reason_codes: Vec<String>,
    /// The `CurrentPerformanceRisk` the current configuration runs.
    pub current_performance_risk: String,
    /// What the resource costs per month in its current configuration.
    pub current_monthly_cost: f64,
    /// What the recommended configuration would cost per month. Equal to
    /// [`Self::current_monthly_cost`] for a resource whose configuration the analysis
    /// would not change, and higher for one it would grow.
    pub recommended_monthly_cost: f64,
    /// The `InferredWorkloadType`s running on the resource; empty where none was inferred.
    pub inferred_workload_types: Vec<String>,
    /// What the analysis recorded about the resource AS A DATABASE, for a row analysed as an
    /// Amazon Aurora or RDS database; `None` for every other resource type, and for a
    /// database whose configuration the analysis did not record. See [`RdsDatabaseDetail`].
    pub rds_database: Option<RdsDatabaseDetail>,
}

impl AnalysedResource {
    /// This resource's ARN as the endpoint in `region` reports it.
    ///
    /// Compute Optimizer is a REGIONAL service: each regional endpoint analyses and reports
    /// the resources of its own region, so a resource the catalog holds is a resource of the
    /// region whose endpoint a request addressed, and the ARN reported for it names that
    /// region. The catalog therefore records the resource's region-free identity and every
    /// reader builds the ARN for the region its request was signed for
    /// ([`request_region`]).
    pub fn arn_in(&self, region: &str) -> String {
        resource_arn_in(&self.arn_service, region, &self.account_id, &self.arn_resource)
    }

    /// The monthly saving of adopting the recommendation for this resource: what it costs
    /// today less what the recommended configuration costs, and NOTHING where the
    /// recommendation does not cost less — an underprovisioned resource is recommended a
    /// larger configuration, which saves no money.
    pub fn monthly_savings(&self) -> f64 {
        (self.current_monthly_cost - self.recommended_monthly_cost).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// Idle resource recommendations
//
// Compute Optimizer "generates recommendations for idle resources that meet a
// specific set of requirements" from the utilization it OBSERVES, so no request to
// this API produces one: `GetIdleRecommendations` only reports what the analysis
// recorded. That analysis is therefore part of the store's initial value
// (`seed_idle_resource_analyses`), the way the volume, group, function and license
// observations are, and everything below is about reading it — the observation a
// recommendation is DERIVED from, the filter vocabulary, the sort dimensions, the
// ARN -> resource-type catalog and the page cursor. Nothing below is written by a
// request.
// ---------------------------------------------------------------------------

/// The largest page `GetIdleRecommendations` will build, whatever `maxResults` asks for,
/// and the size it uses when a request names none: the ceiling the model itself puts on
/// `maxResults` (`IdleMaxResults` range 0..100). Applied in the handler, so no number a
/// request carries can size a response or the allocation that builds it.
pub const IDLE_MAX_PAGE_SIZE: usize = 100;

/// The `IdleRecommendationFilterName` values
/// (`crate::ops::types::IDLE_RECOMMENDATION_FILTER_NAME_VALUES`), named so the filter rule
/// below reads as the model's own vocabulary.
pub const IDLE_FILTER_FINDING: &str = "Finding";
/// See [`IDLE_FILTER_FINDING`].
pub const IDLE_FILTER_RESOURCE_TYPE: &str = "ResourceType";

/// The `Dimension` values `OrderBy$dimension` sorts on (`crate::ops::types::DIMENSION_VALUES`).
pub const DIMENSION_SAVINGS_VALUE: &str = "SavingsValue";
/// See [`DIMENSION_SAVINGS_VALUE`].
pub const DIMENSION_SAVINGS_VALUE_AFTER_DISCOUNT: &str = "SavingsValueAfterDiscount";

/// The `Order` value that sorts descending (`crate::ops::types::ORDER_VALUES` is
/// `Asc | Desc`); see [`idle_sorts_descending`].
pub const ORDER_DESC: &str = "Desc";

/// The `IdleFinding` values (`crate::ops::types::IDLE_FINDING_VALUES`), named so the rules
/// below read as the model's own vocabulary.
pub const IDLE_FINDING_IDLE: &str = "Idle";
/// See [`IDLE_FINDING_IDLE`].
pub const IDLE_FINDING_UNATTACHED: &str = "Unattached";
/// See [`IDLE_FINDING_IDLE`].
pub const IDLE_FINDING_UNUSED: &str = "Unused";

/// One resource Compute Optimizer's analysis found IDLE: the finding it reached, the
/// utilization it observed as the evidence for it, how long it has been observing the
/// resource and what the resource costs its account per month.
///
/// This is the OBSERVATION; every member of the `IdleRecommendation` a reader reports is
/// DERIVED from it ([`Self::to_recommendation`]), so no two reads of one resource can
/// disagree about its finding, the period analysed or what dropping it would save. A record
/// keeps the resource's REGION-FREE identity for the reason [`resource_arn_in`] gives: the
/// ARN a reader reports names the region of the endpoint the request addressed.
#[derive(Debug, Clone)]
pub struct IdleResourceAnalysis {
    /// The account that owns the resource (`IdleRecommendation$accountId`).
    pub account_id: String,
    /// The service namespace of the resource's ARN — its `ec2`, `dynamodb`, … field — which
    /// together with [`Self::arn_resource`] decides the `IdleRecommendationResourceType`
    /// reported for it ([`idle_resource_type_of`]).
    pub arn_service: String,
    /// Everything after the account field of the resource's ARN
    /// (`natgateway/nat-0a1b2c3d4e5f60001`), separators included.
    pub arn_resource: String,
    /// The `IdleFinding` the analysis reached: [`IDLE_FINDING_IDLE`],
    /// [`IDLE_FINDING_UNATTACHED`] or [`IDLE_FINDING_UNUSED`].
    pub finding: String,
    /// The `IdleUtilizationMetric`s the analysis observed — the evidence for the finding, in
    /// the `IdleMetricName` vocabulary the resource's own kind publishes. An idle resource's
    /// utilization does not vary over the period, so the value observed is the value over
    /// any part of it.
    pub utilization_metrics: Vec<IdleUtilizationMetric>,
    /// How many days Compute Optimizer has been observing this resource. A look-back period
    /// longer than this cannot be honoured, so what a reader reports as analysed is the
    /// shorter of the two ([`Self::analysed_days`]).
    pub days_observed: f64,
    /// What the resource costs its account per month, in [`CURRENCY_USD`]: the figure the
    /// savings opportunity is computed from, because the recommendation for a resource
    /// nothing is using is to stop paying for it.
    pub monthly_cost: f64,
    /// The resource's tags, as key/value pairs, in the order its own service reports them.
    pub tags: Vec<(String, String)>,
}

impl IdleResourceAnalysis {
    /// This resource's REGION-FREE identity — the key [`IdleRecommendationStore`] holds the
    /// record under ([`resource_identity`]).
    pub fn identity(&self) -> String {
        resource_identity(&self.arn_service, &self.account_id, &self.arn_resource)
    }

    /// This resource's ARN as the endpoint in `region` reports it ([`resource_arn_in`]).
    pub fn arn_in(&self, region: &str) -> String {
        resource_arn_in(&self.arn_service, region, &self.account_id, &self.arn_resource)
    }

    /// `IdleRecommendation$resourceId` — "the unique identifier for the resource": the last
    /// segment of its ARN's resource part, which is the id or name the resource's own service
    /// gives it (`natgateway/nat-0a1b…` -> `nat-0a1b…`).
    pub fn resource_id(&self) -> String {
        self.arn_resource
            .split(['/', ':'])
            .next_back()
            .unwrap_or_default()
            .to_string()
    }

    /// The `IdleRecommendationResourceType` this resource is, or `None` for a resource kind
    /// idle recommendations do not cover ([`idle_resource_type_of`]).
    pub fn resource_type(&self) -> Option<&'static str> {
        idle_resource_type_of(&self.arn_service, resource_kind_of(&self.arn_resource))
    }

    /// How many days of utilization a recommendation for this resource reports as analysed
    /// under `look_back_period`: the period in force, capped by how long the resource has
    /// been observed — the same rule every other observation of this service follows.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// What adopting this recommendation saves per month, and that saving as a percentage of
    /// what the resource costs.
    ///
    /// `IdleSavingsOpportunity$savingsOpportunityPercentage` is "the estimated monthly
    /// savings possible as a percentage of monthly cost by adopting Compute Optimizer's idle
    /// resource recommendations", and the recommendation for a resource the analysis found
    /// idle is to stop paying for it: the whole of what it costs is saved, whatever kind of
    /// resource it is. A resource costing nothing saves nothing.
    pub fn savings(&self) -> (f64, f64) {
        let cost = round_hundredths(self.monthly_cost.max(0.0));
        (percentage_of(cost, cost), cost)
    }

    /// `findingDescription` — "a summary of the findings for the resource": what the stored
    /// finding says about this kind of resource over the days analysed. Derived from the
    /// record, so the sentence cannot disagree with the members reported beside it.
    pub fn finding_description(&self, look_back_days: f64) -> String {
        let resource_type = self.resource_type().unwrap_or("resource");
        let days = round_hundredths(look_back_days);
        match self.finding.as_str() {
            IDLE_FINDING_UNATTACHED => format!(
                "The {resource_type} was not attached to any resource over the last {days} days."
            ),
            IDLE_FINDING_UNUSED => {
                format!("The {resource_type} was not used over the last {days} days.")
            }
            // `Idle`, and anything else the vocabulary holds: the analysis saw next to no use.
            _ => format!(
                "The {resource_type} showed little to no utilization over the last {days} days."
            ),
        }
    }

    /// The modelled `IdleRecommendation` this analysis yields at the endpoint in `region`
    /// under `look_back_period`, as generated at `last_refresh_timestamp`.
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> IdleRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let (percentage, value) = self.savings();
        let estimated = || {
            Some(IdleEstimatedMonthlySavings {
                currency: Some(CURRENCY_USD.to_string()),
                value,
            })
        };
        IdleRecommendation {
            resource_arn: Some(self.arn_in(region)),
            resource_id: Some(self.resource_id()),
            resource_type: self.resource_type().map(ToString::to_string),
            account_id: Some(self.account_id.clone()),
            finding: Some(self.finding.clone()),
            finding_description: Some(self.finding_description(look_back_days)),
            savings_opportunity: Some(IdleSavingsOpportunity {
                savings_opportunity_percentage: percentage,
                estimated_monthly_savings: estimated(),
            }),
            // This emulator prices every resource from public pricing and records no
            // negotiated discount ([`SAVINGS_ESTIMATION_SOURCE`]), so the after-discount
            // figures are the before-discount ones — the same stance the volume, function and
            // group readers of this service take.
            savings_opportunity_after_discounts: Some(IdleSavingsOpportunityAfterDiscounts {
                savings_opportunity_percentage: percentage,
                estimated_monthly_savings: estimated(),
            }),
            utilization_metrics: (!self.utilization_metrics.is_empty())
                .then(|| self.utilization_metrics.clone()),
            look_back_period_in_days: look_back_days,
            last_refresh_timestamp,
            tags: modelled_tags(&self.tags),
        }
    }
}

/// The idle resource analysis this service holds, keyed by the REGION-FREE identity of the
/// resource it is about, in the order the analysis recorded it.
///
/// Compute Optimizer's own analysis of an observed workload is what produces an idle resource
/// recommendation, so NO operation of this API creates one: the analysis is part of the
/// store's initial value ([`seed_idle_resource_analyses`]) and `GetIdleRecommendations`
/// reports what is held here, never anything a request carried. `insert` is the only writer,
/// so the record order a page walks and the index a lookup uses cannot drift apart.
#[derive(Debug, Default)]
pub struct IdleRecommendationStore {
    /// Resource identities in record order, so a page walks a stable sequence.
    order: Vec<String>,
    by_identity: HashMap<String, IdleResourceAnalysis>,
}

impl IdleRecommendationStore {
    /// Record `analysis` under the identity of the resource it is about. Re-recording the
    /// same resource replaces the stored analysis in place, keeping its position in the
    /// order — a refreshed analysis of one resource does not reorder the rest.
    pub fn insert(&mut self, analysis: IdleResourceAnalysis) {
        let identity = analysis.identity();
        if !self.by_identity.contains_key(&identity) {
            self.order.push(identity.clone());
        }
        self.by_identity.insert(identity, analysis);
    }

    /// The analysis held for the resource `resource_arn` names, or `None` when the service
    /// holds none — which is what puts that ARN in a response's `errors` list. The lookup is
    /// region-free, so an ARN naming the resource at any regional endpoint finds it.
    pub fn get(&self, resource_arn: &str) -> Option<&IdleResourceAnalysis> {
        self.by_identity.get(&identity_of_arn(resource_arn)?)
    }

    /// Every recorded analysis, in record order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &IdleResourceAnalysis> {
        self.order
            .iter()
            .filter_map(|identity| self.by_identity.get(identity))
    }

    /// How many resources the analysis found idle.
    pub fn len(&self) -> usize {
        self.by_identity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_identity.is_empty()
    }
}

impl FromIterator<IdleResourceAnalysis> for IdleRecommendationStore {
    /// The store holding `analyses`, in the order they are given — how the seeded analysis
    /// becomes the store's initial value.
    fn from_iter<T: IntoIterator<Item = IdleResourceAnalysis>>(analyses: T) -> Self {
        let mut store = Self::default();
        for analysis in analyses {
            store.insert(analysis);
        }
        store
    }
}

/// The idle resource analysis this emulator holds, built once when the store is created.
///
/// Compute Optimizer "generates recommendations for idle resources that meet a specific set of
/// requirements" from the utilization it observes, so WHICH resources are idle is analysis
/// output and not something a request writes: like the EBS volume observation, the Auto
/// Scaling group analysis and the findings catalog, it is part of the store's initial value,
/// and every figure a reader reports is computed from what is recorded here.
///
/// The set holds the resource kinds Compute Optimizer reports ONLY as idle resources — a NAT
/// gateway, a DynamoDB table, an ElastiCache cluster, a MemoryDB cluster, a DocumentDB
/// cluster, a WorkSpace and a SageMaker endpoint — plus an UNATTACHED Amazon EBS volume,
/// which is not part of the volume inventory [`observed_volumes`] holds either, because a
/// volume attached to nothing carries no workload to rightsize. Every `IdleFinding` value
/// occurs, the costs all differ so an ordering is observable, the observed periods differ so a
/// look-back preference is observable, and each row's metrics are the ones its own resource
/// kind publishes.
///
/// A row records the resource's REGION-FREE identity ([`resource_arn_in`]): Compute Optimizer
/// is regional, so the region a reported ARN names is the region of the endpoint the request
/// addressed.
fn seed_idle_resource_analyses() -> Vec<IdleResourceAnalysis> {
    let account_id = ::engine::emulator_account_id().to_string();
    // Every metric this analysis records is the highest value observed over the period, which
    // is the one statistic this API reports ([`METRIC_STATISTIC_MAXIMUM`]).
    let metric = |name: &str, value: f64| IdleUtilizationMetric {
        name: Some(name.to_string()),
        statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
        value,
        // The analysis recorded no dimension breakdown for these metrics.
        dimensions: None,
    };
    let row = |arn_service: &str,
               arn_resource: &str,
               finding: &str,
               utilization_metrics: Vec<IdleUtilizationMetric>,
               days_observed: f64,
               monthly_cost: f64,
               tags: &[(&str, &str)]| IdleResourceAnalysis {
        account_id: account_id.clone(),
        arn_service: arn_service.to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        utilization_metrics,
        days_observed,
        monthly_cost: round_hundredths(monthly_cost),
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    };

    vec![
        // A NAT gateway nothing routes through: no connection and no packet from either
        // side. Priced at the published $0.045 per gateway-hour.
        row(
            "ec2",
            "natgateway/nat-0a1b2c3d4e5f60001",
            IDLE_FINDING_UNUSED,
            vec![
                metric("ActiveConnectionCount", 0.0),
                metric("PacketsInFromSource", 0.0),
                metric("PacketsInFromDestination", 0.0),
            ],
            60.0,
            0.045 * HOURS_PER_MONTH,
            &[("Name", "legacy-egress")],
        ),
        // A 200 GiB gp2 volume attached to nothing: no read and no write operation reaches
        // it. Priced by [`volume_monthly_cost`], the same table the volume reader prices
        // from, so one volume type has one price in this service.
        row(
            "ec2",
            "volume/vol-0a1b2c3d4e5f60009",
            IDLE_FINDING_UNATTACHED,
            vec![
                metric("VolumeReadOpsPerSecond", 0.0),
                metric("VolumeWriteOpsPerSecond", 0.0),
            ],
            93.0,
            volume_monthly_cost(&VolumeConfiguration {
                volume_type: Some("gp2".to_string()),
                volume_size: 200,
                volume_baseline_iops: 600,
                volume_burst_iops: 3_000,
                volume_baseline_throughput: 128,
                volume_burst_throughput: 128,
                root_volume: Some(false),
            })
            .unwrap_or_default(),
            &[("Owner", "TeamA")],
        ),
        // A provisioned DynamoDB table nothing reads or writes. Priced at its provisioned
        // capacity: 20 read and 20 write capacity units at the published $0.00013 and
        // $0.00065 per unit-hour.
        row(
            "dynamodb",
            "table/legacy-sessions",
            IDLE_FINDING_IDLE,
            vec![
                metric("ConsumedReadCapacityUnits", 0.0),
                metric("ConsumedWriteCapacityUnits", 0.0),
                metric("ConsumedChangeDataCaptureUnits", 0.0),
            ],
            14.0,
            20.0 * (0.000_13 + 0.000_65) * HOURS_PER_MONTH,
            &[("Owner", "TeamB")],
        ),
        // An ElastiCache cluster serving nothing: no command, no connection and a processor
        // that is all but idle. Priced at the published $0.154 per cache.m6g.large-hour.
        row(
            "elasticache",
            "cluster:sessions-cache",
            IDLE_FINDING_IDLE,
            vec![
                metric("EngineCPUUtilization", 0.8),
                metric("CurrConnections", 0.0),
                metric("GetTypeCmds", 0.0),
                metric("SetTypeCmds", 0.0),
            ],
            30.0,
            0.154 * HOURS_PER_MONTH,
            &[],
        ),
        // A MemoryDB cluster nothing has queried: no keyspace hit and no keyspace miss.
        // Priced at the published $0.068 per db.t4g.small-hour.
        row(
            "memorydb",
            "cluster/leaderboard-archive",
            IDLE_FINDING_UNUSED,
            vec![
                metric("KeyspaceHits", 0.0),
                metric("KeyspaceMisses", 0.0),
                metric("NewConnections", 0.0),
            ],
            9.0,
            0.068 * HOURS_PER_MONTH,
            &[("Owner", "TeamB"), ("Environment", "Archive")],
        ),
        // A DocumentDB cluster no client connects to. Priced at the published $0.078 per
        // db.t3.medium-hour.
        row(
            "rds",
            "cluster:docdb-reporting",
            IDLE_FINDING_IDLE,
            vec![
                metric("DatabaseConnections", 0.0),
                metric("CPU", 1.1),
                metric("Memory", 12.0),
            ],
            45.0,
            0.078 * HOURS_PER_MONTH,
            &[],
        ),
        // A WorkSpace nobody logs in to, billed monthly: the published $35.00 per month of a
        // Value bundle on monthly billing.
        row(
            "workspaces",
            "workspace/ws-0a1b2c3d4e5f6",
            IDLE_FINDING_UNUSED,
            vec![metric("UserConnected", 0.0), metric("IsIdle", 1.0)],
            93.0,
            35.00,
            &[("Owner", "TeamC")],
        ),
        // A SageMaker endpoint nothing invokes. Priced at the published $0.115 per
        // ml.m5.large-hour.
        row(
            "sagemaker",
            "endpoint/scoring-canary",
            IDLE_FINDING_IDLE,
            vec![metric("Invocations", 0.0), metric("CPU", 0.6)],
            21.0,
            0.115 * HOURS_PER_MONTH,
            &[("Owner", "TeamC")],
        ),
    ]
}

/// The `IdleRecommendationResourceType` the `<service>:<resource-kind>` ARN fields name, or
/// `None` for a resource kind idle recommendations do not cover.
///
/// The catalog is the model's own `IdleRecommendationResourceType` value set
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`), each value paired with
/// the `<service>:<resource-kind>` ARN fields AWS gives that resource. A DocumentDB cluster
/// carries the `rds` namespace, and the value set declares no other cluster of that
/// namespace — no Aurora or RDS DB cluster value — so `rds:cluster` maps to
/// `DocumentDBCluster`, the one cluster type idle recommendations cover.
pub fn idle_resource_type_of(service: &str, resource_kind: &str) -> Option<&'static str> {
    let resource_type = match (service, resource_kind) {
        ("ec2", "instance") => "EC2Instance",
        ("ec2", "volume") => "EBSVolume",
        ("ec2", "natgateway") => "NatGateway",
        ("autoscaling", "autoScalingGroup") => "AutoScalingGroup",
        ("ecs", "service") => "ECSService",
        ("rds", "db") => "RDSDBInstance",
        ("rds", "cluster") => "DocumentDBCluster",
        ("dynamodb", "table") => "DynamoDBTable",
        ("elasticache", "cluster") => "ElastiCacheCluster",
        ("memorydb", "cluster") => "MemoryDBCluster",
        ("workspaces", "workspace") => "WorkSpaces",
        ("sagemaker", "endpoint") => "SageMakerEndpoint",
        _ => return None,
    };
    debug_assert!(
        IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES.contains(&resource_type),
        "the catalog may only name modelled IdleRecommendationResourceType values"
    );
    Some(resource_type)
}

/// The `RecommendationSourceType` a resource of `idle_resource_type` is SUMMARISED under,
/// or `None` for a type outside the idle vocabulary.
///
/// A recommendation summary names its group with a `RecommendationSourceType`
/// (`RecommendationSummary$recommendationResourceType`) while an idle record names its
/// resource with an `IdleRecommendationResourceType`
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`), and the two enums spell
/// several of the same resources differently (`EC2Instance` against `Ec2Instance`,
/// `EBSVolume` against `EbsVolume`, `ECSService` against `EcsService`, `RDSDBInstance`
/// against `RdsDBInstance`); the rest spell them identically. Stated once here, so the idle
/// analysis of one resource is summarised under the same group the rightsizing analysis of
/// that resource is.
pub fn source_type_of_idle_resource_type(idle_resource_type: &str) -> Option<&'static str> {
    let source_type = match idle_resource_type {
        "EC2Instance" => SOURCE_TYPE_EC2_INSTANCE,
        "EBSVolume" => SOURCE_TYPE_EBS_VOLUME,
        "ECSService" => SOURCE_TYPE_ECS_SERVICE,
        "RDSDBInstance" => SOURCE_TYPE_RDS_DB_INSTANCE,
        "AutoScalingGroup" => SOURCE_TYPE_AUTO_SCALING_GROUP,
        "NatGateway" => SOURCE_TYPE_NAT_GATEWAY,
        "DynamoDBTable" => SOURCE_TYPE_DYNAMODB_TABLE,
        "ElastiCacheCluster" => SOURCE_TYPE_ELASTICACHE_CLUSTER,
        "MemoryDBCluster" => SOURCE_TYPE_MEMORYDB_CLUSTER,
        "DocumentDBCluster" => SOURCE_TYPE_DOCUMENTDB_CLUSTER,
        "WorkSpaces" => SOURCE_TYPE_WORKSPACES,
        "SageMakerEndpoint" => SOURCE_TYPE_SAGEMAKER_ENDPOINT,
        _ => return None,
    };
    debug_assert!(
        RECOMMENDATION_SOURCE_TYPE_VALUES.contains(&source_type),
        "a summary group may only name a modelled RecommendationSourceType value"
    );
    Some(source_type)
}

/// Whether this service's idle analysis covers resources of `source_type`, i.e. whether a
/// summary of that group has an `IdleFinding` vocabulary to count at all.
///
/// The idle vocabulary is `IdleRecommendationResourceType`
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`) and a summary names its
/// group with a `RecommendationSourceType`, so membership is decided by mapping the idle
/// vocabulary through [`source_type_of_idle_resource_type`] rather than by a second list
/// that could drift from it — and so a `RecommendationSourceType` outside the idle
/// vocabulary (a Lambda function, a license, a storage group) is summarised with no idle
/// counts at all.
pub fn reports_idle_recommendations(source_type: &str) -> bool {
    IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES
        .iter()
        .filter_map(|idle_resource_type| source_type_of_idle_resource_type(idle_resource_type))
        .any(|covered| covered == source_type)
}

/// Check an `IdleRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is an `IdleRecommendationFilterName` and the values a filter carries are the
/// value set of the member that name selects on: `Finding` selects on `IdleFinding` and
/// `ResourceType` on `IdleRecommendationResourceType`. A name or a value outside those sets
/// is refused rather than silently matching nothing.
pub fn idle_filter_rejection(filters: &[IdleRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                IDLE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let vocabulary: &[&str] = match name {
            IDLE_FILTER_FINDING => IDLE_FINDING_VALUES,
            IDLE_FILTER_RESOURCE_TYPE => IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES,
            _ => {
                return Some(format!(
                    "The value supplied for parameter filters is not valid: {name} is not a \
                     filter name, which must be one of [{}].",
                    IDLE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
                ))
            }
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Does `recommendation` satisfy every `GetIdleRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values;
/// several filters are ANDed, and a filter carrying no values constrains nothing. A member
/// the stored recommendation does not carry matches nothing. An unrecognised name never
/// reaches here — [`idle_filter_rejection`] refuses the request first.
pub fn idle_recommendation_matches_filters(
    recommendation: &IdleRecommendation,
    filters: &[IdleRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let member = match filter.name.as_deref() {
            Some(IDLE_FILTER_FINDING) => recommendation.finding.as_deref(),
            Some(IDLE_FILTER_RESOURCE_TYPE) => recommendation.resource_type.as_deref(),
            _ => None,
        };
        member.is_some_and(|member| values.iter().any(|value| value == member))
    })
}

/// The figure `OrderBy$dimension` sorts a recommendation on: the estimated monthly savings
/// of the dimension's savings opportunity — `SavingsValue` reads the opportunity before
/// discounts, `SavingsValueAfterDiscount` the one after them. A recommendation that carries
/// no estimate for that dimension sorts as the modelled default of the member it lacks
/// (`IdleEstimatedMonthlySavings$value` defaults to `0`).
pub fn idle_savings_value(recommendation: &IdleRecommendation, dimension: &str) -> f64 {
    // The two dimensions the model declares, each reading its own savings opportunity. A
    // dimension outside them is refused by the modelled `@enum` on `OrderBy$dimension` at
    // the request boundary; the before-discount figure is the conservative reading of one
    // this model does not yet declare.
    let after_discounts = dimension == DIMENSION_SAVINGS_VALUE_AFTER_DISCOUNT;
    let estimate = if after_discounts {
        recommendation
            .savings_opportunity_after_discounts
            .as_ref()
            .and_then(|opportunity| opportunity.estimated_monthly_savings.as_ref())
    } else {
        recommendation
            .savings_opportunity
            .as_ref()
            .and_then(|opportunity| opportunity.estimated_monthly_savings.as_ref())
    };
    estimate.map_or(0.0, |savings| savings.value)
}

/// Whether an `OrderBy$order` sorts descending. `Desc` does; `Asc`, an absent order and a
/// value the modelled `@enum` on `OrderBy$order` would refuse at the request boundary all
/// sort ascending, because the model documents no default direction and ascending is this
/// emulator's documented choice.
pub fn idle_sorts_descending(order: Option<&str>) -> bool {
    order == Some(ORDER_DESC)
}

/// Where a page of a paginated read resumed from: how far into the selected sequence the
/// previous page stopped, and a fingerprint of the selection it was taken with, so resuming
/// with a different selection is refused instead of answered with a page of another result
/// set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// How many selected recommendations the previous pages already reported.
    pub offset: usize,
    /// The fingerprint of the request members that select and order the result set.
    pub selection_fingerprint: String,
}

/// A stable digest of `value`'s debug rendering, used as the selection fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its members in
/// declaration order, so the same selection always digests to the same string and a
/// different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    ::hex::encode(Sha256::digest(format!("{value:?}").as_bytes()))
}

/// The opaque `nextToken` an offset-paged reader hands out: the cursor itself, hex-encoded,
/// so a page token needs no server-side entry and a read never writes. Shared by every
/// reader of this service that walks a selected sequence by offset, each of which binds its
/// own operation name into the selection fingerprint, so one reader's token resumes no
/// other's walk.
pub fn encode_offset_page_token(cursor: &PageCursor) -> String {
    ::hex::encode(format!("{}:{}", cursor.offset, cursor.selection_fingerprint))
}

/// The cursor a `nextToken` this service issued stands for, or `None` for a token it did
/// not issue. What a decoded cursor may resume is the caller's own check: it compares the
/// cursor's fingerprint against the one it would issue for this request.
pub fn decode_offset_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (offset, selection_fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        selection_fingerprint: selection_fingerprint.to_string(),
    })
}

/// How large a page a paginated reader of this service builds for a request.
///
/// The rule every one of them follows, stated once: `max_results` sizes the page when it asks
/// for a size, `default` sizes it when the request asks for none — an absent member, or the
/// value `0`, the lower bound of the modelled range, which the service documents no meaning
/// for, so it reads as "no size asked for" rather than as an empty page a walk could never
/// advance through — and the result is never larger than `cap`, so no number a request
/// carries can size a response or the allocation that builds it. A reader passes its OWN
/// default and its own ceiling, which differ between operations ([`DEFAULT_PAGE_SIZE`] and
/// [`MAX_PAGE_SIZE`] for a reader the model bounds at 1000, [`IDLE_MAX_PAGE_SIZE`] and
/// [`ENROLLMENT_STATUSES_MAX_PAGE_SIZE`] for the two the service bounds lower); the rule
/// applied to them does not.
pub fn page_size(max_results: Option<i32>, default: usize, cap: usize) -> usize {
    max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(default)
        .min(cap)
}

/// One page of `selected` and the `nextToken` that resumes the walk after it.
///
/// The tail every offset-paged reader of this service ends with, stated once: the page is the
/// `page_size` items of the selected sequence that follow `offset`, and a token is handed out
/// only when the sequence CONTINUES past the page — decided by reading one item further than
/// the page reports — resuming at exactly the item the page stopped before, under the
/// selection the page was taken with ([`PageCursor`], [`encode_offset_page_token`]). So the
/// last page of a walk carries no token, and no two readers can disagree about when a walk
/// has ended or where it resumes.
pub fn offset_page<T>(
    selected: Vec<T>,
    offset: usize,
    page_size: usize,
    selection_fingerprint: String,
) -> (Vec<T>, Option<String>) {
    let mut page: Vec<T> = selected
        .into_iter()
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
    let has_more = page.len() > page_size;
    page.truncate(page_size);
    let next_token = has_more.then(|| {
        encode_offset_page_token(&PageCursor {
            offset: offset.saturating_add(page.len()),
            selection_fingerprint,
        })
    });
    (page, next_token)
}

// ---------------------------------------------------------------------------
// Amazon Aurora and RDS database recommendations
//
// `GetRDSDatabaseRecommendations` "Returns Amazon Aurora and RDS database
// recommendations", which Compute Optimizer "generates ... for Amazon Aurora and RDS
// databases that meet a specific set of requirements" from the utilization it OBSERVES.
// So, like every other findings reader of this service, it REPORTS the rows the findings
// catalog holds — here the ones analysed as `RdsDBInstance`, the source type the model's
// `RDSDBRecommendation` describes — and writes nothing. The rules below are the ones that
// turn a stored row into the modelled recommendation and select over it: the filter
// vocabulary, which finding member a stored `Finding` is reported on, and how a look-back
// preference reads as a number of days.
// ---------------------------------------------------------------------------

/// The `RDSDBRecommendationFilterName` values
/// (`crate::ops::types::R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES`), named so the filter
/// rules below read as the model's own vocabulary.
pub const RDS_FILTER_INSTANCE_FINDING: &str = "InstanceFinding";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_INSTANCE_FINDING_REASON_CODE: &str = "InstanceFindingReasonCode";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_STORAGE_FINDING: &str = "StorageFinding";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_STORAGE_FINDING_REASON_CODE: &str = "StorageFindingReasonCode";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_IDLE: &str = "Idle";

/// The `Idle` enum values (`crate::ops::types::IDLE_VALUES`), which `RDSDBRecommendation$idle`
/// carries: whether the service also holds an IDLE recommendation for the database.
pub const IDLE_TRUE: &str = "True";
/// See [`IDLE_TRUE`].
pub const IDLE_FALSE: &str = "False";

/// What the findings catalog records about a resource it analysed as an Amazon Aurora or RDS
/// DATABASE, beyond the finding, risk and costs every analysed resource carries: the
/// database's configuration, the configuration the analysis would move it to, and the
/// utilization it observed.
///
/// `None` on a row the analysis recorded no database configuration for, and every field is
/// optional in its own right, so a reader reports exactly what was observed and omits the
/// members nothing was recorded for rather than inventing a value for them.
#[derive(Debug, Clone, Default)]
pub struct RdsDatabaseDetail {
    /// `RDSDBRecommendation$engine` — "The database engine of the DB instance".
    pub engine: Option<String>,
    /// `RDSDBRecommendation$engineVersion`.
    pub engine_version: Option<String>,
    /// `RDSDBRecommendation$promotionTier`, set only for an Aurora cluster member.
    pub promotion_tier: Option<i32>,
    /// `RDSDBRecommendation$dbClusterIdentifier`, set only for a cluster member.
    pub db_cluster_identifier: Option<String>,
    /// `RDSDBRecommendation$currentDBInstanceClass`.
    pub current_db_instance_class: Option<String>,
    /// The DB instance class the analysis would move the database to, which is what an
    /// `RDSDBInstanceRecommendationOption` names. Absent where the analysis would keep the
    /// current class.
    pub recommended_db_instance_class: Option<String>,
    /// `RDSDBRecommendation$currentStorageConfiguration`.
    pub current_storage_configuration: Option<DBStorageConfiguration>,
    /// The storage configuration the analysis would move the database to, which is what an
    /// `RDSDBStorageRecommendationOption` names.
    pub recommended_storage_configuration: Option<DBStorageConfiguration>,
    /// The `RDSDBUtilizationMetric`s the analysis observed over its look-back period.
    pub utilization_metrics: Vec<RDSDBUtilizationMetric>,
    /// When the recommendation was last generated (`lastRefreshTimestamp`).
    pub last_refresh_timestamp: Option<DateTime>,
    /// The tags of the database.
    pub tags: Vec<Tag>,
}

/// Which `RDSDBRecommendation` findings a stored `Finding` is reported as: the instance
/// finding and the storage finding, in that order.
///
/// Compute Optimizer analyses a database's INSTANCE and its STORAGE, and reports a finding
/// for each, in the vocabulary the model declares for that member: `RDSInstanceFinding` is
/// `Optimized | Overprovisioned | Underprovisioned`, and `RDSStorageFinding` declares those
/// three plus `NotOptimized`. The catalog records ONE finding per analysed resource, so that
/// finding is reported on the side whose enum declares it — the instance side when both do,
/// since `NotOptimized` is the one finding only storage can carry — and the other side
/// reports `Optimized`, the value both vocabularies use for a configuration the analysis
/// would not change, because the analysis recorded nothing against it. Every `Finding` the
/// catalog can hold is declared by at least one of the two; a value outside both is reported
/// on neither member.
pub fn rds_finding_members(finding: &str) -> (Option<&str>, Option<&str>) {
    // `FINDING_OPTIMIZED` is the `Finding` value `RDSInstanceFinding` and `RDSStorageFinding`
    // both declare, so one constant names it for all three.
    match (
        R_D_S_INSTANCE_FINDING_VALUES.contains(&finding),
        R_D_S_STORAGE_FINDING_VALUES.contains(&finding),
    ) {
        (true, _) => (Some(finding), Some(FINDING_OPTIMIZED)),
        (false, true) => (Some(FINDING_OPTIMIZED), Some(finding)),
        (false, false) => (None, None),
    }
}

/// The stored reason codes `vocabulary` declares, in stored order.
///
/// A resource's reason codes are reported on the member whose enum declares them —
/// `instanceFindingReasonCodes` targets `RDSInstanceFindingReasonCode` and
/// `storageFindingReasonCodes` targets `RDSStorageFindingReasonCode` — so a stored code is
/// reported on the member that can carry it and on no other.
pub fn reason_codes_in(vocabulary: &[&str], codes: &[String]) -> Vec<String> {
    codes
        .iter()
        .filter(|code| vocabulary.contains(&code.as_str()))
        .cloned()
        .collect()
}

/// The `FilterName` values (`crate::ops::types::FILTER_NAME_VALUES`), named so the filter
/// rule below reads as the model's own vocabulary.
pub const FILTER_FINDING: &str = "Finding";
/// See [`FILTER_FINDING`].
pub const FILTER_FINDING_REASON_CODES: &str = "FindingReasonCodes";
/// See [`FILTER_FINDING`].
pub const FILTER_RECOMMENDATION_SOURCE_TYPE: &str = "RecommendationSourceType";
/// See [`FILTER_FINDING`].
pub const FILTER_INFERRED_WORKLOAD_TYPES: &str = "InferredWorkloadTypes";

/// Check a `Filters` list — "a filter that returns a more specific list of
/// recommendations", the filter the EC2 instance and Auto Scaling group surface selects
/// with — against the vocabulary the model declares for it, returning the reason it is not
/// valid.
///
/// `name` is a `FilterName` and the values a filter carries are the value set of the member
/// that name selects on. A name or a value outside those sets is refused rather than
/// silently matching nothing — the same rule [`rds_filter_rejection`] applies to the
/// database surface's filters. A value the model declares is well formed even where no
/// recommendation of the resource type asked about can carry it (an Auto Scaling group
/// reports no finding reason codes): such a filter selects nothing. Shared by every reader
/// and export of this surface, so none of them accepts a filter another refuses.
pub fn resource_filter_rejection(filters: &[Filter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = resource_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Whether one recommendation of this surface satisfies `filter`.
///
/// A filter with no name, or one with no values, constrains nothing. `Finding` matches the
/// finding the analysis reached, `FindingReasonCodes` any reason code behind it,
/// `RecommendationSourceType` the `RecommendationSourceType` of the resource the
/// recommendation is about, and `InferredWorkloadTypes` any workload type the analysis
/// inferred for it — so a reader passes the EMPTY list for a recommendation that reports none,
/// whether because its resource type has none (an Auto Scaling group reports no finding reason
/// codes) or because the account deactivated the inference, and such a filter then selects
/// nothing rather than matching everything.
///
/// The vocabulary itself is checked up front ([`resource_filter_rejection`]), so a filter
/// reaching here carries a name and values this surface declares. ONE matcher for the whole
/// surface, so the readers of two resource types cannot drift apart on the same filter.
pub fn resource_filter_matches(
    filter: &Filter,
    finding: &str,
    finding_reason_codes: &[String],
    inferred_workload_types: &[String],
    source_type: &str,
) -> bool {
    let Some(name) = filter.name.as_deref() else {
        return true;
    };
    let values = filter.values.as_deref().unwrap_or_default();
    if values.is_empty() {
        return true;
    }
    let reported: &[String] = match name {
        FILTER_FINDING => return values.iter().any(|value| value == finding),
        FILTER_RECOMMENDATION_SOURCE_TYPE => {
            return values.iter().any(|value| value == source_type)
        }
        FILTER_FINDING_REASON_CODES => finding_reason_codes,
        FILTER_INFERRED_WORKLOAD_TYPES => inferred_workload_types,
        // A name outside this surface's vocabulary cannot arrive (see
        // [`resource_filter_rejection`]), and selects nothing if it does.
        _ => return false,
    };
    values.iter().any(|value| reported.contains(value))
}

/// The value set a `FilterName` selects on, or `None` for a name the model does not declare.
///
/// `FindingReasonCodes` selects on either reason-code vocabulary this surface declares; the
/// instance one is the superset, so it is what a filter's values are checked against.
fn resource_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        FILTER_FINDING => Some(FINDING_VALUES),
        FILTER_RECOMMENDATION_SOURCE_TYPE => Some(RECOMMENDATION_SOURCE_TYPE_VALUES),
        FILTER_INFERRED_WORKLOAD_TYPES => Some(INFERRED_WORKLOAD_TYPE_VALUES),
        FILTER_FINDING_REASON_CODES => {
            Some(INSTANCE_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

/// Check an `RDSDBRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is an `RDSDBRecommendationFilterName` and the values a filter carries are the value
/// set of the member that name selects on. A name or a value outside those sets is refused
/// rather than silently matching nothing — the same rule [`idle_filter_rejection`] applies to
/// the other findings reader's filters.
pub fn rds_filter_rejection(filters: &[RDSDBRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = rds_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set an `RDSDBRecommendationFilterName` selects on, or `None` for a name the
/// model does not declare.
fn rds_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        RDS_FILTER_INSTANCE_FINDING => Some(R_D_S_INSTANCE_FINDING_VALUES),
        RDS_FILTER_INSTANCE_FINDING_REASON_CODE => {
            Some(R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES)
        }
        RDS_FILTER_STORAGE_FINDING => Some(R_D_S_STORAGE_FINDING_VALUES),
        RDS_FILTER_STORAGE_FINDING_REASON_CODE => Some(R_D_S_STORAGE_FINDING_REASON_CODE_VALUES),
        RDS_FILTER_IDLE => Some(IDLE_VALUES),
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetRDSDatabaseRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for a member that is a LIST of reason codes, when any stored code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`rds_filter_rejection`] refuses the request first.
pub fn rds_recommendation_matches_filters(
    recommendation: &RDSDBRecommendation,
    filters: &[RDSDBRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let matches_one = |member: Option<&str>| {
            member.is_some_and(|member| values.iter().any(|value| value == member))
        };
        let matches_any = |members: Option<&Vec<String>>| {
            members.is_some_and(|members| {
                members
                    .iter()
                    .any(|member| values.iter().any(|value| value == member))
            })
        };
        match filter.name.as_deref() {
            Some(RDS_FILTER_INSTANCE_FINDING) => matches_one(recommendation.instance_finding.as_deref()),
            Some(RDS_FILTER_STORAGE_FINDING) => matches_one(recommendation.storage_finding.as_deref()),
            Some(RDS_FILTER_IDLE) => matches_one(recommendation.idle.as_deref()),
            Some(RDS_FILTER_INSTANCE_FINDING_REASON_CODE) => {
                matches_any(recommendation.instance_finding_reason_codes.as_ref())
            }
            Some(RDS_FILTER_STORAGE_FINDING_REASON_CODE) => {
                matches_any(recommendation.storage_finding_reason_codes.as_ref())
            }
            _ => false,
        }
    })
}

/// Does the analysis of `resource` record a different DB INSTANCE configuration for the
/// database — the condition an `RDSDBInstanceRecommendationOption` is reported under?
///
/// The analysis would move the database's instance only where the configuration it
/// recommends costs something other than the current one AND the instance finding is not
/// `Optimized`, the value that says the analysis found nothing to change on that side.
/// Stated once here so the two readers of one catalog row — the recommendation and the
/// metrics projected for its option — cannot disagree about whether the row has an instance
/// recommendation option at all.
pub fn rds_changes_instance_configuration(resource: &AnalysedResource) -> bool {
    let (instance_finding, _) = rds_finding_members(&resource.finding);
    resource.recommended_monthly_cost != resource.current_monthly_cost
        && instance_finding.is_some_and(|finding| finding != FINDING_OPTIMIZED)
}

/// The capacity of one DB instance class — or EC2 instance type — RELATIVE to the `xlarge`
/// size of the same family, or `None` for a size name outside the catalog below.
///
/// AWS names an instance size `<family>.<size>`, a DB instance class prefixing `db.`, and
/// the standard size names are a doubling sequence — `nano`, `micro`, `small`, `medium`,
/// `large`, `xlarge` — above which `<N>xlarge` carries N times the vCPU and memory of
/// `xlarge`. The catalog is that sequence, each size expressed as a multiple of `xlarge`; a
/// size name outside it (`metal`, say) has no multiple recorded, so a caller projects
/// nothing from it rather than guessing one.
pub fn instance_size_capacity(instance_class: &str) -> Option<f64> {
    let size = instance_class.split('.').next_back()?;
    if let Some(multiple) = size.strip_suffix("xlarge") {
        if multiple.is_empty() {
            return Some(1.0);
        }
        return multiple.parse::<u16>().ok().map(f64::from);
    }
    // Each named size below `xlarge` halves the one above it.
    match size {
        "large" => Some(0.5),
        "medium" => Some(0.25),
        "small" => Some(0.125),
        "micro" => Some(0.062_5),
        "nano" => Some(0.031_25),
        _ => None,
    }
}

/// The `RDSDBMetricName` values ([`R_D_S_D_B_METRIC_NAME_VALUES`]) that measure, as a
/// percentage, a resource the DB INSTANCE CLASS sizes: the instance's vCPU and its memory.
/// Moving the database to another instance class changes what those percentages are OF, so
/// they are the metrics a projection scales; every other metric of that value set measures
/// the workload itself (connections, throughput, IOPs) or the database's storage, neither of
/// which an instance-class change resizes.
pub const RDS_METRIC_CPU: &str = "CPU";
/// See [`RDS_METRIC_CPU`].
pub const RDS_METRIC_MEMORY: &str = "Memory";

/// The highest value a metric measured as a PERCENTAGE of a capacity can take.
pub const UTILIZATION_PERCENTAGE_CEILING: f64 = 100.0;

/// `observed` — a percentage of a capacity of `from` — as a percentage of a capacity of `to`.
///
/// The same work runs on whatever capacity it is moved to, so the percentage scales with the
/// ratio of the two capacities: the same work on half the capacity runs at twice the
/// utilization. It is a percentage, so it cannot pass [`UTILIZATION_PERCENTAGE_CEILING`], and
/// a capacity of nothing is no ratio at all, so the observation stands.
///
/// The ONE percentage projection in this service: a resource's utilization projected onto a
/// recommendation option means the same thing, and rounds to the same place, for an Amazon EC2
/// instance, an Auto Scaling group and an Amazon Aurora or RDS database.
pub fn project_percentage(observed: f64, from: f64, to: f64) -> f64 {
    if to <= 0.0 {
        return round_hundredths(observed);
    }
    round_hundredths((observed * from / to).clamp(0.0, UTILIZATION_PERCENTAGE_CEILING))
}

/// The value `metric_name` is projected to take on the recommended DB instance class, given
/// the `observed` value on the current one.
///
/// A metric measured as a percentage of what the instance class sizes ([`RDS_METRIC_CPU`],
/// [`RDS_METRIC_MEMORY`]) is projected by the ratio of the two classes' capacities — the
/// same work on half the capacity runs at twice the utilization — and cannot exceed the
/// percentage ceiling. Every other metric measures the workload or the storage, which an
/// instance-class change does not resize, so it is projected unchanged. A class whose size
/// name the catalog records no capacity for ([`instance_size_capacity`]) leaves the metric
/// unchanged too, because there is then no ratio to project by.
pub fn rds_projected_metric_value(
    metric_name: &str,
    observed: f64,
    current_instance_class: Option<&str>,
    recommended_instance_class: Option<&str>,
) -> f64 {
    if !rds_metric_sized_by_instance_class(Some(metric_name)) {
        return observed;
    }
    let capacities = current_instance_class
        .and_then(instance_size_capacity)
        .zip(recommended_instance_class.and_then(instance_size_capacity));
    let Some((current, recommended)) = capacities else {
        return observed;
    };
    project_percentage(observed, current, recommended)
}

/// Whether an `RDSDBMetricName` measures, as a percentage, a resource the DB instance class
/// sizes — the metrics [`RDS_METRIC_CPU`] and [`RDS_METRIC_MEMORY`] name, and so the ones a
/// projection scales and a risk is read off.
pub fn rds_metric_sized_by_instance_class(metric_name: Option<&str>) -> bool {
    metric_name.is_some_and(|name| name == RDS_METRIC_CPU || name == RDS_METRIC_MEMORY)
}

/// The utilization the observed workload is projected to reach once the database runs on
/// `recommended_instance_class`, as
/// `RDSDBInstanceRecommendationOption$projectedUtilizationMetrics` reports it: every observed
/// metric, each projected by the ONE rule [`rds_projected_metric_value`] states, so the
/// option's projection and the series `GetRDSDatabaseRecommendationProjectedMetrics` projects
/// for that option cannot disagree. Empty for a database the analysis observed nothing on,
/// which then reports no projection rather than an invented one.
pub fn rds_projected_utilization_metrics(
    observed: &[RDSDBUtilizationMetric],
    current_instance_class: Option<&str>,
    recommended_instance_class: Option<&str>,
) -> Vec<RDSDBUtilizationMetric> {
    observed
        .iter()
        .map(|metric| RDSDBUtilizationMetric {
            value: rds_projected_metric_value(
                metric.name.as_deref().unwrap_or_default(),
                metric.value,
                current_instance_class,
                recommended_instance_class,
            ),
            ..metric.clone()
        })
        .collect()
}

/// The `PerformanceRisk` a DB instance recommendation option carries: how close the
/// utilization projected onto it ([`rds_projected_utilization_metrics`]) comes to the capacity
/// of the recommended class, on the model's 0..4 scale ([`performance_risk_score`]).
///
/// "The higher the risk, the more likely the current [configuration] doesn't have sufficient
/// capacity", so an option is as risky as its most loaded dimension: the highest projected
/// percentage of what the recommended class sizes, against the whole of that capacity
/// ([`UTILIZATION_PERCENTAGE_CEILING`]). An option nothing was projected onto carries no
/// risk, which is the member's modelled default.
pub fn rds_option_performance_risk(projected: &[RDSDBUtilizationMetric]) -> f64 {
    let peak = projected
        .iter()
        .filter(|metric| rds_metric_sized_by_instance_class(metric.name.as_deref()))
        .map(|metric| metric.value)
        .fold(0.0_f64, f64::max);
    round_hundredths(performance_risk_score(
        peak,
        UTILIZATION_PERCENTAGE_CEILING,
    ))
}

/// The rank of the one recommendation option this service reports for a database: "The rank
/// identifier of the DB instance recommendation option" — the analysis records one
/// configuration it would move the database to, so that option is the first-ranked one.
const RDS_FIRST_RANK: i32 = 1;

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// One analysed database as this service REPORTS it: the `RDSDBRecommendation`
    /// `GetRDSDatabaseRecommendations` answers for the catalog row `resource` at the endpoint
    /// of `region`, under the CPU vendor architectures the request asked for.
    ///
    /// Every member is computed from that row, from the database configuration the analysis
    /// recorded with it ([`RdsDatabaseDetail`]) and from the preference records in force for
    /// the database; nothing here is stored as a recommendation, and a member the analysis
    /// recorded nothing for is omitted rather than invented.
    ///
    /// Stated HERE rather than in the reader because an EXPORT of these recommendations fills
    /// each column of its export file from the member that column names ([`export`]), so the
    /// two readings of one catalog row cannot disagree about any of them.
    pub fn rds_recommendation(
        &self,
        resource: &AnalysedResource,
        region: &str,
        requested_cpu_vendor_architectures: Option<&[String]>,
    ) -> RDSDBRecommendation {
        let resource_arn = resource.arn_in(region);
        let detail = resource.rds_database.clone().unwrap_or_default();
        // The catalog records ONE finding per analysed resource; a database is reported with a
        // finding for its instance AND one for its storage, the side the analysis recorded
        // nothing against reading `Optimized`: see [`rds_finding_members`].
        let (instance_finding, storage_finding) = rds_finding_members(&resource.finding);
        let instance_finding_reason_codes = reason_codes_in(
            R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES,
            &resource.finding_reason_codes,
        );
        let storage_finding_reason_codes = reason_codes_in(
            R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
            &resource.finding_reason_codes,
        );

        // "Describes the effective recommendation preferences for Amazon Aurora and RDS
        // databases": the ONE resolution this service has ([`Self::effective_preferences`]),
        // applied to this database, so the preferences a recommendation says it was generated
        // under are the ones `GetEffectiveRecommendationPreferences` reports.
        let effective = self.effective_preferences_for(
            &resource_arn,
            // `ResourceType` and `RecommendationSourceType` name a database identically, so one
            // constant addresses both the preference records and the catalog rows.
            SOURCE_TYPE_RDS_DB_INSTANCE,
            &resource.account_id,
        );

        // The analysis would move the database only where the configuration it recommends
        // costs something different from the current one; where it would not, there is no
        // option to report. The option goes on the side that is not `Optimized`, because that
        // is the side the analysis found something to change on.
        let changes_configuration =
            resource.recommended_monthly_cost != resource.current_monthly_cost;
        let changes = |finding: Option<&str>| {
            changes_configuration && finding.is_some_and(|finding| finding != FINDING_OPTIMIZED)
        };
        // Whether savings are reported net of Savings Plans and Reserved Instances discounts:
        // the account activates that by storing the savings-estimation-mode preference —
        // "specify the `AfterDiscounts` status to activate the preference" — and until it
        // does, the member that preference activates is reported on no option.
        let after_discounts = effective.after_discounts();
        // The one saving this recommendation is for, computed once from the stored costs, so
        // every savings member of the response reports the same two figures.
        let (saved_percentage, saved) = rds_savings_figures(resource);
        // The INSTANCE side of that condition is [`rds_changes_instance_configuration`],
        // stated there so the metrics `GetRDSDatabaseRecommendationProjectedMetrics` projects
        // for an option are projected for exactly the rows that report one here.
        let instance_recommendation_options =
            rds_changes_instance_configuration(resource).then(|| {
                // The utilization the observed workload is projected to reach on the
                // recommended class, and the risk that class then runs: both computed from the
                // stored observation and the two class names, by the one rule this service
                // projects a resized database's utilization with
                // ([`rds_projected_metric_value`]) — so this option and the series
                // `GetRDSDatabaseRecommendationProjectedMetrics` projects for it cannot
                // disagree.
                let projected = rds_projected_utilization_metrics(
                    &detail.utilization_metrics,
                    detail.current_db_instance_class.as_deref(),
                    detail.recommended_db_instance_class.as_deref(),
                );
                let performance_risk = rds_option_performance_risk(&projected);
                vec![RDSDBInstanceRecommendationOption {
                    db_instance_class: detail.recommended_db_instance_class.clone(),
                    projected_utilization_metrics: (!projected.is_empty()).then_some(projected),
                    performance_risk,
                    rank: RDS_FIRST_RANK,
                    savings_opportunity: Some(rds_savings_opportunity(saved_percentage, saved)),
                    // This emulator prices a database from the public prices stored with its
                    // own analysis and records no Reserved Instances or Savings Plans
                    // commitment against its spend, so the after-discount figures are those
                    // before discounts — which is exactly what the reported
                    // [`SAVINGS_ESTIMATION_SOURCE`] says they came from.
                    savings_opportunity_after_discounts: after_discounts.then(|| {
                        RDSInstanceSavingsOpportunityAfterDiscounts {
                            savings_opportunity_percentage: saved_percentage,
                            estimated_monthly_savings: Some(RDSInstanceEstimatedMonthlySavings {
                                currency: Some(CURRENCY_USD.to_string()),
                                value: saved,
                            }),
                        }
                    }),
                }]
            });
        let storage_recommendation_options = changes(storage_finding).then(|| {
            vec![RDSDBStorageRecommendationOption {
                storage_configuration: detail.recommended_storage_configuration.clone(),
                rank: RDS_FIRST_RANK,
                savings_opportunity: Some(rds_savings_opportunity(saved_percentage, saved)),
                // See the instance option above: no discount is recorded against the
                // database's spend, so a saving after discounts is the saving before them.
                savings_opportunity_after_discounts: after_discounts.then(|| {
                    RDSStorageSavingsOpportunityAfterDiscounts {
                        savings_opportunity_percentage: saved_percentage,
                        estimated_monthly_savings: Some(RDSStorageEstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value: saved,
                        }),
                    }
                }),
                // "The estimated monthly volume IOPs cost variation" needs observed IOPs
                // pricing, which the catalog records none of.
                estimated_monthly_volume_io_ps_cost_variation: None,
            }]
        });

        RDSDBRecommendation {
            resource_arn: Some(resource_arn.clone()),
            account_id: Some(resource.account_id.clone()),
            engine: detail.engine,
            engine_version: detail.engine_version,
            promotion_tier: detail.promotion_tier,
            current_db_instance_class: detail.current_db_instance_class,
            current_storage_configuration: detail.current_storage_configuration,
            db_cluster_identifier: detail.db_cluster_identifier,
            // "Describes if an Amazon Aurora or RDS database is idle": it is exactly when this
            // service also holds an IDLE recommendation for it, which is the record
            // `GetIdleRecommendations` reports.
            idle: Some(
                if self.idle_recommendations.get(&resource_arn).is_some() {
                    IDLE_TRUE
                } else {
                    IDLE_FALSE
                }
                .to_string(),
            ),
            instance_finding: instance_finding.map(ToString::to_string),
            storage_finding: storage_finding.map(ToString::to_string),
            instance_finding_reason_codes: (!instance_finding_reason_codes.is_empty())
                .then_some(instance_finding_reason_codes),
            storage_finding_reason_codes: (!storage_finding_reason_codes.is_empty())
                .then_some(storage_finding_reason_codes),
            // `RDSCurrentInstancePerformanceRisk` declares the same four ratings as the
            // `CurrentPerformanceRisk` the catalog stores; a rating outside them is reported on
            // neither member.
            current_instance_performance_risk: R_D_S_CURRENT_INSTANCE_PERFORMANCE_RISK_VALUES
                .contains(&resource.current_performance_risk.as_str())
                .then(|| resource.current_performance_risk.clone()),
            current_storage_estimated_monthly_volume_io_ps_cost_variation: None,
            instance_recommendation_options,
            storage_recommendation_options,
            utilization_metrics: (!detail.utilization_metrics.is_empty())
                .then_some(detail.utilization_metrics),
            effective_recommendation_preferences: Some(RDSEffectiveRecommendationPreferences {
                // `recommendationPreferences` "Describes the recommendation preferences to
                // return in the response of a … GetRDSDatabaseRecommendations … request", so
                // the CPU vendor architecture the request asks for is the one in force for its
                // answer.
                cpu_vendor_architectures: requested_cpu_vendor_architectures
                    .filter(|architectures| !architectures.is_empty())
                    .map(<[String]>::to_vec),
                enhanced_infrastructure_metrics: effective.enhanced_infrastructure_metrics.clone(),
                look_back_period: Some(effective.look_back_period.clone()),
                // Where the money figures came from (`RDSSavingsEstimationModeSource`): every
                // one of them is computed from the prices stored with this service's own
                // analysis ([`SAVINGS_ESTIMATION_SOURCE`]), never from an account's own Cost
                // Explorer or Cost Optimization Hub data, whether or not the
                // savings-estimation-mode preference is active — what that preference changes
                // is which savings members an option carries, above.
                savings_estimation_mode: Some(RDSSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
            }),
            // "The number of days the DB instance utilization metrics were analyzed": the days
            // the look-back period in force covers.
            lookback_period_in_days: look_back_period_in_days(&effective.look_back_period),
            last_refresh_timestamp: detail.last_refresh_timestamp,
            tags: (!detail.tags.is_empty()).then_some(detail.tags),
        }
    }
}

/// The savings opportunity of adopting the recommendation for one database: what it saves
/// per month, and that saving "as a percentage of monthly cost"
/// (`SavingsOpportunity$savingsOpportunityPercentage`) — both from [`rds_savings_figures`], so
/// a reported saving and the costs it came from cannot disagree.
fn rds_savings_opportunity(saved_percentage: f64, saved: f64) -> SavingsOpportunity {
    SavingsOpportunity {
        savings_opportunity_percentage: saved_percentage,
        estimated_monthly_savings: Some(EstimatedMonthlySavings {
            currency: Some(CURRENCY_USD.to_string()),
            value: saved,
        }),
    }
}

/// What adopting the recommendation for one database saves per month, and that saving as a
/// percentage of what the database costs today: both computed from the stored current and
/// recommended monthly costs, so no savings member of a response or of an export row can
/// disagree with the costs beside it. A recommendation that costs MORE than the current
/// configuration saves nothing rather than a negative amount
/// ([`AnalysedResource::monthly_savings`]).
fn rds_savings_figures(resource: &AnalysedResource) -> (f64, f64) {
    let savings = resource.monthly_savings();
    (
        percentage_of(savings, resource.current_monthly_cost),
        round_hundredths(savings),
    )
}

/// The most data points one projected-metrics response carries, whatever window and
/// granularity a request names: a ceiling applied in the handler, so no number a request
/// carries can size a response or the allocation that builds it — the same rule
/// [`MAX_PAGE_SIZE`] states for a paged reader. A request that would cross it is refused
/// with the parameter named, because a coarser period or a shorter window asks for a
/// response this service can build.
pub const MAX_PROJECTED_METRIC_DATA_POINTS: usize = 1_440;

/// The timestamps of the projected metric data points of the window `[start, end)` at a
/// granularity of `period` seconds: one data point per period that BEGINS inside the window,
/// the first at `start`.
///
/// `None` for a window and granularity that is not a series at all (an empty or backwards
/// window, a period that is not positive) or that would carry more than
/// [`MAX_PROJECTED_METRIC_DATA_POINTS`] data points — both of which a caller refuses rather
/// than answer with a series the request did not ask for.
pub fn projected_metric_timestamps(start: f64, end: f64, period: f64) -> Option<Vec<DateTime>> {
    if !period.is_finite() || period <= 0.0 || !start.is_finite() || !end.is_finite() {
        return None;
    }
    if end <= start {
        return None;
    }
    let mut timestamps = Vec::new();
    let mut at = start;
    while at < end {
        if timestamps.len() >= MAX_PROJECTED_METRIC_DATA_POINTS {
            return None;
        }
        timestamps.push(DateTime::from_secs_f64(at));
        at += period;
    }
    Some(timestamps)
}

// ===========================================================================
// The Lambda function analysis `GetLambdaFunctionRecommendations` reports
//
// Compute Optimizer "generates recommendations for functions that meet a specific set
// of requirements" from the utilization it OBSERVES, so no request to this API creates
// a function recommendation: everything below is about the observation
// ([`ObservedFunction`]) and the recommendation DERIVED from it, so the per-function
// reader and `GetRecommendationSummaries` cannot disagree about a function's finding,
// its risk or what adopting its recommendation saves.
// ===========================================================================

/// The function version Lambda gives the unpublished version of a function, and the one
/// Compute Optimizer analyses for an unqualified ARN: "if you specify an unqualified ARN
/// without a function version suffix, Compute Optimizer will return recommendations for
/// the latest ($LATEST) version of the function"
/// (`GetLambdaFunctionRecommendations$functionArns`).
pub const LATEST_FUNCTION_VERSION: &str = "$LATEST";

/// The `LambdaFunctionRecommendationFilterName` values
/// ([`LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES`]), named so the filter rules below
/// read as the model's own vocabulary.
pub const LAMBDA_FILTER_FINDING: &str = "Finding";
/// See [`LAMBDA_FILTER_FINDING`].
pub const LAMBDA_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";

/// The `LambdaFunctionRecommendationFinding` value outside the service-wide `Finding`
/// vocabulary: "Compute Optimizer was unable to generate a recommendation for the
/// function". Its other two values are [`FINDING_OPTIMIZED`] and [`FINDING_NOT_OPTIMIZED`].
pub const LAMBDA_FINDING_UNAVAILABLE: &str = "Unavailable";

/// The `LambdaFunctionRecommendationFindingReasonCode` values of the `Unavailable` finding:
/// `InsufficientData` — "the function does not have sufficient metric data for Compute
/// Optimizer to generate a recommendation" — and `Inconclusive` — "the function does not
/// qualify for a recommendation because Compute Optimizer cannot generate a recommendation
/// with a high degree of confidence". The two codes of the `NotOptimized` finding are
/// [`METRIC_REASON_MEMORY_OVERPROVISIONED`] and [`METRIC_REASON_MEMORY_UNDERPROVISIONED`].
pub const LAMBDA_REASON_INSUFFICIENT_DATA: &str = "InsufficientData";
/// See [`LAMBDA_REASON_INSUFFICIENT_DATA`].
pub const LAMBDA_REASON_INCONCLUSIVE: &str = "Inconclusive";

/// The `LambdaFunctionMetricName` values
/// (`crate::ops::types::LAMBDA_FUNCTION_METRIC_NAME_VALUES`): the duration a function's
/// code spends processing an event, and the memory it uses doing so.
pub const LAMBDA_METRIC_DURATION: &str = "Duration";
/// See [`LAMBDA_METRIC_DURATION`].
pub const LAMBDA_METRIC_MEMORY: &str = "Memory";

/// The statistic value beside [`METRIC_STATISTIC_MAXIMUM`] that this API's metric-statistic
/// vocabularies declare (`crate::ops::types::METRIC_STATISTIC_VALUES` and
/// `crate::ops::types::LAMBDA_FUNCTION_METRIC_STATISTIC_VALUES` are both `Average` |
/// `Maximum`): the other statistic an analysis records an observation under, and so the other
/// one a reader of projected metrics can ask for.
pub const METRIC_STATISTIC_AVERAGE: &str = "Average";

/// The `LambdaFunctionMemoryMetricStatistic` values
/// (`crate::ops::types::LAMBDA_FUNCTION_MEMORY_METRIC_STATISTIC_VALUES`): the range a
/// recommendation option projects the function's duration into, and the value expected
/// inside it.
pub const PROJECTED_STATISTIC_LOWER_BOUND: &str = "LowerBound";
/// See [`PROJECTED_STATISTIC_LOWER_BOUND`].
pub const PROJECTED_STATISTIC_UPPER_BOUND: &str = "UpperBound";
/// See [`PROJECTED_STATISTIC_LOWER_BOUND`].
pub const PROJECTED_STATISTIC_EXPECTED: &str = "Expected";

/// The memory a Lambda function can be configured with, from the AWS Lambda quotas: a
/// function's memory is "128 MB to 10,240 MB, in 1-MB increments", so no memory size this
/// service recommends falls outside that range or carries a fraction of a MB.
pub const LAMBDA_MIN_MEMORY_MB: i32 = 128;
/// See [`LAMBDA_MIN_MEMORY_MB`].
pub const LAMBDA_MAX_MEMORY_MB: i32 = 10_240;

/// How many invocations the analysed period has to hold for Compute Optimizer to reach a
/// finding for a function, from the "Supported resources and requirements" page the
/// `InsufficientData` reason code links: a Lambda function qualifies for a recommendation
/// once it has been invoked at least 50 times over the look-back period. A function below
/// that is reported `Unavailable` / `InsufficientData` rather than sized from the little
/// that was observed.
pub const LAMBDA_MIN_ANALYSED_INVOCATIONS: i64 = 50;

/// AWS Lambda public pricing in the `us-east-1` Region for the x86_64 architecture, as the
/// Lambda pricing page lists it: $0.0000166667 per GB-second of billed duration plus $0.20
/// per 1 million requests. Every cost and saving a function recommendation reports is
/// computed from these two prices, so its figures agree with each other.
pub const LAMBDA_PRICE_PER_GB_SECOND: f64 = 0.000_016_666_7;
/// See [`LAMBDA_PRICE_PER_GB_SECOND`].
pub const LAMBDA_PRICE_PER_REQUEST: f64 = 0.000_000_2;

/// MB in a GB: a function's memory is configured in MB and Lambda bills GB-seconds.
pub const MB_PER_GB: f64 = 1_024.0;

/// Milliseconds in a second: Lambda bills duration in GB-SECONDS and the `Duration` metric
/// is milliseconds.
pub const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// Days in a month, so a rate observed per day turns into the monthly figures Compute
/// Optimizer reports. Derived from [`HOURS_PER_MONTH`] rather than stated again, so one
/// month means the same length everywhere in this service.
pub fn days_per_month() -> f64 {
    HOURS_PER_MONTH / 24.0
}

/// A Lambda function ARN — `arn:<partition>:lambda:<region>:<account>:function:<name>` with
/// an optional `:<version>` suffix — split into the parts the reader needs, or `None` for
/// anything that is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedFunctionArn<'a> {
    /// The ARN's account-id field: the account that owns the function.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the function is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The function's name.
    pub function_name: &'a str,
    /// The version the ARN names, or [`LATEST_FUNCTION_VERSION`] for an UNQUALIFIED ARN:
    /// "if you specify an unqualified ARN without a function version suffix, Compute
    /// Optimizer will return recommendations for the latest ($LATEST) version of the
    /// function. If you specify a qualified ARN with a version suffix, Compute Optimizer
    /// will return recommendations for the specified function version."
    pub function_version: &'a str,
}

/// Parse a Lambda function ARN, or `None` for anything that is not one. See
/// [`ParsedFunctionArn`].
pub fn parse_function_arn(arn: &str) -> Option<ParsedFunctionArn<'_>> {
    let parsed = parse_arn(arn)?;
    if parsed.service != "lambda" {
        return None;
    }
    let mut resource = parsed.resource.splitn(3, ':');
    if resource.next() != Some("function") {
        return None;
    }
    let function_name = resource.next().filter(|name| !name.is_empty())?;
    let function_version = resource
        .next()
        .filter(|version| !version.is_empty())
        .unwrap_or(LATEST_FUNCTION_VERSION);
    Some(ParsedFunctionArn {
        account_id: parsed.account_id,
        region: parsed.region,
        function_name,
        function_version,
    })
}

/// One interval of the period Compute Optimizer analyses a function over, as CloudWatch
/// aggregates it: how often the function ran in that interval, how long an invocation took
/// (shortest, mean and longest) and how much memory it used (mean and peak).
///
/// A sample whose memory utilization was not observed records `0.0` for both memory
/// measurements — the analysis then has no memory demand to size the function from, which
/// is what [`ObservedFunction::finding`] reports `Inconclusive` for.
#[derive(Debug, Clone, Copy, Default)]
pub struct FunctionSample {
    /// How many days before the recommendation's last refresh this interval was observed,
    /// so the look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// How many times the function was invoked in the interval, which is what
    /// `LambdaFunctionRecommendation$numberOfInvocations` totals.
    pub invocations: i64,
    /// The shortest invocation of the interval, in milliseconds.
    pub duration_ms_minimum: f64,
    /// The mean invocation of the interval, in milliseconds.
    pub duration_ms_average: f64,
    /// The longest invocation of the interval, in milliseconds.
    pub duration_ms_maximum: f64,
    /// The mean memory an invocation of the interval used, in MB.
    pub memory_used_mb_average: f64,
    /// The most memory an invocation of the interval used, in MB.
    pub memory_used_mb_maximum: f64,
}

/// One Lambda function version Compute Optimizer observes: how it is configured, its tags,
/// and the metric samples a recommendation for it is computed from. This is the
/// OBSERVATION; everything a `LambdaFunctionRecommendation` reports is derived from it, so
/// no two readers of the same function can disagree.
#[derive(Debug, Clone)]
pub struct ObservedFunction {
    /// The function's name, which with [`Self::function_version`] is its key in the
    /// inventory. The ARN a reader reports is built from them and the region the request was
    /// signed for ([`Self::function_arn`]), because Compute Optimizer is a REGIONAL service:
    /// the functions it reports are the ones in the Region the caller addressed.
    pub function_name: String,
    /// "The version number of the current function"
    /// (`LambdaFunctionRecommendation$functionVersion`) — a published version number, or
    /// [`LATEST_FUNCTION_VERSION`] for the unpublished one an unqualified ARN names.
    pub function_version: String,
    /// The account that owns the function.
    pub account_id: String,
    /// "The amount of memory, in MB, that's allocated to the current function"
    /// (`LambdaFunctionRecommendation$currentMemorySize`).
    pub configured_memory_mb: i32,
    /// How much of an invocation's duration scales with the memory the function is
    /// configured with, as a fraction of it: "Lambda allocates CPU power in proportion to
    /// the amount of memory configured", so the compute-bound part of an invocation gets
    /// faster on more memory and slower on less, while the part spent waiting on another
    /// service does not move at all. An OBSERVED property of the workload, which is what
    /// lets [`Self::projected_duration_ms`] state the duration a different memory size
    /// would run at.
    pub cpu_bound_duration_share: f64,
    /// The function's tags, as key/value pairs, in the order Lambda reports them.
    pub tags: Vec<(String, String)>,
    /// How many days Compute Optimizer has been observing this function. A look-back
    /// preference longer than this cannot be honoured, so the analysed period is the
    /// shorter of the two.
    pub days_observed: f64,
    /// The metric samples observed, most recent first.
    pub samples: Vec<FunctionSample>,
}

impl ObservedFunction {
    /// This function's identity WITHOUT the region — the resource part of its ARN, which is
    /// also its key in the inventory. A published version is a QUALIFIED ARN
    /// (`function:<name>:<version>`); the unpublished `$LATEST` version is the unqualified
    /// one (`function:<name>`), the form a request may name it by.
    pub fn arn_resource(&self) -> String {
        if self.function_version == LATEST_FUNCTION_VERSION {
            format!("function:{}", self.function_name)
        } else {
            format!("function:{}:{}", self.function_name, self.function_version)
        }
    }

    /// This function's ARN in `region`:
    /// `arn:aws:lambda:<region>:<account>:function:<name>[:<version>]`.
    pub fn function_arn(&self, region: &str) -> String {
        if self.account_id == ::engine::emulator_account_id() {
            ::engine::emulator_arn("lambda", region, &self.arn_resource())
        } else {
            format!(
                "arn:aws:lambda:{region}:{}:{}",
                self.account_id,
                self.arn_resource()
            )
        }
    }

    /// The samples the look-back period in force analyses: those observed within
    /// `look_back_days` of the last refresh.
    fn analysed_samples(&self, look_back_days: f64) -> impl Iterator<Item = &FunctionSample> {
        self.samples
            .iter()
            .filter(move |sample| sample.days_ago <= look_back_days)
    }

    /// "The number of days for which utilization metrics were analyzed for the function":
    /// the preferred look-back period ([`look_back_period_in_days`]), or however long this
    /// function has been observed when that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// "The number of times your function code was applied during the look-back period"
    /// (`LambdaFunctionRecommendation$numberOfInvocations`).
    pub fn invocations(&self, look_back_days: f64) -> i64 {
        self.analysed_samples(look_back_days)
            .map(|sample| sample.invocations.max(0))
            .sum()
    }

    /// The highest of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    fn peak(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        self.analysed_samples(look_back_days)
            .map(of)
            .fold(0.0_f64, f64::max)
    }

    /// The lowest of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    fn trough(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        let lowest = self
            .analysed_samples(look_back_days)
            .map(of)
            .fold(f64::INFINITY, f64::min);
        if lowest.is_finite() {
            lowest.max(0.0)
        } else {
            0.0
        }
    }

    /// The mean of one sample measurement over the analysed period, each sample weighted by
    /// the invocations it covers — the mean OVER INVOCATIONS, which is what a per-invocation
    /// average means. `0.0` for a period that holds no invocation.
    fn mean_per_invocation(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        let mut invocations = 0.0_f64;
        let mut total = 0.0_f64;
        for sample in self.analysed_samples(look_back_days) {
            let weight = sample.invocations.max(0) as f64;
            invocations += weight;
            total += weight * of(sample);
        }
        if invocations > 0.0 {
            total / invocations
        } else {
            0.0
        }
    }

    /// The memory an invocation used at its peak over the analysed period, in MB — the
    /// demand the function's configured memory has to carry.
    pub fn peak_memory_mb(&self, look_back_days: f64) -> f64 {
        self.peak(look_back_days, |sample| sample.memory_used_mb_maximum)
    }

    /// The mean duration of an invocation over the analysed period, in milliseconds.
    pub fn average_duration_ms(&self, look_back_days: f64) -> f64 {
        self.mean_per_invocation(look_back_days, |sample| sample.duration_ms_average)
    }

    /// The metrics `LambdaFunctionRecommendation$utilizationMetrics` reports: each
    /// `LambdaFunctionMetricName` at each `LambdaFunctionMetricStatistic`, in the order the
    /// model's own `ExportableLambdaFunctionField` names them (`DurationAverage`,
    /// `DurationMaximum`, `MemoryAverage`, `MemoryMaximum`). Every value is computed from
    /// the analysed samples, so the metrics a recommendation carries are the observation the
    /// rest of it was derived from.
    pub fn utilization_metrics(&self, look_back_days: f64) -> Vec<LambdaFunctionUtilizationMetric> {
        let metric = |name: &str, statistic: &str, value: f64| LambdaFunctionUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(statistic.to_string()),
            value: round_hundredths(value),
        };
        vec![
            metric(
                LAMBDA_METRIC_DURATION,
                METRIC_STATISTIC_AVERAGE,
                self.average_duration_ms(look_back_days),
            ),
            metric(
                LAMBDA_METRIC_DURATION,
                METRIC_STATISTIC_MAXIMUM,
                self.peak(look_back_days, |sample| sample.duration_ms_maximum),
            ),
            metric(
                LAMBDA_METRIC_MEMORY,
                METRIC_STATISTIC_AVERAGE,
                self.mean_per_invocation(look_back_days, |sample| sample.memory_used_mb_average),
            ),
            metric(
                LAMBDA_METRIC_MEMORY,
                METRIC_STATISTIC_MAXIMUM,
                self.peak_memory_mb(look_back_days),
            ),
        ]
    }

    /// The memory size this service recommends for the function, in MB: enough to carry the
    /// peak an invocation used over the analysed period with [`RECOMMENDATION_HEADROOM`] to
    /// spare, inside the range Lambda allows ([`LAMBDA_MIN_MEMORY_MB`]) and in whole MB.
    pub fn recommended_memory_mb(&self, look_back_days: f64) -> i32 {
        let required = self.peak_memory_mb(look_back_days) * (1.0 + RECOMMENDATION_HEADROOM);
        (required.ceil() as i32).clamp(LAMBDA_MIN_MEMORY_MB, LAMBDA_MAX_MEMORY_MB)
    }

    /// The function's `LambdaFunctionRecommendationFinding` over the analysed period.
    ///
    /// `Unavailable` while the analysis has nothing to size the function from — fewer than
    /// [`LAMBDA_MIN_ANALYSED_INVOCATIONS`] invocations, or no memory utilization observed at
    /// all. Otherwise `NotOptimized` exactly when the memory the workload asks for
    /// ([`Self::recommended_memory_mb`]) is not the memory the function is configured with,
    /// because then "its memory configuration can be sized down while still meeting the
    /// performance requirements" or "doesn't meet the performance requirements of the
    /// workload"; `Optimized` when it is — "the function is correctly provisioned to run
    /// your workload based on its current configuration and its utilization history".
    pub fn finding(&self, look_back_days: f64) -> &'static str {
        if self.invocations(look_back_days) < LAMBDA_MIN_ANALYSED_INVOCATIONS
            || self.peak_memory_mb(look_back_days) <= 0.0
        {
            return LAMBDA_FINDING_UNAVAILABLE;
        }
        if self.recommended_memory_mb(look_back_days) == self.configured_memory_mb {
            FINDING_OPTIMIZED
        } else {
            FINDING_NOT_OPTIMIZED
        }
    }

    /// "The reason for the finding classification of the function": the code of the finding
    /// [`Self::finding`] reached, and NOTHING for an optimized function — "functions that
    /// have a finding classification of `Optimized` don't have a finding reason code".
    pub fn finding_reason_codes(&self, look_back_days: f64) -> Vec<String> {
        if self.invocations(look_back_days) < LAMBDA_MIN_ANALYSED_INVOCATIONS {
            return vec![LAMBDA_REASON_INSUFFICIENT_DATA.to_string()];
        }
        if self.peak_memory_mb(look_back_days) <= 0.0 {
            return vec![LAMBDA_REASON_INCONCLUSIVE.to_string()];
        }
        match self
            .recommended_memory_mb(look_back_days)
            .cmp(&self.configured_memory_mb)
        {
            ::std::cmp::Ordering::Less => vec![METRIC_REASON_MEMORY_OVERPROVISIONED.to_string()],
            ::std::cmp::Ordering::Greater => vec![METRIC_REASON_MEMORY_UNDERPROVISIONED.to_string()],
            ::std::cmp::Ordering::Equal => Vec::new(),
        }
    }

    /// The `CurrentPerformanceRisk` the function's configured memory runs: how close the
    /// peak an invocation used came to the memory it is allowed, on the service's one risk
    /// scale ([`performance_risk_score`]).
    pub fn current_performance_risk(&self, look_back_days: f64) -> &'static str {
        performance_risk_band(performance_risk_score(
            self.peak_memory_mb(look_back_days),
            f64::from(self.configured_memory_mb.max(0)),
        ))
    }

    /// The duration an invocation that took `observed_ms` at the configured memory would
    /// take at `memory_mb`.
    ///
    /// Lambda "allocates CPU power in proportion to the amount of memory configured", so
    /// the compute-bound share of the invocation ([`Self::cpu_bound_duration_share`]) scales
    /// with the ratio of the two memory sizes and the rest of it does not move: a function
    /// that spends its time waiting on another service gets no faster on more memory.
    pub fn projected_duration_ms(&self, observed_ms: f64, memory_mb: i32) -> f64 {
        let share = self.cpu_bound_duration_share.clamp(0.0, 1.0);
        let ratio = demand_ratio(
            f64::from(self.configured_memory_mb.max(0)),
            f64::from(memory_mb.max(0)),
        );
        if !ratio.is_finite() {
            return observed_ms.max(0.0);
        }
        round_hundredths(observed_ms.max(0.0) * (share * ratio + (1.0 - share)))
    }

    /// The projected metrics a recommendation option carries: the duration range the
    /// function would run in at `memory_mb`, projected from the shortest, the mean and the
    /// longest invocation the analysed period observed. `LambdaFunctionMemoryMetricName`
    /// declares one metric — `Duration` — so each statistic is reported for it.
    pub fn projected_utilization_metrics(
        &self,
        look_back_days: f64,
        memory_mb: i32,
    ) -> Vec<LambdaFunctionMemoryProjectedMetric> {
        let metric = |statistic: &str, observed_ms: f64| LambdaFunctionMemoryProjectedMetric {
            name: Some(LAMBDA_METRIC_DURATION.to_string()),
            statistic: Some(statistic.to_string()),
            value: self.projected_duration_ms(observed_ms, memory_mb),
        };
        vec![
            metric(
                PROJECTED_STATISTIC_LOWER_BOUND,
                self.trough(look_back_days, |sample| sample.duration_ms_minimum),
            ),
            metric(
                PROJECTED_STATISTIC_EXPECTED,
                self.average_duration_ms(look_back_days),
            ),
            metric(
                PROJECTED_STATISTIC_UPPER_BOUND,
                self.peak(look_back_days, |sample| sample.duration_ms_maximum),
            ),
        ]
    }

    /// How many times the function runs in a month at the rate the analysed period observed.
    pub fn monthly_invocations(&self, look_back_days: f64) -> f64 {
        if look_back_days <= 0.0 {
            return 0.0;
        }
        self.invocations(look_back_days) as f64 / look_back_days * days_per_month()
    }

    /// What a month of this function costs in USD at `memory_mb` with a mean invocation of
    /// `duration_ms`: the GB-seconds that many invocations of that length at that memory
    /// bill, plus the per-request charge, both at [`LAMBDA_PRICE_PER_GB_SECOND`].
    pub fn monthly_cost(&self, memory_mb: i32, duration_ms: f64, look_back_days: f64) -> f64 {
        let invocations = self.monthly_invocations(look_back_days);
        let gb_seconds = invocations
            * (duration_ms.max(0.0) / MILLISECONDS_PER_SECOND)
            * (f64::from(memory_mb.max(0)) / MB_PER_GB);
        round_hundredths(
            gb_seconds * LAMBDA_PRICE_PER_GB_SECOND + invocations * LAMBDA_PRICE_PER_REQUEST,
        )
    }

    /// What the function costs per month as it is configured today.
    pub fn current_monthly_cost(&self, look_back_days: f64) -> f64 {
        self.monthly_cost(
            self.configured_memory_mb,
            self.average_duration_ms(look_back_days),
            look_back_days,
        )
    }

    /// What the function would cost per month at the recommended memory size: the same
    /// invocations at the projected duration that memory size runs them in, so a saving is
    /// never claimed from a memory cut the duration pays back.
    pub fn recommended_monthly_cost(&self, look_back_days: f64) -> f64 {
        let memory_mb = self.recommended_memory_mb(look_back_days);
        self.monthly_cost(
            memory_mb,
            self.projected_duration_ms(self.average_duration_ms(look_back_days), memory_mb),
            look_back_days,
        )
    }

    /// What adopting the recommended memory size saves per month, and that saving "as a
    /// percentage of monthly cost" (`SavingsOpportunity$savingsOpportunityPercentage`). A
    /// recommendation that costs the same or more saves NOTHING rather than a negative
    /// amount — an under-provisioned function is recommended MORE memory.
    pub fn savings(&self, look_back_days: f64) -> (f64, f64) {
        let current = self.current_monthly_cost(look_back_days);
        let saved = round_hundredths((current - self.recommended_monthly_cost(look_back_days)).max(0.0));
        (percentage_of(saved, current), saved)
    }

    /// The function recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every figure is computed here from the stored configuration and samples. A memory
    /// size recommendation option is reported exactly when the analysis reached a finding
    /// for the function: an `Unavailable` function has no memory size to move to, because
    /// "Compute Optimizer was unable to generate a recommendation" for it.
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> LambdaFunctionRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        let reason_codes = self.finding_reason_codes(look_back_days);
        let memory_mb = self.recommended_memory_mb(look_back_days);
        let (percentage, value) = self.savings(look_back_days);

        LambdaFunctionRecommendation {
            function_arn: Some(self.function_arn(region)),
            function_version: Some(self.function_version.clone()),
            account_id: Some(self.account_id.clone()),
            current_memory_size: self.configured_memory_mb,
            number_of_invocations: self.invocations(look_back_days),
            utilization_metrics: Some(self.utilization_metrics(look_back_days)),
            lookback_period_in_days: look_back_days,
            last_refresh_timestamp,
            finding: Some(finding.to_string()),
            finding_reason_codes: (!reason_codes.is_empty()).then_some(reason_codes),
            memory_size_recommendation_options: (finding != LAMBDA_FINDING_UNAVAILABLE).then(|| {
                vec![LambdaFunctionMemoryRecommendationOption {
                    // One option, so it is the first-ranked one: "rank 1 is considered the
                    // best" wherever this service ranks a recommendation option.
                    rank: 1,
                    memory_size: memory_mb,
                    projected_utilization_metrics: Some(
                        self.projected_utilization_metrics(look_back_days, memory_mb),
                    ),
                    savings_opportunity: Some(SavingsOpportunity {
                        savings_opportunity_percentage: percentage,
                        estimated_monthly_savings: Some(EstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value,
                        }),
                    }),
                    // This emulator prices a function from the public price table in
                    // [`LAMBDA_PRICE_PER_GB_SECOND`] and records no negotiated discount for
                    // Lambda, so the after-discount figures are the before-discount ones —
                    // which is what the reported [`SAVINGS_ESTIMATION_SOURCE`] says.
                    savings_opportunity_after_discounts: Some(
                        LambdaSavingsOpportunityAfterDiscounts {
                            savings_opportunity_percentage: percentage,
                            estimated_monthly_savings: Some(LambdaEstimatedMonthlySavings {
                                currency: Some(CURRENCY_USD.to_string()),
                                value,
                            }),
                        },
                    ),
                }]
            }),
            current_performance_risk: Some(self.current_performance_risk(look_back_days).to_string()),
            // `LambdaEffectiveRecommendationPreferences` carries one preference: where the
            // money figures came from ([`SAVINGS_ESTIMATION_SOURCE`]).
            effective_recommendation_preferences: Some(LambdaEffectiveRecommendationPreferences {
                savings_estimation_mode: Some(LambdaSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
            }),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect()
            }),
        }
    }

    /// This function as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it — or `None` for a function no summary can count.
    ///
    /// The projection is what keeps one function's analysis in ONE place: the summary reader
    /// and `GetLambdaFunctionRecommendations` both answer from this observation, so they
    /// cannot disagree about a function's finding, its performance risk or what adopting its
    /// recommendation saves. A function whose finding is `Unavailable` is NOT a row: the
    /// findings a summary of Lambda functions reports are the number of them "that are
    /// NotOptimized, or Optimized" and `Summary$name` cannot even carry `Unavailable`, so
    /// such a function is counted at no finding — matching the per-function reader, which
    /// withholds it unless a request asks for it by name.
    pub fn to_analysed_resource(&self, look_back_period: &str) -> Option<AnalysedResource> {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        if !findings_reported_for(SOURCE_TYPE_LAMBDA_FUNCTION)
            .iter()
            .any(|(stored, _)| *stored == finding)
        {
            return None;
        }
        Some(AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_LAMBDA_FUNCTION.to_string(),
            arn_service: "lambda".to_string(),
            arn_resource: self.arn_resource(),
            finding: finding.to_string(),
            finding_reason_codes: self.finding_reason_codes(look_back_days),
            current_performance_risk: self.current_performance_risk(look_back_days).to_string(),
            current_monthly_cost: self.current_monthly_cost(look_back_days),
            recommended_monthly_cost: self.recommended_monthly_cost(look_back_days),
            // Compute Optimizer infers workload types for EC2 instances, not for functions,
            // and a function recommendation declares no such member.
            inferred_workload_types: Vec::new(),
            // A function is not a database, so there is no database configuration to record.
            rds_database: None,
        })
    }
}

/// Check a `LambdaFunctionRecommendationFilters` list against the vocabulary the model
/// declares for it, returning the reason it is not valid.
///
/// `name` is a `LambdaFunctionRecommendationFilterName` and the values a filter carries are
/// the value set of the member that name selects on: "specify `Optimized`, `NotOptimized`,
/// or `Unavailable` if you specify the `name` parameter as `Finding`"; "specify
/// `MemoryOverprovisioned`, `MemoryUnderprovisioned`, `InsufficientData`, or `Inconclusive`
/// if you specify the `name` parameter as `FindingReasonCode`". A name or a value outside
/// those sets is refused rather than silently matching nothing — the same rule
/// [`rds_filter_rejection`] applies to the other findings readers' filters.
pub fn lambda_filter_rejection(filters: &[LambdaFunctionRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = lambda_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set a `LambdaFunctionRecommendationFilterName` selects on, or `None` for a name
/// the model does not declare.
fn lambda_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        LAMBDA_FILTER_FINDING => Some(LAMBDA_FUNCTION_RECOMMENDATION_FINDING_VALUES),
        LAMBDA_FILTER_FINDING_REASON_CODE => {
            Some(LAMBDA_FUNCTION_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetLambdaFunctionRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for the reason codes, which are a LIST, when any reported code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`lambda_filter_rejection`] refuses the request first.
pub fn lambda_recommendation_matches_filters(
    recommendation: &LambdaFunctionRecommendation,
    filters: &[LambdaFunctionRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        match filter.name.as_deref() {
            Some(LAMBDA_FILTER_FINDING) => recommendation
                .finding
                .as_deref()
                .is_some_and(|finding| values.iter().any(|value| value == finding)),
            Some(LAMBDA_FILTER_FINDING_REASON_CODE) => {
                recommendation
                    .finding_reason_codes
                    .as_ref()
                    .is_some_and(|codes| {
                        codes
                            .iter()
                            .any(|code| values.iter().any(|value| value == code))
                    })
            }
            _ => false,
        }
    })
}

/// Whether a request's filters ask for the functions the analysis reached no finding for.
///
/// "Functions with a finding of unavailable are not returned unless you specify the filter
/// parameter with a value of `Unavailable` in your `GetLambdaFunctionRecommendations`
/// request" (`LambdaFunctionRecommendation$finding`), so the reader withholds them from
/// every other request rather than reporting a recommendation that does not exist.
pub fn lambda_filters_ask_for_unavailable(filters: &[LambdaFunctionRecommendationFilter]) -> bool {
    filters.iter().any(|filter| {
        filter.name.as_deref() == Some(LAMBDA_FILTER_FINDING)
            && filter
                .values
                .iter()
                .flatten()
                .any(|value| value == LAMBDA_FINDING_UNAVAILABLE)
    })
}

/// The Lambda functions this emulator observes, generated once when the store is created.
///
/// The emulator watches no real workload, so the functions and their metric samples are a
/// fixed observation rather than something derived from traffic. Everything a recommendation
/// reports is nevertheless computed from what is stored here — the analysed invocations,
/// durations and memory peaks come from these samples, the recommended memory size from
/// those peaks, and every cost from [`LAMBDA_PRICE_PER_GB_SECOND`] — so a reader cannot
/// report a figure the observation does not support.
///
/// The set spans the cases a function recommendation distinguishes: an over-provisioned
/// function whose heaviest day falls OUTSIDE the default look-back period, an
/// under-provisioned PUBLISHED version (a qualified ARN), one already sized for its
/// workload, one invoked too rarely to analyse, and one whose memory utilization was never
/// observed.
fn observed_functions() -> Vec<ObservedFunction> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        // 1 GB configured for a workload that peaks at 420 MB on its analysed days: the
        // recommendation sizes the memory down, and half of each invocation is spent
        // waiting on another service, so the smaller memory does not give the saving back
        // in duration. Its heaviest day (780 MB) is 40 days old, so a longer look-back
        // preference changes the memory size this function is recommended.
        ObservedFunction {
            function_name: "invoice-renderer".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 1_024,
            cpu_bound_duration_share: 0.5,
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![
                FunctionSample {
                    days_ago: 0.0,
                    invocations: 42_000,
                    duration_ms_minimum: 180.0,
                    duration_ms_average: 310.0,
                    duration_ms_maximum: 900.0,
                    memory_used_mb_average: 350.0,
                    memory_used_mb_maximum: 400.0,
                },
                FunctionSample {
                    days_ago: 6.0,
                    invocations: 58_000,
                    duration_ms_minimum: 200.0,
                    duration_ms_average: 328.0,
                    duration_ms_maximum: 1_100.0,
                    memory_used_mb_average: 362.0,
                    memory_used_mb_maximum: 420.0,
                },
                FunctionSample {
                    days_ago: 40.0,
                    invocations: 30_000,
                    duration_ms_minimum: 240.0,
                    duration_ms_average: 520.0,
                    duration_ms_maximum: 1_800.0,
                    memory_used_mb_average: 610.0,
                    memory_used_mb_maximum: 780.0,
                },
            ],
        },
        // A PUBLISHED version, so a qualified ARN names it: 128 MB for a workload that
        // peaks at 120 MB, which leaves no headroom — the recommendation sizes the memory
        // UP, and an under-provisioned function saves nothing by taking it.
        ObservedFunction {
            function_name: "thumbnail-resizer".to_string(),
            function_version: "3".to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 128,
            cpu_bound_duration_share: 0.8,
            tags: vec![
                ("Owner".to_string(), "TeamB".to_string()),
                ("Environment".to_string(), "production".to_string()),
            ],
            days_observed: 93.0,
            samples: vec![FunctionSample {
                days_ago: 1.0,
                invocations: 8_400,
                duration_ms_minimum: 420.0,
                duration_ms_average: 850.0,
                duration_ms_maximum: 2_400.0,
                memory_used_mb_average: 104.0,
                memory_used_mb_maximum: 120.0,
            }],
        },
        // 128 MB — Lambda's smallest — for a workload whose peak plus headroom still fits
        // inside it, so there is no other memory size to move to: optimized, and observed
        // for only a week, so the period analysed is that week however long a look-back
        // period the account prefers.
        ObservedFunction {
            function_name: "order-dispatcher".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 128,
            cpu_bound_duration_share: 0.6,
            tags: Vec::new(),
            days_observed: 7.0,
            samples: vec![FunctionSample {
                days_ago: 0.0,
                invocations: 15_000,
                duration_ms_minimum: 40.0,
                duration_ms_average: 95.0,
                duration_ms_maximum: 260.0,
                memory_used_mb_average: 88.0,
                memory_used_mb_maximum: 100.0,
            }],
        },
        // Invoked a handful of times in the analysed period — below what the service needs
        // to reach a finding — so the analysis has insufficient data for it.
        ObservedFunction {
            function_name: "log-forwarder".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 512,
            cpu_bound_duration_share: 0.4,
            tags: Vec::new(),
            days_observed: 93.0,
            samples: vec![FunctionSample {
                days_ago: 2.0,
                invocations: 12,
                duration_ms_minimum: 60.0,
                duration_ms_average: 140.0,
                duration_ms_maximum: 300.0,
                memory_used_mb_average: 180.0,
                memory_used_mb_maximum: 240.0,
            }],
        },
        // Invoked often enough, but no memory utilization was ever observed for it, so the
        // analysis has no demand to size its memory from and cannot conclude anything.
        ObservedFunction {
            function_name: "nightly-reconciler".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id,
            configured_memory_mb: 256,
            cpu_bound_duration_share: 0.3,
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![FunctionSample {
                days_ago: 1.0,
                invocations: 2_000,
                duration_ms_minimum: 500.0,
                duration_ms_average: 1_200.0,
                duration_ms_maximum: 4_000.0,
                memory_used_mb_average: 0.0,
                memory_used_mb_maximum: 0.0,
            }],
        },
    ]
}

// ===========================================================================
// The commercial software license analysis `GetLicenseRecommendations` reports
//
// "Compute Optimizer generates recommendations for licenses that meet a specific set
// of requirements" from the license metrics it OBSERVES through CloudWatch
// Application Insights, so no request to this API creates a license recommendation:
// everything below is about the observation ([`ObservedLicensedInstance`]) and the
// recommendation DERIVED from it, so the per-license reader and
// `GetRecommendationSummaries` cannot disagree about a license's finding or what
// adopting its recommendation saves.
// ===========================================================================

/// The `LicenseFinding` value outside the service-wide `Finding` vocabulary
/// ([`LICENSE_FINDING_VALUES`]): "InsufficentMetrics — When Compute Optimizer detects that
/// your CloudWatch Application Insights isn't enabled or is enabled with insufficient
/// permissions" (`LicenseRecommendation$finding`). Its other two values are
/// [`FINDING_OPTIMIZED`] and [`FINDING_NOT_OPTIMIZED`].
pub const LICENSE_FINDING_INSUFFICIENT_METRICS: &str = "InsufficientMetrics";

/// The `LicenseFindingReasonCode` values ([`LICENSE_FINDING_REASON_CODE_VALUES`]), each as
/// `LicenseRecommendation$findingReasonCodes` documents it: `Optimized` — "All
/// specifications of your license meet the performance requirements of your workload";
/// `LicenseOverprovisioned` — "your license can be downgraded while still meeting the
/// performance requirements of your workload"; `InvalidCloudWatchApplicationInsightsSetup`
/// — "CloudWatch Application Insights isn't configured properly";
/// `CloudWatchApplicationInsightsError` — "There is a CloudWatch Application Insights
/// error".
pub const LICENSE_REASON_OPTIMIZED: &str = "Optimized";
/// See [`LICENSE_REASON_OPTIMIZED`].
pub const LICENSE_REASON_OVERPROVISIONED: &str = "LicenseOverprovisioned";
/// See [`LICENSE_REASON_OPTIMIZED`].
pub const LICENSE_REASON_INVALID_INSIGHTS_SETUP: &str =
    "InvalidCloudWatchApplicationInsightsSetup";
/// See [`LICENSE_REASON_OPTIMIZED`].
pub const LICENSE_REASON_INSIGHTS_ERROR: &str = "CloudWatchApplicationInsightsError";

/// The `LicenseRecommendationFilterName` values
/// ([`LICENSE_RECOMMENDATION_FILTER_NAME_VALUES`]), named so the filter rules below read as
/// the model's own vocabulary.
pub const LICENSE_FILTER_FINDING: &str = "Finding";
/// See [`LICENSE_FILTER_FINDING`].
pub const LICENSE_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";
/// See [`LICENSE_FILTER_FINDING`].
pub const LICENSE_FILTER_LICENSE_NAME: &str = "LicenseName";

/// The `LicenseEdition` values ([`LICENSE_EDITION_VALUES`]).
pub const LICENSE_EDITION_ENTERPRISE: &str = "Enterprise";
/// See [`LICENSE_EDITION_ENTERPRISE`].
pub const LICENSE_EDITION_STANDARD: &str = "Standard";
/// See [`LICENSE_EDITION_ENTERPRISE`].
pub const LICENSE_EDITION_FREE: &str = "Free";
/// See [`LICENSE_EDITION_ENTERPRISE`].
pub const LICENSE_EDITION_NOT_FOUND: &str = "NoLicenseEditionFound";

/// The `LicenseModel` values ([`LICENSE_MODEL_VALUES`]).
pub const LICENSE_MODEL_LICENSE_INCLUDED: &str = "LicenseIncluded";
/// See [`LICENSE_MODEL_LICENSE_INCLUDED`].
pub const LICENSE_MODEL_BRING_YOUR_OWN: &str = "BringYourOwnLicense";

/// The one `LicenseName` the model declares ([`LICENSE_NAME_VALUES`]): the commercial
/// software whose licenses this service analyses.
pub const LICENSE_NAME_SQL_SERVER: &str = "SQLServer";

/// The one `MetricSourceProvider` the model declares ([`METRIC_SOURCE_PROVIDER_VALUES`]) —
/// the source "required to generate recommendations for commercial software licenses"
/// (`LicenseConfiguration$metricsSource`).
pub const METRIC_SOURCE_APPLICATION_INSIGHTS: &str = "CloudWatchApplicationInsights";

/// The service namespace of a CloudWatch Application Insights application, which is what a
/// license's metric source names in its `providerArn`.
pub const APPLICATION_INSIGHTS_ARN_SERVICE: &str = "applicationinsights";

/// How much of a licensed product's feature set an edition licenses, as a rank: a workload
/// that stays inside a lower-ranked edition's features runs on that edition, so a license
/// of a higher-ranked edition than its workload needs "can be downgraded while still
/// meeting the performance requirements of your workload"
/// (`LicenseRecommendation$findingReasonCodes`). `NoLicenseEditionFound` names no edition at
/// all, so it has no rank and nothing is recommended for it.
pub fn license_edition_rank(edition: &str) -> Option<u8> {
    match edition {
        LICENSE_EDITION_FREE => Some(0),
        LICENSE_EDITION_STANDARD => Some(1),
        LICENSE_EDITION_ENTERPRISE => Some(2),
        _ => None,
    }
}

/// What one core of a LICENSE-INCLUDED license of `license_name` at `edition` costs per
/// month, in USD, or `None` for a licensed product or edition this emulator holds no price
/// for — whose recommendation then carries no savings figure rather than an invented one.
///
/// AWS publishes no license price list through this API, so this is the emulator's OWN
/// per-core-month table, stated once: every license cost and saving this service reports is
/// computed from it and the instance's core count
/// ([`ObservedLicensedInstance::license_monthly_cost`]), so the figures of one license
/// recommendation agree with each other and with the summary that aggregates it. The `Free`
/// edition is the one that carries no license charge at all, which is why it costs nothing.
pub fn license_core_monthly_price(license_name: &str, edition: &str) -> Option<f64> {
    let per_core_hour = match (license_name, edition) {
        (LICENSE_NAME_SQL_SERVER, LICENSE_EDITION_ENTERPRISE) => 0.40,
        (LICENSE_NAME_SQL_SERVER, LICENSE_EDITION_STANDARD) => 0.11,
        (LICENSE_NAME_SQL_SERVER, LICENSE_EDITION_FREE) => 0.0,
        _ => return None,
    };
    Some(round_hundredths(per_core_hour * HOURS_PER_MONTH))
}

/// What CloudWatch Application Insights — the one metric source a license recommendation is
/// generated from — reports for one licensed instance.
///
/// The three states are the ones `LicenseRecommendation$finding` and its reason codes
/// distinguish: a source that is reporting (so the analysis can reach a finding about the
/// LICENSE), one that "isn't enabled or is enabled with insufficient permissions" (an
/// invalid setup), and one that is answering with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationInsights {
    /// Enabled, permitted, and reporting the license metrics the analysis reads.
    Reporting,
    /// Not enabled, or enabled with insufficient permissions.
    InvalidSetup,
    /// Enabled, but the metric source is answering with an error.
    Failing,
}

/// What adopting a license recommendation is worth, as
/// [`ObservedLicensedInstance::savings`] computes it: NAMED members rather than a pair, so a
/// caller cannot read the percentage as the amount or the other way round.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LicenseSavings {
    /// The saving "as a percentage of monthly cost"
    /// (`SavingsOpportunity$savingsOpportunityPercentage`).
    pub percentage: f64,
    /// What the move saves per month, in [`CURRENCY_USD`].
    pub monthly_amount: f64,
}

/// One Amazon EC2 instance that runs on a commercial software license, as Compute Optimizer
/// observes it: the license it runs under, the metric source the analysis reads, and the
/// edition of that product's feature set the workload was observed USING. This is the
/// OBSERVATION; everything a `LicenseRecommendation` reports is derived from it, so no two
/// readers of the same license can disagree.
#[derive(Debug, Clone)]
pub struct ObservedLicensedInstance {
    /// The EC2 instance id, which is this instance's key in the inventory. The ARN a reader
    /// reports is built from it and the region the request was signed for
    /// ([`Self::instance_arn`]), because Compute Optimizer is a REGIONAL service: the
    /// licenses it reports are the ones in the Region the caller addressed.
    pub instance_id: String,
    /// The account that owns the instance, and so "The Amazon Web Services account ID of the
    /// license" (`LicenseRecommendation$accountId`).
    pub account_id: String,
    /// "The instance type used in the license" (`LicenseConfiguration$instanceType`).
    pub instance_type: String,
    /// "The current number of cores associated with the instance"
    /// (`LicenseConfiguration$numberOfCores`) — what a per-core license is charged on.
    pub number_of_cores: i32,
    /// "The operating system of the instance" (`LicenseConfiguration$operatingSystem`).
    pub operating_system: String,
    /// The `LicenseName` of the product the instance runs.
    pub license_name: String,
    /// The `LicenseEdition` the instance is licensed for today.
    pub license_edition: String,
    /// The `LicenseModel` the license is held under: a license AWS includes in the
    /// instance's price, or one the customer brings.
    pub license_model: String,
    /// "The version of the license for the application that runs on the instance"
    /// (`LicenseConfiguration$licenseVersion`).
    pub license_version: String,
    /// What the metric source reports for this instance, which is what decides whether the
    /// analysis can reach a finding about the license at all.
    pub application_insights: ApplicationInsights,
    /// The resource group of the CloudWatch Application Insights application that watches
    /// the instance, from which [`Self::metrics_source`] builds the metric source's
    /// `providerArn`. `None` for an instance no application watches, which has no metric
    /// source to report.
    pub application_insights_resource_group: Option<String>,
    /// The LOWEST `LicenseEdition` whose licensed feature set covers everything the
    /// workload was observed using — what the metric source reports about the license.
    /// Compared against [`Self::license_edition`], this is what makes a license
    /// over-provisioned or optimized.
    pub edition_in_use: String,
    /// How many days Compute Optimizer has been observing this instance's license. A
    /// look-back preference longer than this cannot be honoured, so the analysed period is
    /// the shorter of the two.
    pub days_observed: f64,
    /// "A list of tags assigned to an EC2 instance" (`LicenseRecommendation$tags`), as
    /// key/value pairs in the order EC2 reports them.
    pub tags: Vec<(String, String)>,
}

impl ObservedLicensedInstance {
    /// This instance's identity WITHOUT the region — the resource part of its ARN.
    pub fn arn_resource(&self) -> String {
        format!("instance/{}", self.instance_id)
    }

    /// This instance's ARN in `region`: "arn:aws:ec2:region:aws_account_id:instance/
    /// instance-id" (`GetLicenseRecommendations$resourceArns`).
    ///
    /// Built through [`engine::emulator_arn`], the one ARN construction path: this emulator
    /// presents exactly one account ([`engine::emulator_account_id`]) and answers for no
    /// other ([`requested_account_id`]), so every observation it holds is that account's.
    pub fn instance_arn(&self, region: &str) -> String {
        debug_assert_eq!(
            self.account_id,
            ::engine::emulator_account_id(),
            "the license inventory holds the emulator account's instances only"
        );
        ::engine::emulator_arn("ec2", region, &self.arn_resource())
    }

    /// "The number of days for which utilization metrics were analyzed for an instance that
    /// runs on a license" (`LicenseRecommendation$lookbackPeriodInDays`): the preferred
    /// look-back period ([`look_back_period_in_days`]), or however long this instance has
    /// been observed when that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// "The list of metric sources required to generate recommendations for commercial
    /// software licenses" (`LicenseConfiguration$metricsSource`): the CloudWatch Application
    /// Insights application that watches this instance, in the Region the request
    /// addressed, or `None` for an instance no application watches.
    pub fn metrics_source(&self, region: &str) -> Option<Vec<MetricSource>> {
        let resource_group = self.application_insights_resource_group.as_ref()?;
        let provider_arn = ::engine::emulator_arn(
            APPLICATION_INSIGHTS_ARN_SERVICE,
            region,
            &format!("application/resource-group/{resource_group}"),
        );
        Some(vec![MetricSource {
            provider: Some(METRIC_SOURCE_APPLICATION_INSIGHTS.to_string()),
            provider_arn: Some(provider_arn),
        }])
    }

    /// The `LicenseEdition` this analysis would move the license to, or `None` where it
    /// would leave the license as it is.
    ///
    /// The edition the workload was observed needing ([`Self::edition_in_use`]), and only
    /// when that edition licenses LESS than the one in force: a license is moved only
    /// downwards, because what Compute Optimizer identifies here is a license that "can be
    /// downgraded while still meeting the performance requirements of your workload". An
    /// instance whose metric source is not reporting has no observed feature set to compare,
    /// so nothing is recommended for it.
    pub fn recommended_license_edition(&self) -> Option<&str> {
        if self.application_insights != ApplicationInsights::Reporting {
            return None;
        }
        let current = license_edition_rank(&self.license_edition)?;
        let needed = license_edition_rank(&self.edition_in_use)?;
        (needed < current).then_some(self.edition_in_use.as_str())
    }

    /// The license's `LicenseFinding`: `InsufficientMetrics` while the metric source is not
    /// reporting ("your CloudWatch Application Insights isn't enabled or is enabled with
    /// insufficient permissions"), `NotOptimized` for a license the analysis would downgrade
    /// (the EC2 infrastructure "isn't using any of the … license features you're paying
    /// for"), and `Optimized` when "all specifications of your license meet the performance
    /// requirements of your workload".
    pub fn finding(&self) -> &'static str {
        if self.application_insights != ApplicationInsights::Reporting {
            return LICENSE_FINDING_INSUFFICIENT_METRICS;
        }
        if self.recommended_license_edition().is_some() {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// "The reason for the finding classification for an instance that runs on a license"
    /// (`LicenseRecommendation$findingReasonCodes`): the `LicenseFindingReasonCode` behind
    /// [`Self::finding`] — which side of the metric source the finding came from, and, for a
    /// license the analysis did read, whether it can be downgraded.
    pub fn finding_reason_codes(&self) -> Vec<String> {
        let code = match self.application_insights {
            ApplicationInsights::InvalidSetup => LICENSE_REASON_INVALID_INSIGHTS_SETUP,
            ApplicationInsights::Failing => LICENSE_REASON_INSIGHTS_ERROR,
            ApplicationInsights::Reporting => {
                if self.recommended_license_edition().is_some() {
                    LICENSE_REASON_OVERPROVISIONED
                } else {
                    LICENSE_REASON_OPTIMIZED
                }
            }
        };
        vec![code.to_string()]
    }

    /// What this instance's license costs per month at `edition`, in USD, or `None` where
    /// this emulator holds no price for it: a license the customer BRINGS is not charged
    /// through AWS, and [`license_core_monthly_price`] holds no price for every licensed
    /// product or edition. A license with no price carries no savings figure rather than an
    /// invented one.
    pub fn license_monthly_cost(&self, edition: &str) -> Option<f64> {
        if self.license_model != LICENSE_MODEL_LICENSE_INCLUDED {
            return None;
        }
        let per_core = license_core_monthly_price(&self.license_name, edition)?;
        Some(round_hundredths(per_core * f64::from(self.number_of_cores.max(0))))
    }

    /// What adopting the recommended edition is worth: the saving as a percentage of monthly
    /// cost (`SavingsOpportunity$savingsOpportunityPercentage`) and the monthly amount, or
    /// `None` where there is no recommended edition or no price for one of the two editions.
    /// A move that costs no less saves nothing rather than a negative amount.
    pub fn savings(&self) -> Option<LicenseSavings> {
        let recommended = self.recommended_license_edition()?;
        let current_cost = self.license_monthly_cost(&self.license_edition)?;
        let recommended_cost = self.license_monthly_cost(recommended)?;
        let saved = round_hundredths((current_cost - recommended_cost).max(0.0));
        Some(LicenseSavings {
            percentage: percentage_of(saved, current_cost),
            monthly_amount: saved,
        })
    }

    /// "An array of objects that describe the license recommendation options"
    /// (`LicenseRecommendation$licenseRecommendationOptions`): the one option this analysis
    /// has — the edition it would move the license to, at the same operating system and
    /// license model, since neither of those is what the analysis found to change — or
    /// `None` for a license it would leave as it is, which has no option to report.
    pub fn license_recommendation_options(&self) -> Option<Vec<LicenseRecommendationOption>> {
        let recommended = self.recommended_license_edition()?;
        Some(vec![LicenseRecommendationOption {
            // One option, so it is the top one: "The top recommendation option is ranked as
            // 1" (`LicenseRecommendationOption$rank`).
            rank: 1,
            operating_system: Some(self.operating_system.clone()),
            license_edition: Some(recommended.to_string()),
            license_model: Some(self.license_model.clone()),
            savings_opportunity: self.savings().map(|savings| SavingsOpportunity {
                savings_opportunity_percentage: savings.percentage,
                estimated_monthly_savings: Some(EstimatedMonthlySavings {
                    currency: Some(CURRENCY_USD.to_string()),
                    value: savings.monthly_amount,
                }),
            }),
        }])
    }

    /// The instance's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        modelled_tags(&self.tags)
    }

    /// "An object that describes the current configuration of an instance that runs on a
    /// license" (`LicenseRecommendation$currentLicenseConfiguration`), as the endpoint in
    /// `region` reports it.
    pub fn current_license_configuration(&self, region: &str) -> LicenseConfiguration {
        LicenseConfiguration {
            number_of_cores: self.number_of_cores,
            instance_type: Some(self.instance_type.clone()),
            operating_system: Some(self.operating_system.clone()),
            license_edition: Some(self.license_edition.clone()),
            license_name: Some(self.license_name.clone()),
            license_model: Some(self.license_model.clone()),
            license_version: Some(self.license_version.clone()),
            metrics_source: self.metrics_source(region),
        }
    }

    /// The license recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every member is computed here from the stored observation: the configuration is the
    /// license in force, the finding and its reason code say what the metric source reported
    /// about it, the option is the edition the observed feature set asks for, and the saving
    /// is what that move is worth at [`license_core_monthly_price`].
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> LicenseRecommendation {
        LicenseRecommendation {
            resource_arn: Some(self.instance_arn(region)),
            account_id: Some(self.account_id.clone()),
            current_license_configuration: Some(self.current_license_configuration(region)),
            lookback_period_in_days: self.analysed_days(look_back_period),
            last_refresh_timestamp,
            finding: Some(self.finding().to_string()),
            finding_reason_codes: Some(self.finding_reason_codes()),
            license_recommendation_options: self.license_recommendation_options(),
            tags: self.tag_list(),
        }
    }

    /// This licensed instance as a row of the findings catalog, i.e. as
    /// `GetRecommendationSummaries` aggregates it — or `None` for one whose finding the
    /// catalog cannot hold.
    ///
    /// The projection is what keeps one license's analysis in ONE place: the summary reader
    /// and `GetLicenseRecommendations` both answer from this observation, so they cannot
    /// disagree about a license's finding or what adopting its recommendation saves. A
    /// license whose metrics were insufficient carries `InsufficientMetrics`, which is a
    /// `LicenseFinding` value and NOT one of the `Finding` values a catalog row and a
    /// `Summary` can carry ([`findings_reported_for`]), so it is no row at all — the same
    /// rule a Lambda function the analysis reached no finding for follows.
    pub fn to_analysed_resource(&self) -> Option<AnalysedResource> {
        let finding = self.finding();
        if finding == LICENSE_FINDING_INSUFFICIENT_METRICS {
            return None;
        }
        let current_monthly_cost = self
            .license_monthly_cost(&self.license_edition)
            .unwrap_or_default();
        let recommended_monthly_cost = self
            .recommended_license_edition()
            .and_then(|edition| self.license_monthly_cost(edition))
            .unwrap_or(current_monthly_cost);
        Some(AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_LICENSE.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a license is the instance it runs on.
            arn_service: "ec2".to_string(),
            arn_resource: self.arn_resource(),
            finding: finding.to_string(),
            // A license's reason codes are `LicenseFindingReasonCode` values, and
            // `ReasonCodeSummary$name` carries only the two `FindingReasonCode` values
            // ([`METRIC_REASON_MEMORY_OVERPROVISIONED`]), so no license reason code is a
            // count a summary can report.
            finding_reason_codes: Vec::new(),
            // `LicenseRecommendation` declares no performance-risk member: the analysis
            // records nothing about the risk a license runs, so the row carries the lowest
            // rating rather than inventing a higher one.
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            current_monthly_cost,
            recommended_monthly_cost,
            // A license recommendation infers no workload type (`LicenseRecommendation`
            // declares no such member).
            inferred_workload_types: Vec::new(),
            // A license is not a database, so there is no database configuration to record.
            rds_database: None,
        })
    }
}

/// Check a `LicenseRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is a `LicenseRecommendationFilterName` and the values a filter carries are the
/// value set of the member that name selects on: "If you specify the `name` parameter as
/// `Finding`, then specify `Optimized`, `NotOptimized`, or `InsufficentMetrics`"; "If you
/// specify the `name` parameter as `FindingReasonCode`, then specify `Optimized`,
/// `LicenseOverprovisioned`, `InvalidCloudwatchApplicationInsights`, or
/// `CloudwatchApplicationInsightsError`" — the model's own
/// [`LICENSE_FINDING_REASON_CODE_VALUES`] spell those last two
/// `InvalidCloudWatchApplicationInsightsSetup` and `CloudWatchApplicationInsightsError`, and
/// the model is what the wire carries. A `LicenseName` filter selects on
/// [`LICENSE_NAME_VALUES`]. A name or a value outside those sets is refused rather than
/// silently matching nothing — the same rule [`lambda_filter_rejection`] applies.
pub fn license_filter_rejection(filters: &[LicenseRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                LICENSE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = license_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                LICENSE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set a `LicenseRecommendationFilterName` selects on, or `None` for a name the
/// model does not declare.
fn license_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        LICENSE_FILTER_FINDING => Some(LICENSE_FINDING_VALUES),
        LICENSE_FILTER_FINDING_REASON_CODE => Some(LICENSE_FINDING_REASON_CODE_VALUES),
        LICENSE_FILTER_LICENSE_NAME => Some(LICENSE_NAME_VALUES),
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetLicenseRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for the reason codes, which are a LIST, when any reported code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`license_filter_rejection`] refuses the request first.
pub fn license_recommendation_matches_filters(
    recommendation: &LicenseRecommendation,
    filters: &[LicenseRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        match filter.name.as_deref() {
            Some(LICENSE_FILTER_FINDING) => recommendation
                .finding
                .as_deref()
                .is_some_and(|finding| values.iter().any(|value| value == finding)),
            Some(LICENSE_FILTER_FINDING_REASON_CODE) => recommendation
                .finding_reason_codes
                .as_ref()
                .is_some_and(|codes| {
                    codes
                        .iter()
                        .any(|code| values.iter().any(|value| value == code))
                }),
            Some(LICENSE_FILTER_LICENSE_NAME) => recommendation
                .current_license_configuration
                .as_ref()
                .and_then(|configuration| configuration.license_name.as_deref())
                .is_some_and(|license_name| values.iter().any(|value| value == license_name)),
            _ => false,
        }
    })
}

/// The licensed Amazon EC2 instances this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so WHICH instances run a commercial software
/// license, and what their metric source reports, is a fixed observation rather than
/// something derived from traffic. Everything a license recommendation reports is
/// nevertheless computed from what is stored here — the finding and its reason code from the
/// metric source and the observed feature set, the recommended edition from that feature
/// set, and every cost from [`license_core_monthly_price`] — so a reader cannot report a
/// figure the observation does not support.
///
/// The set spans the cases a license recommendation distinguishes: a license-included
/// Enterprise license whose workload stays inside Standard's features (so the move saves
/// money), one already at the edition its workload needs, a license the customer brings
/// (which AWS charges nothing for, so its downgrade carries no savings figure), an instance
/// whose metric source is not set up, and one whose metric source is answering with an
/// error.
fn observed_licensed_instances() -> Vec<ObservedLicensedInstance> {
    let account_id = ::engine::emulator_account_id().to_string();
    let instance = |instance_id: &str,
                    instance_type: &str,
                    number_of_cores: i32,
                    license_edition: &str,
                    license_model: &str,
                    application_insights: ApplicationInsights,
                    application_insights_resource_group: Option<&str>,
                    edition_in_use: &str,
                    days_observed: f64,
                    tags: &[(&str, &str)]| ObservedLicensedInstance {
        instance_id: instance_id.to_string(),
        account_id: account_id.clone(),
        instance_type: instance_type.to_string(),
        number_of_cores,
        // The `OperatingSystem` member is a free-form string in the model; this is the
        // platform EC2 reports for a Windows instance that runs SQL Server.
        operating_system: "Windows".to_string(),
        license_name: LICENSE_NAME_SQL_SERVER.to_string(),
        license_edition: license_edition.to_string(),
        license_model: license_model.to_string(),
        license_version: "2019".to_string(),
        application_insights,
        application_insights_resource_group: application_insights_resource_group
            .map(ToString::to_string),
        edition_in_use: edition_in_use.to_string(),
        days_observed,
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    };

    vec![
        // Enterprise, license-included, on 16 cores — and the workload was never observed
        // using a feature outside Standard's set, so the license can be downgraded and the
        // move saves the difference between the two editions' per-core prices.
        instance(
            "i-0a1b2c3d4e5f70001",
            "r5.4xlarge",
            16,
            LICENSE_EDITION_ENTERPRISE,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::Reporting,
            Some("sql-reporting"),
            LICENSE_EDITION_STANDARD,
            93.0,
            &[("Owner", "TeamA"), ("Environment", "production")],
        ),
        // Standard, license-included, running a workload that needs Standard: every
        // specification of the license meets the workload's requirements, so there is
        // nothing to move to. Observed for a week only, so that week is the period analysed
        // however long a look-back period the account prefers.
        instance(
            "i-0a1b2c3d4e5f70002",
            "m5.2xlarge",
            8,
            LICENSE_EDITION_STANDARD,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::Reporting,
            Some("sql-orders"),
            LICENSE_EDITION_STANDARD,
            7.0,
            &[("Owner", "TeamB")],
        ),
        // Enterprise brought by the customer, on a workload that stays inside the free
        // edition: the license can be downgraded, but AWS charges nothing for a license the
        // customer brings, so the option carries no savings figure.
        instance(
            "i-0a1b2c3d4e5f70003",
            "m5.xlarge",
            4,
            LICENSE_EDITION_ENTERPRISE,
            LICENSE_MODEL_BRING_YOUR_OWN,
            ApplicationInsights::Reporting,
            Some("sql-analytics"),
            LICENSE_EDITION_FREE,
            93.0,
            &[],
        ),
        // No CloudWatch Application Insights application watches this instance, so the
        // analysis has no license metrics at all.
        instance(
            "i-0a1b2c3d4e5f70004",
            "m5.large",
            2,
            LICENSE_EDITION_STANDARD,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::InvalidSetup,
            None,
            LICENSE_EDITION_STANDARD,
            93.0,
            &[("Owner", "TeamB")],
        ),
        // Watched, but the metric source is answering with an error.
        instance(
            "i-0a1b2c3d4e5f70005",
            "c5.2xlarge",
            8,
            LICENSE_EDITION_ENTERPRISE,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::Failing,
            Some("sql-billing"),
            LICENSE_EDITION_ENTERPRISE,
            93.0,
            &[],
        ),
    ]
}

/// The `ECSServiceRecommendationFilterName` values
/// (`crate::ops::types::E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES`), named so the
/// rules over them read as the model spells them: "Specify `Finding` to return
/// recommendations with a specific finding classification"; "Specify `FindingReasonCode` to
/// return recommendations with a specific finding reason code".
pub const ECS_FILTER_FINDING: &str = "Finding";
/// See [`ECS_FILTER_FINDING`].
pub const ECS_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";

/// Check an `ECSServiceRecommendationFilters` list against the vocabulary the model declares
/// for it, returning the reason it is not valid.
///
/// `name` is an `ECSServiceRecommendationFilterName` and the values a filter carries are the
/// value set of the member that name selects on: "If you specify the `name` parameter as
/// `Finding`, specify `Optimized`, `Underprovisioned`, or `Overprovisioned`"; "If you
/// specify the `name` parameter as `FindingReasonCode`, specify `CPUUnderprovisioned`,
/// `CPUOverprovisioned`, `MemoryUnderprovisioned`, or `MemoryOverprovisioned`". A name or a
/// value outside those sets is refused rather than silently matching nothing, and so is a
/// filter that names no member to select on at all: the same rule [`ebs_filter_rejection`]
/// applies to the other recommendation filters of this service. Stated once here, so the
/// Amazon ECS service recommendations READER and the Amazon ECS service recommendations
/// EXPORT admit exactly the same filter set.
pub fn ecs_filter_rejection(filters: &[ECSServiceRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = ecs_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set an `ECSServiceRecommendationFilterName` selects on — `Finding` selects on
/// `ECSServiceRecommendationFinding` and `FindingReasonCode` on
/// `ECSServiceRecommendationFindingReasonCode` — or `None` for a name the model does not
/// declare, which is what makes [`ecs_filter_rejection`] refuse it.
fn ecs_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        ECS_FILTER_FINDING => Some(E_C_S_SERVICE_RECOMMENDATION_FINDING_VALUES),
        ECS_FILTER_FINDING_REASON_CODE => {
            Some(E_C_S_SERVICE_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Recommendation export jobs
//
// Every `Export*Recommendations` operation of this service does the same thing: it QUEUES
// one asynchronous export job against the destination bucket the request names, and answers
// the job's id plus the object keys the export file and its metadata file will be written
// under. `DescribeRecommendationExportJobs` is the reader of those jobs. So the job record
// and every rule over it live here ONCE, whichever export operation created it: the keys a
// job's destination derives, the status lifecycle the clock drives, the
// one-job-in-progress-per-Region limit each export operation documents, the seven-day
// window the reader reports over, and the `JobFilter` vocabulary it selects with.
//
// What the job does NOT hold once it has completed is the exported FILE: the two objects the
// job writes are written into the destination bucket when the clock makes the job complete
// ([`export::write_due_exports`]) and are read back from the Amazon S3 service, so a reader of
// this record never serves their bytes.
// ---------------------------------------------------------------------------

/// The `JobStatus` values (`crate::ops::types::JOB_STATUS_VALUES`), named so the rules
/// below read as the model's own vocabulary.
pub const JOB_STATUS_QUEUED: &str = "Queued";
/// See [`JOB_STATUS_QUEUED`].
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`JOB_STATUS_QUEUED`].
pub const JOB_STATUS_COMPLETE: &str = "Complete";
/// See [`JOB_STATUS_QUEUED`]: the terminal status of a job whose export could not be written
/// to its destination, which is what [`ExportJob::failure_reason`] then says.
pub const JOB_STATUS_FAILED: &str = "Failed";

/// The `JobFilterName` values (`crate::ops::types::JOB_FILTER_NAME_VALUES`): "Specify
/// `ResourceType` to return export jobs of a specific resource type ... Specify `JobStatus`
/// to return export jobs with a specific status".
pub const JOB_FILTER_RESOURCE_TYPE: &str = "ResourceType";
/// See [`JOB_FILTER_RESOURCE_TYPE`].
pub const JOB_FILTER_JOB_STATUS: &str = "JobStatus";

/// The `ResourceType` an idle resource export job carries, which
/// `ExportIdleRecommendations` exports recommendations for
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`).
pub const RESOURCE_TYPE_IDLE: &str = "Idle";

/// The `ResourceType` a commercial software license export job carries, which
/// `ExportLicenseRecommendations` exports recommendations for
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`). The `ResourceType` domain, not the
/// `RecommendationSourceType` one of the same spelling ([`SOURCE_TYPE_LICENSE`]) a findings
/// catalog row carries.
pub const RESOURCE_TYPE_LICENSE: &str = "License";

/// The `ResourceType` a Lambda function export job carries, which
/// `ExportLambdaFunctionRecommendations` exports recommendations for
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`). The `ResourceType` domain, not the
/// `RecommendationSourceType` one of the same spelling ([`SOURCE_TYPE_LAMBDA_FUNCTION`]) a
/// findings catalog row carries.
pub const RESOURCE_TYPE_LAMBDA_FUNCTION: &str = "LambdaFunction";

/// How long each step of an export job's lifecycle takes: a created job stays `Queued` for
/// this long, then runs `InProgress` for this long, then is `Complete`.
///
/// AWS documents no duration for either step — an export takes as long as it takes — so
/// this is the emulator's own span. It is deliberately NON-ZERO, so a caller that describes
/// the job straight after creating it still sees the transient `Queued` status and the
/// one-job-in-progress limit it implies, and short enough that a caller can poll the job
/// through to `Complete`.
pub const EXPORT_JOB_STEP: Duration = Duration::from_millis(250);

/// How long a created export job is reported for: "Describes recommendation export jobs
/// created in the last seven days" (`DescribeRecommendationExportJobs`). An older job is
/// neither reported nor kept.
pub const EXPORT_JOB_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The most export jobs this store holds. Every export call allocates a record, so without
/// this bound repeated calls could grow one emulator process without limit. A call past
/// this count is REFUSED with the `LimitExceededException` the export operations model
/// ([`ExportJobStore::can_hold`]).
pub const MAX_EXPORT_JOBS: usize = 1_000;

/// One recommendation export job, as `ExportIdleRecommendations` creates it and
/// `DescribeRecommendationExportJobs` reports it.
///
/// Its STATUS is not stored: it is derived from the creation instant
/// ([`Self::status_at`]), because the lifecycle is owed by the clock rather than by any
/// call. So a reader reports the status the clock has made due without writing to the
/// store, and two readers of one job can never disagree about where it has got to.
#[derive(Debug, Clone)]
pub struct ExportJob {
    /// The `JobId` this job was minted with ([`ExportJobStore::mint_job_id`]).
    pub job_id: String,
    /// The Region whose endpoint created it. Compute Optimizer is regional, so both the
    /// one-job-in-progress limit and what a reader reports are per Region.
    pub region: String,
    /// The `ResourceType` of the exported recommendations.
    pub resource_type: String,
    /// The destination bucket the request named. Compute Optimizer does not create it.
    pub bucket: String,
    /// The object key of the export file, and of its metadata file
    /// ([`export_destination_keys`]).
    pub key: String,
    /// See [`Self::key`].
    pub metadata_key: String,
    /// When the job was created, in seconds since the epoch ([`now_secs`]).
    pub created_at: f64,
    /// The export file and its metadata file, computed when the job was created and held
    /// until the clock makes the job's completion due, at which point they are written into
    /// the destination bucket and TAKEN from the record ([`export::write_due_exports`]).
    /// `None` on a job whose objects have been written, or whose write failed.
    pub pending: Option<export::PendingExport>,
    /// "The reason for an export job failure": why the destination would not take this job's
    /// objects, or `None` for a job that has not failed.
    pub failure_reason: Option<String>,
}

impl ExportJob {
    /// The job's `JobStatus` at `now`: `Queued` until the first step has elapsed,
    /// `InProgress` until the second has, and then terminal — `Complete`, or `Failed` for a
    /// job whose export the destination would not take. A terminal status never moves again.
    pub fn status_at(&self, now: f64) -> &'static str {
        let step = EXPORT_JOB_STEP.as_secs_f64();
        let elapsed = now - self.created_at;
        if elapsed >= 2.0 * step {
            if self.failure_reason.is_some() {
                JOB_STATUS_FAILED
            } else {
                JOB_STATUS_COMPLETE
            }
        } else if elapsed >= step {
            JOB_STATUS_IN_PROGRESS
        } else {
            JOB_STATUS_QUEUED
        }
    }

    /// When the job last changed, at `now`: the instant the status it currently reports
    /// became due, which for a job still `Queued` is the instant it was created.
    pub fn last_updated_at(&self, now: f64) -> f64 {
        let step = EXPORT_JOB_STEP.as_secs_f64();
        match self.status_at(now) {
            JOB_STATUS_IN_PROGRESS => self.created_at + step,
            JOB_STATUS_QUEUED => self.created_at,
            _ => self.created_at + 2.0 * step,
        }
    }

    /// Whether the job is still running at `now`, i.e. has not reached a terminal status.
    /// This is what "only one export job in progress per Amazon Web Services Region"
    /// counts.
    pub fn is_in_progress(&self, now: f64) -> bool {
        matches!(
            self.status_at(now),
            JOB_STATUS_QUEUED | JOB_STATUS_IN_PROGRESS
        )
    }

    /// Whether the job was created within the last seven days, the window
    /// `DescribeRecommendationExportJobs` reports over.
    pub fn is_reported_at(&self, now: f64) -> bool {
        now - self.created_at < EXPORT_JOB_RETENTION.as_secs_f64()
    }

    /// The `S3Destination` of the export: the bucket the request named and the keys the
    /// export file and its metadata file are written under. Answered by the export
    /// operation that created the job and reported by the job's reader, so the two cannot
    /// name different objects.
    pub fn s3_destination(&self) -> S3Destination {
        S3Destination {
            bucket: Some(self.bucket.clone()),
            key: Some(self.key.clone()),
            metadata_key: Some(self.metadata_key.clone()),
        }
    }

    /// The job as `DescribeRecommendationExportJobs` reports it at `now`.
    ///
    /// `failureReason` is "the reason for an export job failure": a job whose export could
    /// not be written to the destination it named carries why, and every other job carries
    /// none.
    pub fn to_wire(&self, now: f64) -> RecommendationExportJob {
        RecommendationExportJob {
            job_id: Some(self.job_id.clone()),
            destination: Some(ExportDestination {
                s3: Some(self.s3_destination()),
            }),
            resource_type: Some(self.resource_type.clone()),
            status: Some(self.status_at(now).to_string()),
            creation_timestamp: Some(DateTime::from_secs_f64(self.created_at)),
            last_updated_timestamp: Some(DateTime::from_secs_f64(self.last_updated_at(now))),
            failure_reason: self.failure_reason.clone(),
        }
    }
}

/// The export jobs this service holds, in CREATION order, so a page boundary a reader
/// hands out stays where it was whatever is created afterwards.
#[derive(Debug)]
pub struct ExportJobStore {
    /// The jobs, oldest first.
    jobs: Vec<ExportJob>,
    /// How many ids this store has minted, which is what makes the next one unique.
    minted: u64,
    /// A per-store seed, so two stores created in one process mint disjoint id sequences.
    seed: u128,
}

impl Default for ExportJobStore {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            minted: 0,
            seed: ::std::time::SystemTime::now()
                .duration_since(::std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
        }
    }
}

impl ExportJobStore {
    /// Mint the next `JobId`. The model types a job id as an opaque string and the service
    /// answers a UUID, so this hands out a UUID-shaped identifier derived from this store's
    /// seed and the number of ids it has already minted: unique per job, and never
    /// reachable from anything the request carried.
    pub fn mint_job_id(&mut self) -> String {
        self.minted = self.minted.saturating_add(1);
        uuid_shaped(&format!("{}:{}", self.seed, self.minted))
    }

    /// Whether another job may be stored: only while the store is below
    /// [`MAX_EXPORT_JOBS`]. Callers refuse with a modelled error rather than growing past
    /// the bound.
    pub fn can_hold(&self) -> bool {
        self.jobs.len() < MAX_EXPORT_JOBS
    }

    /// Record a created job. Newest last, so [`Self::iter_in_order`] stays creation-ordered.
    pub fn insert(&mut self, job: ExportJob) {
        self.jobs.push(job);
    }

    /// Every stored job, oldest first.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &ExportJob> {
        self.jobs.iter()
    }

    /// How many jobs the store holds, whether or not they are still reported.
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether the store holds no job at all.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The job of `resource_type` still running in `region` at `now`, if any: what "You can
    /// have only one ... export job in progress per Amazon Web Services Region" refuses a
    /// second export against.
    pub fn in_progress_in(
        &self,
        region: &str,
        resource_type: &str,
        now: f64,
    ) -> Option<&ExportJob> {
        self.jobs.iter().find(|job| {
            job.region == region && job.resource_type == resource_type && job.is_in_progress(now)
        })
    }

    /// Drop the jobs created more than seven days ago, which no reader reports any more.
    /// Called from the export path, which is already a mutation; a reader never writes.
    pub fn prune_expired(&mut self, now: f64) {
        self.jobs.retain(|job| job.is_reported_at(now));
    }

    /// Take the objects of every job whose completion the clock has made due at `now` and
    /// that still holds them: what [`export::write_due_exports`] writes to the destinations.
    ///
    /// TAKING them is what makes the write happen once: a job whose objects have been taken
    /// holds none, so a later call hands them to nobody.
    pub fn take_due_exports(&mut self, now: f64) -> Vec<export::DueExport> {
        self.jobs
            .iter_mut()
            .filter(|job| !job.is_in_progress(now))
            .filter_map(|job| {
                job.pending.take().map(|pending| export::DueExport {
                    job_id: job.job_id.clone(),
                    bucket: job.bucket.clone(),
                    key: job.key.clone(),
                    metadata_key: job.metadata_key.clone(),
                    pending,
                })
            })
            .collect()
    }

    /// Record `reason` as why the job named `job_id` failed, which is what its
    /// `failureReason` reports and what makes its terminal status `Failed`.
    pub fn fail(&mut self, job_id: &str, reason: String) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.job_id == job_id) {
            job.failure_reason = Some(reason);
        }
    }
}

/// A UUID-shaped identifier for `seed`: the first 30 hex digits of its SHA-256, laid out
/// 8-4-4-4-12 with the version and variant nibbles RFC 4122 fixes for a random UUID. The
/// digest is what makes two seeds that differ anywhere give unrelated identifiers.
fn uuid_shaped(seed: &str) -> String {
    let digest = ::hex::encode(Sha256::digest(seed.as_bytes()));
    format!(
        "{}-{}-4{}-8{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..15],
        &digest[15..18],
        &digest[18..30]
    )
}

/// The object keys an export job writes under: the export file and its metadata file.
///
/// AWS documents the destination as "the Amazon S3 bucket name and key prefix for a
/// recommendations export job" and the two files as "a comma-separated values (CSV) file,
/// and its metadata in a JavaScript Object Notation (JSON) file", but publishes no key
/// layout, so this is the emulator's own: the caller's prefix, then the account the export
/// is for, then the service, the resource type and the job id — which is what makes the two
/// keys of one job unique and tells two jobs of one bucket apart. Shared by every export
/// operation, so none of them lays its destination out differently.
pub fn export_destination_keys(
    key_prefix: Option<&str>,
    account_id: &str,
    resource_type: &str,
    job_id: &str,
) -> (String, String) {
    let prefix = key_prefix
        .map(|prefix| prefix.trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
        .map_or_else(String::new, |prefix| format!("{prefix}/"));
    let stem = format!("{prefix}{account_id}/ComputeOptimizer/{resource_type}/{job_id}");
    (format!("{stem}.csv"), format!("{stem}-metadata.json"))
}

/// Check a `DescribeRecommendationExportJobs` filter set against the model's own
/// vocabulary, returning the reason it is not valid.
///
/// `name` is a `JobFilterName` and the values a filter carries are the value set of the
/// member that name selects on: `ResourceType` selects on `ResourceType` and `JobStatus` on
/// `JobStatus`. A name or a value outside those sets is refused rather than silently
/// matching nothing. (The member's prose still names only `Ec2Instance` and
/// `AutoScalingGroup` as resource types, which the `ResourceType` enum has since outgrown;
/// the enum is the vocabulary.)
pub fn job_filter_rejection(filters: &[JobFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                JOB_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let vocabulary: &[&str] = match name {
            JOB_FILTER_RESOURCE_TYPE => RESOURCE_TYPE_VALUES,
            JOB_FILTER_JOB_STATUS => JOB_STATUS_VALUES,
            _ => {
                return Some(format!(
                    "The value supplied for parameter filters is not valid: {name} is not a \
                     filter name, which must be one of [{}].",
                    JOB_FILTER_NAME_VALUES.join(", ")
                ))
            }
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value of \
                 the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Does `job` satisfy every `DescribeRecommendationExportJobs` filter at `now`?
///
/// A filter matches when the member its name selects on equals any of the filter's values;
/// several filters are ANDed, and a filter carrying no values constrains nothing. An
/// unrecognised name never reaches here — [`job_filter_rejection`] refuses the request
/// first.
pub fn export_job_matches_filters(job: &ExportJob, filters: &[JobFilter], now: f64) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let member = match filter.name.as_deref() {
            Some(JOB_FILTER_RESOURCE_TYPE) => Some(job.resource_type.as_str()),
            Some(JOB_FILTER_JOB_STATUS) => Some(job.status_at(now)),
            _ => None,
        };
        member.is_some_and(|member| values.iter().any(|value| value == member))
    })
}

// ---------------------------------------------------------------------------
// The Amazon EC2 instances Compute Optimizer observes
//
// Compute Optimizer analyses the workload it OBSERVES on an instance, so no operation of
// this API creates an instance or an analysis of one: the observation below is part of the
// store's INITIAL VALUE (`observed_instances`), exactly as the volume, group, function and
// license observations are. Everything a reader reports about an instance — its finding,
// what it costs, the instance types the analysis would move it to, and the utilization
// projected onto each of them — is DERIVED from it, so no two readers of one instance can
// disagree, and the `Ec2Instance` rows of the findings catalog are projected from it
// (`ObservedInstance::to_analysed_resource`) rather than stated a second time.
// ---------------------------------------------------------------------------

/// One Amazon EC2 instance TYPE, as the configuration an instance runs today or as one the
/// analysis found as a candidate for it. Both sides are the same shape because every figure
/// derived from them — the cost, the saving, the utilization projected onto a candidate — is
/// a comparison BETWEEN two of them, the same reason [`GroupConfiguration`] serves both
/// sides of an Auto Scaling group's analysis.
#[derive(Debug, Clone)]
pub struct InstanceConfiguration {
    /// The instance type, `<family>.<size>` — what
    /// `RecommendedOptionProjectedMetric$recommendedInstanceType` names.
    pub instance_type: String,
    /// The vCPUs this instance type provides.
    pub vcpus: f64,
    /// The memory, in GiB, this instance type provides.
    pub memory_gib: f64,
    /// The public on-demand price of one instance-hour, in [`CURRENCY_USD`].
    pub hourly_price: f64,
    /// This type's `CpuVendorArchitecture`: [`CPU_ARCHITECTURE_CURRENT`] or
    /// [`CPU_ARCHITECTURE_AWS_ARM64`].
    pub architecture: String,
}

/// Whether a `MetricName` measures, as a percentage, a resource the instance TYPE sizes — the
/// instance's vCPUs (`Cpu`) and its memory (`Memory`).
///
/// "`Cpu` — the projected percentage of allocated EC2 compute units … identifies the
/// processing power required to run an application" and "`Memory` — the percentage of memory
/// that would be in use … identifies the amount of memory required" (`ProjectedMetric$name`):
/// both are percentages of what the instance type provides, so moving to another type changes
/// what they are OF. Every other `MetricName` of that value set measures the workload's own
/// traffic — its network packets and bytes, its disk and EBS operations — which changing the
/// instance type does not resize.
///
/// They are also exactly the metrics a PROJECTION reports: "The `Cpu` and `Memory` metrics are
/// the only projected utilization metrics returned when you run the
/// `GetEC2RecommendationProjectedMetrics` action" (`RecommendedOptionProjectedMetric`).
pub fn metric_sized_by_instance_type(metric_name: &str) -> bool {
    metric_name == METRIC_CPU || metric_name == METRIC_MEMORY
}

impl InstanceConfiguration {
    /// The estimated monthly on-demand cost of running one instance of this type: the price
    /// of one instance-hour × [`HOURS_PER_MONTH`].
    pub fn monthly_cost(&self) -> f64 {
        round_hundredths(self.hourly_price * HOURS_PER_MONTH)
    }

    /// The capacity a metric named `metric_name` is measured as a percentage OF on this
    /// instance type, or `None` for a metric the instance type does not size
    /// ([`metric_sized_by_instance_type`]).
    pub fn metric_capacity(&self, metric_name: &str) -> Option<f64> {
        match metric_name {
            METRIC_CPU => Some(self.vcpus),
            METRIC_MEMORY => Some(self.memory_gib),
            _ => None,
        }
    }
}

/// One utilization figure the analysis observed on an instance: the `MetricName` measured,
/// the `MetricStatistic` it was observed under, and its value.
#[derive(Debug, Clone)]
pub struct ObservedInstanceMetric {
    /// The `MetricName` measured (`crate::ops::types::METRIC_NAME_VALUES`).
    pub name: String,
    /// The `MetricStatistic` the figure was observed under ([`METRIC_STATISTIC_MAXIMUM`] or
    /// [`METRIC_STATISTIC_AVERAGE`]), which is what a request's `stat` selects on.
    pub statistic: String,
    /// The figure itself: a percentage for a metric the instance type sizes
    /// ([`InstanceConfiguration::metric_capacity`]), and the metric's own unit otherwise.
    pub value: f64,
}

/// One Amazon EC2 instance Compute Optimizer has analysed: the instance type it runs, the
/// utilization observed on it, and the instance types the analysis found as candidates for
/// it. Every value a reader REPORTS about the instance is one of these stored observations
/// or a figure computed from them, so two readers of the same instance cannot disagree.
#[derive(Debug, Clone)]
pub struct ObservedInstance {
    /// The EC2 instance id, which is this inventory's key. The ARN a reader reports is built
    /// from it and the region the request was signed for ([`Self::instance_arn`]), because
    /// Compute Optimizer is a REGIONAL service.
    pub instance_id: String,
    /// The account that owns the instance.
    pub account_id: String,
    /// The instance type the instance runs today.
    pub current: InstanceConfiguration,
    /// The candidate instance types the analysis found, best first — so a reported `rank` is
    /// a candidate's position in this list ("the top recommendation option is ranked as 1").
    /// Empty for an instance the analysis would leave as it is.
    pub candidates: Vec<InstanceConfiguration>,
    /// The utilization the analysis observed over its look-back period, one figure per
    /// metric and statistic.
    pub utilization: Vec<ObservedInstanceMetric>,
    /// The `FindingReasonCode`s behind the instance's finding; empty where it has none.
    pub finding_reason_codes: Vec<String>,
    /// `currentPerformanceRisk`: the `CurrentPerformanceRisk` the instance's current
    /// configuration runs.
    pub current_performance_risk: String,
    /// The `InferredWorkloadType`s the analysis inferred for the instance; empty where it
    /// inferred none.
    pub inferred_workload_types: Vec<String>,
    /// `instanceState`: "the state of the instance when the recommendation was generated",
    /// one of the model's `InstanceState` values
    /// (`crate::ops::types::INSTANCE_STATE_VALUES`).
    pub instance_state: String,
    /// The instance's tags, as key/value pairs, in the order Amazon EC2 reports them. The
    /// `Name` tag is what names the instance ([`Self::instance_name`]).
    pub tags: Vec<(String, String)>,
}

/// The `InstanceState` an instance under analysis is in
/// (`crate::ops::types::INSTANCE_STATE_VALUES`): Compute Optimizer analyses the workload an
/// instance is RUNNING, so that is the state the observations below were taken in.
pub const INSTANCE_STATE_RUNNING: &str = "running";

/// The tag key Amazon EC2 names a resource with, and so the tag an instance's `instanceName`
/// is taken from: "the name of the current instance" (`InstanceRecommendation$instanceName`)
/// is the value of its `Name` tag.
pub const NAME_TAG_KEY: &str = "Name";

/// The `PlatformDifference` value a recommendation option of another CPU architecture carries
/// (`crate::ops::types::PLATFORM_DIFFERENCE_VALUES`).
pub const PLATFORM_DIFFERENCE_ARCHITECTURE: &str = "Architecture";

/// The `ExternalMetricStatusCode` for an account that prefers no external metrics provider
/// (`crate::ops::types::EXTERNAL_METRIC_STATUS_CODE_VALUES`).
pub const EXTERNAL_METRIC_STATUS_NO_METRIC_SET: &str = "NO_EXTERNAL_METRIC_SET";

/// The `ExternalMetricStatus` a recommendation reports: "an object that describes Compute
/// Optimizer's integration status with your external metrics provider".
///
/// WHICH provider the analysis should take metrics from is the account's external-metrics
/// preference (`ExternalMetricsPreference$source`, one of
/// `crate::ops::types::EXTERNAL_METRICS_SOURCE_VALUES`), so an account that prefers none has
/// no integration at all: [`EXTERNAL_METRIC_STATUS_NO_METRIC_SET`]. This emulator derives a
/// recommendation from the utilization it observes itself and ingests no series from a
/// third-party provider, so once a provider IS preferred the status is that provider's
/// "insufficient metrics" code — the condition the service reports while a preferred provider
/// has published nothing the analysis can use. A source outside the model's own value set
/// names no provider to integrate with, so it reports no integration either.
pub fn external_metric_status(source: Option<&str>) -> ExternalMetricStatus {
    /// Each `ExternalMetricsSource` the model declares
    /// (`crate::ops::types::EXTERNAL_METRICS_SOURCE_VALUES`) paired with the
    /// `ExternalMetricStatusCode` that provider's absent metrics are reported under
    /// (`crate::ops::types::EXTERNAL_METRIC_STATUS_CODE_VALUES`) — one row per provider the
    /// model names, so neither value set can drift out of step with the other unnoticed.
    const INSUFFICIENT_METRICS: [(&str, &str); 4] = [
        ("Datadog", "INSUFFICIENT_DATADOG_METRICS"),
        ("Dynatrace", "INSUFFICIENT_DYNATRACE_METRICS"),
        ("Instana", "INSUFFICIENT_INSTANA_METRICS"),
        ("NewRelic", "INSUFFICIENT_NEWRELIC_METRICS"),
    ];
    debug_assert!(
        INSUFFICIENT_METRICS
            .iter()
            .all(|(provider, code)| EXTERNAL_METRICS_SOURCE_VALUES.contains(provider)
                && EXTERNAL_METRIC_STATUS_CODE_VALUES.contains(code)),
        "the catalog may only pair modelled ExternalMetricsSource and \
         ExternalMetricStatusCode values"
    );
    let no_metric_set = || ExternalMetricStatus {
        status_code: Some(EXTERNAL_METRIC_STATUS_NO_METRIC_SET.to_string()),
        status_reason: None,
    };
    let Some(source) = source.filter(|source| !source.is_empty()) else {
        return no_metric_set();
    };
    let code = INSUFFICIENT_METRICS
        .iter()
        .find(|(provider, _)| *provider == source)
        .map(|(_, code)| *code);
    let Some(code) = code else {
        return no_metric_set();
    };
    ExternalMetricStatus {
        status_code: Some(code.to_string()),
        status_reason: Some(format!(
            "Compute Optimizer has received no utilization metrics from {source}."
        )),
    }
}

/// The `MigrationEffort` of moving a workload onto a candidate of `candidate_architecture`,
/// given whether the analysis inferred a workload type for it.
///
/// `VeryLow` "if both the current and recommended instance types are of the same CPU
/// architecture", `Low` "if Amazon EMR is the inferred workload type and an Amazon Web
/// Services Graviton instance type is recommended", and `Medium` "if a workload type couldn't
/// be inferred but an Amazon Web Services Graviton instance type is recommended"
/// (`InstanceRecommendationOption$migrationEffort`).
///
/// The model names Amazon EMR as the example of an inferred workload and states no rule for
/// any other, so this generalises its `Low` case to ANY workload type the analysis inferred:
/// what the two documented Graviton cases differ on is whether a workload type was inferred at
/// all, and naming one of the nine `InferredWorkloadType` values would make the effort of a
/// Kafka or a Redis workload depend on a value the rule says nothing about.
///
/// The ONE rule in this service, shared by every analysis that ranks a candidate instance
/// type, so two recommendations of the same move cannot report different effort.
pub fn migration_effort(candidate_architecture: &str, inferred_workload_type: bool) -> &'static str
{
    if candidate_architecture != CPU_ARCHITECTURE_AWS_ARM64 {
        "VeryLow"
    } else if inferred_workload_type {
        "Low"
    } else {
        "Medium"
    }
}

/// An Amazon EC2 instance ARN — `arn:<partition>:ec2:<region>:<account>:instance/<id>` — split
/// into the parts the readers need, or `None` for anything that is not one.
/// `GetEC2InstanceRecommendations$instanceArns` takes "the Amazon Resource Name (ARN) of the
/// instances for which to return recommendations", so anything else is not an instance ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedInstanceArn<'a> {
    /// The ARN's account-id field: the account that owns the instance.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the instance is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The EC2 instance id, which is the inventory's key.
    pub instance_id: &'a str,
}

/// Parse an Amazon EC2 instance ARN, or `None` for anything that is not one. Built on the one
/// place this service decides what an ARN looks like ([`parse_arn`]). See
/// [`ParsedInstanceArn`].
pub fn parse_instance_arn(arn: &str) -> Option<ParsedInstanceArn<'_>> {
    let parsed = parse_arn(arn)?;
    if parsed.service != "ec2" {
        return None;
    }
    let (resource_kind, instance_id) = parsed.resource.split_once('/')?;
    if resource_kind != "instance" || instance_id.is_empty() {
        return None;
    }
    Some(ParsedInstanceArn {
        account_id: parsed.account_id,
        region: parsed.region,
        instance_id,
    })
}

impl ObservedInstance {
    /// "The name of the current instance" — the value of its `Name` tag, or `None` for an
    /// instance that carries none, which Amazon EC2 reports no name for either.
    pub fn instance_name(&self) -> Option<String> {
        self.tags
            .iter()
            .find(|(key, _)| key == NAME_TAG_KEY)
            .map(|(_, value)| value.clone())
    }

    /// The instance's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        modelled_tags(&self.tags)
    }

    /// The metrics `InstanceRecommendation$utilizationMetrics` reports: every observation the
    /// analysis recorded under [`METRIC_STATISTIC_MAXIMUM`], which is the one statistic this
    /// API reports ("The Compute Optimizer API, Command Line Interface (CLI), and SDKs return
    /// utilization metrics using only the `Maximum` statistic", `UtilizationMetric$statistic`).
    pub fn utilization_metrics(&self) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .filter(|metric| metric.statistic == METRIC_STATISTIC_MAXIMUM)
            .map(|metric| UtilizationMetric {
                name: Some(metric.name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: round_hundredths(metric.value),
            })
            .collect()
    }

    /// The metrics `InstanceRecommendationOption$projectedUtilizationMetrics` reports for
    /// `option`: "The `Cpu` and `Memory` metrics are the only projected utilization metrics
    /// returned", so the observations projected are the ones the instance TYPE sizes
    /// ([`metric_sized_by_instance_type`]), each through the one projection this service
    /// projects an instance's utilization with ([`Self::projected_metric_value`]) — so this
    /// option and the series `GetEC2RecommendationProjectedMetrics` projects for it cannot
    /// disagree.
    pub fn projected_utilization_metrics(
        &self,
        option: &InstanceConfiguration,
    ) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .filter(|metric| metric.statistic == METRIC_STATISTIC_MAXIMUM)
            .filter(|metric| metric_sized_by_instance_type(&metric.name))
            .map(|metric| UtilizationMetric {
                name: Some(metric.name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: self.projected_metric_value(&metric.name, metric.value, option),
            })
            .collect()
    }

    /// The `PerformanceRisk` an option carries: how close the utilization projected onto
    /// `option` comes to the capacity of the type it recommends, on the model's 0..4 scale
    /// ([`performance_risk_score`]) — "the performance risk of the recommended instance is
    /// calculated as the maximum performance risk score across the analyzed resource
    /// specifications", so an option is as risky as its most loaded dimension.
    pub fn option_performance_risk(&self, option: &InstanceConfiguration) -> f64 {
        let peak = self
            .projected_utilization_metrics(option)
            .iter()
            .map(|metric| metric.value)
            .fold(0.0_f64, f64::max);
        round_hundredths(performance_risk_score(peak, UTILIZATION_PERCENTAGE_CEILING))
    }

    /// What adopting `option` saves per month, and that saving "as a percentage of monthly
    /// cost" (`SavingsOpportunity$savingsOpportunityPercentage`). An option that costs the
    /// same or more saves NOTHING rather than a negative amount — an under-provisioned
    /// instance is recommended a LARGER type.
    pub fn savings(&self, option: &InstanceConfiguration) -> (f64, f64) {
        let current = self.current.monthly_cost();
        let saved = round_hundredths((current - option.monthly_cost()).max(0.0));
        (saved, percentage_of(saved, current))
    }

    /// The `PlatformDifference`s between the type the instance runs and `option`
    /// (`crate::ops::types::PLATFORM_DIFFERENCE_VALUES`): "the configuration differences
    /// between the current instance and the recommended instance type".
    ///
    /// The analysis records an instance type's CPU vendor and architecture, so the one
    /// difference it can state is `Architecture` — "the CPU architecture of the recommended
    /// instance type is different than that of the current instance" — which a candidate of
    /// another `CpuVendorArchitecture` has. The remaining values of that enum describe a
    /// hypervisor, a network or storage interface, instance-store availability or a
    /// virtualization type, none of which this analysis observes.
    pub fn platform_differences(&self, option: &InstanceConfiguration) -> Vec<String> {
        if option.architecture == self.current.architecture {
            Vec::new()
        } else {
            vec![PLATFORM_DIFFERENCE_ARCHITECTURE.to_string()]
        }
    }

    /// "The level of effort required to migrate from the current instance type to the
    /// recommended instance type" — the crate's one rule for it ([`migration_effort`]), so an
    /// instance's effort and an Auto Scaling group's mean the same thing.
    pub fn migration_effort(
        &self,
        option: &InstanceConfiguration,
        infers_workload_types: bool,
    ) -> &'static str {
        migration_effort(
            &option.architecture,
            infers_workload_types && !self.inferred_workload_types.is_empty(),
        )
    }

    /// This instance's identity WITHOUT the region — the resource part of its ARN.
    pub fn arn_resource(&self) -> String {
        format!("instance/{}", self.instance_id)
    }

    /// This instance's ARN as the endpoint in `region` reports it:
    /// `arn:aws:ec2:<region>:<account>:instance/<id>`, the form
    /// `GetEC2RecommendationProjectedMetrics$instanceArn` carries.
    pub fn instance_arn(&self, region: &str) -> String {
        resource_arn_in("ec2", region, &self.account_id, &self.arn_resource())
    }

    /// The top-ranked candidate the analysis found, or `None` for an instance it would leave
    /// as it is — which is also the configuration a cost or a saving is computed against.
    pub fn recommended_configuration(&self) -> Option<&InstanceConfiguration> {
        self.candidates.first()
    }

    /// The instance's `Finding`.
    ///
    /// An instance is `Overprovisioned` "if at least one specification of your instance, such
    /// as CPU, memory, or network, can be sized down while still meeting the performance
    /// requirements of your workload", `Underprovisioned` "if at least one specification …
    /// does not meet the performance requirements of your workload", and `Optimized` "when
    /// all specifications … meet the performance requirements" — so the finding follows the
    /// top-ranked candidate: one that GROWS a specification is one the workload needed more
    /// of, one that only SHRINKS specifications is one the instance has more of than the
    /// workload needs, and an instance with no candidate at all, or one at the same capacity
    /// (a newer generation of the same size), is optimized.
    pub fn finding(&self) -> &'static str {
        let Some(option) = self.recommended_configuration() else {
            return FINDING_OPTIMIZED;
        };
        let grows = option.vcpus > self.current.vcpus || option.memory_gib > self.current.memory_gib;
        let shrinks =
            option.vcpus < self.current.vcpus || option.memory_gib < self.current.memory_gib;
        match (grows, shrinks) {
            (true, _) => FINDING_UNDERPROVISIONED,
            (false, true) => FINDING_OVERPROVISIONED,
            (false, false) => FINDING_OPTIMIZED,
        }
    }

    /// The value a metric is projected to take once the instance runs on `option`, given the
    /// `observed` value on the type it runs today.
    ///
    /// "This represents the projected utilization of a recommendation option had you used
    /// that resource during the analyzed period" (`ProjectedMetric`): a metric measured as a
    /// percentage of a resource the instance type sizes
    /// ([`InstanceConfiguration::metric_capacity`]) is projected by the one percentage
    /// projection [`project_percentage`] states. Every other metric measures the workload
    /// itself, which changing the instance type does not resize, so it is projected
    /// unchanged.
    ///
    /// The ONE projection rule for an instance, so the series this projects over a window and
    /// the projected utilization an instance recommendation option reports cannot disagree.
    pub fn projected_metric_value(
        &self,
        metric_name: &str,
        observed: f64,
        option: &InstanceConfiguration,
    ) -> f64 {
        let capacities = self
            .current
            .metric_capacity(metric_name)
            .zip(option.metric_capacity(metric_name));
        let Some((from, to)) = capacities else {
            return observed;
        };
        project_percentage(observed, from, to)
    }

    /// This instance as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it: the finding the analysis reached, what the instance costs today and
    /// what the top-ranked candidate would cost — "the top recommendation option is ranked as
    /// 1" — or its own cost where the analysis found no candidate.
    ///
    /// The projection is what keeps one instance's analysis in ONE place: the summary reader
    /// and every reader of an instance recommendation answer from this observation, so they
    /// cannot disagree about an instance's finding, its performance risk or what adopting its
    /// recommendation saves.
    pub fn to_analysed_resource(&self) -> AnalysedResource {
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_EC2_INSTANCE.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for an instance is its EC2 instance id.
            arn_service: "ec2".to_string(),
            arn_resource: self.arn_resource(),
            finding: self.finding().to_string(),
            finding_reason_codes: self.finding_reason_codes.clone(),
            current_performance_risk: self.current_performance_risk.clone(),
            current_monthly_cost: self.current.monthly_cost(),
            recommended_monthly_cost: self
                .recommended_configuration()
                .unwrap_or(&self.current)
                .monthly_cost(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            // An instance is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}

/// The Amazon EC2 instance inventory this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so WHICH instances it holds and the utilization
/// observed on them are a fixed observation rather than something derived from traffic.
/// Everything a reader reports is nevertheless computed from what is stored here — each
/// cost from the stored on-demand price, each finding from the candidate the analysis found,
/// and every projected metric from the observed one — so a reader cannot report a figure the
/// observation does not support.
///
/// The set spans the cases an instance recommendation distinguishes: an instance the
/// analysis would size DOWN (so its recommendation saves, and one of its candidates is a
/// Graviton type, which a request may ask for on its own), one it would size UP (whose
/// recommendation therefore saves nothing) and one it would leave as it is (which has no
/// candidate to project onto). They differ in whether Amazon EC2 names them — an instance is
/// named by its `Name` tag, and one carries none.
///
/// The prices and capacities are the public on-demand Linux prices of these instance types
/// in the `us-east-1` Region, as the Amazon EC2 On-Demand pricing page lists them: `m5.large`
/// $0.096 per hour with 2 vCPUs and 8 GiB, `m5.xlarge` $0.192 with 4 vCPUs and 16 GiB,
/// `m5.2xlarge` $0.384 with 8 vCPUs and 32 GiB, and the Graviton `m6g.xlarge` $0.154 with
/// 4 vCPUs and 16 GiB.
fn observed_instances() -> Vec<ObservedInstance> {
    let account_id = ::engine::emulator_account_id().to_string();
    let configuration = |instance_type: &str,
                         vcpus: f64,
                         memory_gib: f64,
                         hourly_price: f64,
                         architecture: &str| InstanceConfiguration {
        instance_type: instance_type.to_string(),
        vcpus,
        memory_gib,
        hourly_price,
        architecture: architecture.to_string(),
    };
    let m5_large = || configuration("m5.large", 2.0, 8.0, 0.096, CPU_ARCHITECTURE_CURRENT);
    let m5_xlarge = || configuration("m5.xlarge", 4.0, 16.0, 0.192, CPU_ARCHITECTURE_CURRENT);
    let m5_2xlarge = || configuration("m5.2xlarge", 8.0, 32.0, 0.384, CPU_ARCHITECTURE_CURRENT);
    let m6g_xlarge = || configuration("m6g.xlarge", 4.0, 16.0, 0.154, CPU_ARCHITECTURE_AWS_ARM64);
    let metric = |name: &str, statistic: &str, value: f64| ObservedInstanceMetric {
        name: name.to_string(),
        statistic: statistic.to_string(),
        value,
    };

    vec![
        // Half the workload's memory and processing power is idle: the analysis would move
        // the instance to half the capacity, which costs half as much. It found a Graviton
        // candidate of the same capacity too, which a request asking for `AWS_ARM64` gets on
        // its own.
        ObservedInstance {
            instance_id: "i-0a1b2c3d4e5f60001".to_string(),
            account_id: account_id.clone(),
            current: m5_2xlarge(),
            candidates: vec![m5_xlarge(), m6g_xlarge()],
            utilization: vec![
                metric(METRIC_CPU, METRIC_STATISTIC_MAXIMUM, 22.5),
                metric(METRIC_MEMORY, METRIC_STATISTIC_MAXIMUM, 31.0),
                metric(METRIC_CPU, METRIC_STATISTIC_AVERAGE, 12.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_AVERAGE, 20.0),
                // A metric the instance type does not size: the workload's own network
                // traffic, in bytes per second.
                metric(
                    "NETWORK_IN_BYTES_PER_SECOND",
                    METRIC_STATISTIC_MAXIMUM,
                    1_250_000.0,
                ),
            ],
            finding_reason_codes: vec![METRIC_REASON_MEMORY_OVERPROVISIONED.to_string()],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["PostgreSql".to_string()],
            instance_state: INSTANCE_STATE_RUNNING.to_string(),
            tags: vec![
                (NAME_TAG_KEY.to_string(), "reporting-db".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
            ],
        },
        // The workload outgrew the instance: the analysis would move it to twice the
        // capacity, which costs MORE — a recommendation is not always a saving.
        ObservedInstance {
            instance_id: "i-0a1b2c3d4e5f60002".to_string(),
            account_id: account_id.clone(),
            current: m5_large(),
            candidates: vec![m5_xlarge()],
            utilization: vec![
                metric(METRIC_CPU, METRIC_STATISTIC_MAXIMUM, 88.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_MAXIMUM, 97.0),
                metric(METRIC_CPU, METRIC_STATISTIC_AVERAGE, 64.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_AVERAGE, 90.0),
            ],
            finding_reason_codes: vec![METRIC_REASON_MEMORY_UNDERPROVISIONED.to_string()],
            current_performance_risk: PERFORMANCE_RISK_HIGH.to_string(),
            inferred_workload_types: vec!["Redis".to_string()],
            instance_state: INSTANCE_STATE_RUNNING.to_string(),
            tags: vec![(NAME_TAG_KEY.to_string(), "session-cache".to_string())],
        },
        // Already the right size for its workload: no candidate, so nothing to project onto.
        ObservedInstance {
            instance_id: "i-0a1b2c3d4e5f60003".to_string(),
            account_id,
            current: m5_xlarge(),
            candidates: Vec::new(),
            utilization: vec![
                metric(METRIC_CPU, METRIC_STATISTIC_MAXIMUM, 55.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_MAXIMUM, 60.0),
                metric(METRIC_CPU, METRIC_STATISTIC_AVERAGE, 41.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_AVERAGE, 48.0),
            ],
            finding_reason_codes: Vec::new(),
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: Vec::new(),
            instance_state: INSTANCE_STATE_RUNNING.to_string(),
            // An instance nobody tagged: it carries no tags, and so no name either.
            tags: Vec::new(),
        },
    ]
}

/// Per-service state for com-amazonaws-computeoptimizer-computeoptimizerservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The account's enrollment record, written by `UpdateEnrollmentStatus` and reported
    /// by `GetEnrollmentStatus`.
    pub enrollment: Enrollment,
    /// The organization's member accounts and their enrollment, which
    /// `GetEnrollmentStatusesForOrganization` reports and
    /// `GetEnrollmentStatus$numberOfMemberAccountsOptedIn` counts over. See
    /// [`OrganizationMemberStore`]: no operation of this API creates a member account.
    pub organization_members: OrganizationMemberStore,
    /// Every stored recommendation preference, ordered by key so a page boundary is
    /// stable across calls.
    pub preferences: BTreeMap<PreferenceKey, PreferenceRecord>,
    /// The Amazon EBS volumes Compute Optimizer observes, by volume id, so a page
    /// boundary is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetEBSVolumeRecommendations` REPORTS this
    /// observation and must not mutate the store (see [`observed_volumes`]). It is the
    /// SINGLE source for a volume: the EBS volume rows of [`Self::analysed_resources`]
    /// are projected from it.
    pub volumes: BTreeMap<String, ObservedVolume>,
    /// The Amazon EC2 instances Compute Optimizer observes, by instance id, so the
    /// iteration order is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: a reader REPORTS this analysis and must not
    /// mutate the store (see [`observed_instances`]). It is the SINGLE source for an
    /// instance: the `Ec2Instance` rows of [`Self::analysed_resources`] are projected from
    /// it, and `GetEC2RecommendationProjectedMetrics` projects its observed utilization onto
    /// the instance types the analysis recommends.
    pub instances: BTreeMap<String, ObservedInstance>,
    /// The Lambda function versions Compute Optimizer observes, by the resource part of
    /// their ARN, so a page boundary is stable across calls and the iteration order is the
    /// order a reader reports them in. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetLambdaFunctionRecommendations` REPORTS
    /// this observation and must not mutate the store (see [`observed_functions`]). It is
    /// the SINGLE source for a function: the Lambda function rows of
    /// [`Self::analysed_resources`] are projected from it.
    pub functions: BTreeMap<String, ObservedFunction>,
    /// Every Auto Scaling group Compute Optimizer has analysed, in analysis order, so a
    /// page boundary is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetAutoScalingGroupRecommendations` REPORTS
    /// this analysis and must not mutate the store (see
    /// [`seed_auto_scaling_group_analyses`]). It is the SINGLE source for a group: the Auto
    /// Scaling group rows of [`Self::analysed_resources`] are projected from it.
    pub auto_scaling_group_analyses: Vec<AnalyzedAutoScalingGroup>,
    /// The resources Compute Optimizer has analysed, in catalog order. Part of the
    /// store's initial value rather than something a reader writes on first use, so every
    /// findings reader serves the same rows and none of them mutates the store. The Auto
    /// Scaling group rows are PROJECTED from [`Self::auto_scaling_group_analyses`], the
    /// EBS volume rows from [`Self::volumes`], the Amazon EC2 instance rows from
    /// [`Self::instances`] and the Lambda function rows from
    /// [`Self::functions`] ([`seed_analysed_resources`]), so the two readers of one resource
    /// cannot disagree about its finding, its risk or what it costs.
    pub analysed_resources: Vec<AnalysedResource>,
    /// The resources Compute Optimizer's analysis found IDLE, which
    /// `GetIdleRecommendations` reports. Part of the store's initial value rather than
    /// something a reader writes on first use: that reader REPORTS this analysis and must
    /// not mutate the store (see [`IdleRecommendationStore`] and
    /// [`seed_idle_resource_analyses`]). It is the SINGLE source for an idle resource:
    /// `GetRDSDatabaseRecommendations$idle` is derived from it too.
    pub idle_recommendations: IdleRecommendationStore,
    /// The licensed Amazon EC2 instances Compute Optimizer observes, by instance id, so a
    /// page boundary is stable across calls and the iteration order is the order a reader
    /// reports them in. Part of the store's initial value rather than something a reader
    /// writes on first use: `GetLicenseRecommendations` REPORTS this observation and must
    /// not mutate the store (see [`observed_licensed_instances`]). It is the SINGLE source
    /// for a license: the `License` rows of [`Self::analysed_resources`] are projected from
    /// it.
    pub licenses: BTreeMap<String, ObservedLicensedInstance>,
    /// The recommendation export jobs the account has created, which every
    /// `Export*Recommendations` operation writes to and
    /// `DescribeRecommendationExportJobs` reports. See [`ExportJobStore`].
    pub export_jobs: ExportJobStore,
}

impl Default for ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The store's initial value: the account is not opted in, this emulator records no
    /// organization so it holds no member account, nobody has written a preference, the EBS
    /// volume observation, the Auto Scaling group analysis, the findings catalog and the
    /// idle resource analysis this emulator analysed — which a reader only sees once the
    /// account is enrolled — and no export job (only an export call creates one).
    fn default() -> Self {
        Self {
            enrollment: Enrollment::default(),
            organization_members: OrganizationMemberStore::default(),
            preferences: BTreeMap::new(),
            volumes: observed_volumes()
                .into_iter()
                .map(|volume| (volume.volume_id.clone(), volume))
                .collect(),
            instances: observed_instances()
                .into_iter()
                .map(|instance| (instance.instance_id.clone(), instance))
                .collect(),
            functions: observed_functions()
                .into_iter()
                .map(|function| (function.arn_resource(), function))
                .collect(),
            auto_scaling_group_analyses: seed_auto_scaling_group_analyses(),
            analysed_resources: seed_analysed_resources(),
            idle_recommendations: seed_idle_resource_analyses().into_iter().collect(),
            licenses: observed_licensed_instances()
                .into_iter()
                .map(|license| (license.instance_id.clone(), license))
                .collect(),
            export_jobs: ExportJobStore::default(),
        }
    }
}

/// The findings catalog, built once when the store is created.
///
/// This emulator observes no real workloads, so WHICH resources it holds is a fixed set
/// rather than something derived from traffic; every figure a summary reports is
/// nevertheless computed from the costs, findings, reason codes and risks stored here, so
/// the counts, the savings and the percentage always agree with each other and with the
/// rows a per-resource findings reader would report.
///
/// The catalog spans the finding, reason-code, performance-risk and inferred-workload
/// vocabularies of several resource types so an aggregate over it is not a single row
/// restated: it holds a resource whose recommendation saves money, one the analysis would
/// GROW (so it saves none), and one it leaves alone.
///
/// A row records a resource's REGION-FREE identity (its service namespace and resource
/// part): Compute Optimizer is regional, so the region a reported ARN names is the region
/// of the endpoint the request addressed ([`AnalysedResource::arn_in`]).
fn seed_analysed_resources() -> Vec<AnalysedResource> {
    let account = ::engine::emulator_account_id();
    let row = |resource_type: &str,
               arn_service: &str,
               arn_resource: &str,
               finding: &str,
               finding_reason_codes: &[&str],
               current_performance_risk: &str,
               current_monthly_cost: f64,
               recommended_monthly_cost: f64,
               inferred_workload_types: &[&str]| AnalysedResource {
        account_id: account.to_string(),
        resource_type: resource_type.to_string(),
        arn_service: arn_service.to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        finding_reason_codes: finding_reason_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect(),
        current_performance_risk: current_performance_risk.to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        inferred_workload_types: inferred_workload_types
            .iter()
            .map(|workload| (*workload).to_string())
            .collect(),
        // Set below for the rows analysed as a database; every other resource type has no
        // database configuration to record.
        rds_database: None,
    };

    vec![
        row(
            SOURCE_TYPE_ECS_SERVICE,
            "ecs",
            "service/payments/checkout",
            FINDING_OVERPROVISIONED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            120.0,
            80.0,
            &[],
        ),
        // An over-provisioned Aurora/RDS database: the analysis recommends a smaller DB
        // instance class, so it saves. The database's own configuration and the utilization
        // observed on it are recorded too, which is what `GetRDSDatabaseRecommendations`
        // reports beside the finding, the risk and the costs every analysed row carries.
        AnalysedResource {
            rds_database: Some(RdsDatabaseDetail {
                // `Engine`/`EngineVersion` are free-form strings in the model; these are the
                // engine name and version an RDS PostgreSQL instance reports.
                engine: Some("postgres".to_string()),
                engine_version: Some("15.4".to_string()),
                // Not a cluster member, so no promotion tier and no cluster identifier.
                promotion_tier: None,
                db_cluster_identifier: None,
                current_db_instance_class: Some("db.r6g.2xlarge".to_string()),
                recommended_db_instance_class: Some("db.r6g.xlarge".to_string()),
                current_storage_configuration: Some(DBStorageConfiguration {
                    storage_type: Some("gp3".to_string()),
                    allocated_storage: 500,
                    iops: Some(3_000),
                    max_allocated_storage: Some(1_000),
                    storage_throughput: Some(125),
                }),
                // The analysis would keep the storage as it is: its finding is about the
                // instance, so there is no storage configuration to move to.
                recommended_storage_configuration: None,
                // `RDSDBMetricName` / `RDSDBMetricStatistic` values
                // (`crate::ops::types::R_D_S_D_B_METRIC_NAME_VALUES`).
                utilization_metrics: vec![
                    RDSDBUtilizationMetric {
                        name: Some("CPU".to_string()),
                        statistic: Some("Maximum".to_string()),
                        value: 17.5,
                    },
                    RDSDBUtilizationMetric {
                        name: Some("Memory".to_string()),
                        statistic: Some("Maximum".to_string()),
                        value: 24.0,
                    },
                    RDSDBUtilizationMetric {
                        name: Some("DatabaseConnections".to_string()),
                        statistic: Some("Maximum".to_string()),
                        value: 12.0,
                    },
                ],
                last_refresh_timestamp: None,
                tags: Vec::new(),
            }),
            ..row(
                SOURCE_TYPE_RDS_DB_INSTANCE,
                "rds",
                "db:orders-primary",
                FINDING_OVERPROVISIONED,
                &[],
                PERFORMANCE_RISK_MEDIUM,
                300.0,
                225.0,
                &[],
            )
        },
    ]
    .into_iter()
    // The Amazon EC2 instance rows are the SAME observation an instance reader reports,
    // projected onto the findings row ([`ObservedInstance::to_analysed_resource`]) rather
    // than stated a second time here, so a summary of the account's instances counts
    // exactly the instances that reader reports and agrees with it on each one's finding,
    // risk and cost.
    .chain(
        observed_instances()
            .iter()
            .map(ObservedInstance::to_analysed_resource),
    )
    // The Auto Scaling group rows are the SAME analysis
    // `GetAutoScalingGroupRecommendations` reports, projected onto the findings row
    // ([`AnalyzedAutoScalingGroup::to_analysed_resource`]) rather than stated a second
    // time here, so a summary of the account's Auto Scaling groups counts exactly the
    // groups that reader reports and agrees with it on each one's finding, risk and cost.
    .chain(
        seed_auto_scaling_group_analyses()
            .iter()
            .map(AnalyzedAutoScalingGroup::to_analysed_resource),
    )
    // The Amazon EBS volume rows are the SAME observation
    // `GetEBSVolumeRecommendations` reports, projected the same way
    // ([`ObservedVolume::to_analysed_resource`]) under the look-back period in force when
    // no preference sets one, so a summary of the account's volumes counts exactly the
    // volumes that reader reports and agrees with it on each one's finding, risk and cost.
    .chain(
        observed_volumes()
            .iter()
            .map(|volume| volume.to_analysed_resource(DEFAULT_LOOK_BACK_PERIOD)),
    )
    // The Lambda function rows are the SAME observation
    // `GetLambdaFunctionRecommendations` reports, projected the same way
    // ([`ObservedFunction::to_analysed_resource`]) under the look-back period in force when
    // no preference sets one, so a summary of the account's functions counts exactly the
    // functions that reader reports and agrees with it on each one's finding, risk and
    // cost. A function the analysis reached no finding for is no row at all.
    .chain(
        observed_functions()
            .iter()
            .filter_map(|function| function.to_analysed_resource(DEFAULT_LOOK_BACK_PERIOD)),
    )
    // The commercial software license rows are the SAME observation
    // `GetLicenseRecommendations` reports, projected the same way
    // ([`ObservedLicensedInstance::to_analysed_resource`]), so a summary of the account's
    // licenses counts exactly the licenses that reader reports and agrees with it on each
    // one's finding and what adopting its recommendation saves. A license whose metrics were
    // insufficient carries a finding no `Summary` can hold, so it is no row at all.
    .chain(
        observed_licensed_instances()
            .iter()
            .filter_map(ObservedLicensedInstance::to_analysed_resource),
    )
    .collect()
}

/// The `FindingReasonCode` values (`crate::ops::types::FINDING_REASON_CODE_VALUES`) — the
/// only two `ReasonCodeSummary$name` can carry.
pub const METRIC_REASON_MEMORY_OVERPROVISIONED: &str = "MemoryOverprovisioned";
/// See [`METRIC_REASON_MEMORY_OVERPROVISIONED`].
pub const METRIC_REASON_MEMORY_UNDERPROVISIONED: &str = "MemoryUnderprovisioned";

/// The opaque `nextToken` `GetRecommendationSummaries` hands out: the
/// `RecommendationSourceType` the next page starts at, hex-encoded so a client can
/// neither read nor construct one. A token from the other paginated reader decodes to
/// nothing here (and this one to nothing there), so neither can resume the other's walk.
pub fn encode_summary_page_token(resource_type: &str) -> String {
    ::hex::encode(::serde_json::json!({ "summariesFrom": resource_type }).to_string())
}

/// The resource type a `nextToken` this operation issued stands for, or `None` for a
/// token it did not.
pub fn decode_summary_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok()?;
    decoded
        .get("summariesFrom")?
        .as_str()
        .map(ToString::to_string)
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// Whether a `PutRecommendationPreferences` for `key` may be stored: an update to a
    /// key already held always may, a NEW key only while the store is below
    /// [`MAX_PREFERENCE_RECORDS`]. Callers refuse with a modelled error rather than
    /// growing past the bound.
    pub fn can_hold(&self, key: &PreferenceKey) -> bool {
        self.preferences.len() < MAX_PREFERENCE_RECORDS || self.preferences.contains_key(key)
    }

    /// The stored records that apply to `resource_arn`, MOST SPECIFIC FIRST, so a caller
    /// resolving an effective value takes the first record that sets the field it wants.
    ///
    /// The ACCOUNT whose account-level records apply is the one this service answers for
    /// ([`engine::emulator_account_id`]), never the ARN's own account field: a resource of
    /// any other account is refused by the reader that took the ARN
    /// ([`is_foreign_account`]), so a record scoped to another account applies to nothing.
    pub fn records_applying_to(
        &self,
        resource_arn: &str,
        arn: ParsedResourceArn<'_>,
    ) -> Vec<&PreferenceRecord> {
        self.records_applying_to_resource(
            resource_arn,
            arn.resource_type,
            ::engine::emulator_account_id(),
        )
    }

    /// The stored records that apply to one resource of `resource_type` owned by
    /// `account_id`, MOST SPECIFIC FIRST — the same precedence [`records_applying_to`]
    /// resolves with, for a resource whose type and account a caller already knows (a
    /// findings reader reporting the preferences in force for a row it holds, say) rather
    /// than one it parses out of an ARN.
    ///
    /// [`records_applying_to`]: Self::records_applying_to
    pub fn records_applying_to_resource(
        &self,
        resource_arn: &str,
        resource_type: &str,
        account_id: &str,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == resource_type)
            .filter_map(|(key, record)| {
                scope_precedence(key, resource_arn, account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The stored records that apply to a resource of `resource_type` owned by
    /// `account_id`, MOST SPECIFIC FIRST — [`records_applying_to`] for a resource type no
    /// `ResourceArn` scope can name (see [`account_scope_precedence`]).
    ///
    /// [`records_applying_to`]: Self::records_applying_to
    pub fn records_for_account(
        &self,
        resource_type: &str,
        account_id: &str,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == resource_type)
            .filter_map(|(key, record)| {
                account_scope_precedence(key, account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The look-back period in force for a resource of `resource_type` owned by
    /// `account_id`: the value the most specific record that sets it carries, or
    /// [`DEFAULT_LOOK_BACK_PERIOD`] when none does.
    pub fn effective_look_back_period(&self, resource_type: &str, account_id: &str) -> String {
        self.records_for_account(resource_type, account_id)
            .into_iter()
            .find_map(|record| record.look_back_period.clone())
            .unwrap_or_else(|| DEFAULT_LOOK_BACK_PERIOD.to_string())
    }

    /// The Amazon EBS volumes whose observation a caller can read, by volume id.
    ///
    /// The same gate [`visible_auto_scaling_group_analyses`] applies: Compute Optimizer
    /// reports recommendations for an ENROLLED account only, and only once the analysis
    /// that enrollment starts is due ([`ANALYSIS_DELAY`]), so an account that never opted
    /// in, one that opted out, and one that has just opted in all read the empty set
    /// rather than an analysis nothing has produced yet.
    ///
    /// [`visible_auto_scaling_group_analyses`]: Self::visible_auto_scaling_group_analyses
    pub fn visible_volumes(&self) -> impl Iterator<Item = &ObservedVolume> {
        let visible = self.enrollment.analysis_visible();
        self.volumes.values().filter(move |_| visible)
    }

    /// The Lambda functions whose observation a caller can read, in ARN order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_functions(&self) -> impl Iterator<Item = &ObservedFunction> {
        let visible = self.enrollment.analysis_visible();
        self.functions.values().filter(move |_| visible)
    }

    /// The licensed Amazon EC2 instances whose observation a caller can read, in instance-id
    /// order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_licenses(&self) -> impl Iterator<Item = &ObservedLicensedInstance> {
        let visible = self.enrollment.analysis_visible();
        self.licenses.values().filter(move |_| visible)
    }

    /// The Amazon EC2 instances whose analysis a caller can read, in instance-id order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_instances(&self) -> impl Iterator<Item = &ObservedInstance> {
        let visible = self.enrollment.analysis_visible();
        self.instances.values().filter(move |_| visible)
    }

    /// The idle resource analysis a caller can read, in record order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_idle_recommendations(&self) -> impl Iterator<Item = &IdleResourceAnalysis> {
        let visible = self.enrollment.analysis_visible();
        self.idle_recommendations
            .iter_in_order()
            .filter(move |_| visible)
    }

    /// The findings catalog rows a caller can read, in catalog order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet. Every reader of [`Self::analysed_resources`] goes through here — the
    /// per-resource readers of a catalogued recommendation and
    /// [`analysed_resources_by_type`], which the summary reader and the export plan group
    /// the catalog with — so no two readers of one analysis can disagree about whether it
    /// exists yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    /// [`analysed_resources_by_type`]: Self::analysed_resources_by_type
    pub fn visible_analysed_resources(&self) -> impl Iterator<Item = &AnalysedResource> {
        let visible = self.enrollment.analysis_visible();
        self.analysed_resources.iter().filter(move |_| visible)
    }
}

/// The Amazon EBS volume inventory this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so the volumes and their metric samples are a
/// fixed observation rather than something derived from traffic. Everything a
/// recommendation reports is nevertheless computed from what is stored here — the
/// analysed peaks come from these samples, the recommended configuration from those
/// peaks, and every cost from [`volume_monthly_cost`] — so a reader cannot report a
/// figure the observation does not support.
///
/// The set spans the cases a volume recommendation distinguishes: a previous-generation
/// volume whose demand fits its capacity (cheaper on gp3), a gp3 volume whose demand
/// outruns its provisioned IOPS (needs more performance), a heavily over-provisioned
/// Provisioned IOPS volume whose peak demand arrived outside the default look-back
/// period, and a gp3 volume already provisioned for its workload.
fn observed_volumes() -> Vec<ObservedVolume> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        // gp2, 500 GiB: 3 IOPS per GiB of baseline (1,500) with a 3,000 IOPS burst and
        // 250 MiB/s of throughput. Its peak fits that comfortably, so the gp3 option
        // carries the same workload for less money.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60001".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some("gp2".to_string()),
                volume_size: 500,
                volume_baseline_iops: 1_500,
                volume_burst_iops: 3_000,
                volume_baseline_throughput: 250,
                volume_burst_throughput: 250,
                root_volume: Some(true),
            },
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 0.0,
                    read_ops_per_second: 300.0,
                    write_ops_per_second: 400.0,
                    read_bytes_per_second: 20.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 25.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 3.0,
                    read_ops_per_second: 450.0,
                    write_ops_per_second: 450.0,
                    read_bytes_per_second: 30.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 30.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 40.0,
                    read_ops_per_second: 200.0,
                    write_ops_per_second: 150.0,
                    read_bytes_per_second: 10.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 8.0 * BYTES_PER_MIB,
                },
            ],
        },
        // gp3, 100 GiB at its included performance, driven past it: the option has to
        // provision more IOPS and more throughput, which costs MORE than the volume
        // costs today — a recommendation is not always a saving.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60002".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some(VOLUME_TYPE_GP3.to_string()),
                volume_size: 100,
                volume_baseline_iops: GP3_INCLUDED_IOPS,
                volume_burst_iops: 0,
                volume_baseline_throughput: GP3_INCLUDED_THROUGHPUT_MIB,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: vec![("Owner".to_string(), "TeamB".to_string())],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 0.0,
                    read_ops_per_second: 2_400.0,
                    write_ops_per_second: 2_400.0,
                    read_bytes_per_second: 60.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 60.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 10.0,
                    read_ops_per_second: 1_000.0,
                    write_ops_per_second: 900.0,
                    read_bytes_per_second: 40.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 30.0 * BYTES_PER_MIB,
                },
            ],
        },
        // io1, 200 GiB with 8,000 provisioned IOPS: far more than the workload asks for,
        // and its heaviest day falls OUTSIDE the default look-back period, so a longer
        // look-back preference changes the option this volume gets.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60003".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some("io1".to_string()),
                volume_size: 200,
                volume_baseline_iops: 8_000,
                volume_burst_iops: 0,
                volume_baseline_throughput: 500,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: vec![
                ("Owner".to_string(), "TeamB".to_string()),
                ("Environment".to_string(), "production".to_string()),
            ],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 1.0,
                    read_ops_per_second: 600.0,
                    write_ops_per_second: 600.0,
                    read_bytes_per_second: 30.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 20.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 50.0,
                    read_ops_per_second: 1_500.0,
                    write_ops_per_second: 1_500.0,
                    read_bytes_per_second: 100.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 60.0 * BYTES_PER_MIB,
                },
            ],
        },
        // gp3, 50 GiB, already provisioned for its workload — and observed for only a
        // week, so the period analysed is that week however long a look-back period the
        // account prefers.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60004".to_string(),
            account_id,
            configuration: VolumeConfiguration {
                volume_type: Some(VOLUME_TYPE_GP3.to_string()),
                volume_size: 50,
                volume_baseline_iops: GP3_INCLUDED_IOPS,
                volume_burst_iops: 0,
                volume_baseline_throughput: GP3_INCLUDED_THROUGHPUT_MIB,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: Vec::new(),
            days_observed: 7.0,
            samples: vec![VolumeSample {
                days_ago: 0.0,
                read_ops_per_second: 700.0,
                write_ops_per_second: 800.0,
                read_bytes_per_second: 20.0 * BYTES_PER_MIB,
                write_bytes_per_second: 20.0 * BYTES_PER_MIB,
            }],
        },
    ]
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The preferences in force for one resource: the value from the most specific scope
    /// that sets each field, and the service's documented default for every field no
    /// record sets — "considers all applicable preferences that you might have set at the
    /// resource, account, and organization level", each field independently, so a
    /// resource-level record that sets one preference still inherits the rest.
    ///
    /// The ONE resolution in this service: `GetEffectiveRecommendationPreferences` reports
    /// it and every recommendation reader states the preferences it was generated under
    /// from it, so the two cannot disagree.
    ///
    /// The account-level records considered are those of the account this service answers
    /// for ([`engine::emulator_account_id`]), not of the ARN's own account field — see
    /// [`records_applying_to`](Self::records_applying_to).
    pub fn effective_preferences(
        &self,
        resource_arn: &str,
        arn: ParsedResourceArn<'_>,
    ) -> EffectivePreferences {
        self.effective_preferences_for(
            resource_arn,
            arn.resource_type,
            ::engine::emulator_account_id(),
        )
    }

    /// The preferences in force for one resource of `resource_type` owned by `account_id` —
    /// the same resolution [`effective_preferences`] performs, for a resource whose type and
    /// account a caller already knows (a findings reader reporting the preferences a stored
    /// row's recommendation was generated under, say) rather than one it parses out of an
    /// ARN.
    ///
    /// [`effective_preferences`]: Self::effective_preferences
    pub fn effective_preferences_for(
        &self,
        resource_arn: &str,
        resource_type: &str,
        account_id: &str,
    ) -> EffectivePreferences {
        let applicable = self.records_applying_to_resource(resource_arn, resource_type, account_id);
        let effective = |field: fn(&PreferenceRecord) -> bool| {
            applicable.iter().copied().find(|record| field(record))
        };
        EffectivePreferences {
            enhanced_infrastructure_metrics: effective(|record| {
                record.enhanced_infrastructure_metrics.is_some()
            })
            .and_then(|record| record.enhanced_infrastructure_metrics.clone()),
            inferred_workload_types: effective(|record| record.inferred_workload_types.is_some())
                .and_then(|record| record.inferred_workload_types.clone())
                .unwrap_or_else(|| INFERRED_WORKLOAD_TYPES_ACTIVE.to_string()),
            external_metrics_preference: effective(|record| {
                record.external_metrics_preference.is_some()
            })
            .and_then(|record| record.external_metrics_preference.clone()),
            look_back_period: effective(|record| record.look_back_period.is_some())
                .and_then(|record| record.look_back_period.clone())
                .unwrap_or_else(|| DEFAULT_LOOK_BACK_PERIOD.to_string()),
            // "You can only set CPU and memory utilization preferences for the Amazon EC2
            // instance resource type", so for any other resource type there is no default
            // to report and the list is empty.
            utilization_preferences: effective(|record| !record.utilization_preferences.is_empty())
                .map(|record| record.utilization_preferences.clone())
                .unwrap_or_else(|| {
                    if resource_type == RESOURCE_TYPE_EC2_INSTANCE {
                        default_utilization_preferences()
                    } else {
                        Vec::new()
                    }
                }),
            preferred_resources: effective(|record| !record.preferred_resources.is_empty())
                .map(|record| {
                    record
                        .preferred_resources
                        .iter()
                        .map(effective_preferred_resource)
                        .collect()
                })
                .unwrap_or_else(default_preferred_resources),
            savings_estimation_mode: effective(|record| record.savings_estimation_mode.is_some())
                .and_then(|record| record.savings_estimation_mode.clone()),
        }
    }

    /// Whether the account this service answers for MANAGES an organization — whether it is
    /// an organization's management account or delegated administrator.
    ///
    /// AWS Organizations is what records an account's membership of an organization, and no
    /// operation of THIS API creates a member account ([`OrganizationMemberStore`]), so the
    /// account manages an organization exactly when this service's store holds member
    /// accounts for it: an emulator that records none presents a standalone account. The ONE
    /// test for the whole crate, so every operation whose surface is served only to a
    /// management account — the reader of the organization's enrollment statuses, and every
    /// operation taking the organization-wide `includeMemberAccounts` — judges the caller the
    /// same way.
    pub fn manages_an_organization(&self) -> bool {
        !self.organization_members.is_empty()
    }

    /// Why an `includeMemberAccounts` the request carries cannot be honoured, or `None` when
    /// it can.
    ///
    /// The member is the organization-wide switch every operation that takes it conditions on
    /// the caller's role: it asks for the resources, or the enrollment, of "all member
    /// accounts of the organization", which only a management account or delegated
    /// administrator has. So asking for them while managing no organization
    /// ([`manages_an_organization`](Self::manages_an_organization)) is refused with
    /// [`INCLUDE_MEMBER_ACCOUNTS_MESSAGE`] before anything is written, rather than accepted as
    /// a call over an organization this service does not record. Asking NOT to include them is
    /// what a standalone account's own call carries and is always honoured.
    pub fn include_member_accounts_rejection(
        &self,
        include_member_accounts: bool,
    ) -> Option<&'static str> {
        (include_member_accounts && !self.manages_an_organization())
            .then_some(INCLUDE_MEMBER_ACCOUNTS_MESSAGE)
    }

    /// Every member account of the organization as `GetEnrollmentStatusesForOrganization`
    /// reports it at `now`, in account-id order.
    pub fn organization_enrollment_statuses(&self, now: f64) -> Vec<AccountEnrollmentStatus> {
        self.organization_members
            .iter_in_order()
            .map(|member| member.to_status(now))
            .collect()
    }

    /// How many organization member accounts are opted in at `now` — the count
    /// `GetEnrollmentStatus` reports. DERIVED from the member accounts themselves, so the
    /// count and the rows `GetEnrollmentStatusesForOrganization` reports cannot disagree.
    pub fn member_accounts_opted_in(&self, now: f64) -> i32 {
        let opted_in = self
            .organization_members
            .iter_in_order()
            .filter(|member| member.is_opted_in_at(now))
            .count();
        i32::try_from(opted_in).unwrap_or(i32::MAX)
    }

    /// The Auto Scaling groups whose analysis a caller can read, in analysis order.
    ///
    /// Compute Optimizer reports recommendations for an ENROLLED account only, and only
    /// once the analysis that enrollment starts is due ([`ANALYSIS_DELAY`]), so an
    /// account that never opted in, one that opted out, and one that has just opted in
    /// all read the empty set rather than an analysis nothing has produced yet. The rule
    /// itself is [`Enrollment::analysis_visible`], which every surface that serves this
    /// service's analysis asks.
    pub fn visible_auto_scaling_group_analyses(&self) -> &[AnalyzedAutoScalingGroup] {
        if !self.enrollment.analysis_visible() {
            return &[];
        }
        &self.auto_scaling_group_analyses
    }

    /// The analysed resources `account_id` owns, grouped by `RecommendationSourceType` and
    /// keyed by it, so a reader walks the groups in one stable order whatever the catalog's
    /// own order is.
    ///
    /// The rows are the ones a caller can read ([`visible_analysed_resources`]), so a group
    /// an account has no analysis for yet is absent rather than summarised.
    ///
    /// This emulator presents exactly one account ([`engine::emulator_account_id`]), so a
    /// request that asks for any other account's findings — which only an organization's
    /// management account may do — selects nothing rather than someone else's resources.
    ///
    /// [`visible_analysed_resources`]: Self::visible_analysed_resources
    pub fn analysed_resources_by_type(
        &self,
        account_id: &str,
    ) -> BTreeMap<&str, Vec<&AnalysedResource>> {
        let mut grouped: BTreeMap<&str, Vec<&AnalysedResource>> = BTreeMap::new();
        for resource in self.visible_analysed_resources() {
            if resource.account_id != account_id {
                continue;
            }
            grouped
                .entry(resource.resource_type.as_str())
                .or_default()
                .push(resource);
        }
        grouped
    }

    /// The idle resource analysis `account_id` owns, grouped by the
    /// `RecommendationSourceType` a summary of it is reported under
    /// ([`source_type_of_idle_resource_type`]) and keyed by it, so the summary reader walks
    /// the idle groups in the SAME order it walks the findings catalog's
    /// ([`Self::analysed_resources_by_type`]) and the two agree on which group a resource
    /// belongs to.
    ///
    /// The records are the ones a caller can read ([`visible_idle_recommendations`]), so a
    /// group an account has no analysis for yet is absent rather than summarised, and a
    /// record of a resource kind the idle vocabulary does not cover is in no group at all.
    ///
    /// [`visible_idle_recommendations`]: Self::visible_idle_recommendations
    pub fn idle_analyses_by_source_type(
        &self,
        account_id: &str,
    ) -> BTreeMap<&'static str, Vec<&IdleResourceAnalysis>> {
        let mut grouped: BTreeMap<&'static str, Vec<&IdleResourceAnalysis>> = BTreeMap::new();
        for analysis in self.visible_idle_recommendations() {
            if analysis.account_id != account_id {
                continue;
            }
            let Some(source_type) = analysis
                .resource_type()
                .and_then(source_type_of_idle_resource_type)
            else {
                continue;
            };
            grouped.entry(source_type).or_default().push(analysis);
        }
        grouped
    }
}

// ===========================================================================
// The Amazon ECS service analysis the ECS service readers report
//
// Compute Optimizer "generates recommendations for Amazon ECS services on Fargate" from
// the configuration the Amazon ECS inventory holds for a service and the utilization
// published for it, so no request to THIS API creates an ECS service recommendation. The
// INPUTS of that analysis and the SIZING it performs live here, stated once:
// `GetECSServiceRecommendations` reports the recommendation computed from them, and
// `GetECSServiceRecommendationProjectedMetrics` the series projected onto its recommended
// task size, so the two readers of one analysis cannot disagree on which service an ARN
// names, on what was observed for it, on the task size recommended for it, or on the
// utilization projected onto that size.
// ===========================================================================

/// The CloudWatch namespace Amazon ECS publishes a service's utilization in, and the two
/// series an ECS service recommendation is computed from — the metrics the model's own
/// `ECSServiceRecommendation$findingReasonCodes` doc names ("analyzing the
/// `CPUUtilization` metric of the current service during the look-back period", and the
/// same for `MemoryUtilization`).
pub const ECS_METRIC_NAMESPACE: &str = "AWS/ECS";
/// See [`ECS_METRIC_NAMESPACE`].
pub const ECS_CPU_UTILIZATION_METRIC: &str = "CPUUtilization";
/// See [`ECS_METRIC_NAMESPACE`].
pub const ECS_MEMORY_UTILIZATION_METRIC: &str = "MemoryUtilization";

/// The dimensions that identify ONE service's series in that namespace.
const ECS_CLUSTER_DIMENSION: &str = "ClusterName";
/// See [`ECS_CLUSTER_DIMENSION`].
const ECS_SERVICE_DIMENSION: &str = "ServiceName";

/// The ARN namespace Amazon ECS resources carry, and the resource KIND an ECS service's
/// ARN names (`service/cluster-name/service-name`).
const ECS_ARN_NAMESPACE: &str = "ecs";
/// See [`ECS_ARN_NAMESPACE`].
const ECS_SERVICE_RESOURCE_KIND: &str = "service";

/// The `ECSServiceLaunchType` values (`E_C_S_SERVICE_LAUNCH_TYPE_VALUES`). Compute
/// Optimizer "generates recommendations for Amazon ECS services on Fargate", so only
/// [`ECS_SERVICE_LAUNCH_TYPE_FARGATE`] is an analysable service.
pub const ECS_SERVICE_LAUNCH_TYPE_FARGATE: &str = "Fargate";
/// See [`ECS_SERVICE_LAUNCH_TYPE_FARGATE`].
pub const ECS_SERVICE_LAUNCH_TYPE_EC2: &str = "EC2";

/// The ECS `LaunchType` enum values (the ECS model's `LAUNCH_TYPE_VALUES`) this analysis
/// reads, and the ECS capacity providers that run a task on Fargate. ECS spells them in
/// upper case; Compute Optimizer spells its own `ECSServiceLaunchType` in mixed case.
const ECS_LAUNCH_TYPE_FARGATE: &str = "FARGATE";
/// See [`ECS_LAUNCH_TYPE_FARGATE`].
const ECS_LAUNCH_TYPE_EC2: &str = "EC2";
/// See [`ECS_LAUNCH_TYPE_FARGATE`].
const ECS_FARGATE_CAPACITY_PROVIDERS: [&str; 2] = ["FARGATE", "FARGATE_SPOT"];

/// How many seconds one day of a look-back period is.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// The task-level CPU/memory combinations AWS Fargate supports, smallest first: CPU units
/// paired with every task memory (MiB) that CPU size admits.
///
/// Verbatim from the Amazon ECS model's own `RegisterTaskDefinitionRequest$memory`
/// documentation ("If using the Fargate launch type, this field is required and you must
/// use one of the following values. This determines your range of supported values for the
/// `cpu` parameter"), which lists: 512/1024/2048 MiB for 256 CPU units; 1024-4096 in 1 GB
/// steps for 512; 2048-8192 in 1 GB steps for 1024; "Between 4096 (4 GB) and 16384 (16 GB)
/// in increments of 1024 (1 GB)" for 2048; "Between 8192 (8 GB) and 30720 (30 GB) in
/// increments of 1024 (1 GB)" for 4096; "Between 16 GB and 60 GB in 4 GB increments" for
/// 8192; "Between 32GB and 120 GB in 8 GB increments" for 16384.
///
/// A recommendation option is picked from THIS list, so the size reported back is one a
/// Fargate task definition could actually be registered with.
const FARGATE_TASK_SIZES: &[(i32, &[i32])] = &[
    (256, &[512, 1024, 2048]),
    (512, &[1024, 2048, 3072, 4096]),
    (1024, &[2048, 3072, 4096, 5120, 6144, 7168, 8192]),
    (
        2048,
        &[
            4096, 5120, 6144, 7168, 8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360, 16384,
        ],
    ),
    (
        4096,
        &[
            8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360, 16384, 17408, 18432, 19456,
            20480, 21504, 22528, 23552, 24576, 25600, 26624, 27648, 28672, 29696, 30720,
        ],
    ),
    (
        8192,
        &[
            16384, 20480, 24576, 28672, 32768, 36864, 40960, 45056, 49152, 53248, 57344, 61440,
        ],
    ),
    (
        16384,
        &[
            32768, 40960, 49152, 57344, 65536, 73728, 81920, 90112, 98304, 106496, 114688, 122880,
        ],
    ),
];

/// The Amazon ECS service one ARN names.
#[derive(Debug, Clone)]
pub struct EcsServiceRef {
    /// The ARN as given, which a reader reports back as the service's own.
    pub arn: String,
    /// The ARN's account field: the account that owns the service.
    pub account_id: String,
    /// The ARN's region field: the regional endpoint the service belongs to.
    pub region: String,
    /// The cluster that hosts the service, and the service's name — the pair that keys
    /// the ECS store and names the service's CloudWatch dimensions.
    pub cluster: String,
    /// See [`EcsServiceRef::cluster`].
    pub service: String,
}

/// The ARN of an Amazon ECS service, split into the parts the analysis reads, or `None`
/// for anything that is not one.
///
/// What an ARN looks like at all is [`parse_arn`]'s decision — "the one place this service
/// decides what an ARN looks like, so every operation that takes one accepts and refuses the
/// same strings" — and this adds what the member documents on top of it:
/// `arn:aws:ecs:region:aws_account_id:service/cluster-name/service-name`
/// (`GetECSServiceRecommendations$serviceArns`, `GetECSServiceRecommendationProjectedMetrics$serviceArn`),
/// whose account field must be an account id and whose resource part names the CLUSTER as
/// well as the service. The cluster/name split is the ECS store's own
/// (`ecs::state::cluster_and_service_from_reference`), which is what keeps these readers and
/// the service that owns the record from disagreeing on which service an ARN names.
pub fn parse_ecs_service_arn(arn: &str) -> Option<EcsServiceRef> {
    let parsed = parse_arn(arn)?;
    if parsed.service != ECS_ARN_NAMESPACE
        || parsed.resource_kind() != ECS_SERVICE_RESOURCE_KIND
        || parsed.region.is_empty()
        || !is_account_id(parsed.account_id)
    {
        return None;
    }
    let (cluster, service) = ecs::state::cluster_and_service_from_reference(arn);
    let cluster = cluster?;
    if cluster.is_empty() || service.is_empty() {
        return None;
    }
    Some(EcsServiceRef {
        arn: arn.to_string(),
        account_id: parsed.account_id.to_string(),
        region: parsed.region.to_string(),
        cluster,
        service,
    })
}

/// The persisted ECS configuration of one service: every value taken from the ECS store,
/// which is why a recommendation can report a current configuration at all.
#[derive(Debug, Clone)]
pub struct EcsServiceConfig {
    /// The service's ECS `launchType`, when it names one.
    pub launch_type: Option<String>,
    /// The capacity providers its `capacityProviderStrategy` names, when it names no
    /// launch type.
    pub capacity_providers: Vec<String>,
    /// The task definition the service runs.
    pub task_definition_arn: Option<String>,
    /// That task definition's task-level `cpu`, in CPU units.
    pub cpu_units: Option<i32>,
    /// That task definition's task-level `memory`, in MiB.
    pub memory_mib: Option<i32>,
    /// How many tasks the service keeps running (`Service$desiredCount`): the multiplier
    /// between what ONE task of the size above costs per month and what the SERVICE does.
    pub desired_tasks: i32,
    /// Its container definitions, as the modelled `ContainerConfiguration` list.
    pub containers: Vec<ContainerConfiguration>,
    /// The service's tags.
    pub tags: Vec<Tag>,
}

/// What the ECS store holds for one service, or `None` when it holds no ACTIVE record for
/// it. A service whose record is INACTIVE (deleted) is gone as far as a reader is
/// concerned, exactly as `DescribeServices` reports it.
pub fn ecs_service_config(
    inventory: &ecs::state::ComAmazonawsEcsAmazonec2containerservicev20141113State,
    reference: &EcsServiceRef,
) -> Option<EcsServiceConfig> {
    let record = inventory
        .services
        .get(&(reference.cluster.clone(), reference.service.clone()))
        .filter(|record| record.is_active())?;
    let definition = record
        .task_definition
        .as_ref()
        .and_then(|(family, revision)| {
            inventory.task_definitions.get(&(family.clone(), *revision))
        });
    Some(EcsServiceConfig {
        launch_type: record.launch_type.clone(),
        capacity_providers: record
            .capacity_provider_strategy
            .iter()
            .map(|item| item.capacity_provider.clone())
            .collect(),
        task_definition_arn: record.task_definition_arn.clone(),
        cpu_units: definition
            .and_then(|definition| definition.cpu.as_deref())
            .and_then(ecs_cpu_units),
        memory_mib: definition
            .and_then(|definition| definition.memory.as_deref())
            .and_then(ecs_memory_mib),
        desired_tasks: record.desired_count,
        containers: definition
            .map(|definition| {
                definition
                    .container_definitions
                    .iter()
                    .map(|container| ContainerConfiguration {
                        container_name: container.name.clone(),
                        memory_size_configuration: (container.memory.is_some()
                            || container.memory_reservation.is_some())
                        .then_some(MemorySizeConfiguration {
                            memory: container.memory,
                            memory_reservation: container.memory_reservation,
                        }),
                        // The ECS member defaults to `0`, which is "no container-level
                        // CPU reservation" rather than a reservation of nothing.
                        cpu: (container.cpu > 0).then_some(container.cpu),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        tags: record
            .tags
            .iter()
            .map(|tag| Tag {
                key: tag.key.clone(),
                value: tag.value.clone(),
            })
            .collect(),
    })
}

/// A task definition's task-level `cpu` as CPU units, or `None` for a value that is
/// neither form the ECS model documents: "It can be expressed as an integer using CPU
/// units (for example, `1024`) or as a string using vCPUs (for example, `1 vCPU` or `1
/// vcpu`) … String values are converted to an integer indicating the CPU units"
/// (`RegisterTaskDefinitionRequest$cpu`). One vCPU is 1024 CPU units.
pub fn ecs_cpu_units(value: &str) -> Option<i32> {
    let value = value.trim();
    let lowered = value.to_ascii_lowercase();
    for suffix in ["vcpus", "vcpu"] {
        if let Some(count) = lowered.strip_suffix(suffix) {
            let count: f64 = count.trim().parse().ok()?;
            return Some(ecs_rounded_units(count * 1024.0));
        }
    }
    value.parse().ok()
}

/// A task definition's task-level `memory` as MiB, or `None` for a value that is neither
/// form the ECS model documents: "It can be expressed as an integer using MiB (for example
/// ,`1024`) or as a string using GB (for example, `1GB` or `1 GB`) … String values are
/// converted to an integer indicating the MiB" (`RegisterTaskDefinitionRequest$memory`).
/// One GB is 1024 MiB.
pub fn ecs_memory_mib(value: &str) -> Option<i32> {
    let value = value.trim();
    let lowered = value.to_ascii_lowercase();
    if let Some(count) = lowered.strip_suffix("gb") {
        let count: f64 = count.trim().parse().ok()?;
        return Some(ecs_rounded_units(count * 1024.0));
    }
    value.parse().ok()
}

/// A capacity in whole units, rounded to the nearest one and bounded by the modelled
/// `i32`, so no parsed or computed size can overflow or go negative.
pub fn ecs_rounded_units(value: f64) -> i32 {
    if !value.is_finite() {
        return i32::MAX;
    }
    value.round().clamp(0.0, f64::from(i32::MAX)) as i32
}

/// A capacity rounded UP to a whole unit: the smallest size that still holds `value`.
fn ecs_ceiled_units(value: f64) -> i32 {
    if !value.is_finite() {
        return i32::MAX;
    }
    value.ceil().clamp(0.0, f64::from(i32::MAX)) as i32
}

/// The Compute Optimizer `ECSServiceLaunchType` one service runs with, or `None` for a
/// launch type Compute Optimizer does not classify.
///
/// A service names either a `launchType` or a `capacityProviderStrategy`; the ECS
/// `FARGATE` / `FARGATE_SPOT` capacity providers are the ones that run a task on Fargate.
pub fn ecs_launch_type_of(config: &EcsServiceConfig) -> Option<&'static str> {
    if let Some(launch_type) = config.launch_type.as_deref() {
        return match launch_type {
            ECS_LAUNCH_TYPE_FARGATE => Some(ECS_SERVICE_LAUNCH_TYPE_FARGATE),
            ECS_LAUNCH_TYPE_EC2 => Some(ECS_SERVICE_LAUNCH_TYPE_EC2),
            _ => None,
        };
    }
    config
        .capacity_providers
        .iter()
        .any(|provider| ECS_FARGATE_CAPACITY_PROVIDERS.contains(&provider.as_str()))
        .then_some(ECS_SERVICE_LAUNCH_TYPE_FARGATE)
}

/// One series' observations over the look-back period, as the analysis reads them.
#[derive(Debug, Clone)]
pub struct EcsObserved {
    /// The largest observation in the window — the peak the sizing must hold.
    pub maximum: f64,
    /// The mean of the observations in the window.
    pub average: f64,
    /// The time stamp of the newest observation in the window.
    pub newest_secs: f64,
}

/// What the account published for ONE service's two analysed series over the look-back
/// window: `None` for a series it published nothing in that window, because an analysis
/// without observed utilization would be invented rather than computed.
#[derive(Debug, Clone)]
pub struct EcsObservedUtilization {
    /// The service's `CPUUtilization` observations.
    pub cpu: Option<EcsObserved>,
    /// The service's `MemoryUtilization` observations.
    pub memory: Option<EcsObserved>,
}

impl EcsObservedUtilization {
    /// The two series an analysis needs, or `None` when either was not observed.
    pub fn both(&self) -> Option<(&EcsObserved, &EcsObserved)> {
        self.cpu.as_ref().zip(self.memory.as_ref())
    }
}

/// What the account published for one service's analysed series over the `look_back_period`
/// ending at `now`.
///
/// The fold is CloudWatch's own ([`cloudwatch::state::BasicStatistics`]), the same one
/// `GetMetricStatistics` aggregates a period with, so the utilization this analysis reports
/// is the utilization CloudWatch itself would report for the window; the window's length is
/// the one [`look_back_period_in_days`] states, which is what every reader of this service
/// reports `lookbackPeriodInDays` from.
pub fn ecs_observed_utilization(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    reference: &EcsServiceRef,
    look_back_period: &str,
    now: f64,
) -> EcsObservedUtilization {
    let oldest = now - look_back_period_in_days(look_back_period) * SECONDS_PER_DAY;
    EcsObservedUtilization {
        cpu: ecs_observed_series(
            observations,
            reference,
            ECS_CPU_UTILIZATION_METRIC,
            oldest,
            now,
        ),
        memory: ecs_observed_series(
            observations,
            reference,
            ECS_MEMORY_UTILIZATION_METRIC,
            oldest,
            now,
        ),
    }
}

/// What the account published for ONE of a service's series over the window, or `None` when
/// it published nothing in it.
fn ecs_observed_series(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    reference: &EcsServiceRef,
    metric_name: &str,
    oldest: f64,
    now: f64,
) -> Option<EcsObserved> {
    let key = cloudwatch::state::MetricKey {
        namespace: ECS_METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: [
            (ECS_CLUSTER_DIMENSION.to_string(), reference.cluster.clone()),
            (ECS_SERVICE_DIMENSION.to_string(), reference.service.clone()),
        ]
        .into_iter()
        .collect(),
    };
    let data = observations.metrics.get(&key)?;
    // A datapoint whose aggregates are not finite describes no utilization, so it is left
    // out of the fold exactly as one outside the window is: a series with none left is a
    // series with nothing to analyse, not a service whose utilization folds to NaN.
    let window = data.iter().filter(|datum| {
        datum.timestamp_secs.is_finite()
            && datum.timestamp_secs >= oldest
            && datum.timestamp_secs <= now
            && datum.minimum.is_finite()
            && datum.maximum.is_finite()
            && datum.sum.is_finite()
            && datum.sample_count.is_finite()
    });
    let newest_secs = window
        .clone()
        .map(|datum| datum.timestamp_secs)
        .fold(f64::NEG_INFINITY, f64::max);
    let folded = cloudwatch::state::BasicStatistics::of(window);
    Some(EcsObserved {
        maximum: folded.named(METRIC_STATISTIC_MAXIMUM)?,
        average: folded.average()?,
        newest_secs: newest_secs.is_finite().then_some(newest_secs)?,
    })
}

/// The capacity one dimension of an ECS service needs: the absolute usage an observed peak
/// of `peak_percent` per cent of `current` stands for, carried with the crate's one
/// recommendation headroom ([`RECOMMENDATION_HEADROOM`], the service's documented
/// "`PERCENT_20` for headroom"), rounded up to a whole unit. The SAME arithmetic the EBS
/// sizing applies, so this service states one headroom rule.
pub fn ecs_required_capacity(current: i32, peak_percent: f64) -> i32 {
    let used = f64::from(current) * peak_percent.max(0.0) / 100.0;
    ecs_ceiled_units(used * (1.0 + RECOMMENDATION_HEADROOM))
}

/// The Fargate task size an ECS service's observed peaks are sized to: the smallest size
/// the catalog offers that holds what BOTH dimensions need ([`ecs_required_capacity`]).
///
/// The ONE recommendation option an ECS service recommendation carries, so the option
/// `GetECSServiceRecommendations` reports and the option
/// `GetECSServiceRecommendationProjectedMetrics` projects onto are the same task size.
pub fn ecs_recommended_task_size(
    current_cpu: i32,
    current_memory: i32,
    cpu_peak_percent: f64,
    memory_peak_percent: f64,
) -> (i32, i32) {
    ecs_smallest_fargate_size(
        ecs_required_capacity(current_cpu, cpu_peak_percent),
        ecs_required_capacity(current_memory, memory_peak_percent),
    )
}

/// The smallest Fargate task size that holds `cpu` CPU units and `memory` MiB, from
/// [`FARGATE_TASK_SIZES`]; the largest size in the catalog when nothing in it is big
/// enough, since there is no larger task to recommend.
///
/// CPU is the PRIMARY key: the catalog is walked in CPU order and the first row that can
/// hold the memory requirement wins, so the result is the smallest CPU size that fits —
/// which is the size a memory requirement forces up, exactly as the catalog's own
/// "Available `cpu` values" wording reads (a task memory is offered BY cpu size). A later
/// reader should not reorder this to minimise some combined total: the two dimensions are
/// priced and offered per CPU row, not as a sum.
pub fn ecs_smallest_fargate_size(cpu: i32, memory: i32) -> (i32, i32) {
    for (size_cpu, memories) in FARGATE_TASK_SIZES {
        if *size_cpu < cpu {
            continue;
        }
        if let Some(size_memory) = memories.iter().find(|candidate| **candidate >= memory) {
            return (*size_cpu, *size_memory);
        }
    }
    // Past the catalog's largest size there is no bigger Fargate task to recommend, so the
    // largest one IS the recommendation: the service is still analysable (its utilization
    // was observed and its finding stays `Underprovisioned`, which is exactly the
    // "performance needs are not met" the finding reports), so this is not the
    // unsupported-resource case — that answers a service the analysis has no INPUT for, not
    // one whose demand exceeds what Fargate offers.
    let (largest_cpu, largest_memories) = FARGATE_TASK_SIZES
        .last()
        .expect("the Fargate task size catalog is not empty");
    (
        *largest_cpu,
        *largest_memories
            .last()
            .expect("every Fargate CPU size admits at least one task memory"),
    )
}

/// The smallest task CPU size in [`FARGATE_TASK_SIZES`] that holds `cpu` CPU units, or the
/// catalog's largest when none does. The per-DIMENSION floor an over-provisioned reason
/// code is judged against, so a CPU requirement that fits a smaller size is reported as
/// sizeable-down even when the memory requirement forces the recommended task up.
pub fn ecs_smallest_fargate_cpu(cpu: i32) -> i32 {
    FARGATE_TASK_SIZES
        .iter()
        .map(|(size_cpu, _)| *size_cpu)
        .find(|size_cpu| *size_cpu >= cpu)
        .unwrap_or_else(|| {
            FARGATE_TASK_SIZES
                .last()
                .map_or(cpu, |(largest_cpu, _)| *largest_cpu)
        })
}

/// The smallest task memory [`FARGATE_TASK_SIZES`] offers at ANY CPU size that holds
/// `memory` MiB, or the catalog's largest when none does. The memory counterpart of
/// [`ecs_smallest_fargate_cpu`].
pub fn ecs_smallest_fargate_memory(memory: i32) -> i32 {
    let offered = FARGATE_TASK_SIZES
        .iter()
        .flat_map(|(_, memories)| memories.iter().copied());
    offered
        .clone()
        .filter(|size_memory| *size_memory >= memory)
        .min()
        .unwrap_or_else(|| offered.max().unwrap_or(memory))
}

/// The utilization `observed` per cent of `current` capacity becomes at `option` capacity:
/// the same absolute usage over the new size.
pub fn ecs_projected_metric_value(observed: f64, current: i32, option: i32) -> f64 {
    if option <= 0 {
        return 0.0;
    }
    observed * f64::from(current) / f64::from(option)
}

/// The utilization an ECS service's observed statistics become at the recommended task
/// size, as `ECSServiceRecommendationOption$projectedUtilizationMetrics` reports them:
/// each analysed metric under each statistic the analysis observed it with, projected by
/// the ONE rule [`ecs_projected_metric_value`] states.
///
/// Stated once so the projection an option carries and the series
/// `GetECSServiceRecommendationProjectedMetrics` reports for that option cannot disagree.
/// The projection is a single value computed from the observation, so both reported bounds
/// are that value — a wider interval would claim a spread the observation does not carry.
pub fn ecs_projected_utilization_metrics(
    cpu: &EcsObserved,
    memory: &EcsObserved,
    current: (i32, i32),
    option: (i32, i32),
) -> Vec<ECSServiceProjectedUtilizationMetric> {
    // The metric names are the `ECSServiceMetricName` values ("`Cpu` — The amount of CPU
    // capacity that's used in the service", and the same for `Memory`), which this service's
    // one metric-name vocabulary ([`METRIC_CPU`], [`METRIC_MEMORY`]) already spells; each is
    // projected on the sizes of the dimension it measures.
    [
        (
            METRIC_CPU,
            METRIC_STATISTIC_MAXIMUM,
            cpu.maximum,
            current.0,
            option.0,
        ),
        (
            METRIC_CPU,
            METRIC_STATISTIC_AVERAGE,
            cpu.average,
            current.0,
            option.0,
        ),
        (
            METRIC_MEMORY,
            METRIC_STATISTIC_MAXIMUM,
            memory.maximum,
            current.1,
            option.1,
        ),
        (
            METRIC_MEMORY,
            METRIC_STATISTIC_AVERAGE,
            memory.average,
            current.1,
            option.1,
        ),
    ]
    .into_iter()
    .map(|(name, statistic, observed, current, option)| {
        let value = ecs_projected_metric_value(observed, current, option);
        ECSServiceProjectedUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(statistic.to_string()),
            lower_bound_value: value,
            upper_bound_value: value,
        }
    })
    .collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComputeoptimizerComputeoptimizerserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComputeoptimizerComputeoptimizerserviceState>)
}

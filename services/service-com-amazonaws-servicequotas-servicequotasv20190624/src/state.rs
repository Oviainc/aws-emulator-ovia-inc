// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicequotas-servicequotasv20190624 service.
//!
//! Service Quotas serves three kinds of data, and this module is the single owner of
//! all three so every operation reads the same copy:
//!
//! * the **service catalog** (`ListServices`) and the **AWS default quotas**
//!   (`GetAWSDefaultServiceQuota`, `ListAWSDefaultServiceQuotas`) — seeded in
//!   [`Default`] and never mutated, which is what makes a default value immutable by
//!   construction rather than by convention;
//! * the **applied quota overlay** (`GetServiceQuota`, `ListServiceQuotas`) — written
//!   only when a quota increase request is decided, and read with the default as the
//!   fallback;
//! * the **quota increase requests** (`RequestServiceQuotaIncrease`,
//!   `GetRequestedServiceQuotaChange`,
//!   `ListRequestedServiceQuotaChangeHistoryByQuota`) — append-only, with an
//!   insertion-ordered index so a history listing is newest-first and a pagination
//!   token keeps meaning the same position;
//! * the **tags on an applied quota** (`TagResource`, `ListTagsForResource`,
//!   `UntagResource`) — keyed by the quota's own ARN, the only thing a tagging
//!   operation names its resource by, so one quota has exactly one tag set however
//!   it is reached.
//!
//! Beside them it owns the account's **Automatic Management configuration**
//! (`StartAutoManagement` / `UpdateAutoManagement` / `StopAutoManagement` write it,
//! `GetAutoManagementConfiguration` reports it). That one is ACCOUNT scoped — its
//! only `OptInLevel` is `ACCOUNT` — so it is stored once for the account and reads
//! the same at every regional endpoint.
//!
//! The overlay and the requests are per region (Service Quotas is a regional service:
//! an increase applied in one region does not change another's value), while the
//! catalog and the defaults are the same everywhere. The account is the emulator's
//! single [`engine::emulator_account_id`].
//!
//! The quota rows in [`seed_quota_catalog`] are a CATALOG rather than a sample: for
//! a service such as EC2 the list runs to hundreds of entries, the way AWS's
//! published list does. That is what the enumerations are: `ListServices` reports the
//! whole integrated service list and `ListAWSDefaultServiceQuotas` /
//! `ListServiceQuotas` report a service's whole quota list, so with a page size the
//! model allows (`MaxResults` 1..100) they genuinely run to many
//! `NextToken`-chained pages.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use base64::Engine as _;

use crate::ops::types::{
    MetricInfo, QuotaInfo, QuotaPeriod, RequestedServiceQuotaChange, ServiceInfo, ServiceQuota,
    Tag,
};

/// The region a request carries no SigV4 credential scope to name, and the one a
/// scope naming a region this emulator does not know is served as.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The regions this emulator keeps state for.
///
/// Source: AWS's published region codes (the `Region` column of the AWS Regional
/// Services List, which is also what `aws ec2 describe-regions` reports), plus the
/// GovCloud and China partitions.
///
/// This list is what BOUNDS the per-region state: the region comes out of the
/// caller's own SigV4 credential scope, so a request naming anything not on this
/// list is served as [`DEFAULT_REGION`] (see [`request_region`]) rather than minting
/// a state bucket for a string a caller chose.
pub const KNOWN_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "cn-north-1",
    "cn-northwest-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-gov-east-1",
    "us-gov-west-1",
    "us-west-1",
    "us-west-2",
];

/// The ARN service segment: `awsService.arnNamespace` of this service's model
/// (`smithy_query(query="service_info")` reports `arnNamespace: servicequotas`).
pub const ARN_NAMESPACE: &str = "servicequotas";

/// `AppliedLevelEnum` / `AdjustableAtLevelEnum` member `ACCOUNT`.
pub const LEVEL_ACCOUNT: &str = "ACCOUNT";
/// `AppliedLevelEnum` / `AdjustableAtLevelEnum` member `ALL`.
pub const LEVEL_ALL: &str = "ALL";

/// `OptInStatus` member `ENABLED`: Automatic Management is started.
pub const OPT_IN_STATUS_ENABLED: &str = "ENABLED";
/// `OptInStatus` member `DISABLED`: Automatic Management is stopped — and what an
/// account that has never started it reports.
pub const OPT_IN_STATUS_DISABLED: &str = "DISABLED";

/// `RequestStatus` member `PENDING`: the status a newly minted request carries.
pub const STATUS_PENDING: &str = "PENDING";
/// `RequestStatus` member `APPROVED`: the status a decided request carries once the
/// increase has been applied to the quota.
pub const STATUS_APPROVED: &str = "APPROVED";
/// `RequestStatus` member `NOT_APPROVED`: the terminal status of a request whose
/// submitter opted out of the support case that reviews it.
///
/// Source: `RequestServiceQuotaIncreaseRequest$SupportCaseAllowed`'s own documentation
/// (`smithy_query(query="doc", shape="RequestServiceQuotaIncreaseRequest",
/// member="SupportCaseAllowed")`) — "If you set the flag to False, Amazon Web Services
/// does not open a support case and updates the request status to Not approved."
pub const STATUS_NOT_APPROVED: &str = "NOT_APPROVED";

/// What `SupportCaseAllowed` means when a request does not carry it: the model's own
/// default, "By default, this flag is set to True".
pub const SUPPORT_CASE_ALLOWED_BY_DEFAULT: bool = true;

/// The one Region the quota request TEMPLATE plane is served from.
///
/// Source: AWS's Service Quotas user guide, "Using quota request templates" — the
/// template operations are called in the US East (N. Virginia) Region, and a call made
/// anywhere else is refused with the modelled `TemplatesNotAvailableInRegionException`
/// ("The Service Quotas template is not available in this Amazon Web Services
/// Region."). It is the same string as [`DEFAULT_REGION`] but a different fact, so it
/// is named separately: this one is the template plane's home, that one is the region a
/// request with no usable credential scope is served as.
pub const TEMPLATE_HOME_REGION: &str = "us-east-1";

/// How long a quota increase request stays `PENDING` before it is decided.
///
/// CHOSEN, not an AWS-documented duration: a real increase is decided by Service
/// Quotas' own review and publishes no deadline. It is kept inside the 250-750 ms
/// band this project fixes for a transition the clock owes, so a caller that reads
/// the request straight after making it still observes `PENDING`, and nothing ever
/// sleeps: the due instant is stored on the request and materialised on the next
/// read that is already past it.
pub const REQUEST_DECISION_DELAY: Duration = Duration::from_millis(500);

/// The largest page size a CALLER may ask for: the maximum the model's `MaxResults`
/// range declares (1..100). It bounds `MaxResults`, NOT the unpaginated listing — a
/// caller that names no `MaxResults` gets the whole listing in one page (see
/// [`paginate`]).
pub const MAX_PAGE_SIZE: usize = 100;

/// The most quota increase requests this emulator stores per region.
///
/// The request store is append-only — a history listing must keep reporting the
/// requests it has already reported — so it needs a ceiling. Past it a further
/// request is refused with the `QuotaExceededException` (HTTP 409) the model declares
/// on `RequestServiceQuotaIncrease`, instead of growing the store without bound.
/// The number is CHOSEN (AWS publishes no limit on stored quota requests); the
/// regions it is charged against are bounded by [`KNOWN_REGIONS`].
pub const MAX_REQUESTS_PER_REGION: usize = 1_000;

/// The window a quota whose usage CloudWatch publishes as an `AWS/Usage`
/// `ResourceCount` metric reports as its `Period`: that metric is published every
/// five minutes, which is the period the recorded response for a measured quota
/// carries.
const RESOURCE_COUNT_PERIOD_MINUTES: i32 = 5;

/// The most tags one applied quota can carry.
///
/// Source: the "Tag restrictions" section of the Service Quotas User Guide — the page
/// the model's own `TooManyTagsException` links to — which publishes a maximum of 50
/// tags per resource. It is well inside the model's `OutputTags` bound (len 0..200),
/// so `ListTagsForResource` can always report the whole set.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix AWS reserves for its own tags.
///
/// Source: AWS's tagging restrictions (shared by every taggable service and repeated
/// in the Service Quotas User Guide's tag restrictions): the `aws:` prefix is
/// reserved for AWS use, and a caller may neither create nor edit a tag whose key
/// carries it. A key in this namespace is what this service's
/// `TagPolicyViolationException` ("the specified tag is a reserved word and cannot be
/// used") refuses.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// One AWS-default quota, as `GetAWSDefaultServiceQuota` reports it.
///
/// Seeded from AWS's published Service Quotas defaults (the per-service "Service
/// quotas" pages) and never mutated: an increase writes [`AppliedQuota`] instead, so
/// the default a caller reads after an increase is the same one it read before.
#[derive(Debug, Clone)]
pub struct DefaultQuota {
    /// The `ServiceCode` the quota belongs to.
    pub service_code: String,
    /// The `QuotaCode` that identifies the quota within its service.
    pub quota_code: String,
    /// `QuotaName`, as AWS spells it.
    pub quota_name: String,
    /// The AWS default `Value`.
    pub value: f64,
    /// `Unit` — `None` for a countable quota, otherwise the unit AWS reports.
    pub unit: String,
    /// `Adjustable`: whether an increase can be requested at all.
    pub adjustable: bool,
    /// `GlobalQuota`: the quota counts account-wide rather than per region.
    pub global_quota: bool,
    /// `AdjustableAtLevelEnum`: the level an increase may name. `ACCOUNT` means a
    /// request may not carry a `ContextId`, because there is no resource context to
    /// adjust the quota for.
    pub adjustable_at_level: String,
    /// `UsageMetric`, for the quotas AWS publishes a CloudWatch usage metric for.
    pub usage_metric: Option<MetricInfo>,
    /// `Period`: the window the quota's value is measured over — the rate denominator
    /// of a rate quota, or the publication interval of its usage metric.
    pub period: Option<QuotaPeriod>,
    /// `Description`: the quota's published description.
    pub description: Option<String>,
}

impl DefaultQuota {
    /// The row of [`seed_quota_catalog`] for `service_code`, with the `QuotaCode`
    /// AWS's catalog gives it.
    fn from_row(service_code: &str, quota_code: String, row: QuotaRow) -> Self {
        Self {
            service_code: service_code.to_string(),
            quota_code,
            quota_name: row.name,
            value: row.value,
            unit: row.unit,
            adjustable: row.adjustable,
            global_quota: row.global_quota,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: row.usage_metric,
            period: row.period,
            description: row.description,
        }
    }
}

/// The applied value of one quota: the overlay a decided increase writes over the
/// AWS default.
#[derive(Debug, Clone)]
pub struct AppliedQuota {
    /// The value now in force.
    pub value: f64,
    /// `QuotaAppliedAtLevel` — the level the value was applied at.
    pub quota_applied_at_level: String,
}

/// The account's Service Quotas Automatic Management configuration.
///
/// One record per account, not per region: the only `OptInLevel` the model declares
/// is `ACCOUNT`.
#[derive(Debug, Clone)]
pub struct AutoManagementConfig {
    /// `OptInLevel` — an `OptInLevel` member, as the opt-in named it.
    pub opt_in_level: String,
    /// `OptInType` — an `OptInType` member, as the opt-in named it.
    pub opt_in_type: String,
    /// `OptInStatus` — [`OPT_IN_STATUS_ENABLED`] while Automatic Management is
    /// started, [`OPT_IN_STATUS_DISABLED`] once it is stopped. Stopping keeps the
    /// record, so the preferences a later start would otherwise have to be given
    /// again are still reported.
    pub opt_in_status: String,
    /// `NotificationArn` — the User Notifications ARN, when one was given.
    pub notification_arn: Option<String>,
    /// `ExclusionList`: `ExcludedService` -> the excluded `QuotaCode`s. Stored as the
    /// codes the caller named; `GetAutoManagementConfiguration` reports them as
    /// `QuotaInfo` entries, pairing each code with the catalog's `QuotaName`.
    pub exclusion_list: BTreeMap<String, Vec<String>>,
}

/// One stored quota increase request.
#[derive(Debug, Clone)]
pub struct QuotaRequest {
    /// `Id` — minted here and stored; every later read answers from this record.
    pub id: String,
    /// The region the request was made in, so its `QuotaArn` is stable across reads.
    pub region: String,
    pub service_code: String,
    pub quota_code: String,
    pub desired_value: f64,
    /// A `RequestStatus` member.
    pub status: String,
    pub created: SystemTime,
    pub last_updated: SystemTime,
    /// When the request becomes due for a decision. Stored, never slept on.
    pub decided_at: SystemTime,
    /// `Requester` — the identity that made the request.
    pub requester: String,
    /// `CaseId`: `None` unless a support case was opened for the request.
    pub case_id: Option<String>,
    /// `SupportCaseAllowed`, as the request carried it (or
    /// [`SUPPORT_CASE_ALLOWED_BY_DEFAULT`] when it carried nothing). `false` is the
    /// submitter opting out of the support case that reviews an increase, which is what
    /// decides the request's terminal status.
    pub support_case_allowed: bool,
    /// `QuotaRequestedAtLevel` — an `AppliedLevelEnum` member.
    pub quota_requested_at_level: String,
}

/// Why a `ResourceARN` handed to a tagging operation names no applied quota.
///
/// The two cases are different refusals in the model: input this service cannot read
/// as one of its own ARNs at all, and a readable ARN whose quota this account does
/// not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnError {
    /// Not an applied-quota ARN: the wrong number of segments, a different service's
    /// `arnNamespace`, or a resource part that is not `<service code>/<quota code>`.
    /// The model's `IllegalArgumentException` ("Invalid input was provided").
    Malformed,
    /// A readable applied-quota ARN, but no quota of this account answers to it —
    /// another account, a region this emulator does not serve, or a
    /// `(ServiceCode, QuotaCode)` pair the catalog does not hold. The model's
    /// `NoSuchResourceException` ("The specified resource does not exist").
    NoSuchQuota,
}

/// A `TagResource` the tag ceiling refuses: the resource would have ended up holding
/// [`TagCeilingExceeded::would_hold`] tags, past [`MAX_TAGS_PER_RESOURCE`]. The
/// model's `TooManyTagsException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagCeilingExceeded {
    /// The number of tags the request would have left on the resource.
    pub would_hold: usize,
}

/// The mutable, per-region half of this service's state.
#[derive(Debug, Default)]
struct RegionState {
    /// Keyed by `(service code, quota code)`; written only by a decided request.
    applied_quotas: BTreeMap<(String, String), AppliedQuota>,
    quota_requests: BTreeMap<String, QuotaRequest>,
    /// Request ids in the order they were minted: the index a history listing walks
    /// backwards and a pagination token indexes into.
    request_order: Vec<String>,
    /// How many requests this region has minted; part of the id derivation, so two
    /// requests for the same quota in the same nanosecond still get distinct ids.
    minted: u64,
}

/// Per-service state for com-amazonaws-servicequotas-servicequotasv20190624. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsServicequotasServicequotasv20190624State {
    /// `ServiceCode` -> `ServiceName`, in `ServiceCode` order.
    services: BTreeMap<String, String>,
    /// The AWS default quotas, in the order Service Quotas PUBLISHES them for each
    /// service: both listings report a service's quotas in this order, so the order
    /// is part of the state rather than an artefact of how the rows are keyed.
    /// Never mutated.
    default_quotas: Vec<DefaultQuota>,
    /// The applied overlay of the quotas AWS counts ACCOUNT-WIDE (`GlobalQuota`).
    /// A global quota has one value for the whole account — which is why its ARN
    /// carries no region — so an approved increase on it is in force at every
    /// regional endpoint, not just the one it was requested at.
    global_applied_quotas: BTreeMap<(String, String), AppliedQuota>,
    /// The mutable state, per region.
    regions: BTreeMap<String, RegionState>,
    /// The account's Automatic Management configuration: `None` until the account
    /// opts in for the first time.
    auto_management: Option<AutoManagementConfig>,
    /// The tags on an applied quota: the quota's CANONICAL ARN (what
    /// [`quota_arn`] builds, so a global quota's regionless one) -> `TagKey` ->
    /// `TagValue`, in tag-key order.
    ///
    /// Keyed by the ARN rather than per region because the ARN is what a tagging
    /// operation names its resource by and already carries the region (or, for a
    /// global quota, deliberately carries none) — so the same quota has one tag set
    /// whichever regional endpoint reaches it, and a tag map is never minted for a
    /// quota that does not exist.
    quota_tags: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for ComAmazonawsServicequotasServicequotasv20190624State {
    fn default() -> Self {
        Self {
            services: seed_services(),
            default_quotas: seed_default_quotas(),
            global_applied_quotas: BTreeMap::new(),
            regions: BTreeMap::new(),
            auto_management: None,
            quota_tags: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsServicequotasServicequotasv20190624State {
    /// The catalog, as `ListServices` reports it, in `ServiceCode` order.
    pub fn services(&self) -> Vec<ServiceInfo> {
        self.services
            .iter()
            .map(|(code, name)| ServiceInfo {
                service_code: Some(code.clone()),
                service_name: Some(name.clone()),
            })
            .collect()
    }

    /// The account's Automatic Management configuration as stored, or `None` while
    /// the account has never opted in.
    pub fn auto_management(&self) -> Option<&AutoManagementConfig> {
        self.auto_management.as_ref()
    }

    /// Record the account's Automatic Management configuration: what an opt-in
    /// (`StartAutoManagement`), a change to it (`UpdateAutoManagement`) and an opt-out
    /// (`StopAutoManagement`, which only moves `OptInStatus` to
    /// [`OPT_IN_STATUS_DISABLED`]) write.
    pub fn set_auto_management(&mut self, config: AutoManagementConfig) {
        self.auto_management = Some(config);
    }

    /// The stored exclusion list as `GetAutoManagementConfiguration` reports it:
    /// every excluded `QuotaCode`, in stored order, paired with the `QuotaName` the
    /// quota catalog holds for it.
    ///
    /// A code the catalog does not know is still reported, with no `QuotaName`: the
    /// exclusion is the caller's own, so dropping it would under-report the
    /// configuration. Empty when the account has no configuration stored.
    pub fn auto_management_exclusion_view(&self) -> HashMap<String, Vec<QuotaInfo>> {
        let Some(config) = self.auto_management.as_ref() else {
            return HashMap::new();
        };
        config
            .exclusion_list
            .iter()
            .map(|(service_code, quota_codes)| {
                let quotas = quota_codes
                    .iter()
                    .map(|quota_code| QuotaInfo {
                        quota_code: Some(quota_code.clone()),
                        quota_name: self
                            .default_quota(service_code, quota_code)
                            .map(|quota| quota.quota_name.clone()),
                    })
                    .collect();
                (service_code.clone(), quotas)
            })
            .collect()
    }

    /// Whether the catalog knows `service_code`. Codes are matched exactly: AWS's
    /// service codes are lowercase and `VPC` is not `vpc`.
    pub fn knows_service(&self, service_code: &str) -> bool {
        self.services.contains_key(service_code)
    }

    /// The `ServiceName` for a `ServiceCode`.
    pub fn service_name(&self, service_code: &str) -> Option<&str> {
        self.services.get(service_code).map(String::as_str)
    }

    /// The AWS default quota for a `(ServiceCode, QuotaCode)` pair — the row for
    /// EXACTLY that pair, so a read never answers with another quota's record.
    pub fn default_quota(&self, service_code: &str, quota_code: &str) -> Option<&DefaultQuota> {
        self.default_quotas
            .iter()
            .find(|quota| quota.service_code == service_code && quota.quota_code == quota_code)
    }

    /// Every `QuotaCode` the service has a default quota for, in the order the
    /// catalog publishes them.
    pub fn quota_codes(&self, service_code: &str) -> Vec<String> {
        self.default_quotas
            .iter()
            .filter(|quota| quota.service_code == service_code)
            .map(|quota| quota.quota_code.clone())
            .collect()
    }

    /// The value in force for a quota: the applied overlay when a decided request
    /// wrote one, else the AWS default.
    pub fn applied_value(&self, region: &str, service_code: &str, quota_code: &str) -> Option<f64> {
        let default = self.default_quota(service_code, quota_code)?;
        Some(
            self.applied_entry(region, service_code, quota_code)
                .map_or(default.value, |applied| applied.value),
        )
    }

    /// The quota as `GetServiceQuota` / `ListServiceQuotas` report it: the AWS
    /// default with the applied overlay laid over its `Value` and
    /// `QuotaAppliedAtLevel`.
    pub fn applied_quota_view(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Option<ServiceQuota> {
        let default = self.default_quota(service_code, quota_code)?;
        let applied = self.applied_entry(region, service_code, quota_code);
        let mut quota = self.quota_view(region, default, true);
        quota.value = Some(applied.map_or(default.value, |entry| entry.value));
        quota.quota_applied_at_level = Some(applied.map_or_else(
            || LEVEL_ACCOUNT.to_string(),
            |entry| entry.quota_applied_at_level.clone(),
        ));
        Some(quota)
    }

    /// The quota as `GetAWSDefaultServiceQuota` / `ListAWSDefaultServiceQuotas`
    /// report it: the AWS default, with no applied level (nothing has been applied)
    /// and the ACCOUNT-LESS ARN an AWS default quota carries (see
    /// [`default_quota_arn`]).
    pub fn default_quota_view(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Option<ServiceQuota> {
        let default = self.default_quota(service_code, quota_code)?;
        Some(self.quota_view(region, default, false))
    }

    /// Mint and store a quota increase request, and return the stored record —
    /// `None` when this region already holds [`MAX_REQUESTS_PER_REGION`] of them.
    ///
    /// The caller has already checked that the quota exists, that it is adjustable,
    /// and that `desired_value` is above the value in force. The ceiling is enforced
    /// HERE, at the one insertion point, so no caller can grow the store past it.
    ///
    /// `support_case_allowed` is recorded, not acted on: it is what
    /// [`Self::settle_due_requests`] reads when the decision comes due.
    #[allow(clippy::too_many_arguments)]
    pub fn record_request(
        &mut self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        desired_value: f64,
        support_case_allowed: bool,
        requester: String,
        now: SystemTime,
    ) -> Option<QuotaRequest> {
        if self.stored_request_count(region) >= MAX_REQUESTS_PER_REGION {
            return None;
        }
        let id = {
            let state = self.regions.entry(region.to_string()).or_default();
            state.minted += 1;
            request_id(region, service_code, quota_code, state.minted, now)
        };
        let request = QuotaRequest {
            id: id.clone(),
            region: region.to_string(),
            service_code: service_code.to_string(),
            quota_code: quota_code.to_string(),
            desired_value,
            status: STATUS_PENDING.to_string(),
            created: now,
            last_updated: now,
            decided_at: now + REQUEST_DECISION_DELAY,
            requester,
            case_id: None,
            support_case_allowed,
            quota_requested_at_level: LEVEL_ACCOUNT.to_string(),
        };
        let state = self.regions.entry(region.to_string()).or_default();
        state.quota_requests.insert(id.clone(), request.clone());
        state.request_order.push(id);
        Some(request)
    }

    /// How many quota increase requests this region holds, against
    /// [`MAX_REQUESTS_PER_REGION`].
    pub fn stored_request_count(&self, region: &str) -> usize {
        self.regions
            .get(region)
            .map_or(0, |state| state.request_order.len())
    }

    /// Materialise every decision the clock has already made due in `region`.
    ///
    /// A `PENDING` request whose `decided_at` has passed is decided by what its
    /// submitter asked for:
    ///
    /// * `SupportCaseAllowed` (the default) — the increase is `APPROVED` and its
    ///   desired value becomes the quota's applied value;
    /// * `SupportCaseAllowed = false` — the submitter opted out of the support case
    ///   that reviews an increase, so AWS opens none and the request ends
    ///   `NOT_APPROVED`: no `CaseId`, and NOTHING is put in force, so the quota keeps
    ///   the value it had.
    ///
    /// This is the ONLY writer of the applied overlay, and the decision is owed at
    /// `decided_at` whether or not anybody calls — so a reporting operation may settle
    /// it before answering, and reading twice yields the same answer.
    ///
    /// The overlay goes where the quota's own scope says: account-wide for a
    /// `GlobalQuota`, this region for a regional one.
    pub fn settle_due_requests(&mut self, region: &str, now: SystemTime) {
        let Some(state) = self.regions.get(region) else {
            return;
        };
        let due: Vec<String> = state
            .request_order
            .iter()
            .filter(|id| {
                state
                    .quota_requests
                    .get(*id)
                    .is_some_and(|request| request.status == STATUS_PENDING && request.decided_at <= now)
            })
            .cloned()
            .collect();
        if due.is_empty() {
            return;
        }

        // The decision: flip the stored requests, and note the overlay each APPROVED
        // one puts in force. A request that opted out of its support case settles to
        // NOT_APPROVED and contributes no overlay.
        let mut decided: Vec<(String, String, AppliedQuota)> = Vec::new();
        if let Some(state) = self.regions.get_mut(region) {
            for id in due {
                let Some(request) = state.quota_requests.get_mut(&id) else {
                    continue;
                };
                // The decision happened when it came due, not when somebody looked.
                request.last_updated = request.decided_at;
                if !request.support_case_allowed {
                    request.status = STATUS_NOT_APPROVED.to_string();
                    continue;
                }
                request.status = STATUS_APPROVED.to_string();
                decided.push((
                    request.service_code.clone(),
                    request.quota_code.clone(),
                    AppliedQuota {
                        value: request.desired_value,
                        quota_applied_at_level: request.quota_requested_at_level.clone(),
                    },
                ));
            }
        }

        for (service_code, quota_code, applied) in decided {
            let key = (service_code, quota_code);
            if self
                .default_quota(&key.0, &key.1)
                .is_some_and(|quota| quota.global_quota)
            {
                self.global_applied_quotas.insert(key, applied);
            } else if let Some(state) = self.regions.get_mut(region) {
                state.applied_quotas.insert(key, applied);
            }
        }
    }

    /// One stored request by `Id`, within the caller's region.
    pub fn request(&self, region: &str, id: &str) -> Option<&QuotaRequest> {
        self.regions.get(region)?.quota_requests.get(id)
    }

    /// Every stored request for one quota, NEWEST FIRST.
    pub fn requests_for_quota(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Vec<QuotaRequest> {
        let Some(state) = self.regions.get(region) else {
            return Vec::new();
        };
        state
            .request_order
            .iter()
            .rev()
            .filter_map(|id| state.quota_requests.get(id))
            .filter(|request| request.service_code == service_code && request.quota_code == quota_code)
            .cloned()
            .collect()
    }

    /// A stored request as `RequestedServiceQuotaChange` on the wire.
    pub fn request_view(&self, request: &QuotaRequest) -> RequestedServiceQuotaChange {
        let default = self.default_quota(&request.service_code, &request.quota_code);
        RequestedServiceQuotaChange {
            id: Some(request.id.clone()),
            // `RequestType` has one member, `AutomaticManagement`, which marks a
            // request the Auto Management feature raised; a caller's own request
            // carries none.
            request_type: None,
            case_id: request.case_id.clone(),
            service_code: Some(request.service_code.clone()),
            service_name: self.service_name(&request.service_code).map(str::to_string),
            quota_code: Some(request.quota_code.clone()),
            quota_name: default.map(|quota| quota.quota_name.clone()),
            desired_value: Some(request.desired_value),
            status: Some(request.status.clone()),
            created: Some(epoch_timestamp(request.created)),
            last_updated: Some(epoch_timestamp(request.last_updated)),
            requester: Some(request.requester.clone()),
            quota_arn: Some(quota_arn(
                &request.region,
                &request.service_code,
                &request.quota_code,
                default.is_some_and(|quota| quota.global_quota),
            )),
            global_quota: default.is_some_and(|quota| quota.global_quota),
            unit: default.map(|quota| quota.unit.clone()),
            quota_requested_at_level: Some(request.quota_requested_at_level.clone()),
            // Only a quota that is adjustable per resource carries a context.
            quota_context: None,
        }
    }

    /// The applied quota a tagging operation's `ResourceARN` names, as the ARN this
    /// service reports for it — or why it names none.
    ///
    /// An applied quota's ARN is
    /// `arn:<partition>:servicequotas:<region>:<account>:<service code>/<quota code>`,
    /// and [`quota_arn`] is the one form of it this emulator reports (through
    /// `GetServiceQuota`, `ListServiceQuotas` and every quota increase request):
    /// one partition, one account, and no region for a `GlobalQuota`, which counts
    /// account-wide. The rule here is therefore ONE rule, applied uniformly: the
    /// string must be the ARN this service reports for an existing quota. That is
    /// what makes one quota carry exactly one tag set — there is no second spelling
    /// of a quota's ARN to tag it under.
    ///
    /// `Malformed` is the shape failing: not an ARN, a partition outside the form the
    /// modelled `AmazonResourceName` declares (`^arn:aws(-[\w]+)*:…`), another
    /// service's `arnNamespace`, or a resource part that is not
    /// `<service code>/<quota code>`. `NoSuchQuota` is the shape holding but the
    /// resource not existing here: another partition or account, a region outside
    /// [`KNOWN_REGIONS`] (checked for the same reason [`request_region`] checks the
    /// credential scope — the string is caller-supplied and a region this emulator
    /// does not serve holds no quotas), or a `(ServiceCode, QuotaCode)` pair the
    /// catalog does not hold.
    pub fn resolve_quota_arn(&self, resource_arn: &str) -> Result<String, ResourceArnError> {
        let segments: Vec<&str> = resource_arn.splitn(6, ':').collect();
        let [prefix, partition, namespace, region, _account, resource] = segments[..] else {
            return Err(ResourceArnError::Malformed);
        };
        if prefix != "arn" || !is_aws_partition(partition) || namespace != ARN_NAMESPACE {
            return Err(ResourceArnError::Malformed);
        }
        let Some((service_code, quota_code)) = resource.split_once('/') else {
            return Err(ResourceArnError::Malformed);
        };
        if service_code.is_empty() || quota_code.is_empty() || quota_code.contains('/') {
            return Err(ResourceArnError::Malformed);
        }
        let Some(default) = self.default_quota(service_code, quota_code) else {
            return Err(ResourceArnError::NoSuchQuota);
        };
        // A region this emulator does not serve is checked before the ARN is rebuilt,
        // because rebuilding would otherwise echo the caller's own string back.
        if !default.global_quota && !KNOWN_REGIONS.contains(&region) {
            return Err(ResourceArnError::NoSuchQuota);
        }
        // Comparing against the reported ARN is what checks the partition, the account
        // and (for a global quota, which carries none) the region in one place.
        let reported = quota_arn(region, service_code, quota_code, default.global_quota);
        if reported == resource_arn {
            Ok(reported)
        } else {
            Err(ResourceArnError::NoSuchQuota)
        }
    }

    /// The tags on the applied quota `canonical_arn` carries, in `TagKey` order —
    /// empty when nothing has tagged it.
    ///
    /// `canonical_arn` is what [`Self::resolve_quota_arn`] returned, so a quota that
    /// does not exist never reaches here.
    pub fn resource_tags(&self, canonical_arn: &str) -> Vec<Tag> {
        self.quota_tags
            .get(canonical_arn)
            .into_iter()
            .flatten()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Add `tags` to the applied quota `canonical_arn`, replacing the value of a key
    /// it already carries.
    ///
    /// A tag set is keyed by `TagKey`, so tagging is an upsert and a repeated key
    /// leaves one tag, not two — which is why the ceiling is charged against the
    /// DISTINCT keys the request would add. Nothing is written when it would be
    /// exceeded, so a refused call leaves exactly the set it found, and an untagged
    /// quota keeps no entry.
    pub fn put_resource_tags(
        &mut self,
        canonical_arn: &str,
        tags: &[Tag],
    ) -> Result<(), TagCeilingExceeded> {
        let existing = self.quota_tags.get(canonical_arn);
        let held = existing.map_or(0, BTreeMap::len);
        let added: BTreeSet<&str> = tags
            .iter()
            .map(|tag| tag.key.as_str())
            .filter(|key| existing.is_none_or(|set| !set.contains_key(*key)))
            .collect();
        let would_hold = held + added.len();
        if would_hold > MAX_TAGS_PER_RESOURCE {
            return Err(TagCeilingExceeded { would_hold });
        }
        let set = self.quota_tags.entry(canonical_arn.to_string()).or_default();
        for tag in tags {
            set.insert(tag.key.clone(), tag.value.clone());
        }
        Ok(())
    }

    /// Remove the tags `keys` names from the applied quota `canonical_arn`.
    ///
    /// Removal is idempotent: a key the resource does not carry is not an error, and
    /// the tags it does carry under other keys are left alone. A quota left with no
    /// tags keeps no entry, so an untagged quota costs nothing to store.
    pub fn remove_resource_tags(&mut self, canonical_arn: &str, keys: &[String]) {
        let Some(tags) = self.quota_tags.get_mut(canonical_arn) else {
            return;
        };
        for key in keys {
            tags.remove(key);
        }
        if tags.is_empty() {
            self.quota_tags.remove(canonical_arn);
        }
    }

    /// The applied overlay in force for a quota, read from where that quota's scope
    /// keeps it: account-wide for a `GlobalQuota`, the caller's region otherwise.
    fn applied_entry(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Option<&AppliedQuota> {
        let key = (service_code.to_string(), quota_code.to_string());
        if self
            .default_quota(service_code, quota_code)
            .is_some_and(|quota| quota.global_quota)
        {
            return self.global_applied_quotas.get(&key);
        }
        self.regions.get(region)?.applied_quotas.get(&key)
    }

    /// The fields both quota views share.
    ///
    /// `applied` picks the ARN form: an APPLIED quota belongs to the account, so its
    /// ARN names it, while an AWS DEFAULT quota belongs to no account and its ARN's
    /// account segment is empty (see [`default_quota_arn`]).
    fn quota_view(&self, region: &str, default: &DefaultQuota, applied: bool) -> ServiceQuota {
        let service_code = default.service_code.as_str();
        let quota_code = default.quota_code.as_str();
        let arn = if applied {
            quota_arn(region, service_code, quota_code, default.global_quota)
        } else {
            default_quota_arn(region, service_code, quota_code, default.global_quota)
        };
        ServiceQuota {
            service_code: Some(service_code.to_string()),
            service_name: self.service_name(service_code).map(str::to_string),
            quota_arn: Some(arn),
            quota_code: Some(quota_code.to_string()),
            quota_name: Some(default.quota_name.clone()),
            value: Some(default.value),
            unit: Some(default.unit.clone()),
            adjustable: default.adjustable,
            global_quota: default.global_quota,
            usage_metric: default.usage_metric.clone(),
            period: default.period.clone(),
            // `ErrorReason` reports a dependency failure while resolving the quota;
            // reading seeded state cannot fail that way.
            error_reason: None,
            quota_applied_at_level: None,
            quota_context: None,
            description: default.description.clone(),
        }
    }
}

/// An `ExclusionList` as it arrives on the wire (service code -> quota codes), in the
/// form [`AutoManagementConfig::exclusion_list`] stores: keyed in service-code order,
/// with each service's codes kept exactly as the caller named them, and empty when the
/// request named no exclusion list at all.
pub fn stored_exclusion_list(
    exclusion_list: Option<HashMap<String, Vec<String>>>,
) -> BTreeMap<String, Vec<String>> {
    exclusion_list.unwrap_or_default().into_iter().collect()
}

/// A wall-clock instant as the modelled `Timestamp` the wire carries.
fn epoch_timestamp(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The ARN of one APPLIED quota:
/// `arn:aws:servicequotas:<region>:<account>:<service>/<quota>`.
///
/// An applied quota is the account's own — it is what an increase applies to — so its
/// ARN names the account, through the canonical [`::engine::emulator_arn`] helper.
/// `GetServiceQuota`, `ListServiceQuotas` and the quota increase requests report this
/// form; the AWS DEFAULT of the same quota reports [`default_quota_arn`].
///
/// A global quota's ARN carries no region, the way every global-service ARN does, so
/// the same quota always reports the same ARN whichever regional endpoint is called.
pub fn quota_arn(region: &str, service_code: &str, quota_code: &str, global_quota: bool) -> String {
    let region = if global_quota { "" } else { region };
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{service_code}/{quota_code}"))
}

/// The ARN of one AWS DEFAULT quota:
/// `arn:aws:servicequotas:<region>::<service>/<quota>` — the ACCOUNT segment is
/// EMPTY.
///
/// An AWS default is published by the service, not owned by an account: the same
/// default is the same value in every account, so its ARN names no account. That is
/// the one difference from [`quota_arn`], and it is what distinguishes a default from
/// the applied quota of the same pair rather than a formatting accident.
///
/// `::engine::emulator_arn` always fills the account segment with the emulator's
/// canonical account, so the account-less form is built here; there is no account in
/// it to disclose, and the region rule is the same as [`quota_arn`]'s.
pub fn default_quota_arn(
    region: &str,
    service_code: &str,
    quota_code: &str,
    global_quota: bool,
) -> String {
    let region = if global_quota { "" } else { region };
    format!("arn:aws:{ARN_NAMESPACE}:{region}::{service_code}/{quota_code}")
}

/// Whether `partition` is an AWS ARN partition, as the modelled
/// `AmazonResourceName` pattern spells one: `aws(-[\w]+)*` — `aws`, `aws-cn`,
/// `aws-us-gov`.
fn is_aws_partition(partition: &str) -> bool {
    let Some(suffix) = partition.strip_prefix("aws") else {
        return false;
    };
    if suffix.is_empty() {
        return true;
    }
    let Some(parts) = suffix.strip_prefix('-') else {
        return false;
    };
    parts.split('-').all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|character| character.is_alphanumeric() || character == '_')
    })
}

/// Whether `key` is in the tag namespace AWS reserves for itself
/// ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// Matched case-insensitively, the way AWS states the restriction (the prefix is
/// reserved, not one spelling of it), and on the key's leading bytes only, so a key
/// that merely CONTAINS the prefix is a caller's own tag.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// How a tagging operation states that `resource_arn` is not one of its ARNs, for the
/// modelled `IllegalArgumentException`. Built from the request, naming the member and
/// the form it has to take.
pub fn malformed_resource_arn_message(resource_arn: &str) -> String {
    format!(
        "ResourceARN '{resource_arn}' is not an applied quota ARN of the form \
         arn:<partition>:{ARN_NAMESPACE}:<region>:<account>:<ServiceCode>/<QuotaCode>"
    )
}

/// How a tagging operation states that `resource_arn` names no applied quota of this
/// account, for the modelled `NoSuchResourceException`.
pub fn no_such_resource_arn_message(resource_arn: &str) -> String {
    format!("no applied quota with ResourceARN '{resource_arn}' exists")
}

/// The identity a request records as its `Requester`: this emulator's single
/// account's root identity, built through the canonical ARN helper.
pub fn requester_identity() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// Why a quota request TEMPLATE call is refused.
///
/// Both arms are errors every template operation of this service models
/// (`smithy_query(query="operation",
/// operation="PutServiceQuotaIncreaseRequestIntoTemplate,…")`), so each handler maps
/// this to its own error enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateRefusal {
    /// The call was made at a Region the template plane is not served from:
    /// `TemplatesNotAvailableInRegionException`.
    NotAvailableInRegion,
    /// The calling account belongs to no AWS Organization:
    /// `NoAvailableOrganizationException`.
    NoAvailableOrganization,
}

/// The gate every quota request template operation passes through, evaluated for a call
/// made at `region`.
///
/// Two conditions stand in front of the whole template plane, and this is the only
/// place either is decided:
///
/// 1. the plane is served from one Region only ([`TEMPLATE_HOME_REGION`]), so a call
///    anywhere else is refused with [`TemplateRefusal::NotAvailableInRegion`];
/// 2. a quota request template belongs to an AWS ORGANIZATION — AWS applies it to the
///    organization's member accounts — so an account that is in none has no template to
///    read, write or associate and is refused with
///    [`TemplateRefusal::NoAvailableOrganization`].
///
/// There is no third answer for the account this emulator serves: it is a single
/// STANDALONE account. Service Quotas models no organization membership and has no
/// operation that could create or join one (AWS Organizations is a different service,
/// with its own state in its own crate), so condition 2 can never be satisfied from
/// here and no template entry can become observable. That is why this returns a
/// refusal rather than a `Result`: a template STORE would be state nothing could ever
/// write or read, and a handler that pretended otherwise would report a template the
/// account does not have.
pub fn template_refusal(region: &str) -> TemplateRefusal {
    if region == TEMPLATE_HOME_REGION {
        TemplateRefusal::NoAvailableOrganization
    } else {
        TemplateRefusal::NotAvailableInRegion
    }
}

/// The region a request is addressed to: the SigV4 credential scope's region, which
/// is what the caller's endpoint resolved to, else [`DEFAULT_REGION`].
///
/// The scope is caller-supplied, so only a region in [`KNOWN_REGIONS`] is taken: a
/// request naming anything else is served as [`DEFAULT_REGION`]. That is what keeps
/// the number of per-region state buckets bounded by that list, and what stops a
/// caller from side-stepping a per-region invariant by varying the string.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| KNOWN_REGIONS.contains(&region.as_str()))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Derive a request `Id`: 32 lowercase hex characters, which satisfies the model's
/// `RequestId` pattern `^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$`.
///
/// Derived from the region's own mint counter plus the clock, so two requests never
/// share an id, and stored by the caller — a client only ever sees an id that is in
/// state.
fn request_id(
    region: &str,
    service_code: &str,
    quota_code: &str,
    sequence: u64,
    now: SystemTime,
) -> String {
    use sha2::{Digest as _, Sha256};

    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(format!(
        "{region}:{service_code}:{quota_code}:{sequence}:{nanos}"
    ));
    hex::encode(&digest[..16])
}

/// One page of an ordered listing.
#[derive(Debug)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// The token that resumes where this page stopped; `None` on the last page.
    pub next_token: Option<String>,
}

/// A `NextToken` that does not decode to a position in the listing.
#[derive(Debug)]
pub struct InvalidPageToken;

/// Cut one page out of an ordered listing.
///
/// `listing` names the listing the token belongs to (the operation's own name), so a
/// token issued by one listing is refused by another instead of resuming at an
/// unrelated position. `max_results` is the caller's page size (the model bounds it
/// to 1..100 before a handler runs) — a caller that names NONE gets every item in
/// one page — and `next_token` is an opaque position from a
/// previous page. The token is base64 of `<listing>:<offset>` over the standard
/// alphabet, so it matches the modelled `NextToken` pattern
/// `^[a-zA-Z0-9/+]*={0,2}$` and tells a caller nothing it should be reading.
pub fn paginate<T>(
    listing: &str,
    all: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, InvalidPageToken> {
    let start = match next_token {
        Some(token) => decode_page_token(listing, token)?,
        None => 0,
    };
    let total = all.len();
    if start > total {
        return Err(InvalidPageToken);
    }
    // No `MaxResults` is no page size: the listing is returned whole, in one page,
    // and only a caller that asks for a page size gets one. `MaxResults` bounds what
    // a CALLER may ask for (1..100), not what the service returns unasked — a
    // catalog listing of several hundred entries comes back in a single response.
    // Nothing here grows without bound: the catalog and the defaults are seeded
    // tables and the request store is capped by [`MAX_REQUESTS_PER_REGION`].
    let size = max_results.map_or(total, |requested| {
        (requested.max(1) as usize).min(MAX_PAGE_SIZE)
    });
    let items: Vec<T> = all.into_iter().skip(start).take(size).collect();
    let stopped_at = start + items.len();
    Ok(Page {
        items,
        next_token: (stopped_at < total).then(|| encode_page_token(listing, stopped_at)),
    })
}

/// The opaque token for "resume `listing` at `offset`".
fn encode_page_token(listing: &str, offset: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{listing}:{offset}"))
}

/// The position a token names in `listing`, or [`InvalidPageToken`] when it is not
/// one this service issued for that listing.
fn decode_page_token(listing: &str, token: &str) -> Result<usize, InvalidPageToken> {
    // An empty token is the same request as no token at all: start at the beginning.
    if token.is_empty() {
        return Ok(0);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(token)
        .map_err(|_| InvalidPageToken)?;
    let text = String::from_utf8(decoded).map_err(|_| InvalidPageToken)?;
    text.strip_prefix(&format!("{listing}:"))
        .and_then(|offset| offset.parse::<usize>().ok())
        .ok_or(InvalidPageToken)
}

/// One quota of the catalog, before its `QuotaCode` is resolved.
///
/// A row carries the shape most published quotas have — adjustable, counted per
/// region, `Unit` `None` — and the builder methods below name the exceptions.
#[derive(Debug, Clone)]
struct QuotaRow {
    /// AWS's own `QuotaCode`, where this catalog carries it; otherwise `None`, and
    /// the code is derived from the service code and the quota name (see
    /// [`derived_quota_code`]).
    code: Option<String>,
    /// `QuotaName`, as AWS publishes it.
    name: String,
    /// The AWS default `Value`.
    value: f64,
    /// `Unit`: `"None"` for a countable quota.
    unit: String,
    /// `Adjustable`: whether an increase can be requested at all.
    adjustable: bool,
    /// `GlobalQuota`: counted account-wide rather than per region.
    global_quota: bool,
    /// `UsageMetric`, for a quota CloudWatch publishes usage for.
    usage_metric: Option<MetricInfo>,
    /// `Period`, for a quota AWS publishes as a RATE (so many per unit of time).
    period: Option<QuotaPeriod>,
    /// `Description`, where AWS publishes one for the quota.
    description: Option<String>,
}

/// A countable, adjustable, per-region quota: the shape most published quotas have.
fn quota(name: impl Into<String>, value: f64) -> QuotaRow {
    QuotaRow {
        code: None,
        name: name.into(),
        value,
        unit: "None".to_string(),
        adjustable: true,
        global_quota: false,
        usage_metric: None,
        period: None,
        description: None,
    }
}

impl QuotaRow {
    /// AWS's own `QuotaCode` for this quota.
    fn code(mut self, code: &str) -> Self {
        self.code = Some(code.to_string());
        self
    }

    /// The `Unit` AWS reports for a measured quota.
    fn unit(mut self, unit: &str) -> Self {
        self.unit = unit.to_string();
        self
    }

    /// AWS publishes this quota as NOT adjustable.
    fn fixed(mut self) -> Self {
        self.adjustable = false;
        self
    }

    /// AWS counts this quota account-wide rather than per region.
    fn global(mut self) -> Self {
        self.global_quota = true;
        self
    }

    /// The CloudWatch usage metric AWS publishes for this quota.
    ///
    /// A measured quota also publishes the `Period` its usage metric is published
    /// over, unless the row already names a rate window: the `AWS/Usage`
    /// `ResourceCount` metrics are published every
    /// [`RESOURCE_COUNT_PERIOD_MINUTES`] minutes, which is the period the recorded
    /// response for a measured quota carries.
    fn metric(mut self, metric: MetricInfo) -> Self {
        self.usage_metric = Some(metric);
        if self.period.is_none() {
            self.period = Some(QuotaPeriod {
                period_value: Some(RESOURCE_COUNT_PERIOD_MINUTES),
                period_unit: Some("MINUTE".to_string()),
            });
        }
        self
    }

    /// The `Period` of a quota AWS publishes as a rate: so many per unit of time.
    fn period(mut self, value: i32, unit: &str) -> Self {
        self.period = Some(QuotaPeriod {
            period_value: Some(value),
            period_unit: Some(unit.to_string()),
        });
        self
    }

    /// The `Description` AWS publishes for this quota.
    fn describe(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }
}

/// One service of the quota catalog: the service the rows belong to, and the quotas
/// `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` report for it.
#[derive(Debug)]
struct CatalogService {
    /// `ServiceCode` — AWS's code for the service (its endpoint prefix).
    code: &'static str,
    /// `ServiceName`, as Service Quotas reports it. Used only for a service
    /// [`SEED_SERVICE_CATALOG`] does not already name: a service whose quotas this
    /// emulator publishes must be listed, or its quotas would be unreachable.
    name: &'static str,
    /// The quotas the service publishes.
    quotas: Vec<QuotaRow>,
}

/// One catalog entry.
fn service(code: &'static str, name: &'static str, quotas: Vec<QuotaRow>) -> CatalogService {
    CatalogService { code, name, quotas }
}

/// The seeded AWS default quotas, in the order [`seed_quota_catalog`] PUBLISHES
/// them for each service.
///
/// Order is state: both listings report a service's quotas in this order, so the
/// first element of a listing is the first quota the catalog publishes for that
/// service rather than whichever `QuotaCode` happens to sort first. Lookups go
/// through [`ComAmazonawsServicequotasServicequotasv20190624State::default_quota`],
/// which finds the row for exactly the `(ServiceCode, QuotaCode)` pair asked for.
///
/// Coverage is per PAIR, not per service: the table holds the published defaults
/// this emulator carries. A service code in [`SEED_SERVICE_CATALOG`] with no pair
/// here therefore has an EMPTY default quota set, and `ListAWSDefaultServiceQuotas`
/// answers it with HTTP 200 and an empty `Quotas` list — the code exists, so
/// `NoSuchResourceException` (which belongs to a code that is not an AWS service
/// code at all) would be the wrong answer.
///
/// The `QuotaCode` of a row is AWS's own where the table carries it, else the
/// derived one ([`derived_quota_code`]).
fn seed_default_quotas() -> Vec<DefaultQuota> {
    let mut default_quotas: Vec<DefaultQuota> = Vec::new();
    for entry in seed_quota_catalog() {
        for row in entry.quotas {
            let code = row
                .code
                .clone()
                .unwrap_or_else(|| derived_quota_code(entry.code, &row.name));
            // A pair identifies a quota, so a repeated QuotaCode within one service
            // would make one of the two rows unreachable through
            // `GetServiceQuota`. A new table row that collides — with another row's
            // code or with a derived one — fails here, at the source, rather than in
            // a listing far away.
            debug_assert!(
                !default_quotas
                    .iter()
                    .any(|held| held.service_code == entry.code && held.quota_code == code),
                "two catalog rows share the QuotaCode ({:?}, {code:?})",
                entry.code
            );
            default_quotas.push(DefaultQuota::from_row(entry.code, code, row));
        }
    }
    default_quotas
}

/// The `QuotaCode` of a catalog row whose published code this catalog does not
/// carry.
///
/// AWS mints an opaque `L-` code for every quota it publishes. Where this catalog
/// does not carry AWS's own code, the code is DERIVED from the service code and the
/// quota's published name: 8 uppercase hex characters after the `L-`, the shape
/// AWS's codes have and one the modelled `QuotaCode` pattern
/// `^[a-zA-Z][a-zA-Z0-9-]{1,128}$` accepts. It is the same on every run, in every
/// region, and unique per (service, quota name) — never derived from a request, and
/// never from the clock.
fn derived_quota_code(service_code: &str, quota_name: &str) -> String {
    use sha2::{Digest as _, Sha256};

    let digest = Sha256::digest(format!("{service_code}:{quota_name}"));
    format!("L-{}", hex::encode_upper(&digest[..4]))
}

/// The seeded quota catalog: the quotas each service publishes, and (for a service
/// [`SEED_SERVICE_CATALOG`] does not name) the name to list it under.
///
/// SOURCE: AWS's own published Service Quotas data — the quota names, default
/// values, adjustability and scope of the per-service "Service quotas" pages (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>`
/// reports). A quota's default in a real account depends on the account and the
/// region (AWS publishes these as defaults "for new accounts"), so these are the
/// defaults this emulator serves, and every operation computes its answer from
/// them rather than from a literal in a handler.
///
/// A service's quota list is a CATALOG, not a sample: for a service such as EC2 it
/// runs past the largest page `MaxResults` allows (1..100), so a walk of
/// `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` genuinely takes many
/// `NextToken`-chained pages.
fn seed_quota_catalog() -> Vec<CatalogService> {
    vec![
        service(
            "access-analyzer",
            "IAM Access Analyzer",
            vec![
                quota("Analyzers per Region", 1.0).fixed(),
                quota("Archive rules per analyzer", 100.0),
            ],
        ),
        service(
            "acm",
            "AWS Certificate Manager (ACM)",
            vec![
                quota("ACM certificates", 2500.0),
                quota("Imported certificates", 2500.0),
            ],
        ),
        service(
            "acm-pca",
            "AWS Private Certificate Authority",
            vec![quota("Certificate authorities", 200.0)],
        ),
        service(
            "airflow",
            "Amazon Managed Workflows for Apache Airflow",
            vec![quota("Environments per Region", 10.0)],
        ),
        service(
            "amplify",
            "AWS Amplify",
            vec![
                quota("Apps per Region", 25.0),
                quota("Branches per app", 50.0),
            ],
        ),
        service(
            "apigateway",
            "Amazon API Gateway",
            vec![
                quota("Regional APIs per Region", 600.0),
                quota("Edge-optimized APIs per account", 120.0),
                quota("Stages per API", 10.0),
            ],
        ),
        service(
            "appconfig",
            "AWS AppConfig",
            vec![quota("Configuration profiles per application", 100.0)],
        ),
        service(
            "appflow",
            "Amazon AppFlow",
            vec![quota("Flows per account", 1000.0)],
        ),
        service(
            "application-autoscaling",
            "Application Auto Scaling",
            vec![quota("Scalable targets per resource type", 500.0)],
        ),
        service(
            "appmesh",
            "AWS App Mesh",
            vec![
                quota("Meshes per account", 15.0),
                quota("Virtual services per mesh", 200.0),
            ],
        ),
        service(
            "apprunner",
            "AWS App Runner",
            vec![quota("Services per account", 100.0)],
        ),
        service(
            "appstream2",
            "Amazon AppStream 2.0",
            vec![quota("Fleets", 10.0)],
        ),
        service(
            "appsync",
            "AWS AppSync",
            vec![quota("APIs per Region", 25.0)],
        ),
        service(
            "athena",
            "Amazon Athena",
            vec![
                quota("Active DML queries", 20.0),
                quota("Query timeout", 30.0).unit("Minutes"),
                quota("Workgroups per Region", 1000.0),
            ],
        ),
        service(
            "autoscaling",
            "Amazon EC2 Auto Scaling",
            vec![
                quota("Auto Scaling groups per Region", 200.0),
                quota("Launch configurations per Region", 200.0),
            ],
        ),
        service(
            "autoscaling-plans",
            "AWS Auto Scaling",
            vec![quota("Scaling plans per Region", 5.0)],
        ),
        service(
            "backup",
            "AWS Backup",
            vec![
                quota("Backup plans per account", 100.0),
                quota("Backup vaults per account", 100.0),
            ],
        ),
        service(
            "batch",
            "AWS Batch",
            vec![
                quota("Job queues", 50.0),
                quota("Compute environments", 50.0),
            ],
        ),
        service(
            "budgets",
            "AWS Budgets",
            vec![quota("Budgets", 20000.0).global()],
        ),
        service(
            "cassandra",
            "Amazon Keyspaces (for Apache Cassandra)",
            vec![quota("Tables per Region", 256.0)],
        ),
        service(
            "cloud9",
            "AWS Cloud9",
            vec![quota("Environments", 50.0)],
        ),
        service(
            "cloudformation",
            "AWS CloudFormation",
            vec![
                quota("Stack count", 2000.0),
                quota("Stack sets per administrator account", 100.0),
                quota("Stack instances per stack set", 10000.0),
            ],
        ),
        service(
            "cloudfront",
            "Amazon CloudFront",
            vec![
                // CloudFront is a global service: its quotas count account-wide.
                quota("Web distributions per account", 200.0).global(),
                quota("Alternate domain names (CNAMEs) per distribution", 100.0).global(),
                quota("Cache behaviors per distribution", 25.0).global(),
            ],
        ),
        service(
            "cloudhsm",
            "AWS CloudHSM",
            vec![quota("Clusters per Region", 4.0)],
        ),
        service(
            "cloudtrail",
            "AWS CloudTrail",
            vec![quota("Trails per Region", 5.0).fixed()],
        ),
        service(
            "codeartifact",
            "AWS CodeArtifact",
            vec![quota("Domains per account", 10.0)],
        ),
        service(
            "codebuild",
            "AWS CodeBuild",
            vec![
                quota("Build projects", 5000.0),
                quota("Concurrently running builds", 60.0),
            ],
        ),
        service(
            "codecommit",
            "AWS CodeCommit",
            vec![quota("Repositories per account", 1000.0)],
        ),
        service(
            "codedeploy",
            "AWS CodeDeploy",
            vec![
                quota("Applications per Region", 1000.0),
                quota("Deployment groups per application", 1000.0),
            ],
        ),
        service(
            "codepipeline",
            "AWS CodePipeline",
            vec![quota("Pipelines per Region", 1000.0)],
        ),
        service(
            "cognito-identity",
            "Amazon Cognito Federated Identities",
            vec![quota("Identity pools", 1000.0)],
        ),
        service(
            "cognito-idp",
            "Amazon Cognito User Pools",
            vec![
                quota("User pools per account", 1000.0),
                quota("App clients per user pool", 1000.0),
            ],
        ),
        service(
            "comprehend",
            "Amazon Comprehend",
            vec![quota("Custom classifiers per account", 10.0)],
        ),
        service(
            "config",
            "AWS Config",
            vec![
                quota("Config rules per Region", 150.0),
                quota("Delivery channels", 1.0).fixed(),
            ],
        ),
        service(
            "connect",
            "Amazon Connect",
            vec![quota("Amazon Connect instances per account", 5.0)],
        ),
        service(
            "datasync",
            "AWS DataSync",
            vec![
                quota("Tasks per account", 100.0),
                quota("Agents per account", 50.0),
            ],
        ),
        service(
            "dax",
            "Amazon DynamoDB Accelerator (DAX)",
            vec![quota("Nodes per cluster", 11.0).fixed()],
        ),
        service(
            "directconnect",
            "AWS Direct Connect",
            vec![quota("Connections per Region", 10.0)],
        ),
        service(
            "dms",
            "AWS Database Migration Service",
            vec![
                quota("Replication instances", 20.0),
                quota("Endpoints", 1000.0),
            ],
        ),
        service(
            "docdb",
            "Amazon DocumentDB",
            vec![
                quota("Clusters", 40.0),
                quota("Instances per cluster", 16.0),
            ],
        ),
        service(
            "ds",
            "AWS Directory Service",
            vec![quota("Directories", 10.0)],
        ),
        service(
            "dynamodb",
            "Amazon DynamoDB",
            vec![
                quota("Maximum number of tables", 2500.0)
                    .code("L-F98FE922")
                    // DynamoDB publishes table count as a CloudWatch usage metric
                    // (`AWS/Usage`, `ResourceCount`, `Resource=TableCount`), which is
                    // what Service Quotas reports as this quota's `UsageMetric`.
                    .metric(usage_metric(
                        "DynamoDB",
                        &[
                            ("Class", "None"),
                            ("Resource", "TableCount"),
                            ("Type", "Resource"),
                        ],
                    ))
                    .describe(
                        "The maximum number of tables that can be created in this account in \
                         the current Region.",
                    ),
                quota("Account-level write throughput limit", 80000.0),
                quota("Global secondary indexes per table", 20.0).fixed(),
            ],
        ),
        service(
            "ebs",
            "Amazon Elastic Block Store (Amazon EBS)",
            vec![
                quota("Storage for General Purpose SSD (gp3) volumes, in TiB", 50.0).unit("TiB"),
                quota("Snapshots per Region", 100000.0),
            ],
        ),
        service(
            "ec2",
            "Amazon Elastic Compute Cloud (Amazon EC2)",
            ec2_quotas(),
        ),
        service(
            "ecr",
            "Amazon Elastic Container Registry (Amazon ECR)",
            vec![
                quota("Repositories", 10000.0),
                quota("Images per repository", 10000.0),
            ],
        ),
        service(
            "ecs",
            "Amazon Elastic Container Service (Amazon ECS)",
            vec![
                quota("Clusters per account", 10000.0),
                quota("Services per cluster", 5000.0),
                quota("Tasks per service", 5000.0),
            ],
        ),
        service(
            "eks",
            "Amazon Elastic Kubernetes Service (Amazon EKS)",
            vec![
                quota("Clusters", 100.0),
                quota("Managed node groups per cluster", 30.0),
                quota("Nodes per managed node group", 450.0),
            ],
        ),
        service(
            "elasticache",
            "Amazon ElastiCache",
            vec![
                quota("Nodes per Region", 300.0),
                quota("Parameter groups per Region", 150.0),
            ],
        ),
        service(
            "elasticbeanstalk",
            "AWS Elastic Beanstalk",
            vec![
                quota("Applications", 75.0),
                quota("Environments", 200.0),
            ],
        ),
        service(
            "elasticfilesystem",
            "Amazon Elastic File System (Amazon EFS)",
            vec![
                quota("File systems per account", 1000.0),
                quota("Access points per file system", 1000.0),
            ],
        ),
        service(
            "elasticloadbalancing",
            "Elastic Load Balancing (ELB)",
            vec![
                quota("Application Load Balancers per Region", 50.0),
                quota("Classic Load Balancers per Region", 20.0),
                quota("Target groups per Region", 3000.0),
            ],
        ),
        service(
            "elasticmapreduce",
            "Amazon EMR",
            vec![quota("Active clusters", 100.0)],
        ),
        service(
            "emr-serverless",
            "Amazon EMR Serverless",
            vec![quota("Applications per account", 100.0)],
        ),
        service(
            "es",
            "Amazon OpenSearch Service",
            vec![
                quota("Domains per Region", 100.0),
                quota("Instances per domain", 80.0),
            ],
        ),
        service(
            "events",
            "Amazon EventBridge (CloudWatch Events)",
            vec![
                quota("Event buses per account", 100.0),
                quota("Rules per event bus", 300.0),
                quota("PutEvents throttle limit in transactions per second", 10000.0).fixed(),
            ],
        ),
        service(
            "firehose",
            "Amazon Data Firehose",
            vec![quota("Delivery streams per Region", 5000.0)],
        ),
        service(
            "fms",
            "AWS Firewall Manager",
            vec![quota("Policies per Region", 50.0)],
        ),
        service(
            "forecast",
            "Amazon Forecast",
            vec![quota("Datasets per account", 100.0)],
        ),
        service(
            "fsx",
            "Amazon FSx",
            vec![quota("File systems per account", 100.0)],
        ),
        service(
            "gamelift",
            "Amazon GameLift",
            vec![quota("Fleets", 100.0)],
        ),
        service(
            "glacier",
            "Amazon S3 Glacier",
            vec![quota("Vaults per Region", 1000.0).fixed()],
        ),
        service(
            "globalaccelerator",
            "AWS Global Accelerator",
            vec![quota("Accelerators per account", 20.0).global()],
        ),
        service(
            "glue",
            "AWS Glue",
            vec![
                quota("Databases per account", 10000.0),
                quota("Tables per database", 200000.0),
                quota("Concurrent job runs per account", 200.0),
            ],
        ),
        service(
            "groundstation",
            "AWS Ground Station",
            vec![quota("Mission profiles per account", 500.0)],
        ),
        service(
            "guardduty",
            "Amazon GuardDuty",
            vec![
                quota("Detectors per Region", 1.0).fixed(),
                quota("Threat intel sets per detector", 6.0),
            ],
        ),
        service(
            "iam",
            "AWS Identity and Access Management (IAM)",
            vec![
                // IAM is a global service: every one of its quotas is account-wide.
                quota("Roles per account", 1000.0).global(),
                quota("Managed policies per account", 1500.0).global(),
                quota("Instance profiles per account", 1000.0).global(),
                quota("Users per account", 5000.0).global(),
            ],
        ),
        service(
            "imagebuilder",
            "EC2 Image Builder",
            vec![quota("Image pipelines", 1000.0)],
        ),
        service(
            "iot",
            "AWS IoT Core",
            vec![quota("Rules per account", 1000.0)],
        ),
        service(
            "iotevents",
            "AWS IoT Events",
            vec![quota("Detector models per account", 1000.0)],
        ),
        service(
            "ivs",
            "Amazon Interactive Video Service",
            vec![quota("Channels per account", 5000.0)],
        ),
        service(
            "kafka",
            "Amazon Managed Streaming for Apache Kafka (MSK)",
            vec![
                quota("Clusters per account", 90.0),
                quota("Brokers per cluster", 30.0),
            ],
        ),
        service(
            "kendra",
            "Amazon Kendra",
            vec![quota("Indexes per account", 5.0)],
        ),
        service(
            "kinesis",
            "Amazon Kinesis Data Streams",
            vec![quota("Shards per Region", 500.0)],
        ),
        service(
            "kinesisanalytics",
            "Amazon Managed Service for Apache Flink",
            vec![quota("Applications per Region", 50.0)],
        ),
        service(
            "kinesisvideo",
            "Amazon Kinesis Video Streams",
            vec![quota("Streams per account", 10000.0)],
        ),
        service(
            "kms",
            "AWS Key Management Service (AWS KMS)",
            vec![
                quota("Customer managed KMS keys", 100000.0),
                quota("Aliases per KMS key", 50.0).fixed(),
                quota("Grants per KMS key", 50000.0),
            ],
        ),
        service(
            "lakeformation",
            "AWS Lake Formation",
            vec![quota("Data lake administrators per account", 30.0)],
        ),
        service(
            "lambda",
            "AWS Lambda",
            vec![
                // AWS's own `QuotaCode`s for nine of Lambda's published quotas, in
                // the order the service reports them — the order a recorded
                // `ListServiceQuotas("lambda")` page carries, which is why the rate
                // quotas come first. Their names, values and units are the ones the
                // AWS Lambda quotas page publishes; a code this table does not carry
                // gets the derived one ([`derived_quota_code`]).
                //
                // The API request-rate quotas AWS publishes as not adjustable, each
                // with its own `Period`: so many requests per second.
                quota(
                    "Rate of control plane API requests (excludes invocation, GetFunction, \
                     and GetPolicy requests)",
                    15.0,
                )
                .code("L-B2AA0F47")
                .fixed()
                .period(1, "SECOND")
                .describe(
                    "The maximum rate of control plane API requests, excluding invocation, \
                     GetFunction and GetPolicy requests.",
                ),
                quota("Rate of GetFunction API requests", 100.0)
                    .code("L-72E0D058")
                    .fixed()
                    .period(1, "SECOND")
                    .describe("The maximum rate of GetFunction API requests."),
                quota("Rate of GetPolicy API requests", 15.0)
                    .code("L-F55076EC")
                    .fixed()
                    .period(1, "SECOND")
                    .describe("The maximum rate of GetPolicy API requests."),
                quota("Function timeout", 900.0)
                    .code("L-B82A30EA")
                    .unit("Seconds")
                    .fixed()
                    .describe(
                        "The maximum amount of time that a function can run before Lambda \
                         stops it.",
                    ),
                quota("Function layers", 5.0)
                    .code("L-14320ECE")
                    .fixed()
                    .describe("The maximum number of layers that a function can use."),
                quota("Environment variable size", 4.0)
                    .code("L-25EEC0A4")
                    .unit("Kilobytes")
                    .fixed()
                    .describe(
                        "The maximum total size of the environment variables of a function.",
                    ),
                quota("Function resource-based policy", 20.0)
                    .code("L-4273958C")
                    .unit("Kilobytes")
                    .fixed()
                    .describe("The maximum size of the resource-based policy of a function."),
                quota("Invocation payload (synchronous)", 6.0)
                    .code("L-B430C318")
                    .unit("Megabytes")
                    .fixed()
                    .describe(
                        "The maximum size of the request and response payload of a \
                         synchronous invocation.",
                    ),
                quota("Invocation payload (asynchronous)", 256.0)
                    .code("L-BA29C22B")
                    .unit("Kilobytes")
                    .fixed()
                    .describe(
                        "The maximum size of the event payload of an asynchronous invocation.",
                    ),
                quota("Function and layer storage", 75.0)
                    .code("L-2ACBD22F")
                    .unit("Gigabytes")
                    .describe(
                        "The maximum total storage for the .zip deployment packages and layers \
                         of this account in this Region.",
                    ),
                quota("Concurrent executions", 1000.0)
                    .code("L-B99A9384")
                    .describe(
                        "The maximum number of function invocations that can run concurrently \
                         in this Region.",
                    )
                    .metric(usage_metric(
                        "Lambda",
                        &[
                            ("Class", "None"),
                            ("Resource", "ConcurrentExecutions"),
                            ("Type", "Resource"),
                        ],
                    )),
                quota("Elastic network interfaces per VPC", 250.0).describe(
                    "The maximum number of elastic network interfaces that the functions of \
                     this account can attach to one VPC.",
                ),
            ],
        ),
        service(
            "lex",
            "Amazon Lex",
            vec![quota("Bots per account", 100.0)],
        ),
        service(
            "license-manager",
            "AWS License Manager",
            vec![quota("License configurations per account", 25.0)],
        ),
        service(
            "lightsail",
            "Amazon Lightsail",
            vec![
                quota("Instances", 20.0),
                quota("Static IP addresses", 5.0),
            ],
        ),
        service(
            "logs",
            "Amazon CloudWatch Logs",
            vec![
                quota("Subscription filters per log group", 2.0).fixed(),
                quota("Metric filters per log group", 100.0).fixed(),
                quota("DescribeLogStreams throttle limit in transactions per second", 25.0),
            ],
        ),
        service(
            "memorydb",
            "Amazon MemoryDB",
            vec![quota("Nodes per Region", 300.0)],
        ),
        service(
            "mediaconvert",
            "AWS Elemental MediaConvert",
            vec![quota("Queues per Region", 10.0)],
        ),
        service(
            "medialive",
            "AWS Elemental MediaLive",
            vec![quota("Channels per Region", 5.0)],
        ),
        service(
            "mediapackage",
            "AWS Elemental MediaPackage",
            vec![quota("Channels per Region", 200.0)],
        ),
        service(
            "mgn",
            "AWS Application Migration Service",
            vec![quota("Source servers per account", 3000.0)],
        ),
        service(
            "monitoring",
            "Amazon CloudWatch",
            vec![
                quota("Alarms per Region", 5000.0),
                quota("Dashboards per account", 5000.0),
                quota("Metric streams per account", 100.0),
            ],
        ),
        service(
            "mq",
            "Amazon MQ",
            vec![quota("Brokers per Region", 50.0)],
        ),
        service(
            "neptune",
            "Amazon Neptune",
            vec![quota("Clusters", 40.0)],
        ),
        service(
            "opsworks",
            "AWS OpsWorks",
            vec![quota("Stacks per Region", 40.0)],
        ),
        service(
            "organizations",
            "AWS Organizations",
            vec![quota("Default maximum number of accounts", 10.0).global()],
        ),
        service(
            "personalize",
            "Amazon Personalize",
            vec![quota("Dataset groups per account", 50.0)],
        ),
        service(
            "pinpoint",
            "Amazon Pinpoint",
            vec![quota("Projects per account", 100.0)],
        ),
        service(
            "qldb",
            "Amazon Quantum Ledger Database (Amazon QLDB)",
            vec![quota("Ledgers per Region", 5.0)],
        ),
        service(
            "quicksight",
            "Amazon QuickSight",
            vec![quota("Datasets", 1000.0)],
        ),
        service(
            "ram",
            "AWS Resource Access Manager (AWS RAM)",
            vec![quota("Resource shares per account", 5000.0)],
        ),
        service(
            "rds",
            "Amazon Relational Database Service (Amazon RDS)",
            vec![
                quota("DB instances", 40.0),
                quota("DB clusters", 40.0),
                quota("Manual DB instance snapshots", 100.0),
            ],
        ),
        service(
            "redshift",
            "Amazon Redshift",
            vec![
                quota("Clusters", 100.0),
                quota("Nodes per cluster", 128.0).fixed(),
            ],
        ),
        service(
            "redshift-serverless",
            "Amazon Redshift Serverless",
            vec![quota("Namespaces per account", 25.0)],
        ),
        service(
            "rekognition",
            "Amazon Rekognition",
            vec![quota("Collections per Region", 1000.0)],
        ),
        service(
            "resource-groups",
            "AWS Resource Groups",
            vec![quota("Resource groups per account", 1000.0)],
        ),
        service(
            "route53",
            "Amazon Route 53",
            vec![
                // Route 53's hosted zones and health checks are account-wide.
                quota("Hosted zones", 500.0).global(),
                quota("Records per hosted zone", 10000.0).global(),
                quota("Health checks", 200.0).global(),
            ],
        ),
        service(
            "route53resolver",
            "Amazon Route 53 Resolver",
            vec![
                quota("Resolver endpoints per Region", 4.0),
                quota("Resolver rules per Region", 1000.0),
            ],
        ),
        service(
            "s3",
            "Amazon Simple Storage Service (Amazon S3)",
            vec![
                // Buckets are counted per account across every region.
                quota("General purpose buckets", 10000.0)
                    .code("L-DC2B2D3D")
                    .global(),
                quota("Access points per Region", 10000.0),
                quota("Multi-Region Access Points per account", 100.0).global(),
            ],
        ),
        service(
            "sagemaker",
            "Amazon SageMaker",
            vec![
                quota("ml.m5.xlarge for notebook instance usage", 2.0),
                quota("ml.m5.xlarge for endpoint usage", 2.0),
                quota("Maximum number of training jobs", 100.0),
            ],
        ),
        service(
            "scheduler",
            "Amazon EventBridge Scheduler",
            vec![quota("Schedules per account", 1000000.0)],
        ),
        service(
            "secretsmanager",
            "AWS Secrets Manager",
            vec![quota("Secrets per Region", 500000.0)],
        ),
        service(
            "securityhub",
            "AWS Security Hub",
            vec![quota("Member accounts per administrator account", 5000.0)],
        ),
        service(
            "servicecatalog",
            "AWS Service Catalog",
            vec![quota("Portfolios", 100.0)],
        ),
        service(
            "servicediscovery",
            "AWS Cloud Map",
            vec![quota("Namespaces per Region", 100.0)],
        ),
        service(
            "ses",
            "Amazon Simple Email Service (Amazon SES)",
            vec![
                quota("Daily sending quota", 200.0),
                quota("Sending rate", 1.0),
            ],
        ),
        service(
            "shield",
            "AWS Shield Advanced",
            vec![quota("Protections per account", 1000.0).global()],
        ),
        service(
            "signer",
            "AWS Signer",
            vec![quota("Signing profiles per account", 100.0)],
        ),
        service(
            "sns",
            "Amazon Simple Notification Service (Amazon SNS)",
            vec![
                quota("Topics per account", 100000.0),
                quota("Subscriptions per topic", 12500000.0),
                quota("Pending subscriptions per account", 5000.0),
            ],
        ),
        service(
            "sqs",
            "Amazon Simple Queue Service (Amazon SQS)",
            vec![
                quota("FIFO queue transactions per second", 3000.0).fixed(),
                quota("Messages in flight per standard queue", 120000.0).fixed(),
            ],
        ),
        service(
            "ssm",
            "AWS Systems Manager",
            vec![
                quota("Parameter Store standard parameters per Region", 10000.0),
                quota("Concurrently executing automations per account", 100.0),
            ],
        ),
        service(
            "sso",
            "AWS IAM Identity Center",
            vec![quota("Permission sets", 500.0).global()],
        ),
        service(
            "states",
            "AWS Step Functions",
            vec![
                quota("State machines per account", 10000.0),
                quota("Open executions per account", 1000000.0),
            ],
        ),
        service(
            "storagegateway",
            "AWS Storage Gateway",
            vec![quota("Gateways per Region", 50.0)],
        ),
        service(
            "swf",
            "Amazon Simple Workflow Service (Amazon SWF)",
            vec![
                quota("Domains per account", 100.0),
                quota("Workflow executions per domain", 100000.0),
            ],
        ),
        service(
            "timestream",
            "Amazon Timestream",
            vec![quota("Databases per account", 500.0)],
        ),
        service(
            "transfer",
            "AWS Transfer Family",
            vec![quota("Servers per Region", 50.0)],
        ),
        service(
            "vpc",
            "Amazon Virtual Private Cloud (Amazon VPC)",
            vpc_quotas(),
        ),
        service(
            "vpc-lattice",
            "Amazon VPC Lattice",
            vec![quota("Service networks per account", 10.0)],
        ),
        service(
            "wafv2",
            "AWS WAF",
            vec![
                quota("Web ACLs per Region", 100.0),
                quota("Rule groups per Region", 100.0),
            ],
        ),
        service(
            "workmail",
            "Amazon WorkMail",
            vec![quota("Organizations per account", 5.0)],
        ),
        service(
            "workspaces",
            "Amazon WorkSpaces",
            vec![quota("WorkSpaces", 1.0)],
        ),
        service(
            "xray",
            "AWS X-Ray",
            vec![quota("Sampling rules", 1000.0)],
        ),
    ]
}

/// The instance families AWS publishes a `Running Dedicated <family> Hosts` quota
/// for, from EC2's "Service quotas" page. One quota per family is what makes the
/// EC2 catalog as long as it is: the published EC2 quota list runs to hundreds of
/// entries, not a handful.
const DEDICATED_HOST_FAMILIES: &[&str] = &[
    "a1", "c3", "c4", "c5", "c5a", "c5d", "c5n", "c6a", "c6g", "c6gd", "c6gn", "c6i", "c6id",
    "c6in", "c7a", "c7g", "c7gd", "c7gn", "c7i", "d2", "d3", "d3en", "dl1", "f1", "f2", "g3",
    "g3s", "g4ad", "g4dn", "g5", "g5g", "g6", "g6e", "gr6", "h1", "i2", "i3", "i3en", "i4g",
    "i4i", "im4gn", "inf1", "inf2", "is4gen", "m3", "m4", "m5", "m5a", "m5ad", "m5d", "m5dn",
    "m5n", "m5zn", "m6a", "m6g", "m6gd", "m6i", "m6id", "m6idn", "m6in", "m7a", "m7g", "m7gd",
    "m7i", "mac1", "mac2", "p2", "p3", "p3dn", "p4d", "p5", "r3", "r4", "r5", "r5a", "r5ad",
    "r5b", "r5d", "r5dn", "r5n", "r6a", "r6g", "r6gd", "r6i", "r6id", "r6idn", "r6in", "r7a",
    "r7g", "r7gd", "r7i", "r7iz", "t3", "trn1", "u-6tb1", "u-9tb1", "u-12tb1", "u-18tb1",
    "u-24tb1", "vt1", "x1", "x1e", "x2gd", "x2idn", "x2iedn", "x2iezn", "z1d",
];

/// One instance-family group AWS meters as vCPU quotas.
struct VcpuGroup {
    /// The group as the published `Running On-Demand <group> instances` /
    /// `All <group> Spot Instance Requests` names spell it.
    group: &'static str,
    /// The token its CloudWatch `AWS/Usage` `Class` dimension is built from.
    class: &'static str,
    /// The default vCPU count AWS publishes for a new account: the Standard group
    /// starts with a small allowance and the accelerated groups start at zero, which
    /// is why an increase has to be requested for them.
    default_vcpus: f64,
    /// AWS's own `QuotaCode` for the On-Demand quota, where this catalog carries it.
    on_demand_code: Option<&'static str>,
    /// AWS's own `QuotaCode` for the Spot quota, where this catalog carries it.
    spot_code: Option<&'static str>,
}

/// The instance-family groups AWS meters as vCPU quotas, On-Demand and Spot.
const VCPU_GROUPS: &[VcpuGroup] = &[
    VcpuGroup {
        group: "Standard (A, C, D, H, I, M, R, T, Z)",
        class: "Standard",
        default_vcpus: 5.0,
        on_demand_code: Some("L-1216C47A"),
        spot_code: Some("L-34B43A08"),
    },
    VcpuGroup {
        group: "DL",
        class: "DL",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "F",
        class: "F",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "G and VT",
        class: "G",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "High Memory",
        class: "HighMemory",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "Inf",
        class: "Inf",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "P",
        class: "P",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "Trn",
        class: "Trn",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "X",
        class: "X",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
];

/// EC2's published quota catalog: the vCPU quotas (On-Demand and Spot) per
/// instance-family group, one Dedicated Hosts quota per instance family, and the
/// account-wide resource counts.
fn ec2_quotas() -> Vec<QuotaRow> {
    let mut rows = Vec::new();

    for group in VCPU_GROUPS {
        // The `Class` dimension of the usage metric CloudWatch publishes for a vCPU
        // quota is the group's token plus the purchase model it meters.
        let on_demand_class = format!("{}/OnDemand", group.class);
        let spot_class = format!("{}/Spot", group.class);
        let mut on_demand = quota(
            format!("Running On-Demand {} instances", group.group),
            group.default_vcpus,
        )
        .metric(vcpu_usage_metric(&on_demand_class));
        let mut spot = quota(
            format!("All {} Spot Instance Requests", group.group),
            group.default_vcpus,
        )
        .metric(vcpu_usage_metric(&spot_class));
        if let Some(code) = group.on_demand_code {
            on_demand = on_demand.code(code);
        }
        if let Some(code) = group.spot_code {
            spot = spot.code(code);
        }
        rows.push(on_demand);
        rows.push(spot);
    }

    // AWS publishes a Dedicated Hosts quota per instance family: two hosts for a
    // family of shared hardware, one for the single-tenant Mac families.
    for family in DEDICATED_HOST_FAMILIES {
        let default_hosts = if family.starts_with("mac") { 1.0 } else { 2.0 };
        rows.push(quota(
            format!("Running Dedicated {family} Hosts"),
            default_hosts,
        ));
    }

    rows.extend(ec2_resource_quotas());
    rows
}

/// The `AWS/Usage` metric CloudWatch publishes for a vCPU quota: the group's own
/// `Class` dimension, counted as a resource.
fn vcpu_usage_metric(class: &str) -> MetricInfo {
    usage_metric(
        "EC2",
        &[("Class", class), ("Resource", "vCPU"), ("Type", "Resource")],
    )
}
/// The service catalog `ListServices` enumerates: every `(ServiceCode,
/// ServiceName)` pair this emulator knows, and therefore every code
/// `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` resolve.
///
/// Source: AWS's own Service Quotas service catalog — the pairs `aws service-quotas
/// list-services` reports (`ServiceInfo{ServiceCode, ServiceName}`, see
/// `smithy_query(query="shape", shape="ServiceInfo")`). Every `ServiceCode` here is
/// an AWS service code and satisfies the modelled `ServiceCode` constraints (length
/// 1..63, pattern `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`), which `tests/all/
/// default_quota_catalog.rs` pins over the whole table.
///
/// This is the catalog's ONE source: [`seed_services`] builds the `ListServices`
/// listing from it and nothing else decides which codes exist, so the set of codes
/// that list and the set that resolve can never drift apart.
///
/// The order here is only for reading; the catalog is stored in a `BTreeMap`, so
/// `ListServices` pages it in `ServiceCode` order whatever order this table is in.
const SEED_SERVICE_CATALOG: &[(&str, &str)] = &[
    ("AWSCloudMap", "AWS Cloud Map"),
    ("access-analyzer", "IAM Access Analyzer"),
    ("acm", "AWS Certificate Manager (ACM)"),
    ("acm-pca", "AWS Private Certificate Authority"),
    ("airflow", "Amazon Managed Workflows for Apache Airflow"),
    ("amplify", "AWS Amplify"),
    ("amplifyuibuilder", "AWS Amplify UI Builder"),
    ("aoss", "Amazon OpenSearch Serverless"),
    ("apigateway", "Amazon API Gateway"),
    ("appconfig", "AWS AppConfig"),
    ("appflow", "Amazon AppFlow"),
    ("application-autoscaling", "Application Auto Scaling"),
    ("application-signals", "Amazon CloudWatch Application Signals"),
    ("appmesh", "AWS App Mesh"),
    ("apprunner", "AWS App Runner"),
    ("appstream2", "Amazon AppStream 2.0"),
    ("appstudio", "AWS App Studio"),
    ("appsync", "AWS AppSync"),
    ("athena", "Amazon Athena"),
    ("auditmanager", "AWS Audit Manager"),
    ("autoscaling", "Amazon EC2 Auto Scaling"),
    ("autoscaling-plans", "AWS Auto Scaling"),
    ("aws-marketplace", "AWS Marketplace"),
    ("b2bi", "AWS B2B Data Interchange"),
    ("backup", "AWS Backup"),
    ("batch", "AWS Batch"),
    ("bedrock", "Amazon Bedrock"),
    ("braket", "Amazon Braket"),
    ("cases", "Amazon Connect Cases"),
    ("cassandra", "Amazon Keyspaces (for Apache Cassandra)"),
    ("chime", "Amazon Chime"),
    ("cleanrooms", "AWS Clean Rooms"),
    ("cloud9", "AWS Cloud9"),
    ("cloudformation", "AWS CloudFormation"),
    ("cloudfront", "Amazon CloudFront"),
    ("cloudhsm", "AWS CloudHSM"),
    ("cloudsearch", "Amazon CloudSearch"),
    ("cloudshell", "AWS CloudShell"),
    ("cloudtrail", "AWS CloudTrail"),
    ("cloudwatch", "Amazon CloudWatch"),
    ("codeartifact", "AWS CodeArtifact"),
    ("codebuild", "AWS CodeBuild"),
    ("codecatalyst", "Amazon CodeCatalyst"),
    ("codecommit", "AWS CodeCommit"),
    ("codedeploy", "AWS CodeDeploy"),
    ("codeguru-profiler", "Amazon CodeGuru Profiler"),
    ("codeguru-reviewer", "Amazon CodeGuru Reviewer"),
    ("codepipeline", "AWS CodePipeline"),
    ("cognito-identity", "Amazon Cognito Federated Identities"),
    ("cognito-idp", "Amazon Cognito User Pools"),
    ("cognito-sync", "Amazon Cognito Sync"),
    ("comprehend", "Amazon Comprehend"),
    ("comprehendmedical", "Amazon Comprehend Medical"),
    ("config", "AWS Config"),
    ("connect", "Amazon Connect"),
    ("connect-campaigns", "Amazon Connect Outbound Campaigns"),
    ("controltower", "AWS Control Tower"),
    ("databrew", "AWS Glue DataBrew"),
    ("dataexchange", "AWS Data Exchange"),
    ("datapipeline", "AWS Data Pipeline"),
    ("datasync", "AWS DataSync"),
    ("dax", "Amazon DynamoDB Accelerator (DAX)"),
    ("deepracer", "AWS DeepRacer"),
    ("detective", "Amazon Detective"),
    ("devicefarm", "AWS Device Farm"),
    ("directconnect", "AWS Direct Connect"),
    ("discovery", "AWS Application Discovery Service"),
    ("dlm", "Amazon Data Lifecycle Manager"),
    ("dms", "AWS Database Migration Service"),
    ("docdb", "Amazon DocumentDB (with MongoDB compatibility)"),
    ("docdb-elastic", "Amazon DocumentDB Elastic Clusters"),
    ("drs", "AWS Elastic Disaster Recovery"),
    ("ds", "AWS Directory Service"),
    ("dsql", "Amazon Aurora DSQL"),
    ("dynamodb", "Amazon DynamoDB"),
    ("ebs", "Amazon Elastic Block Store (Amazon EBS)"),
    ("ec2", "Amazon Elastic Compute Cloud (Amazon EC2)"),
    ("ec2-ipam", "Amazon VPC IP Address Manager"),
    ("ec2fastlaunch", "Amazon EC2 Fast Launch"),
    ("ecr", "Amazon Elastic Container Registry (Amazon ECR)"),
    ("ecs", "Amazon Elastic Container Service (Amazon ECS)"),
    ("eks", "Amazon Elastic Kubernetes Service (Amazon EKS)"),
    ("elastic-inference", "Amazon Elastic Inference"),
    ("elasticache", "Amazon ElastiCache"),
    ("elasticbeanstalk", "AWS Elastic Beanstalk"),
    ("elasticfilesystem", "Amazon Elastic File System (Amazon EFS)"),
    ("elasticloadbalancing", "Elastic Load Balancing (ELB)"),
    ("elasticmapreduce", "Amazon EMR"),
    ("elastictranscoder", "Amazon Elastic Transcoder"),
    ("emr-containers", "Amazon EMR on EKS"),
    ("emr-serverless", "Amazon EMR Serverless"),
    ("entityresolution", "AWS Entity Resolution"),
    ("es", "Amazon OpenSearch Service"),
    ("events", "Amazon EventBridge"),
    ("evidently", "Amazon CloudWatch Evidently"),
    ("evs", "Amazon Elastic VMware Service"),
    ("fargate", "AWS Fargate"),
    ("firehose", "Amazon Data Firehose"),
    ("fms", "AWS Firewall Manager"),
    ("forecast", "Amazon Forecast"),
    ("frauddetector", "Amazon Fraud Detector"),
    ("fsx", "Amazon FSx"),
    ("gamelift", "Amazon GameLift"),
    ("gameliftstreams", "Amazon GameLift Streams"),
    ("glacier", "Amazon S3 Glacier"),
    ("globalaccelerator", "AWS Global Accelerator"),
    ("glue", "AWS Glue"),
    ("grafana", "Amazon Managed Grafana"),
    ("greengrass", "AWS IoT Greengrass"),
    ("groundstation", "AWS Ground Station"),
    ("guardduty", "Amazon GuardDuty"),
    ("healthlake", "AWS HealthLake"),
    ("iam", "AWS Identity and Access Management (IAM)"),
    ("imagebuilder", "EC2 Image Builder"),
    ("inspector", "Amazon Inspector Classic"),
    ("inspector2", "Amazon Inspector"),
    ("internetmonitor", "Amazon CloudWatch Internet Monitor"),
    ("iot", "AWS IoT Core"),
    ("iotanalytics", "AWS IoT Analytics"),
    ("iotdeviceadvisor", "AWS IoT Core Device Advisor"),
    ("iotevents", "AWS IoT Events"),
    ("iotsitewise", "AWS IoT SiteWise"),
    ("iotthingsgraph", "AWS IoT Things Graph"),
    ("iottwinmaker", "AWS IoT TwinMaker"),
    ("ivs", "Amazon Interactive Video Service"),
    ("ivschat", "Amazon IVS Chat"),
    ("kafka", "Amazon Managed Streaming for Apache Kafka (Amazon MSK)"),
    ("kafkaconnect", "Amazon MSK Connect"),
    ("kendra", "Amazon Kendra"),
    ("kendra-ranking", "Amazon Kendra Intelligent Ranking"),
    ("kinesis", "Amazon Kinesis Data Streams"),
    ("kinesisanalytics", "Amazon Managed Service for Apache Flink"),
    ("kinesisvideo", "Amazon Kinesis Video Streams"),
    ("kms", "AWS Key Management Service (AWS KMS)"),
    ("lakeformation", "AWS Lake Formation"),
    ("lambda", "AWS Lambda"),
    ("launchwizard", "AWS Launch Wizard"),
    ("lex", "Amazon Lex"),
    ("license-manager", "AWS License Manager"),
    (
        "license-manager-linux-subscriptions",
        "AWS License Manager Linux Subscriptions",
    ),
    (
        "license-manager-user-subscriptions",
        "AWS License Manager User Subscriptions",
    ),
    ("logs", "Amazon CloudWatch Logs"),
    ("lookoutvision", "Amazon Lookout for Vision"),
    ("m2", "AWS Mainframe Modernization"),
    ("machinelearning", "Amazon Machine Learning"),
    ("macie2", "Amazon Macie"),
    ("marketplaceagreement", "AWS Marketplace Agreement Service"),
    ("mediaconnect", "AWS Elemental MediaConnect"),
    ("mediaconvert", "AWS Elemental MediaConvert"),
    ("medialive", "AWS Elemental MediaLive"),
    ("mediapackage", "AWS Elemental MediaPackage"),
    ("mediapackagev2", "AWS Elemental MediaPackage v2"),
    ("mediastore", "AWS Elemental MediaStore"),
    ("mediatailor", "AWS Elemental MediaTailor"),
    ("medical-imaging", "AWS HealthImaging"),
    ("memorydb", "Amazon MemoryDB"),
    ("mgn", "AWS Application Migration Service"),
    ("migrationhuborchestrator", "AWS Migration Hub Orchestrator"),
    (
        "migrationhubstrategy",
        "AWS Migration Hub Strategy Recommendations",
    ),
    ("monitoring", "Amazon CloudWatch"),
    ("mq", "Amazon MQ"),
    ("neptune", "Amazon Neptune"),
    ("neptune-graph", "Amazon Neptune Analytics"),
    ("network-firewall", "AWS Network Firewall"),
    ("networkmanager", "AWS Cloud WAN"),
    ("networkmonitor", "Amazon CloudWatch Network Monitor"),
    ("oam", "Amazon CloudWatch Observability Access Manager"),
    (
        "observabilityadmin",
        "Amazon CloudWatch Observability Admin Service",
    ),
    ("omics", "AWS HealthOmics"),
    ("opensearch", "Amazon OpenSearch Service"),
    ("organizations", "AWS Organizations"),
    ("outposts", "AWS Outposts"),
    ("panorama", "AWS Panorama"),
    ("payment-cryptography", "AWS Payment Cryptography"),
    (
        "pca-connector-ad",
        "AWS Private CA Connector for Active Directory",
    ),
    ("pca-connector-scep", "AWS Private CA Connector for SCEP"),
    ("pcs", "AWS Parallel Computing Service"),
    ("personalize", "Amazon Personalize"),
    ("pinpoint", "Amazon Pinpoint"),
    ("polly", "Amazon Polly"),
    ("private-networks", "AWS Private 5G"),
    ("profile", "Amazon Connect Customer Profiles"),
    ("proton", "AWS Proton"),
    ("qapps", "Amazon Q Apps"),
    ("qbusiness", "Amazon Q Business"),
    ("qldb", "Amazon Quantum Ledger Database (Amazon QLDB)"),
    ("quicksight", "Amazon QuickSight"),
    ("ram", "AWS Resource Access Manager"),
    ("rbin", "Amazon Recycle Bin"),
    ("rds", "Amazon Relational Database Service (Amazon RDS)"),
    ("redshift", "Amazon Redshift"),
    ("redshift-serverless", "Amazon Redshift Serverless"),
    ("refactor-spaces", "AWS Migration Hub Refactor Spaces"),
    ("rekognition", "Amazon Rekognition"),
    ("repostspace", "AWS re:Post Private"),
    ("resource-explorer-2", "AWS Resource Explorer"),
    ("resource-groups", "AWS Resource Groups"),
    ("robomaker", "AWS RoboMaker"),
    ("rolesanywhere", "AWS IAM Roles Anywhere"),
    ("route53", "Amazon Route 53"),
    (
        "route53-recovery-control-config",
        "Amazon Route 53 Application Recovery Controller",
    ),
    (
        "route53-recovery-readiness",
        "Amazon Route 53 Application Recovery Controller Readiness",
    ),
    ("route53domains", "Amazon Route 53 Domains"),
    ("route53profiles", "Amazon Route 53 Profiles"),
    ("route53resolver", "Amazon Route 53 Resolver"),
    ("rtbfabric", "AWS RTB Fabric"),
    ("rum", "Amazon CloudWatch RUM"),
    ("s3", "Amazon Simple Storage Service (Amazon S3)"),
    ("s3-outposts", "Amazon S3 on Outposts"),
    ("sagemaker", "Amazon SageMaker AI"),
    ("scheduler", "Amazon EventBridge Scheduler"),
    ("schemas", "Amazon EventBridge Schemas"),
    ("scn", "AWS Supply Chain"),
    ("sdb", "Amazon SimpleDB"),
    ("secretsmanager", "AWS Secrets Manager"),
    ("security-ir", "AWS Security Incident Response"),
    ("securityhub", "AWS Security Hub"),
    ("serverlessrepo", "AWS Serverless Application Repository"),
    ("servicecatalog", "AWS Service Catalog"),
    ("servicediscovery", "AWS Cloud Map Service Discovery"),
    ("servicequotas", "Service Quotas"),
    ("ses", "Amazon Simple Email Service (Amazon SES)"),
    ("shield", "AWS Shield Advanced"),
    ("signer", "AWS Signer"),
    ("simspaceweaver", "AWS SimSpace Weaver"),
    ("sns", "Amazon Simple Notification Service (Amazon SNS)"),
    ("sqs", "Amazon Simple Queue Service (Amazon SQS)"),
    ("ssm", "AWS Systems Manager"),
    ("ssm-contacts", "AWS Systems Manager Incident Manager Contacts"),
    ("ssm-guiconnect", "AWS Systems Manager GUI Connect"),
    ("ssm-incidents", "AWS Systems Manager Incident Manager"),
    ("ssm-quicksetup", "AWS Systems Manager Quick Setup"),
    ("ssm-sap", "AWS Systems Manager for SAP"),
    ("sso", "AWS IAM Identity Center"),
    ("states", "AWS Step Functions"),
    ("storagegateway", "AWS Storage Gateway"),
    ("sts", "AWS Security Token Service (AWS STS)"),
    ("supportapp", "AWS Support App in Slack"),
    ("swf", "Amazon Simple Workflow Service"),
    ("synthetics", "Amazon CloudWatch Synthetics"),
    ("textract", "Amazon Textract"),
    ("timestream", "Amazon Timestream"),
    ("transcribe", "Amazon Transcribe"),
    ("transfer", "AWS Transfer Family"),
    ("translate", "Amazon Translate"),
    ("trustedadvisor", "AWS Trusted Advisor"),
    ("verifiedpermissions", "Amazon Verified Permissions"),
    ("vpc", "Amazon Virtual Private Cloud (Amazon VPC)"),
    ("vpc-lattice", "Amazon VPC Lattice"),
    ("waf", "AWS WAF Classic"),
    ("waf-regional", "AWS WAF Classic Regional"),
    ("wafv2", "AWS WAF"),
    ("wellarchitected", "AWS Well-Architected Tool"),
    ("wisdom", "Amazon Connect Wisdom"),
    ("workdocs", "Amazon WorkDocs"),
    ("workmail", "Amazon WorkMail"),
    ("workspaces", "Amazon WorkSpaces"),
    ("workspaces-web", "Amazon WorkSpaces Secure Browser"),
    ("xray", "AWS X-Ray"),
    // AWS's catalog reports these service codes too, but this emulator has no
    // published display name it can cite for them (they are recent launches). The
    // code is reported as the name rather than inventing one: `ServiceCode` — the
    // member every other operation keys on, and the one the model constrains — stays
    // exactly AWS's, and nothing downstream reads `ServiceName`.
    ("account-access", "account-access"),
    ("aco-automation", "aco-automation"),
    ("agent-registry", "agent-registry"),
    ("aidevops", "aidevops"),
    ("aiops", "aiops"),
    ("airflow-serverless", "airflow-serverless"),
    ("aws-mcp", "aws-mcp"),
    ("axis", "axis"),
    ("bedrock-websearch", "bedrock-websearch"),
    ("cloudwatchpredictions", "cloudwatchpredictions"),
    ("eks-mcp", "eks-mcp"),
    ("elemental-inference", "elemental-inference"),
    ("health-agent", "health-agent"),
    ("interconnect", "interconnect"),
    ("networkinsights", "networkinsights"),
    ("s3files", "s3files"),
    ("securityagent", "securityagent"),
];

/// The seeded service catalog: [`SEED_SERVICE_CATALOG`], keyed by `ServiceCode` so
/// `ListServices` pages it in code order and a lookup is exact.
///
/// A service the quota catalog carries rows for is listed too, under the name that
/// table gives it: a service whose quotas this emulator publishes must appear in the
/// listing a client discovers service codes from, or those quotas are unreachable.
/// `SEED_SERVICE_CATALOG` is the authority on the name wherever both name a service.
fn seed_services() -> BTreeMap<String, String> {
    let mut services: BTreeMap<String, String> = SEED_SERVICE_CATALOG
        .iter()
        .map(|(code, name)| ((*code).to_string(), (*name).to_string()))
        .collect();
    for entry in seed_quota_catalog() {
        services
            .entry(entry.code.to_string())
            .or_insert_with(|| entry.name.to_string());
    }
    services
}

/// EC2's account-wide resource quotas: the published EC2 quotas that are neither
/// vCPU quotas nor Dedicated Hosts quotas.
fn ec2_resource_quotas() -> Vec<QuotaRow> {
    vec![
        quota("EC2-VPC Elastic IPs", 5.0).code("L-0263D0A3"),
        quota("AMI sharing", 500.0),
        quota("Launch templates per Region", 5000.0),
        quota("Versions per launch template", 10000.0),
        quota("Key pairs per Region", 5000.0),
        quota("Placement groups per Region", 500.0),
        quota("Partitions per partition placement group", 7.0).fixed(),
        quota("Active Spot Fleet requests per Region", 1000.0),
        quota("Target capacity per Spot Fleet request", 5000.0),
        quota("Active EC2 Fleets per Region", 1000.0),
        quota("Target capacity per EC2 Fleet", 5000.0),
        quota("Attributes per launch template", 300.0).fixed(),
    ]
}

/// Amazon VPC's published quota catalog.
fn vpc_quotas() -> Vec<QuotaRow> {
    vec![
        // The recorded AWS response for this quota carries all three of the optional
        // members a `ServiceQuota` can publish: a description, the CloudWatch usage
        // metric, and a `Period`.
        quota("VPCs per Region", 5.0)
            .code("L-F678F1CE")
            // The VPC resource counts CloudWatch publishes sit under the EC2 service
            // dimension — VPC is served by the EC2 API — which is the dimension the
            // recorded response for this quota carries.
            .metric(usage_metric(
                "EC2",
                &[("Class", "None"), ("Resource", "VPC"), ("Type", "Resource")],
            ))
            .period(5, "MINUTE")
            .describe("The maximum number of VPCs per Region in this account."),
        quota("Internet gateways per Region", 5.0).code("L-A4707A72"),
        quota("Network interfaces per Region", 5000.0).code("L-045E6E9C"),
        // AWS publishes this one as not adjustable.
        quota("IPv6 CIDR blocks per VPC", 1.0).code("L-085A6257").fixed(),
        quota("Subnets per VPC", 200.0),
        quota("Security groups per VPC", 2500.0),
        quota("Inbound or outbound rules per security group", 60.0),
        quota("Security groups per network interface", 5.0),
        quota("Route tables per VPC", 200.0),
        quota("Routes per route table", 50.0),
        quota("Network ACLs per VPC", 200.0),
        quota("Rules per network ACL", 20.0),
        quota("NAT gateways per Availability Zone", 5.0),
        quota("Egress-only internet gateways per Region", 5.0),
        quota("Active VPC peering connections per VPC", 50.0),
        quota("Gateway VPC endpoints per Region", 20.0),
        quota("Interface VPC endpoints per VPC", 50.0),
        quota("Prefix lists per Region", 100.0),
    ]
}

/// A `UsageMetric` in the `AWS/Usage` namespace, the one Service Quotas reports for
/// a quota CloudWatch publishes usage for.
fn usage_metric(service: &str, dimensions: &[(&str, &str)]) -> MetricInfo {
    let mut metric_dimensions = std::collections::HashMap::new();
    metric_dimensions.insert("Service".to_string(), service.to_string());
    for (name, value) in dimensions {
        metric_dimensions.insert((*name).to_string(), (*value).to_string());
    }
    MetricInfo {
        metric_namespace: Some("AWS/Usage".to_string()),
        metric_name: Some("ResourceCount".to_string()),
        metric_dimensions: Some(metric_dimensions),
        metric_statistic_recommendation: Some("Maximum".to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicequotasServicequotasv20190624State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicequotasServicequotasv20190624State>)
}

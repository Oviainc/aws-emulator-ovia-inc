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
//!   token keeps meaning the same position.
//!
//! The overlay and the requests are per region (Service Quotas is a regional service:
//! an increase applied in one region does not change another's value), while the
//! catalog and the defaults are the same everywhere. The account is the emulator's
//! single [`engine::emulator_account_id`].

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use base64::Engine as _;

use crate::ops::types::{MetricInfo, QuotaPeriod, RequestedServiceQuotaChange, ServiceInfo, ServiceQuota};

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

/// `RequestStatus` member `PENDING`: the status a newly minted request carries.
pub const STATUS_PENDING: &str = "PENDING";
/// `RequestStatus` member `APPROVED`: the status a decided request carries once the
/// increase has been applied to the quota.
pub const STATUS_APPROVED: &str = "APPROVED";

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

/// One AWS-default quota, as `GetAWSDefaultServiceQuota` reports it.
///
/// Seeded from AWS's published Service Quotas defaults (the per-service "Service
/// quotas" pages) and never mutated: an increase writes [`AppliedQuota`] instead, so
/// the default a caller reads after an increase is the same one it read before.
#[derive(Debug, Clone)]
pub struct DefaultQuota {
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
    /// `Period`, for a rate quota (requests per unit of time).
    pub period: Option<QuotaPeriod>,
    /// `Description`, when AWS publishes one.
    pub description: Option<String>,
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
    /// `QuotaRequestedAtLevel` — an `AppliedLevelEnum` member.
    pub quota_requested_at_level: String,
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
    /// `(ServiceCode, QuotaCode)` -> the AWS default quota. Never mutated.
    default_quotas: BTreeMap<(String, String), DefaultQuota>,
    /// The applied overlay of the quotas AWS counts ACCOUNT-WIDE (`GlobalQuota`).
    /// A global quota has one value for the whole account — which is why its ARN
    /// carries no region — so an approved increase on it is in force at every
    /// regional endpoint, not just the one it was requested at.
    global_applied_quotas: BTreeMap<(String, String), AppliedQuota>,
    /// The mutable state, per region.
    regions: BTreeMap<String, RegionState>,
}

impl Default for ComAmazonawsServicequotasServicequotasv20190624State {
    fn default() -> Self {
        Self {
            services: seed_services(),
            default_quotas: seed_default_quotas(),
            global_applied_quotas: BTreeMap::new(),
            regions: BTreeMap::new(),
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

    /// Whether the catalog knows `service_code`. Codes are matched exactly: AWS's
    /// service codes are lowercase and `VPC` is not `vpc`.
    pub fn knows_service(&self, service_code: &str) -> bool {
        self.services.contains_key(service_code)
    }

    /// The `ServiceName` for a `ServiceCode`.
    pub fn service_name(&self, service_code: &str) -> Option<&str> {
        self.services.get(service_code).map(String::as_str)
    }

    /// The AWS default quota for a `(ServiceCode, QuotaCode)` pair.
    pub fn default_quota(&self, service_code: &str, quota_code: &str) -> Option<&DefaultQuota> {
        self.default_quotas
            .get(&(service_code.to_string(), quota_code.to_string()))
    }

    /// Every `QuotaCode` the service has a default quota for, in `QuotaCode` order.
    pub fn quota_codes(&self, service_code: &str) -> Vec<String> {
        self.default_quotas
            .keys()
            .filter(|(code, _)| code == service_code)
            .map(|(_, quota)| quota.clone())
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
        let mut quota = self.quota_view(region, service_code, quota_code, default);
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
        let mut quota = self.quota_view(region, service_code, quota_code, default);
        quota.quota_arn = Some(default_quota_arn(
            region,
            service_code,
            quota_code,
            default.global_quota,
        ));
        Some(quota)
    }

    /// Mint and store a quota increase request, and return the stored record —
    /// `None` when this region already holds [`MAX_REQUESTS_PER_REGION`] of them.
    ///
    /// The caller has already checked that the quota exists, that it is adjustable,
    /// and that `desired_value` is above the value in force. The ceiling is enforced
    /// HERE, at the one insertion point, so no caller can grow the store past it.
    pub fn record_request(
        &mut self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        desired_value: f64,
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
    /// A `PENDING` request whose `decided_at` has passed is `APPROVED` and its
    /// desired value becomes the quota's applied value. This is the ONLY writer of
    /// the applied overlay, and it is owed at `decided_at` whether or not anybody
    /// calls — so a reporting operation may settle it before answering, and reading
    /// twice yields the same answer.
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

        // The decision: flip the stored requests, and note the overlay each one puts
        // in force.
        let mut decided: Vec<(String, String, AppliedQuota)> = Vec::new();
        if let Some(state) = self.regions.get_mut(region) {
            for id in due {
                let Some(request) = state.quota_requests.get_mut(&id) else {
                    continue;
                };
                request.status = STATUS_APPROVED.to_string();
                // The decision happened when it came due, not when somebody looked.
                request.last_updated = request.decided_at;
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
    fn quota_view(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        default: &DefaultQuota,
    ) -> ServiceQuota {
        ServiceQuota {
            service_code: Some(service_code.to_string()),
            service_name: self.service_name(service_code).map(str::to_string),
            quota_arn: Some(quota_arn(region, service_code, quota_code, default.global_quota)),
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

/// The identity a request records as its `Requester`: this emulator's single
/// account's root identity, built through the canonical ARN helper.
pub fn requester_identity() -> String {
    ::engine::emulator_arn("iam", "", "root")
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
fn seed_services() -> BTreeMap<String, String> {
    SEED_SERVICE_CATALOG
        .iter()
        .map(|(code, name)| ((*code).to_string(), (*name).to_string()))
        .collect()
}

/// The seeded AWS default quotas, keyed by `(ServiceCode, QuotaCode)`.
///
/// Quota codes, names, default values and adjustability come from AWS's published
/// per-service quota pages (the same values `aws service-quotas
/// get-aws-default-service-quota` reports). `Unit` is `None` for a countable quota,
/// which is what Service Quotas reports for all but the measured ones.
///
/// Coverage is per PAIR, not per service: this table holds the published defaults
/// this emulator can cite, and nothing is invented to fill a service out. A service
/// code in [`SEED_SERVICE_CATALOG`] with no pair here therefore has an EMPTY default
/// quota set, and `ListAWSDefaultServiceQuotas` answers it with HTTP 200 and an empty
/// `Quotas` list — the code exists, so `NoSuchResourceException` (which belongs to a
/// code that is not an AWS service code at all) would be the wrong answer. Every
/// `ServiceCode` used here must be a catalog code; `tests/all/default_quota_catalog.rs`
/// pins that.
fn seed_default_quotas() -> BTreeMap<(String, String), DefaultQuota> {
    let mut quotas = BTreeMap::new();
    let mut add = |service_code: &str, quota_code: &str, quota: DefaultQuota| {
        quotas.insert((service_code.to_string(), quota_code.to_string()), quota);
    };

    add(
        "dynamodb",
        "L-F98FE922",
        DefaultQuota {
            quota_name: "Maximum number of tables".to_string(),
            value: 2500.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            // DynamoDB publishes table count as a CloudWatch usage metric
            // (`AWS/Usage`, `ResourceCount`, `Resource=TableCount`), which is what
            // Service Quotas reports as this quota's `UsageMetric`.
            usage_metric: Some(usage_metric(
                "DynamoDB",
                &[
                    ("Class", "None"),
                    ("Resource", "TableCount"),
                    ("Type", "Resource"),
                ],
            )),
            period: None,
            description: Some(
                "The maximum number of tables that can be created in this account in the \
                 current Region."
                    .to_string(),
            ),
        },
    );
    add(
        "ec2",
        "L-0263D0A3",
        DefaultQuota {
            quota_name: "EC2-VPC Elastic IPs".to_string(),
            value: 5.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );
    add(
        "ec2",
        "L-1216C47A",
        DefaultQuota {
            quota_name: "Running On-Demand Standard (A, C, D, H, I, M, R, T, Z) instances"
                .to_string(),
            value: 5.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            // The CloudWatch usage metric AWS publishes for the vCPU-based
            // On-Demand instance quotas.
            usage_metric: Some(usage_metric(
                "EC2",
                &[("Class", "Standard/OnDemand"), ("Resource", "vCPU"), ("Type", "Resource")],
            )),
            period: None,
            description: None,
        },
    );
    add(
        "lambda",
        "L-2ACBD22F",
        DefaultQuota {
            quota_name: "Function and layer storage".to_string(),
            value: 75.0,
            unit: "Gigabytes".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );
    add(
        "lambda",
        "L-B99A9384",
        DefaultQuota {
            quota_name: "Concurrent executions".to_string(),
            value: 1000.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: Some(usage_metric(
                "Lambda",
                &[
                    ("Class", "None"),
                    ("Resource", "ConcurrentExecutions"),
                    ("Type", "Resource"),
                ],
            )),
            period: None,
            description: None,
        },
    );
    add(
        "s3",
        "L-DC2B2D3D",
        DefaultQuota {
            quota_name: "General purpose buckets".to_string(),
            value: 10000.0,
            unit: "None".to_string(),
            adjustable: true,
            // Buckets are counted per account across every region.
            global_quota: true,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );
    add(
        "vpc",
        "L-045E6E9C",
        DefaultQuota {
            quota_name: "Network interfaces per Region".to_string(),
            value: 5000.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );
    add(
        "vpc",
        "L-085A6257",
        DefaultQuota {
            quota_name: "IPv6 CIDR blocks per VPC".to_string(),
            value: 1.0,
            unit: "None".to_string(),
            // AWS publishes this one as not adjustable.
            adjustable: false,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );
    add(
        "vpc",
        "L-A4707A72",
        DefaultQuota {
            quota_name: "Internet gateways per Region".to_string(),
            value: 5.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );
    add(
        "vpc",
        "L-F678F1CE",
        DefaultQuota {
            quota_name: "VPCs per Region".to_string(),
            value: 5.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: LEVEL_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        },
    );

    quotas
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

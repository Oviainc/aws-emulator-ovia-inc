// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-billing-awsbilling service.
//!
//! The service's primary resource is the **billing view**. Every one of the five
//! implemented lifecycle operations (Create/Get/List/Update/DeleteBillingView)
//! computes its answer from [`ComAmazonawsBillingAwsbillingState::billing_views`],
//! which is keyed by the view's ARN and ordered, so a list and its pagination are
//! deterministic.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::Expression;

/// `CUSTOM`, the `BillingViewType` a view created through `CreateBillingView`
/// carries (one of `BILLING_VIEW_TYPE_VALUES` in `ops::types`).
pub const BILLING_VIEW_TYPE_CUSTOM: &str = "CUSTOM";

/// `PRIMARY`, the `BillingViewType` of the view every account owns by default.
///
/// From the `ListBillingViews` doc: "Every Amazon Web Services account has a
/// unique `PRIMARY` billing view that represents the billing data available by
/// default."
pub const BILLING_VIEW_TYPE_PRIMARY: &str = "PRIMARY";

/// `HEALTHY`, a `BillingViewStatus` value (`BILLING_VIEW_STATUS_VALUES`).
pub const BILLING_VIEW_STATUS_HEALTHY: &str = "HEALTHY";

/// `STARTS_WITH`, the only `SearchOption` the model declares, used by the
/// `ListBillingViews` `names` filter.
pub const SEARCH_OPTION_STARTS_WITH: &str = "STARTS_WITH";

/// The `resourceType` the modeled error shapes report for a billing view.
pub const BILLING_VIEW_RESOURCE_TYPE: &str = "billingview";

/// The most billing views this emulator keeps resident.
///
/// `billing_views` grows with every `CreateBillingView` call, so it needs a
/// ceiling: without one a caller's request rate is the only limit on the memory
/// of a process that also serves every other emulated service. This is an
/// emulator resource bound, not a published AWS quota; `CreateBillingView`
/// refuses past it with the modeled `ServiceQuotaExceededException` ("You've
/// reached the limit of resources you can create").
pub const MAX_BILLING_VIEWS: usize = 256;

/// The most `clientToken` -> ARN entries this emulator keeps resident.
///
/// An entry is written only beside a view it names and removed when that view is
/// deleted, so the map stays no larger than `billing_views`; the cap is checked
/// anyway, so a future caller of the store cannot grow it past a known bound.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_BILLING_VIEWS;

/// The `serviceCode` the modeled `ServiceQuotaExceededException` reports.
pub const BILLING_SERVICE_CODE: &str = "billing";

/// The `quotaCode` the modeled `ServiceQuotaExceededException` reports for the
/// billing-view count.
pub const BILLING_VIEWS_QUOTA_CODE: &str = "BillingViewsPerAccount";

/// The resource id of the account's default `PRIMARY` billing view.
const PRIMARY_VIEW_ID: &str = "primary";

/// The name AWS gives the account's default `PRIMARY` billing view.
const PRIMARY_VIEW_NAME: &str = "Primary View";

/// One persisted billing view.
///
/// `sourceViewCount` and `derivedViewCount` are NOT fields: the first is
/// `source_views.len()` and the second is the number of views whose
/// `source_views` name this one, both derived on read so neither can go stale.
#[derive(Debug, Clone)]
pub struct BillingView {
    /// The ARN this view is keyed by, minted by [`ComAmazonawsBillingAwsbillingState::mint_view_arn`].
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// A `BillingViewType` value, e.g. [`BILLING_VIEW_TYPE_CUSTOM`].
    pub billing_view_type: String,
    /// The 12-digit account that owns the view.
    pub owner_account_id: String,
    /// The account owning the source view, for a derived view.
    pub source_account_id: Option<String>,
    /// The billing-view ARNs this view draws its data from.
    pub source_views: Vec<String>,
    pub data_filter_expression: Option<Expression>,
    pub created_at: SystemTime,
    pub updated_at: Option<SystemTime>,
    pub view_definition_last_updated_at: Option<SystemTime>,
    /// A `BillingViewStatus` value, e.g. [`BILLING_VIEW_STATUS_HEALTHY`].
    pub health_status_code: String,
    /// The view's resource tags, by key.
    pub tags: BTreeMap<String, String>,
}

/// Per-service state for com-amazonaws-billing-awsbilling. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBillingAwsbillingState {
    /// Every persisted billing view, keyed and ordered by ARN. Bounded by
    /// [`MAX_BILLING_VIEWS`].
    pub billing_views: BTreeMap<String, BillingView>,
    /// `clientToken` -> the ARN its first `CreateBillingView` minted, so a
    /// replayed create returns that ARN instead of making a second view.
    /// Bounded by [`MAX_IDEMPOTENCY_TOKENS`]: an entry is reclaimed by
    /// [`Self::forget_view`] when its view is deleted.
    pub idempotency_tokens: HashMap<String, String>,
    /// Source of the next `billingview/custom-<n>` resource id. Used ONLY to
    /// build an ARN that is then stored in `billing_views`.
    pub next_view_id: u64,
}

impl Default for ComAmazonawsBillingAwsbillingState {
    fn default() -> Self {
        // The account's default PRIMARY view exists before any API call: see
        // BILLING_VIEW_TYPE_PRIMARY for the `ListBillingViews` doc sentence.
        let now = SystemTime::now();
        let primary = BillingView {
            arn: billing_view_arn(PRIMARY_VIEW_ID),
            name: PRIMARY_VIEW_NAME.to_string(),
            description: None,
            billing_view_type: BILLING_VIEW_TYPE_PRIMARY.to_string(),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            source_account_id: None,
            source_views: Vec::new(),
            data_filter_expression: None,
            created_at: now,
            updated_at: None,
            view_definition_last_updated_at: None,
            health_status_code: BILLING_VIEW_STATUS_HEALTHY.to_string(),
            tags: BTreeMap::new(),
        };
        let mut billing_views = BTreeMap::new();
        billing_views.insert(primary.arn.clone(), primary);
        Self {
            billing_views,
            idempotency_tokens: HashMap::new(),
            next_view_id: 1,
        }
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// Mint an unused billing-view ARN and advance the id source.
    ///
    /// The returned ARN is not stored by this call — the caller inserts the
    /// record it belongs to.
    pub fn mint_view_arn(&mut self) -> String {
        loop {
            let arn = billing_view_arn(&format!("custom-{}", self.next_view_id));
            self.next_view_id += 1;
            if !self.billing_views.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// The ARNs of the views that use `arn` as one of their source views —
    /// the resources a delete would break, and the count `derivedViewCount`
    /// reports.
    pub fn derived_view_arns(&self, arn: &str) -> Vec<String> {
        self.billing_views
            .values()
            .filter(|view| view.source_views.iter().any(|source| source == arn))
            .map(|view| view.arn.clone())
            .collect()
    }

    /// Drop the view at `arn` and every idempotency token that named it, so a
    /// create/delete loop reclaims everything it allocated instead of leaving
    /// entries behind that no API can observe or free.
    pub fn forget_view(&mut self, arn: &str) -> Option<BillingView> {
        let removed = self.billing_views.remove(arn);
        self.idempotency_tokens.retain(|_, stored| stored != arn);
        removed
    }

    /// Whether another view fits under [`MAX_BILLING_VIEWS`], counting the
    /// idempotency entry it would write against [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn has_room_for_another_view(&self, writes_token: bool) -> bool {
        self.billing_views.len() < MAX_BILLING_VIEWS
            && (!writes_token || self.idempotency_tokens.len() < MAX_IDEMPOTENCY_TOKENS)
    }
}

/// The ARN of the billing view with resource id `view_id`, in the modeled
/// `arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/…` form. Billing is a global
/// service, so the region segment is empty.
pub fn billing_view_arn(view_id: &str) -> String {
    ::engine::emulator_arn("billing", "", &format!("billingview/{view_id}"))
}

/// A stored instant as the modeled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(epoch_secs(at))
}

/// A stored instant as seconds since the epoch, for comparing against a
/// request's `Timestamp`.
pub fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// A request `Timestamp` as seconds since the epoch, on the same scale as
/// [`epoch_secs`].
pub fn date_time_secs(at: &::aws_smithy_types::DateTime) -> f64 {
    at.secs() as f64 + f64::from(at.subsec_nanos()) / 1_000_000_000.0
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBillingAwsbillingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBillingAwsbillingState>)
}

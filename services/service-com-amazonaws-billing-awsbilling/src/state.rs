// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-billing-awsbilling service.
//!
//! The service's primary resource is the **billing view**. Every one of the five
//! implemented lifecycle operations (Create/Get/List/Update/DeleteBillingView)
//! computes its answer from [`ComAmazonawsBillingAwsbillingState::billing_views`],
//! which is keyed by the view's ARN and ordered, so a list and its pagination are
//! deterministic.
//!
//! Beside it, [`ComAmazonawsBillingAwsbillingState::credits`] holds the account
//! credits `GetCredits` reports, keyed and ordered by `creditId`.

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

/// The most credits this emulator keeps resident for the whole billing family.
///
/// `credits` is the store `GetCredits` reports from; like `billing_views` it
/// needs a ceiling so that nothing can grow the process without bound. This is
/// an emulator resource bound, not a published AWS quota:
/// [`ComAmazonawsBillingAwsbillingState::insert_credit`] refuses past it.
pub const MAX_CREDITS: usize = 1024;

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

/// A monetary amount, as the modeled `Amount` shape carries it: an ISO-4217
/// `currencyCode` (`^[A-Z]{3}$`) and a decimal `currencyAmount`.
///
/// Held as the wire form rather than a float: the modeled `CurrencyAmount` is a
/// string, and money must not be rounded by a binary float on the way through
/// the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditAmount {
    pub currency_code: String,
    pub currency_amount: String,
}

/// One persisted account credit — the record `GetCredits` reports.
///
/// Mirrors the modeled `CreditData` shape, with instants kept as
/// [`SystemTime`] (converted on read by [`timestamp`]) so that stored and
/// requested periods compare on one scale. Enum-typed members are held as the
/// model's own string values (`ApplicationType`, `CreditSharingType`,
/// `CreditStatus`).
#[derive(Debug, Clone)]
pub struct Credit {
    /// The modeled `CreditId`: 1..32 characters matching `^[0-9]+$`. This is
    /// the key the credit is stored under.
    pub credit_id: String,
    /// The 12-digit account that owns the credit.
    pub account_id: String,
    pub credit_type: String,
    pub initial_amount: CreditAmount,
    pub remaining_amount: CreditAmount,
    pub estimated_amount: Option<CreditAmount>,
    /// The products the credit applies to; empty when it applies to any.
    pub applicable_product_names: Vec<String>,
    pub description: String,
    /// When the credit becomes applicable.
    pub start_date: SystemTime,
    /// When the credit expires; `None` for a credit that does not expire.
    pub end_date: Option<SystemTime>,
    /// When the credit was fully consumed, if it has been.
    pub exhaust_date: Option<SystemTime>,
    /// An `ApplicationType` value, e.g. `AFTER_DISCOUNTS`.
    pub application_type: Option<String>,
    /// The accounts the credit may be shared with.
    pub shareable_accounts: Vec<String>,
    pub account_has_credit_sharing_enabled: Option<bool>,
    pub credit_console_visibility: Option<String>,
    /// A `CreditSharingType` value, e.g. `DEFAULT`.
    pub credit_sharing_type: Option<String>,
    pub cost_category_arn: Option<String>,
    pub rule_name: Option<String>,
    /// A `CreditStatus` value: `ENABLED` or `DISABLED`.
    pub credit_status: Option<String>,
    /// The purchase types the credit applies to.
    pub purchase_type_applications: Vec<String>,
}

impl Credit {
    /// Whether this credit's own life overlaps the closed period
    /// `[start_secs, end_secs]`, both on [`epoch_secs`]' scale.
    ///
    /// A credit with no `end_date` never expires, so it overlaps every period
    /// that ends on or after it started.
    pub fn overlaps_period(&self, start_secs: f64, end_secs: f64) -> bool {
        epoch_secs(self.start_date) <= end_secs
            && self
                .end_date
                .is_none_or(|end| epoch_secs(end) >= start_secs)
    }
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
    /// Every credit held by an account of this emulator's consolidated billing
    /// family, keyed and ordered by `creditId`, so `GetCredits` answers in a
    /// deterministic order. Bounded by [`MAX_CREDITS`].
    pub credits: BTreeMap<String, Credit>,
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
            // A fresh account holds no credits: the modeled `GetCredits`
            // response "returns an empty list when no credits exist".
            credits: BTreeMap::new(),
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

    /// Store `credit` under its `creditId`, replacing any credit already held
    /// under that id. Answers `false` — storing nothing — when the store is
    /// already at [`MAX_CREDITS`], so a caller can refuse with a quota error
    /// rather than growing the process without bound.
    pub fn insert_credit(&mut self, credit: Credit) -> bool {
        if !self.credits.contains_key(&credit.credit_id) && self.credits.len() >= MAX_CREDITS {
            return false;
        }
        self.credits.insert(credit.credit_id.clone(), credit);
        true
    }

    /// The credits `GetCredits` reports, in `creditId` order.
    ///
    /// `account_id` selects the owning account. `whole_family` widens the
    /// selection to every account of the consolidated billing family — every
    /// account this store knows, since the emulator's own account
    /// (`engine::emulator_account_id`) is the family's management account.
    /// Both bounds of the requested period are inclusive, on [`epoch_secs`]'
    /// scale, and a credit is reported when its own life overlaps it.
    ///
    /// A pure read: it answers from the store and changes nothing in it.
    pub fn credits_for_period(
        &self,
        account_id: &str,
        whole_family: bool,
        start_secs: f64,
        end_secs: f64,
    ) -> Vec<Credit> {
        self.credits
            .values()
            .filter(|credit| whole_family || credit.account_id == account_id)
            .filter(|credit| credit.overlaps_period(start_secs, end_secs))
            .cloned()
            .collect()
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

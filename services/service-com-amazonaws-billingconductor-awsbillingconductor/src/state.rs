// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-billingconductor-awsbillingconductor service.
//!
//! The pricing-rule / pricing-plan / billing-group core loop is persisted here.
//! Every record is keyed by its FULL ARN, because that is the identity the wire
//! uses: the modeled `BillingGroupArn` / `PricingPlanArn` / `PricingRuleArn`
//! patterns accept either the full ARN or the bare resource id, so a handler
//! normalises its input through [`normalize_billing_group_arn`] /
//! [`normalize_pricing_plan_arn`] / [`normalize_pricing_rule_arn`] before touching
//! the maps and two spellings of one resource cannot become two records.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// This service's ARN namespace (`OPERATIONS.md`: "ARN namespace: billingconductor").
const ARN_SERVICE: &str = "billingconductor";

/// billingconductor is a global (billing) service, so its ARNs carry no region —
/// the modeled patterns have an empty region segment
/// (`arn:aws:billingconductor::<account>:billinggroup/...`).
const ARN_REGION: &str = "";

/// The pricing plans Amazon Web Services owns rather than the caller: the
/// `PricingPlanFullArn` pattern enumerates them beside the 10-character
/// customer-minted id
/// (`.../pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`), so a billing
/// group may name one without the caller ever having created it.
pub const AWS_MANAGED_PRICING_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// The alphabet the modeled resource-id patterns allow (`[a-zA-Z0-9]`).
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Wall-clock millis since the epoch — the unit the modeled `Instant` long carries
/// (`CreationTime` / `LastModifiedTime`).
pub fn now_millis() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The full ARN of a pricing plan resource id.
pub fn pricing_plan_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("pricingplan/{id}"))
}

/// The full ARN of a billing group resource id.
pub fn billing_group_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("billinggroup/{id}"))
}

/// The full ARN of a custom line item resource id.
pub fn custom_line_item_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("customlineitem/{id}"))
}

/// The full ARN of a pricing rule resource id.
pub fn pricing_rule_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("pricingrule/{id}"))
}

/// A `PricingRuleArn` as this service stores it: the pattern accepts the full ARN
/// or the bare `[a-zA-Z0-9]{10}` id, and both name the same resource. There are no
/// AWS-managed pricing rules — the `PricingRuleArn` pattern allows only a 12-digit
/// account segment — so a bare id always resolves into this account.
pub fn normalize_pricing_rule_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else {
        pricing_rule_arn(value)
    }
}

/// A `BillingGroupArn` as this service stores it: the pattern accepts the full ARN
/// or the bare `[a-zA-Z0-9]{10,12}` id, and both name the same resource.
pub fn normalize_billing_group_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else {
        billing_group_arn(value)
    }
}

/// A `CustomLineItemArn` as this service stores it: the pattern accepts the full ARN
/// or the bare `[a-zA-Z0-9]{10}` id. See [`normalize_billing_group_arn`].
pub fn normalize_custom_line_item_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else {
        custom_line_item_arn(value)
    }
}

/// How a resource associated to a percentage custom line item relates to it.
///
/// `CustomLineItemAssociationElement` admits a `billinggroup` ARN or a
/// `customlineitem` ARN, and `CustomLineItemRelationship` has exactly two values —
/// one per class: the billing group a custom line item is computed inside sits ABOVE
/// it (`PARENT`), and another custom line item whose charge feeds the percentage sits
/// BELOW it (`CHILD`). So the resource type segment of the ARN decides, and the
/// `Relationship` filter separates the two classes.
pub fn association_relationship(arn: &str) -> &'static str {
    if arn.contains(":billinggroup/") {
        "PARENT"
    } else {
        "CHILD"
    }
}

/// A `BillingPeriod` (`^\d{4}-(0?[1-9]|1[012])$`) as a comparable month count, so
/// `2026-7` and `2026-07` are the same period and two periods order by time.
/// `None` when the value is not a billing period at all.
///
/// The shape of each segment is checked HERE rather than assumed: this is the point
/// that decides whether a caller-supplied string is a billing period, so the year is
/// exactly four ASCII digits and the month one or two — which also keeps the month
/// count far inside `i64`, whatever a request carries.
pub fn billing_period_key(value: &str) -> Option<i64> {
    let (year, month) = value.split_once('-')?;
    if year.len() != 4 || !year.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    if !(1..=2).contains(&month.len()) || !month.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let year: i64 = year.parse::<u16>().ok()?.into();
    let month: i64 = month.parse::<u8>().ok()?.into();
    (1..=12).contains(&month).then_some(year * 12 + month)
}

/// A `PricingPlanArn` as this service stores it. See [`normalize_billing_group_arn`].
///
/// A bare AWS-managed id resolves to the ARN Amazon Web Services owns (account
/// segment `aws`), not to this emulator's account: the `PricingPlanFullArn` pattern
/// allows `(aws|[0-9]{12})` for exactly that reason.
pub fn normalize_pricing_plan_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else if AWS_MANAGED_PRICING_PLAN_IDS.contains(&value) {
        aws_managed_pricing_plan_arn(value)
    } else {
        pricing_plan_arn(value)
    }
}

/// The ARN of an AWS-managed pricing plan: owned by Amazon Web Services, so its
/// account segment is the literal `aws` the `PricingPlanFullArn` pattern allows
/// beside a 12-digit account.
fn aws_managed_pricing_plan_arn(id: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{ARN_REGION}:aws:pricingplan/{id}")
}

/// True when the value is one of the AWS-managed plans of
/// [`AWS_MANAGED_PRICING_PLAN_IDS`] — the WHOLE ARN has to be the one Amazon Web
/// Services owns, so another partition's or another account's ARN ending in the
/// same id is not managed and must exist in state like any other plan.
pub fn is_aws_managed_pricing_plan(arn: &str) -> bool {
    AWS_MANAGED_PRICING_PLAN_IDS
        .iter()
        .any(|id| arn == *id || arn == aws_managed_pricing_plan_arn(id))
}

/// The pricing rule type whose rates come from tiers rather than from a
/// percentage modifier. The other values of `PricingRuleType`
/// (`MARKUP`, `DISCOUNT`) price by `ModifierPercentage`.
pub const TIERING_PRICING_RULE_TYPE: &str = "TIERING";

/// The rate members a pricing rule holds: its `ModifierPercentage` and its
/// `Tiering` configuration, of which a rule ever holds exactly one.
pub type PricingRuleRate = (Option<f64>, Option<TieringRecord>);

/// A refusal a shared rule hands back for the calling operation to raise as its
/// OWN modelled `ValidationException` (each operation's error enum is its own
/// type): the reason, the member at fault and the message.
pub type ValidationRefusal = (&'static str, &'static str, String);

/// The rate members a pricing rule of `rule_type` must hold, or the refusal the
/// request earns.
///
/// A rule prices EITHER by a percentage (`MARKUP`/`DISCOUNT`) or by tiers
/// (`TIERING`), never both, which is what the reasons `ValidationExceptionReason`
/// enumerates for each of them (`ILLEGAL_MODIFIER_PERCENTAGE`,
/// `ILLEGAL_TIERING_INPUT`) refuse a rule for. So a request that NAMES the member
/// its resulting type does not price by is refused, the one the type needs has to
/// be there (named by the request or already stored), and a value only carried
/// over from storage into a new type is dropped rather than refused.
///
/// `Ok` carries the `(ModifierPercentage, Tiering)` pair to store; `Err` carries
/// the `(reason, member, message)` of the modelled `ValidationException` the
/// calling operation refuses the request with. `CreatePricingRule` states the
/// create-time half of the same rule in its own handler (it has no stored record
/// to merge with); this is the merging form `UpdatePricingRule` applies.
pub fn reconcile_pricing_rule_rate(
    rule_type: &str,
    modifier_percentage: Option<f64>,
    tiering: Option<TieringRecord>,
    modifier_named_by_request: bool,
    tiering_named_by_request: bool,
) -> Result<PricingRuleRate, ValidationRefusal> {
    if rule_type == TIERING_PRICING_RULE_TYPE {
        if modifier_named_by_request {
            return Err((
                "ILLEGAL_MODIFIER_PERCENTAGE",
                "ModifierPercentage",
                format!("a {TIERING_PRICING_RULE_TYPE} pricing rule has no modifier percentage"),
            ));
        }
        let tiering = tiering.filter(|tiering| !tiering.is_empty());
        if tiering.is_none() {
            return Err((
                "ILLEGAL_TIERING_INPUT",
                "Tiering",
                format!("a {TIERING_PRICING_RULE_TYPE} pricing rule needs a tiering configuration"),
            ));
        }
        return Ok((None, tiering));
    }
    if tiering_named_by_request {
        return Err((
            "ILLEGAL_TIERING_INPUT",
            "Tiering",
            format!("a {rule_type} pricing rule has no tiering configuration"),
        ));
    }
    if modifier_percentage.is_none() {
        return Err((
            "ILLEGAL_MODIFIER_PERCENTAGE",
            "ModifierPercentage",
            format!("a {rule_type} pricing rule needs a modifier percentage"),
        ));
    }
    Ok((modifier_percentage, None))
}

/// A pricing plan as `CreatePricingPlan` persisted it.
#[derive(Debug, Clone, Default)]
pub struct PricingPlanRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub pricing_rule_arns: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
}

/// A pricing rule as `CreatePricingRule` persisted it.
///
/// `AssociatedPricingPlanCount` is DERIVED from the pricing plans that reference
/// the rule (see
/// [`associated_pricing_plan_count`](ComAmazonawsBillingconductorAwsbillingconductorState::associated_pricing_plan_count)),
/// never stored, so it cannot drift from the associations it counts.
#[derive(Debug, Clone, Default)]
pub struct PricingRuleRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// `PricingRuleScope`, one of `PRICING_RULE_SCOPE_VALUES`.
    pub scope: String,
    /// `PricingRuleType`, one of `PRICING_RULE_TYPE_VALUES`.
    pub rule_type: String,
    pub modifier_percentage: Option<f64>,
    pub service: Option<String>,
    pub billing_entity: Option<String>,
    pub tiering: Option<TieringRecord>,
    pub usage_type: Option<String>,
    pub operation: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
}

/// A pricing rule's tiering configuration.
///
/// `CreateTieringInput` and the `Tiering` a read reports differ only in their
/// free-tier structure (`CreateFreeTierConfig` / `FreeTierConfig`, both a single
/// `Activated` boolean), and both carry the SAME `CustomTier` shape — so the
/// boolean is what is stored and `CustomTier` is reused as modelled.
#[derive(Debug, Clone, Default)]
pub struct TieringRecord {
    /// `Tiering.FreeTier.Activated`, absent when the request named no free tier.
    pub free_tier_activated: Option<bool>,
    /// `Tiering.CustomTiers`, in the order the request listed them.
    pub custom_tiers: Option<Vec<crate::ops::types::CustomTier>>,
}

impl TieringRecord {
    /// True when the record holds no tiering configuration, which is how an
    /// update that names neither member leaves a rule with no `Tiering` at all.
    pub fn is_empty(&self) -> bool {
        self.free_tier_activated.is_none()
            && self.custom_tiers.as_ref().is_none_or(Vec::is_empty)
    }
}

/// A billing group as `CreateBillingGroup` persisted it. `Size`, `Status` and
/// `BillingGroupType` are DERIVED (see the methods below), never stored, so they
/// cannot drift from the membership and the preferences they are computed from.
#[derive(Debug, Clone, Default)]
pub struct BillingGroupRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub primary_account_id: Option<String>,
    pub pricing_plan_arn: String,
    pub linked_account_ids: Vec<String>,
    pub auto_associate: Option<bool>,
    pub responsibility_transfer_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
}

impl BillingGroupRecord {
    /// `Size`: "The number of accounts in the particular billing group."
    pub fn size(&self) -> i64 {
        i64::try_from(self.linked_account_ids.len()).unwrap_or(i64::MAX)
    }

    /// `BillingGroupStatus`. A group needs a billing owner: either a primary
    /// account, or a responsibility transfer relationship — the model documents
    /// `AccountGrouping.ResponsibilityTransferArn` as "When specified, the
    /// PrimaryAccountId is no longer required". Without either, the group is
    /// `PRIMARY_ACCOUNT_MISSING`.
    pub fn status(&self) -> &'static str {
        if self.primary_account_id.is_some() || self.responsibility_transfer_arn.is_some() {
            "ACTIVE"
        } else {
            "PRIMARY_ACCOUNT_MISSING"
        }
    }

    /// `StatusReason`: why the group is in [`status`](Self::status), absent when it
    /// is `ACTIVE`.
    pub fn status_reason(&self) -> Option<String> {
        (self.status() == "PRIMARY_ACCOUNT_MISSING").then(|| {
            "The billing group has no primary account and no responsibility transfer ARN."
                .to_string()
        })
    }

    /// `BillingGroupType`: a group that bills through a transfer relationship is
    /// `TRANSFER_BILLING`, every other group is `STANDARD`.
    pub fn billing_group_type(&self) -> &'static str {
        if self.responsibility_transfer_arn.is_some() {
            "TRANSFER_BILLING"
        } else {
            "STANDARD"
        }
    }
}

/// One resource associated to a percentage custom line item, as
/// `BatchAssociateResourcesToCustomLineItem` (or the `AssociatedValues` of the
/// charge details the item was created with) persisted it. The billing period range
/// is the one the associating call named, so `EndBillingPeriod` is reported from the
/// association rather than guessed.
#[derive(Debug, Clone, Default)]
pub struct ResourceAssociationRecord {
    /// The FULL ARN of the associated billing group or custom line item.
    pub arn: String,
    pub inclusive_start_billing_period: Option<String>,
    pub exclusive_end_billing_period: Option<String>,
}

impl ResourceAssociationRecord {
    /// `Relationship`, DERIVED from the resource the ARN names — see
    /// [`association_relationship`].
    pub fn relationship(&self) -> &'static str {
        association_relationship(&self.arn)
    }

    /// Is this association in effect during `billing_period`? The start is
    /// inclusive and the end exclusive, as the member names say
    /// (`InclusiveStartBillingPeriod` / `ExclusiveEndBillingPeriod`); an absent
    /// bound is open, so an association made without a range covers every period.
    pub fn covers(&self, billing_period: &str) -> bool {
        let Some(wanted) = billing_period_key(billing_period) else {
            return false;
        };
        let after_start = self
            .inclusive_start_billing_period
            .as_deref()
            .and_then(billing_period_key)
            .is_none_or(|start| wanted >= start);
        let before_end = self
            .exclusive_end_billing_period
            .as_deref()
            .and_then(billing_period_key)
            .is_none_or(|end| wanted < end);
        after_start && before_end
    }
}

/// A custom line item as `CreateCustomLineItem` persisted it. `AssociationSize` and
/// the percentage/flat distinction are DERIVED from this record (see the methods
/// below), never stored twice.
#[derive(Debug, Clone, Default)]
pub struct CustomLineItemRecord {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub billing_group_arn: String,
    pub account_id: Option<String>,
    /// `CustomLineItemChargeDetails.Type`: `FEE` or `CREDIT`.
    pub charge_type: String,
    /// `ChargeDetails.Flat.ChargeValue`, set for a flat custom line item only.
    pub flat_charge_value: Option<f64>,
    /// `ChargeDetails.Percentage.PercentageValue`, set for a percentage custom line
    /// item only — the kind resources can be associated to.
    pub percentage_value: Option<f64>,
    pub line_item_filters: Vec<crate::ops::types::LineItemFilter>,
    pub computation_rule: Option<String>,
    /// `PresentationDetails.Service`.
    pub presentation_service: Option<String>,
    pub inclusive_start_billing_period: Option<String>,
    pub exclusive_end_billing_period: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The associated resources, in association order.
    pub associated_resources: Vec<ResourceAssociationRecord>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
}

impl CustomLineItemRecord {
    /// True for a PERCENTAGE custom line item: the model documents
    /// `CustomLineItemChargeDetails` as containing "only one of `Flat` or
    /// `Percentage`", and a percentage value is "multiplied against the combined
    /// value of its associated resources" — so it is the only kind that has
    /// associated resources at all.
    pub fn is_percentage(&self) -> bool {
        self.percentage_value.is_some()
    }

    /// `AssociationSize`: "The number of resources that are associated to the custom
    /// line item."
    pub fn association_size(&self) -> i64 {
        i64::try_from(self.associated_resources.len()).unwrap_or(i64::MAX)
    }

    /// The association for `arn`, if this item already holds one.
    pub fn association(&self, arn: &str) -> Option<&ResourceAssociationRecord> {
        self.associated_resources
            .iter()
            .find(|association| association.arn == arn)
    }
}

/// Per-service state for com-amazonaws-billingconductor-awsbillingconductor. A handler reaches it through [`service_state`].
///
/// The maps are ordered (`BTreeMap`/`BTreeSet`) so that the list operations page
/// over a stable sequence: a `NextToken` handed back by one call has to name the
/// same cut point on the next one.
///
/// # Growth
///
/// EVERY store here is bounded by an explicit maximum, because a caller decides
/// how many requests to make:
///
/// * `pricing_rules` / `pricing_rule_names` hold one entry per rule, at most
///   [`MAX_PRICING_RULES`], and `DeletePricingRule` reclaims both entries;
///   `pricing_plans` / `pricing_plan_names` hold one entry per plan, at most
///   [`MAX_PRICING_PLANS`]; `billing_groups` / `billing_group_names` one per group,
///   at most [`MAX_BILLING_GROUPS`]; a group's membership and its share of
///   `account_associations` at most [`MAX_ACCOUNTS_PER_BILLING_GROUP`]. A create or
///   an association past its cap is REFUSED with the modelled
///   `ServiceLimitExceededException` — never by evicting a stored resource, which a
///   later caller must still be able to read. They are written only on a success
///   path (a replay, a name conflict, a missing pricing plan or a taken account all
///   return before any insert), and
///   [`remove_billing_group`](ComAmazonawsBillingconductorAwsbillingconductorState::remove_billing_group)
///   reclaims a deleted group's share of all of them.
/// * `custom_line_items` holds one entry per custom line item, at most
///   [`MAX_CUSTOM_LINE_ITEMS`], and each item at most
///   [`MAX_RESOURCES_PER_CUSTOM_LINE_ITEM`] associated resources — the maximum the
///   model itself can represent. A create past the cap is REFUSED with
///   `ServiceLimitExceededException`, and an association past it fails THAT resource
///   with the modelled `SERVICE_LIMIT_EXCEEDED` reason; a deleted billing group takes
///   its items, and their associations, with it.
/// * `unassociated_accounts` keeps the released accounts a later
///   `ListAccountAssociations` has to report as `UNMONITORED`. Nothing deletes a
///   linked account, so this one is capped at [`MAX_UNASSOCIATED_ACCOUNTS`] and
///   evicts oldest-first.
/// * the idempotency index is capped at [`MAX_IDEMPOTENCY_TOKENS`], evicts
///   oldest-first, and additionally drops the entries that point at a deleted
///   group.
/// * a resource's `tags` map is part of its record, so it is reclaimed with the
///   resource and bounded by [`MAX_TAGS_PER_RESOURCE`] — a `TagResource` whose merge
///   would take one past that cap is refused rather than stored.
///
/// A read never writes. The response side is capped too: every page is at most
/// the request's `MaxResults`, or [`DEFAULT_PAGE_SIZE`] when it names none.
#[derive(Debug, Default)]
pub struct ComAmazonawsBillingconductorAwsbillingconductorState {
    /// Pricing rules by full ARN.
    pub pricing_rules: BTreeMap<String, PricingRuleRecord>,
    /// Pricing rule name -> ARN; the uniqueness index behind the name conflict
    /// ("The names must be unique to each pricing rule").
    pub pricing_rule_names: BTreeMap<String, String>,
    /// Pricing plans by full ARN.
    pub pricing_plans: BTreeMap<String, PricingPlanRecord>,
    /// Pricing plan name -> ARN; the uniqueness index behind the name conflict
    /// ("The names must be unique to each pricing plan").
    pub pricing_plan_names: BTreeMap<String, String>,
    /// Billing groups by full ARN.
    pub billing_groups: BTreeMap<String, BillingGroupRecord>,
    /// Custom line items by full ARN, at most [`MAX_CUSTOM_LINE_ITEMS`]; each one
    /// holds at most [`MAX_RESOURCES_PER_CUSTOM_LINE_ITEM`] associated resources.
    /// A custom line item applies to a billing group, so
    /// [`remove_billing_group`](ComAmazonawsBillingconductorAwsbillingconductorState::remove_billing_group)
    /// reclaims the ones that named a deleted group.
    pub custom_line_items: BTreeMap<String, CustomLineItemRecord>,
    /// Billing group name -> ARN ("The billing group name. The names must be unique.").
    pub billing_group_names: BTreeMap<String, String>,
    /// Account id -> billing group ARN. The index that makes "not associated with
    /// another billing group" decidable and the MONITORED filter computable. One
    /// entry per account of a live billing group; a delete reclaims them.
    pub account_associations: BTreeMap<String, String>,
    /// The bounded tail of linked accounts that belong to no billing group: the ones
    /// a delete released, and a primary account that is not a member of its own
    /// group. They are what `UNMONITORED` reports. Private so the cap below cannot
    /// be bypassed by a handler inserting directly.
    unassociated_accounts: VecDeque<String>,
    /// `<operation>:<ClientToken>` -> the ARN that token's first call minted, so a
    /// retry "completes successfully without performing any further actions".
    /// Private for the same reason as `unassociated_accounts`.
    idempotency_tokens: BTreeMap<String, String>,
    /// The keys of `idempotency_tokens` in insertion order, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    idempotency_order: VecDeque<String>,
    /// Monotonic source of the alphanumeric resource ids the ARN patterns require.
    id_counter: u64,
}

impl ComAmazonawsBillingconductorAwsbillingconductorState {
    /// Mint the next `width`-character `[a-zA-Z0-9]` resource id. Monotonic, so no
    /// two resources of this service ever share an id.
    pub fn mint_resource_id(&mut self, width: usize) -> String {
        self.id_counter += 1;
        let mut value = self.id_counter;
        let mut id = vec![b'0'; width];
        for slot in id.iter_mut().rev() {
            let alphabet_len = u64::try_from(ID_ALPHABET.len()).unwrap_or(36);
            *slot = ID_ALPHABET[usize::try_from(value % alphabet_len).unwrap_or(0)];
            value /= alphabet_len;
        }
        String::from_utf8(id).unwrap_or_else(|_| "0".repeat(width))
    }

    /// The ARN a previous call with this `ClientToken` minted, if any.
    pub fn replayed_idempotent_arn(&self, operation: &str, token: Option<&str>) -> Option<String> {
        let token = token?;
        self.idempotency_tokens
            .get(&idempotency_key(operation, token))
            .cloned()
    }

    /// Remember which ARN this `ClientToken` minted, evicting the oldest remembered
    /// token once [`MAX_IDEMPOTENCY_TOKENS`] are held.
    pub fn record_idempotent_arn(&mut self, operation: &str, token: Option<&str>, arn: &str) {
        let Some(token) = token else { return };
        let key = idempotency_key(operation, token);
        if self.idempotency_tokens.insert(key.clone(), arn.to_string()).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// Record a linked account that belongs to no billing group, so a later
    /// `ListAccountAssociations` reports it as `UNMONITORED`. Capped at
    /// [`MAX_UNASSOCIATED_ACCOUNTS`], oldest evicted first.
    pub fn note_unassociated_account(&mut self, account_id: &str) {
        if self
            .unassociated_accounts
            .iter()
            .any(|known| known == account_id)
        {
            return;
        }
        self.unassociated_accounts.push_back(account_id.to_string());
        while self.unassociated_accounts.len() > MAX_UNASSOCIATED_ACCOUNTS {
            self.unassociated_accounts.pop_front();
        }
    }

    /// Every linked account this service can report, in account-id order, each with
    /// the billing group it is associated with (`None` when it is in none).
    ///
    /// The association index decides: an account that is associated is reported with
    /// its group even if it is also in the released tail, so a re-associated account
    /// cannot be reported twice or as `UNMONITORED`.
    pub fn linked_accounts(&self) -> BTreeMap<String, Option<String>> {
        let mut accounts: BTreeMap<String, Option<String>> = self
            .unassociated_accounts
            .iter()
            .map(|account_id| (account_id.clone(), None))
            .collect();
        for (account_id, group_arn) in &self.account_associations {
            accounts.insert(account_id.clone(), Some(group_arn.clone()));
        }
        accounts
    }

    /// Delete a billing group and everything that only existed because of it: its
    /// name index entry, its account associations (whose accounts become
    /// `UNMONITORED`) and the idempotency entries that named it. `None` when no such
    /// group is stored.
    pub fn remove_billing_group(&mut self, group_arn: &str) -> Option<BillingGroupRecord> {
        let removed = self.billing_groups.remove(group_arn)?;
        self.billing_group_names.remove(&removed.name);

        // A custom line item "applies to" one billing group, so it cannot outlive
        // it; and an association names a resource that exists, so every association
        // of the deleted group or of one of its items goes with them.
        let orphaned: Vec<String> = self
            .custom_line_items
            .iter()
            .filter(|(_, item)| item.billing_group_arn == group_arn)
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in &orphaned {
            self.custom_line_items.remove(arn);
        }
        for item in self.custom_line_items.values_mut() {
            item.associated_resources.retain(|association| {
                association.arn != group_arn && !orphaned.contains(&association.arn)
            });
        }

        let released: Vec<String> = self
            .account_associations
            .iter()
            .filter(|(_, arn)| arn.as_str() == group_arn)
            .map(|(account_id, _)| account_id.clone())
            .collect();
        for account_id in released {
            self.account_associations.remove(&account_id);
            self.note_unassociated_account(&account_id);
        }

        self.idempotency_tokens
            .retain(|_, arn| arn != group_arn && !orphaned.contains(arn));
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// Delete a pricing rule and the index entries that only existed because of
    /// it: its name-index entry and the idempotency entries that named it, so a
    /// `ClientToken` cannot replay into the ARN of a rule that no longer exists.
    /// `None` when no such rule is stored.
    pub fn remove_pricing_rule(&mut self, rule_arn: &str) -> Option<PricingRuleRecord> {
        let removed = self.pricing_rules.remove(rule_arn)?;
        self.pricing_rule_names.remove(&removed.name);

        self.idempotency_tokens.retain(|_, arn| arn != rule_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// The ARNs of every stored pricing plan whose `PricingRuleArns` names this
    /// pricing rule. A plan may name a rule in either spelling the
    /// `PricingRuleArn` pattern allows, so both sides are normalised.
    pub fn pricing_plans_referencing_rule(&self, rule_arn: &str) -> Vec<String> {
        self.pricing_plans
            .iter()
            .filter(|(_, plan)| {
                plan.pricing_rule_arns
                    .iter()
                    .any(|arn| normalize_pricing_rule_arn(arn) == rule_arn)
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// `AssociatedPricingPlanCount`: "The pricing plans count that this pricing rule
    /// is associated with."
    pub fn associated_pricing_plan_count(&self, rule_arn: &str) -> i64 {
        i64::try_from(self.pricing_plans_referencing_rule(rule_arn).len()).unwrap_or(i64::MAX)
    }

    /// True when the pricing plan ARN can be used by a billing group: either the
    /// caller created it, or it is one of the AWS-managed plans the
    /// `PricingPlanFullArn` pattern names.
    pub fn pricing_plan_exists(&self, arn: &str) -> bool {
        is_aws_managed_pricing_plan(arn) || self.pricing_plans.contains_key(arn)
    }

    /// The stored resource a `CustomLineItemAssociationElement` names, as its full
    /// ARN, or `None` when no such resource exists.
    ///
    /// The member admits a full `customlineitem` or `billinggroup` ARN and the bare
    /// `[a-zA-Z0-9]{10,12}` id of either, so a bare id is resolved against what is
    /// stored and names the same resource the ARN form does.
    pub fn resolve_association_element(&self, value: &str) -> Option<String> {
        if value.starts_with("arn:") {
            let arn = value.to_string();
            return (self.custom_line_items.contains_key(&arn)
                || self.billing_groups.contains_key(&arn))
            .then_some(arn);
        }
        let item_arn = custom_line_item_arn(value);
        if self.custom_line_items.contains_key(&item_arn) {
            return Some(item_arn);
        }
        let group_arn = billing_group_arn(value);
        self.billing_groups.contains_key(&group_arn).then_some(group_arn)
    }

    /// The pricing rules associated with the plan `plan_arn` names: FULL rule ARNs,
    /// deduplicated and in ARN order, or `None` when this account has no such plan.
    ///
    /// The association is to the RULE, not to the spelling the association was
    /// recorded with, so each stored value is normalised
    /// ([`normalize_pricing_rule_arn`]) and two spellings of one rule collapse to
    /// one association. An AWS-managed plan exists without the caller creating it
    /// and carries no rule of the caller's, so it associates an EMPTY set rather
    /// than being unknown. The order is the ARN order the ids are minted in, so a
    /// `NextToken` cut over this sequence names the same point on the next call.
    pub fn associated_pricing_rule_arns(&self, plan_arn: &str) -> Option<Vec<String>> {
        match self.pricing_plans.get(plan_arn) {
            Some(plan) => Some(
                plan.pricing_rule_arns
                    .iter()
                    .map(|arn| normalize_pricing_rule_arn(arn))
                    .collect::<BTreeSet<String>>()
                    .into_iter()
                    .collect(),
            ),
            None => is_aws_managed_pricing_plan(plan_arn).then(Vec::new),
        }
    }

    /// The pricing rule a name is taken by, when it is not `rule_arn`'s own name.
    pub fn conflicting_pricing_rule_name(&self, name: &str, rule_arn: &str) -> Option<&str> {
        self.pricing_rule_names
            .get(name)
            .map(String::as_str)
            .filter(|existing| *existing != rule_arn)
    }

    /// The group an account is already associated with, when it is not `group_arn`.
    pub fn conflicting_association(&self, account_id: &str, group_arn: &str) -> Option<&str> {
        self.account_associations
            .get(account_id)
            .map(String::as_str)
            .filter(|existing| *existing != group_arn)
    }

    /// The tags stored for the resource a tagging `ResourceArn` names, or `None` when
    /// this service holds no such resource — the decision every tagging operation's
    /// not-found answer is made from.
    ///
    /// Tags live ON the resource record (the same map `CreateBillingGroup` /
    /// `CreatePricingPlan` persisted their `Tags` into), so there is one copy of a
    /// resource's tags and a delete reclaims them with the resource.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        let resource = parse_taggable_arn(resource_arn)?;
        match resource.kind {
            TaggableResourceKind::BillingGroup => self
                .billing_groups
                .get(&resource.arn)
                .map(|record| &record.tags),
            TaggableResourceKind::PricingPlan => self
                .pricing_plans
                .get(&resource.arn)
                .map(|record| &record.tags),
            // Nothing in this slice persists a pricing rule or a custom line item
            // (`CreatePricingRule` and `CreateCustomLineItem` are still scaffolds), so
            // no ARN of either kind names a resource that exists yet.
            TaggableResourceKind::PricingRule | TaggableResourceKind::CustomLineItem => None,
        }
    }

    /// [`resource_tags`](Self::resource_tags), for a tagging operation that WRITES.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let resource = parse_taggable_arn(resource_arn)?;
        match resource.kind {
            TaggableResourceKind::BillingGroup => self
                .billing_groups
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::PricingPlan => self
                .pricing_plans
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::PricingRule | TaggableResourceKind::CustomLineItem => None,
        }
    }
}

/// The resource kinds this service's tagging surface addresses, as the resource
/// segment of the modelled tagging `Arn` spells them
/// (`smithy_query(query="operation",operation="TagResource")`:
/// `billinggroup/?[0-9]{12}`, `pricingplan/[a-zA-Z0-9]{10}`,
/// `pricingrule/[a-zA-Z0-9]{10}` and `(customlineitem/)?[a-zA-Z0-9]{10}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggableResourceKind {
    BillingGroup,
    PricingPlan,
    PricingRule,
    CustomLineItem,
}

impl TaggableResourceKind {
    /// Every kind, in the order the `Arn` pattern enumerates them.
    pub const ALL: [Self; 4] = [
        Self::BillingGroup,
        Self::PricingPlan,
        Self::PricingRule,
        Self::CustomLineItem,
    ];

    /// The resource segment's prefix, which is also how this service's errors name
    /// the kind (`ConflictException.ResourceType` spells a plan `pricingplan`).
    pub fn segment(self) -> &'static str {
        match self {
            Self::BillingGroup => "billinggroup",
            Self::PricingPlan => "pricingplan",
            Self::PricingRule => "pricingrule",
            Self::CustomLineItem => "customlineitem",
        }
    }
}

/// A resource a tagging operation addressed: which kind its ARN names, that ARN in the
/// ONE spelling this service's maps are keyed by (see the module header), and the bare
/// resource id.
#[derive(Debug, Clone)]
pub struct TaggableResource {
    pub kind: TaggableResourceKind,
    pub id: String,
    pub arn: String,
}

/// Read a tagging `ResourceArn` into the resource it names, `None` when it names no
/// resource of this service at all.
///
/// The modelled `Arn` accepts the full ARN of any of the four taggable kinds — with the
/// `billinggroup` separator OPTIONAL — or a bare custom-line-item id, so one value has
/// several spellings and they must all resolve to the one key the maps use. The
/// partition and account segments are KEPT: another account's (or another partition's)
/// ARN names a resource this emulator does not hold, which is a not-found rather than a
/// hit on the same-id local resource. A region segment is kept the same way: this
/// service is global, so every ARN it mints carries the empty region [`ARN_REGION`] and
/// an ARN naming a region names no resource of this service.
pub fn parse_taggable_arn(value: &str) -> Option<TaggableResource> {
    let (partition, account, resource) = match value.strip_prefix("arn:") {
        Some(rest) => {
            let mut segments = rest.splitn(5, ':');
            let partition = segments.next()?;
            let service = segments.next()?;
            let region = segments.next()?;
            let account = segments.next()?;
            let resource = segments.next()?;
            if service != ARN_SERVICE || region != ARN_REGION {
                return None;
            }
            (
                partition.to_string(),
                account.to_string(),
                resource.to_string(),
            )
        }
        // The only form the pattern admits with no ARN prefix is a custom line item id
        // (`^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}$`),
        // so a bare value names a custom line item of this emulator's own account.
        None => (
            "aws".to_string(),
            ::engine::emulator_account_id().to_string(),
            format!("customlineitem/{value}"),
        ),
    };

    let kind = TaggableResourceKind::ALL
        .into_iter()
        .find(|kind| resource.starts_with(kind.segment()))?;
    // `billinggroup/?` — the separator the pattern makes optional, so both spellings of
    // one resource resolve to the same id.
    let rest = &resource[kind.segment().len()..];
    let id = rest.strip_prefix('/').unwrap_or(rest);
    if id.is_empty() {
        return None;
    }
    Some(TaggableResource {
        kind,
        id: id.to_string(),
        arn: format!(
            "arn:{partition}:{ARN_SERVICE}::{account}:{}/{id}",
            kind.segment()
        ),
    })
}

/// A `ClientToken` is scoped to the operation that minted it: two operations may
/// legitimately be retried with the same token value.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// How many `ClientToken` -> ARN entries this service remembers.
///
/// A CHOSEN bound, not an AWS limit: AWS's own idempotency windows are finite (a
/// client token stops being recognised after some hours), and this is the
/// emulator's equivalent — the oldest token is forgotten once the cap is reached, so
/// a caller replaying a token it used long ago creates a new resource instead of
/// growing this map without limit.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1024;

/// How many released (`UNMONITORED`) linked accounts this service remembers.
///
/// A CHOSEN bound, not an AWS limit: nothing in this API deletes a linked account,
/// so the set of accounts that belong to no billing group would otherwise grow with
/// every create-and-delete. The oldest released account is forgotten first; an
/// account that is still associated with a group is never in this tail.
pub const MAX_UNASSOCIATED_ACCOUNTS: usize = 1024;

/// How many pricing plans this emulator stores for the account.
///
/// A CHOSEN bound, not an AWS limit (the real service's quota is adjustable per
/// account). It exists because nothing in this slice deletes a pricing plan, so
/// without it one caller could grow the store with every request. A create past the
/// cap is REFUSED with the modelled `ServiceLimitExceededException` rather than
/// evicting someone else's resource: a stored resource must stay readable.
pub const MAX_PRICING_PLANS: usize = 1000;

/// How many billing groups this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced.
pub const MAX_BILLING_GROUPS: usize = 1000;

/// How many custom line items this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced.
pub const MAX_CUSTOM_LINE_ITEMS: usize = 1000;

/// How many resources one percentage custom line item may hold.
///
/// A CHOSEN bound, informed by the model rather than read off it:
/// `CustomLineItemAssociationsList` — the `ChargeDetails.Percentage.AssociatedValues`
/// a custom line item is CREATED with — carries `@length(max: 5)`, so five is the
/// most the create's own shape can express, and holding a stored item within what
/// the create can express keeps it representable. (The documented AWS quota for
/// resources per percentage custom line item is not in the model;
/// `BatchAssociateResourcesToCustomLineItem`'s `ResourceArns` admits 1..30 per
/// request, which bounds a request rather than the membership it accumulates.) An
/// association past the bound fails THAT resource with the modelled
/// `SERVICE_LIMIT_EXCEEDED` reason rather than displacing a stored association.
pub const MAX_RESOURCES_PER_CUSTOM_LINE_ITEM: usize = 5;

/// How many pricing rules this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced. A rule IS
/// deletable (`DeletePricingRule`), so a caller that reaches the cap can make room
/// by deleting one rather than being stuck.
pub const MAX_PRICING_RULES: usize = 1000;

/// `ModifierPercentage` as the service stores it: "Your entry will be rounded to
/// the nearest 2 decimal places." (the member's own documentation), so the value
/// a later read reports is the rounded one rather than whatever precision the
/// request carried.
pub fn round_modifier_percentage(value: f64) -> f64 {
    if value.is_finite() {
        (value * 100.0).round() / 100.0
    } else {
        value
    }
}

/// How many linked accounts one billing group may hold. A CHOSEN bound; see
/// [`MAX_PRICING_PLANS`]. The model caps one `AssociateAccounts` call at 30 account
/// ids, which bounds a single request but not the membership it accumulates.
pub const MAX_ACCOUNTS_PER_BILLING_GROUP: usize = 1000;

/// The Service Quotas-style service code this service's limit errors carry.
pub const SERVICE_CODE: &str = "billingconductor";

/// How many tags one resource holds.
///
/// The modelled `TagMap`'s own `@length` maximum
/// (`smithy_query(query="shape",shape="TagMap")`: len 1..200) — the same bound the
/// boundary already enforces on ONE request's map, applied to what a resource
/// ACCUMULATES over several `TagResource` calls. A merge that would take a resource
/// past it is REFUSED with the modelled `ValidationException` rather than stored, so a
/// caller cannot grow one resource's tag map without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The wire error code a tagging operation answers when its `ResourceArn` names no
/// resource of this service.
///
/// NOT a shape this service's model declares — the model lists
/// `ResourceNotFoundException` (404) for these operations — but the code the real
/// service answers the tagging surface with, as recorded by the behavioral corpus:
/// HTTP 404, error code `NotFoundException`, message "Resource was not found" (see
/// `issues/.../20261022T200837Z-billingconductor-tagresource-stub.md`, whose check
/// asserts `tag_code == "NotFoundException"`, and cause 6 of
/// `...-billingconductor-payer-account-authorization-gate-absent-across-the.md`, which
/// records the same code, status and message). The code and the status are what a
/// client classifies on, so they follow the recording rather than the model.
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The status [`TAGGING_NOT_FOUND_CODE`] is served on (recorded: HTTP 404, the same
/// status the model gives `ResourceNotFoundException`).
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The message the real service's tagging not-found carries. It names nothing from the
/// request, so it is the same sentence for every resource.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The tagging not-found failure, as a tagging handler returns it: the recorded code
/// and status under the operation's own error channel.
pub fn tagging_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_NOT_FOUND_CODE,
        TAGGING_NOT_FOUND_STATUS,
        TAGGING_NOT_FOUND_MESSAGE,
    )
}

/// The page size the list operations apply when the request names none.
///
/// `ListAccountAssociations` is `@paginated` with a `NextToken` but models no
/// `MaxResults` at all, and `ListBillingGroups`'s `MaxResults` is optional, so in
/// both cases the SERVER chooses. This is the upper bound the service itself models
/// for that member (`MaxBillingGroupResults`, range 1..100), so one response is
/// bounded however much state exists, and a caller that wants more pages the
/// `NextToken` it already gets.
pub const DEFAULT_PAGE_SIZE: i32 = 100;

/// Cut one page out of an ordered, already-filtered sequence of `(key, element)`.
///
/// The `NextToken` IS the key of the first element left out, so it is a cursor
/// rather than an offset: resuming skips every key below it. The page size is the
/// request's `max_results` CLAMPED to 1..=[`DEFAULT_PAGE_SIZE`] — a request that
/// names none, or names a value the modelled range does not allow, is served the
/// default — so a page is bounded whatever reaches this helper.
pub fn paginate<T>(
    items: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let start = next_token.unwrap_or("");
    let mut page: Vec<(String, T)> = items
        .into_iter()
        .filter(|(key, _)| key.as_str() >= start)
        .collect();
    let requested = max_results
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .min(DEFAULT_PAGE_SIZE);
    let limit = usize::try_from(requested).unwrap_or(1);
    let token = page.get(limit).map(|(key, _)| key.clone());
    page.truncate(limit);
    (page.into_iter().map(|(_, element)| element).collect(), token)
}

/// The first value that appears more than once in `values`, if any.
pub fn first_duplicate(values: &[String]) -> Option<&String> {
    let mut seen = BTreeSet::new();
    values.iter().find(|value| !seen.insert(value.as_str()))
}

/// Is a `CustomLineItemBillingPeriodRange` ordered, i.e. does it describe a non-empty
/// span of billing periods?
///
/// The member names carry the rule: `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not, so an end equal to or earlier than the start
/// would cover no billing period at all. A range with no end is open and always
/// ordered.
pub fn billing_period_range_is_ordered(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> bool {
    let Some(end) = range
        .exclusive_end_billing_period
        .as_deref()
        .and_then(billing_period_key)
    else {
        return true;
    };
    billing_period_key(&range.inclusive_start_billing_period).is_none_or(|start| start < end)
}

/// The modelled `ValidationException` this service refuses a request with: the
/// reason (one of `ValidationExceptionReason`), the member at fault, and the text.
///
/// It lives in this module rather than one of its own because `src/ops/mod.rs` is
/// GENERATED, so a helper several operations share has no other home in this crate.
pub fn validation_exception(
    reason: &str,
    field: &str,
    message: impl Into<String>,
) -> crate::ops::types::ValidationException {
    let message = message.into();
    crate::ops::types::ValidationException {
        message: message.clone(),
        reason: Some(reason.to_string()),
        fields: Some(vec![crate::ops::types::ValidationExceptionField {
            name: field.to_string(),
            message,
        }]),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBillingconductorAwsbillingconductorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBillingconductorAwsbillingconductorState>)
}

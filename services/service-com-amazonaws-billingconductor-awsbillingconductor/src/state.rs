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

/// The CURRENT billing period: the month a billing read uses when the request names
/// none (`ListCustomLineItems`: "If you don't provide a billing period, the current
/// billing period is used").
///
/// Built from the SAME clock as [`now_millis`] and rendered in the form
/// `BillingPeriod`'s pattern accepts (`^\d{4}-(0?[1-9]|1[012])$`), so it is a value a
/// caller could have sent and [`billing_period_key`] reads it back. Billing is
/// reported in UTC, so the month is the UTC one.
///
/// A `BillingPeriod` is exactly the `<year>-<month>` head of the ISO 8601 date-time
/// (`YYYY-MM-DDThh:mm:ssZ`) the SAME instant renders as, so the calendar conversion is
/// `aws_smithy_types::DateTime` — the formatter this emulator already serializes its
/// `@timestampFormat("date-time")` members with — rather than a second date library or
/// hand-written civil-calendar arithmetic.
pub fn current_billing_period() -> String {
    billing_period_at(now_millis())
}

/// The `BillingPeriod` the instant `millis` falls in — the month a call made then
/// bills in.
///
/// The calendar conversion is the one [`current_billing_period`] describes, applied to
/// an instant the store holds rather than to the clock, so the period a stored write
/// took effect in is derivable from the instant it was recorded at and no second
/// spelling of a month is persisted beside it.
pub fn billing_period_at(millis: i64) -> String {
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|rendered| rendered.get(.."YYYY-MM".len()).map(str::to_string))
        // Unreachable for a clock the formatter accepts; the epoch month is still a
        // billing period, so a read is answered rather than failed.
        .unwrap_or_else(|| "1970-01".to_string())
}

/// Is `billing_period` inside the half-open range `[start, end)` a custom line item or
/// a resource association carries?
///
/// The member names carry the rule — `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not — and an ABSENT bound is open, so a resource
/// stored without a range covers every billing period. A `billing_period` that is not
/// a billing period at all covers nothing.
pub fn billing_period_range_covers(
    inclusive_start: Option<&str>,
    exclusive_end: Option<&str>,
    billing_period: &str,
) -> bool {
    let Some(wanted) = billing_period_key(billing_period) else {
        return false;
    };
    let after_start = inclusive_start
        .and_then(billing_period_key)
        .is_none_or(|start| wanted >= start);
    let before_end = exclusive_end
        .and_then(billing_period_key)
        .is_none_or(|end| wanted < end);
    after_start && before_end
}

/// The billing period a comparable month count ([`billing_period_key`]) names, in the
/// canonical `YYYY-MM` spelling [`billing_period_at`] renders.
///
/// The inverse of [`billing_period_key`], whose count is `year * 12 + month` with the
/// month in `1..=12`, so the year is the count one month back divided by twelve and the
/// month is what is left of it.
fn billing_period_of_key(key: i64) -> String {
    let year = (key - 1).div_euclid(12);
    let month = key - year * 12;
    format!("{year:04}-{month:02}")
}

/// The billing periods of the half-open range `[inclusive_start, exclusive_end)`, in
/// ascending order and each in the canonical `YYYY-MM` spelling — the periods a report
/// over that range covers.
///
/// The start is in the range and the end is not, as the member names of a
/// `BillingPeriodRange` say. The periods are ENUMERATED from the range rather than read
/// off a stored value, so their spelling is canonical however the request spelled the
/// bounds, and the list is cut at [`MAX_BILLING_PERIODS_PER_RANGE`] so a caller cannot
/// ask one read to walk an unbounded number of months. A bound that is not a billing
/// period, or a range that covers none, enumerates nothing.
pub fn billing_periods_in_range(inclusive_start: &str, exclusive_end: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (
        billing_period_key(inclusive_start),
        billing_period_key(exclusive_end),
    ) else {
        return Vec::new();
    };
    (start..end)
        .take(MAX_BILLING_PERIODS_PER_RANGE)
        .map(billing_period_of_key)
        .collect()
}

/// How many billing periods one `BillingPeriodRange` may cover: "You can specify up to
/// 12 months" (`GetBillingGroupCostReportInput.BillingPeriodRange`).
pub const MAX_BILLING_PERIODS_PER_RANGE: usize = 12;

/// How many billing periods the half-open range `[inclusive_start, exclusive_end)`
/// covers, or `None` when either bound is not a billing period.
pub fn billing_period_range_length(inclusive_start: &str, exclusive_end: &str) -> Option<i64> {
    let start = billing_period_key(inclusive_start)?;
    let end = billing_period_key(exclusive_end)?;
    Some(end - start)
}

/// Does the half-open bound `start` come strictly before `end`? An absent `start` is
/// the unbounded past and an absent `end` the unbounded future, so either one absent
/// orders. Both bounds are the month counts [`billing_period_key`] returns.
pub fn billing_period_precedes(start: Option<i64>, end: Option<i64>) -> bool {
    match (start, end) {
        (Some(start), Some(end)) => start < end,
        _ => true,
    }
}

/// One half-open span of billing periods, `[InclusiveStartBillingPeriod,
/// ExclusiveEndBillingPeriod)`, as a `CustomLineItemBillingPeriodRange` names one and
/// as [`billing_period_range_covers`] reads it: an ABSENT bound is open, so a span with
/// neither bound is every billing period.
///
/// The bounds are kept as the STRINGS a request carried, because a billing period has
/// two spellings of one month (`2026-7` and `2026-07`) and a read reports the period
/// back as the call that set it named it; ordering goes through
/// [`billing_period_key`], so the two spellings still compare equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BillingPeriodSpan {
    pub inclusive_start: Option<String>,
    pub exclusive_end: Option<String>,
}

impl BillingPeriodSpan {
    /// A span of the two bounds a `BillingPeriodRange` carries.
    pub fn new(inclusive_start: Option<String>, exclusive_end: Option<String>) -> Self {
        Self {
            inclusive_start,
            exclusive_end,
        }
    }

    /// Is `billing_period` inside this span? See [`billing_period_range_covers`].
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            self.inclusive_start.as_deref(),
            self.exclusive_end.as_deref(),
            billing_period,
        )
    }

    /// This span's inclusive start as a comparable month count; `None` when it is open.
    fn start_key(&self) -> Option<i64> {
        self.inclusive_start.as_deref().and_then(billing_period_key)
    }

    /// This span's exclusive end as a comparable month count; `None` when it is open.
    fn end_key(&self) -> Option<i64> {
        self.exclusive_end.as_deref().and_then(billing_period_key)
    }

    /// The billing periods of this span that are NOT in `removed`, as spans in
    /// ascending order.
    ///
    /// Subtracting one half-open span from another leaves the part BEFORE the removal
    /// starts and the part FROM the removal's end on — so a removal that is bounded on
    /// both sides and falls strictly inside this span leaves two, a removal that
    /// overlaps one end leaves one, and a removal that covers this span leaves none.
    /// A removal that shares no billing period with this span leaves it as it is.
    pub fn minus(&self, removed: &Self) -> Vec<Self> {
        let (start, end) = (self.start_key(), self.end_key());
        let (removed_start, removed_end) = (removed.start_key(), removed.end_key());
        // Two half-open spans share a billing period only when each one starts before
        // the other ends.
        if !billing_period_precedes(start, removed_end)
            || !billing_period_precedes(removed_start, end)
        {
            return vec![self.clone()];
        }
        let mut left = Vec::new();
        if removed_start.is_some() && billing_period_precedes(start, removed_start) {
            left.push(Self::new(
                self.inclusive_start.clone(),
                removed.inclusive_start.clone(),
            ));
        }
        if removed_end.is_some() && billing_period_precedes(removed_end, end) {
            left.push(Self::new(
                removed.exclusive_end.clone(),
                self.exclusive_end.clone(),
            ));
        }
        left
    }
}

/// The `CurrencyCode` a resource's charges are denominated in, DERIVED from the
/// PARTITION of its ARN.
///
/// `CurrencyCode` has exactly two values
/// ([`crate::ops::types::CURRENCY_CODE_VALUES`]: `CNY`, `USD`) and the modelled charge
/// value is documented "in USD" (`ListCustomLineItemFlatChargeDetails.ChargeValue`),
/// while the only other partition this service's ARN patterns admit is China
/// (`arn:aws-cn`), which bills in CNY. So the partition the resource lives in decides,
/// and nothing stores a currency beside the resource.
pub fn partition_currency_code(arn: &str) -> &'static str {
    if arn.starts_with("arn:aws-cn:") {
        "CNY"
    } else {
        "USD"
    }
}

/// The `CurrencyCode` a custom line item's charge value is denominated in — the
/// currency of the partition it lives in, see [`partition_currency_code`].
pub fn custom_line_item_currency_code(arn: &str) -> &'static str {
    partition_currency_code(arn)
}

/// The product a custom line item's charges are presented under in the bill.
///
/// `PresentationDetails.Service` is "the service under which the custom line item
/// charges will be presented", so an item that carries one is presented under it. An
/// item that carries none is presented under the service that raises the charge, which
/// is this one — the name its own model gives it
/// (`smithy_query(query="service_info")`: `AWSBillingConductor`) — so the product of a
/// charge is always known and never invented per request.
pub fn custom_line_item_product_name(item: &CustomLineItemRecord) -> &str {
    item.presentation_service
        .as_deref()
        .unwrap_or(BILLING_CONDUCTOR_PRODUCT_NAME)
}

/// The product name of this service's own charges — see
/// [`custom_line_item_product_name`].
pub const BILLING_CONDUCTOR_PRODUCT_NAME: &str = "AWSBillingConductor";

/// The sign a `CustomLineItemChargeDetails.Type` gives a charge on a bill: a `FEE` adds
/// to the bill and a `CREDIT` comes off it ("The type of the custom line item that
/// indicates whether the charge is a fee or credit";
/// [`crate::ops::types::CUSTOM_LINE_ITEM_TYPE_VALUES`] has exactly those two values).
pub fn charge_sign(charge_type: &str) -> f64 {
    if charge_type == CREDIT_CHARGE_TYPE {
        -1.0
    } else {
        1.0
    }
}

/// The `CustomLineItemType` that takes its value OFF the bill rather than adding it.
const CREDIT_CHARGE_TYPE: &str = "CREDIT";

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

/// The account every request to this emulator is made by.
///
/// Every generated emulator presents the one canonical account id
/// ([`engine::emulator_account_id`]); it is never derived from the caller's access
/// key. This is the account the authorization gate below judges, and the account id
/// the denial names.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The modelled `AccessDeniedException` (`@httpError 403`) AWS answers a
/// billing-conductor call made by an account that is not the organization's payer
/// (management) account with.
///
/// The text is built FROM THE REQUEST — the calling account id and the operation name
/// — which is the construction the real service uses; the account id in a recording is
/// environment-specific and an example only. It is deliberately NOT the generic IAM
/// "no identity-based policy allows" wording: that is how a caller tells a per-account
/// entitlement gate from a missing permission.
pub fn not_payer_account(operation: &str) -> crate::ops::types::AccessDeniedException {
    crate::ops::types::AccessDeniedException {
        message: format!(
            "Account {} is not authorized to call {operation}. Only payer account is authorized",
            caller_account_id()
        ),
    }
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
/// calling operation refuses the request with. BOTH paths that set a rule's rate
/// decide it here, so one rule has one behaviour: `UpdatePricingRule` passes the
/// stored record merged with the request, and `CreatePricingRule` — which has no
/// stored record behind it — passes the request's own members as the whole rate.
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
    /// The rules this plan associates, each with the billing period its association
    /// started in — see [`PricingRuleAssociationRecord`].
    pub pricing_rule_associations: Vec<PricingRuleAssociationRecord>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
}

impl PricingPlanRecord {
    /// The FULL ARNs of the rules this plan associates IN `billing_period`,
    /// deduplicated and in ARN order.
    ///
    /// The association is to the RULE, not to the spelling it was recorded with, so
    /// two spellings of one rule collapse to one association.
    pub fn associated_rule_arns(&self, billing_period: &str) -> Vec<String> {
        self.pricing_rule_associations
            .iter()
            .filter(|association| association.covers(billing_period))
            .map(|association| normalize_pricing_rule_arn(&association.arn))
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }
}

/// One pricing rule association on a pricing plan, with the billing period it
/// started in.
///
/// A Billing Conductor association is scoped to a billing period — every read of one
/// names the period it is asking about (`BillingPeriod`: "the billing period for which
/// the pricing rule associations are to be listed") — so the record keeps the period
/// from which the association is in force rather than only the rule. The period an
/// association starts in is the one the associating call was made in, the same
/// `InclusiveStartBillingPeriod` sense a custom line item's billing period range
/// carries (see [`billing_period_range_covers`]).
/// `Default` is deliberately NOT derived: an empty start period is not a billing
/// period, and a record that carried one would be read as unbounded. Every
/// association is built through [`Self::started_now`].
#[derive(Debug, Clone)]
pub struct PricingRuleAssociationRecord {
    /// The FULL pricing rule ARN ([`normalize_pricing_rule_arn`]).
    pub arn: String,
    /// The first billing period this association is in force for.
    pub inclusive_start_billing_period: String,
}

impl PricingRuleAssociationRecord {
    /// An association made NOW: in force from the current billing period onward.
    pub fn started_now(rule_arn: &str) -> Self {
        Self {
            arn: normalize_pricing_rule_arn(rule_arn),
            inclusive_start_billing_period: current_billing_period(),
        }
    }

    /// Is this association in force in `billing_period`? An association has no end
    /// while it stands, so every period from the one it started in onward is covered.
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            Some(&self.inclusive_start_billing_period),
            None,
            billing_period,
        )
    }
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

/// The billing group statuses a CALLER may ask for, out of the three the modelled
/// `BillingGroupStatus` enumerates
/// (`smithy_query(query="shape",shape="BillingGroupStatus")`).
///
/// `PRIMARY_ACCOUNT_MISSING` is not among them: it reports that the group has no
/// billing owner, a condition of the resource the service DERIVES (see
/// [`BillingGroupRecord::status`]), so it is not a status a request chooses. The
/// other two say whether the group is billing (`ACTIVE`) or not yet (`PENDING`),
/// which is the group's own operational state and so is the caller's to set.
pub const CALLER_SETTABLE_BILLING_GROUP_STATUSES: &[&str] = &["ACTIVE", "PENDING"];

/// A billing group as `CreateBillingGroup` persisted it. `Size`,
/// `BillingGroupType` and the owner half of `Status` are DERIVED (see the methods
/// below), never stored, so they cannot drift from the membership and the
/// preferences they are computed from.
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
    /// The operational `BillingGroupStatus` a caller asked this group to hold, one
    /// of [`CALLER_SETTABLE_BILLING_GROUP_STATUSES`]. `None` for a group no caller
    /// has set a status on, which reads back as `ACTIVE` — the status a group with a
    /// billing owner holds until something says otherwise.
    pub requested_status: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
}

impl BillingGroupRecord {
    /// `Size`: "The number of accounts in the particular billing group."
    pub fn size(&self) -> i64 {
        i64::try_from(self.linked_account_ids.len()).unwrap_or(i64::MAX)
    }

    /// True when the group has a billing owner: either a primary account, or a
    /// responsibility transfer relationship — the model documents
    /// `AccountGrouping.ResponsibilityTransferArn` as "When specified, the
    /// PrimaryAccountId is no longer required".
    pub fn has_billing_owner(&self) -> bool {
        self.primary_account_id.is_some() || self.responsibility_transfer_arn.is_some()
    }

    /// `BillingGroupStatus`. A group with no billing owner
    /// ([`has_billing_owner`](Self::has_billing_owner)) is `PRIMARY_ACCOUNT_MISSING`,
    /// whatever any request asked for: that status reports the absence rather than a
    /// choice. A group that HAS one holds the operational status last asked for, and
    /// `ACTIVE` until one is.
    pub fn status(&self) -> &'static str {
        if !self.has_billing_owner() {
            return "PRIMARY_ACCOUNT_MISSING";
        }
        match self.requested_status.as_deref() {
            Some("PENDING") => "PENDING",
            _ => "ACTIVE",
        }
    }

    /// `StatusReason`: why the group is in [`status`](Self::status), absent when it
    /// is `ACTIVE`.
    pub fn status_reason(&self) -> Option<String> {
        match self.status() {
            "PRIMARY_ACCOUNT_MISSING" => Some(
                "The billing group has no primary account and no responsibility transfer ARN."
                    .to_string(),
            ),
            "PENDING" => {
                Some("The billing group is not billing yet at its owner's request.".to_string())
            }
            _ => None,
        }
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
///
/// The periods the association is in effect in are held as a LIST of half-open spans,
/// because a call that changes the association in a bounded billing period range
/// changes it only there: removing a range from the middle of the association leaves
/// the periods on either side of it, which one span cannot hold. The spans are
/// disjoint and in ascending order, so a resource is ONE record however many spans it
/// is in effect over and `AssociationSize` counts it once.
#[derive(Debug, Clone, Default)]
pub struct ResourceAssociationRecord {
    /// The FULL ARN of the associated billing group or custom line item.
    pub arn: String,
    /// The billing periods this association is in effect in — see
    /// [`BillingPeriodSpan`]. An association made with no range at all is the one
    /// fully open span. The spans are disjoint and ascending, and there are at most
    /// [`MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION`] of them: a caller decides how many
    /// bounded calls to make, and each one can carve one more stretch, so the list is
    /// capped like every other store here.
    pub spans: Vec<BillingPeriodSpan>,
}

/// What taking a range of billing periods out of a resource association did — see
/// [`ResourceAssociationRecord::remove_periods`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodsRemoval {
    /// The billing periods the association is in effect in changed.
    Changed,
    /// The range shares no billing period with the association, so nothing changed —
    /// which is also how a retry of a call that already applied answers.
    Unchanged,
    /// REFUSED: the removal would leave the association in more separate stretches of
    /// billing periods than [`MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION`], so nothing
    /// was written.
    AtSpanLimit,
}

impl ResourceAssociationRecord {
    /// An association of `arn` in effect over `span` — the billing period range the
    /// associating call named, which is fully open when it named none.
    pub fn new(arn: String, span: BillingPeriodSpan) -> Self {
        Self {
            arn,
            spans: vec![span],
        }
    }

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
        self.spans.iter().any(|span| span.covers(billing_period))
    }

    /// `EndBillingPeriod`: the billing period this association stops applying at,
    /// EXCLUSIVE, absent when it does not stop.
    ///
    /// A read that names a billing period is asking about the association as it stands
    /// IN that period, so the span covering it is the one that ends; a read that names
    /// none is asking about the association as a whole, which ends where its LAST span
    /// does.
    pub fn end_billing_period(&self, billing_period: Option<&str>) -> Option<String> {
        match billing_period {
            Some(period) => self
                .spans
                .iter()
                .find(|span| span.covers(period))
                .and_then(|span| span.exclusive_end.clone()),
            None => self
                .spans
                .last()
                .and_then(|span| span.exclusive_end.clone()),
        }
    }

    /// Take the billing periods `removed` names out of this association, and report
    /// what that did to the periods it is in effect in.
    ///
    /// Each span keeps whatever of itself lies outside the removal
    /// ([`BillingPeriodSpan::minus`]), so a removal bounded on both sides takes only
    /// the periods between its bounds and the association resumes at its exclusive
    /// end. An association left in effect in no period at all is empty
    /// ([`Self::is_empty`]) and its record goes.
    ///
    /// A removal that would carve the association into more than
    /// [`MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION`] separate stretches is REFUSED and
    /// writes nothing, because a caller decides how many such calls to make and each
    /// one can add a stretch.
    pub fn remove_periods(&mut self, removed: &BillingPeriodSpan) -> PeriodsRemoval {
        let left: Vec<BillingPeriodSpan> = self
            .spans
            .iter()
            .flat_map(|span| span.minus(removed))
            .collect();
        if left == self.spans {
            return PeriodsRemoval::Unchanged;
        }
        if left.len() > MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION {
            return PeriodsRemoval::AtSpanLimit;
        }
        self.spans = left;
        PeriodsRemoval::Changed
    }

    /// Is this association in effect in no billing period at all, so that nothing of
    /// it is left to report?
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// One stretch of time during which an account was a member of a billing group,
/// measured in the `BillingPeriod` months the billing API reports in.
///
/// An account-association read "lists linked accounts that are linked to the payer
/// account for the specified time period" (`ListAccountAssociations`), so the store
/// keeps WHEN each membership held rather than only which group holds the account
/// now. The range is half-open, exactly as every other billing-period range here is
/// (see [`billing_period_range_covers`]): the account is a member from
/// `inclusive_start_billing_period` on, and `exclusive_end_billing_period` is the
/// first period it is no longer one — absent while the membership stands.
///
/// `Default` is deliberately NOT derived, for the reason
/// [`PricingRuleAssociationRecord`] gives: an empty start period is not a billing
/// period, and a record carrying one would be read as unbounded.
#[derive(Debug, Clone)]
pub struct AccountAssociationRecord {
    /// The FULL billing group ARN ([`normalize_billing_group_arn`]).
    pub billing_group_arn: String,
    /// The first billing period the account is a member of that group for.
    pub inclusive_start_billing_period: String,
    /// The first billing period it is no longer one, absent while it still is.
    pub exclusive_end_billing_period: Option<String>,
}

impl AccountAssociationRecord {
    /// Does this membership still stand — has nothing ended it?
    pub fn is_live(&self) -> bool {
        self.exclusive_end_billing_period.is_none()
    }

    /// Was the account a member of this group in `billing_period`? Half-open, as
    /// [`billing_period_range_covers`] reads it, so a membership that began and
    /// ended in the SAME period covers no period at all — which is what a listing
    /// for that period reports, and what the live membership says too.
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            Some(&self.inclusive_start_billing_period),
            self.exclusive_end_billing_period.as_deref(),
            billing_period,
        )
    }
}

/// The billing group one account was a member of in `billing_period`, out of the
/// memberships it has had: the NEWEST one covering that period, so an account
/// released and re-associated within one period is reported in the group it ended up
/// in. `None` when none of them covers it.
///
/// One reverse scan per account, bounded by
/// [`MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT`], and the only place that decides which
/// membership a period reports — so every period-scoped read answers the same way.
fn membership_in<'a>(
    memberships: &'a [AccountAssociationRecord],
    billing_period: &str,
) -> Option<&'a str> {
    memberships
        .iter()
        .rev()
        .find(|membership| membership.covers(billing_period))
        .map(|membership| membership.billing_group_arn.as_str())
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
    /// The instant the item's CURRENT generation took effect: the create that minted
    /// the item, or the later write that opened a new generation of it (see
    /// [`ComAmazonawsBillingconductorAwsbillingconductorState::put_custom_line_item`]).
    /// It is what a version reports as its `StartTime`.
    pub version_start_time_millis: i64,
    /// The generations of this item that later writes SUPERSEDED, oldest first, at
    /// most [`MAX_CUSTOM_LINE_ITEM_VERSIONS`] of them. The generation in effect NOW is
    /// not among them — it is projected from this record by [`Self::current_version`] —
    /// so the history and the live item cannot drift. Held on the record so a deleted
    /// item takes its history with it.
    pub superseded_versions: Vec<CustomLineItemVersion>,
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

    /// Is this custom line item in effect during `billing_period`? The
    /// `BillingPeriodRange` the create set is half-open, and an item stored without one
    /// applies to every billing period — see [`billing_period_range_covers`].
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            self.inclusive_start_billing_period.as_deref(),
            self.exclusive_end_billing_period.as_deref(),
            billing_period,
        )
    }

    /// The billing period this item's CURRENT generation is in effect FROM.
    ///
    /// The billing-period range a write names is the span the item it leaves applies
    /// in, and a write that names none leaves the item in effect from the billing
    /// period that write itself landed in — so a generation always has a start billing
    /// period, and that period is what identifies it.
    pub fn version_start_billing_period(&self) -> String {
        self.inclusive_start_billing_period
            .clone()
            .unwrap_or_else(|| billing_period_at(self.version_start_time_millis))
    }

    /// This item's CURRENT generation, as a version snapshot.
    ///
    /// Projected from the live record — through the same `ChargeDetails` and
    /// `AssociationSize` derivations every other read of the item uses — rather than
    /// stored twice, so the newest version a history read reports is the item a read of
    /// the item itself reports.
    pub fn current_version(&self) -> CustomLineItemVersion {
        CustomLineItemVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            billing_group_arn: self.billing_group_arn.clone(),
            account_id: self.account_id.clone(),
            charge_details: self.charge_details(),
            association_size: self.association_size(),
            computation_rule: self.computation_rule.clone(),
            presentation_service: self.presentation_service.clone(),
            start_billing_period: self.version_start_billing_period(),
            exclusive_end_billing_period: self.exclusive_end_billing_period.clone(),
            creation_time_millis: self.creation_time_millis,
            last_modified_time_millis: self.last_modified_time_millis,
            start_time_millis: self.version_start_time_millis,
        }
    }

    /// Every generation of this item, OLDEST FIRST: the ones later writes superseded,
    /// frozen as they stood, and then the one in effect now.
    pub fn versions(&self) -> Vec<CustomLineItemVersion> {
        let mut versions = self.superseded_versions.clone();
        versions.push(self.current_version());
        versions
    }

    /// The association for `arn`, if this item already holds one.
    pub fn association(&self, arn: &str) -> Option<&ResourceAssociationRecord> {
        self.associated_resources
            .iter()
            .find(|association| association.arn == arn)
    }

    /// `ChargeDetails` as this service's surface REPORTS a stored item
    /// (`ListCustomLineItemChargeDetails`).
    ///
    /// "It should contain only one of `Flat` or `Percentage`", so the one the item
    /// holds is reported and the other is absent; the `@required Type` is the one the
    /// item carries; and the filter list is omitted entirely when the item holds none,
    /// because an absent list and an empty one are the same membership.
    ///
    /// It lives on the record so that every operation reporting a custom line item —
    /// `ListCustomLineItems`, `UpdateCustomLineItem` — reports ONE projection and the
    /// two views cannot drift.
    pub fn charge_details(&self) -> crate::ops::types::ListCustomLineItemChargeDetails {
        crate::ops::types::ListCustomLineItemChargeDetails {
            flat: self.flat_charge_value.map(|charge_value| {
                crate::ops::types::ListCustomLineItemFlatChargeDetails { charge_value }
            }),
            percentage: self.percentage_value.map(|percentage_value| {
                crate::ops::types::ListCustomLineItemPercentageChargeDetails { percentage_value }
            }),
            r#type: self.charge_type.clone(),
            line_item_filters: (!self.line_item_filters.is_empty())
                .then(|| self.line_item_filters.clone()),
        }
    }
}

/// One GENERATION of a custom line item — a VERSION of it, as
/// `ListCustomLineItemVersions` reports one.
///
/// A generation is the state the item had while it was in effect, together with the
/// billing period it took effect in and the instant it did. A superseded generation is
/// FROZEN: nothing writes to it again, so the history a read reports is what the item
/// really was, not a recomputation from the item it has since become.
#[derive(Debug, Clone, Default)]
pub struct CustomLineItemVersion {
    pub name: String,
    pub description: String,
    pub billing_group_arn: String,
    pub account_id: Option<String>,
    /// `ChargeDetails` as this generation reported them.
    pub charge_details: crate::ops::types::ListCustomLineItemChargeDetails,
    /// `AssociationSize`: the resources associated to the item in this generation.
    pub association_size: i64,
    pub computation_rule: Option<String>,
    /// `PresentationDetails.Service`.
    pub presentation_service: Option<String>,
    /// The billing period this generation is in effect FROM, inclusive — see
    /// [`CustomLineItemRecord::version_start_billing_period`].
    pub start_billing_period: String,
    /// The billing period this generation stops applying at, EXCLUSIVE, when the write
    /// that left it named one. A generation that outlived no range of its own is closed
    /// by the generation that replaced it instead.
    pub exclusive_end_billing_period: Option<String>,
    /// The item's `CreationTime`, which every generation of it shares.
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
    /// `StartTime`: "The inclusive start time" — the instant this generation took
    /// effect.
    pub start_time_millis: i64,
}

/// The auto billing group creation preference recorded for ONE billing transfer, as
/// `UpdateBillingTransferPreference` persisted it.
///
/// `AutoTransferBillingGroupCreationPreference` carries `Enabled` and — only while the
/// preference is enabled — the `PricingPlanArn` the automatically created billing groups
/// are to compute with. The record adds the instant the preference was last set, which
/// is what `LastModifiedTime` reports; a transfer with NO record is how "the preference
/// has never been set" is represented, so that case reports the documented default and
/// an empty `LastModifiedTime` rather than a stored zero.
#[derive(Debug, Clone, Default)]
pub struct BillingTransferPreferenceRecord {
    /// The `ResponsibilityTransferArn` of the billing transfer the preference applies
    /// to — this record's identity. The modelled pattern admits the full ARN only (no
    /// bare-id spelling), so it needs no normalisation.
    pub responsibility_transfer_arn: String,
    /// `AutoBillingTransferBillingGroupCreation.Enabled`.
    pub enabled: bool,
    /// The FULL pricing plan ARN the preference names, held only while `enabled`:
    /// "This value is required when `Enabled` is `true`, and must be omitted when
    /// `Enabled` is `false`."
    pub pricing_plan_arn: Option<String>,
    pub last_modified_time_millis: i64,
}

/// The account segment of a `ResponsibilityTransferArn`: the management account of the
/// organization that owns the billing transfer.
///
/// The modelled pattern spells a billing transfer as an AWS Organizations resource ARN
/// (`^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-…`), whose account
/// segment is the organization's management account — the fifth colon-separated field,
/// the same position [`parse_taggable_arn`] reads an account id from.
pub fn responsibility_transfer_owner_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4).filter(|account| !account.is_empty())
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
///   [`MAX_PRICING_PLANS`]; a plan's own association set at most
///   [`MAX_PRICING_RULES_PER_PRICING_PLAN`]; `billing_groups` / `billing_group_names` one per group,
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
///   its items, and their associations, with it. One association is in effect over at
///   most [`MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION`] separate stretches of billing
///   periods, since a disassociation bounded to a range inside one of them splits it:
///   the one that would carve a stretch too many fails THAT resource with the same
///   reason and writes nothing. An item also carries the generations
///   of it that later writes superseded, at most [`MAX_CUSTOM_LINE_ITEM_VERSIONS`],
///   oldest dropped first.
/// * `billing_transfer_preferences` holds one entry per billing transfer whose
///   preference was set, at most [`MAX_BILLING_TRANSFER_PREFERENCES`]; nothing in this
///   API deletes a billing transfer, so that store evicts the oldest-set preference
///   first, which reads back as the "never set" it was before.
/// * `unassociated_accounts` keeps the released accounts a later
///   `ListAccountAssociations` has to report as `UNMONITORED`. Nothing deletes a
///   linked account, so this one is capped at [`MAX_UNASSOCIATED_ACCOUNTS`] and
///   evicts oldest-first, forgetting the evicted account's billing group
///   memberships with it; one account keeps at most
///   [`MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT`] of them, oldest ENDED membership
///   dropped first.
/// * the idempotency index is capped at [`MAX_IDEMPOTENCY_TOKENS`], evicts
///   oldest-first, and additionally drops the entries that point at a deleted
///   group.
/// * a resource's `tags` map is part of its record, so it is reclaimed with the
///   resource and bounded by [`MAX_TAGS_PER_RESOURCE`] — a `TagResource` whose merge
///   would take one past that cap is refused rather than stored. That cap bounds the
///   `ListTagsForResource` response too, since the response IS the stored map.
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
    /// Account id -> the billing group memberships that account has had, oldest
    /// first: the ones that have ended and, last, the one that stands now (at most
    /// one, since an account belongs to at most one billing group). The index that
    /// makes "not associated with another billing group" decidable, the MONITORED
    /// filter computable, and a read scoped to a billing period answerable from the
    /// membership that was in force THEN (through [`membership_in`]). At most
    /// [`MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT`] per account, oldest ended membership
    /// dropped first; a delete ends the memberships the deleted group held.
    ///
    /// A handler writes it only through [`Self::associate_account`] and
    /// [`Self::release_account`], and reads it through
    /// [`Self::account_billing_group`], [`Self::linked_accounts`] and
    /// [`Self::billing_group_sizes_in`], so that cap and the one-live-membership
    /// invariant hold wherever a membership changes.
    pub account_associations: BTreeMap<String, Vec<AccountAssociationRecord>>,
    /// The auto billing group creation preference of every billing transfer that has
    /// one, keyed by its `ResponsibilityTransferArn`. At most
    /// [`MAX_BILLING_TRANSFER_PREFERENCES`]. Private so the cap below cannot be
    /// bypassed by a handler inserting directly: reach it through
    /// [`Self::billing_transfer_preference`] and
    /// [`Self::set_billing_transfer_preference`].
    billing_transfer_preferences: BTreeMap<String, BillingTransferPreferenceRecord>,
    /// The keys of `billing_transfer_preferences` in first-set order, so the cap evicts
    /// the oldest-set preference rather than an arbitrary one.
    billing_transfer_preference_order: VecDeque<String>,
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
    /// The account that is the PAYER (management) account of the organization this
    /// store records, if any.
    ///
    /// Billing Conductor is a payer-account-only API: its control plane is authorized
    /// for the payer account of the consolidated billing family alone, and every other
    /// caller is refused `AccessDeniedException` (see
    /// [`Self::is_payer_account`] and [`not_payer_account`]). This emulator presents one
    /// account ([`caller_account_id`]) and emulates no Organizations control plane, so
    /// there is no organization and no payer account to record: the field holds `None`
    /// and the emulated account is a standalone, non-payer account — the caller every
    /// recorded Billing Conductor response was taken from. A test (or a future
    /// Organizations surface) designates a payer account by setting it, which is the
    /// only way the control plane becomes reachable.
    pub payer_account_id: Option<String>,
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

    /// The preference recorded for the billing transfer `arn` names, or `None` when no
    /// call ever set one — the case `GetBillingTransferPreference` reports the
    /// documented default and an empty `LastModifiedTime` for.
    pub fn billing_transfer_preference(
        &self,
        arn: &str,
    ) -> Option<&BillingTransferPreferenceRecord> {
        self.billing_transfer_preferences.get(arn)
    }

    /// Record one billing transfer's preference and hand back what was stored,
    /// forgetting the oldest-set preference once [`MAX_BILLING_TRANSFER_PREFERENCES`]
    /// are held.
    pub fn set_billing_transfer_preference(
        &mut self,
        arn: &str,
        enabled: bool,
        pricing_plan_arn: Option<String>,
        now: i64,
    ) -> BillingTransferPreferenceRecord {
        let record = BillingTransferPreferenceRecord {
            responsibility_transfer_arn: arn.to_string(),
            enabled,
            pricing_plan_arn,
            last_modified_time_millis: now,
        };
        if self
            .billing_transfer_preferences
            .insert(arn.to_string(), record.clone())
            .is_none()
        {
            self.billing_transfer_preference_order.push_back(arn.to_string());
        }
        while self.billing_transfer_preference_order.len() > MAX_BILLING_TRANSFER_PREFERENCES {
            if let Some(oldest) = self.billing_transfer_preference_order.pop_front() {
                self.billing_transfer_preferences.remove(&oldest);
                // The eviction is the ONE thing about this store a caller cannot see in
                // a response (the forgotten transfer reads back as "never set"), so it
                // is logged rather than silent. Only the transfer's ARN, which the
                // caller itself supplied, never a secret.
                ::tracing::warn!(
                    responsibility_transfer_arn = %oldest,
                    cap = MAX_BILLING_TRANSFER_PREFERENCES,
                    "the oldest billing transfer preference was forgotten at the store's cap"
                );
            }
        }
        record
    }

    /// The billing transfers whose ENABLED preference names the pricing plan `plan_arn`:
    /// "While a pricing plan is specified in an enabled preference, you can't delete
    /// that pricing plan" (`UpdateBillingTransferPreference`'s own documentation), the
    /// case `ConflictExceptionReason` names
    /// `PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT`.
    ///
    /// A disabled preference names no plan at all, so only the enabled ones can hold a
    /// plan back. Stored plan ARNs are normalised
    /// ([`normalize_pricing_plan_arn`]), so one plan has exactly one spelling here.
    pub fn billing_transfer_preferences_referencing_pricing_plan(
        &self,
        plan_arn: &str,
    ) -> Vec<String> {
        self.billing_transfer_preferences
            .values()
            .filter(|record| record.enabled && record.pricing_plan_arn.as_deref() == Some(plan_arn))
            .map(|record| record.responsibility_transfer_arn.clone())
            .collect()
    }

    /// Whether the billing transfer `arn` names is one of THIS organization's.
    ///
    /// A billing transfer is an AWS Organizations resource and its ARN carries the
    /// management account of the organization that owns it
    /// ([`responsibility_transfer_owner_account`]). Billing Conductor's control plane is
    /// the payer (management) account's alone, so the one organization this store can
    /// answer for is the one whose management account it records as the payer
    /// ([`Self::payer_account_id`]): an ARN owned by any other account names a transfer
    /// this organization does not have.
    ///
    /// Fails CLOSED, like the authorization gate: a store that records no payer account
    /// records no organization, so it owns no transfer.
    pub fn transfer_belongs_to_this_organization(&self, arn: &str) -> bool {
        match (
            self.payer_account_id.as_deref(),
            responsibility_transfer_owner_account(arn),
        ) {
            (Some(management), Some(owner)) => management == owner,
            _ => false,
        }
    }

    /// Record a linked account that belongs to no billing group, so a later
    /// `ListAccountAssociations` reports it as `UNMONITORED`. Capped at
    /// [`MAX_UNASSOCIATED_ACCOUNTS`], oldest evicted first.
    ///
    /// An evicted account is forgotten whole: the memberships it once had go with it,
    /// unless it is in a billing group NOW, in which case the membership that stands
    /// is what keeps it reportable. So the membership history is bounded by the same
    /// cap as the roster it belongs to.
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
            let Some(evicted) = self.unassociated_accounts.pop_front() else {
                break;
            };
            if self.account_billing_group(&evicted).is_none() {
                self.account_associations.remove(&evicted);
            }
        }
    }

    /// Associate `account_id` with `group_arn` from the CURRENT billing period on —
    /// the period the associating call is made in, the same sense a pricing rule
    /// association ([`PricingRuleAssociationRecord`]) records.
    ///
    /// At most one membership stands at a time, so the one to end is the newest live
    /// one: a membership of ANOTHER group is ended in this same period, and restating
    /// the membership the account already has changes nothing — the period it began in
    /// is the one it is still in force from.
    pub fn associate_account(&mut self, account_id: &str, group_arn: &str) {
        let billing_period = current_billing_period();
        let memberships = self
            .account_associations
            .entry(account_id.to_string())
            .or_default();
        if let Some(live) = memberships.iter_mut().rev().find(|held| held.is_live()) {
            if live.billing_group_arn == group_arn {
                return;
            }
            live.exclusive_end_billing_period = Some(billing_period.clone());
        }
        memberships.push(AccountAssociationRecord {
            billing_group_arn: group_arn.to_string(),
            inclusive_start_billing_period: billing_period,
            exclusive_end_billing_period: None,
        });
        while memberships.len() > MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT {
            memberships.remove(0);
        }
    }

    /// Release `account_id` from `group_arn`: the membership of that group it still
    /// has ends at the CURRENT billing period, and the account is recorded as
    /// belonging to no billing group — which is what `UNMONITORED` reports. A no-op
    /// on the memberships of an account that group does not hold now, so a release
    /// never rewrites what some other group's membership says.
    pub fn release_account(&mut self, account_id: &str, group_arn: &str) {
        let billing_period = current_billing_period();
        if let Some(live) = self
            .account_associations
            .get_mut(account_id)
            .and_then(|memberships| memberships.iter_mut().rev().find(|held| held.is_live()))
        {
            if live.billing_group_arn == group_arn {
                live.exclusive_end_billing_period = Some(billing_period);
            }
        }
        self.note_unassociated_account(account_id);
    }

    /// The billing group `account_id` belongs to NOW, or `None` when it is in none —
    /// the decision "not associated with another billing group" is made from.
    pub fn account_billing_group(&self, account_id: &str) -> Option<&str> {
        self.account_associations
            .get(account_id)?
            .iter()
            .rev()
            .find(|membership| membership.is_live())
            .map(|membership| membership.billing_group_arn.as_str())
    }

    /// Every linked account this service can report, in account-id order, each with
    /// the billing group it was associated with IN `billing_period` (`None` when it
    /// was in none then).
    ///
    /// The roster is every account this service has seen, whatever period is asked
    /// about: a linked account stays in the consolidated billing family whether or
    /// not it was in a billing group then, so one that joined a group later is
    /// reported for an earlier period with no group at all.
    pub fn linked_accounts(&self, billing_period: &str) -> BTreeMap<String, Option<String>> {
        let mut accounts: BTreeMap<String, Option<String>> = self
            .unassociated_accounts
            .iter()
            .map(|account_id| (account_id.clone(), None))
            .collect();
        for (account_id, memberships) in &self.account_associations {
            let group_arn = membership_in(memberships, billing_period).map(str::to_string);
            accounts.insert(account_id.clone(), group_arn);
        }
        accounts
    }

    /// `Size` — "The number of accounts in the particular billing group" — as of
    /// `billing_period`, for each of the groups in `group_arns` (a group nothing was
    /// a member of then counts zero).
    ///
    /// ONE rule answers every period, the membership index through [`membership_in`],
    /// so the count cannot disagree with the listing that reports the same
    /// memberships. And ONE pass over that index answers every group asked about, so a
    /// listing reporting `Size` for a page of groups never rescans the accounts per
    /// group: the work is a single bounded pass, whatever period is asked about.
    pub fn billing_group_sizes_in(
        &self,
        group_arns: &[String],
        billing_period: &str,
    ) -> BTreeMap<String, i64> {
        let mut sizes: BTreeMap<String, i64> = group_arns
            .iter()
            .map(|group_arn| (group_arn.clone(), 0))
            .collect();
        for memberships in self.account_associations.values() {
            let Some(group_arn) = membership_in(memberships, billing_period) else {
                continue;
            };
            if let Some(size) = sizes.get_mut(group_arn) {
                *size = size.saturating_add(1);
            }
        }
        sizes
    }

    /// Delete a billing group and everything that only existed because of it: its
    /// name index entry, the account memberships it held (which END at the current
    /// billing period, so those accounts become `UNMONITORED` from now on while the
    /// periods they were members in still report the group) and the idempotency
    /// entries that named it. `None` when no such group is stored.
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
        // Through the SAME removal `DeleteCustomLineItem` performs, so an item taken
        // by its billing group leaves exactly as little behind as one deleted
        // directly.
        for arn in &orphaned {
            self.remove_custom_line_item(arn);
        }
        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != group_arn);
        }

        // The memberships the group held END with it, rather than being forgotten: a
        // read scoped to a billing period the group existed in still has to report
        // the accounts it held then.
        let released: Vec<String> = self
            .account_associations
            .keys()
            .filter(|account_id| self.account_billing_group(account_id) == Some(group_arn))
            .cloned()
            .collect();
        for account_id in released {
            self.release_account(&account_id, group_arn);
        }

        // The tokens that named one of the deleted ITEMS went with each of them
        // ([`Self::remove_custom_line_item`]), so what is left to reclaim here is the
        // group's own.
        self.idempotency_tokens.retain(|_, arn| arn != group_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// Delete a custom line item and everything that only existed because of it: the
    /// associations other percentage custom line items held TO it (an association
    /// names a resource that exists, and its own associations go with the record) and
    /// the idempotency entries that named it, so a `ClientToken` cannot replay into
    /// the ARN of an item that no longer exists. `None` when no such item is stored.
    pub fn remove_custom_line_item(&mut self, item_arn: &str) -> Option<CustomLineItemRecord> {
        let removed = self.custom_line_items.remove(item_arn)?;

        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != item_arn);
        }

        self.idempotency_tokens.retain(|_, arn| arn != item_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// Write `updated` over the stored custom line item it names, keeping the
    /// generations a version read reports.
    ///
    /// A version of a custom line item is the generation of it that applies in a
    /// billing period, so the billing period a write takes effect in decides what the
    /// write does to the history: a write taking effect in a DIFFERENT billing period
    /// than the one the item's current generation started in closes that generation —
    /// frozen as it stood, so a later read of the history reports the item as it really
    /// was — and opens a new generation at `now`, while a write taking effect in the
    /// generation's own billing period REPLACES it, because one billing period has one
    /// version of the item.
    ///
    /// The kept history is bounded like every other store here, since a caller decides
    /// how many writes to make: past [`MAX_CUSTOM_LINE_ITEM_VERSIONS`] superseded
    /// generations the OLDEST is dropped, which reads back as a history that starts
    /// later — never as a lost current item, which a later caller must still see.
    pub fn put_custom_line_item(&mut self, mut updated: CustomLineItemRecord, now: i64) {
        let arn = updated.arn.clone();
        let superseded = self
            .custom_line_items
            .get(&arn)
            .filter(|previous| {
                previous.version_start_billing_period() != updated.version_start_billing_period()
            })
            .map(CustomLineItemRecord::current_version);
        if let Some(superseded) = superseded {
            updated.superseded_versions.push(superseded);
            while updated.superseded_versions.len() > MAX_CUSTOM_LINE_ITEM_VERSIONS {
                updated.superseded_versions.remove(0);
            }
            updated.version_start_time_millis = now;
        }
        self.custom_line_items.insert(arn, updated);
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

    /// Delete a pricing plan and the index entries that only existed because of it:
    /// its name-index entry (so the name frees up for a later create) and the
    /// idempotency entries that named it, so a `ClientToken` cannot replay into the
    /// ARN of a plan that no longer exists. `None` when no such plan is stored.
    ///
    /// A plan's rule ASSOCIATIONS live on its own record, so they go with it; the
    /// rules themselves are separate resources and outlive the plan.
    pub fn remove_pricing_plan(&mut self, plan_arn: &str) -> Option<PricingPlanRecord> {
        let removed = self.pricing_plans.remove(plan_arn)?;
        self.pricing_plan_names.remove(&removed.name);

        self.idempotency_tokens.retain(|_, arn| arn != plan_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// The ARNs of every stored billing group whose
    /// `ComputationPreference.PricingPlanArn` names this pricing plan — the
    /// association "the pricing plan must not be associated with any billing groups
    /// to delete successfully" is decided from. A group may have been created naming
    /// the plan in either spelling the `PricingPlanArn` pattern allows, so both sides
    /// are normalised.
    pub fn billing_groups_referencing_pricing_plan(&self, plan_arn: &str) -> Vec<String> {
        self.billing_groups
            .iter()
            .filter(|(_, group)| normalize_pricing_plan_arn(&group.pricing_plan_arn) == plan_arn)
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// The ARNs of every stored pricing plan that associates this pricing rule IN
    /// `billing_period`. A plan may name a rule in either spelling the
    /// `PricingRuleArn` pattern allows, so both sides are normalised, and an
    /// association that had not started by `billing_period` is not in force then.
    pub fn pricing_plans_referencing_rule(
        &self,
        rule_arn: &str,
        billing_period: &str,
    ) -> Vec<String> {
        self.pricing_plans
            .iter()
            .filter(|(_, plan)| {
                plan.associated_rule_arns(billing_period)
                    .iter()
                    .any(|arn| arn == rule_arn)
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// `AssociatedPricingPlanCount`: "The pricing plans count that this pricing rule
    /// is associated with" — in `billing_period`, the period the read is scoped to.
    pub fn associated_pricing_plan_count(&self, rule_arn: &str, billing_period: &str) -> i64 {
        i64::try_from(self.pricing_plans_referencing_rule(rule_arn, billing_period).len())
            .unwrap_or(i64::MAX)
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

    /// The pricing rules associated with the plan `plan_arn` names IN
    /// `billing_period`: FULL rule ARNs, deduplicated and in ARN order, or `None`
    /// when this account has no such plan.
    ///
    /// An association is in force from the billing period it was made in onward, so a
    /// period before it started reports it not at all — see
    /// [`PricingPlanRecord::associated_rule_arns`]. An AWS-managed plan exists without
    /// the caller creating it and carries no rule of the caller's, so it associates an
    /// EMPTY set rather than being unknown. The order is the ARN order the ids are
    /// minted in, so a `NextToken` cut over this sequence names the same point on the
    /// next call.
    pub fn associated_pricing_rule_arns(
        &self,
        plan_arn: &str,
        billing_period: &str,
    ) -> Option<Vec<String>> {
        match self.pricing_plans.get(plan_arn) {
            Some(plan) => Some(plan.associated_rule_arns(billing_period)),
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

    /// The pricing plan a name is taken by, when it is not `plan_arn`'s own name — the
    /// uniqueness index behind "The name must be unique to each pricing plan", decided
    /// the same way as for a rule so a rename to the name already held is not a
    /// conflict.
    pub fn conflicting_pricing_plan_name(&self, name: &str, plan_arn: &str) -> Option<&str> {
        self.pricing_plan_names
            .get(name)
            .map(String::as_str)
            .filter(|existing| *existing != plan_arn)
    }

    /// The billing group a name is taken by, when it is not `group_arn`'s own name —
    /// the uniqueness index behind "The names must be unique to each billing group",
    /// decided the same way as for a pricing plan so a rename to the name already
    /// held is not a conflict.
    pub fn conflicting_billing_group_name(&self, name: &str, group_arn: &str) -> Option<&str> {
        self.billing_group_names
            .get(name)
            .map(String::as_str)
            .filter(|existing| *existing != group_arn)
    }

    /// The ARNs of the stored billing groups OTHER than `group_arn` that have
    /// automatic account association enabled — what the
    /// [`MAX_AUTO_ASSOCIATE_BILLING_GROUPS`] cap is decided against.
    ///
    /// `group_arn` is the group the request is about, and the empty string for a group
    /// that does not exist yet, so a request that leaves the flag where the addressed
    /// group already has it is never refused by the group's own enablement.
    pub fn other_auto_associate_billing_groups(&self, group_arn: &str) -> Vec<String> {
        self.billing_groups
            .iter()
            .filter(|(arn, group)| {
                group.auto_associate.unwrap_or(false) && arn.as_str() != group_arn
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// The group an account is already associated with, when it is not `group_arn`.
    pub fn conflicting_association(&self, account_id: &str, group_arn: &str) -> Option<&str> {
        self.account_billing_group(account_id)
            .filter(|existing| *existing != group_arn)
    }

    /// Whether `account_id` is the payer (management) account of the organization this
    /// store records.
    ///
    /// False whenever no payer account is recorded, which is the store's default (see
    /// [`Self::payer_account_id`]): the Billing Conductor control plane is then
    /// available to nobody, exactly as it is to an account that is not a payer.
    pub fn is_payer_account(&self, account_id: &str) -> bool {
        self.payer_account_id.as_deref() == Some(account_id)
    }

    /// The tags stored for the resource a tagging `ResourceArn` names, or `None` when
    /// this service holds no such resource — the decision every tagging operation's
    /// not-found answer is made from.
    ///
    /// Tags live ON the resource record (the same map `CreateBillingGroup` /
    /// `CreatePricingPlan` / `CreatePricingRule` / `CreateCustomLineItem` persisted their
    /// `Tags` into), so there is one copy of a resource's tags and a delete reclaims them
    /// with the resource. EVERY taggable kind this service persists is resolved here: a
    /// kind left unresolved would make the tags its create stored unreadable.
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
            TaggableResourceKind::PricingRule => self
                .pricing_rules
                .get(&resource.arn)
                .map(|record| &record.tags),
            TaggableResourceKind::CustomLineItem => self
                .custom_line_items
                .get(&resource.arn)
                .map(|record| &record.tags),
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
            TaggableResourceKind::PricingRule => self
                .pricing_rules
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::CustomLineItem => self
                .custom_line_items
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
        }
    }

    /// What the custom line items of one billing group put on its PRO FORMA bill in
    /// `billing_period`, summed per product and SIGNED by each item's charge type.
    ///
    /// A custom line item is an adjustment to what the billing group is charged — "a
    /// one-time fixed charge that can be applied to a single billing group", either "a
    /// fee or discount" — so it is part of the pro forma cost and of nothing else. Every
    /// item of the group whose own billing period range covers the period contributes its
    /// charge with the sign of its type ([`charge_sign`]), under the product it is
    /// presented as ([`custom_line_item_product_name`]). An empty map is a billing group
    /// with no item in effect in that period, so its pro forma cost there is its actual
    /// cost.
    ///
    /// The charge of each item is computed at most ONCE per billing period, however the
    /// associations between them are arranged, so the work of one period is bounded by the
    /// store (at most [`MAX_CUSTOM_LINE_ITEMS`] items of at most
    /// [`MAX_RESOURCES_PER_CUSTOM_LINE_ITEM`] associations each) rather than by the shape
    /// of the associations a caller made.
    pub fn proforma_custom_line_item_charges(
        &self,
        group_arn: &str,
        billing_period: &str,
    ) -> BTreeMap<String, f64> {
        let mut charges: BTreeMap<String, f64> = BTreeMap::new();
        let mut computed: BTreeMap<String, f64> = BTreeMap::new();
        let mut being_computed: BTreeSet<String> = BTreeSet::new();
        for item in self.custom_line_items.values() {
            if item.billing_group_arn != group_arn || !item.covers(billing_period) {
                continue;
            }
            let charge = charge_sign(&item.charge_type)
                * self.custom_line_item_charge(
                    item,
                    billing_period,
                    &mut computed,
                    &mut being_computed,
                );
            *charges
                .entry(custom_line_item_product_name(item).to_string())
                .or_insert(0.0) += charge;
        }
        charges
    }

    /// The charge ONE custom line item carries in `billing_period`, as the UNSIGNED
    /// amount its charge details describe — the item's type is what decides which way it
    /// moves the bill.
    ///
    /// "It should contain only one of `Flat` or `Percentage`": a flat item's charge is its
    /// `ChargeValue`, and a percentage item's is its `PercentageValue` "multiplied against
    /// the combined value of its associated resources", counting only the resources whose
    /// association is in effect in that period. An associated resource is another custom
    /// line item, whose own charge it contributes, or a billing group, which contributes
    /// the metered usage of its accounts — of which this emulator records none, as the
    /// `AWSCost` its cost reports answer with says; an ARN naming neither contributes
    /// nothing.
    ///
    /// `computed` carries the charges already worked out for this billing period, so an
    /// item several others are associated to is computed once. `being_computed` carries
    /// the items whose charge this walk is in the middle of: nothing stops two percentage
    /// items from being associated to each other, and an item whose own value is not
    /// settled yet has no value to lend, so it contributes nothing and that nothing is not
    /// remembered — it is a property of the walk rather than of the item.
    fn custom_line_item_charge(
        &self,
        item: &CustomLineItemRecord,
        billing_period: &str,
        computed: &mut BTreeMap<String, f64>,
        being_computed: &mut BTreeSet<String>,
    ) -> f64 {
        if let Some(charge) = computed.get(&item.arn) {
            return *charge;
        }
        let charge = if let Some(charge_value) = item.flat_charge_value {
            charge_value
        } else if let Some(percentage_value) = item.percentage_value {
            if !being_computed.insert(item.arn.clone()) {
                return 0.0;
            }
            let combined: f64 = item
                .associated_resources
                .iter()
                .filter(|association| association.covers(billing_period))
                .map(|association| {
                    self.custom_line_items
                        .get(&association.arn)
                        .map_or(0.0, |associated| {
                            self.custom_line_item_charge(
                                associated,
                                billing_period,
                                computed,
                                being_computed,
                            )
                        })
                })
                .sum();
            being_computed.remove(&item.arn);
            percentage_value / 100.0 * combined
        } else {
            // Neither charge member: an item no create of this service can leave behind,
            // since exactly one of them is required.
            0.0
        };
        computed.insert(item.arn.clone(), charge);
        charge
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

/// How many billing transfer preferences this service remembers.
///
/// A CHOSEN bound, not an AWS limit: a billing transfer is an AWS Organizations
/// resource this API never creates, so the set of transfer ARNs a caller may set a
/// preference for is bounded by nothing this store can see. `UpdateBillingTransferPreference`
/// models no `ServiceLimitExceededException`, so a set past the cap is not refused:
/// the oldest-SET preference is forgotten, exactly as the idempotency index forgets its
/// oldest token ([`MAX_IDEMPOTENCY_TOKENS`]). A forgotten preference reads back as
/// "never set" — the same answer the store gave for that transfer before anybody set
/// one — and only a write ever evicts, so no read changes what a later caller sees.
pub const MAX_BILLING_TRANSFER_PREFERENCES: usize = 1000;

/// How many released (`UNMONITORED`) linked accounts this service remembers.
///
/// A CHOSEN bound, not an AWS limit: nothing in this API deletes a linked account,
/// so the set of accounts that belong to no billing group would otherwise grow with
/// every create-and-delete. The oldest released account is forgotten first; an
/// account that is still associated with a group is never in this tail.
pub const MAX_UNASSOCIATED_ACCOUNTS: usize = 1024;

/// How many billing group memberships this service remembers per linked account:
/// the one that stands now plus the ones that have ended.
///
/// A CHOSEN bound, not an AWS limit: a caller decides how often it associates and
/// disassociates an account, and each cycle is one more ended membership to report a
/// past billing period from. The oldest ENDED membership is dropped first, so a
/// forgotten one reads back the way that period read before anybody recorded it, and
/// the membership in force is never the one evicted. A billing period is a month, so
/// this holds every membership a long history of them could be reported for.
pub const MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT: usize = 100;

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

/// How many of a consolidated billing family's billing groups may have automatic
/// account association enabled at once.
///
/// NOT a chosen bound: `AccountGrouping.AutoAssociate` elects the ONE group that
/// "will automatically associate newly added Amazon Web Services accounts that join
/// your consolidated billing family" (the member's own documentation), and an account
/// belongs to at most one billing group — so two enabled groups would both claim the
/// same joining account and the election would not decide anything. The model names
/// the refusal a request past the cap earns:
/// `ValidationExceptionReason` enumerates `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`). The cap is on
/// STATE, so every operation that can turn the flag on enforces it, and a group that
/// already has it on may restate it.
pub const MAX_AUTO_ASSOCIATE_BILLING_GROUPS: usize = 1;

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

/// How many SEPARATE stretches of billing periods one resource association may be in
/// effect over ([`ResourceAssociationRecord::spans`]).
///
/// A CHOSEN bound, not an AWS limit: a caller decides how many bounded
/// `BatchDisassociateResourcesFromCustomLineItem` calls to make, and one whose range
/// falls inside a stretch the association already has splits it in two, so the list
/// has to be capped like the kept history is ([`MAX_CUSTOM_LINE_ITEM_VERSIONS`], the
/// same order of magnitude, for the same reason). A disassociation that would carve
/// one more stretch past the cap fails THAT resource with the modelled
/// `SERVICE_LIMIT_EXCEEDED` reason and writes nothing — never by coarsening the
/// periods a stored association is in effect over, which a later read must still
/// report faithfully.
pub const MAX_BILLING_PERIOD_SPANS_PER_ASSOCIATION: usize = 100;

/// How many SUPERSEDED generations of one custom line item this emulator keeps for
/// `ListCustomLineItemVersions` to report.
///
/// A CHOSEN bound, not an AWS limit: a caller decides how many times to update an
/// item, and each update that takes effect in a new billing period adds a generation,
/// so the kept history has to be capped. The OLDEST generation is dropped first, which
/// reads back as a history that begins later — never as the loss of the generation in
/// effect now, which is projected from the item itself and so is always reported.
pub const MAX_CUSTOM_LINE_ITEM_VERSIONS: usize = 100;

/// How many pricing rules this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced. A rule IS
/// deletable (`DeletePricingRule`), so a caller that reaches the cap can make room
/// by deleting one rather than being stuck.
pub const MAX_PRICING_RULES: usize = 1000;

/// How many pricing rules ONE pricing plan may associate.
///
/// A CHOSEN bound, informed by the model rather than read off it:
/// `CreatePricingPlanInput.PricingRuleArns` (`PricingRuleArnsInput`, len 0..30) is
/// the largest association set a create can express, and
/// `AssociatePricingRules`/`DisassociatePricingRules` move at most 30 rules per call
/// (`PricingRuleArnsNonEmptyInput`, len 1..30) — so holding a stored plan within
/// what a create can express keeps that plan representable. (A documented AWS quota
/// for rules per plan is not in the model; the `@length` maxima above bound a
/// request rather than the membership it accumulates.) An association that would
/// take a plan past the bound is refused with `AssociatePricingRules`'s modelled
/// `ServiceLimitExceededException` — the one operation of the associate/disassociate
/// pair that declares it — rather than displacing a stored association.
pub const MAX_PRICING_RULES_PER_PRICING_PLAN: usize = 30;

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

/// The tag-key prefix Amazon Web Services keeps for the tags it generates itself.
///
/// Amazon Web Services' tagging rules reserve this prefix service-wide: a caller can
/// neither create nor edit nor delete a tag key carrying it, and a request that tries is
/// refused. The model cannot express the rule — `TagKey` carries only
/// `@length(1..128)` and no pattern (`smithy_query(query="shape",shape="TagKey")`), so
/// the boundary walk admits such a key — which is why every path taking a
/// caller-supplied tag key enforces it through [`check_tag_keys_not_reserved`].
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The comparison is case-insensitive, because the namespace is reserved however it is
/// cased. It is a byte-slice compare rather than a lowercasing one so no allocation
/// happens per key; `get` answers `None` for a key shorter than the prefix, or one whose
/// fourth byte falls inside a multi-byte character, and both of those are exactly "not
/// reserved" — a `TagKey`'s only modelled constraint is its length in characters, so any
/// character at all reaches here.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `Ok(())` when no key in `keys` is reserved, or the modelled `ValidationException`
/// that refuses the request.
///
/// One helper, so every operation that takes caller-supplied tag keys — `TagResource`
/// and `UntagResource`, and the `Tags` of `CreateBillingGroup`, `CreatePricingPlan`,
/// `CreatePricingRule` and `CreateCustomLineItem` — refuses that namespace the same way
/// and cannot drift apart. `member` names the input member that carried the keys, which
/// is what travels in `Fields`, and the message is built from the request: the offenders
/// are sorted and deduplicated, so a map's iteration order never shows through the text.
pub fn check_tag_keys_not_reserved<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), crate::ops::types::ValidationException> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    Err(validation_exception(
        "FIELD_VALIDATION_FAILED",
        member,
        format!(
            "Value at '{member}' failed to satisfy constraint: a tag key must not begin \
             with '{RESERVED_TAG_KEY_PREFIX}', the prefix reserved for the tags Amazon Web \
             Services generates; this request names {}",
            reserved.join(", ")
        ),
    ))
}

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
/// default — so a page is bounded whatever reaches this helper. An operation whose own
/// `MaxResults` range reaches past that default pages through
/// [`paginate_bounded`] instead.
pub fn paginate<T>(
    items: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    paginate_bounded(items, next_token, max_results, DEFAULT_PAGE_SIZE)
}

/// [`paginate`], for an operation whose own `MaxResults` member models a range the
/// default page size does not fit.
///
/// `page_size_bound` is the largest page that operation can be asked for — the maximum
/// of its modelled `MaxResults` range — and is also the size the server picks when the
/// request names none, so a caller that asks for a page the model allows is served it
/// whole and one that asks for nothing still gets a bounded page.
pub fn paginate_bounded<T>(
    items: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
    page_size_bound: i32,
) -> (Vec<T>, Option<String>) {
    let start = next_token.unwrap_or("");
    let mut page: Vec<(String, T)> = items
        .into_iter()
        .filter(|(key, _)| key.as_str() >= start)
        .collect();
    let requested = max_results
        .filter(|max| *max > 0)
        .unwrap_or(page_size_bound)
        .min(page_size_bound);
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

/// How many billing periods back a request may still apply a change to, counted from
/// the current one: the PREVIOUS billing period is the oldest, which is what the
/// operations bounded to it document ("in the current, or previous billing period").
const MODIFIABLE_BILLING_PERIODS_BACK: i64 = 1;

/// Is `billing_period` one of the periods such an operation may apply a change to —
/// the CURRENT billing period or the one before it?
///
/// A period older than that belongs to a bill that is already closed, and one later
/// than the current period to a bill that does not exist yet; neither is the current
/// or the previous billing period. A value that is not a billing period at all
/// ([`billing_period_key`] reads none) is not one of them either.
pub fn billing_period_is_modifiable(billing_period: &str) -> bool {
    let Some(wanted) = billing_period_key(billing_period) else {
        return false;
    };
    let Some(current) = billing_period_key(&current_billing_period()) else {
        return false;
    };
    (current - MODIFIABLE_BILLING_PERIODS_BACK..=current).contains(&wanted)
}

/// May a request carrying `range` be applied by an operation the model documents as
/// acting in the current, or previous, billing period — or is that the refusal it
/// earns?
///
/// `InclusiveStartBillingPeriod` is the period from which the request takes effect, so
/// it is the bound this window judges: a start before the previous billing period
/// restates a closed bill and a start after the current one a bill that does not exist
/// yet, while the exclusive end only says how far forward the change reaches, exactly
/// as an absent end (the open future) does. `ValidationExceptionReason` enumerates
/// both the reason for a value that is no billing period (`ILLEGAL_BILLING_PERIOD`) and
/// the reason for a real period this operation may not act in
/// (`INVALID_BILLING_PERIOD_FOR_OPERATION`).
///
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, so every operation bounded to that
/// window answers an out-of-window range the same way.
pub fn check_billing_period_range_is_modifiable(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> Result<(), ValidationRefusal> {
    let start = &range.inclusive_start_billing_period;
    if billing_period_is_modifiable(start) {
        return Ok(());
    }
    let member = "BillingPeriodRange.InclusiveStartBillingPeriod";
    if billing_period_key(start).is_none() {
        return Err((
            "ILLEGAL_BILLING_PERIOD",
            member,
            format!("{start} is not a billing period"),
        ));
    }
    let current = current_billing_period();
    Err((
        "INVALID_BILLING_PERIOD_FOR_OPERATION",
        member,
        format!(
            "billing period {start} is neither the current billing period ({current}) nor the \
             previous one"
        ),
    ))
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

/// The modelled refusal a request earns when one of the account ids it names is
/// already associated with a DIFFERENT billing group.
///
/// An account belongs to at most one billing group, so an account list naming one
/// another group already owns is a fault in the INPUT, not an inconsistent state of
/// the group addressed: `ValidationExceptionReason` enumerates
/// `ACCOUNTS_ALREADY_ASSOCIATED` for exactly this case, while no
/// `ConflictExceptionReason` value describes it
/// (`smithy_query(query="shape",shape="ValidationExceptionReason,ConflictExceptionReason")`).
/// So the answer is the 400 `ValidationException` with that reason, built here so
/// every operation that takes account ids answers it the same way; `field` names the
/// input member that carried the id (`AccountIds`, `AccountGrouping.LinkedAccountIds`,
/// …), which is what goes in `Fields`.
pub fn accounts_already_associated(
    field: &str,
    account_id: &str,
    owner_group_arn: &str,
) -> crate::ops::types::ValidationException {
    validation_exception(
        "ACCOUNTS_ALREADY_ASSOCIATED",
        field,
        format!(
            "account {account_id} is already associated with billing group {owner_group_arn}"
        ),
    )
}

/// The modelled refusal a request earns when enabling automatic account association
/// would take the consolidated billing family past
/// [`MAX_AUTO_ASSOCIATE_BILLING_GROUPS`] groups that have it enabled.
///
/// Built here so every operation that can turn `AccountGrouping.AutoAssociate` on
/// answers it the same way; `field` names the input member that carried the flag, and
/// `holders` the groups that already have it, which is what the message is built from.
pub fn too_many_auto_associate_billing_groups(
    field: &str,
    holders: &[String],
) -> crate::ops::types::ValidationException {
    validation_exception(
        "TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS",
        field,
        format!(
            "at most {MAX_AUTO_ASSOCIATE_BILLING_GROUPS} billing group of a consolidated \
             billing family may automatically associate newly added accounts; it is already \
             enabled on {}",
            holders.join(", ")
        ),
    )
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

/// The authorization decision for one call of this service's payer-gated surface: whether
/// the caller IS the payer account this store records.
///
/// A READ — it takes the store's lock to compare one account id and writes nothing, so a
/// denied request changes nothing a later caller sees.
///
/// Fails CLOSED: a store this process cannot reach records no payer account, and an
/// authorization check that cannot run must not admit the request. The refusal itself is
/// [`not_payer_account`].
pub fn caller_is_payer_account<Eng: Send + Sync + 'static>(engine: &Eng) -> bool {
    service_state(engine).is_some_and(|state| state.lock().is_payer_account(caller_account_id()))
}

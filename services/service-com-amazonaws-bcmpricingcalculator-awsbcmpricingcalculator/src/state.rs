// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator service.
//!
//! The bill scenario is this service's root resource: a bill estimate is derived from a
//! scenario, and a usage modification only exists inside one, so every modification is
//! stored on its parent scenario rather than in a second map that could drift from it.
//!
//! The workload estimate is the other root resource, kept in its own insertion-ordered
//! collection beside the scenarios, and sharing their id space (every `ResourceId` this
//! service mints is unique across both) and their idempotency map.
//!
//! Beside the resources, the store carries the two facts about the ACCOUNT this service
//! answers for: its rate-type [`Preferences`], and its [`AccountClass`], which decides
//! whether the bill-planning surface is open to the caller at all.

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    BillInterval, Expression, HistoricalUsageEntity, UsageAmount, UsageQuantity,
    ValidationException, WorkloadEstimateUsageQuantity,
};

/// The region an ARN carries when the request's SigV4 credential scope names none.
/// `engine::emulator_arn`'s contract: "emulators default to us-east-1".
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as the Smithy model's `aws.api#service` declares it
/// (`arnNamespace: bcm-pricing-calculator`).
pub const ARN_NAMESPACE: &str = "bcm-pricing-calculator";

/// How long after creation a bill scenario reports it will expire.
///
/// CHOSEN, not documented: neither the Smithy model nor the API reference states a
/// retention span for a bill scenario, so this emulator picks one, reports it
/// consistently through `expiresAt` on create, get and list, and enforces it in
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::purge_expired`].
pub const BILL_SCENARIO_RETENTION_SECS: i64 = 15 * 24 * 60 * 60;

/// The page size a `@paginated` list operation whose page-size member is `MaxResults`
/// uses when the caller sends no `maxResults`: the model's own maximum for that member
/// (`range 1..25`).
pub const MAX_PAGE_SIZE: i32 = 25;

/// The page size `ListWorkloadEstimateUsage` uses when the caller sends no
/// `maxResults`. Same rule as [`MAX_PAGE_SIZE`] — the modelled maximum of that
/// operation's own page-size member — but that member is `WorkloadEstimateUsageMaxResults`
/// (smithy_query: `shape="WorkloadEstimateUsageMaxResults"`, `range 1..300`) rather than
/// `MaxResults`.
pub const WORKLOAD_ESTIMATE_USAGE_MAX_PAGE_SIZE: i32 = 300;

/// The greatest number of bill scenarios this emulator holds at once.
///
/// CHOSEN, not documented: AWS publishes no bill-scenario quota in the model or the
/// API reference. A cap is still required rather than optional, for two reasons: the
/// model declares `ServiceQuotaExceededException` (HTTP 402, "The request would cause
/// you to exceed your service quota") on `CreateBillScenario`, which is unreachable
/// without one; and without it a caller could grow a long-lived emulator's store
/// without bound.
pub const MAX_BILL_SCENARIOS: usize = 1000;

/// The greatest number of usage modifications ONE bill scenario holds at once. Chosen
/// for the same two reasons as [`MAX_BILL_SCENARIOS`], and answered with the same
/// modelled `ServiceQuotaExceededException`.
pub const MAX_USAGE_MODIFICATIONS_PER_BILL_SCENARIO: usize = 2000;

/// How long after creation a workload estimate reports it will expire.
///
/// CHOSEN, not documented, for the same reason as [`BILL_SCENARIO_RETENTION_SECS`]:
/// neither the Smithy model nor the API reference states a retention span for a
/// workload estimate. The same window is used for both resources so one store ages
/// uniformly, it is reported through `expiresAt` on create and get, and it is enforced
/// in [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::purge_expired`].
pub const WORKLOAD_ESTIMATE_RETENTION_SECS: i64 = BILL_SCENARIO_RETENTION_SECS;

/// The greatest number of workload estimates this emulator holds at once. Chosen for
/// the same two reasons as [`MAX_BILL_SCENARIOS`] — `CreateWorkloadEstimate` models
/// `ServiceQuotaExceededException` (HTTP 402), which is unreachable without a cap, and
/// an uncapped store grows for the life of the process — and answered with that same
/// modelled error.
pub const MAX_WORKLOAD_ESTIMATES: usize = 1000;

/// The status a workload estimate carries while the service is still working out its
/// cost: `WorkloadEstimateStatus`'s own non-terminal value (smithy_query:
/// `shape="WorkloadEstimateStatus"`, values `UPDATING | VALID | INVALID |
/// ACTION_NEEDED`).
pub const WORKLOAD_ESTIMATE_STATUS_UPDATING: &str = "UPDATING";

/// The status a workload estimate settles in once the cost it was computing is in:
/// `WorkloadEstimateStatus`'s terminal success value.
pub const WORKLOAD_ESTIMATE_STATUS_VALID: &str = "VALID";

/// How long a workload estimate stays `UPDATING` before the clock owes it
/// `VALID`.
///
/// CHOSEN, not documented: AWS publishes no duration for the recomputation a create
/// starts. It is deliberately NON-ZERO, so a caller reading the estimate straight after
/// creating it still sees the transient status the real service reports, and short
/// enough that a caller polling for the terminal status is not kept waiting.
pub const WORKLOAD_ESTIMATE_UPDATING_MILLIS: i64 = 500;

/// What the `resourceType` member of a modelled error names a workload estimate as:
/// the resource's own Smithy shape name (`com.amazonaws.bcmpricingcalculator#\
/// WorkloadEstimate`), as the bill-scenario handlers name theirs `BillScenario`.
pub const WORKLOAD_ESTIMATE_RESOURCE_TYPE: &str = "WorkloadEstimate";

/// What the `resourceType` member of a modelled error names ONE usage line of a
/// workload estimate as: that line's own Smithy shape name
/// (`com.amazonaws.bcmpricingcalculator#WorkloadEstimateUsageItem`), as the
/// bill-scenario handlers name theirs `BillScenarioUsageModification`.
pub const WORKLOAD_ESTIMATE_USAGE_RESOURCE_TYPE: &str = "WorkloadEstimateUsageItem";

/// The greatest number of usage lines ONE workload estimate holds at once. Chosen for
/// the same two reasons as [`MAX_USAGE_MODIFICATIONS_PER_BILL_SCENARIO`] —
/// `BatchCreateWorkloadEstimateUsage` models `ServiceQuotaExceededException` (HTTP 402),
/// which is unreachable without a cap, and an uncapped store grows for the life of the
/// process — and answered with that same modelled error.
pub const MAX_USAGE_PER_WORKLOAD_ESTIMATE: usize = 2000;

/// The only `CurrencyCode` the model declares (smithy_query: `shape="CurrencyCode"`,
/// values `USD`), so it is the currency every cost this service reports is in.
pub const COST_CURRENCY: &str = "USD";

/// The `WorkloadEstimateCostStatus` a cost that is current carries (smithy_query:
/// `shape="WorkloadEstimateCostStatus"`, values `VALID | INVALID | STALE`). Held here
/// rather than in one handler, so every reader of a usage line's cost qualifies it the
/// same way (see [`WorkloadEstimateUsage::cost_status`]).
pub const COST_STATUS_VALID: &str = "VALID";

/// What the `resourceType` member of a modelled error names a bill scenario as: the
/// resource's own Smithy shape name (`com.amazonaws.bcmpricingcalculator#BillScenario`),
/// the name the bill-scenario handlers already answer with.
pub const BILL_SCENARIO_RESOURCE_TYPE: &str = "BillScenario";

/// The resource-type segment a bill scenario's ARN carries, as `CreateBillScenario`
/// mints it: `arn:aws:bcm-pricing-calculator:<region>:<account>:bill-scenario/<id>`.
pub const BILL_SCENARIO_ARN_TYPE: &str = "bill-scenario";

/// The resource-type segment a workload estimate's ARN carries, as
/// `CreateWorkloadEstimate` mints it:
/// `arn:aws:bcm-pricing-calculator:<region>:<account>:workload-estimate/<id>`.
pub const WORKLOAD_ESTIMATE_ARN_TYPE: &str = "workload-estimate";

/// Every resource kind an ARN of this service names, as `(ARN segment, Smithy shape
/// name)`: the segment is what an `Arn` carries on the wire, the shape name is what a
/// modelled error's `resourceType` reports.
///
/// `bill-estimate` is listed although `CreateBillEstimate` is still a scaffold stub, so
/// an ARN naming a bill estimate is reported as the kind it names rather than as an
/// unrecognised string — the store holds none, so such an ARN resolves to nothing
/// exactly as an ARN naming a deleted resource does.
const ARN_RESOURCE_TYPES: [(&str, &str); 3] = [
    (BILL_SCENARIO_ARN_TYPE, BILL_SCENARIO_RESOURCE_TYPE),
    (WORKLOAD_ESTIMATE_ARN_TYPE, WORKLOAD_ESTIMATE_RESOURCE_TYPE),
    ("bill-estimate", "BillEstimate"),
];

/// The greatest number of tags ONE resource carries at once, INFERRED from the model's
/// maximum for the `Tags` map every tagging operation reads and writes (smithy_query:
/// `shape="Tags"`, `length 0..200`).
///
/// It is an inference from that payload bound, not a service quota AWS states: neither
/// the model nor the API reference publishes a per-resource tag quota for this service
/// (AWS's general tagging guidance is lower, 50 tags per resource). The payload bound is
/// nevertheless a true ceiling on what a resource can carry here, because
/// `ListTagsForResource` reports a resource's tags as a single `Tags` map, so a resource
/// holding more could not be read back. A cap is required rather than optional:
/// `TagResource` models `ServiceQuotaExceededException` (HTTP 402), which is unreachable
/// without one, and without it a caller could grow one resource's tag map without bound.
pub const MAX_RESOURCE_TAGS: usize = 200;

/// The tag-key prefix AWS reserves for its own tags.
///
/// AWS's tagging rules state that the `aws:` prefix is reserved for AWS use and that a
/// caller can neither create nor modify a tag key carrying it, so `TagResource` refuses
/// such a key instead of storing it. The comparison is case-insensitive, because the
/// prefix is reserved however it is cased.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is a tag key AWS reserves for itself (see
/// [`RESERVED_TAG_KEY_PREFIX`]).
///
/// `str::get` rather than a byte-range slice (`key[..4]`), which PANICS when the request's
/// key carries a multi-byte character straddling that byte index — the modelled key
/// pattern `^[\w\s:+=@/-]+$` is matched with a Unicode-aware `\w`, so such a key passes
/// validation and reaches this helper. `get` answers `None` for a key shorter than the
/// prefix or one whose boundary falls mid-character, which is exactly "not reserved".
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The resource-type segment `arn` names (`bill-scenario`, `workload-estimate`, …), or
/// `None` when the string does not have the ARN form this service's `Arn` shape pins.
///
/// The segment is the sixth `:`-field up to its `/`, from the form every ARN this service
/// mints carries: `arn:<partition>:bcm-pricing-calculator:<region>:<account>:<type>/<id>`.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    arn.split(':').nth(5)?.split('/').next()
}

/// What a modelled error's `resourceType` names the resource `arn` addresses as: the
/// resource kind's own Smithy shape name when the ARN names one of this service's kinds
/// ([`ARN_RESOURCE_TYPES`]), and otherwise the segment as the ARN wrote it, since that
/// segment is all that names the thing the caller asked for.
pub fn arn_resource_label(arn: &str) -> String {
    let Some(segment) = arn_resource_type(arn) else {
        // Unreachable through the wire: `Arn`'s pattern pins the ARN form before a
        // handler runs. Reported as the whole string rather than invented, so a
        // hand-built value reaching here is visible instead of silently relabelled.
        return arn.to_string();
    };
    ARN_RESOURCE_TYPES
        .iter()
        .find(|(arn_type, _)| *arn_type == segment)
        .map_or_else(|| segment.to_string(), |(_, shape)| (*shape).to_string())
}

/// One stored usage modification of a bill scenario.
#[derive(Debug, Clone)]
pub struct UsageModification {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// The `key` the batch entry that created this modification carried, so a later
    /// batch response can report an error against the same key.
    pub key: Option<String>,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    pub usage_account_id: String,
    pub availability_zone: Option<String>,
    /// The pricing location of this line. Only resolvable from the pricing catalog,
    /// which this emulator does not carry, so it stays unset.
    pub location: Option<String>,
    pub group: Option<String>,
    pub amounts: Vec<UsageAmount>,
    pub historical_usage: Option<HistoricalUsageEntity>,
}

impl UsageModification {
    /// The modified usage quantities, one per requested `UsageAmount`.
    ///
    /// `unit` stays unset: a usage type's unit of measure comes from the pricing
    /// catalog, which this emulator does not carry, and `UsageQuantity$unit` is
    /// optional precisely so it can be absent.
    pub fn quantities(&self) -> Option<Vec<UsageQuantity>> {
        if self.amounts.is_empty() {
            return None;
        }
        Some(
            self.amounts
                .iter()
                .map(|amount| UsageQuantity {
                    start_hour: Some(amount.start_hour),
                    unit: None,
                    amount: Some(amount.amount),
                })
                .collect(),
        )
    }

    /// The value a `ListUsageFilter` name selects on this modification, or `None` when
    /// this modification carries nothing for it: [`usage_filter_value`] over this
    /// modification's attributes.
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        usage_filter_value(
            &UsageFilterFields {
                usage_account_id: self.usage_account_id.as_str(),
                service_code: self.service_code.as_str(),
                usage_type: self.usage_type.as_str(),
                operation: self.operation.as_str(),
                location: self.location.as_deref(),
                group: self.group.as_deref(),
                historical_usage: self.historical_usage.as_ref(),
            },
            name,
        )
    }
}

/// The attributes a `ListUsageFilter` can select on, borrowed from one stored usage row.
///
/// It is a struct rather than a parameter list so [`usage_filter_value`] names what it
/// reads: this service has two kinds of usage row (a bill scenario's
/// [`UsageModification`] and a workload estimate's [`WorkloadEstimateUsage`]) and both
/// are filtered through the SAME `ListUsageFilters` shape, so both must answer one
/// filter table.
pub struct UsageFilterFields<'a> {
    pub usage_account_id: &'a str,
    pub service_code: &'a str,
    pub usage_type: &'a str,
    pub operation: &'a str,
    /// The row's pricing location. Only resolvable from the price list, which this
    /// emulator does not carry, so no row has one yet and a `LOCATION` filter selects
    /// nothing rather than being refused — the real service does support it.
    pub location: Option<&'a str>,
    pub group: Option<&'a str>,
    pub historical_usage: Option<&'a HistoricalUsageEntity>,
}

/// The value a `ListUsageFilter` name selects on a usage row, or `None` when the row
/// carries nothing for it (which selects the row out).
///
/// ONE table for every usage row this service filters, so a `ListUsageFilterName` the
/// model adds is handled in one place instead of drifting between two copies. The names
/// are that enum's own values (smithy_query: `shape="ListUsageFilterName"`); the
/// `HISTORICAL_*` half reads the row's `historicalUsage`.
pub fn usage_filter_value<'a>(row: &UsageFilterFields<'a>, name: &str) -> Option<&'a str> {
    let historical = row.historical_usage;
    match name {
        "USAGE_ACCOUNT_ID" => Some(row.usage_account_id),
        "SERVICE_CODE" => Some(row.service_code),
        "USAGE_TYPE" => Some(row.usage_type),
        "OPERATION" => Some(row.operation),
        "LOCATION" => row.location,
        "USAGE_GROUP" => row.group,
        "HISTORICAL_USAGE_ACCOUNT_ID" => historical.map(|h| h.usage_account_id.as_str()),
        "HISTORICAL_SERVICE_CODE" => historical.map(|h| h.service_code.as_str()),
        "HISTORICAL_USAGE_TYPE" => historical.map(|h| h.usage_type.as_str()),
        "HISTORICAL_OPERATION" => historical.map(|h| h.operation.as_str()),
        "HISTORICAL_LOCATION" => historical.and_then(|h| h.location.as_deref()),
        _ => None,
    }
}

/// One stored usage line of a workload estimate: the usage a
/// `BatchCreateWorkloadEstimateUsage` entry asked to model, stored on its parent
/// estimate rather than in a second map that could drift from it (as a bill scenario's
/// [`UsageModification`]s are stored on their scenario).
#[derive(Debug, Clone)]
pub struct WorkloadEstimateUsage {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// The `key` the batch entry that created this line carried, so a later batch
    /// response can report an error against the same key.
    pub key: Option<String>,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    pub usage_account_id: String,
    /// The pricing location of this line. Only resolvable from the price list, which
    /// this emulator does not carry, so it stays unset.
    pub location: Option<String>,
    pub group: Option<String>,
    /// The usage amount the entry asked to model, which is what this line's
    /// `quantity.amount` reports back.
    pub amount: f64,
    /// What this line's usage costs, in [`COST_CURRENCY`].
    ///
    /// Pricing a usage type needs the AWS price list, which this emulator does not
    /// carry, so it stays unset rather than carrying a number nothing computed — the
    /// same reason [`Self::location`] and a quantity's `unit` stay unset. It is stored
    /// (rather than assumed absent) because it is what
    /// [`WorkloadEstimate::cost_over_usage`] sums, so a line that ever does carry a
    /// cost is counted without changing that sum's definition.
    pub cost: Option<f64>,
    pub historical_usage: Option<HistoricalUsageEntity>,
}

impl WorkloadEstimateUsage {
    /// The modelled quantity of this line's usage.
    ///
    /// `unit` stays unset: a usage type's unit of measure comes from the price list,
    /// which this emulator does not carry, and `WorkloadEstimateUsageQuantity$unit` is
    /// optional precisely so it can be absent.
    pub fn quantity(&self) -> WorkloadEstimateUsageQuantity {
        WorkloadEstimateUsageQuantity {
            unit: None,
            amount: Some(self.amount),
        }
    }

    /// The value a `ListUsageFilter` name selects on this usage line, or `None` when
    /// this line carries nothing for it: [`usage_filter_value`] over this line's
    /// attributes, the same table a bill scenario's [`UsageModification`] answers,
    /// since both are filtered through the SAME `ListUsageFilters` shape.
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        usage_filter_value(
            &UsageFilterFields {
                usage_account_id: self.usage_account_id.as_str(),
                service_code: self.service_code.as_str(),
                usage_type: self.usage_type.as_str(),
                operation: self.operation.as_str(),
                location: self.location.as_deref(),
                group: self.group.as_deref(),
                historical_usage: self.historical_usage.as_ref(),
            },
            name,
        )
    }

    /// The currency a reader reports this line's cost in: [`COST_CURRENCY`] exactly when
    /// a `cost` is reported, since `currency` says which currency THAT cost is in and has
    /// nothing to qualify without one.
    pub fn cost_currency(&self) -> Option<String> {
        self.cost.map(|_| COST_CURRENCY.to_string())
    }

    /// The `WorkloadEstimateCostStatus` a reader reports for this line: it qualifies the
    /// reported `cost`, so it is reported exactly when a cost is — and the cost a reader
    /// has just computed off stored state is current, hence [`COST_STATUS_VALID`]. With
    /// no cost there is nothing to qualify, so it stays unset rather than asserting a
    /// cost state nothing established.
    pub fn cost_status(&self) -> Option<String> {
        self.cost.map(|_| COST_STATUS_VALID.to_string())
    }
}

/// One stored workload estimate.
#[derive(Debug, Clone)]
pub struct WorkloadEstimate {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// `arn:aws:bcm-pricing-calculator:<region>:<account>:workload-estimate/<id>`,
    /// built through `engine::emulator_arn` so the account segment is the canonical
    /// one.
    ///
    /// This is what the operations that address an estimate by ARN (`TagResource`,
    /// `UntagResource`, `ListTagsForResource`) resolve against, through
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::resource_tags`].
    pub arn: String,
    pub name: String,
    /// A `WorkloadEstimateStatus` value (`UPDATING` | `VALID` | `INVALID` |
    /// `ACTION_NEEDED`).
    pub status: String,
    /// The epoch-millisecond instant at which the clock owes this estimate a move out
    /// of [`WORKLOAD_ESTIMATE_STATUS_UPDATING`], applied by
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::settle`].
    pub status_due_at_millis: i64,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    /// The `WorkloadEstimateRateType` the request asked for, absent when it asked for
    /// none: the model declares no default for the member, so nothing is invented here.
    pub rate_type: Option<String>,
    /// The instant of the pricing rates this estimate is measured against — the moment
    /// it was created, since that is when the rates were read.
    pub rate_timestamp: DateTime,
    /// The estimated cost of this estimate's usage, in [`COST_CURRENCY`].
    ///
    /// Maintained by the operations that change that usage: each of them starts the
    /// recomputation ([`WorkloadEstimate::begin_cost_recompute`]) that
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::settle`] lands
    /// here as [`WorkloadEstimate::cost_over_usage`] once the window closes. An
    /// estimate holding no usage costs `0.0`.
    pub total_cost: f64,
    pub failure_message: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// This estimate's usage lines, insertion-ordered so a paginated read of them is
    /// stable across calls.
    pub usage: Vec<WorkloadEstimateUsage>,
}

impl WorkloadEstimate {
    /// The value a `ListWorkloadEstimatesFilter` name selects on this estimate, or
    /// `None` when this estimate carries nothing for it.
    ///
    /// Names are the `ListWorkloadEstimatesFilterName` enum's own values (smithy_query:
    /// `shape="ListWorkloadEstimatesFilterName"`, values `STATUS | NAME`).
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        match name {
            "STATUS" => Some(self.status.as_str()),
            "NAME" => Some(self.name.as_str()),
            _ => None,
        }
    }

    /// The usage line with this id, if this estimate holds it.
    pub fn usage_line(&self, id: &str) -> Option<&WorkloadEstimateUsage> {
        self.usage.iter().find(|line| line.id == id)
    }

    /// The cost of this estimate's usage: the sum over the costs of its usage lines.
    ///
    /// A line whose cost this emulator cannot resolve contributes nothing (see
    /// [`WorkloadEstimateUsage::cost`]), so an estimate whose lines are all unpriced
    /// costs the same `0.0` an estimate holding no usage does.
    pub fn cost_over_usage(&self) -> f64 {
        self.usage.iter().filter_map(|line| line.cost).sum()
    }

    /// Start the cost recomputation a change to this estimate's usage owes it: the
    /// estimate re-enters the non-terminal status it was created in, and the clock owes
    /// it the terminal one — with the recomputed cost — one
    /// [`WORKLOAD_ESTIMATE_UPDATING_MILLIS`] window later, applied by
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::settle`].
    ///
    /// The window is never zero, so a caller reading the estimate straight after
    /// changing its usage still sees the transient status the real service reports
    /// while it works the new cost out.
    pub fn begin_cost_recompute(&mut self, now_millis: i64) {
        self.status = WORKLOAD_ESTIMATE_STATUS_UPDATING.to_string();
        self.status_due_at_millis = now_millis + WORKLOAD_ESTIMATE_UPDATING_MILLIS;
    }
}

/// One stored bill scenario.
#[derive(Debug, Clone)]
pub struct BillScenario {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// `arn:aws:bcm-pricing-calculator:<region>:<account>:bill-scenario/<id>`, built
    /// through `engine::emulator_arn` so the account segment is the canonical one.
    ///
    /// This is what the operations that address a scenario by ARN (`TagResource`,
    /// `UntagResource`, `ListTagsForResource`) resolve against, through
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::resource_tags`].
    pub arn: String,
    pub name: String,
    /// A `BillScenarioStatus` value (`READY` | `LOCKED` | `STALE` | `FAILED`).
    pub status: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    pub bill_interval_start: DateTime,
    pub bill_interval_end: DateTime,
    pub failure_message: Option<String>,
    pub group_sharing_preference: Option<String>,
    pub cost_category_group_sharing_preference_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Insertion-ordered, so a paginated read of this scenario's modifications is
    /// stable across calls.
    pub usage_modifications: Vec<UsageModification>,
}

impl BillScenario {
    /// The time period this scenario covers, as the modelled structure.
    pub fn bill_interval(&self) -> BillInterval {
        BillInterval {
            start: Some(self.bill_interval_start),
            end: Some(self.bill_interval_end),
        }
    }

    /// The value a `ListBillScenariosFilter` name selects on this scenario, or `None`
    /// when this scenario carries nothing for it.
    ///
    /// Names are the `ListBillScenariosFilterName` enum's own values (smithy_query:
    /// `shape="ListBillScenariosFilterName"`).
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        match name {
            "STATUS" => Some(self.status.as_str()),
            "NAME" => Some(self.name.as_str()),
            "GROUP_SHARING_PREFERENCE" => self.group_sharing_preference.as_deref(),
            "COST_CATEGORY_ARN" => self.cost_category_group_sharing_preference_arn.as_deref(),
            _ => None,
        }
    }

    /// The modification with this id, if this scenario holds it.
    pub fn usage_modification(&self, id: &str) -> Option<&UsageModification> {
        self.usage_modifications.iter().find(|m| m.id == id)
    }
}

/// The rate-type selection an account that has never written one has in effect.
///
/// CHOSEN, not documented: neither the Smithy model nor the API reference states the
/// selection the service applies before anybody writes one. A selection there has to be,
/// though, and it cannot be the empty list: `GetPreferences` is a read of the account's
/// EFFECTIVE preferences — the rate types Pricing Calculator quotes with, which exist
/// from the moment the account does — and `RateTypes` is `@length 1..3` (smithy_query:
/// `shape="RateTypes"`), so an empty or absent selection is not a value the service can
/// legitimately report. The value picked is the un-discounted list rate, `RateType`'s
/// `BEFORE_DISCOUNTS`: the only rate a calculator can quote knowing nothing about an
/// account's discounts or commitments.
pub const DEFAULT_RATE_TYPE_SELECTION: &str = "BEFORE_DISCOUNTS";

/// Which kind of account, in AWS Organizations terms, this service presents its caller
/// as. It decides the account-class gate in
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::member_account_refusal`],
/// so it is stored rather than re-derived per call: every operation of one store answers
/// for the same account.
///
/// The model carries this three-way split itself — `GetPreferences` and
/// `UpdatePreferences` report one rate-type selection per account class
/// (management/member/standalone), and `ValidationExceptionReason` has the value
/// `invalidRequestFromMember` for a request a member account may not make.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountClass {
    /// An account whose bills another account in its organization pays. This is the
    /// DEFAULT, and it is what this emulator presents unless a store is built stating
    /// otherwise: it is the account class the recorded behaviour of this service was
    /// observed in, so the refusals a member account gets are the ones a caller of an
    /// unconfigured emulator sees.
    #[default]
    Member,
    /// The payer (management) account of an organization: it pays its members' bills, so
    /// it may plan them.
    Management,
    /// An account in no organization at all, which pays only for itself.
    Standalone,
}

impl AccountClass {
    /// Whether an account of this class may reach the bill-planning surface — the bill
    /// scenarios and the bill estimates derived from them, and the write of the
    /// account-level rate-type preferences.
    ///
    /// A member account may not: those subjects belong to whichever account pays the
    /// bill being modelled, which is the organization's payer account or, for an account
    /// in no organization, itself.
    pub fn may_plan_bills(self) -> bool {
        matches!(self, Self::Management | Self::Standalone)
    }
}

/// `ValidationExceptionReason`'s value for a request the caller's account class does not
/// permit (smithy_query: `shape="ValidationExceptionReason"` → `unknownOperation |
/// cannotParse | fieldValidationFailed | invalidRequestFromMember | disallowedRate |
/// other`). The wire value is the model's own spelling, so a client that matches on the
/// modelled enum recognises it.
pub const VALIDATION_REASON_INVALID_REQUEST_FROM_MEMBER: &str = "invalidRequestFromMember";

/// The account's Pricing Calculator preferences: the preferred rate types for each
/// kind of account, written by `UpdatePreferences` and reported by `GetPreferences`.
///
/// One record, not one per account: the preference belongs to the account the request
/// is made for, and this emulator serves a single account (`engine::mock_account_id`),
/// so a second key would only ever hold that one record.
///
/// Every selection is held as a NON-EMPTY list rather than an option, because the
/// account's effective preferences exist whether or not anybody wrote them: the record a
/// fresh store holds is [`DEFAULT_RATE_TYPE_SELECTION`] for each of the three account
/// classes, and a write replaces a selection rather than bringing it into being. That is
/// what lets `GetPreferences` report all three members for an account that never called
/// `UpdatePreferences`, which a `@length 1..3` list could not do with an empty or absent
/// value.
#[derive(Debug, Clone)]
pub struct Preferences {
    /// `RateTypes` (len 1..3, of `RateType`: `BEFORE_DISCOUNTS` | `AFTER_DISCOUNTS` |
    /// `AFTER_DISCOUNTS_AND_COMMITMENTS`) — smithy_query: `shape="RateType"`.
    pub management_account_rate_type_selections: Vec<String>,
    pub member_account_rate_type_selections: Vec<String>,
    pub standalone_account_rate_type_selections: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        let default_selection = || vec![DEFAULT_RATE_TYPE_SELECTION.to_string()];
        Self {
            management_account_rate_type_selections: default_selection(),
            member_account_rate_type_selections: default_selection(),
            standalone_account_rate_type_selections: default_selection(),
        }
    }
}

impl Preferences {
    /// Apply one `UpdatePreferences` request: each selection the request supplies
    /// REPLACES the stored one (the member is a whole list, so there is nothing to
    /// merge inside it), and each selection the request omits keeps its stored value.
    /// Every member is individually optional, so an omitted one states nothing about
    /// that preference rather than clearing it.
    pub fn merge(
        &mut self,
        management_account_rate_type_selections: Option<Vec<String>>,
        member_account_rate_type_selections: Option<Vec<String>>,
        standalone_account_rate_type_selections: Option<Vec<String>>,
    ) {
        if let Some(selections) = management_account_rate_type_selections {
            self.management_account_rate_type_selections = selections;
        }
        if let Some(selections) = member_account_rate_type_selections {
            self.member_account_rate_type_selections = selections;
        }
        if let Some(selections) = standalone_account_rate_type_selections {
            self.standalone_account_rate_type_selections = selections;
        }
    }
}

/// What a previous request carrying an `@idempotencyToken` did.
#[derive(Debug, Clone)]
pub struct IdempotentRecord {
    /// A digest of the request that made this record, so the same token replayed with a
    /// DIFFERENT request is recognised as a conflict rather than answered with this
    /// request's result.
    pub fingerprint: String,
    /// The resource ids that request created.
    pub ids: Vec<String>,
}

/// What [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::replay`] found
/// for a request's `@idempotencyToken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replay {
    /// No record: a first request, or no token at all.
    Fresh,
    /// This exact request was already served; these are the ids it created.
    Replayed(Vec<String>),
    /// This token was already used for a DIFFERENT request; these are the ids that
    /// request created, so the conflict can name the resource it already made.
    Mismatch(Vec<String>),
}

/// Per-service state for com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState {
    /// Every live bill scenario, in creation order so a paginated read is stable.
    pub bill_scenarios: Vec<BillScenario>,
    /// Every live workload estimate, in creation order so a paginated read is stable.
    pub workload_estimates: Vec<WorkloadEstimate>,
    /// `<operation>:<clientToken>` -> what that request created, so a replayed
    /// `@idempotencyToken` answers with the first result instead of minting a second
    /// resource. The key carries the operation name because the same token may
    /// legitimately be reused for a different operation.
    pub idempotency: HashMap<String, IdempotentRecord>,
    /// The rate-type preferences `UpdatePreferences` last stored, which
    /// `GetPreferences` reports. Default-constructed: an account that never called
    /// `UpdatePreferences` has the effective selections [`Preferences::default`] holds.
    pub preferences: Preferences,
    /// Which kind of account this store answers for; see [`AccountClass`].
    pub account_class: AccountClass,
}

impl ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState {
    /// The refusal the caller's account class owes this request, or `None` when that
    /// class may make it.
    ///
    /// The bill-planning surface — the bill scenarios, the bill estimates derived from
    /// them, and the write of the account-level rate-type preferences — belongs to the
    /// account that pays the bill being modelled
    /// ([`AccountClass::may_plan_bills`]). A request for one of those subjects from a
    /// member account is refused whatever it carries, so the refusal is answered BEFORE
    /// the request is read and before anything is written, and reports the modelled
    /// `reason` ([`VALIDATION_REASON_INVALID_REQUEST_FROM_MEMBER`]) so a client can tell
    /// an account-class refusal from a malformed-field one.
    ///
    /// `fieldList` is left unset: no field of the request is at fault.
    pub fn member_account_refusal(&self, operation: &str) -> Option<ValidationException> {
        if self.account_class.may_plan_bills() {
            return None;
        }
        Some(ValidationException {
            message: format!(
                "Operation {operation} is not permitted for member accounts. This API is \
                 only allowed for regular or payer accounts."
            ),
            reason: Some(VALIDATION_REASON_INVALID_REQUEST_FROM_MEMBER.to_string()),
            field_list: None,
        })
    }

    /// The scenario with this id, or `None` when it was never created or was deleted.
    pub fn bill_scenario(&self, id: &str) -> Option<&BillScenario> {
        self.bill_scenarios.iter().find(|s| s.id == id)
    }

    /// The scenario with this id, mutably.
    pub fn bill_scenario_mut(&mut self, id: &str) -> Option<&mut BillScenario> {
        self.bill_scenarios.iter_mut().find(|s| s.id == id)
    }

    /// The workload estimate with this id, or `None` when it was never created or was
    /// deleted.
    pub fn workload_estimate(&self, id: &str) -> Option<&WorkloadEstimate> {
        self.workload_estimates.iter().find(|e| e.id == id)
    }

    /// The workload estimate with this id, mutably.
    pub fn workload_estimate_mut(&mut self, id: &str) -> Option<&mut WorkloadEstimate> {
        self.workload_estimates.iter_mut().find(|e| e.id == id)
    }

    /// The tags of the resource `arn` addresses, or `None` when this store holds no
    /// resource with that ARN.
    ///
    /// The ARN is matched against the one the resource was minted with, so an ARN is
    /// resolved to the resource whose ARN it IS — every segment of it, partition, region
    /// and account included. A well-formed ARN naming a resource this store does not hold
    /// (one that was deleted, one that expired, or one of a kind this service cannot yet
    /// create) resolves to nothing, which every tagging operation answers with the
    /// modelled `ResourceNotFoundException`.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(scenario) = self.bill_scenarios.iter().find(|s| s.arn == arn) {
            return Some(&scenario.tags);
        }
        self.workload_estimates
            .iter()
            .find(|e| e.arn == arn)
            .map(|estimate| &estimate.tags)
    }

    /// The tags of the resource `arn` addresses, to be changed in place. `None` for an
    /// ARN this store holds no resource for; see [`Self::resource_tags`].
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(scenario) = self.bill_scenarios.iter_mut().find(|s| s.arn == arn) {
            return Some(&mut scenario.tags);
        }
        self.workload_estimates
            .iter_mut()
            .find(|e| e.arn == arn)
            .map(|estimate| &mut estimate.tags)
    }

    /// Drop the workload estimate with this id. `true` when one was removed.
    pub fn remove_workload_estimate(&mut self, id: &str) -> bool {
        let before = self.workload_estimates.len();
        self.workload_estimates.retain(|e| e.id != id);
        let removed = before != self.workload_estimates.len();
        if removed {
            self.prune_idempotency();
        }
        removed
    }

    /// One fresh `ResourceId`, as [`Self::mint_resource_ids`] mints a batch of them.
    pub fn mint_resource_id(&self) -> String {
        // `mint_resource_ids` returns exactly as many ids as it is asked for, so the
        // fallback is unreachable; it is here so a handler needs no unwrap.
        self.mint_resource_ids(1)
            .pop()
            .unwrap_or_else(candidate_resource_id)
    }

    /// `count` fresh `ResourceId`s, none of which a live scenario, a live usage
    /// modification, or another id of this batch already carries.
    ///
    /// The live-id set is built ONCE per call rather than per candidate, so minting a
    /// batch costs one pass over the store instead of one per row. The rejection loop
    /// is what makes uniqueness exact: a candidate the store already holds is discarded
    /// rather than overwriting somebody's resource, so a caller never loses a row to a
    /// repeated id.
    pub fn mint_resource_ids(&self, count: usize) -> Vec<String> {
        let mut live: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
        for scenario in &self.bill_scenarios {
            live.insert(scenario.id.as_str());
            live.extend(
                scenario
                    .usage_modifications
                    .iter()
                    .map(|modification| modification.id.as_str()),
            );
        }
        for estimate in &self.workload_estimates {
            live.insert(estimate.id.as_str());
            live.extend(estimate.usage.iter().map(|line| line.id.as_str()));
        }
        let mut minted: Vec<String> = Vec::with_capacity(count);
        while minted.len() < count {
            let candidate = candidate_resource_id();
            if !live.contains(candidate.as_str()) && !minted.contains(&candidate) {
                minted.push(candidate);
            }
        }
        minted
    }

    /// Drop the scenario with this id (and, with it, its usage modifications).
    /// `true` when one was removed.
    pub fn remove_bill_scenario(&mut self, id: &str) -> bool {
        let before = self.bill_scenarios.len();
        self.bill_scenarios.retain(|s| s.id != id);
        let removed = before != self.bill_scenarios.len();
        if removed {
            self.prune_idempotency();
        }
        removed
    }

    /// Materialise the expiry the clock has already made due: drop every bill scenario
    /// and workload estimate whose `expiresAt` has passed, and the idempotency entries
    /// that named them.
    ///
    /// The expiry is owed at `now_secs` whether or not anybody called, so even a
    /// reporting operation may apply it. Without it the retention window every reader
    /// advertises through `expiresAt` would never be enforced, and the store would grow
    /// for the life of the process.
    pub fn purge_expired(&mut self, now_secs: i64) {
        let before = (self.bill_scenarios.len(), self.workload_estimates.len());
        self.bill_scenarios
            .retain(|scenario| scenario.expires_at.secs() > now_secs);
        self.workload_estimates
            .retain(|estimate| estimate.expires_at.secs() > now_secs);
        if before != (self.bill_scenarios.len(), self.workload_estimates.len()) {
            self.prune_idempotency();
        }
    }

    /// Materialise EVERYTHING the clock has already made due at `now_millis`: the
    /// expiry [`Self::purge_expired`] applies, and the move out of the transient
    /// `UPDATING` status a workload estimate was owed once its recomputation window
    /// closed — which lands the recomputed cost
    /// ([`WorkloadEstimate::cost_over_usage`]) with it, since that recomputation is
    /// what the transient status was reporting.
    ///
    /// Both changes were owed at `now_millis` whether or not anybody called, which is
    /// why a reporting operation may apply them; every operation that resolves a
    /// workload estimate calls this first, so no reader reports a status the clock has
    /// already moved on from.
    pub fn settle(&mut self, now_millis: i64) {
        self.purge_expired(now_millis.div_euclid(1000));
        for estimate in &mut self.workload_estimates {
            if estimate.status == WORKLOAD_ESTIMATE_STATUS_UPDATING
                && estimate.status_due_at_millis <= now_millis
            {
                let recomputed = estimate.cost_over_usage();
                estimate.status = WORKLOAD_ESTIMATE_STATUS_VALID.to_string();
                estimate.total_cost = recomputed;
            }
        }
    }

    /// Forget every resource id an idempotency record names that no longer exists, and
    /// drop a record left naming nothing.
    ///
    /// A consequence worth stating: once the resources a token created are deleted (or
    /// expired), that token no longer replays, so the same `clientToken` sent again
    /// creates a new resource rather than answering with the vanished one. The
    /// alternative — keeping records for a retention period of their own — would let a
    /// caller grow this map without bound by creating and deleting under fresh tokens,
    /// and would have a replay promise nothing can honour, since the resource it names
    /// is gone.
    ///
    /// Pruning per id rather than per record keeps the record and the replay it
    /// promises in agreement: a replay answers with exactly the resources that still
    /// exist, instead of falling through (because one id of a batch vanished) and
    /// minting a second set under the same token. And a record naming nothing could
    /// never answer a replay at all, so keeping it would only let a caller grow this
    /// map by repeatedly creating and deleting under fresh tokens.
    pub fn prune_idempotency(&mut self) {
        // Nothing to prune, and the live-id set is the expensive part: the delete and
        // expiry paths reach here on every call.
        if self.idempotency.is_empty() {
            return;
        }
        // Destructured so the live set can BORROW the scenario ids while the records
        // are mutated: they are disjoint fields, which a whole-`self` borrow would not
        // let the compiler see.
        let Self {
            bill_scenarios,
            workload_estimates,
            idempotency,
            preferences: _,
            account_class: _,
        } = self;
        let live: std::collections::HashSet<&str> = bill_scenarios
            .iter()
            .flat_map(|scenario| {
                std::iter::once(scenario.id.as_str()).chain(
                    scenario
                        .usage_modifications
                        .iter()
                        .map(|modification| modification.id.as_str()),
                )
            })
            .chain(workload_estimates.iter().flat_map(|estimate| {
                std::iter::once(estimate.id.as_str())
                    .chain(estimate.usage.iter().map(|line| line.id.as_str()))
            }))
            .collect();
        idempotency.retain(|_, record| {
            record.ids.retain(|id| live.contains(id.as_str()));
            !record.ids.is_empty()
        });
    }

    /// What a request's `@idempotencyToken` says about this request: a first request, a
    /// genuine retry of the same one, or the same token used for a different one.
    pub fn replay(
        &self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> Replay {
        let Some(token) = client_token else {
            return Replay::Fresh;
        };
        match self.idempotency.get(&idempotency_key(operation, token)) {
            None => Replay::Fresh,
            Some(record) if record.fingerprint == fingerprint => {
                Replay::Replayed(record.ids.clone())
            }
            Some(record) => Replay::Mismatch(record.ids.clone()),
        }
    }

    /// Record what this request created against its `@idempotencyToken`.
    ///
    /// A request that carried no token, or created nothing, records nothing: a record
    /// naming no resource could never answer a replay and would never be pruned.
    pub fn record_idempotent(
        &mut self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
        ids: Vec<String>,
    ) {
        if ids.is_empty() {
            return;
        }
        if let Some(token) = client_token {
            self.idempotency.insert(
                idempotency_key(operation, token),
                IdempotentRecord {
                    fingerprint: fingerprint.to_string(),
                    ids,
                },
            );
        }
    }
}

fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// A stable digest of the request parts an `@idempotencyToken` is scoped to, and of
/// the selection a page of results was measured against.
///
/// It is what lets a genuine retry (the same token AND the same request) be told from a
/// token reused for a different request, which is a conflict rather than a replay. The
/// parts are length-prefixed, so `["a", "bc"]` and `["ab", "c"]` do not collide.
///
/// Digesting rather than keeping the parts matters for the second use: the digest
/// travels inside a `NextPageToken`, which the model caps at 2048 characters, while a
/// request's `filters` are unbounded.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    use sha2::Digest;

    let mut digest = sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// How deep a `filterExpression` is digested before the digest stops descending.
///
/// `Expression` nests through caller-supplied `and`/`or`/`not`, so
/// [`push_expression_digest`] is bounded rather than trusting the request's depth. A
/// request deeper than this digests to the same value past the cap, which costs only
/// that two such requests could share one idempotency token — never a crash.
pub const MAX_EXPRESSION_DEPTH: usize = 32;

/// Push the digest parts of one `HistoricalUsageEntity`, the shape every batch entry of
/// this service carries, so each of those operations fingerprints it the same way.
///
/// Written member by member rather than from a derived `Debug`, so the digest is a
/// contract: ADD A LINE HERE when the model gives `HistoricalUsageEntity` another
/// member, or two requests differing only in that member will share one fingerprint.
/// [`request_fingerprint`] length-prefixes every part, so parts cannot run together.
pub fn push_historical_usage_digest(parts: &mut Vec<String>, historical: &HistoricalUsageEntity) {
    parts.push(historical.service_code.clone());
    parts.push(historical.usage_type.clone());
    parts.push(historical.operation.clone());
    parts.push(historical.location.clone().unwrap_or_default());
    parts.push(historical.usage_account_id.clone());
    parts.push(
        historical
            .bill_interval
            .start
            .map(|start| start.secs())
            .unwrap_or_default()
            .to_string(),
    );
    parts.push(
        historical
            .bill_interval
            .end
            .map(|end| end.secs())
            .unwrap_or_default()
            .to_string(),
    );
    push_expression_digest(parts, &historical.filter_expression, 0);
}

/// Push the digest parts of a filter expression, descending at most
/// [`MAX_EXPRESSION_DEPTH`] levels.
pub fn push_expression_digest(parts: &mut Vec<String>, expression: &Expression, depth: usize) {
    if depth > MAX_EXPRESSION_DEPTH {
        return;
    }
    for (member, filter) in [
        ("costCategories", &expression.cost_categories),
        ("dimensions", &expression.dimensions),
        ("tags", &expression.tags),
    ] {
        if let Some(filter) = filter {
            parts.push(member.to_string());
            parts.push(filter.key.clone().unwrap_or_default());
            parts.extend(filter.match_options.clone().unwrap_or_default());
            parts.extend(filter.values.clone().unwrap_or_default());
        }
    }
    for (member, branch) in [("and", &expression.and), ("or", &expression.or)] {
        for child in branch.iter().flatten() {
            parts.push(member.to_string());
            push_expression_digest(parts, child, depth + 1);
        }
    }
    if let Some(negated) = &expression.not {
        parts.push("not".to_string());
        push_expression_digest(parts, negated, depth + 1);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState>)
}

/// Wall-clock seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Wall-clock milliseconds since the Unix epoch, for the sub-second instants a
/// transient status is due at.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// A candidate `ResourceId`: the lowercase-hex v4 UUID form this service's
/// `ResourceId` shape declares
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, len 36..36), with
/// RFC 9562's version nibble and variant bits set, so it is a well-formed v4 value and
/// not merely 32 hex characters.
///
/// The 128 bits come from the standard library's `RandomState`, which the OS seeds, and
/// a process-wide sequence number so two ids minted in the same nanosecond differ.
/// Uniqueness is then made EXACT rather than probabilistic by
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::mint_resource_id`],
/// which rejects a candidate the store already holds — the property the service needs,
/// since a repeated id would overwrite a caller's resource.
fn candidate_resource_id() -> String {
    use ::std::hash::{BuildHasher, Hasher};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let half = |salt: u64| -> u64 {
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u64(sequence);
        hasher.write_u128(nanos);
        hasher.finish()
    };
    // The version nibble (4) and the two variant bits (0b10) are the only fixed bits of
    // a v4 UUID; everything else is random.
    let high = (half(0) & 0xffff_ffff_ffff_0fff) | 0x0000_0000_0000_4000;
    let low = (half(1) & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffff_ffff_ffff,
    )
}

/// The UTC calendar month containing `secs`, as `[start, end)` epoch seconds — the
/// billing period a bill scenario covers.
pub fn month_bounds(secs: i64) -> (i64, i64) {
    use ::aws_smithy_types::date_time::Format;

    let bounds = || -> Option<(i64, i64)> {
        // The calendar reading and the conversion back both come from
        // `aws_smithy_types::DateTime`, the crate this service already serialises its
        // timestamps with: `Format::DateTime` renders the instant as RFC 3339
        // (`2026-10-22T09:30:00Z`), whose first seven characters are the calendar year
        // and month, and `DateTime::from_str` turns the first instant of a month back
        // into epoch seconds. The only arithmetic written here is the month rollover.
        let text = DateTime::from_secs(secs).fmt(Format::DateTime).ok()?;
        let year: i32 = text.get(..4)?.parse().ok()?;
        let month: u32 = text.get(5..7)?.parse().ok()?;
        let first_instant_of = |year: i32, month: u32| -> Option<i64> {
            let text = format!("{year:04}-{month:02}-01T00:00:00Z");
            Some(DateTime::from_str(&text, Format::DateTime).ok()?.secs())
        };
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        Some((
            first_instant_of(year, month)?,
            first_instant_of(next_year, next_month)?,
        ))
    };
    // An instant no calendar can render (a year outside RFC 3339's four digits) has no
    // calendar month; report the instant itself rather than a fabricated interval.
    bounds().unwrap_or((secs, secs))
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// The value comes from the caller's SigV4 credential scope, so it is only accepted
/// when it has the shape of a region (lowercase alphanumerics and hyphens, 1..32
/// characters); anything else falls back to the default rather than being interpolated
/// into an ARN the real service could never emit.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            (1..=32).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `candidate` satisfies a filter's `values` under its `matchOption`.
///
/// The `MatchOption` enum's values are `EQUALS` | `STARTS_WITH` | `CONTAINS`
/// (smithy_query: `shape="MatchOption"`); a filter that sends no `matchOption` is an
/// exact match. The values of one filter are OR-ed.
pub fn matches_filter(candidate: &str, values: &[String], match_option: Option<&str>) -> bool {
    values.iter().any(|value| match match_option {
        Some("STARTS_WITH") => candidate.starts_with(value.as_str()),
        Some("CONTAINS") => candidate.contains(value.as_str()),
        _ => candidate == value.as_str(),
    })
}

/// Whether `moment` falls inside a `FilterTimestamp` window, which is inclusive at
/// both ends and open at an end the caller left unset.
///
/// The inclusivity of each bound is CHOSEN, not documented: `FilterTimestamp`'s members
/// say only "Include results after/before this timestamp" (smithy_query:
/// `shape="FilterTimestamp"`), which does not settle whether the bound itself is
/// included. Both bounds are inclusive here, consistently for every window this service
/// applies, and that choice is pinned by a test row on each boundary instant.
pub fn within_window(
    moment: DateTime,
    after: Option<DateTime>,
    before: Option<DateTime>,
) -> bool {
    let key = (moment.secs(), moment.subsec_nanos());
    if let Some(after) = after {
        if key < (after.secs(), after.subsec_nanos()) {
            return false;
        }
    }
    if let Some(before) = before {
        if key > (before.secs(), before.subsec_nanos()) {
            return false;
        }
    }
    true
}

/// The prefix every pagination token this service emits carries, so a token minted by
/// another service (or a hand-written one) is rejected rather than silently read as an
/// offset.
const PAGE_TOKEN_PREFIX: &str = "bcmpc-offset:";

/// What a caller is told about a token this service did not issue for THIS request.
const UNISSUED_PAGE_TOKEN: &str = "nextToken is not a token this service issued for this request";

/// The opaque `nextToken` naming the item at `offset` in an insertion-ordered
/// collection, bound to `scope`.
///
/// `scope` is a [`request_fingerprint`] of the request the page belongs to (its
/// operation and the selection it asked for), so an offset is only ever applied to the
/// list it was measured against: a token from another operation, or from the same
/// operation under different `filters`, is refused rather than silently skipping or
/// repeating items.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{scope}:{offset}"))
}

/// The offset [`encode_page_token`] encoded for `scope`, or `Err` with the reason the
/// token is not one this service issued for this request.
///
/// A token is meaningful only to the process that minted it: the offset indexes an
/// in-memory collection, so it is refused anywhere else, which is the safe direction.
pub fn decode_page_token(token: &str, scope: &str) -> Result<usize, String> {
    let decoded = hex::decode(token).map_err(|_| UNISSUED_PAGE_TOKEN.to_string())?;
    let text = String::from_utf8(decoded).map_err(|_| UNISSUED_PAGE_TOKEN.to_string())?;
    let body = text
        .strip_prefix(PAGE_TOKEN_PREFIX)
        .ok_or_else(|| UNISSUED_PAGE_TOKEN.to_string())?;
    let (token_scope, offset) = body
        .rsplit_once(':')
        .ok_or_else(|| UNISSUED_PAGE_TOKEN.to_string())?;
    if token_scope != scope {
        return Err(UNISSUED_PAGE_TOKEN.to_string());
    }
    offset
        .parse::<usize>()
        .map_err(|_| UNISSUED_PAGE_TOKEN.to_string())
}

/// One page of `items`, starting at the offset `next_token` names.
///
/// `max_results` is the caller's page size, defaulting to the model's maximum, and
/// `scope` binds the token to this request (see [`encode_page_token`]). The returned
/// token is `Some` only while items remain after this page, so a caller that paginates
/// to the end sees it absent — and an offset past the end is an empty last page rather
/// than an error, because the item a live token named may have been deleted in between.
pub fn paginate<T: Clone>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    scope: &str,
) -> Result<(Vec<T>, Option<String>), String> {
    paginate_sized(items, next_token, max_results, MAX_PAGE_SIZE, scope)
}

/// [`paginate`], for an operation whose `maxResults` member is modelled with a
/// different maximum than [`MAX_PAGE_SIZE`] — `default_page_size` is the page size a
/// caller that sends no `maxResults` gets, which this service takes to be that
/// operation's own modelled maximum.
pub fn paginate_sized<T: Clone>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    default_page_size: i32,
    scope: &str,
) -> Result<(Vec<T>, Option<String>), String> {
    let offset = match next_token {
        Some(token) if !token.is_empty() => decode_page_token(token, scope)?,
        _ => 0,
    };
    // Every `maxResults` member of this service is modelled with a minimum of 1, so the
    // boundary validator has already refused anything below it; refuse rather than
    // clamp, so a page size that somehow reached here is reported instead of silently
    // becoming 1.
    let page_size = match max_results {
        Some(requested) if requested < 1 => {
            return Err(format!("maxResults must be at least 1, got {requested}"))
        }
        Some(requested) => requested as usize,
        None => default_page_size as usize,
    };
    if offset >= items.len() {
        return Ok((Vec::new(), None));
    }
    let end = (offset + page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = (end < items.len()).then(|| encode_page_token(scope, end));
    Ok((page, token))
}

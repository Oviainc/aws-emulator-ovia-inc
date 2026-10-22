// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-budgets-awsbudgetservicegateway service.
//!
//! The resource this service is built around is the **Budget**. AWS gives a budget
//! its identity from the account that owns it plus its name — `Budget$BudgetName`
//! is documented as "The name must be unique within an account" — so the store is
//! keyed by that pair. A budget OWNS the notifications, their subscribers and the
//! resource tags created with it ("After the end date, Amazon Web Services deletes
//! the budget and all the associated notifications and subscribers"), so they are
//! held inside the stored budget and go away with it.
//!
//! The helpers below (ARN rendering, the opaque pagination token, the cross-member
//! rules `CreateBudget`/`UpdateBudget` document) are shared by several operation
//! handlers, so they live here rather than being copied into each `src/ops` file —
//! a new file under `src/ops/` would be picked up by `build.rs` as an operation.

use crate::ops::types::{
    Budget, CostTypes, HealthStatus, Notification, ResourceTag, Spend, Subscriber, TimePeriod,
};
use std::collections::HashMap;
use std::time::SystemTime;

/// Per-service state for com-amazonaws-budgets-awsbudgetservicegateway. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBudgetsAwsbudgetservicegatewayState {
    /// Every budget this emulator holds, keyed by `(AccountId, BudgetName)`.
    pub budgets: HashMap<(String, String), StoredBudget>,
}

/// One persisted budget: the modelled shape as the caller last supplied it, with the
/// members the service owns stamped over it ([`stamp_service_owned`]), plus the
/// notifications and tags created alongside it.
///
/// No ARN is stored: nothing in this slice reports one. The operations that resolve a
/// budget by ARN (`TagResource` / `UntagResource` / `ListTagsForResource`) are still
/// 501 and filed as their own issues, and they should build it through the runtime's
/// `engine::emulator_arn` rather than a string this module formats.
#[derive(Debug, Clone)]
pub struct StoredBudget {
    /// The budget as `DescribeBudget` / `DescribeBudgets` read it back.
    pub budget: Budget,
    /// The notifications created with this budget, in the order they were supplied.
    pub notifications: Vec<StoredNotification>,
    /// `ResourceTags` supplied at creation.
    pub tags: Vec<ResourceTag>,
}

/// A notification plus the subscribers attached to it. A notification cannot exist
/// without at least one subscriber (`Subscribers` is `@length(min: 1)`), so the two
/// are stored together.
#[derive(Debug, Clone)]
pub struct StoredNotification {
    pub notification: Notification,
    pub subscribers: Vec<Subscriber>,
}

/// `HealthStatusValue$HEALTHY` — the only state a budget in this emulator can be in:
/// every budget here derives from a billing view this emulator always serves, so
/// nothing can make it `UNHEALTHY` (which is why no `StatusReason` is reported either:
/// `HealthStatus$StatusReason` is "the reason for the current status", and a healthy
/// resource has none).
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";

/// `NotificationState$OK` — a notification that has not passed its threshold.
/// "Specifies whether this notification is in alarm. If a budget notification is in the
/// `ALARM` state, you passed the set threshold for the budget."
pub const NOTIFICATION_STATE_OK: &str = "OK";

/// Seconds since the Unix epoch at `at`, clamped at the epoch for an earlier instant.
fn unix_secs(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The wire form a budget timestamp is rendered in and read back from, so the calendar
/// is the one the runtime's own `DateTime` implements rather than arithmetic written
/// here: `YYYY-MM-DDThh:mm:ssZ`.
const CIVIL_FORMAT: ::aws_smithy_types::date_time::Format =
    ::aws_smithy_types::date_time::Format::DateTime;

/// The instant `civil` names, which is `YYYY-MM-DDT00:00:00Z` for every caller here.
///
/// `None` for a string that is not a date-time, which none of the callers below can
/// produce (each builds it from a rendered date).
fn midnight_at(civil: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(civil, CIVIL_FORMAT).ok()
}

/// The year, month and day of `at` in UTC, read off the runtime's own rendering of it
/// (`YYYY-MM-DD...`), so no date arithmetic is written here.
fn civil_date(at: SystemTime) -> Option<(i64, u32, u32)> {
    let rendered = ::aws_smithy_types::DateTime::from_secs(unix_secs(at))
        .fmt(CIVIL_FORMAT)
        .ok()?;
    let date = rendered.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    Some((year, month, day))
}

/// The end date AWS gives a budget whose caller supplied none: "If you didn't specify
/// an end date, Amazon Web Services set your end date to `06/15/87 00:00 UTC`"
/// (`TimePeriod$End`). Written as the date it is rather than as an epoch literal.
pub fn default_time_period_end() -> ::aws_smithy_types::DateTime {
    midnight_at("2087-06-15T00:00:00Z").expect("06/15/87 00:00 UTC is a date-time")
}

/// The start date AWS gives a budget whose caller supplied none: "Amazon Web Services
/// defaults to the start of your chosen time period (DAILY, MONTHLY, QUARTERLY,
/// ANNUALLY, or CUSTOM) ... if you created your budget on January 24, 2018, chose
/// `DAILY`, and didn't set a start date, Amazon Web Services set your start date to
/// `01/24/18 00:00 UTC`. If you chose `MONTHLY`, Amazon Web Services set your start date
/// to `01/01/18 00:00 UTC`" (`TimePeriod$Start`) — the midnight-UTC boundary of the
/// period `time_unit` names that contains `at`.
///
/// `CUSTOM` names no period length, so it takes the same boundary as `DAILY`: the day
/// the budget was written. A `TimeUnit` this emulator does not know yet takes that
/// boundary too, which is the narrowest period the enum has.
///
/// The boundary is chosen on the rendered date and turned back into an instant by the
/// runtime's own date-time parser, so this holds no calendar arithmetic of its own.
pub fn default_time_period_start(time_unit: &str, at: SystemTime) -> ::aws_smithy_types::DateTime {
    let Some((year, month, day)) = civil_date(at) else {
        // Unreachable for any instant the runtime can render; the instant itself is
        // still a sound answer for "the period containing it".
        return ::aws_smithy_types::DateTime::from_secs(unix_secs(at));
    };
    let (month, day) = match time_unit {
        "MONTHLY" => (month, 1),
        // Calendar quarters: the first month of the quarter `month` falls in.
        "QUARTERLY" => (((month.saturating_sub(1)) / 3) * 3 + 1, 1),
        "ANNUALLY" => (1, 1),
        _ => (month, day),
    };
    midnight_at(&format!("{year:04}-{month:02}-{day:02}T00:00:00Z"))
        .unwrap_or_else(|| ::aws_smithy_types::DateTime::from_secs(unix_secs(at)))
}

/// The per-member defaults `CostTypes` documents: "The default value is `true`" on every
/// `Include*` member, and `false` on `UseBlended` and `UseAmortized`. A member the caller
/// set keeps its value.
fn apply_cost_type_defaults(cost_types: &mut CostTypes) {
    for include in [
        &mut cost_types.include_tax,
        &mut cost_types.include_subscription,
        &mut cost_types.include_refund,
        &mut cost_types.include_credit,
        &mut cost_types.include_upfront,
        &mut cost_types.include_recurring,
        &mut cost_types.include_other_subscription,
        &mut cost_types.include_support,
        &mut cost_types.include_discount,
    ] {
        if include.is_none() {
            *include = Some(true);
        }
    }
    for rate in [&mut cost_types.use_blended, &mut cost_types.use_amortized] {
        if rate.is_none() {
            *rate = Some(false);
        }
    }
}

/// Whether this budget HAS cost types at all.
///
/// Two documented rules decide it: "`USAGE`, `RI_UTILIZATION`, `RI_COVERAGE`,
/// `SAVINGS_PLANS_UTILIZATION`, and `SAVINGS_PLANS_COVERAGE` budgets don't have
/// `CostTypes`" (`CostTypes`), so only a `COST` budget does; and "only one set of filter
/// and metric selections can be present in the syntax at one time. Either
/// `FilterExpression` and `Metrics` or `CostFilters` and `CostTypes`, not both"
/// (`CreateBudget`), so a budget written in the `FilterExpression`/`Metrics` syntax has
/// none either — defaulting them there would store exactly the combination that rule
/// forbids (and which [`validate_budget`] refuses from a caller).
fn cost_types_apply(budget: &Budget) -> bool {
    budget.budget_type == COST_BUDGET_TYPE
        && budget.filter_expression.is_none()
        && budget.metrics.is_none()
}

/// The `BudgetType` that has cost types.
const COST_BUDGET_TYPE: &str = "COST";

/// Stamp the members of `budget` the SERVICE owns, on the `CreateBudget` /
/// `UpdateBudget` write path, so that every later read reports what was stored rather
/// than recomputing it:
///
/// * `LastUpdatedTime` — "the last time that you updated this budget", so a value the
///   caller sent is replaced by the time of this write.
/// * `TimePeriod` — a budget always covers a period; an absent `Start` or `End` takes
///   its documented default (see [`default_time_period_start`] / [`default_time_period_end`]).
/// * `CostTypes` — the unset members of a `COST` budget's cost types take their
///   documented defaults, unless the budget uses the `FilterExpression`/`Metrics` syntax
///   instead (see [`cost_types_apply`]).
/// * `HealthStatus` — the operational state of the billing-view-derived resource, which
///   the caller does not set. `previous` carries the status the budget already had: the
///   status did not change because somebody edited the budget, so neither does its
///   `LastUpdatedTime`.
pub fn stamp_service_owned(budget: &mut Budget, at: SystemTime, previous: Option<HealthStatus>) {
    let stamped = ::aws_smithy_types::DateTime::from(at);
    budget.last_updated_time = Some(stamped);

    let default_start = default_time_period_start(&budget.time_unit, at);
    let period = budget.time_period.get_or_insert_with(TimePeriod::default);
    if period.start.is_none() {
        period.start = Some(default_start);
    }
    if period.end.is_none() {
        period.end = Some(default_time_period_end());
    }

    if cost_types_apply(budget) {
        apply_cost_type_defaults(budget.cost_types.get_or_insert_with(CostTypes::default));
    }

    budget.health_status = Some(previous.unwrap_or_else(|| HealthStatus {
        status: Some(HEALTH_STATUS_HEALTHY.to_string()),
        status_reason: None,
        last_updated_time: Some(stamped),
    }));
}

/// Stamp the members of `notification` the SERVICE owns, on the write path that creates
/// it: a notification the caller did not place in alarm starts out of alarm
/// ([`NOTIFICATION_STATE_OK`]).
pub fn stamp_notification(notification: &mut Notification) {
    if notification.notification_state.is_none() {
        notification.notification_state = Some(NOTIFICATION_STATE_OK.to_string());
    }
}

/// The `PlannedBudgetLimits` entry in force at `at`.
///
/// The map's key is the start of the planned month or quarter, "`UTC` in epoch seconds"
/// (`Budget$PlannedBudgetLimits`), so the entry in force is the latest period that has
/// started. "After all of the `BudgetLimit` values in `PlannedBudgetLimits` are used, the
/// budget continues to use the last limit as the `BudgetLimit`" — which is the same entry
/// once no later period has started. A budget whose planned periods all begin in the
/// future ("If the budget begins at a date in the future, provide `PlannedBudgetLimits`
/// values from the start date of the budget") reports its first.
///
/// A key that is not an epoch names no period, so it is skipped; `None` for a map with
/// no usable entry at all.
pub fn planned_limit_in_force(planned: &HashMap<String, Spend>, at: SystemTime) -> Option<Spend> {
    let now = unix_secs(at);
    let mut periods: Vec<(i64, &Spend)> = planned
        .iter()
        .filter_map(|(start, limit)| start.trim().parse::<i64>().ok().map(|at| (at, limit)))
        .collect();
    periods.sort_by_key(|(start, _)| *start);
    periods
        .iter()
        .rev()
        .find(|(start, _)| *start <= now)
        .or_else(|| periods.first())
        .map(|(_, limit)| (*limit).clone())
}

/// The `Budget` a read reports for `stored`, as of `at`.
///
/// Every member is the one the write path stored, except the `BudgetLimit` of a
/// planned-limit budget, which is derived: "`DescribeBudget` and `DescribeBudgets`
/// response along with `PlannedBudgetLimits` also contain `BudgetLimit` representing the
/// current month or quarter limit present in `PlannedBudgetLimits`"
/// (`Budget$PlannedBudgetLimits`). It is derived on the READ and never written back —
/// which entry is in force follows the clock, not anything a caller did.
pub fn project_budget(stored: &StoredBudget, at: SystemTime) -> Budget {
    let mut budget = stored.budget.clone();
    if budget.budget_limit.is_none() {
        if let Some(planned) = &budget.planned_budget_limits {
            budget.budget_limit = planned_limit_in_force(planned, at);
        }
    }
    budget
}

/// Marker the opaque pagination token carries, so a token minted by another
/// service (or invented by a caller) does not decode here.
const NEXT_TOKEN_PREFIX: &str = "budgets:offset:";

/// Encode an offset into the opaque `NextToken` a paginated operation hands back.
pub fn encode_next_token(offset: usize) -> String {
    hex::encode(format!("{NEXT_TOKEN_PREFIX}{offset}"))
}

/// Decode a `NextToken` minted by [`encode_next_token`].
///
/// `None` for anything else — the caller answers that with
/// `InvalidNextTokenException` ("The pagination token is invalid").
pub fn decode_next_token(token: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(NEXT_TOKEN_PREFIX)?.parse().ok()
}

/// One page of `items`, plus the token for the page after it.
///
/// `max_results` is the modelled `@paginated` page size (already range-checked at
/// the boundary); absent means "every remaining item". `next_token` is the token a
/// previous page returned. `None` is returned when `next_token` does not decode, or
/// decodes to an offset this list never had a page at — which the caller reports as
/// `InvalidNextTokenException`.
pub fn page<T: Clone>(
    items: &[T],
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => {
            let offset = decode_next_token(token)?;
            // A token is only minted while items remain, so its offset was below the
            // length of the list it was minted for. An offset at or past the end was
            // never minted for THIS list: it is stale or tampered with, not a last page.
            if offset >= items.len() {
                return None;
            }
            offset
        }
        None => 0,
    };
    let page_size = match max_results {
        // At least one item per page: a zero page would hand back a token pointing at
        // the same offset, which is a caller loop that never terminates. The modelled
        // `@range(min: 1)` already makes this unreachable from the wire.
        Some(max) => usize::try_from(max).unwrap_or(1).max(1),
        None => items.len(),
    };
    let end = start.saturating_add(page_size).min(items.len());
    let next = (end < items.len()).then(|| encode_next_token(end));
    Some((items[start..end].to_vec(), next))
}

/// Every budget stored for `account_id`, as a read reports it at `at`, ordered by
/// `BudgetName`.
///
/// The order is the one the pagination token indexes into, so it must not depend
/// on `HashMap` iteration order. Only the `Budget` is projected — the notifications,
/// subscribers and tags hanging off a stored budget are not reported by a list call.
pub fn budgets_for_account(
    state: &ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    account_id: &str,
    at: SystemTime,
) -> Vec<Budget> {
    let mut found: Vec<&StoredBudget> = state
        .budgets
        .iter()
        .filter(|((account, _), _)| account == account_id)
        .map(|(_, stored)| stored)
        .collect();
    found.sort_by(|a, b| a.budget.budget_name.cmp(&b.budget.budget_name));
    found
        .into_iter()
        .map(|stored| project_budget(stored, at))
        .collect()
}

/// How many budgets `account_id` already holds — what the per-account quota below is
/// checked against.
pub fn budget_count_for_account(
    state: &ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    account_id: &str,
) -> usize {
    state
        .budgets
        .keys()
        .filter(|(account, _)| account == account_id)
        .count()
}

/// AWS Budgets' documented default service quota: "Budgets: 20,000 per account"
/// (Billing and Cost Management service quotas). `CreateBudget` answers
/// `ServiceQuotaExceededException` ("You've reached a Service Quota limit on this
/// resource", HTTP 402) once an account holds this many.
pub const MAX_BUDGETS_PER_ACCOUNT: usize = 20_000;

/// The whole store lives in memory, and both halves of a budget's key come from the
/// request, so the number of ACCOUNTS a caller can name is unbounded too. This is the
/// emulator's own ceiling on the total number of budgets it will hold — not an AWS
/// quota — so a client cannot grow the process without limit by varying `AccountId`.
/// It is answered with the same modelled `ServiceQuotaExceededException`.
pub const MAX_BUDGETS_TOTAL: usize = 100_000;

/// `BudgetName`'s modelled `@pattern`, verbatim from the model
/// (`smithy_query(query="shape",shape="BudgetName")`). The rule
/// [`budget_name_matches_pattern`] enforces, in one place, so the code and the
/// comment above it cannot drift from the model.
pub const BUDGET_NAME_PATTERN: &str =
    r"^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$";

/// Whether `name` satisfies [`BUDGET_NAME_PATTERN`] — documented on `BudgetName` as
/// "The ':' and '\' characters, and the '/action/' substring, aren't allowed", with
/// names "validated for content".
///
/// Checked here rather than left to the boundary validator because that pattern uses
/// a negative look-ahead, which the framework's regex engine cannot compile, and an
/// uncompilable `@pattern` is treated as matching ("we never reject a value because of
/// an unparseable model pattern"). The `@length(1, 100)` half of the constraint IS
/// enforced upstream, so it is not repeated here.
fn budget_name_matches_pattern(name: &str) -> bool {
    // `[^:\\]+`: at least one character, and no ':' or '\' anywhere. Any name holding
    // one of those two also fails the look-ahead's `[^:\\]*/action/` alternative, so
    // rejecting them here covers "/action/" with a prefix of other characters too.
    if name.is_empty() || name.contains(':') || name.contains('\\') {
        return false;
    }
    if name.contains("/action/") {
        return false;
    }
    // `(?i).*<script>.*</script>.*` — the case-insensitive half of the look-ahead.
    let lowered = name.to_lowercase();
    !lowered
        .split_once("<script>")
        .is_some_and(|(_, rest)| rest.contains("</script>"))
}

/// The refusal a `BudgetName` outside its modelled `@pattern` takes, or `None` when
/// the name satisfies it.
///
/// A value outside a MODELLED constraint is refused at request validation, before the
/// operation runs — the same class of refusal as a bad `AccountId` or an out-of-range
/// `MaxResults`, which the boundary validator answers with HTTP 400
/// `ValidationException` and a message naming the failing member path and the
/// constraint it violated. This pattern reaches a handler only because the framework's
/// regex engine cannot compile a negative look-ahead, so the refusal states the same
/// two facts, in the clause shape the framework's own renderer uses
/// (`N validation error(s) detected: Value … at '<path>' failed to satisfy constraint:
/// <requirement>` — [`::aws_smithy_schema_server::Violation::message`]).
///
/// It states them WITHOUT the rejected name and without the pattern, which is where it
/// departs from that renderer. `BudgetName` is a content rule, not a syntax rule: the
/// model documents that "Budget names are validated for content. Names that contain
/// phone numbers, URLs, or email addresses combined with certain terms may be
/// rejected", and the pattern itself refuses `<script>…</script>`. Echoing a name back
/// because it was refused for its content reflects exactly the content the rule exists
/// to keep out of this service, and restating the filter tells a caller how to get
/// around it. The member path and the constraint are what a caller needs to fix the
/// request, and they are what AWS's recorded refusal states.
///
/// `member_path` is the trail of Smithy member names from the operation input inwards
/// (`["Budget", "BudgetName"]` for `CreateBudget`, `["BudgetName"]` for the
/// operations that take the name directly); each segment is lowerCamel-ed by the
/// framework's own path builder.
pub fn budget_name_violation(member_path: &[&str], name: &str) -> Option<String> {
    if budget_name_matches_pattern(name) {
        return None;
    }
    let path: Vec<String> = member_path
        .iter()
        .map(|segment| ::aws_smithy_schema_server::constraints::path::member(segment))
        .collect();
    // Every segment is a Smithy member name chosen by a call site in this crate, so
    // nothing a caller sent reaches the sentence below.
    let path = ::aws_smithy_schema_server::constraints::path::join(&path);
    Some(format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern"
    ))
}

/// The cross-member rules `CreateBudget` and `UpdateBudget` document for the
/// `Budget` they accept, none of which a Smithy constraint trait can express.
///
/// A rule the model DOES express is not checked here: a value outside a modelled
/// constraint is a request-validation refusal, answered by
/// [`budget_name_violation`]'s renderers, not by this service's own
/// `InvalidParameterException`.
///
/// `Err` carries the sentence that goes in `InvalidParameterException$Message`
/// ("An error on the client occurred. Typically, the cause is an invalid input
/// value."). Every rule here is quoted from the model's own documentation:
///
/// * "You can't use `BudgetLimit` with `PlannedBudgetLimits` for `CreateBudget`
///   and `UpdateBudget` actions." (`Budget$BudgetLimit`)
/// * "only one set of filter and metric selections can be present in the syntax at
///   one time. Either `FilterExpression` and `Metrics` or `CostFilters` and
///   `CostTypes`, not both or a different combination." (`CreateBudget`)
/// * "The start date must come before the end date." (`Budget$TimePeriod`)
pub fn validate_budget(budget: &Budget) -> Result<(), String> {
    if budget.budget_limit.is_some() && budget.planned_budget_limits.is_some() {
        return Err(
            "Unable to create/update budget: BudgetLimit can't be used with \
             PlannedBudgetLimits."
                .to_string(),
        );
    }
    let legacy_filters = budget.cost_filters.is_some() || budget.cost_types.is_some();
    let expression_filters = budget.filter_expression.is_some() || budget.metrics.is_some();
    if legacy_filters && expression_filters {
        return Err(
            "Unable to create/update budget: FilterExpression and Metrics can't be used with \
             CostFilters and CostTypes."
                .to_string(),
        );
    }
    if let Some(period) = &budget.time_period {
        if let (Some(start), Some(end)) = (&period.start, &period.end) {
            let start_before_end = (start.secs(), start.subsec_nanos())
                < (end.secs(), end.subsec_nanos());
            if !start_before_end {
                return Err(
                    "Unable to create/update budget: TimePeriod Start must come before End."
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBudgetsAwsbudgetservicegatewayState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>)
}

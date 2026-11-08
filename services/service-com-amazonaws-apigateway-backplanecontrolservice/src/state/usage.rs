// SPDX-License-Identifier: CC0-1.0

//! The USAGE a usage plan's quota is metered against: how many requests each API key of a
//! plan sent on each day, and the daily `[used quota, remaining quota]` log a read of that
//! usage reports from it.
//!
//! A usage plan carries two kinds of limit, and this module holds the second one's
//! accounting. The `throttle` is a RATE — the token bucket beside this
//! ([`super::ThrottleBucket`]) admits it — while the `quota` is a COUNT: "the target
//! maximum number of requests that can be made in a given time period"
//! (`QuotaSettings$limit`), counted per `(usage plan, API key)` pair for the same reason
//! the throttle is, and reset at the start of every period the plan's `period` names.
//!
//! Both halves of the count live here so the `execute-api` data plane that WRITES it
//! (`crate::service::invoke`, one count per request a plan admitted) and the read that
//! REPORTS it answer from one rule: what the report calls used is what the data plane
//! counted, and what it calls remaining is what the next request would be admitted
//! against.

use std::collections::BTreeMap;

use ::chrono::{Datelike, Days, NaiveDate};

use super::{metering_key, ComAmazonawsApigatewayBackplanecontrolserviceState, UsagePlanEntry};

/// The format every date of this surface travels in: `GetUsage`'s `startDate` and
/// `endDate` are "the starting date (e.g., 2016-01-01)" and "the ending date (e.g.,
/// 2016-12-31) of the usage data" (`smithy_query(query="doc", shape="GetUsageRequest",
/// member="startDate")`), and the reported `startDate`/`endDate` are the same days in the
/// same spelling.
const USAGE_DATE_FORMAT: &str = "%Y-%m-%d";

/// The day `value` names, or `None` for a value that is no such date.
///
/// Whole days only, in the one format above: a value carrying a time, an offset or any
/// other calendar spelling names no day of the usage data and is refused
/// ([`INVALID_USAGE_START_DATE`]).
pub fn usage_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, USAGE_DATE_FORMAT).ok()
}

/// `day` as this surface spells a date, so a reported `startDate`/`endDate` is the day the
/// report covers rather than the bytes the request happened to carry.
pub fn usage_date_string(day: NaiveDate) -> String {
    day.format(USAGE_DATE_FORMAT).to_string()
}

/// The UTC day `at` falls on — the day a metered request is counted against.
///
/// UTC because a usage plan's quota is: a period "resets at midnight UTC", so the day a
/// request belongs to is its UTC day and not the day of any local zone.
pub fn usage_day_of(at: ::std::time::SystemTime) -> NaiveDate {
    ::chrono::DateTime::<::chrono::Utc>::from(at).date_naive()
}

/// The UTC day now, as the data plane counts a request it is metering.
pub fn usage_today() -> NaiveDate {
    usage_day_of(::std::time::SystemTime::now())
}

/// The widest interval of days one read of the usage data may cover.
///
/// An EMULATOR RESOURCE GUARD rather than a published quota: the report holds one entry
/// per day per metered key, so an interval a caller names has to be bounded for the answer
/// to be. Ninety days is the window API Gateway's usage data is available over, so the
/// guard admits every interval the data can cover and refuses only an interval no usage
/// could be reported for ([`usage_interval_too_wide`]). It also bounds what is KEPT: a
/// count older than this is dropped as the data plane records a newer one
/// ([`UsageLog::record`]).
pub const MAX_USAGE_INTERVAL_DAYS: u64 = 90;

/// The requests one metered `(usage plan, API key)` pair sent, counted by the UTC day each
/// one fell on.
///
/// Only the days the pair SENT something on are held — a day with no entry is a day of no
/// requests, which is what a report of zero used is — so the log of an idle key costs
/// nothing and a busy one grows by one entry per day, bounded by
/// [`MAX_USAGE_INTERVAL_DAYS`].
#[derive(Debug, Clone, Default)]
pub struct UsageLog {
    /// The request count of each day the pair sent a request on.
    pub per_day: BTreeMap<NaiveDate, i64>,
}

impl UsageLog {
    /// How many requests the pair sent on `day`.
    pub fn used_on(&self, day: NaiveDate) -> i64 {
        self.per_day.get(&day).copied().unwrap_or_default()
    }

    /// How many requests the pair sent from `from` through `through`, both days included —
    /// what a quota period's own limit is counted down by.
    pub fn used_between(&self, from: NaiveDate, through: NaiveDate) -> i64 {
        self.per_day
            .range(from..=through)
            .map(|(_, count)| *count)
            .fold(0i64, i64::saturating_add)
    }

    /// Count one request sent on `day`, forgetting the counts that fell outside the window
    /// a report may cover ([`MAX_USAGE_INTERVAL_DAYS`]).
    fn record(&mut self, day: NaiveDate) {
        let counted = self.per_day.entry(day).or_default();
        *counted = counted.saturating_add(1);
        if let Some(oldest) = day.checked_sub_days(Days::new(MAX_USAGE_INTERVAL_DAYS)) {
            self.per_day.retain(|counted, _| *counted > oldest);
        }
    }
}

/// The quota window a day falls in: when the metering period that holds it began, and how
/// many requests the plan admits in that period.
#[derive(Debug, Clone, Copy)]
pub struct QuotaWindow {
    /// The first day of the period — where the count the limit is applied to starts, which
    /// is what makes the quota RESET at a period boundary.
    pub start: NaiveDate,
    /// The requests admissible in the period: the plan's `limit`, less the `offset` in the
    /// initial period.
    pub budget: i64,
}

/// The quota window `day` falls in for `plan`, or `None` for a plan that meters no quota in
/// a window — one carrying no `quota` at all, and one whose quota names no `period`, which
/// is a limit with no window to apply it in.
///
/// `offset` is "the number of requests subtracted from the given limit in the initial time
/// period" (`QuotaSettings$offset`), and the INITIAL period is the one the plan's own
/// creation day falls in: every later period starts at the whole limit again, which is the
/// reset the period names.
pub fn quota_window(plan: &UsagePlanEntry, day: NaiveDate) -> Option<QuotaWindow> {
    let quota = plan.quota.as_ref()?;
    let period = quota.period.as_deref()?;
    let start = quota_period_start(period, day)?;
    let initial = plan
        .created_on
        .and_then(|created| quota_period_start(period, created))
        .is_some_and(|created_period| created_period == start);
    let offset = if initial { i64::from(quota.offset) } else { 0 };
    Some(QuotaWindow {
        start,
        budget: i64::from(quota.limit).saturating_sub(offset).max(0),
    })
}

/// The first day of the metering `period` that `day` falls in, or `None` for a period this
/// service knows no window for.
///
/// The periods are the modelled `QuotaPeriodType` values, matched as
/// [`super::max_quota_offset`] matches them. Each window is the CALENDAR one its name
/// gives it — a day, the week its day starts the count of, or the month it is a day of —
/// because a quota "resets at midnight UTC" at the start of its period rather than at the
/// end of a rolling count: that is what makes the first day of a month or a week the day a
/// quota is whole again.
pub fn quota_period_start(period: &str, day: NaiveDate) -> Option<NaiveDate> {
    match period {
        "DAY" => Some(day),
        "WEEK" => day.checked_sub_days(Days::new(u64::from(
            day.weekday().num_days_from_sunday(),
        ))),
        "MONTH" => day.with_day(1),
        _ => None,
    }
}

impl ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// Count one request from the API key `key_id` against the quota of `usage_plan_id` on
    /// the UTC day `day`, and report whether that quota admitted it.
    ///
    /// The count is the one thing a quota is: a period admits [`QuotaWindow::budget`]
    /// requests, so a request arriving once the period's own count has reached it is NOT
    /// admitted and NOT counted — the period is already at its limit, and a refused
    /// request consumed none of it. A plan that meters no quota ([`quota_window`]) admits
    /// every request and still counts it, because what it sent is what a read of the usage
    /// reports as used.
    ///
    /// This is the ONE place a usage plan's quota is accounted, so the data plane and every
    /// read of the usage data agree on how many requests a key has sent and how many it has
    /// left.
    pub fn quota_admits(&mut self, usage_plan_id: &str, key_id: &str, day: NaiveDate) -> bool {
        let window = self
            .usage_plans
            .get(usage_plan_id)
            .and_then(|plan| quota_window(plan, day));
        let counted = metering_key(usage_plan_id, key_id);
        if let Some(window) = window {
            let spent = self
                .usage_logs
                .get(&counted)
                .map_or(0, |log| log.used_between(window.start, day));
            if spent >= window.budget {
                return false;
            }
        }
        self.usage_logs.entry(counted).or_default().record(day);
        true
    }

    /// Whether the pair has usage data over the days `from` through `through`: whether it
    /// sent any request on one of them.
    ///
    /// What a report of the usage data is INDEXED by: a key the plan meters but that sent
    /// nothing over the interval has no daily log in it.
    pub fn has_usage_data(
        &self,
        usage_plan_id: &str,
        key_id: &str,
        from: NaiveDate,
        through: NaiveDate,
    ) -> bool {
        self.usage_logs
            .get(&metering_key(usage_plan_id, key_id))
            .is_some_and(|log| log.used_between(from, through) > 0)
    }

    /// The daily log of used and remaining quota one `(usage plan, API key)` pair earns
    /// over the days `from` through `through`, both included: one `[used, remaining]` entry
    /// per day, in date order.
    ///
    /// `used` is what the pair sent THAT DAY and `remaining` what the quota has left AFTER
    /// it — the period's budget less everything the pair sent in that period up to and
    /// including the day — so a day in a new period reports the whole budget again however
    /// much the one before it consumed. A pair metered by no quota has no budget to count
    /// down and reports `QuotaSettings$limit`'s own default of zero remaining.
    pub fn daily_usage(
        &self,
        usage_plan_id: &str,
        key_id: &str,
        from: NaiveDate,
        through: NaiveDate,
    ) -> Vec<Vec<i64>> {
        let plan = self.usage_plans.get(usage_plan_id);
        let log = self.usage_logs.get(&metering_key(usage_plan_id, key_id));
        let mut entries = Vec::new();
        let mut day = from;
        while day <= through {
            let used = log.map_or(0, |log| log.used_on(day));
            let remaining = plan
                .and_then(|plan| quota_window(plan, day))
                .map_or(0, |window| {
                    let spent = log.map_or(0, |log| log.used_between(window.start, day));
                    window.budget.saturating_sub(spent).max(0)
                });
            entries.push(vec![used, remaining]);
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }
        entries
    }
}

/// The `BadRequestException` message for a `startDate` that names no day.
///
/// A SENTENCE OF ITS OWN, like [`super::NO_SUCH_USAGE_PLAN`]: it names the member that is
/// wrong and quotes the rejected value back nowhere.
pub const INVALID_USAGE_START_DATE: &str = "Invalid start date specified";

/// The `BadRequestException` message for an `endDate` that names no day; see
/// [`INVALID_USAGE_START_DATE`].
pub const INVALID_USAGE_END_DATE: &str = "Invalid end date specified";

/// The `BadRequestException` message for an interval whose end precedes its start: the
/// days it covers are none at all, so there is no usage data it could report.
pub const INVALID_USAGE_INTERVAL: &str =
    "Invalid usage interval specified. The start date must not follow the end date";

/// The `BadRequestException` message for an interval wider than the window a report may
/// cover; see [`MAX_USAGE_INTERVAL_DAYS`] for what bounds it.
pub fn usage_interval_too_wide() -> String {
    format!(
        "Invalid usage interval specified. The usage interval must be at most \
         {MAX_USAGE_INTERVAL_DAYS} days"
    )
}

/// The interval `start_date` through `end_date` covers, or the sentence the pair is refused
/// with: both must name a day, in order, no wider than the window a report may cover.
///
/// One check for both reads of the usage data, so the interval a report covers is admitted
/// by one rule whichever operation asked for it.
pub fn usage_interval(start_date: &str, end_date: &str) -> Result<(NaiveDate, NaiveDate), String> {
    let from = usage_date(start_date).ok_or_else(|| INVALID_USAGE_START_DATE.to_string())?;
    let through = usage_date(end_date).ok_or_else(|| INVALID_USAGE_END_DATE.to_string())?;
    if through < from {
        return Err(INVALID_USAGE_INTERVAL.to_string());
    }
    // Both endpoints are covered, so an interval of one day is the pair that names the
    // same day twice.
    let covered = (through - from).num_days().saturating_add(1);
    if covered > i64::try_from(MAX_USAGE_INTERVAL_DAYS).unwrap_or(i64::MAX) {
        return Err(usage_interval_too_wide());
    }
    Ok((from, through))
}

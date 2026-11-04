// SPDX-License-Identifier: CC0-1.0

//! Sampling statistics, reservoir shares and boosts: what services report for a
//! sampling rule, the windows those reports accumulate into, and the share of a
//! rule's reservoir each reporting service is allocated out of them.
//!
//! One concern, one file: `GetSamplingTargets` writes here and
//! `GetSamplingStatisticSummaries` reads here, so the window span, the pool
//! division and the spans a grant stays live for are each stated once.

use std::collections::BTreeMap;

use super::{
    now_epoch_seconds, timestamp_of, MAX_SAMPLING_RULES, MAX_SEGMENT_EPOCH_SECONDS,
};
use crate::ops::types::{SamplingRateBoost, SamplingStatisticSummary};

/// How long one sampling reporting window is, in seconds.
///
/// The model documents the span twice: a `SamplingStatisticsDocument` carries
/// results "for the last 10 seconds unless the service has been assigned a
/// longer reporting interval after a previous call to GetSamplingTargets", and a
/// `SamplingStatisticSummary` is "aggregated request sampling data for a
/// sampling rule across all services for a 10-second window"
/// (`smithy_query(query="shape",shape="SamplingStatisticsDocument")`). Reported
/// results are therefore accumulated per rule into windows of this length, and
/// a summary reports one of them.
pub const SAMPLING_WINDOW_SECONDS: i64 = 10;

/// `SamplingTargetDocument$Interval`: "the number of seconds for the service to
/// wait before getting sampling targets again".
///
/// Served only on a document that GRANTS a share of a rule's reservoir, which
/// is what the member is for: the service is told to come back for its next
/// share when the one it was just granted expires
/// ([`SAMPLING_ALLOCATION_TTL_SECONDS`]), one reporting window
/// ([`SAMPLING_WINDOW_SECONDS`]) later. A document that grants no share assigns
/// no cadence either, so it carries no interval at all and the reporting
/// service keeps the one it came with — the case a `SamplingStatisticsDocument`
/// describes as its default, "the last 10 seconds unless the service has been
/// assigned a longer reporting interval after a previous call".
pub const SAMPLING_TARGET_INTERVAL_SECONDS: i32 = 10;

/// How long an allocated reservoir quota and a granted sampling boost stay
/// live, in seconds.
///
/// CHOSEN, not documented: the allocation is granted for the interval the same
/// document assigns the service ([`SAMPLING_TARGET_INTERVAL_SECONDS`]), which is
/// the reporting window it reports over ([`SAMPLING_WINDOW_SECONDS`]), so a
/// service that comes back when it was told to is answered the share the cycle
/// it just took part in computed, and one that goes quiet falls back to the
/// rule's `FixedRate` instead of holding a share of the pool forever.
pub const SAMPLING_ALLOCATION_TTL_SECONDS: f64 = 10.0;

/// How long a computed allocation stays live after the cycle that computed it,
/// in seconds.
///
/// CHOSEN: an allocation outlives the cycle that produced it, because it is
/// what a service is told at its NEXT report — including when the rule itself
/// has left the account in between, which does not cancel the share the service
/// was already granted. Once it has aged past this, a service reporting for a
/// rule the account no longer holds is answered as unprocessed.
pub const SAMPLING_ALLOCATION_CACHE_SECONDS: f64 = 60.0;

/// How long a service's report keeps it in the pool sharing a rule's reservoir,
/// in seconds.
///
/// CHOSEN: several missed reporting intervals, after which a service is no
/// longer counted when the reservoir is shared out, so a service that stops
/// calling stops diluting the share of the ones still reporting.
pub const SAMPLING_CLIENT_REPORT_TTL_SECONDS: f64 = 60.0;

/// The most reporting windows one rule keeps — about half an hour of them.
/// Reaching it drops the oldest window, so a service reporting forever cannot
/// grow the store without bound.
pub const MAX_SAMPLING_WINDOWS_PER_RULE: usize = 180;

/// The most services one rule's reservoir is shared between. Reaching it drops
/// the least recently reporting service.
pub const MAX_SAMPLING_CLIENTS_PER_RULE: usize = 100;

/// The most services one rule records a boost grant for. Reaching it drops the
/// least recently granted service, so a caller naming a fresh `ServiceName` on
/// every call cannot grow the store without bound.
pub const MAX_SAMPLING_BOOST_SERVICES_PER_RULE: usize = 100;

/// The most rules the statistics store tracks: the rules an account may hold
/// ([`MAX_SAMPLING_RULES`]) plus the service-managed catch-all. Reaching it
/// drops the rule whose last report is the oldest, so a create/delete cycle
/// through many names cannot grow the store without bound.
pub const MAX_SAMPLING_STATISTIC_RULES: usize = MAX_SAMPLING_RULES + 1;

/// The start of the [`SAMPLING_WINDOW_SECONDS`] window the instant `seconds`
/// falls in, in whole seconds of epoch time.
///
/// Windows are aligned to the epoch rather than to whichever instant reported
/// first, so two services reporting the same ten seconds accumulate into ONE
/// window and the summary of it is one row. Bounded exactly as
/// [`timestamp_of`] is, so no reported instant can produce a window start a
/// reader cannot answer.
pub fn sampling_window_start(seconds: f64) -> i64 {
    let bounded = if seconds.is_nan() {
        0.0
    } else {
        seconds.clamp(-MAX_SEGMENT_EPOCH_SECONDS, MAX_SEGMENT_EPOCH_SECONDS)
    };
    #[allow(clippy::cast_possible_truncation)]
    let whole = bounded.floor() as i64;
    whole - whole.rem_euclid(SAMPLING_WINDOW_SECONDS)
}

/// A summed counter as the modelled `Integer` member of a
/// `SamplingStatisticSummary` carries it.
///
/// Counters are summed as `i64` so no sequence of reports overflows a total,
/// and the wire member is a 32-bit `Integer`: a total past what it can carry is
/// reported at its maximum rather than wrapping to a count nobody reported.
fn wire_count(total: i64) -> i32 {
    i32::try_from(total).unwrap_or(i32::MAX)
}

/// One service's reported results for one rule: what a
/// `SamplingStatisticsDocument` carries, in this module's units.
#[derive(Debug, Clone, Copy)]
pub struct ReportedSamplingCounts {
    /// The document's `Timestamp` ("the current time"), in seconds of epoch
    /// time — which window the results are accumulated into.
    pub timestamp: f64,
    /// `RequestCount`: "the number of requests that matched the rule".
    pub request_count: i64,
    /// `SampledCount`: "the number of requests recorded".
    pub sampled_count: i64,
    /// `BorrowCount`: "the number of requests recorded with borrowed reservoir
    /// quota".
    pub borrow_count: i64,
}

/// A set of sampling counters for one rule and one reporting window: either
/// what ONE service states for that window, or — once every reporting service's
/// is added up — what a `SamplingStatisticSummary` is ("aggregated request
/// sampling data for a sampling rule across all services for a 10-second
/// window").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SamplingStatisticsWindow {
    /// The `RequestCount` of the window.
    pub request_count: i64,
    /// The `SampledCount` of the window.
    pub sampled_count: i64,
    /// The `BorrowCount` of the window.
    pub borrow_count: i64,
}

impl SamplingStatisticsWindow {
    /// The counters one reported `SamplingStatisticsDocument` states.
    fn of(report: ReportedSamplingCounts) -> Self {
        Self {
            request_count: report.request_count,
            sampled_count: report.sampled_count,
            borrow_count: report.borrow_count,
        }
    }

    /// Add another service's counters for the same window, saturatingly, so no
    /// number of reporting services can overflow a total.
    fn add(&mut self, other: Self) {
        self.request_count = self.request_count.saturating_add(other.request_count);
        self.sampled_count = self.sampled_count.saturating_add(other.sampled_count);
        self.borrow_count = self.borrow_count.saturating_add(other.borrow_count);
    }
}

/// One row of the account's reported sampling results: which rule and which
/// window, and the counters every service reporting that window stated.
///
/// `GetSamplingStatisticSummaries` reports these, so the rows a reader walks
/// are recomputed from what services reported rather than echoed from any one
/// request.
#[derive(Debug, Clone)]
pub struct SamplingStatisticsEntry {
    /// The rule the results were reported for.
    pub rule_name: String,
    /// The start of the window, in whole seconds of epoch time — the summary's
    /// `Timestamp`, "the start time of the reporting window".
    pub window_start: i64,
    /// The counters of the window, added up across its reporting services.
    pub counts: SamplingStatisticsWindow,
}

impl SamplingStatisticsEntry {
    /// This row as the modelled `SamplingStatisticSummary`.
    pub fn to_summary(&self) -> SamplingStatisticSummary {
        #[allow(clippy::cast_precision_loss)]
        let window_start = self.window_start as f64;
        SamplingStatisticSummary {
            rule_name: Some(self.rule_name.clone()),
            timestamp: Some(timestamp_of(window_start)),
            request_count: wire_count(self.counts.request_count),
            borrow_count: wire_count(self.counts.borrow_count),
            sampled_count: wire_count(self.counts.sampled_count),
        }
    }

    /// The pagination token that resumes a listing AT this row.
    ///
    /// It NAMES the row — its rule and its window — so a token for a row the
    /// store no longer holds is refused rather than answered with an arbitrary
    /// page, exactly as `GetSamplingRules`' token is.
    pub fn token(&self) -> String {
        format!("{}|{}", self.rule_name, self.window_start)
    }
}

/// What one service last reported for one rule, and the allocation the service
/// HOLDS for it.
#[derive(Debug, Clone, Copy)]
pub struct SamplingClientReport {
    /// When the report was accepted, in seconds of epoch time — what
    /// [`SAMPLING_CLIENT_REPORT_TTL_SECONDS`] is measured from.
    pub reported_at: f64,
    /// The `RequestCount` it reported: this service's share of the traffic the
    /// rule matched.
    pub request_count: i64,
    /// The allocation computed for this service at the END of a reporting cycle
    /// it took part in, which is what its NEXT report is answered with. `None`
    /// until a cycle has run for it.
    pub allocation: Option<SamplingAllocation>,
}

/// The allocation one reporting cycle computed for one service and one rule.
///
/// A service is told the allocation of the cycle it already took part in, never
/// one computed out of the report it is making right now: before its first
/// report there is no cycle it was part of, so it holds no reservoir share at
/// all and samples at the rule's fixed rate until the next cycle has run.
#[derive(Debug, Clone, Copy)]
pub struct SamplingAllocation {
    /// The share of the rule's reservoir the cycle gave this service.
    pub reservoir_quota: i32,
    /// When the cycle ran, in seconds of epoch time.
    pub allocated_at: f64,
}

/// The pool one allocation cycle divided, as that cycle found the rule it is
/// for configured.
///
/// Held per rule NAME, because that is what a `SamplingTargetDocument`
/// identifies what it allocates by: the target computation answers a report out
/// of the division it last made for that name, so it keeps the rate and the
/// pool size that division was made under rather than re-reading the rule
/// store. A name no cycle has divided a pool for has none of this, which is the
/// only case a reported document is answered as unprocessed.
#[derive(Debug, Clone, Copy)]
pub struct SamplingPool {
    /// The rule's `FixedRate` as the cycle found it.
    pub fixed_rate: f64,
    /// The rule's `ReservoirSize` as the cycle found it — the size of the pool
    /// the cycle divided.
    pub reservoir_size: i32,
    /// When the cycle divided it, in seconds of epoch time.
    pub divided_at: f64,
}

/// The account's sampling results and the allocations computed from them.
///
/// `GetSamplingTargets` writes here — recording what each service reported and
/// reading back the share of each rule's reservoir that report entitles it to —
/// and `GetSamplingStatisticSummaries` reports the accumulated windows. Nothing
/// in a response is echoed from the request that stored it: a target document's
/// `FixedRate` comes from the stored rule and its `ReservoirQuota` from the
/// reports of every service using that rule.
#[derive(Debug)]
pub struct SamplingStatisticsStore {
    /// When the account's sampling-rule configuration last changed, in seconds
    /// of epoch time — `GetSamplingTargetsResult$LastRuleModification`, "the
    /// last time a user changed the sampling rule configuration", which a
    /// service compares against to decide whether to re-read the rules.
    ///
    /// Stamped by every create, update and delete of a rule. Until one happens
    /// it is when this account's configuration came into being, which is a
    /// FIXED instant rather than a reading of the clock that would move on
    /// every read.
    pub rules_modified_at: f64,
    /// Reported results per rule, per [`SAMPLING_WINDOW_SECONDS`] window keyed
    /// by the window's start, and within a window per reporting service's
    /// `ClientID` — at most [`MAX_SAMPLING_WINDOWS_PER_RULE`] windows per rule
    /// and [`MAX_SAMPLING_CLIENTS_PER_RULE`] services per window.
    ///
    /// Per SERVICE rather than one running total, because a summary aggregates
    /// "across all services" for a window: what a service states for a window
    /// is that service's whole contribution to it, so a service restating the
    /// window replaces its own figures instead of being counted twice.
    windows: BTreeMap<String, BTreeMap<i64, BTreeMap<String, SamplingStatisticsWindow>>>,
    /// The latest report each service made for a rule, keyed by rule and then
    /// by the reporting service's `ClientID`, at most
    /// [`MAX_SAMPLING_CLIENTS_PER_RULE`] of them per rule.
    clients: BTreeMap<String, BTreeMap<String, SamplingClientReport>>,
    /// The pool the latest allocation cycle divided for a rule, keyed by rule
    /// NAME ([`SamplingPool`]) — the state the target computation answers a
    /// report out of, which is why it survives the rule leaving the store.
    pools: BTreeMap<String, SamplingPool>,
    /// When a rule's sampling boost was last granted to a service, keyed by
    /// rule and then by the reporting `ServiceName` — what the configured
    /// cooldown window is measured from.
    boosts: BTreeMap<String, BTreeMap<String, f64>>,
}

impl Default for SamplingStatisticsStore {
    fn default() -> Self {
        Self {
            rules_modified_at: now_epoch_seconds(),
            windows: BTreeMap::new(),
            clients: BTreeMap::new(),
            pools: BTreeMap::new(),
            boosts: BTreeMap::new(),
        }
    }
}

impl SamplingStatisticsStore {
    /// Record one reported `SamplingStatisticsDocument`: store its counters as
    /// the reporting service's figures for the window its `Timestamp` falls in,
    /// and remember the report as this service's claim on the rule's reservoir.
    ///
    /// A document states the results of a reporting window, so a service that
    /// reports the same window again — a client polling for its reservoir
    /// share restates the window it is in — RESTATES its own figures rather
    /// than adding to them; a summary of the window adds up what each service
    /// stated for it.
    ///
    /// The share the reporting service is answered is the one it already HOLDS
    /// ([`Self::held_allocation`]), computed at the end of a cycle it took part
    /// in, so recording a report never hands out a share of its own: the first
    /// report a service makes precedes the first allocation cycle for it.
    pub fn record(
        &mut self,
        rule_name: &str,
        client_id: &str,
        report: ReportedSamplingCounts,
        now: f64,
    ) {
        let windows = self.windows.entry(rule_name.to_string()).or_default();
        let window = windows
            .entry(sampling_window_start(report.timestamp))
            .or_default();
        window.insert(
            client_id.to_string(),
            SamplingStatisticsWindow::of(report),
        );
        while window.len() > MAX_SAMPLING_CLIENTS_PER_RULE {
            // Which service's figures make room is arbitrary — the window holds
            // no order over them — so the first by `ClientID` goes, and the
            // window stays bounded.
            let Some(crowded) = window.keys().next().cloned() else {
                break;
            };
            window.remove(&crowded);
        }
        while windows.len() > MAX_SAMPLING_WINDOWS_PER_RULE {
            let Some(oldest) = windows.keys().next().copied() else {
                break;
            };
            windows.remove(&oldest);
        }

        let clients = self.clients.entry(rule_name.to_string()).or_default();
        let held = clients
            .get(client_id)
            .and_then(|report| report.allocation)
            .filter(|allocation| {
                now - allocation.allocated_at <= SAMPLING_ALLOCATION_CACHE_SECONDS
            });
        clients.insert(
            client_id.to_string(),
            SamplingClientReport {
                reported_at: now,
                request_count: report.request_count,
                allocation: held,
            },
        );
        // A service that has gone quiet for longer than a report stays live no
        // longer shares the pool.
        clients.retain(|_, report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS);
        while clients.len() > MAX_SAMPLING_CLIENTS_PER_RULE {
            let Some(stale) = clients
                .iter()
                .min_by(|left, right| left.1.reported_at.total_cmp(&right.1.reported_at))
                .map(|(client, _)| client.clone())
            else {
                break;
            };
            clients.remove(&stale);
        }
        self.clients.retain(|_, clients| !clients.is_empty());
        self.prune_rules();
    }

    /// The pool the latest allocation cycle divided for `rule_name`, while that
    /// division is still live — the state the target computation answers a
    /// report for that rule NAME out of.
    ///
    /// `None` means no cycle has divided a pool for the name, or the last one
    /// has aged past [`SAMPLING_ALLOCATION_CACHE_SECONDS`]: there is no division
    /// to tell a reporting service its share of. Whether the account still
    /// holds a rule under the name does not enter into it — the division stands
    /// on its own, exactly as the share granted out of it does.
    pub fn divided_pool(&self, rule_name: &str, now: f64) -> Option<SamplingPool> {
        self.pools
            .get(rule_name)
            .copied()
            .filter(|pool| now - pool.divided_at <= SAMPLING_ALLOCATION_CACHE_SECONDS)
    }

    /// What is left UNCLAIMED of a divided pool of `reservoir_size`: the size
    /// of the pool less every share the services still live hold out of it.
    ///
    /// This is what a service that was not in the division is told, because the
    /// pool "applies to all services using the rule collectively" and is
    /// zero-sum: a newcomer may have what the division did not give away, which
    /// is nothing once the pool is fully claimed. Never negative, and never more
    /// than the pool.
    pub fn unclaimed(&self, rule_name: &str, reservoir_size: i32, now: f64) -> i32 {
        if reservoir_size <= 0 {
            return 0;
        }
        let claimed = self.clients.get(rule_name).map_or(0_i64, |clients| {
            clients
                .values()
                .filter(|report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS)
                .filter_map(|report| report.allocation)
                .filter(|allocation| {
                    now - allocation.allocated_at <= SAMPLING_ALLOCATION_CACHE_SECONDS
                })
                .map(|allocation| i64::from(allocation.reservoir_quota))
                .fold(0_i64, i64::saturating_add)
        });
        i32::try_from(i64::from(reservoir_size).saturating_sub(claimed))
            .unwrap_or(0)
            .clamp(0, reservoir_size)
    }

    /// The allocation the service `client_id` still HOLDS for `rule_name`,
    /// without reporting anything new.
    ///
    /// This is the share a cycle already granted it, which stands until it ages
    /// past [`SAMPLING_ALLOCATION_CACHE_SECONDS`] — including when the rule has
    /// left the account in between, because deleting a rule does not reach into
    /// an allocation the service was already told about.
    pub fn held_allocation(
        &self,
        rule_name: &str,
        client_id: &str,
        now: f64,
    ) -> Option<SamplingAllocation> {
        self.clients
            .get(rule_name)?
            .get(client_id)?
            .allocation
            .filter(|allocation| {
                now - allocation.allocated_at <= SAMPLING_ALLOCATION_CACHE_SECONDS
            })
    }

    /// Run the allocation cycle for one service and one rule: compute the share
    /// of the pool every live report entitles it to and store it as the
    /// allocation that service HOLDS, which its next report is answered with.
    pub fn allocate(
        &mut self,
        rule_name: &str,
        client_id: &str,
        reservoir_size: i32,
        fixed_rate: f64,
        now: f64,
    ) {
        // The cycle divides the pool as it finds the rule configured NOW, and
        // records what it divided under the rule's name, so the target
        // computation keeps answering out of that division without re-reading
        // the rule store.
        self.pools.insert(
            rule_name.to_string(),
            SamplingPool {
                fixed_rate,
                reservoir_size,
                divided_at: now,
            },
        );
        // A division that has aged past the span a share of it stays live can
        // no longer be answered out of, so it is dropped rather than kept.
        self.pools
            .retain(|_, pool| now - pool.divided_at <= SAMPLING_ALLOCATION_CACHE_SECONDS);
        let allocation = SamplingAllocation {
            reservoir_quota: self.reservoir_quota(rule_name, client_id, reservoir_size, now),
            allocated_at: now,
        };
        if let Some(report) = self
            .clients
            .get_mut(rule_name)
            .and_then(|clients| clients.get_mut(client_id))
        {
            report.allocation = Some(allocation);
        }
    }

    /// Keep the store to [`MAX_SAMPLING_STATISTIC_RULES`] rules, dropping the
    /// rule whose most recent window is the oldest.
    fn prune_rules(&mut self) {
        while self.windows.len() > MAX_SAMPLING_STATISTIC_RULES {
            let Some(stale) = self
                .windows
                .iter()
                .min_by_key(|(_, windows)| windows.keys().next_back().copied().unwrap_or(i64::MIN))
                .map(|(rule_name, _)| rule_name.clone())
            else {
                break;
            };
            self.windows.remove(&stale);
            self.clients.remove(&stale);
            self.pools.remove(&stale);
            self.boosts.remove(&stale);
        }
    }

    /// The share of `rule_name`'s reservoir the service `client_id` is
    /// allocated, from what every service using the rule reported.
    ///
    /// A rule's `ReservoirSize` is "a fixed number of matching requests to
    /// instrument per second, prior to applying the fixed rate ... not used
    /// directly by services, but applies to all services using the rule
    /// collectively" (`smithy_query(query="shape",shape="SamplingRule")`), so
    /// the pool is SHARED: each reporting service is allocated the fraction of
    /// it that its own reported `RequestCount` is of everything reported for the
    /// rule by services still live, and services that report no requests at all
    /// split it evenly. The share is floored, so the pool is never
    /// over-allocated and a service whose share rounds to nothing instruments at
    /// the rule's fixed rate instead.
    pub fn reservoir_quota(
        &self,
        rule_name: &str,
        client_id: &str,
        reservoir_size: i32,
        now: f64,
    ) -> i32 {
        if reservoir_size <= 0 {
            return 0;
        }
        let Some(clients) = self.clients.get(rule_name) else {
            return 0;
        };
        let live = || {
            clients
                .values()
                .filter(|report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS)
        };
        let Some(mine) = clients
            .get(client_id)
            .filter(|report| now - report.reported_at <= SAMPLING_CLIENT_REPORT_TTL_SECONDS)
        else {
            return 0;
        };
        let reported: i64 = live()
            .map(|report| report.request_count)
            .fold(0_i64, i64::saturating_add);
        let sharing = live().count();
        #[allow(clippy::cast_precision_loss)]
        let share = if reported > 0 {
            mine.request_count as f64 / reported as f64
        } else if sharing > 0 {
            1.0 / sharing as f64
        } else {
            return 0;
        };
        let pool = f64::from(reservoir_size);
        #[allow(clippy::cast_possible_truncation)]
        {
            (pool * share).floor().clamp(0.0, pool) as i32
        }
    }

    /// The boost rate to grant the service `service_name` for `rule_name`, or
    /// `None` when no boost is due.
    ///
    /// A rule carries a boost configuration only when a caller asked for one:
    /// `MaxRate` "defines max temporary sampling rate to apply when a boost is
    /// triggered. Calculated boost rate by X-Ray will be less than or equal to
    /// this max rate", and `CooldownWindowMinutes` "sets the time window in
    /// which only one sampling rate boost can be triggered. After a boost
    /// occurs, no further boosts are allowed until the next window"
    /// (`smithy_query(query="shape",shape="SamplingRateBoost")`). So a boost is
    /// triggered by the ANOMALIES a service reported — at the rate it reported
    /// them, capped by the configured maximum — and only once per cooldown
    /// window. A service that reported no anomaly is not boosted at all.
    pub fn grant_boost(
        &mut self,
        rule_name: &str,
        service_name: &str,
        anomaly_rate: f64,
        configured: &SamplingRateBoost,
        now: f64,
    ) -> Option<f64> {
        if !anomaly_rate.is_finite() || anomaly_rate <= 0.0 || configured.max_rate <= 0.0 {
            return None;
        }
        let cooldown = f64::from(configured.cooldown_window_minutes) * 60.0;
        let granted = self
            .boosts
            .get(rule_name)
            .and_then(|services| services.get(service_name).copied());
        if granted.is_some_and(|at| now - at < cooldown) {
            return None;
        }
        self.boosts
            .entry(rule_name.to_string())
            .or_default()
            .insert(service_name.to_string(), now);
        self.prune_boosts(rule_name, cooldown, now);
        Some(anomaly_rate.min(configured.max_rate))
    }

    /// Keep one rule's boost grants to what they are for: a grant older than the
    /// cooldown window can no longer suppress a boost, so it carries no
    /// information and is dropped, and the rule records at most
    /// [`MAX_SAMPLING_BOOST_SERVICES_PER_RULE`] services, the least recently
    /// granted one going first.
    fn prune_boosts(&mut self, rule_name: &str, cooldown: f64, now: f64) {
        if let Some(services) = self.boosts.get_mut(rule_name) {
            // A grant that has aged past the cooldown window can no longer
            // suppress a boost, so it is dropped rather than kept; a rule with
            // no cooldown window suppresses nothing and so records nothing.
            services.retain(|_, granted| now - *granted < cooldown);
            while services.len() > MAX_SAMPLING_BOOST_SERVICES_PER_RULE {
                let Some(stale) = services
                    .iter()
                    .min_by(|left, right| left.1.total_cmp(right.1))
                    .map(|(service, _)| service.clone())
                else {
                    break;
                };
                services.remove(&stale);
            }
        }
        self.boosts.retain(|_, services| !services.is_empty());
    }

    /// Every reported window the account holds, in rule-name order and then in
    /// window order — what `GetSamplingStatisticSummaries` pages over.
    ///
    /// Each row adds up what every service reporting that window stated for it,
    /// which is what the summary aggregates "across all services".
    ///
    /// Bounded by [`MAX_SAMPLING_STATISTIC_RULES`] *
    /// [`MAX_SAMPLING_WINDOWS_PER_RULE`].
    pub fn entries(&self) -> Vec<SamplingStatisticsEntry> {
        self.windows
            .iter()
            .flat_map(|(rule_name, windows)| {
                windows.iter().map(move |(window_start, reporters)| {
                    let mut counts = SamplingStatisticsWindow::default();
                    for stated in reporters.values() {
                        counts.add(*stated);
                    }
                    SamplingStatisticsEntry {
                        rule_name: rule_name.clone(),
                        window_start: *window_start,
                        counts,
                    }
                })
            })
            .collect()
    }
}

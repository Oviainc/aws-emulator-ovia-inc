// SPDX-License-Identifier: CC0-1.0

//! The time series a workspace HOLDS: what its Prometheus data plane writes and what an
//! instant query reads back.
//!
//! A workspace's series hang off the workspace record itself
//! ([`crate::state::Workspace::metrics`]), because they are the workspace's own data:
//! deleting the workspace releases them, and no second map can disagree about which
//! workspace a series was written to. `crate::state` re-exports everything here, so the
//! data plane names `crate::state::…` for a series exactly as a handler does for a
//! workspace.
//!
//! ## What the model and the configuration decide
//!
//! Ingestion is judged against the workspace's OWN stored configuration
//! ([`crate::state::WorkspaceConfiguration`]), never against a constant written here:
//!
//! * `outOfOrderTimeWindowInSeconds` — how far behind the newest sample of a series a
//!   further sample may still be written. The member defaults to zero
//!   ([`crate::state::DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS`]), so a workspace
//!   nobody configured takes samples in order only.
//! * `retentionPeriodInDays` — how long a sample is retained, so a query reports only the
//!   samples still inside the window and ingestion keeps nothing older.
//! * `limitsPerLabelSet[].maxSeries` — "The maximum number of active series the label set
//!   can hold", enforced against the series the workspace already holds for that label set.
//!
//! Two further bounds are the EMULATOR's own, stated as such: [`MAX_ACTIVE_SERIES`] and
//! [`MAX_SAMPLES_PER_SERIES`]. They are what stops one workspace's series map growing
//! without end.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::state::{LabelSetLimit, WorkspaceConfiguration};

/// The label that carries a series' metric name, as the Prometheus data model defines it.
pub const METRIC_NAME_LABEL: &str = "__name__";

/// The path space a workspace is addressed under, which is also the one its Prometheus
/// API is served beneath: `/workspaces/{workspaceId}/…`, as every workspace-addressed
/// operation of this service is bound.
pub const WORKSPACES_PATH_PREFIX: &str = "/workspaces/";

/// The version segments the Prometheus HTTP API is served under, which the documented
/// `prometheusEndpoint` ends with.
pub const PROMETHEUS_API_VERSION: &str = "api/v1";

/// The path a workspace's Prometheus API is served at — the path half of
/// [`crate::state::prometheus_endpoint`], and the prefix the data plane matches a request
/// against, so the announced URL and the served route come from ONE rule.
pub fn workspace_prometheus_path(workspace_id: &str) -> String {
    format!("{WORKSPACES_PATH_PREFIX}{workspace_id}/{PROMETHEUS_API_VERSION}/")
}

/// How far back from the evaluation instant an instant query looks for the latest sample
/// of a series.
///
/// This is Prometheus's own query lookback delta, whose published default is 5 minutes
/// (`--query.lookback-delta`): a series whose last sample is older than that is not part
/// of the instant vector at all, which is why a query reports an empty result rather than
/// a stale sample.
pub const QUERY_LOOKBACK: Duration = Duration::from_secs(5 * 60);

/// How many distinct series ONE workspace holds before a write is refused rather than
/// adding another.
///
/// The EMULATOR's own bound on the store, not a claim about any account's quota: Amazon
/// Managed Service for Prometheus publishes an adjustable active-series quota per
/// workspace and this emulator models neither Service Quotas nor an account's
/// adjustments. It is what keeps one workspace's series map bounded; the map is owned by
/// the workspace record, so deleting the workspace releases every series in it. It also
/// bounds the largest instant vector one query can answer, since a query reports at most
/// one sample per series held.
pub const MAX_ACTIVE_SERIES: usize = 10_000;

/// How many samples ONE series retains: past it the oldest sample is dropped as the next
/// one is written, so a series that is written to for ever stays bounded.
///
/// The EMULATOR's own bound, for the same reason [`MAX_ACTIVE_SERIES`] is. A sample is
/// also dropped once it falls outside the workspace's configured retention period, which
/// is the service's own rule rather than this one.
pub const MAX_SAMPLES_PER_SERIES: usize = 1_000;

/// One sample of one series: a value at an instant, as the remote-write wire format
/// carries it (a `double` value and a millisecond Unix timestamp).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Milliseconds since the Unix epoch — the unit the Prometheus data model and the
    /// remote-write format both state for a sample's timestamp.
    pub timestamp_ms: i64,
    pub value: f64,
}

/// One series: its label set and the samples written to it, oldest first.
#[derive(Debug, Clone, Default)]
pub struct TimeSeries {
    /// `labelName -> labelValue`, including [`METRIC_NAME_LABEL`]. Sorted, so a query
    /// reports one stable order and two writes of the same label set land on one series
    /// whatever order the request listed the labels in.
    pub labels: BTreeMap<String, String>,
    /// Ordered by timestamp, oldest first: a write inserts in order, and the latest
    /// sample is therefore the last one.
    pub samples: Vec<Sample>,
}

impl TimeSeries {
    /// The timestamp of the newest sample this series holds, if it holds any.
    pub fn latest_timestamp_ms(&self) -> Option<i64> {
        self.samples.last().map(|sample| sample.timestamp_ms)
    }

    /// The series' metric name — the value of [`METRIC_NAME_LABEL`].
    pub fn metric_name(&self) -> Option<&str> {
        self.labels.get(METRIC_NAME_LABEL).map(String::as_str)
    }
}

/// Every series one workspace holds, keyed by the canonical rendering of its label set
/// ([`series_key`]), so one label set is one series however its labels were ordered.
#[derive(Debug, Clone, Default)]
pub struct IngestedMetrics {
    series: BTreeMap<String, TimeSeries>,
}

/// Why a remote-write request is refused, and nothing of it is written.
///
/// Each arm carries the status the Prometheus remote-write specification gives it: a
/// request the sender must fix is `400`, and a limit the receiver enforces is `429`, which
/// a sender is expected to back off and retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteRefusal {
    /// A series carries no `__name__`, or one that is not a metric name.
    InvalidMetricName,
    /// A label name is not one the Prometheus data model allows.
    InvalidLabelName,
    /// A sample is older than the series' newest sample by more than the workspace's
    /// configured out-of-order window allows.
    OutOfOrder,
    /// The write would take the workspace past [`MAX_ACTIVE_SERIES`].
    SeriesLimit,
    /// The write would take a configured label set past its `maxSeries`.
    LabelSetLimit {
        /// The limit that would be exceeded.
        max_series: i64,
    },
}

impl WriteRefusal {
    /// The HTTP status this refusal is answered with.
    pub fn status(&self) -> u16 {
        match self {
            Self::InvalidMetricName | Self::InvalidLabelName | Self::OutOfOrder => 400,
            Self::SeriesLimit | Self::LabelSetLimit { .. } => 429,
        }
    }

    /// What the refusal says. The three 400s are worded as Prometheus words the same
    /// storage and parse failures, and carry nothing from the request; the two limits name
    /// the bound that was reached.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidMetricName => "invalid metric name".to_string(),
            Self::InvalidLabelName => "invalid label name".to_string(),
            Self::OutOfOrder => "out of order sample".to_string(),
            Self::SeriesLimit => {
                format!("this emulator holds at most {MAX_ACTIVE_SERIES} active series per workspace")
            }
            Self::LabelSetLimit { max_series } => format!(
                "the active series limit of {max_series} for this label set has been reached"
            ),
        }
    }
}

impl IngestedMetrics {
    /// Write every sample of `written` to this workspace, or refuse the whole request.
    ///
    /// ATOMIC: every series and every sample is judged first, against the workspace's own
    /// `configuration` and the two bounds of this module, and only then is anything
    /// stored — so a refused write leaves the workspace holding exactly what it held.
    /// That is also what the Prometheus remote-write specification asks of a receiver:
    /// a `400` says the sender must fix the request, not that part of it landed.
    ///
    /// `now` is the instant the request is served at, and decides which stored samples
    /// have fallen outside the configured retention period: they are dropped here, which
    /// materialises an expiry the clock already owed rather than reporting it away.
    pub fn accept(
        &mut self,
        written: &[TimeSeries],
        configuration: &WorkspaceConfiguration,
        now: SystemTime,
    ) -> Result<usize, WriteRefusal> {
        let now_ms = unix_millis(now);
        let window_ms = i64::from(configuration.out_of_order_time_window_in_seconds) * 1_000;
        // Every candidate series, keyed as it will be stored, so the label rules and both
        // limits are judged on the whole request before a byte of it is kept.
        let mut candidates: Vec<(String, &TimeSeries)> = Vec::with_capacity(written.len());
        for series in written {
            check_label_set(&series.labels)?;
            let key = series_key(&series.labels);
            let newest = self
                .series
                .get(&key)
                .and_then(TimeSeries::latest_timestamp_ms);
            for sample in &series.samples {
                if let Some(newest) = newest {
                    // Saturating: a sample's timestamp is a caller-supplied `int64`, so
                    // the comparison must hold for every value the wire format admits.
                    if sample.timestamp_ms.saturating_add(window_ms) < newest {
                        return Err(WriteRefusal::OutOfOrder);
                    }
                }
            }
            candidates.push((key, series));
        }
        self.check_limits(&candidates, configuration)?;

        let retention = retention(configuration);
        let mut stored = 0usize;
        for (key, series) in candidates {
            let held = self.series.entry(key).or_insert_with(|| TimeSeries {
                labels: series.labels.clone(),
                samples: Vec::new(),
            });
            for sample in &series.samples {
                held.write(*sample);
                stored += 1;
            }
            held.retain_from(now_ms.saturating_sub(retention));
        }
        // A series whose every sample has expired is no series: it matches nothing and
        // counts against no limit, so it is dropped rather than left behind empty.
        self.series.retain(|_, series| !series.samples.is_empty());
        Ok(stored)
    }

    /// Every series' newest sample inside the lookback window that is also still inside
    /// the configured retention period — the SNAPSHOT a query is evaluated against.
    ///
    /// Deliberately matcher-free: the caller's own expression is applied to this snapshot
    /// AFTER the store's lock is released (see
    /// `crate::service`'s Prometheus plane), so no work whose size the caller chose runs
    /// while the store is held. The work here is one binary search per series held, and
    /// the result carries at most one entry per series, so both are bounded by
    /// [`MAX_ACTIVE_SERIES`] alone.
    ///
    /// A pure read: a sample outside retention is not reported, and the store is left as
    /// it stands (ingestion is what drops it).
    pub fn instant_snapshot(
        &self,
        at: SystemTime,
        configuration: &WorkspaceConfiguration,
    ) -> Vec<(BTreeMap<String, String>, Sample)> {
        let at_ms = unix_millis(at);
        let oldest_reported = at_ms
            .saturating_sub(QUERY_LOOKBACK.as_millis().try_into().unwrap_or(i64::MAX))
            .max(at_ms.saturating_sub(retention(configuration)));
        self.series
            .values()
            .filter_map(|series| {
                // The samples are ordered by timestamp, so the newest one at or before
                // the evaluation instant is found by bisection rather than by walking
                // the series.
                let at_or_before = series
                    .samples
                    .partition_point(|sample| sample.timestamp_ms <= at_ms);
                let sample = *series
                    .samples
                    .get(at_or_before.checked_sub(1)?)
                    .filter(|sample| sample.timestamp_ms >= oldest_reported)?;
                Some((series.labels.clone(), sample))
            })
            .collect()
    }

    /// How many series this workspace holds.
    pub fn series_count(&self) -> usize {
        self.series.len()
    }

    /// The series this workspace holds, for a reader that walks them all.
    pub fn series(&self) -> impl Iterator<Item = &TimeSeries> {
        self.series.values()
    }

    /// Refuse a write that would take the workspace past [`MAX_ACTIVE_SERIES`], or a
    /// configured label set past its `maxSeries`. Counted over the series that would
    /// exist AFTER the write, so a request that only appends to series the workspace
    /// already holds is never refused for a limit.
    fn check_limits(
        &self,
        candidates: &[(String, &TimeSeries)],
        configuration: &WorkspaceConfiguration,
    ) -> Result<(), WriteRefusal> {
        let fresh: Vec<&(String, &TimeSeries)> = candidates
            .iter()
            .filter(|(key, _)| !self.series.contains_key(key))
            .collect();
        if self.series.len() + distinct(&fresh) > MAX_ACTIVE_SERIES {
            return Err(WriteRefusal::SeriesLimit);
        }
        for limit in &configuration.limits_per_label_set {
            let Some(max_series) = limit.max_series else {
                continue;
            };
            let held = self
                .series
                .values()
                .filter(|series| carries(&series.labels, limit))
                .count();
            let added = fresh
                .iter()
                .filter(|(_, series)| carries(&series.labels, limit))
                .map(|(key, _)| key)
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            if i64::try_from(held + added).unwrap_or(i64::MAX) > max_series {
                return Err(WriteRefusal::LabelSetLimit { max_series });
            }
        }
        Ok(())
    }
}

impl TimeSeries {
    /// Write one sample, keeping the samples ordered by timestamp.
    ///
    /// A sample at a timestamp the series already holds REPLACES that sample rather than
    /// sitting beside it: one series holds one value per instant, so the latest write for
    /// an instant is what a query reports. Past [`MAX_SAMPLES_PER_SERIES`] the oldest
    /// sample is dropped.
    fn write(&mut self, sample: Sample) {
        match self
            .samples
            .binary_search_by_key(&sample.timestamp_ms, |held| held.timestamp_ms)
        {
            Ok(at) => self.samples[at] = sample,
            Err(at) => self.samples.insert(at, sample),
        }
        if self.samples.len() > MAX_SAMPLES_PER_SERIES {
            let over = self.samples.len() - MAX_SAMPLES_PER_SERIES;
            self.samples.drain(..over);
        }
    }

    /// Drop every sample older than `oldest_ms` — the retention the workspace's
    /// configuration states.
    fn retain_from(&mut self, oldest_ms: i64) {
        self.samples
            .retain(|sample| sample.timestamp_ms >= oldest_ms);
    }
}

/// The retention period the configuration states, in milliseconds.
fn retention(configuration: &WorkspaceConfiguration) -> i64 {
    i64::from(configuration.retention_period_in_days) * 24 * 60 * 60 * 1_000
}

/// How many DISTINCT series the fresh candidates name: one request may carry the same
/// label set twice, which is one series.
fn distinct(fresh: &[&(String, &TimeSeries)]) -> usize {
    fresh
        .iter()
        .map(|(key, _)| key)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

/// Whether `labels` carries every label pair of a configured label set, which is what
/// makes a series count against that set's `maxSeries`.
fn carries(labels: &BTreeMap<String, String>, limit: &LabelSetLimit) -> bool {
    limit
        .label_set
        .iter()
        .all(|(name, value)| labels.get(name) == Some(value))
}

/// Milliseconds since the Unix epoch for `time`, which is the unit a sample's timestamp
/// is carried in. An instant before the epoch is negative, as the wire format allows.
pub fn unix_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// The instant a millisecond Unix timestamp names, when it is one this clock can state:
/// `None` for a value so far from the epoch that no `SystemTime` holds it, which a
/// caller-supplied timestamp may well be.
pub fn instant_of(timestamp_ms: i64) -> Option<SystemTime> {
    let magnitude = Duration::from_millis(timestamp_ms.unsigned_abs());
    if timestamp_ms < 0 {
        UNIX_EPOCH.checked_sub(magnitude)
    } else {
        UNIX_EPOCH.checked_add(magnitude)
    }
}

/// The canonical rendering of a label set, which is the key one series is stored under:
/// `name{label="value",…}` with the labels in name order, as Prometheus renders a series.
/// A value's `\` and `"` are escaped, so two different label sets cannot render alike.
pub fn series_key(labels: &BTreeMap<String, String>) -> String {
    let name = labels
        .get(METRIC_NAME_LABEL)
        .map(String::as_str)
        .unwrap_or_default();
    let inner: Vec<String> = labels
        .iter()
        .filter(|(label, _)| label.as_str() != METRIC_NAME_LABEL)
        .map(|(label, value)| {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{label}=\"{escaped}\"")
        })
        .collect();
    format!("{name}{{{}}}", inner.join(","))
}

/// Check a written series' labels against the Prometheus data model: it must carry a
/// `__name__` that is a metric name, and every label name must be a label name.
fn check_label_set(labels: &BTreeMap<String, String>) -> Result<(), WriteRefusal> {
    match labels.get(METRIC_NAME_LABEL) {
        Some(name) if is_metric_name(name) => {}
        _ => return Err(WriteRefusal::InvalidMetricName),
    }
    if labels
        .keys()
        .any(|label| label.as_str() != METRIC_NAME_LABEL && !super::is_label_name(label))
    {
        return Err(WriteRefusal::InvalidLabelName);
    }
    Ok(())
}

/// Whether `name` is a metric name, which the Prometheus data model writes as
/// `[a-zA-Z_:][a-zA-Z0-9_:]*`.
///
/// A LABEL name is the same expression without the colons, and the service already
/// decides that one for the metric-label map keys it takes
/// (`crate::state::PROMETHEUS_METRIC_LABEL`), so this module reads that rule from there
/// rather than stating a second copy of it.
pub fn is_metric_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == ':')
        && characters.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

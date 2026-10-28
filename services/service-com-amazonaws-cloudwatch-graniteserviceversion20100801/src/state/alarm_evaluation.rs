// SPDX-License-Identifier: CC0-1.0

//! A METRIC alarm's `StateValue`, COMPUTED from the datapoints `PutMetricData` stored.
//!
//! A metric alarm's state is not a value a caller writes: it is the alarm's own comparison
//! ("The arithmetic operation to use when comparing the specified statistic and threshold",
//! `MetricAlarm$ComparisonOperator`) applied to the statistic of the metric over its
//! evaluation periods, so it is derived here from the series the metric store holds — the
//! same way a composite alarm's state is derived from its `AlarmRule`
//! ([`super::ComAmazonawsCloudwatchGraniteserviceversion20100801State::evaluate_composite_alarms`]).
//!
//! The rule, as `PutMetricAlarm` and the CloudWatch alarm documentation state it:
//!
//! * the metric is aggregated into periods of `Period` seconds each, ANCHORED at multiples of
//!   the period — "CloudWatch aggregates all data points with time stamps that fall within
//!   each one-hour period" — so which period a datapoint belongs to is decided by its own time
//!   stamp and not by the instant the evaluation runs, the newest period being the one that
//!   instant falls in;
//! * each window yields one datapoint — the alarm's `Statistic` or `ExtendedStatistic` over
//!   the datapoints in it — or no datapoint at all when the window holds none;
//! * the evaluation looks over a RANGE of windows wider than `EvaluationPeriods`, and
//!   evaluates the `EvaluationPeriods` most recent datapoints it really found there:
//!   CloudWatch "attempts to retrieve a higher number of data points than the number
//!   specified as Evaluation Periods" and "all real data points in the evaluation range are
//!   included in the evaluation", filling what is still missing with the missing-data
//!   treatment "only as few times as possible";
//! * a datapoint BREACHES when the comparison between it and `Threshold` holds;
//! * a slot no real datapoint filled counts as whatever `TreatMissingData` says ("Valid
//!   Values: breaching | notBreaching | ignore | missing", default `missing`) — `breaching`
//!   and `notBreaching` substitute a datapoint for it, `missing` leaves it out of the
//!   evaluation altogether, and `ignore` leaves it out too but, when NO datapoint at all
//!   could be evaluated, keeps the alarm in the state it has ("the current alarm state is
//!   maintained");
//! * the predicate an "M out of N" alarm is decided by has TWO directions, each with a minimum
//!   of its own built from the alarm's configuration alone: the alarm is in `ALARM` once at
//!   least `DatapointsToAlarm` of the evaluated datapoints breach ("The number of data points
//!   that must be breaching to trigger the alarm" — the M, defaulting to `EvaluationPeriods`,
//!   which is the N), and in `OK` once at least `EvaluationPeriods - DatapointsToAlarm + 1` of
//!   them do not — the count that makes M breaching datapoints unreachable inside the same
//!   window, which is the minimum CloudWatch's own explanation of an `OK` state reports;
//! * a window neither direction's minimum is reached over is one the evaluation cannot decide
//!   — only a treatment that substitutes NO datapoint (`missing`, `ignore`) can leave it that
//!   short of datapoints — so the alarm is in `INSUFFICIENT_DATA`, or keeps the state it has
//!   where the treatment is `ignore` ("the current alarm state is maintained");
//! * the `StateReason` ("An explanation for the alarm state, in text format.") explains how
//!   the state was decided: it NAMES the datapoints the evaluation graded — the value the
//!   statistic took over each window and the instant that window ends — beside the counts it
//!   made over them, and a window that held NO datapoint of the metric is reported as the hole
//!   it was, naming the missing-data arm that was applied to it, rather than being counted as a
//!   datapoint the evaluation graded — a client can tell an alarm that saw clean data from one
//!   that saw none. The count it reports is of the datapoints that satisfied the DIRECTION the
//!   state stands for, and its closing clause names that direction and the minimum it needed,
//!   so both integers of the alarm's configuration are recoverable from the sentence.
//!
//! The evaluation is a pure function of the stored datapoints and the clock, and it writes
//! only when the state it computes differs from the state the alarm already reports — so
//! whichever call materialises it, every later caller sees the same thing. The first
//! evaluation of an alarm becomes due [`EVALUATION_DELAY_SECS`] after the write that
//! configured it ("When this operation creates an alarm, the alarm state is immediately set
//! to INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately." —
//! `PutMetricAlarm`), and a state `SetAlarmState` forced stands until that alarm's next
//! evaluation, one of its own periods away ([`forced_state_span_secs`]) or sooner if a
//! datapoint of the series it watches arrives before then.
//!
//! What is NOT evaluated here keeps the state it has, rather than being answered with a
//! state nothing computed: an alarm with `EvaluationCriteria` (a PromQL query), one whose
//! threshold is an anomaly-detection band (`ThresholdMetricId`, or one of the
//! `…LowerOrGreaterThanUpper…` comparisons), one whose configuration names no stored metric at
//! all, and one over a `Metrics` query array this engine cannot resolve to a single time
//! series.
//!
//! An alarm configured with a `Metrics` query array IS evaluated: "Use the `Metrics` parameter
//! to create an alarm based on a metric math expression" (`PutMetricAlarmInput$Metrics`), and
//! the series it grades is the value of the query that array returns — the EXPRESSION's output,
//! computed over the same stored datapoints `GetMetricData` answers the same array from
//! ([`super::metric_query`]) — against the same threshold, the same m-of-n predicate and the
//! same explanation as an alarm over one metric.

use ::serde_json::{Map, Value};

use super::alarm_actions::AlarmNotification;
use super::metric_math::MathValue;
use super::{
    dimension_set, to_timestamp, weighted_percentile, AlarmStateSnapshot, BasicStatistics,
    ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, MetricKey, StoredDatum,
    ALARM_STATE_ALARM, ALARM_STATE_INSUFFICIENT_DATA, ALARM_STATE_OK, ALARM_TYPE_METRIC,
    HISTORY_DATA_VERSION, MAX_STATE_REASON_DATA_LEN,
};
use crate::ops::types::{AlarmHistoryItem, MetricAlarm, MetricDataQuery, STATISTIC_VALUES};

/// How long after the write that configured an alarm its FIRST evaluation is owed, and how
/// long after an evaluation the NEXT one is owed.
///
/// `PutMetricAlarm` creates an alarm in `INSUFFICIENT_DATA` and documents that "the alarm is
/// then evaluated and its state is set appropriately", so the evaluated state is not due at
/// the instant of the write: a caller reading straight after the write still sees the state
/// the write left behind. CloudWatch publishes no duration for that gap, so this span is the
/// emulator's own choice inside the framework's 250-750 ms window for a transition the clock
/// owes, and it is never slept on — the due instant is stored and the evaluation is
/// materialised by the first call at or after it. It is the same span, for the same reason,
/// as [`super::COMPOSITE_EVALUATION_DELAY_SECS`].
///
/// An evaluation RE-ARMS the alarm by the same span, so an alarm just evaluated is not folded
/// again by every following request: a state its datapoints already imply becomes visible to
/// the next caller a span later — the same window a clock-owed transition takes — and the fold
/// one alarm costs is charged at most once per span rather than once per request.
pub const EVALUATION_DELAY_SECS: f64 = 0.5;

/// The most period windows ONE alarm's LOOK-BACK covers.
///
/// A capacity bound, not an AWS quota: `EvaluationPeriods` is modelled with a minimum only,
/// so without this one alarm could ask one call to fold arbitrarily many windows. CloudWatch's
/// own published ceiling on an alarm's look-back is that `EvaluationPeriods` x `Period` may
/// not exceed one day, which at the shortest regular period (60 s) is exactly this many
/// windows. An alarm asking for more is evaluated over its most recent windows rather than
/// refused, because the windows past the bound are older than any datapoint CloudWatch would
/// evaluate.
const MAX_EVALUATED_PERIODS: usize = 1_440;

/// How much wider than the look-back the EVALUATION RANGE is: the span of periods an
/// evaluation searches for the datapoints it evaluates.
///
/// CloudWatch "attempts to retrieve a higher number of data points than the number specified
/// as Evaluation Periods" and calls that span the evaluation range, so a datapoint just older
/// than the look-back is still evaluated rather than counted as missing — which is why a
/// series published a little slower than the alarm's period still decides its state. How much
/// wider the range is depends on the period and the resolution and is not published as a
/// number, so this factor is the emulator's own bounded choice: wide enough that a sparse
/// series is evaluated on what it really published, and a fixed multiple of the look-back so
/// the periods one evaluation folds stay bounded by it.
///
/// The periods are anchored at multiples of the period, so the newest of them is the PARTIAL
/// one the evaluation instant falls in: the range reaches one whole period further back than a
/// span measured from that instant would, which is why this factor leaves room for one period
/// more than the look-back plus the sparse-series margin.
const EVALUATION_RANGE_FACTOR: usize = 3;

/// The most action notifications the store holds waiting for delivery.
///
/// A capacity bound: the notifications live in this process's memory, so their number has to
/// be finite. Reaching it drops the OLDEST — a notification nothing has delivered yet is the
/// stale one — and says so in the log, which is the honest answer for a delivery that did
/// not happen.
pub const MAX_PENDING_NOTIFICATIONS: usize = 256;

/// The `TreatMissingData` value that counts a missing datapoint as breaching.
const MISSING_BREACHING: &str = "breaching";

/// The `TreatMissingData` value that counts a missing datapoint as not breaching.
const MISSING_NOT_BREACHING: &str = "notBreaching";

/// The `TreatMissingData` value under which an evaluation that cannot decide the alarm leaves
/// it in the state it has: "the current alarm state is maintained". A window the datapoints
/// present DO decide still decides it.
const MISSING_IGNORE: &str = "ignore";

/// The `TreatMissingData` default: "If `TreatMissingData` is omitted, the default behavior of
/// `missing` is used" (`PutMetricAlarmInput$TreatMissingData`) — a missing datapoint is
/// neither breaching nor not breaching, so the alarm is evaluated on the datapoints it has and
/// reports insufficient data where they are too few to reach either direction's minimum.
const MISSING_MISSING: &str = "missing";

/// The most alarms ONE request may evaluate.
///
/// A per-request bound, not an AWS quota: the store may hold [`super::MAX_ALARMS`] alarms and
/// an evaluation pass visits the ones that are due, so without this one small request's work
/// would grow with everything the account has ever created. It sits far above what a settled
/// store asks of one request — an alarm is re-armed for [`EVALUATION_DELAY_SECS`] once
/// evaluated, so a request pays only for the alarms that came due in that span — and an
/// evaluation this bound stops is not answered wrongly, only settled later: it stays DUE and
/// the next call carries it, because the evaluation is a pure function of the stored
/// datapoints and the clock.
const MAX_ALARMS_EVALUATED_PER_REQUEST: usize = 256;

/// The most stored datapoints the folds of ONE request may visit.
///
/// The other factor of the same product: each alarm's evaluation folds the series it watches,
/// which holds up to [`super::MAX_DATAPOINTS_PER_SERIES`] datapoints, so bounding the number
/// of alarms alone would leave `alarms x datapoints` unbounded. The charge is made before a
/// series is folded and saturates, so one request folds at most this many datapoints plus the
/// one series that exhausted the budget.
const MAX_DATAPOINTS_FOLDED_PER_REQUEST: usize = 200_000;

/// How long ONE request may spend materialising the evaluations the clock has made due.
///
/// The wall-clock half of the budget, so the work is bounded in TIME as well as in counts,
/// however cheap or expensive each charged unit turns out to be. Short enough to stay a small
/// part of any request's own latency; what it does not reach stays due for the next call. It
/// bounds what a pass does AFTER its first evaluation, never whether that first one runs
/// ([`EvaluationBudget::spent`]), because a span this short can be gone before a pass has
/// begun — the wait for the store's lock, and the scheduler, are not under the pass's control.
const EVALUATION_DEADLINE: std::time::Duration = std::time::Duration::from_millis(50);

/// The work ONE request may spend materialising the alarm evaluations the clock has already
/// made due: alarms visited, datapoints folded, and wall clock.
///
/// ONE budget covers a whole evaluation STAGE — the metric-alarm pass and the composite-alarm
/// pass that follows it — so the two cannot each spend a full one, and every charge is made
/// BEFORE the work it pays for. A handler that must evaluate before its own write charges one
/// for that stage and the settle that follows the write charges one more, so a request's
/// evaluation work is bounded by a small fixed number of these budgets rather than by what
/// the store holds. An evaluation a budget stops is left due for the next call rather than
/// answered from nothing — which is only true while every pass the budget bounds settles one
/// of them, so each pass is owed its first evaluation before the bounds answer at all
/// ([`EvaluationBudget::start_pass`], [`EvaluationBudget::spent`]).
#[derive(Debug)]
pub struct EvaluationBudget {
    /// How many more alarms this request may evaluate.
    alarms: usize,
    /// How many more stored datapoints this request's folds may visit.
    datapoints: usize,
    /// When this request stops evaluating, whatever is left of the counts above.
    deadline: std::time::Instant,
    /// Whether the pass this budget is bounding has settled an evaluation yet.
    settled_in_pass: bool,
}

impl Default for EvaluationBudget {
    fn default() -> Self {
        Self::for_one_request()
    }
}

impl EvaluationBudget {
    /// The budget ONE request starts with.
    #[must_use]
    pub fn for_one_request() -> Self {
        Self {
            alarms: MAX_ALARMS_EVALUATED_PER_REQUEST,
            datapoints: MAX_DATAPOINTS_FOLDED_PER_REQUEST,
            deadline: std::time::Instant::now() + EVALUATION_DEADLINE,
            settled_in_pass: false,
        }
    }

    /// Begin one PASS over the alarms this budget bounds. The pass is owed its first
    /// evaluation: until that one is settled, this budget is not spent ([`Self::spent`]).
    pub(super) fn start_pass(&mut self) {
        self.settled_in_pass = false;
    }

    /// Whether this request has spent its budget and must stop evaluating.
    ///
    /// A bound on the work one request may do DEFERS the evaluations it stops: they stay due,
    /// and the next call carries them. That holds only while every pass makes progress — a
    /// pass stopped before its first evaluation defers nothing, it starves it, and a pass can
    /// be stopped that early by a clock it does not control, since the time the wait for the
    /// store's lock and the scheduler took before the pass began is time off this budget. Were
    /// one pass after another stopped that way, an evaluation the clock has made due would
    /// never be materialised at all and the alarm would keep the state its last write left
    /// behind for as long as the load lasted. So the bounds answer from a pass's SECOND
    /// evaluation onwards, and one request pays for at most one evaluation per pass beyond
    /// them — each still bounded on its own by what one alarm's fold may visit.
    #[must_use]
    pub fn spent(&self) -> bool {
        self.settled_in_pass
            && (self.alarms == 0
                || self.datapoints == 0
                || std::time::Instant::now() >= self.deadline)
    }

    /// Charge one alarm's evaluation.
    pub(super) fn charge_alarm(&mut self) {
        self.alarms = self.alarms.saturating_sub(1);
    }

    /// Record that this pass has settled one alarm's evaluation, whatever that evaluation
    /// computed — from here on the pass is bounded again. Recorded AFTER the evaluation rather
    /// than before it, so the one evaluation a pass is owed is a whole one.
    pub(super) fn settled_one(&mut self) {
        self.settled_in_pass = true;
    }

    /// Charge the datapoints one fold is about to visit. Saturating: the fold that exhausts
    /// the budget still runs — it is bounded on its own by
    /// [`super::MAX_DATAPOINTS_PER_SERIES`] — and no further alarm is evaluated by this
    /// request.
    fn charge_datapoints(&mut self, datapoints: usize) {
        self.datapoints = self.datapoints.saturating_sub(datapoints);
    }
}

/// How long after a force the SCHEDULED evaluation that takes the state back is owed, for an
/// alarm with NO datapoint arriving to trigger one.
///
/// This span is the emulator's OWN choice, and it is deliberately outside the 250-750 ms window
/// a transition this store's clock usually owes. "Metric alarms returns to their actual state
/// quickly, often within seconds. Because the metric alarm state change happens quickly, it is
/// typically only visible in the alarm's History tab […] or through DescribeAlarmHistory"
/// (`SetAlarmState`) is the only thing CloudWatch states about the latency, and the recorded
/// behaviour of a force on an alarm over a metric nothing writes pins it from both sides: a
/// caller's own reads right after the call still report the state it forced, which a sub-second
/// span could not show at all, and the state is nevertheless taken back with no further call.
/// So the span has to be SECONDS.
///
/// Ten of them: the shortest period CloudWatch evaluates an alarm on at all ("Valid values are
/// 10, 20, 30, and any multiple of 60", `PutMetricAlarmInput$Period`) — so it is the alarm's own
/// period for a high-resolution alarm, and for a longer-period alarm it is that alarm's
/// scheduled pass brought FORWARD, which keeps the reclaim observable inside one client's
/// session rather than minutes after it. The COMPRESSION is the choice; nothing else about the
/// evaluation is compressed, since what the pass computes stays a pure function of the stored
/// datapoints and the clock.
const SCHEDULED_EVALUATION_SECS: f64 = 10.0;

/// How one evaluated window's datapoint stands against the alarm's threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Datapoint {
    /// The comparison between the datapoint and the threshold held.
    Breaching,
    /// The window had a datapoint and the comparison did not hold.
    NotBreaching,
    /// The window held no datapoint of this metric.
    Missing,
}

/// The series ONE metric alarm watches: namespace, metric name and normalised dimension set,
/// the identity its evaluation reads and the one a datapoint is stored under.
///
/// The one place an alarm's configuration is turned into a series identity, so the evaluation,
/// the forced-state span and [`watched_series_set`] can never resolve it three ways.
pub(super) fn watched_series(alarm: &MetricAlarm) -> MetricKey {
    MetricKey {
        namespace: alarm.namespace.clone().unwrap_or_default(),
        metric_name: alarm.metric_name.clone().unwrap_or_default(),
        dimensions: dimension_set(alarm.dimensions.as_deref()),
    }
}

/// EVERY series one alarm's evaluation reads, each beside the `Unit` it reads that series
/// under.
///
/// An alarm over one metric reads the series its own members name, under its own `Unit`; an
/// alarm configured with a query array reads the series each of that array's `MetricStat`
/// queries names, each under that query's own `Unit`. It is what [`State::make_alarms_due`]
/// matches an arriving datapoint against, resolved from the same members the evaluation itself
/// reads the store by.
pub(super) fn watched_series_set(alarm: &MetricAlarm) -> Vec<(MetricKey, Option<String>)> {
    let Some(queries) = alarm.metrics.as_deref() else {
        return vec![(watched_series(alarm), alarm.unit.clone())];
    };
    queries
        .iter()
        .filter_map(|query| query.metric_stat.as_ref())
        .filter_map(|stat| {
            super::metric_query::query_series_key(stat).map(|key| (key, stat.unit.clone()))
        })
        .collect()
}

/// The width of the windows an alarm's query ARRAY is graded in.
///
/// "The granularity, in seconds, of the returned data points" (`MetricDataQuery$Period`) when
/// the returning query names one, and the `MetricStat$Period` when that query IS a metric —
/// the granularity the shared resolution folds that query at
/// ([`super::metric_query::query_period`]). Otherwise the period of the metrics the expression
/// reads — the widest of them, the granularity every one of those series has a value at —
/// because the period of an expression is the one of the metrics it is computed from.
fn graded_period(queries: &[MetricDataQuery], graded: &MetricDataQuery) -> f64 {
    if graded.period.is_some() || graded.metric_stat.is_some() {
        return super::metric_query::query_period(graded);
    }
    let widest = queries
        .iter()
        .filter(|query| query.period.is_some() || query.metric_stat.is_some())
        .map(super::metric_query::query_period)
        .fold(0.0_f64, f64::max);
    if widest >= 1.0 {
        widest
    } else {
        // An array that reads no metric at all computes its series on the default expression
        // grid, which is the granularity it is then graded in.
        super::metric_query::DEFAULT_EXPRESSION_PERIOD
    }
}

/// How many of the alarm's periods ONE evaluation grades — its `EvaluationPeriods`, within the
/// capacity bound — or `None` for an alarm that names none.
fn evaluated_periods(alarm: &MetricAlarm) -> Option<usize> {
    usize::try_from(alarm.evaluation_periods?)
        .ok()
        .filter(|periods| *periods >= 1)
        .map(|periods| periods.min(MAX_EVALUATED_PERIODS))
}

/// How many period windows the EVALUATION RANGE spans for a look-back of `evaluation_periods`
/// ([`EVALUATION_RANGE_FACTOR`]).
fn evaluation_range_periods(evaluation_periods: usize) -> usize {
    evaluation_periods.saturating_mul(EVALUATION_RANGE_FACTOR)
}

/// The names of `alarms` in the order their evaluations came DUE: oldest first, and by name
/// between two owed one at the same instant.
///
/// The one definition of that order, so the metric-alarm pass
/// ([`State::evaluate_metric_alarms`]) and the composite-alarm pass
/// ([`super::ComAmazonawsCloudwatchGraniteserviceversion20100801State::evaluate_composite_alarms`])
/// cannot settle the alarms a request pays for in two different orders. Oldest first is what
/// makes the alarm that has waited longest the one a request whose budget stops it early still
/// settles; the name tie-break is what keeps the pass deterministic.
pub(super) fn due_order<'name>(
    alarms: impl Iterator<Item = (&'name str, f64)>,
) -> Vec<String> {
    let mut ordered: Vec<(&str, f64)> = alarms.collect();
    ordered.sort_by(|(left_name, left_due), (right_name, right_due)| {
        left_due
            .total_cmp(right_due)
            .then_with(|| left_name.cmp(right_name))
    });
    ordered
        .into_iter()
        .map(|(name, _due)| name.to_string())
        .collect()
}

/// Whether `datum` is one the alarm's `Unit` selects ([`alarm_grades_unit`]).
fn carries_alarm_unit(alarm: &MetricAlarm, datum: &StoredDatum) -> bool {
    alarm_grades_unit(alarm, datum.unit.as_deref())
}

/// Whether a datapoint carrying `unit` is one this alarm GRADES: "If you specify a unit, the
/// operation returns only data that was collected with that unit specified." An alarm naming no
/// unit grades every datapoint of the series it watches.
fn alarm_grades_unit(alarm: &MetricAlarm, unit: Option<&str>) -> bool {
    grades_unit(alarm.unit.as_deref(), unit)
}

/// Whether a datapoint carrying `unit` is one a reader selecting `selector` GRADES: "If you
/// specify a unit, the operation returns only data that was collected with that unit
/// specified." A reader naming no unit grades every datapoint of the series.
///
/// The one definition of that selector, so the evaluation fold, the forced-state span and
/// [`State::make_alarms_due`] cannot answer "is this data for this alarm?" three ways —
/// whether the unit is the alarm's own or the one its query names.
pub(super) fn grades_unit(selector: Option<&str>, unit: Option<&str>) -> bool {
    selector.is_none_or(|wanted| unit == Some(wanted))
}

/// One window of the series an alarm GRADES: the value it took, and how many observations it
/// aggregated where the series is one metric's.
#[derive(Debug, Clone, Copy)]
struct GradedDatapoint {
    /// The value the graded series took over the window.
    value: f64,
    /// How many observations that window aggregated, for a window of one metric's series;
    /// `None` where the value was computed rather than measured.
    sample_count: Option<f64>,
}

/// The series ONE alarm grades, window by window.
struct GradedSeries<'alarm> {
    /// The statistic the series was folded by, for an alarm that names one; `None` for a
    /// series a query array computed, which no single statistic describes.
    statistic: Option<AlarmStatistic<'alarm>>,
    /// How wide one window is, in seconds.
    period: f64,
    /// What each window of the evaluation range took, newest first — index 0 being the window
    /// the evaluation instant falls in — and `None` for a window the series has no value for.
    windows: Vec<Option<GradedDatapoint>>,
}

/// What ONE evaluation grades its series against: the alarm's threshold and comparison, how
/// many of its periods the verdict covers, and what it does with a window that has no value.
#[derive(Debug, Clone, Copy)]
struct GradedAgainst<'alarm> {
    /// "The value to compare with the specified statistic." (`MetricAlarm$Threshold`)
    threshold: f64,
    /// The comparison the alarm makes between a datapoint and that threshold.
    comparison: Comparison,
    /// How many of the alarm's periods the verdict is made over — its `EvaluationPeriods`.
    evaluation_periods: usize,
    /// "Sets how this alarm is to handle missing data points."
    /// (`MetricAlarm$TreatMissingData`)
    treat_missing: &'alarm str,
}

/// One datapoint ONE evaluation evaluated.
#[derive(Debug, Clone, Copy)]
struct Evaluated {
    /// Which window of the evaluation range the datapoint came from, counting back from the
    /// evaluation instant; `None` for a slot no real datapoint filled.
    window: Option<usize>,
    /// The statistic over that window, for a real datapoint.
    value: Option<f64>,
    /// How many observations that window aggregated, for a real datapoint.
    sample_count: Option<f64>,
    /// How it stands against the threshold.
    datapoint: Datapoint,
}

/// What the datapoints a verdict RESTS ON are: how a datapoint that satisfied the direction the
/// state was decided in reads against the threshold, and how many of them that direction needed.
#[derive(Debug, Clone, Copy)]
struct Verdict {
    /// The classification of a datapoint that satisfied the reported direction: breaching for
    /// `ALARM`, non-breaching for `OK`.
    satisfying: Datapoint,
    /// How many of them the direction needed — the alarm's M for `ALARM`, and `N - M + 1` for
    /// `OK`.
    minimum: usize,
}

/// The state one evaluation computed for one alarm, with the explanation it reports.
struct Evaluation {
    /// The computed `StateValue`.
    state: &'static str,
    /// `StateReason`: "The reason that this alarm is set to this specific state, in text
    /// format." (`MetricAlarm$StateReason`)
    reason: String,
    /// `StateReasonData`: "An explanation for the alarm state, in JSON format."
    reason_data: String,
}

impl State {
    /// Arm the span the state `SetAlarmState` has just forced on the metric alarm `name` stands
    /// for: its next evaluation is owed at the end of it
    /// ([`Self::forced_state_span_secs`]). A name no metric alarm carries arms nothing.
    pub fn arm_forced_state(&mut self, name: &str, now_secs: f64) {
        // The alarm as it now stands, read before the write: the span is a function of its own
        // configuration and of the series it watches.
        let Some(alarm) = self.alarms.get(name).map(|stored| stored.alarm.clone()) else {
            return;
        };
        let due = now_secs + self.forced_state_span_secs(&alarm, now_secs);
        if let Some(stored) = self.alarms.get_mut(name) {
            stored.evaluation_due_secs = due;
        }
    }

    /// How long a state `SetAlarmState` forces on `alarm` stands before the evaluation takes
    /// the alarm back over: until that alarm's NEXT evaluation.
    ///
    /// "Temporarily sets the state of an alarm for testing purposes. […] Metric alarms returns
    /// to their actual state quickly, often within seconds." (`SetAlarmState`) — what takes the
    /// alarm back is its next evaluation, and an alarm is evaluated from the datapoints of the
    /// metric it watches, "the length, in seconds, used each time the metric specified in
    /// MetricName is evaluated" being its own `Period` (`PutMetricAlarmInput$Period`). So:
    ///
    /// * a metric that is really being published — one whose series holds a datapoint inside
    ///   the range this alarm's evaluation grades — is evaluated again as soon as its next
    ///   datapoint lands, which is what "within seconds" describes, so the force stands for the
    ///   span every transition this store's clock owes ([`EVALUATION_DELAY_SECS`]) and the
    ///   datapoint that arrives meanwhile is what the reclaiming evaluation grades
    ///   ([`State::make_alarms_due`]);
    /// * a metric nothing writes has no arriving datapoint to trigger an evaluation, so the
    ///   force stands until the alarm's next SCHEDULED pass
    ///   ([`SCHEDULED_EVALUATION_SECS`]), bounded by the alarm's own period.
    ///
    /// An alarm whose configuration names no period at all — one evaluated over a `Metrics`
    /// query array or an `EvaluationCriteria` query, where the period belongs to each query —
    /// has no scheduled pass of its own, so the force stands for that same clock span. Either
    /// way the span is never zero, so a caller reading straight after the call still sees the
    /// state it forced.
    #[must_use]
    pub fn forced_state_span_secs(&self, alarm: &MetricAlarm, now_secs: f64) -> f64 {
        let Some(period) = alarm.period.filter(|period| *period >= 1).map(f64::from) else {
            return EVALUATION_DELAY_SECS;
        };
        if self.evaluation_range_holds_datapoint(alarm, period, now_secs) {
            return EVALUATION_DELAY_SECS;
        }
        period.clamp(EVALUATION_DELAY_SECS, SCHEDULED_EVALUATION_SECS)
    }

    /// Whether the series `alarm` watches holds a datapoint inside the range its evaluation
    /// grades at `now_secs`: the window span [`evaluation_range_periods`] covers, the series
    /// identity [`watched_series`] resolves and the unit selector [`carries_alarm_unit`]
    /// applies are the SAME ones [`Self::metric_alarm_evaluation`] folds with, so "the metric
    /// is being published" means exactly "this alarm has a datapoint to grade".
    ///
    /// One series is visited, bounded on its own by [`super::MAX_DATAPOINTS_PER_SERIES`].
    fn evaluation_range_holds_datapoint(
        &self,
        alarm: &MetricAlarm,
        period: f64,
        now_secs: f64,
    ) -> bool {
        let range_periods = evaluation_range_periods(evaluated_periods(alarm).unwrap_or(1));
        // The periods are ANCHORED at multiples of the period, exactly as the fold anchors
        // them: the oldest window of the range is the one whose index is `range_periods - 1`
        // counting back from the window `now_secs` falls in, so the range starts where that
        // window starts rather than one span back from this instant.
        #[allow(clippy::cast_precision_loss)]
        let oldest_window = (now_secs / period).floor() - (range_periods.saturating_sub(1) as f64);
        let range_from = oldest_window * period;
        self.metrics
            .get(&watched_series(alarm))
            .is_some_and(|series| {
                series.iter().any(|datum| {
                    datum.aggregated_instant() >= range_from && carries_alarm_unit(alarm, datum)
                })
            })
    }

    /// Materialise every metric-alarm evaluation the clock has already made due, within what
    /// `budget` lets this request pay for.
    ///
    /// Alarms are visited in the order their evaluations came due ([`due_order`]). Each
    /// alarm's evaluation reads only the metric store and the clock, so the order never
    /// changes the outcome, and an alarm the budget does not reach stays due for the next
    /// call — while the pass settles the first due alarm it finds whatever the budget already
    /// holds ([`EvaluationBudget::start_pass`]), so what it defers is never everything.
    pub fn evaluate_metric_alarms(&mut self, now_secs: f64, budget: &mut EvaluationBudget) {
        budget.start_pass();
        let ordered = due_order(
            self.alarms
                .iter()
                .map(|(name, stored)| (name.as_str(), stored.evaluation_due_secs)),
        );
        for name in ordered {
            if budget.spent() {
                break;
            }
            self.evaluate_metric_alarm(&name, now_secs, budget);
        }
    }

    /// Evaluate ONE metric alarm, writing its new state only when the computed state differs
    /// from the one the alarm reports.
    fn evaluate_metric_alarm(
        &mut self,
        name: &str,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) {
        let Some(stored) = self.alarms.get(name) else {
            return;
        };
        if now_secs < stored.evaluation_due_secs {
            return;
        }
        // Everything the evaluation reads is read before anything is written.
        let alarm = stored.alarm.clone();
        budget.charge_alarm();
        // This alarm is being evaluated now, so its NEXT evaluation is owed a span from here
        // rather than being due again at once: it is re-armed before the fold runs, so the
        // fold is charged to this request only and a following request pays for the alarms
        // that came due since.
        if let Some(stored) = self.alarms.get_mut(name) {
            stored.evaluation_due_secs = now_secs + EVALUATION_DELAY_SECS;
        }
        let Some(evaluation) = self.metric_alarm_evaluation(&alarm, now_secs, budget) else {
            // This pass has paid for one alarm's evaluation, so what remains of the budget
            // bounds it again — whether the evaluation computed a state or this is an alarm
            // this engine computes nothing for.
            budget.settled_one();
            return;
        };
        budget.settled_one();
        if alarm.state_value.as_deref() == Some(evaluation.state) {
            return;
        }

        let previous = AlarmStateSnapshot::of_metric_alarm(&alarm);
        let summary = format!(
            "Alarm updated from {} to {}",
            previous
                .state_value
                .as_deref()
                .unwrap_or(ALARM_STATE_INSUFFICIENT_DATA),
            evaluation.state
        );
        let Some(stored) = self.alarms.get_mut(name) else {
            return;
        };
        stored.alarm.state_value = Some(evaluation.state.to_string());
        stored.alarm.state_reason = Some(evaluation.reason);
        stored.alarm.state_reason_data = Some(evaluation.reason_data);
        stored.alarm.state_updated_timestamp = Some(to_timestamp(now_secs));
        // "The timestamp of the last change to the alarm's StateValue."
        // (`MetricAlarm$StateTransitionedTimestamp`) — this evaluation changed it.
        stored.alarm.state_transitioned_timestamp = Some(to_timestamp(now_secs));
        let updated = AlarmStateSnapshot::of_metric_alarm(&stored.alarm);
        let notification = AlarmNotification::of_transition(
            &stored.alarm,
            previous.state_value.as_deref(),
            now_secs,
        );

        self.record_history(
            name,
            now_secs,
            AlarmHistoryItem {
                alarm_name: Some(name.to_string()),
                alarm_contributor_id: None,
                alarm_type: Some(ALARM_TYPE_METRIC.to_string()),
                timestamp: Some(to_timestamp(now_secs)),
                history_item_type: Some("StateUpdate".to_string()),
                history_summary: Some(summary),
                history_data: Some(super::state_transition_history_data(&previous, &updated)),
                alarm_contributor_attributes: None,
            },
        );
        if let Some(notification) = notification {
            self.enqueue_notification(notification);
        }
        // This alarm's new state is an input to every composite alarm whose rule reads it, so
        // their evaluations are owed from this instant too — and the composite pass that
        // follows this one in the same settle takes them.
        self.make_parents_due(name, now_secs);
    }

    /// The state the alarm's own comparison puts it in at `now_secs`, or `None` for an alarm
    /// this engine does not evaluate (see the module docs) and for one whose computed state
    /// is the state it already has. The datapoints the fold visits are charged to `budget`.
    fn metric_alarm_evaluation(
        &self,
        alarm: &MetricAlarm,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<Evaluation> {
        // An alarm over a PromQL query, and one whose threshold is an anomaly-detection band
        // rather than a number, is not graded against a static threshold at all, so nothing
        // is computed for it.
        if alarm.evaluation_criteria.is_some() || alarm.threshold_metric_id.is_some() {
            return None;
        }
        let threshold = alarm.threshold?;
        let comparison = comparison_of(alarm.comparison_operator.as_deref()?)?;
        let evaluation_periods = evaluated_periods(alarm)?;
        // The windows of the EVALUATION RANGE: window 0 is the period the evaluation instant
        // falls in, window i the period before it, and the range reaches further back than the
        // look-back ([`EVALUATION_RANGE_FACTOR`]).
        let range_periods = evaluation_range_periods(evaluation_periods);
        // The series this alarm GRADES, as the value each of those windows took.
        let graded = self.graded_series(alarm, range_periods, now_secs, budget)?;

        let treat_missing = alarm.treat_missing_data.as_deref().unwrap_or(MISSING_MISSING);
        // The REAL datapoints of the range, newest first and at most `EvaluationPeriods` of
        // them: "all real data points in the evaluation range are included in the
        // evaluation", and where the range holds more than the alarm evaluates, it is the
        // most recent ones it evaluates.
        let mut evaluated: Vec<Evaluated> = Vec::with_capacity(evaluation_periods);
        for (index, window) in graded.windows.iter().enumerate() {
            if evaluated.len() == evaluation_periods {
                break;
            }
            let Some(datapoint) = window else {
                continue;
            };
            evaluated.push(Evaluated {
                window: Some(index),
                value: Some(datapoint.value),
                // How many observations the window stood for, which is what a decision record
                // reports beside the value the statistic took over them.
                sample_count: datapoint.sample_count,
                datapoint: if (comparison.breaches)(datapoint.value, threshold) {
                    Datapoint::Breaching
                } else {
                    Datapoint::NotBreaching
                },
            });
        }
        self.graded_evaluation(
            alarm,
            &graded,
            GradedAgainst {
                threshold,
                comparison,
                evaluation_periods,
                treat_missing,
            },
            evaluated,
            now_secs,
        )
    }

    /// The series ONE alarm grades, as the value each window of its evaluation range took.
    ///
    /// Which series that is, is what the alarm's configuration says: an alarm over one metric
    /// grades that metric folded by its own `Statistic` over its own `Period`, and an alarm
    /// configured with a query array ("Use the `Metrics` parameter to create an alarm based on
    /// a metric math expression", `PutMetricAlarmInput$Metrics`) grades the value of the query
    /// that array RETURNS — the expression's output, not the metrics it reads. Either way the
    /// values come from the datapoints `PutMetricData` stored and from nothing else.
    fn graded_series<'alarm>(
        &self,
        alarm: &'alarm MetricAlarm,
        range_periods: usize,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<GradedSeries<'alarm>> {
        match alarm.metrics.as_deref() {
            Some(queries) => self.graded_query_array(queries, range_periods, now_secs, budget),
            None => self.graded_metric(alarm, range_periods, now_secs, budget),
        }
    }

    /// The windows of the ONE metric an alarm names, folded by the statistic it names.
    fn graded_metric<'alarm>(
        &self,
        alarm: &'alarm MetricAlarm,
        range_periods: usize,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<GradedSeries<'alarm>> {
        let statistic = AlarmStatistic::of(alarm)?;
        let period = f64::from(alarm.period.filter(|period| *period >= 1)?);
        let key = watched_series(alarm);
        // An alarm this engine evaluates names the series it watches, which `watched_series`
        // resolves from those same members.
        if key.namespace.is_empty() || key.metric_name.is_empty() {
            return None;
        }

        // One pass over the series fills every window of the evaluation range.
        let mut windows: Vec<Vec<&StoredDatum>> = vec![Vec::new(); range_periods];
        let series = self.metrics.get(&key);
        // The period the evaluation instant falls in, as a count of whole periods since the
        // epoch: the anchor every datapoint's window is measured against.
        let current_period = (now_secs / period).floor();
        // The datapoints this fold is about to visit, charged before it visits them: this is
        // the factor that multiplies with the number of alarms one request evaluates.
        budget.charge_datapoints(series.map_or(0, Vec::len));
        for datum in series.into_iter().flatten() {
            // The alarm's `Unit` selects which datapoints of the series it watches.
            if !carries_alarm_unit(alarm, datum) {
                continue;
            }
            // A datapoint belongs to the period its OWN time stamp falls in, so which window
            // it is aggregated into is decided by the datapoint and not by the instant the
            // evaluation runs — the same fold this store's readers make over a series
            // ([`super::windows_of`]), down to the granularity the datapoint's own
            // `StorageResolution` stores it at ([`StoredDatum::aggregated_instant`]).
            //
            // A datapoint may be stamped up to two hours ahead of now ("as much as 2 hours
            // after the current day and time", `PutMetricData`); it belongs to the newest
            // window, the one the evaluation instant falls in.
            let periods_back =
                (current_period - (datum.aggregated_instant() / period).floor()).max(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = periods_back as usize;
            if let Some(window) = windows.get_mut(index) {
                window.push(datum);
            }
        }

        Some(GradedSeries {
            // The statistic the alarm folded its windows by, which its decision record names.
            statistic: Some(statistic),
            period,
            windows: windows
                .iter()
                .map(|window| {
                    statistic.over(window).map(|value| GradedDatapoint {
                        value,
                        // How many observations the window stood for, which is what a
                        // decision record reports beside the value over them.
                        sample_count: Some(window.iter().map(|datum| datum.sample_count).sum()),
                    })
                })
                .collect(),
        })
    }

    /// The windows of the series an alarm's `Metrics` query ARRAY grades: the value of the
    /// query that array returns, computed over the same stored datapoints `GetMetricData`
    /// answers the same array from ([`super::metric_query`]).
    ///
    /// `None` for an array this engine cannot resolve to ONE series — a malformed or
    /// unevaluable expression, a query naming no stored metric, a value that is a number or
    /// several series rather than one — so such an alarm keeps the state it has rather than
    /// being given one nothing computed.
    fn graded_query_array(
        &self,
        queries: &[MetricDataQuery],
        range_periods: usize,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<GradedSeries<'static>> {
        // "When used in PutMetricAlarm, specify true for the one expression result to use as
        // the alarm." … "If you omit this, the default of true is used."
        // (`MetricDataQuery$ReturnData`) — the query that returns data is the one the alarm
        // grades, whether it is an expression or a metric.
        let graded = queries
            .iter()
            .find(|query| query.return_data != Some(false))?;
        let period = graded_period(queries, graded);
        // The range the array is resolved over is exactly the windows the evaluation grades:
        // anchored at multiples of the period, the newest being the one this instant falls in.
        let current_period = (now_secs / period).floor();
        #[allow(clippy::cast_precision_loss)]
        let start = (current_period - (range_periods.saturating_sub(1) as f64)) * period;
        let end = (current_period + 1.0) * period;

        // How much work this array asks for is bounded BEFORE anything is parsed or folded:
        // the queries of the array and the windows of the range are the factors of every walk
        // below, so an array past what one resolution may do leaves the alarm in the state it
        // has rather than being graded from a part of itself.
        if budget.spent()
            || super::metric_query::resolution_refusal(queries, start, end).is_some()
        {
            return None;
        }
        // Every expression is parsed, and the order they are evaluated in settled, before any
        // of them is evaluated, exactly as a request over the same array is answered.
        let parsed = super::metric_query::parse_queries(queries).ok()?;
        let order = super::metric_query::evaluation_order(queries, &parsed).ok()?;
        let mut resolution = super::metric_query::Resolution::default();
        for query in queries {
            // The queries of ONE array are the second factor of this pass's work, so the
            // budget is checked between them too: an array the budget does not reach leaves the
            // alarm due for the next call rather than being graded from a part of itself.
            if budget.spent() {
                return None;
            }
            let Some(stat) = query.metric_stat.as_ref() else {
                continue;
            };
            // The datapoints this query's fold is about to visit, charged before it visits
            // them, as for an alarm over one metric.
            budget.charge_datapoints(
                super::metric_query::query_series_key(stat)
                    .and_then(|key| self.metrics.get(&key))
                    .map_or(0, Vec::len),
            );
            let series =
                super::metric_query::retrieved_series(self, query, stat, start, end, now_secs)
                    .ok()?;
            resolution.retrieve(query, series).ok()?;
        }
        for id in order {
            if budget.spent() {
                return None;
            }
            let tree = parsed.get(id)?;
            let evaluated = resolution
                .evaluate(
                    id,
                    tree,
                    &super::metric_query::StoreSeriesSource {
                        store: self,
                        requested_start: start,
                        end,
                        now: now_secs,
                    },
                    (start, end),
                    period,
                )
                .ok()?;
            resolution.record(id, evaluated);
        }

        // "Any expression used in a PutMetricAlarm operation must return a single time
        // series." (`MetricDataQuery`) — a number is no series at all, and an array of
        // several is not the one series an alarm watches.
        let points = match resolution.value(&graded.id)? {
            MathValue::Series(series) => &series.points,
            MathValue::Array(series) if series.len() == 1 => &series.first()?.points,
            _ => return None,
        };
        // Which window each point of that series belongs to is decided by its own time stamp,
        // the same anchoring an alarm over one metric folds its datapoints by; where a series
        // carries several points inside one window — an expression spaced finer than the
        // period it is graded in — the window took the NEWEST of them.
        let mut graded_windows: Vec<Option<(f64, GradedDatapoint)>> = vec![None; range_periods];
        for (timestamp, value) in points {
            // A point the arithmetic produced no finite number for is not a value the alarm
            // can grade, and it is reported as dropped rather than graded as a zero.
            if !value.is_finite() {
                continue;
            }
            let periods_back = (current_period - (timestamp / period).floor()).max(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = periods_back as usize;
            let Some(slot) = graded_windows.get_mut(index) else {
                continue;
            };
            if slot.is_none_or(|(held, _)| *timestamp >= held) {
                *slot = Some((
                    *timestamp,
                    GradedDatapoint {
                        value: *value,
                        // Nothing measured a sample count for a value an expression computed:
                        // the series is not one metric's observations.
                        sample_count: None,
                    },
                ));
            }
        }
        Some(GradedSeries {
            // No single statistic describes a series a query array computed, so its decision
            // record names none.
            statistic: None,
            period,
            windows: graded_windows
                .into_iter()
                .map(|slot| slot.map(|(_, datapoint)| datapoint))
                .collect(),
        })
    }

    /// The state the alarm's "M out of N" predicate puts it in over the datapoints `evaluated`,
    /// with the explanation it reports — the one verdict every metric alarm is decided by,
    /// whatever series it grades.
    fn graded_evaluation(
        &self,
        alarm: &MetricAlarm,
        graded: &GradedSeries<'_>,
        against: GradedAgainst<'_>,
        mut evaluated: Vec<Evaluated>,
        now_secs: f64,
    ) -> Option<Evaluation> {
        let GradedAgainst {
            threshold,
            comparison,
            evaluation_periods,
            treat_missing,
        } = against;
        let period = graded.period;
        // What the missing-data treatment fills the slots no real datapoint filled with: a
        // substituted datapoint under `breaching` and `notBreaching`, and nothing at all
        // under `missing` and `ignore`, which leave the evaluation with fewer datapoints.
        let substituted = match treat_missing {
            MISSING_BREACHING => Datapoint::Breaching,
            MISSING_NOT_BREACHING => Datapoint::NotBreaching,
            _ => Datapoint::Missing,
        };
        for _ in evaluated.len()..evaluation_periods {
            evaluated.push(Evaluated {
                window: None,
                value: None,
                sample_count: None,
                datapoint: substituted,
            });
        }

        let breaching = evaluated
            .iter()
            .filter(|slot| slot.datapoint == Datapoint::Breaching)
            .count();
        let not_breaching = evaluated
            .iter()
            .filter(|slot| slot.datapoint == Datapoint::NotBreaching)
            .count();
        let present = evaluated
            .iter()
            .filter(|slot| slot.datapoint != Datapoint::Missing)
            .count();
        // The slots no real datapoint filled that the treatment SUBSTITUTED a datapoint for:
        // these are the holes the evaluation graded without having seen any data for them, and
        // the reason has to report them as holes rather than as datapoints it counted.
        let substituted = evaluated
            .iter()
            .filter(|slot| slot.window.is_none() && slot.datapoint != Datapoint::Missing)
            .count();
        // "The number of data points that must be breaching to trigger the alarm. This is
        // used only if you are setting an "M out of N" alarm." — an alarm that names no M
        // breaches when every one of its N datapoints does.
        let to_alarm = alarm
            .datapoints_to_alarm
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count >= 1)
            .unwrap_or(evaluation_periods)
            .min(evaluation_periods);
        // The complement of M over the same N: the number of NON-breaching datapoints that
        // makes M breaching ones unreachable inside the window, which is the minimum an
        // evaluation needs to put the alarm in `OK`. Both minima are built from the alarm's
        // configuration, so neither depends on how many datapoints this one evaluation found.
        let to_ok = evaluation_periods
            .saturating_sub(to_alarm)
            .saturating_add(1);

        let state = if breaching >= to_alarm {
            ALARM_STATE_ALARM
        } else if not_breaching >= to_ok {
            ALARM_STATE_OK
        } else if treat_missing == MISSING_IGNORE {
            // Neither direction's minimum was reached, and the datapoints the treatment left
            // out are why: "ignore — the current alarm state is maintained", so this
            // evaluation produces no state at all and the alarm keeps the one it has.
            return None;
        } else {
            // The window the evaluation graded is short of the datapoints EITHER direction
            // needs — the treatments that substitute nothing are the only ones that can leave
            // it so — so there is no verdict to report: the alarm had insufficient data.
            ALARM_STATE_INSUFFICIENT_DATA
        };
        // An evaluation that computes the state the alarm already reports changes nothing, so
        // there is no transition to explain: the alarm keeps the reason it has rather than
        // being given one that names a state as the state it moved from and to.
        if alarm.state_value.as_deref() == Some(state) {
            return None;
        }
        // Which datapoints the verdict RESTS ON: the direction the state was decided in, and
        // how many datapoints of it that direction needed. A state no datapoint decided —
        // insufficient data — rests on none, and its sentence names no minimum either.
        let verdict = if state == ALARM_STATE_ALARM {
            Some(Verdict {
                satisfying: Datapoint::Breaching,
                minimum: to_alarm,
            })
        } else if state == ALARM_STATE_OK {
            Some(Verdict {
                satisfying: Datapoint::NotBreaching,
                minimum: to_ok,
            })
        } else {
            None
        };
        let counts = EvaluationCounts {
            breaching,
            not_breaching,
            present,
            substituted,
            periods: evaluated.len(),
            minimum: verdict.map_or(0, |verdict| verdict.minimum),
        };
        Some(Evaluation {
            state,
            reason: state_reason(
                state,
                counts,
                missing_data_arm(treat_missing),
                comparison.phrase,
                threshold,
                &cited_datapoints(&evaluated, now_secs, period),
            ),
            reason_data: state_reason_data(
                graded.statistic.as_ref(),
                period,
                threshold,
                now_secs,
                &evaluated,
                verdict,
            ),
        })
    }

    /// Queue one action notification a transition owes, within
    /// [`MAX_PENDING_NOTIFICATIONS`].
    pub(crate) fn enqueue_notification(&mut self, notification: AlarmNotification) {
        while self.pending_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
            let Some(dropped) = self.pending_notifications.pop_front() else {
                break;
            };
            ::tracing::warn!(
                alarm = %dropped.alarm_name,
                pending = MAX_PENDING_NOTIFICATIONS,
                "this store already holds the most undelivered alarm notifications it can; \
                 the oldest is dropped undelivered"
            );
        }
        self.pending_notifications.push_back(notification);
    }

    /// Take at most `limit` of the queued notifications, oldest first, leaving the rest for
    /// the next caller: the caller DELIVERS what it takes, so a taken notification must not be
    /// left for another caller to deliver twice, and a limit is what bounds the fan-out ONE
    /// request performs.
    pub fn take_pending_notifications(
        &mut self,
        limit: usize,
    ) -> std::collections::VecDeque<AlarmNotification> {
        let taken = limit.min(self.pending_notifications.len());
        self.pending_notifications.drain(..taken).collect()
    }
}

/// What one `ComparisonOperator` means to an evaluation: the test it makes between a datapoint
/// and the threshold, and the words an explanation states that test in.
///
/// The two live together so a reason can never word a comparison the evaluation did not make.
#[derive(Debug, Clone, Copy)]
struct Comparison {
    /// Whether a datapoint BREACHES the threshold under this operator.
    breaches: fn(f64, f64) -> bool,
    /// How a `StateReason` reads the comparison: "… was <phrase> the threshold (<threshold>)",
    /// and negated ("was not <phrase> …") for the state in which it did not hold.
    phrase: &'static str,
}

/// The comparison one `ComparisonOperator` makes between a datapoint and the threshold, or
/// `None` for the anomaly-detection band operators, which compare against a model this
/// engine does not compute rather than against a static threshold.
fn comparison_of(comparison: &str) -> Option<Comparison> {
    // The static-threshold members of the modelled `ComparisonOperator` enum
    // (`smithy_query(query="shape",shape="ComparisonOperator")`), each with the reading
    // CloudWatch's own alarm explanations give it.
    match comparison {
        "GreaterThanOrEqualToThreshold" => Some(Comparison {
            breaches: |value, threshold| value >= threshold,
            phrase: "greater than or equal to",
        }),
        "GreaterThanThreshold" => Some(Comparison {
            breaches: |value, threshold| value > threshold,
            phrase: "greater than",
        }),
        "LessThanThreshold" => Some(Comparison {
            breaches: |value, threshold| value < threshold,
            phrase: "less than",
        }),
        "LessThanOrEqualToThreshold" => Some(Comparison {
            breaches: |value, threshold| value <= threshold,
            phrase: "less than or equal to",
        }),
        // "LessThanLowerOrGreaterThanUpperThreshold", "LessThanLowerThreshold" and
        // "GreaterThanUpperThreshold" compare against an anomaly detection band.
        _ => None,
    }
}

/// The statistic one alarm aggregates each of its windows with.
#[derive(Debug, Clone, Copy)]
enum AlarmStatistic<'a> {
    /// "The statistic for the metric specified in MetricName" — one of the basic five
    /// (`MetricAlarm$Statistic`).
    Basic(&'a str),
    /// "The percentile statistic for the metric associated with the alarm"
    /// (`MetricAlarm$ExtendedStatistic`), as the name and the percentile it parses to.
    Percentile(&'a str, f64),
}

impl AlarmStatistic<'_> {
    /// The statistic the alarm names, or `None` when it names none this engine can compute:
    /// "For an alarm based on a metric, you must specify either Statistic or
    /// ExtendedStatistic but not both." (`PutMetricAlarmInput$Statistic`)
    fn of(alarm: &MetricAlarm) -> Option<AlarmStatistic<'_>> {
        if let Some(statistic) = alarm.statistic.as_deref() {
            // Only one of the modelled `Statistic` values is a statistic `BasicStatistics`
            // folds; anything else names a statistic nothing computes.
            return STATISTIC_VALUES
                .contains(&statistic)
                .then_some(AlarmStatistic::Basic(statistic));
        }
        let extended = alarm.extended_statistic.as_deref()?;
        super::parse_percentile(extended).map(|percentile| AlarmStatistic::Percentile(extended, percentile))
    }

    /// This statistic over one window's datapoints; `None` for a window with no datapoint of
    /// the metric at all, and for one whose individual values a percentile needs were never
    /// published.
    fn over(&self, window: &[&StoredDatum]) -> Option<f64> {
        if window.is_empty() {
            return None;
        }
        match self {
            Self::Basic(name) => BasicStatistics::of(window.iter().copied()).named(name),
            Self::Percentile(_, percentile) => weighted_percentile(window, *percentile),
        }
    }

    /// The name the alarm gave this statistic.
    fn name(&self) -> &str {
        match self {
            Self::Basic(name) | Self::Percentile(name, _) => name,
        }
    }

    /// Whether this is the percentile form, which a reason document records under its own
    /// member.
    fn is_percentile(&self) -> bool {
        matches!(self, Self::Percentile(_, _))
    }
}

/// What ONE evaluation counted over the slots it evaluated, which is what its `StateReason`
/// explains.
#[derive(Debug, Clone, Copy)]
struct EvaluationCounts {
    /// Datapoints whose comparison against the threshold held.
    breaching: usize,
    /// Datapoints the comparison did not hold for: the ones that satisfy the `OK` direction of
    /// the alarm's predicate.
    not_breaching: usize,
    /// Datapoints the evaluation really had to decide on: the real ones plus the ones the
    /// missing-data treatment substituted.
    present: usize,
    /// How many of those were SUBSTITUTED for a window that held no datapoint of the metric.
    substituted: usize,
    /// How many windows the look-back covered — the alarm's `EvaluationPeriods`.
    periods: usize,
    /// How many datapoints the DIRECTION this state was decided in needs: the alarm's M for
    /// `ALARM`, and its complement `N - M + 1` for `OK`. Zero for a state no direction decided,
    /// whose sentence names no minimum.
    minimum: usize,
}

/// The name the missing-data treatment is reported under in a `StateReason`, or `None` for a
/// treatment that substitutes no datapoint at all (`missing` and `ignore` leave a hole out of
/// the evaluation instead, so no arm decided anything for it).
///
/// The bracketed names are the recorded wording of the reason AWS reports for an evaluation
/// whose windows held no data; the arms themselves are the documented `TreatMissingData`
/// values (`smithy_query(query="doc",shape="MetricAlarm",member="TreatMissingData")`).
fn missing_data_arm(treat_missing: &str) -> Option<&'static str> {
    match treat_missing {
        MISSING_BREACHING => Some("Breaching"),
        MISSING_NOT_BREACHING => Some("NonBreaching"),
        _ => None,
    }
}

/// The noun suffix and verb a count takes: "1 datapoint was", "2 datapoints were".
fn plural(count: usize) -> (&'static str, &'static str) {
    if count == 1 {
        ("", "was")
    } else {
        ("s", "were")
    }
}

/// The `StateReason` an evaluated state reports: the DATAPOINTS the evaluation graded, how many
/// of them read the way the state says against the threshold, the minimum the transition needed
/// and the transition itself — and, where a window held no datapoint of the metric at all, that
/// the window was EMPTY and which missing-data arm was applied to it.
///
/// The sentence is the one CloudWatch publishes for an evaluated alarm:
/// `Threshold Crossed: <n> out of the last <m> datapoints [<value> (<instant>), …] was|were
/// [not] <comparison> the threshold (<threshold>) (minimum <k> datapoint(s) for <from> -> <to>
/// transition).` for a graded state, and `Insufficient Data: <m> datapoint(s) was|were unknown.`
/// for a state decided with too few datapoints to grade. The state the alarm moved to decides
/// whether the comparison is stated as it holds or negated, the count is of the datapoints that
/// read that way, and the closing clause names that direction and the minimum it needed
/// ([`transition_minimum`]).
///
/// The datapoints are NAMED, not just counted ([`cited_datapoints`]): the explanation of a
/// state says which values decided it and when each was measured, so a reader can tell the
/// evaluation that graded the series it published from one that graded something else.
///
/// A hole is reported as a hole: an evaluation every one of whose datapoints was substituted
/// by the missing-data treatment graded no data, so it says it received none and names the arm
/// that decided the state instead of reporting a count of datapoints it never had, and an
/// evaluation that mixed real datapoints with substituted ones names the arm beside its
/// counts.
///
/// Built from the evaluation rather than from any request: the text states the datapoints, the
/// counts, the comparison and the threshold the alarm is configured with. Inside the modelled
/// `@length 0..1023` of `StateReason` for every alarm, because the only variable parts are
/// four counts, two state names, a comparison reading of at most 24 characters, one number, and
/// a datapoint list bounded by [`MAX_CITED_DATAPOINT_CHARS`].
fn state_reason(
    state: &str,
    counts: EvaluationCounts,
    arm: Option<&str>,
    comparison: &str,
    threshold: f64,
    cited: &str,
) -> String {
    let EvaluationCounts {
        breaching,
        not_breaching,
        present,
        substituted,
        periods,
        minimum,
    } = counts;
    let threshold = decimal_text(threshold);
    if state == ALARM_STATE_INSUFFICIENT_DATA {
        // The window was short of the datapoints either direction of the predicate needs, so
        // the evaluation reports the ones it did not have: the windows of its look-back that
        // no datapoint, real or substituted, stood for.
        let unknown = periods.saturating_sub(present);
        let (plural_suffix, verb) = plural(unknown);
        return format!("Insufficient Data: {unknown} datapoint{plural_suffix} {verb} unknown.");
    }
    if let Some(arm) = arm.filter(|_| substituted > 0 && substituted == present) {
        // EVERY slot was substituted: the state was decided by the missing-data arm alone, so
        // that — and not a graded verdict over datapoints the evaluation never received — is
        // what the reason reports.
        let (periods_suffix, _) = plural(periods);
        let (datapoints_suffix, verb) = plural(substituted);
        return format!(
            "Threshold Crossed: no datapoints were received for {periods} \
             period{periods_suffix} and {substituted} missing datapoint{datapoints_suffix} \
             {verb} treated as [{arm}]."
        );
    }
    // Some real datapoint was graded. Where the treatment also substituted datapoints for
    // windows that held none, the reason names those holes and the arm applied to them beside
    // the counts.
    let substituted_clause = match arm.filter(|_| substituted > 0) {
        Some(arm) => {
            let (datapoints_suffix, verb) = plural(substituted);
            format!(
                " {substituted} missing datapoint{datapoints_suffix} {verb} treated as [{arm}]."
            )
        }
        None => String::new(),
    };
    if state == ALARM_STATE_ALARM {
        // The comparison HELD often enough, so the sentence states it as it reads and counts
        // the datapoints it held for.
        return format!(
            "Threshold Crossed: {breaching} out of the last {present} datapoints{cited} \
             {} {comparison} the threshold ({threshold}) ({}).{substituted_clause}",
            plural(breaching).1,
            transition_minimum(minimum, state)
        );
    }
    // The alarm is in OK, so the comparison did not hold: the sentence states its NEGATION and
    // counts the datapoints that read that way — the ones that satisfied the direction being
    // reported, not the ones that breached.
    format!(
        "Threshold Crossed: {not_breaching} out of the last {present} datapoints{cited} {} not \
         {comparison} the threshold ({threshold}) ({}).{substituted_clause}",
        plural(not_breaching).1,
        transition_minimum(minimum, state)
    )
}

/// The clause a graded reason closes with: how many datapoints the direction the state was
/// decided in needs, and that direction — `minimum 1 datapoint for OK -> ALARM transition` for a
/// state the breaching datapoints decided, and `minimum <N - M + 1> datapoints for ALARM -> OK
/// transition` for one the non-breaching ones decided.
///
/// The clause names the DIRECTION of the predicate the state stands for, not the state the alarm
/// happened to report before: the minimum belongs to the transition into the state being
/// explained, so both integers of an "M out of N" configuration are recoverable from the
/// sentence whichever state the evaluation moved out of.
fn transition_minimum(minimum: usize, state: &str) -> String {
    let (datapoints_suffix, _) = plural(minimum);
    let transition = if state == ALARM_STATE_ALARM {
        format!("{ALARM_STATE_OK} -> {ALARM_STATE_ALARM}")
    } else {
        format!("{ALARM_STATE_ALARM} -> {ALARM_STATE_OK}")
    };
    format!("minimum {minimum} datapoint{datapoints_suffix} for {transition} transition")
}

/// The most characters the datapoint list a `StateReason` cites may take.
///
/// `StateReason` is modelled `@length 0..1023` and the sentence around the list needs room
/// inside that, so the list is the part that is bounded: an alarm may evaluate as many windows
/// as [`MAX_EVALUATED_PERIODS`] and each datapoint renders a value of its own width. The
/// datapoints past the bound are left out rather than the sentence truncated, because the
/// counts and the threshold are what the state was decided on and `StateReasonData` carries
/// every evaluated datapoint anyway.
const MAX_CITED_DATAPOINT_CHARS: usize = 700;

/// The datapoints an evaluation GRADED, newest first, each as the value the statistic took over
/// its window and the instant that window ends: ` [137.0 (25/10/26 02:02:00), …]`.
///
/// Empty when the evaluation graded no real datapoint at all — a slot the missing-data
/// treatment substituted stands for a window that held nothing, so there is no measured value
/// to name for it — which is also why the list is prefixed with its own space and can be
/// dropped from the sentence without leaving a gap.
fn cited_datapoints(evaluated: &[Evaluated], now_secs: f64, period: f64) -> String {
    let mut cited = String::new();
    for slot in evaluated {
        let (Some(index), Some(value)) = (slot.window, slot.value) else {
            continue;
        };
        let entry = match reason_instant(window_start(now_secs, index, period)) {
            Some(at) => format!("{} ({at})", decimal_text(value)),
            None => decimal_text(value),
        };
        if cited.len() + entry.len() + 2 > MAX_CITED_DATAPOINT_CHARS {
            break;
        }
        if !cited.is_empty() {
            cited.push_str(", ");
        }
        cited.push_str(&entry);
    }
    if cited.is_empty() {
        return cited;
    }
    format!(" [{cited}]")
}

/// The instant the `index`th period back from the one the evaluation instant falls in BEGINS,
/// which is the time stamp a datapoint aggregated out of that period carries.
///
/// The periods are the ones this store's readers fold a series into, so the start of one is
/// the shared [`super::window_start`] measured from the period the evaluation instant falls in:
/// one definition of a period's start serves the alarm evaluator and the metric readers.
fn window_start(now_secs: f64, index: usize, period: f64) -> f64 {
    let current_period_start = (now_secs / period).floor() * period;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let periods_back = index as i64;
    super::window_start(current_period_start, -periods_back, period)
}

/// The instant a graded datapoint is named by, in the form an alarm explanation writes one:
/// the day, the month and the last two digits of the year, then the time of day in UTC to the
/// second (`25/10/26 17:40:25`).
///
/// The calendar fields come from the modelled timestamp type's own rendering
/// ([`super::rfc3339`]) and are only re-ordered here, so no second date implementation lives
/// in this crate and an instant a sentence names is the instant a document reports. `None` for
/// an instant that type cannot render.
fn reason_instant(secs: f64) -> Option<String> {
    /// How many digits of the year the form writes.
    const YEAR_DIGITS: usize = 2;

    let rendered = super::rfc3339(secs)?;
    let (date, time) = rendered.split_once('T')?;
    let mut fields = date.split('-');
    let year = fields.next()?;
    let month = fields.next()?;
    let day = fields.next()?;
    // Whole seconds: neither the fraction a timestamp may carry nor the zone suffix is part of
    // the form.
    let time = time.split('.').next()?.trim_end_matches('Z');
    let year = year.get(year.len().saturating_sub(YEAR_DIGITS)..)?;
    Some(format!("{day}/{month}/{year} {time}"))
}

/// One measured quantity as an explanation names it: a decimal that always carries a fraction,
/// so a value and a threshold read as the measurements they are rather than as counts.
fn decimal_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        return format!("{value:.1}");
    }
    format!("{value}")
}

/// The `StateReasonData` an evaluated state reports: the query the evaluation made, the values
/// of the window it graded, and the datapoints the verdict RESTS ON — the ones that satisfied
/// the direction the state was decided in, as many as that direction needed, each with the
/// instant of the period it was aggregated out of and how many observations that period held.
///
/// A period the missing-data treatment substituted a datapoint for carries no instant and no
/// value: nothing was measured over it, and the arm that decided it is what the entry names.
///
/// "An explanation for the alarm state, in JSON format." (`MetricAlarm$StateReasonData`),
/// bounded by [`MAX_STATE_REASON_DATA_LEN`] — a document is JSON, so it can never be
/// truncated to fit: the datapoint list is dropped instead when an alarm evaluates more
/// windows than the bound holds. Every instant inside it is written in the form a document of
/// this service stamps an instant with ([`super::document_instant`]).
///
/// The statistic is the one the graded series was FOLDED by, so a series a query array
/// computed — which no single statistic describes — contributes no statistic member.
fn state_reason_data(
    statistic: Option<&AlarmStatistic<'_>>,
    period: f64,
    threshold: f64,
    now_secs: f64,
    evaluated: &[Evaluated],
    verdict: Option<Verdict>,
) -> String {
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(HISTORY_DATA_VERSION));
    // The instant the evaluation queried the series at, and the start of the OLDEST period it
    // graded: together they say which span of the series the state was decided over. Both are
    // written in the form this service's JSON documents stamp an instant with
    // ([`super::document_instant`]).
    if let Some(at) = super::document_instant(now_secs) {
        document.insert("queryDate".to_string(), Value::from(at));
    }
    if let Some(at) = evaluated
        .iter()
        .filter_map(|slot| slot.window)
        .max()
        .and_then(|index| super::document_instant(window_start(now_secs, index, period)))
    {
        document.insert("startDate".to_string(), Value::from(at));
    }
    if let Some(statistic) = statistic {
        let member = if statistic.is_percentile() {
            "extendedStatistic"
        } else {
            "statistic"
        };
        document.insert(member.to_string(), Value::from(statistic.name()));
    }
    document.insert("period".to_string(), Value::from(period));
    document.insert("threshold".to_string(), Value::from(threshold));
    // The values the evaluation really had, oldest first — the order they were observed in.
    // `evaluated` is newest first and holds a slot for every period the alarm evaluates, so
    // the slots a missing-data treatment filled contribute no value here: this list reports
    // the window the state was decided over, and `evaluatedDatapoints` below reports the
    // datapoints the verdict rests on.
    let recent: Vec<Value> = evaluated
        .iter()
        .rev()
        .filter_map(|slot| slot.value.map(Value::from))
        .collect();
    document.insert("recentDatapoints".to_string(), Value::Array(recent));

    let mut datapoints: Vec<Value> = Vec::new();
    if let Some(verdict) = verdict {
        for slot in evaluated
            .iter()
            .filter(|slot| slot.datapoint == verdict.satisfying)
            .take(verdict.minimum)
        {
            let mut entry = Map::new();
            if let Some(at) = slot
                .window
                .and_then(|index| super::document_instant(window_start(now_secs, index, period)))
            {
                entry.insert("timestamp".to_string(), Value::from(at));
            }
            if let Some(count) = slot.sample_count {
                entry.insert("sampleCount".to_string(), Value::from(count));
            }
            if let Some(value) = slot.value {
                entry.insert("value".to_string(), Value::from(value));
            }
            entry.insert(
                "datapoint".to_string(),
                Value::from(match slot.datapoint {
                    Datapoint::Breaching => "breaching",
                    Datapoint::NotBreaching => "notBreaching",
                    Datapoint::Missing => "missing",
                }),
            );
            datapoints.push(Value::Object(entry));
        }
    }
    let mut full = document.clone();
    full.insert("evaluatedDatapoints".to_string(), Value::Array(datapoints));
    let rendered = Value::Object(full).to_string();
    if rendered.len() <= MAX_STATE_REASON_DATA_LEN {
        return rendered;
    }
    Value::Object(document).to_string()
}

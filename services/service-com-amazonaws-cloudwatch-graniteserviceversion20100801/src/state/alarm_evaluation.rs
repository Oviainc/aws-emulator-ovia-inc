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
//! * the metric is aggregated into windows of `Period` seconds each, the newest ending at
//!   the instant of the evaluation;
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
//! * the alarm is in `ALARM` once at least `DatapointsToAlarm` of the evaluated datapoints
//!   breach ("The number of data points that must be breaching to trigger the alarm",
//!   defaulting to `EvaluationPeriods`) — or, where the missing-data treatment left the
//!   evaluation with FEWER datapoints than that, once every one of the datapoints it does
//!   have breaches, because an alarm evaluated on the datapoints that are present cannot be
//!   asked for more of them than it has; it is in `OK` when fewer breach, and in
//!   `INSUFFICIENT_DATA` when no datapoint was evaluated at all;
//! * the `StateReason` ("An explanation for the alarm state, in text format.") explains how
//!   the state was decided: it NAMES the datapoints the evaluation graded — the value the
//!   statistic took over each window and the instant that window ends — beside the counts it
//!   made over them, and a window that held NO datapoint of the metric is reported as the hole
//!   it was, naming the missing-data arm that was applied to it, rather than being counted as a
//!   datapoint the evaluation graded — a client can tell an alarm that saw clean data from one
//!   that saw none.
//!
//! The evaluation is a pure function of the stored datapoints and the clock, and it writes
//! only when the state it computes differs from the state the alarm already reports — so
//! whichever call materialises it, every later caller sees the same thing. The first
//! evaluation of an alarm becomes due [`EVALUATION_DELAY_SECS`] after the write that
//! configured it ("When this operation creates an alarm, the alarm state is immediately set
//! to INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately." —
//! `PutMetricAlarm`), and a state `SetAlarmState` forced stands for one evaluation period
//! ([`forced_state_span_secs`]) before the evaluation takes the alarm back over.
//!
//! What is NOT evaluated here keeps the state it has, rather than being answered with a
//! state nothing computed: an alarm over a `Metrics` query array (metric math), one with
//! `EvaluationCriteria` (a PromQL query), one whose threshold is an anomaly-detection band
//! (`ThresholdMetricId`, or one of the `…LowerOrGreaterThanUpper…` comparisons) and one whose
//! configuration names no stored metric at all.

use ::serde_json::{Map, Value};

use super::alarm_actions::AlarmNotification;
use super::{
    dimension_set, to_timestamp, weighted_percentile, AlarmStateSnapshot, BasicStatistics,
    ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, MetricKey, StoredDatum,
    ALARM_STATE_ALARM, ALARM_STATE_INSUFFICIENT_DATA, ALARM_STATE_OK, ALARM_TYPE_METRIC,
    HISTORY_DATA_VERSION, MAX_STATE_REASON_DATA_LEN,
};
use crate::ops::types::{AlarmHistoryItem, MetricAlarm, STATISTIC_VALUES};

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

/// The evaluation period of an alarm that carries no `Period`: one minute, "the period, in
/// seconds, over which the statistic is applied" at its shortest regular resolution
/// (`PutMetricAlarmInput$Period`: "a period can be as short as one minute (60 seconds)").
const DEFAULT_PERIOD_SECS: f64 = 60.0;

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

/// How much wider than the look-back the EVALUATION RANGE is: the span of windows an
/// evaluation searches for the datapoints it evaluates.
///
/// CloudWatch "attempts to retrieve a higher number of data points than the number specified
/// as Evaluation Periods" and calls that span the evaluation range, so a datapoint just older
/// than the look-back is still evaluated rather than counted as missing — which is why a
/// series published a little slower than the alarm's period still decides its state. How much
/// wider the range is depends on the period and the resolution and is not published as a
/// number, so this factor is the emulator's own bounded choice: wide enough that a sparse
/// series is evaluated on what it really published, and a fixed multiple of the look-back so
/// the windows one evaluation folds stay bounded by it.
const EVALUATION_RANGE_FACTOR: usize = 2;

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

/// The `TreatMissingData` value under which an evaluation with no datapoint at all leaves the
/// alarm in the state it has: "the current alarm state is maintained". A datapoint that IS
/// present still decides the state.
const MISSING_IGNORE: &str = "ignore";

/// The `TreatMissingData` default: "If `TreatMissingData` is omitted, the default behavior of
/// `missing` is used" (`PutMetricAlarmInput$TreatMissingData`) — a missing datapoint is
/// neither breaching nor not breaching, so the alarm is evaluated on the datapoints it has.
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
/// part of any request's own latency; what it does not reach stays due for the next call.
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
/// answered from nothing.
#[derive(Debug)]
pub struct EvaluationBudget {
    /// How many more alarms this request may evaluate.
    alarms: usize,
    /// How many more stored datapoints this request's folds may visit.
    datapoints: usize,
    /// When this request stops evaluating, whatever is left of the counts above.
    deadline: std::time::Instant,
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
        }
    }

    /// Whether this request has spent its budget and must stop evaluating.
    #[must_use]
    pub fn spent(&self) -> bool {
        self.alarms == 0 || self.datapoints == 0 || std::time::Instant::now() >= self.deadline
    }

    /// Charge one alarm's evaluation.
    pub(super) fn charge_alarm(&mut self) {
        self.alarms = self.alarms.saturating_sub(1);
    }

    /// Charge the datapoints one fold is about to visit. Saturating: the fold that exhausts
    /// the budget still runs — it is bounded on its own by
    /// [`super::MAX_DATAPOINTS_PER_SERIES`] — and no further alarm is evaluated by this
    /// request.
    fn charge_datapoints(&mut self, datapoints: usize) {
        self.datapoints = self.datapoints.saturating_sub(datapoints);
    }
}

/// How long a state `SetAlarmState` forced on a metric alarm stands before the evaluation
/// takes the alarm back over.
///
/// "Temporarily sets the state of an alarm for testing purposes. […] Metric alarms return to
/// their actual state quickly" (`SetAlarmState`) — the thing that takes the alarm back is the
/// next evaluation, and an alarm is evaluated over its `Period` ("the period, in seconds,
/// over which the statistic is applied"), so that is the span: the forced state stands for
/// one evaluation period of the alarm it was forced on, and the evaluation owns the state
/// again from its end.
#[must_use]
pub fn forced_state_span_secs(alarm: &MetricAlarm) -> f64 {
    period_secs(alarm)
}

/// The alarm's evaluation period in seconds, defaulting to [`DEFAULT_PERIOD_SECS`] for an
/// alarm that carries none (a PromQL alarm, which has `EvaluationInterval` instead).
fn period_secs(alarm: &MetricAlarm) -> f64 {
    alarm
        .period
        .filter(|period| *period >= 1)
        .map_or(DEFAULT_PERIOD_SECS, f64::from)
}

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

/// One datapoint ONE evaluation evaluated.
#[derive(Debug, Clone, Copy)]
struct Evaluated {
    /// Which window of the evaluation range the datapoint came from, counting back from the
    /// evaluation instant; `None` for a slot no real datapoint filled.
    window: Option<usize>,
    /// The statistic over that window, for a real datapoint.
    value: Option<f64>,
    /// How it stands against the threshold.
    datapoint: Datapoint,
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
    /// Materialise every metric-alarm evaluation the clock has already made due, within what
    /// `budget` lets this request pay for.
    ///
    /// Alarms are visited in name order, so the pass is deterministic. Each alarm's
    /// evaluation reads only the metric store and the clock, so the order never changes the
    /// outcome, and an alarm the budget does not reach stays due for the next call.
    pub fn evaluate_metric_alarms(&mut self, now_secs: f64, budget: &mut EvaluationBudget) {
        let mut names: Vec<String> = self.alarms.keys().cloned().collect();
        names.sort();
        for name in names {
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
            return;
        };
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
        // The evaluation owns the alarm's state again: whatever a caller forced has been
        // taken back over.
        stored.forced_state_until_secs = None;
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
        // An alarm over a metric-math query array, a PromQL query or an anomaly-detection
        // band is not evaluated from one stored series, so nothing is computed for it.
        if alarm.metrics.is_some()
            || alarm.evaluation_criteria.is_some()
            || alarm.threshold_metric_id.is_some()
        {
            return None;
        }
        let threshold = alarm.threshold?;
        let breaches = breach_test(alarm.comparison_operator.as_deref()?)?;
        let statistic = AlarmStatistic::of(alarm)?;
        let period = f64::from(alarm.period.filter(|period| *period >= 1)?);
        let evaluation_periods = usize::try_from(alarm.evaluation_periods?)
            .ok()
            .filter(|periods| *periods >= 1)?
            .min(MAX_EVALUATED_PERIODS);
        let key = MetricKey {
            namespace: alarm.namespace.clone()?,
            metric_name: alarm.metric_name.clone()?,
            dimensions: dimension_set(alarm.dimensions.as_deref()),
        };

        // One pass over the series fills every window of the EVALUATION RANGE: window 0 is
        // the `Period` seconds ending at the evaluation instant, window i the one before it,
        // and the range reaches further back than the look-back ([`EVALUATION_RANGE_FACTOR`]).
        let range_periods = evaluation_periods.saturating_mul(EVALUATION_RANGE_FACTOR);
        let mut windows: Vec<Vec<&StoredDatum>> = vec![Vec::new(); range_periods];
        let series = self.metrics.get(&key);
        // The datapoints this fold is about to visit, charged before it visits them: this is
        // the factor that multiplies with the number of alarms one request evaluates.
        budget.charge_datapoints(series.map_or(0, Vec::len));
        for datum in series.into_iter().flatten() {
            // "If you specify a unit, the operation returns only data that was collected
            // with that unit specified." — the alarm's `Unit` selects which datapoints of the
            // series it watches.
            if alarm
                .unit
                .as_deref()
                .is_some_and(|unit| datum.unit.as_deref() != Some(unit))
            {
                continue;
            }
            // A datapoint may be stamped up to two hours ahead of now ("as much as 2 hours
            // after the current day and time", `PutMetricData`); it belongs to the newest
            // window, the one the evaluation instant falls in.
            let age = (now_secs - datum.timestamp_secs).max(0.0);
            let index = (age / period).floor();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = index as usize;
            if let Some(window) = windows.get_mut(index) {
                window.push(datum);
            }
        }

        let treat_missing = alarm.treat_missing_data.as_deref().unwrap_or(MISSING_MISSING);
        // The REAL datapoints of the range, newest first and at most `EvaluationPeriods` of
        // them: "all real data points in the evaluation range are included in the
        // evaluation", and where the range holds more than the alarm evaluates, it is the
        // most recent ones it evaluates.
        let mut evaluated: Vec<Evaluated> = Vec::with_capacity(evaluation_periods);
        for (index, window) in windows.iter().enumerate() {
            if evaluated.len() == evaluation_periods {
                break;
            }
            let Some(value) = statistic.over(window) else {
                continue;
            };
            evaluated.push(Evaluated {
                window: Some(index),
                value: Some(value),
                datapoint: if breaches(value, threshold) {
                    Datapoint::Breaching
                } else {
                    Datapoint::NotBreaching
                },
            });
        }
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
                datapoint: substituted,
            });
        }

        let breaching = evaluated
            .iter()
            .filter(|slot| slot.datapoint == Datapoint::Breaching)
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
        // An evaluation left with fewer datapoints than M — which only the treatments that
        // substitute NOTHING can leave it with — is decided by the datapoints it has: asking
        // for more breaching datapoints than the evaluation holds could never be satisfied,
        // and CloudWatch uses a missing datapoint "only as few times as possible".
        let required = to_alarm.min(present);

        let state = if present > 0 && breaching >= required {
            ALARM_STATE_ALARM
        } else if present == 0 {
            // Nothing was evaluated: the whole range was missing. "ignore — the current alarm
            // state is maintained", so under that setting this evaluation produces no state
            // at all and the alarm keeps the one it has; under every other setting it reports
            // that it had no data to decide on. A datapoint that IS present still decides,
            // whichever setting is in force — only missing data is what `TreatMissingData`
            // speaks about.
            if treat_missing == MISSING_IGNORE {
                return None;
            }
            ALARM_STATE_INSUFFICIENT_DATA
        } else {
            ALARM_STATE_OK
        };
        let counts = EvaluationCounts {
            breaching,
            present,
            substituted,
            periods: evaluated.len(),
            required,
        };
        Some(Evaluation {
            state,
            reason: state_reason(
                state,
                counts,
                missing_data_arm(treat_missing),
                alarm.comparison_operator.as_deref().unwrap_or_default(),
                threshold,
                &cited_datapoints(&evaluated, now_secs, period),
            ),
            reason_data: state_reason_data(&statistic, period, threshold, now_secs, &evaluated),
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

/// The comparison one `ComparisonOperator` makes between a datapoint and the threshold, or
/// `None` for the anomaly-detection band operators, which compare against a model this
/// engine does not compute rather than against a static threshold.
fn breach_test(comparison: &str) -> Option<fn(f64, f64) -> bool> {
    // The static-threshold members of the modelled `ComparisonOperator` enum
    // (`smithy_query(query="shape",shape="ComparisonOperator")`).
    match comparison {
        "GreaterThanOrEqualToThreshold" => Some(|value, threshold| value >= threshold),
        "GreaterThanThreshold" => Some(|value, threshold| value > threshold),
        "LessThanThreshold" => Some(|value, threshold| value < threshold),
        "LessThanOrEqualToThreshold" => Some(|value, threshold| value <= threshold),
        // "LessThanLowerOrGreaterThanUpperThreshold", "LessThanLowerThreshold" and
        // "GreaterThanUpperThreshold" compare against an anomaly detection band.
        _ => None,
    }
}

/// The statistic one alarm aggregates each of its windows with.
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
    /// Datapoints the evaluation really had to decide on: the real ones plus the ones the
    /// missing-data treatment substituted.
    present: usize,
    /// How many of those were SUBSTITUTED for a window that held no datapoint of the metric.
    substituted: usize,
    /// How many windows the look-back covered — the alarm's `EvaluationPeriods`.
    periods: usize,
    /// How many breaching datapoints this evaluation needed to put the alarm in `ALARM`.
    required: usize,
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

/// The `StateReason` an evaluated state reports: the DATAPOINTS the evaluation graded, what it
/// counted over them against what it needed — and, where a window held no datapoint of the
/// metric at all, that the window was EMPTY and which missing-data arm was applied to it.
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
/// four counts, an operator name of at most 41 characters, one number, and a datapoint list
/// bounded by [`MAX_CITED_DATAPOINT_CHARS`].
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
        present,
        substituted,
        periods,
        required,
    } = counts;
    let threshold = decimal_text(threshold);
    if state == ALARM_STATE_INSUFFICIENT_DATA {
        // No datapoint was evaluated and the treatment substituted none: the evaluation had
        // nothing to decide on.
        let (plural_suffix, verb) = plural(periods);
        return format!("Insufficient Data: {periods} datapoint{plural_suffix} {verb} unknown.");
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
        return format!(
            "Threshold Crossed: {breaching} out of the last {present} datapoints{cited} \
             satisfied {comparison} with the threshold ({threshold}); {required} datapoints \
             must for the alarm to be in ALARM state.{substituted_clause}"
        );
    }
    // The alarm is in OK, so the comparison did not hold often enough.
    format!(
        "Threshold Crossed: {breaching} out of the last {present} datapoints{cited} satisfied \
         {comparison} with the threshold ({threshold}), fewer than the {required} datapoints \
         required for the alarm to be in ALARM state.{substituted_clause}"
    )
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
/// its window and the instant that window ends: ` [137.0 (2026-10-25T02:02:00Z), …]`.
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
        let entry = match super::rfc3339(window_end(now_secs, index, period)) {
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

/// The instant the `index`th window back from the evaluation instant ENDS: window 0 ends at
/// the evaluation instant itself, and each window before it a `period` earlier.
fn window_end(now_secs: f64, index: usize, period: f64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let windows_back = index as f64;
    now_secs - windows_back * period
}

/// One measured quantity as an explanation names it: a decimal that always carries a fraction,
/// so a value and a threshold read as the measurements they are rather than as counts.
fn decimal_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        return format!("{value:.1}");
    }
    format!("{value}")
}

/// The `StateReasonData` an evaluated state reports: the query the evaluation made and the
/// datapoints it evaluated, newest first, each with the end of the window it came from — and
/// no window at all for a slot the missing-data treatment filled.
///
/// "An explanation for the alarm state, in JSON format." (`MetricAlarm$StateReasonData`),
/// bounded by [`MAX_STATE_REASON_DATA_LEN`] — a document is JSON, so it can never be
/// truncated to fit: the datapoint list is dropped instead when an alarm evaluates more
/// windows than the bound holds.
fn state_reason_data(
    statistic: &AlarmStatistic<'_>,
    period: f64,
    threshold: f64,
    now_secs: f64,
    evaluated: &[Evaluated],
) -> String {
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(HISTORY_DATA_VERSION));
    document.insert("queryDateSeconds".to_string(), Value::from(now_secs));
    let member = if statistic.is_percentile() {
        "extendedStatistic"
    } else {
        "statistic"
    };
    document.insert(member.to_string(), Value::from(statistic.name()));
    document.insert("period".to_string(), Value::from(period));
    document.insert("threshold".to_string(), Value::from(threshold));
    // The values the evaluation really had, oldest first — the order they were observed in.
    // `evaluated` is newest first and holds a slot for every period the alarm evaluates, so
    // the slots a missing-data treatment filled contribute no value here: this list reports
    // the datapoints, and `evaluatedDatapoints` below reports the periods.
    let recent: Vec<Value> = evaluated
        .iter()
        .rev()
        .filter_map(|slot| slot.value.map(Value::from))
        .collect();
    document.insert("recentDatapoints".to_string(), Value::Array(recent));

    let datapoints: Vec<Value> = evaluated
        .iter()
        .map(|slot| {
            let mut window = Map::new();
            if let Some(index) = slot.window {
                window.insert(
                    "windowEndSeconds".to_string(),
                    Value::from(window_end(now_secs, index, period)),
                );
            }
            if let Some(value) = slot.value {
                window.insert("value".to_string(), Value::from(value));
            }
            window.insert(
                "datapoint".to_string(),
                Value::from(match slot.datapoint {
                    Datapoint::Breaching => "breaching",
                    Datapoint::NotBreaching => "notBreaching",
                    Datapoint::Missing => "missing",
                }),
            );
            Value::Object(window)
        })
        .collect();
    let mut full = document.clone();
    full.insert("evaluatedDatapoints".to_string(), Value::Array(datapoints));
    let rendered = Value::Object(full).to_string();
    if rendered.len() <= MAX_STATE_REASON_DATA_LEN {
        return rendered;
    }
    Value::Object(document).to_string()
}

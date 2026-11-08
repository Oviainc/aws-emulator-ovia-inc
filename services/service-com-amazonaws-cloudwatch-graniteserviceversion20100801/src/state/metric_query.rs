// SPDX-License-Identifier: CC0-1.0

//! A `MetricDataQuery` ARRAY, resolved over the series this store holds.
//!
//! "Use the `Metrics` parameter to create an alarm based on a metric math expression" — the
//! same array `GetMetricData` answers from (`GetMetricDataInput$MetricDataQueries`) is what a
//! metric alarm's configuration can BE (`PutMetricAlarmInput$Metrics`), so both readers
//! resolve it here: one query naming a metric is folded out of the store by its `Stat`, and one
//! carrying an `Expression` is evaluated by [`super::metric_math`] over the values of the
//! queries it refers to, which are resolved first.
//!
//! Resolving is driven by the CALLER, one query at a time ([`Resolution`]): the operation that
//! answers a request holds the store's lock for each query only and yields between them, while
//! the alarm evaluator resolves under the lock it already holds. So the two cannot disagree
//! about what an array means, and neither dictates the other's locking.

use std::collections::{HashMap, HashSet};

use super::metric_math::{
    AnomalyModel, Evaluation, Expression, MathError, MathScope, MathSeries, MathValue,
    SearchResult, SeriesMetric, SeriesSource,
};
use super::{ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, MetricKey};
use crate::ops::types::{MetricDataQuery, MetricStat};

/// Why one query array does not resolve over this store.
///
/// What each reader DOES with a refusal is its own: `GetMetricData` reports it as the
/// parameter fault or the expression refusal CloudWatch answers that request with, and an
/// alarm whose configuration does not resolve is left in the state it has rather than being
/// given one nothing computed.
#[derive(Debug, Clone)]
pub enum QueryRefusal {
    /// "The value of an input parameter is bad or out-of-range." — the message naming it.
    InvalidValue(String),
    /// Members of the request that cannot be combined — the message naming them.
    InvalidCombination(String),
    /// An expression this service does not evaluate: the `Id` of the query carrying it, and
    /// what is wrong with the expression.
    Expression {
        /// The `Id` of the query whose expression it is.
        id: String,
        /// What is wrong with the expression, in the words the refusal reports.
        detail: String,
    },
    /// Expressions that refer to each other in a circle, in the order they do, so no value of
    /// theirs can be computed.
    Circular(Vec<String>),
    /// A Metrics Insights query — a `SELECT` statement in its own language — which this
    /// service does not run.
    MetricsInsights,
}

/// Why the ids of the query array `queries` are not a set of ids one array may carry, or
/// `None` when they are.
///
/// Two rules, both the member's own, and both about the SHAPE of the request rather than about
/// any value a query names — so both are judged before a query's parameters are looked at, by
/// whichever operation the array arrived on:
///
/// * "The valid characters are letters, numbers, and underscore. The first character must be a
///   lowercase letter." (`MetricDataQuery$Id`) — the grammar
///   ([`super::METRIC_QUERY_ID_PATTERN`]), which no modelled constraint expresses, stated to a
///   client as the expression it can re-validate against;
/// * "This name must be unique within a single call" (`MetricDataQuery$Id`) — every result is
///   reported under the id of the query it answers, so an array using one id twice reports
///   nothing a caller can tie back to a query.
///
/// `array` is the request member the queries arrived on, which is what the refusal names: the
/// same array shape is `GetMetricDataInput$MetricDataQueries` on a read and
/// `PutMetricAlarmInput$Metrics` on an alarm.
pub fn query_id_refusal(queries: &[MetricDataQuery], array: &str) -> Option<String> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(queries.len());
    let mut unique = true;
    for (index, query) in queries.iter().enumerate() {
        if !super::is_metric_query_id(&query.id) {
            return Some(super::value_pattern_message(
                &format!("{}.Id", super::list_element_path(array, index + 1)),
                super::METRIC_QUERY_ID_PATTERN,
            ));
        }
        unique &= seen.insert(query.id.as_str());
    }
    // The refusal names no position: it is the ARRAY's ids that are not unique, and either of
    // the queries sharing one id may be the one the caller meant to change.
    (!unique).then(|| super::values_not_unique_message(super::QUERY_ID_MEMBER, array))
}

/// The granularity an expression's constructed series takes when nothing names one: one
/// minute, the cadence a regular-resolution metric is stored at ("a period can be as short as
/// one minute (60 seconds)", `MetricDataQuery$Period`).
pub const DEFAULT_EXPRESSION_PERIOD: f64 = 60.0;

/// How many data points ONE resolution may retrieve: "A GetMetricData operation that does not
/// include a query can retrieve as many as 500 different metrics in a single request, with a
/// total of as many as 100,800 data points." (`GetMetricData`)
const MAX_RETRIEVED_POINTS: usize = 100_800;

/// How many queries ONE resolution resolves: "A single GetMetricData call can include up to
/// 500 MetricDataQuery structures." (`GetMetricData`)
///
/// It bounds the whole resolution, not just a request: the array is parsed, ordered and folded
/// per query, so this is the factor every one of those walks multiplies with — including the
/// depth of the reference graph [`evaluation_order`] follows, which cannot exceed the number of
/// queries the array defines.
pub const MAX_RESOLVED_QUERIES: usize = 500;

/// How many series one search answers with at most: "A GetMetricData operation that does not
/// include a query can retrieve as many as 500 different metrics in a single request."
const MAX_SEARCH_SERIES: usize = 500;

/// The refusal for a resolution whose retrieval crosses [`MAX_RETRIEVED_POINTS`] as it runs.
fn too_many_points() -> QueryRefusal {
    QueryRefusal::InvalidCombination(format!(
        "the queries of this request retrieve more than the {MAX_RETRIEVED_POINTS} data \
         points one call can retrieve; narrow the time range or widen the Period"
    ))
}

/// Why this array must not be resolved over `[start, end)` AT ALL, or `None` when resolving it
/// is work one resolution may do.
///
/// Checked BEFORE the first fold, because every factor of that work is the caller's: how many
/// queries the array carries, how wide the range is, and how narrow each retrieving query's
/// `Period` is. Each of those queries asks for one datapoint per `Period` of the range, which
/// is what CloudWatch calls the datapoints the request HAS REQUESTED — an operation that
/// answers for them up to the published total ("a total of as many as 100,800 data points",
/// `GetMetricData`) and refuses past it is refused here, naming how many were asked for and the
/// two ways to ask for fewer, rather than materialising them and then trimming to the page the
/// caller reads. An operation that PAGES the range instead counts the same datapoints
/// ([`requested_datapoints`]) and slices rather than refusing.
#[must_use]
pub fn resolution_refusal(
    queries: &[MetricDataQuery],
    start: f64,
    end: f64,
) -> Option<QueryRefusal> {
    if queries.len() > MAX_RESOLVED_QUERIES {
        return Some(too_many_queries(queries.len()));
    }
    let requested = requested_datapoints(queries, start, end);
    if requested > MAX_RETRIEVED_POINTS {
        return Some(QueryRefusal::InvalidCombination(format!(
            "You have requested up to {requested} datapoints, which exceeds the limit of \
             {MAX_RETRIEVED_POINTS}. You may reduce the datapoints requested by increasing \
             Period, or decreasing the time range."
        )));
    }
    None
}

/// How many datapoints an array asks for over `[start, end)`: one per `Period` of the range for
/// each query that RETRIEVES a metric, summed over those queries.
///
/// A query that retrieves no metric requests no datapoint: an expression's own series is
/// computed from the values of the queries it reads, bounded by the evaluator's budget.
#[must_use]
pub fn requested_datapoints(queries: &[MetricDataQuery], start: f64, end: f64) -> usize {
    let span = (end - start).max(0.0);
    let mut requested = 0usize;
    for query in queries {
        if query.metric_stat.is_none() {
            continue;
        }
        let period = query_period(query).max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let asked = (span / period).ceil().min(f64::from(u32::MAX)) as usize;
        requested = requested.saturating_add(asked);
    }
    requested
}

/// The granularity ONE query is folded at: its own `Period`, else the `Period` of the
/// `MetricStat` it retrieves, else the default an expression's constructed series takes
/// ([`DEFAULT_EXPRESSION_PERIOD`]).
#[must_use]
pub fn query_period(query: &MetricDataQuery) -> f64 {
    query
        .period
        .or_else(|| query.metric_stat.as_ref().map(|stat| stat.period))
        .filter(|period| *period >= 1)
        .map_or(DEFAULT_EXPRESSION_PERIOD, f64::from)
}

/// The identity of the series ONE `MetricStat` query names, or `None` when it names only half
/// of one: "a metric is uniquely defined by a name, a namespace, and zero or more dimensions",
/// so a query missing either half names no series at all.
///
/// The one place a query becomes a series identity, so the fold that answers it, the budget
/// that pays for it and the alarm that watches it all resolve it the same way.
#[must_use]
pub fn query_series_key(stat: &MetricStat) -> Option<MetricKey> {
    Some(MetricKey {
        namespace: stat.metric.namespace.clone()?,
        metric_name: stat.metric.metric_name.clone()?,
        dimensions: super::dimension_set(stat.metric.dimensions.as_deref()),
    })
}

/// Answer one `MetricStat` query from the store: the datapoints of the metric it names, inside
/// `[requested_start, end)`, folded into `Period` windows by its `Stat`.
///
/// A window holding no datapoint is not reported — CloudWatch returns a value only for the
/// periods that have data — so the two lists a result carries stay in step.
pub fn retrieved_series(
    store: &State,
    query: &MetricDataQuery,
    stat: &MetricStat,
    requested_start: f64,
    end: f64,
    now: f64,
) -> Result<MathSeries, QueryRefusal> {
    // "MetricStat" names ONE metric, and "a metric is uniquely defined by a name, a
    // namespace, and zero or more dimensions", so both halves of that identity are required
    // for the query to name a series at all.
    let Some(key) = query_series_key(stat) else {
        return Err(QueryRefusal::InvalidCombination(format!(
            "the query {} must name both MetricStat.Metric.Namespace and \
             MetricStat.Metric.MetricName",
            query.id
        )));
    };

    // "The statistic to return. It can include any CloudWatch statistic or extended
    // statistic." (`MetricStat$Stat`) — an unconstrained string in the model, so no boundary
    // validator refuses a name outside the statistic grammar before a handler: a name that is
    // not a statistic at all is a bad parameter value.
    if !super::is_cloudwatch_statistic(&stat.stat) {
        return Err(QueryRefusal::InvalidValue(format!(
            "{} is not a supported statistic",
            stat.stat
        )));
    }

    // "When used in GetMetricData, [Period] is the granularity, in seconds, of the returned
    // data points": the query's own `Period` member overrides the one on its `MetricStat`.
    let period_secs = query.period.unwrap_or(stat.period);
    if let Some(message) = super::period_error(period_secs) {
        return Err(QueryRefusal::InvalidValue(message));
    }

    let points = folded_series(
        store,
        &key,
        &stat.stat,
        stat.unit.as_deref(),
        period_secs,
        requested_start,
        end,
        now,
    );

    // "If Label is omitted, CloudWatch generates a default": the metric the query names,
    // which is the label every read of one series reports it under. The label the REQUEST
    // names is applied by the reader that answers the request, not here: it may be a dynamic
    // label, and `${LABEL}` inside one stands for exactly this default.
    let label = key.metric_name.clone();

    // The series still IS this metric's series, which is what an anomaly detector is created
    // for, so the identity it was retrieved under travels with it.
    Ok(MathSeries::new(label, points).of_metric(SeriesMetric {
        namespace: key.namespace,
        metric_name: key.metric_name,
        dimensions: key.dimensions,
        stat: stat.stat.clone(),
    }))
}

/// One stored series folded into the `period_secs`-wide windows of `[requested_start, end)` by
/// `statistic`.
///
/// The one place a series becomes a list of points, so a query naming a metric, a search
/// discovering one and an alarm grading one are answered from the same fold, at the same window
/// boundaries.
#[allow(clippy::too_many_arguments)]
pub fn folded_series(
    store: &State,
    key: &MetricKey,
    statistic: &str,
    unit: Option<&str>,
    period_secs: i32,
    requested_start: f64,
    end: f64,
    now: f64,
) -> Vec<(f64, f64)> {
    let period = f64::from(period_secs);
    // The granularity a RANGE can be read at is the one its start time's retention tier still
    // holds — and a start reaching back before the horizon any tier ends at is read from that
    // horizon ([`super::granularity_available_from`]), so a wide window is answered with the
    // history there is rather than with nothing. Which datapoints INSIDE the range a read then
    // sees is decided per datapoint, by its own age, in the fold below
    // ([`super::retained_at_period`]).
    if !super::granularity_available_from(requested_start, period_secs, now) {
        return Vec::new();
    }
    let start = super::rounded_start(requested_start, period_secs, now);
    // `StartTime` inclusive, `EndTime` exclusive; windows are `Period` seconds wide and
    // anchored at the rounded start time, folded by the one shared helper every reader of a
    // series uses ([`super::windows_of`]).
    let stored = store.metrics.get(key).map_or(&[][..], Vec::as_slice);
    let windows = super::windows_of(stored, start, end, period_secs, unit, now);

    let mut points: Vec<(f64, f64)> = Vec::with_capacity(windows.len());
    for (index, data) in windows {
        // A statistic the span does not define — a percentile over an opaque aggregate, say —
        // reports no value for that window rather than an invented one.
        let Some(value) = super::statistic_value(&data, statistic) else {
            continue;
        };
        points.push((super::window_start(start, index, period), value));
    }
    points
}

/// What one resolution has resolved so far: the value of every query, the series it retrieved,
/// and what it has spent.
///
/// A `MetricStat` query is folded out of the store ([`retrieved_series`]) and recorded with
/// [`Resolution::retrieve`]; an `Expression` query is evaluated over the values of the queries
/// it refers to ([`Resolution::evaluate`]) and recorded with [`Resolution::record`]. Both
/// charge the work done, so the datapoints one resolution reads are bounded whatever the
/// queries and the expressions in it nest.
#[derive(Debug, Default)]
pub struct Resolution {
    /// The value of every query resolved so far, by its `Id`.
    values: HashMap<String, MathValue>,
    /// The series this resolution RETRIEVES, in request order, each beside the metric name a
    /// `METRICS()` filter matches against — what `METRICS()` answers with.
    retrieved: Vec<(String, MathSeries)>,
    /// How many points an expression's arithmetic could not produce a number for, by query.
    dropped: HashMap<String, usize>,
    /// How many data points this resolution has RETRIEVED, against [`MAX_RETRIEVED_POINTS`].
    points: usize,
    /// How much of the evaluation budget this resolution's expressions have spent, against the
    /// evaluator's own ceiling.
    spent: usize,
}

/// One expression's answer: its value and the points its arithmetic dropped.
pub struct Evaluated {
    /// The value the expression took.
    value: MathValue,
    /// How many points its arithmetic could not produce a number for.
    dropped: usize,
    /// How much of the resolution's evaluation budget is spent once it is evaluated.
    spent: usize,
}

impl Resolution {
    /// Record what one `MetricStat` query retrieved, refusing a resolution that retrieves more
    /// data points than one may.
    pub fn retrieve(
        &mut self,
        query: &MetricDataQuery,
        series: MathSeries,
    ) -> Result<(), QueryRefusal> {
        self.points = self.points.saturating_add(series.points.len());
        if self.points > MAX_RETRIEVED_POINTS {
            return Err(too_many_points());
        }
        let metric_name = query
            .metric_stat
            .as_ref()
            .and_then(|stat| stat.metric.metric_name.clone())
            .unwrap_or_default();
        self.retrieved.push((metric_name, series.clone()));
        self.values
            .insert(query.id.clone(), MathValue::Series(series));
        Ok(())
    }

    /// Evaluate the expression of the query `id` over what this resolution holds, at the
    /// granularity `period` and over the range `window`.
    pub fn evaluate(
        &self,
        id: &str,
        tree: &Expression,
        source: &dyn SeriesSource,
        window: (f64, f64),
        period: f64,
    ) -> Result<Evaluated, QueryRefusal> {
        let scope = MathScope {
            resolved: &self.values,
            retrieved: &self.retrieved,
            source,
            start: window.0,
            end: window.1,
            period,
        };
        // The budget carries over from the expressions already evaluated, so it bounds the
        // whole resolution rather than each of its expressions.
        let mut evaluation = Evaluation::resumed(&scope, self.spent);
        let value = evaluation.value(tree).map_err(|error| QueryRefusal::Expression {
            id: id.to_string(),
            detail: error.message,
        })?;
        Ok(Evaluated {
            value,
            dropped: evaluation.dropped(),
            spent: evaluation.spent(),
        })
    }

    /// Keep what one expression evaluated to.
    pub fn record(&mut self, id: &str, evaluated: Evaluated) {
        self.values.insert(id.to_string(), evaluated.value);
        self.dropped.insert(id.to_string(), evaluated.dropped);
        self.spent = evaluated.spent;
    }

    /// The value the query `id` resolved to, or `None` for an id this resolution did not
    /// resolve.
    #[must_use]
    pub fn value(&self, id: &str) -> Option<&MathValue> {
        self.values.get(id)
    }

    /// How many points the arithmetic of the query `id` could not produce a number for.
    #[must_use]
    pub fn dropped(&self, id: &str) -> usize {
        self.dropped.get(id).copied().unwrap_or_default()
    }
}

/// Every expression query of `queries`, parsed, by its `Id`.
///
/// Parsing them all before any is evaluated is what refuses an array carrying one malformed
/// expression without evaluating the others.
/// Every expression query of `queries`, parsed, by its `Id`.
///
/// Parsing them all before any is evaluated is what refuses an array carrying one malformed
/// expression without evaluating the others. The array itself is bounded first
/// ([`MAX_RESOLVED_QUERIES`]): the parse is per query, so how much of it one caller can ask for
/// is the length of the array it submitted.
pub fn parse_queries(
    queries: &[MetricDataQuery],
) -> Result<HashMap<&str, Expression>, QueryRefusal> {
    if queries.len() > MAX_RESOLVED_QUERIES {
        return Err(too_many_queries(queries.len()));
    }
    let mut parsed: HashMap<&str, Expression> = HashMap::new();
    for query in queries {
        let Some(expression) = query.expression.as_deref() else {
            continue;
        };
        // "This field can contain either a Metrics Insights query, or a metric math
        // expression": an Insights query is a SELECT statement in its own language, which
        // this service does not run.
        if super::metric_math::is_metrics_insights_query(expression) {
            return Err(QueryRefusal::MetricsInsights);
        }
        let tree =
            super::metric_math::parse(expression).map_err(|error| QueryRefusal::Expression {
                id: query.id.clone(),
                detail: error.message,
            })?;
        parsed.insert(query.id.as_str(), tree);
    }
    Ok(parsed)
}

/// The refusal for an array carrying more queries than one resolution resolves, in the words
/// the limit is published in.
fn too_many_queries(queries: usize) -> QueryRefusal {
    QueryRefusal::InvalidValue(format!(
        "MetricDataQueries carries {queries} queries; one call can include up to \
         {MAX_RESOLVED_QUERIES}"
    ))
}

/// The order the array's expressions are evaluated in: every expression after the ones it
/// refers to.
///
/// "A math expression can use the `Id` of the other metrics or queries to refer to those
/// metrics, and can also use the `Id` of other expressions to use the result of those
/// expressions." (`MetricDataQuery$Expression`) — so the references form a graph, and one that
/// goes in a circle names a value nothing can compute.
pub fn evaluation_order<'a>(
    queries: &'a [MetricDataQuery],
    parsed: &HashMap<&'a str, Expression>,
) -> Result<Vec<&'a str>, QueryRefusal> {
    if queries.len() > MAX_RESOLVED_QUERIES {
        return Err(too_many_queries(queries.len()));
    }
    let mut order: Vec<&'a str> = Vec::with_capacity(parsed.len());
    let mut placed: HashSet<&'a str> = HashSet::with_capacity(parsed.len());
    for query in queries {
        place(query.id.as_str(), parsed, &mut order, &mut placed)?;
    }
    Ok(order)
}

/// One id of the walk: the query being placed, and the references of its expression still to
/// visit, in the order the expression names them (the walk takes them off the end).
struct Pending<'a> {
    /// The `Id` this frame places.
    id: &'a str,
    /// The ids its expression refers to that the walk has not visited yet, reversed.
    references: Vec<String>,
}

impl<'a> Pending<'a> {
    /// The frame that places `id`, whose expression is `tree`.
    fn of(id: &'a str, tree: &Expression) -> Self {
        let mut references = super::metric_math::references(tree);
        references.reverse();
        Self { id, references }
    }
}

/// Place `id` after everything it refers to, unless it is placed already.
///
/// The walk keeps its own stack ON THE HEAP rather than recursing: how deep a chain of
/// expressions referring to expressions goes is the CALLER's choice, so the depth is bounded
/// explicitly ([`MAX_RESOLVED_QUERIES`] — a cycle-free chain cannot be longer than the number
/// of queries the array defines) and a graph past that is refused instead of growing this
/// thread's stack.
fn place<'a>(
    id: &'a str,
    parsed: &HashMap<&'a str, Expression>,
    order: &mut Vec<&'a str>,
    placed: &mut HashSet<&'a str>,
) -> Result<(), QueryRefusal> {
    if placed.contains(id) {
        return Ok(());
    }
    // An id no expression query of this array defines is either a retrieving query (already
    // resolved) or an id nothing defines, which the evaluator refuses where the reference is
    // read rather than here.
    let Some((root, tree)) = parsed.get_key_value(id) else {
        return Ok(());
    };
    let mut open: Vec<Pending<'a>> = vec![Pending::of(root, tree)];
    while !open.is_empty() {
        let next = open
            .last_mut()
            .and_then(|pending| pending.references.pop());
        let Some(reference) = next else {
            // Everything this id refers to is placed, so it is placed after them.
            if let Some(pending) = open.pop() {
                placed.insert(pending.id);
                order.push(pending.id);
            }
            continue;
        };
        // The reference names one of THIS array's ids, so the borrow the walk keeps is the
        // array's own key rather than the string the walk built.
        let Some((key, tree)) = parsed.get_key_value(reference.as_str()) else {
            continue;
        };
        let key = *key;
        if placed.contains(key) {
            continue;
        }
        if let Some(opened) = open.iter().position(|pending| pending.id == key) {
            // The cycle itself, as the refusal names it: the ids from the one that closed back
            // on itself, in the order the expressions refer to each other, and back to it.
            let mut cycle: Vec<String> = open
                .get(opened..)
                .unwrap_or_default()
                .iter()
                .map(|pending| pending.id.to_string())
                .collect();
            cycle.push(key.to_string());
            return Err(QueryRefusal::Circular(cycle));
        }
        if open.len() >= MAX_RESOLVED_QUERIES {
            return Err(QueryRefusal::Expression {
                id: key.to_string(),
                detail: format!(
                    "the expressions of this request refer to each other more deeply than the \
                     {MAX_RESOLVED_QUERIES} queries one call can include"
                ),
            });
        }
        open.push(Pending::of(key, tree));
    }
    Ok(())
}

/// The store's answer to a `SEARCH`: every series it holds that the query matches, folded by
/// the search's statistic into its period's windows.
pub struct StoreSeriesSource<'a> {
    /// The series this resolution reads.
    pub store: &'a State,
    /// The start of the range, before it is rounded to the period's boundary.
    pub requested_start: f64,
    /// The first instant after the range.
    pub end: f64,
    /// The instant the resolution is made at, which is what the start-time rounding is
    /// measured from.
    pub now: f64,
}

impl SeriesSource for StoreSeriesSource<'_> {
    fn search(&self, query: &str, stat: &str, period: f64) -> Result<SearchResult, MathError> {
        let parsed = super::metric_math::parse_search_query(query)?;
        if !super::is_cloudwatch_statistic(stat) {
            return Err(MathError {
                message: format!("{stat} is not a supported statistic"),
            });
        }
        #[allow(clippy::cast_possible_truncation)]
        let period_secs = period.trunc() as i32;
        if let Some(message) = super::period_error(period_secs) {
            return Err(MathError { message });
        }

        // The matched series are answered in a stable order — a search names no order of its
        // own, and the store is a hash map, so the keys are sorted before anything is folded.
        let mut matched: Vec<&MetricKey> = self
            .store
            .metrics
            .keys()
            .filter(|key| parsed.matches(&key.namespace, &key.metric_name, &key.dimensions))
            .collect();
        matched.sort();
        matched.truncate(MAX_SEARCH_SERIES);

        Ok(SearchResult {
            series: matched
                .into_iter()
                .map(|key| {
                    MathSeries::new(
                        search_label(key),
                        folded_series(
                            self.store,
                            key,
                            stat,
                            None,
                            period_secs,
                            self.requested_start,
                            self.end,
                            self.now,
                        ),
                    )
                    // A discovered series is still that metric's series, so a band over the
                    // one series a search found is drawn from its detector's model.
                    .of_metric(SeriesMetric {
                        namespace: key.namespace.clone(),
                        metric_name: key.metric_name.clone(),
                        dimensions: key.dimensions.clone(),
                        stat: stat.to_string(),
                    })
                })
                .collect(),
            // What the search COST its caller: every series of the store was matched against
            // the query, whether or not it matched, and the walk charges that to the
            // expression's budget.
            scanned: self.store.metrics.len(),
        })
    }

    fn anomaly_model(&self, series: &MathSeries) -> Option<AnomalyModel> {
        // The model is the DETECTOR's: only a detector created for this metric and statistic
        // trained one, so the metric the series was retrieved for is what the lookup is keyed
        // on, under the identity the write that stored the detector was keyed by. A series
        // computed from others is no metric's series and no detector's.
        let metric = series.metric.as_ref()?;
        let detector_id = super::AnomalyDetectorForm::of_series_metric(metric).detector_id();
        let stored = self.store.anomaly_detectors.get(&detector_id)?;
        // What the model was TRAINED on is the metric's own history and not the span this read
        // happens to cover ([`anomaly_training_history`]), so the band a graph shows over one
        // window and the band an alarm compares one period against are the same numbers.
        let history = anomaly_training_history(self.store, metric, self.now);
        super::anomaly_model_of(stored, &history, &series.points, self.now)
    }

    fn anomaly_training_datapoints(&self, series: &MathSeries) -> usize {
        // What the fold above will VISIT: the datapoints the metric's own series holds, which
        // is the same count [`Resolution::retrieve`] and the alarm evaluator charge a fold of
        // one series by. A series no detector was created for is never folded, so it costs
        // nothing.
        let Some(metric) = series.metric.as_ref() else {
            return 0;
        };
        let detector_id = super::AnomalyDetectorForm::of_series_metric(metric).detector_id();
        if !self.store.anomaly_detectors.contains_key(&detector_id) {
            return 0;
        }
        self.store
            .metrics
            .get(&training_series_key(metric))
            .map_or(0, Vec::len)
    }
}

/// The series identity a model's training history is folded out of: the metric the detector was
/// created for.
fn training_series_key(metric: &SeriesMetric) -> MetricKey {
    MetricKey {
        namespace: metric.namespace.clone(),
        metric_name: metric.metric_name.clone(),
        dimensions: metric.dimensions.clone(),
    }
}

/// The history an anomaly detection model is trained on: the series its detector watches,
/// folded by the statistic the detector was created for, over the training span that ends at
/// `now` ([`super::ANOMALY_TRAINING_SPAN_SECS`]).
///
/// Folded by the SAME fold every reader of a series uses ([`folded_series`]), so a model is
/// trained on exactly the values a caller reading that metric would see, and what the walk
/// costs is the datapoints the series holds rather than the span — the count its caller charges
/// to its budget before this runs ([`SeriesSource::anomaly_training_datapoints`]).
fn anomaly_training_history(store: &State, metric: &SeriesMetric, now: f64) -> Vec<(f64, f64)> {
    let key = training_series_key(metric);
    let period = super::ANOMALY_TRAINING_PERIOD_SECS;
    folded_series(
        store,
        &key,
        &metric.stat,
        None,
        period,
        now - super::ANOMALY_TRAINING_SPAN_SECS,
        // The period the clock is IN is part of the history too: a datapoint just published is
        // one the metric's history holds, so the fold reaches the end of that period.
        now + f64::from(period),
        now,
    )
}

/// The label a searched series is reported under: the metric's name and the values of its
/// dimensions, which is what tells two series of one metric apart.
fn search_label(key: &MetricKey) -> String {
    let mut label = key.metric_name.clone();
    for value in key.dimensions.values() {
        label.push(' ');
        label.push_str(value);
    }
    label
}

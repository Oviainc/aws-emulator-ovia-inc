// SPDX-License-Identifier: CC0-1.0

//! The PromQL an instant query carries: parsed with `promql-parser` and evaluated against
//! the samples the workspace HOLDS.
//!
//! The expression is the caller's own query language, so it is PARSED — by
//! `promql-parser`, the Rust implementation of Prometheus's own grammar — and evaluated
//! from the syntax tree it returns. Nothing here reads the query's text: a matcher is
//! applied as the grammar built it, which is why a label matcher's regular expression, its
//! `or` alternatives and its operators behave as PromQL defines them rather than as a
//! hand-written reading of the string would.
//!
//! What this plane evaluates is stated once, here, and reported honestly for anything
//! else ([`QueryError::Unsupported`]):
//!
//! * an instant vector selector — a metric name, label matchers, or both;
//! * one of the parameterless aggregation operators over such a selector, with `by` or
//!   `without`;
//! * a number literal, which is PromQL's scalar.
//!
//! A range selector, a function call, a binary operation, a subquery and the two
//! parameterised aggregations (`quantile`, `topk`/`bottomk`) are parsed and then reported
//! as expressions this emulator does not evaluate — never answered with a result nothing
//! computed.
//!
//! ## What bounds a query
//!
//! The expression is caller-supplied text and the samples it is evaluated against are
//! caller-supplied too, so the cost of a query is bounded at both ends, and the bounds are
//! this process's own:
//!
//! * its SIZE ([`MAX_QUERY_BYTES`]), which is what makes the parse itself bounded: at the
//!   two bytes a nesting level costs at least, no admissible expression names a tree
//!   deeper than [`MAX_PARSED_DEPTH`], however it nests — brackets, an operator chain or
//!   repeated unary operators alike;
//! * its DEPTH for planning ([`MAX_NESTING_DEPTH`]), measured on the parsed tree by an
//!   iterative walk, so no caller-chosen depth is walked with the call stack;
//! * the number of label matchers the plan carries ([`MAX_MATCHERS`]), which caps the
//!   PRODUCT the evaluation costs: at most that many matchers against each series the
//!   workspace holds ([`crate::state::MAX_ACTIVE_SERIES`]);
//! * a step and wall-clock [`Budget`], charged as the evaluation goes, so a query stops
//!   where its budget runs out.
//!
//! The evaluation itself reads a SNAPSHOT
//! ([`crate::state::IngestedMetrics::instant_snapshot`]) rather than the store, so none
//! of this work runs while the store's lock is held, and the plane runs the whole of it —
//! parse, plan and evaluation — on the blocking pool.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use promql_parser::label::Matcher;
use promql_parser::parser::{Expr, LabelModifier};

use crate::state::{Sample, METRIC_NAME_LABEL};

/// The longest expression this module parses.
///
/// The EMULATOR's own bound, and the FIRST one applied: it is what makes handing the text
/// to a parser safe. Every nesting level of PromQL costs at least two bytes of text (a
/// bracket pair, or an operator and the operand after it), so an expression within this
/// bound names a tree at most [`MAX_PARSED_DEPTH`] deep whichever way it nests — and that
/// is the tree the parser builds and later drops. Far above any query a dashboard or a
/// recording rule sends, and in the range an HTTP intermediary will carry in a URL.
pub const MAX_QUERY_BYTES: usize = 4 << 10;

/// The deepest tree an expression within [`MAX_QUERY_BYTES`] can name, at the two bytes a
/// nesting level costs at least. Stated rather than checked: it is the consequence of the
/// size bound that makes the parse itself bounded, while [`MAX_NESTING_DEPTH`] is what an
/// expression this module goes on to PLAN may be.
pub const MAX_PARSED_DEPTH: usize = MAX_QUERY_BYTES / 2;

/// How deep a caller's expression may be for this module to plan it.
///
/// The EMULATOR's own bound, measured on the parsed tree by [`tree_depth`], an iterative
/// walk — so no caller-chosen depth is ever walked with the call stack. Far above any
/// query a dashboard or a recording rule writes; Prometheus itself bounds expression
/// complexity in the same spirit.
pub const MAX_NESTING_DEPTH: usize = 32;

/// How many label matchers one expression may carry, counted across its `or`
/// alternatives.
///
/// The EMULATOR's own bound, and the one that caps the PRODUCT a query's cost is: the
/// evaluation applies at most this many matchers to each series the workspace holds
/// ([`crate::state::MAX_ACTIVE_SERIES`]), so neither factor can be grown without the
/// other staying capped.
pub const MAX_MATCHERS: usize = 64;

/// How many steps one query may spend — one step per series examined and one per matcher
/// evaluated.
///
/// The EMULATOR's own bound, and a BACKSTOP rather than a working limit: the two caps
/// above already hold a query to `MAX_MATCHERS` x `MAX_ACTIVE_SERIES` matcher
/// evaluations, which is below this, so a query that exhausts it is one the other bounds
/// did not foresee.
pub const MAX_QUERY_STEPS: u64 = 4 << 20;

/// How long one query may spend evaluating.
///
/// The EMULATOR's own bound, held beside the step budget because a step's cost is not
/// fixed: a regular-expression matcher's cost depends on the value it is matched against.
/// Checked every [`CLOCK_CHECK_STEPS`] steps, so reading the clock is not itself the
/// query's cost.
pub const QUERY_TIME_BUDGET: Duration = Duration::from_secs(2);

/// How many steps pass between two readings of the clock — see [`QUERY_TIME_BUDGET`].
const CLOCK_CHECK_STEPS: u64 = 1 << 10;

/// What one query may spend: a step count and a wall-clock deadline, both of this
/// process's own choosing and neither reachable from the request.
///
/// Charged as the evaluation goes, so the work stops where the budget runs out instead of
/// being abandoned still running.
#[derive(Debug)]
pub struct Budget {
    remaining: u64,
    until: Instant,
    next_clock_check: u64,
}

impl Budget {
    /// The budget one query is served under.
    pub fn for_one_query() -> Self {
        Self {
            remaining: MAX_QUERY_STEPS,
            until: Instant::now() + QUERY_TIME_BUDGET,
            next_clock_check: CLOCK_CHECK_STEPS,
        }
    }

    /// Charge `steps` of work, or report the budget spent.
    fn charge(&mut self, steps: u64) -> Result<(), QueryError> {
        self.remaining = self.remaining.saturating_sub(steps);
        if self.remaining == 0 {
            return Err(QueryError::Exhausted);
        }
        self.next_clock_check = self.next_clock_check.saturating_sub(steps);
        if self.next_clock_check == 0 {
            self.next_clock_check = CLOCK_CHECK_STEPS;
            if Instant::now() >= self.until {
                return Err(QueryError::Exhausted);
            }
        }
        Ok(())
    }
}

/// Why a query is not answered with a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The expression is not PromQL. Carries what the parser said, which names the
    /// position and the token it stopped on — the same detail Prometheus reports for a
    /// query it cannot parse.
    Parse(String),
    /// The expression parses, and this emulator does not evaluate expressions of that
    /// kind. Carries the kind, so the answer says what was not evaluated.
    Unsupported(&'static str),
    /// The expression is past a bound this emulator puts on an expression's own
    /// complexity ([`MAX_NESTING_DEPTH`], [`MAX_MATCHERS`]): the caller's to simplify,
    /// and refused before anything is evaluated.
    TooComplex(&'static str),
    /// The evaluation ran out of [`Budget`]. The expression was admissible; what it asked
    /// of the samples held was more than one query may spend.
    Exhausted,
}

/// What an instant query evaluates to: PromQL's two instant result types.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryValue {
    /// `resultType: "scalar"` — one value at the evaluation instant.
    Scalar(Sample),
    /// `resultType: "vector"` — one sample per series, each with its label set.
    Vector(Vec<VectorSample>),
}

/// One element of an instant vector: a label set and its sample.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorSample {
    /// The series' labels, as the result reports them. An aggregation's result carries
    /// the grouping labels only, with no metric name — as PromQL defines it.
    pub labels: BTreeMap<String, String>,
    pub sample: Sample,
}

/// An instant query this plane can evaluate.
#[derive(Debug, Clone)]
pub enum QueryPlan {
    /// A number literal.
    Scalar(f64),
    /// An instant vector selector, optionally wrapped in one aggregation.
    Vector {
        selector: SeriesSelector,
        aggregate: Option<Aggregation>,
    },
}

/// The series one selector selects: PromQL's label matchers, in the `or` alternatives the
/// grammar grouped them into (a series is selected when it satisfies every matcher of any
/// one alternative).
#[derive(Debug, Clone)]
pub struct SeriesSelector {
    alternatives: Vec<Vec<Matcher>>,
}

impl SeriesSelector {
    /// Whether `labels` is selected, charging `budget` for the series and for every
    /// matcher evaluated.
    ///
    /// A label the series does not carry is matched as the empty string, which is how
    /// PromQL defines matching: `{job!="x"}` selects a series carrying no `job`.
    pub fn selects(
        &self,
        labels: &BTreeMap<String, String>,
        budget: &mut Budget,
    ) -> Result<bool, QueryError> {
        budget.charge(1)?;
        let mut selected = false;
        for alternative in &self.alternatives {
            let mut satisfied = true;
            for matcher in alternative {
                budget.charge(1)?;
                let value = labels.get(&matcher.name).map(String::as_str).unwrap_or("");
                if !matcher.is_match(value) {
                    satisfied = false;
                    break;
                }
            }
            selected |= satisfied;
            if selected {
                break;
            }
        }
        Ok(selected)
    }

    /// How many matchers this selector carries across its `or` alternatives — what
    /// [`MAX_MATCHERS`] bounds.
    fn matcher_count(&self) -> usize {
        self.alternatives
            .iter()
            .map(|alternative| alternative.len())
            .sum()
    }
}

/// One aggregation over an instant vector: the operator and how the input is grouped.
#[derive(Debug, Clone)]
pub struct Aggregation {
    pub operator: AggregationOperator,
    pub grouping: Grouping,
}

/// The parameterless aggregation operators PromQL defines, which are the ones this plane
/// evaluates. `quantile` and `topk`/`bottomk` take a parameter and are reported
/// unsupported instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationOperator {
    Sum,
    Min,
    Max,
    Avg,
    Count,
    Group,
    Stddev,
    Stdvar,
}

impl AggregationOperator {
    /// The operator PromQL spells `name`, when it is one this plane evaluates.
    fn named(name: &str) -> Option<Self> {
        match name {
            "sum" => Some(Self::Sum),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "avg" => Some(Self::Avg),
            "count" => Some(Self::Count),
            "group" => Some(Self::Group),
            "stddev" => Some(Self::Stddev),
            "stdvar" => Some(Self::Stdvar),
            _ => None,
        }
    }

    /// Apply the operator to one group's values. The group is never empty: it exists
    /// because a series fell into it.
    fn apply(self, values: &[f64]) -> f64 {
        let count = values.len() as f64;
        let sum: f64 = values.iter().sum();
        match self {
            Self::Sum => sum,
            Self::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
            Self::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Self::Avg => sum / count,
            Self::Count => count,
            // "group" reports 1 for every group, whatever the values are.
            Self::Group => 1.0,
            Self::Stddev => variance(values, sum / count).sqrt(),
            Self::Stdvar => variance(values, sum / count),
        }
    }
}

/// The population variance of a group, which is what PromQL's `stdvar` reports (and
/// `stddev` takes the square root of).
fn variance(values: &[f64], mean: f64) -> f64 {
    values
        .iter()
        .map(|value| (value - mean) * (value - mean))
        .sum::<f64>()
        / values.len() as f64
}

/// How an aggregation groups its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grouping {
    /// No modifier: every series falls into one group, whose label set is empty.
    All,
    /// `by (…)`: the group keeps these labels.
    By(Vec<String>),
    /// `without (…)`: the group drops these labels and the metric name.
    Without(Vec<String>),
}

impl Grouping {
    /// The labels one result element of this grouping carries.
    fn key(&self, labels: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        match self {
            Self::All => BTreeMap::new(),
            Self::By(kept) => kept
                .iter()
                .filter_map(|label| {
                    labels
                        .get(label)
                        .map(|value| (label.clone(), value.clone()))
                })
                .collect(),
            Self::Without(dropped) => {
                let dropped: BTreeSet<&str> = dropped.iter().map(String::as_str).collect();
                labels
                    .iter()
                    .filter(|(label, _)| {
                        label.as_str() != METRIC_NAME_LABEL && !dropped.contains(label.as_str())
                    })
                    .map(|(label, value)| (label.clone(), value.clone()))
                    .collect()
            }
        }
    }
}

/// Parse `expression` and plan how it is evaluated.
///
/// Every bound this module places on the caller's expression is applied HERE: its SIZE,
/// which is what makes handing the text to a parser safe at all; its DEPTH, measured on
/// the tree by an iterative walk ([`MAX_NESTING_DEPTH`]); and the number of matchers the
/// plan carries ([`MAX_MATCHERS`]). What survives is an expression whose evaluation cost
/// is bounded by [`MAX_MATCHERS`] times the series the workspace holds, charged step by
/// step against a [`Budget`].
pub fn plan(expression: &str) -> Result<QueryPlan, QueryError> {
    // The SIZE is judged first, and it is what bounds the tree the parser may build and
    // then drop: every nesting level of PromQL costs at least two bytes of text (a
    // bracket pair, or an operator and the operand after it), so an expression within
    // this bound cannot name a tree deeper than [`MAX_PARSED_DEPTH`] however it is
    // written — with brackets, with an operator chain, or with repeated unary operators.
    // Nothing here reads the text as the lexer does; a scan that had to agree with the
    // lexer about where a string or a comment ends would be a second, divergent reading
    // of the caller's expression.
    if expression.len() > MAX_QUERY_BYTES {
        return Err(QueryError::TooComplex(
            "the expression is longer than this emulator parses",
        ));
    }
    let parsed = promql_parser::parser::parse(expression).map_err(QueryError::Parse)?;
    // And the DEPTH is judged on the tree itself, exactly, by a walk with its own
    // worklist — so everything downstream, including this module's own planning, sees a
    // tree no deeper than this.
    if tree_depth(&parsed) > MAX_NESTING_DEPTH {
        return Err(QueryError::TooComplex("the expression nests too deeply"));
    }
    let plan = plan_of(&parsed)?;
    if let QueryPlan::Vector { selector, .. } = &plan {
        if selector.matcher_count() > MAX_MATCHERS {
            return Err(QueryError::TooComplex(
                "the expression carries more label matchers than this emulator evaluates",
            ));
        }
    }
    Ok(plan)
}

/// How deep `root` is, counted by an ITERATIVE walk with its own worklist: the depth is
/// the caller's to choose, so measuring it with the call stack is not available.
///
/// Stops as soon as the answer is past [`MAX_NESTING_DEPTH`], so a deep tree costs one
/// walk of its first levels rather than of all of them. The worklist is bounded by the
/// tree, which is bounded by [`MAX_QUERY_BYTES`].
fn tree_depth(root: &Expr) -> usize {
    let mut deepest = 0usize;
    let mut pending: Vec<(&Expr, usize)> = vec![(root, 1)];
    while let Some((expr, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        if deepest > MAX_NESTING_DEPTH {
            return deepest;
        }
        match expr {
            Expr::Aggregate(aggregate) => {
                pending.push((&aggregate.expr, depth + 1));
                if let Some(param) = &aggregate.param {
                    pending.push((param, depth + 1));
                }
            }
            Expr::Unary(unary) => pending.push((&unary.expr, depth + 1)),
            Expr::Binary(binary) => {
                pending.push((&binary.lhs, depth + 1));
                pending.push((&binary.rhs, depth + 1));
            }
            Expr::Paren(paren) => pending.push((&paren.expr, depth + 1)),
            Expr::Subquery(subquery) => pending.push((&subquery.expr, depth + 1)),
            Expr::Call(call) => {
                for argument in &call.args.args {
                    pending.push((argument, depth + 1));
                }
            }
            // A selector and a literal carry no sub-expression, and an extension carries
            // one this module cannot walk — it is refused by `plan_of` whatever its
            // depth.
            _ => {}
        }
    }
    deepest
}

/// The plan for one parsed expression. `paren` is transparent, as PromQL defines it, and
/// is unwrapped ITERATIVELY: the chain's length is the caller's to choose, so walking it
/// with the call stack is not available.
fn plan_of(expr: &Expr) -> Result<QueryPlan, QueryError> {
    let expr = unparenthesised(expr);
    match expr {
        Expr::NumberLiteral(number) => Ok(QueryPlan::Scalar(number.val)),
        Expr::VectorSelector(_) => Ok(QueryPlan::Vector {
            selector: selector_of(expr)?,
            aggregate: None,
        }),
        Expr::Aggregate(aggregate) => {
            if aggregate.param.is_some() {
                return Err(QueryError::Unsupported("parameterised aggregation"));
            }
            let name = aggregate.op.to_string();
            let Some(operator) = AggregationOperator::named(&name) else {
                return Err(QueryError::Unsupported("aggregation operator"));
            };
            let grouping = match &aggregate.modifier {
                None => Grouping::All,
                Some(LabelModifier::Include(labels)) => Grouping::By(label_names(labels)),
                Some(LabelModifier::Exclude(labels)) => Grouping::Without(label_names(labels)),
            };
            Ok(QueryPlan::Vector {
                selector: selector_of(&aggregate.expr)?,
                aggregate: Some(Aggregation {
                    operator,
                    grouping,
                }),
            })
        }
        Expr::MatrixSelector(_) => Err(QueryError::Unsupported("range selector")),
        Expr::Subquery(_) => Err(QueryError::Unsupported("subquery")),
        Expr::Call(_) => Err(QueryError::Unsupported("function call")),
        Expr::Binary(_) => Err(QueryError::Unsupported("binary operation")),
        Expr::Unary(_) => Err(QueryError::Unsupported("unary operation")),
        Expr::StringLiteral(_) => Err(QueryError::Unsupported("string literal")),
        _ => Err(QueryError::Unsupported("expression")),
    }
}

/// The expression a chain of `paren` wrappers wraps. A loop, not a recursion: the depth
/// is caller-chosen, and [`MAX_NESTING_DEPTH`] bounds how many hops there can be.
fn unparenthesised(expr: &Expr) -> &Expr {
    let mut expr = expr;
    while let Expr::Paren(inner) = expr {
        expr = &inner.expr;
    }
    expr
}

/// The selector of an instant vector expression, which is what both an unwrapped
/// selector and an aggregation's input must be.
fn selector_of(expr: &Expr) -> Result<SeriesSelector, QueryError> {
    match unparenthesised(expr) {
        Expr::VectorSelector(selector) => {
            if selector.offset.is_some() || selector.at.is_some() {
                return Err(QueryError::Unsupported("offset or @ modifier"));
            }
            // The metric name a selector states is `__name__` equality, which is how
            // PromQL defines it; the parser leaves it beside the matchers when the query
            // wrote it as a name.
            let mut required: Vec<Matcher> = Vec::new();
            if let Some(name) = &selector.name {
                required.push(Matcher::new(
                    promql_parser::label::MatchOp::Equal,
                    METRIC_NAME_LABEL,
                    name,
                ));
            }
            let alternatives = selector_alternatives(selector, &required);
            Ok(SeriesSelector { alternatives })
        }
        Expr::MatrixSelector(_) => Err(QueryError::Unsupported("range selector")),
        _ => Err(QueryError::Unsupported("nested expression")),
    }
}

/// Evaluate `plan` against `snapshot` — the samples the workspace held at the evaluation
/// instant ([`crate::state::IngestedMetrics::instant_snapshot`]), read out under the store's lock and
/// handed here without it.
///
/// `budget` is charged for every series examined and every matcher evaluated, and the
/// evaluation stops with [`QueryError::Exhausted`] when it runs out — so the work one
/// query can ask for is bounded whatever its expression and however many series the
/// workspace holds.
pub fn evaluate(
    plan: &QueryPlan,
    snapshot: Vec<(BTreeMap<String, String>, Sample)>,
    at: SystemTime,
    budget: &mut Budget,
) -> Result<QueryValue, QueryError> {
    let at_ms = crate::state::unix_millis(at);
    match plan {
        QueryPlan::Scalar(value) => Ok(QueryValue::Scalar(Sample {
            timestamp_ms: at_ms,
            value: *value,
        })),
        QueryPlan::Vector {
            selector,
            aggregate,
        } => {
            let mut instant: Vec<VectorSample> = Vec::new();
            for (labels, sample) in snapshot {
                if selector.selects(&labels, budget)? {
                    instant.push(VectorSample { labels, sample });
                }
            }
            match aggregate {
                None => Ok(QueryValue::Vector(instant)),
                Some(aggregation) => Ok(QueryValue::Vector(aggregated(
                    aggregation, instant, at_ms, budget,
                )?)),
            }
        }
    }
}

/// One aggregation applied to an instant vector: the input grouped by the modifier, each
/// group reported at the evaluation instant. One step per element grouped.
fn aggregated(
    aggregation: &Aggregation,
    instant: Vec<VectorSample>,
    at_ms: i64,
    budget: &mut Budget,
) -> Result<Vec<VectorSample>, QueryError> {
    let mut groups: BTreeMap<String, (BTreeMap<String, String>, Vec<f64>)> = BTreeMap::new();
    for element in instant {
        budget.charge(1)?;
        let labels = aggregation.grouping.key(&element.labels);
        let key = crate::state::series_key(&labels);
        groups
            .entry(key)
            .or_insert_with(|| (labels, Vec::new()))
            .1
            .push(element.sample.value);
    }
    Ok(groups
        .into_values()
        .map(|(labels, values)| VectorSample {
            labels,
            sample: Sample {
                timestamp_ms: at_ms,
                value: aggregation.operator.apply(&values),
            },
        })
        .collect())
}

/// The label names a `by`/`without` modifier lists.
fn label_names(labels: &promql_parser::label::Labels) -> Vec<String> {
    labels.labels.to_vec()
}

/// The `or` alternatives of one selector, each carrying the matchers the query stated for
/// it plus the metric name's own equality matcher.
///
/// A selector with no `or` is one alternative, so matching has one shape whether the
/// query used `or` or not.
fn selector_alternatives(
    selector: &promql_parser::parser::VectorSelector,
    required: &[Matcher],
) -> Vec<Vec<Matcher>> {
    let mut alternatives: Vec<Vec<Matcher>> = Vec::new();
    for alternative in &selector.matchers.or_matchers {
        let mut matchers = required.to_vec();
        matchers.extend(alternative.iter().cloned());
        alternatives.push(matchers);
    }
    let mut first = required.to_vec();
    first.extend(selector.matchers.matchers.iter().cloned());
    if alternatives.is_empty() || !selector.matchers.matchers.is_empty() {
        alternatives.insert(0, first);
    }
    alternatives
}

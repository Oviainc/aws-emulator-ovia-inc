// SPDX-License-Identifier: CC0-1.0

//! X-Ray filter expressions: EVALUATING the query language
//! `GetTraceSummaries$FilterExpression` and `Group$FilterExpression` carry.
//!
//! The language is PARSED in exactly one place — [`super::filter_expression`],
//! which owns the grammar, the catalog of fields X-Ray indexes, the bounds on
//! how long and how deeply nested an expression may be, and the wording of every
//! refusal that follows from them. A surface that only STORES an expression
//! (`CreateGroup`, `UpdateGroup`) asks it and nothing else.
//!
//! This module takes the tree that parser produces and makes it ANSWERABLE from
//! the stored segment documents, which needs three things the parser does not
//! decide:
//!
//! * which fields this emulator can derive a value for — a subset of the indexed
//!   catalog, and a field outside it is refused BY NAME rather than quietly
//!   evaluating to false;
//! * whether a comparison's operands can be compared at all — a string field
//!   against a number can never be true for any trace, which is a mistake in the
//!   request wherever it appears, so [`operand_check`] lends that one judgement
//!   back to the storing surfaces;
//! * what one QUERY may cost, which the parser cannot know because it sees one
//!   expression and not the store it will be run over ([`MAX_QUERY_STEPS`]).
//!
//! A filter expression "selects a subset of the traces in the requested time
//! frame" (`smithy_query(query="doc",shape="GetTraceSummaries")`), so it is
//! evaluated against each stored trace here.
//!
//! ## The keywords, and where each one's value comes from
//!
//! Every keyword reads a value this service already derives from the stored
//! segment documents, and the catalog is the `TraceSummary` the reader returns
//! (`smithy_query(query="shape",shape="TraceSummary")`) plus the root-cause
//! shapes it carries:
//!
//! | keyword | type | value |
//! |---|---|---|
//! | `ok`, `error`, `fault`, `throttle` | boolean | the entity classifications in scope ([`Classification`]) |
//! | `partial` | boolean | `TraceSummary$IsPartial` |
//! | `duration`, `responsetime` | number | `TraceSummary$Duration` / `$ResponseTime` |
//! | `http.status` | number | `Http$HttpStatus` |
//! | `http.url`, `http.method`, `http.useragent`, `http.clientip` | string | the other `Http` members |
//! | `user` | string | `TraceUser$UserName` |
//! | `resource.arn` | string | `ResourceARNDetail$ARN` |
//! | `annotation.<key>` | any | `TraceSummary$Annotations` |
//! | `rootcause.{fault,error}.service.{name,type}` | string | `FaultRootCauseService$Name` / `$Type` |
//! | `rootcause.{fault,error}.service.inferred` | boolean | `FaultRootCauseService$Inferred` |
//! | `rootcause.{fault,error}.entity.name` | string | `FaultRootCauseEntity$Name` |
//! | `rootcause.{fault,error}.entity.remote` | boolean | `FaultRootCauseEntity$Remote` |
//! | `rootcause.{fault,error}.exception.{name,message}` | string | `RootCauseException$Name` / `$Message` |
//!
//! `service("name"[, "type"])` and `edge("source", "destination")` scope the
//! keywords inside their `{ … }` block to one service's segments, or to one
//! edge's destination segments; a scope keyword without a block asks only
//! whether the trace ran through it.
//!
//! A keyword whose value this emulator does not derive is REFUSED by name:
//! `rootcause.responsetime.*` needs `TraceSummary$ResponseTimeRootCauses`, which
//! nothing in a submitted document states, so it is not answered here.
//!
//! A value a trace carries more than once (an annotation several services
//! recorded, the entities of a root cause) satisfies a comparison when ANY of
//! those values does.
//!
//! ## What one query may cost
//!
//! An expression's own size is bounded by the parser
//! ([`super::filter_expression::MAX_LENGTH`] characters,
//! [`super::filter_expression::MAX_DEPTH`] levels of nesting), which also bounds
//! this module's recursion: the tree holds at most one node per two characters
//! of source, and [`evaluate`] and the recursive `Drop` of the tree walk its
//! height.
//!
//! What those bounds do NOT cap is the PRODUCT one request drives — traces
//! scanned × nodes × segments per node — so [`MAX_QUERY_STEPS`] does, carried
//! through a whole query as a [`QueryBudget`]. Every trace a query reads spends
//! from the same budget, and a query that spends it is answered by the caller
//! with the modelled throttling error rather than from a selection that stopped
//! halfway. Per trace, evaluation visits each node at most once, and the values
//! the nodes read are derived ONCE per trace into [`Facts`] rather than per node.

use std::collections::BTreeMap;

use super::filter_expression::{self, Operand, Operator};
use super::{
    Classification, Outcome, RootCauseKind, RootCauseView, SegmentRecord, SubsegmentRecord,
    TraceRecord,
};

/// The most steps one filtered query may spend across EVERY trace it reads.
///
/// The parser caps one expression; this caps the PRODUCT a single request can
/// drive — the traces it scans, the nodes it evaluates per trace, and the
/// segments each node reads — because capping the factors alone still multiplies
/// them. A step is one node visit or one segment, subsegment or root cause read,
/// counted by [`QueryBudget`]; a query that would spend more is answered with the
/// modelled throttling error (the caller narrows the window or simplifies the
/// expression) rather than from a selection that stopped halfway.
///
/// Sized so that no request within the other bounds reaches it in normal use:
/// the whole trace store ([`super::MAX_TRACES`]) read by a 20-node expression
/// over 10-segment traces spends about 200_000 steps.
pub const MAX_QUERY_STEPS: usize = 2_000_000;

/// What is left of one query's [`MAX_QUERY_STEPS`].
///
/// One budget is created per `GetTraceSummaries` call and threaded through every
/// trace it evaluates, so the bound is on the REQUEST, not on one trace.
#[derive(Debug)]
pub struct QueryBudget {
    remaining: usize,
}

impl Default for QueryBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryBudget {
    /// A budget of [`MAX_QUERY_STEPS`].
    pub fn new() -> Self {
        Self {
            remaining: MAX_QUERY_STEPS,
        }
    }

    /// Whether the query has spent its budget. The caller answers the request
    /// with the modelled throttling error when it has, because the selection it
    /// would otherwise report is incomplete.
    pub fn exhausted(&self) -> bool {
        self.remaining == 0
    }

    /// Spend `steps`, saturating at nothing left.
    fn spend(&mut self, steps: usize) {
        self.remaining = self.remaining.saturating_sub(steps);
    }
}

/// Why an expression could not be evaluated. The caller renders it as the
/// modelled `InvalidRequestException` (HTTP 400).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError {
    /// What is wrong with the expression, naming the part at fault.
    pub message: String,
    /// Whether the fault is a COMPARISON whose operands cannot be compared at
    /// all — a string keyword against a number, say.
    ///
    /// Such an expression can never be true for any trace, so it is a mistake in
    /// the request wherever it appears, and the surfaces that only store an
    /// expression refuse it too ([`operand_check`]). Every other fault this
    /// module reports is about what this evaluator can answer, which is not
    /// those surfaces' business.
    pub operand_mismatch: bool,
}

impl FilterError {
    fn new(detail: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Invalid filter expression: {detail}"),
            operand_mismatch: false,
        }
    }

    /// A comparison whose operands cannot be compared.
    fn operands(detail: impl std::fmt::Display) -> Self {
        Self {
            operand_mismatch: true,
            ..Self::new(detail)
        }
    }
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// An expression compiled into what this module can evaluate.
#[derive(Debug, Clone)]
pub struct FilterExpression {
    root: Node,
    /// Which derived values the expression reads, so a trace derives only
    /// those, and derives each once.
    reads: Reads,
}

/// Which derived values an expression's keywords read. Noted once at compile
/// time, so evaluating a trace never derives a value no keyword asks for.
#[derive(Debug, Clone, Copy, Default)]
struct Reads {
    fault_root_causes: bool,
    error_root_causes: bool,
    annotations: bool,
    /// Whether any scope is an `edge(…)`, which resolves a parent entity id to
    /// the service that holds it.
    edges: bool,
}

/// Parse `source` with [`super::filter_expression`] and compile the tree it
/// answers into what this module evaluates.
pub fn parse(source: &str) -> Result<FilterExpression, FilterError> {
    let parsed = filter_expression::parse(source).map_err(|failure| FilterError {
        message: failure.message(),
        operand_mismatch: false,
    })?;
    let root = compile(&parsed)?;
    let mut reads = Reads::default();
    note_reads(&root, &mut reads);
    Ok(FilterExpression { root, reads })
}

/// Whether `source` holds a comparison whose operands cannot be compared at all.
///
/// The surfaces that STORE an expression ask this beside the parser: everything
/// else this module can refuse is about what the evaluator can answer rather
/// than about the request — but a comparison between a string keyword and a
/// number (or an ordering operator on a value that has no order) can never be
/// true for any trace, so it is a mistake in the request wherever it appears.
/// `Ok(())` for anything the parser itself refuses: that refusal is the
/// storing surface's own call.
pub fn operand_check(source: &str) -> Result<(), FilterError> {
    match parse(source) {
        Ok(_) => Ok(()),
        Err(error) if error.operand_mismatch => Err(error),
        Err(_) => Ok(()),
    }
}

impl FilterExpression {
    /// Whether `trace` satisfies this expression, spending `budget`.
    ///
    /// The values the expression reads are derived from the trace ONCE, into
    /// [`Facts`], before the tree is walked — so a node costs a lookup rather
    /// than a re-derivation, and the whole evaluation is one pass over the trace
    /// plus one pass over the tree.
    ///
    /// `budget` is the WHOLE query's, so the work a request does over every
    /// trace it reads is bounded, not just the work one trace costs. Once it is
    /// spent, evaluation stops and answers `false`; the caller checks
    /// [`QueryBudget::exhausted`] and answers the request with the modelled
    /// throttling error rather than reporting a selection that stopped halfway.
    pub fn matches(&self, trace: &TraceRecord, budget: &mut QueryBudget) -> bool {
        if budget.exhausted() {
            return false;
        }
        let facts = Facts::of(trace, self.reads, budget);
        evaluate(&self.root, &facts, &Scope::whole(trace.segments.len()), budget)
    }
}

// ---------------------------------------------------------------------------
// Compiling the parsed tree
// ---------------------------------------------------------------------------

/// What this module evaluates: the parsed tree with every predicate resolved to
/// a value it can read and a comparison it can make.
#[derive(Debug, Clone)]
enum Node {
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    /// A keyword compared to a literal, or a boolean keyword on its own (which
    /// compiles to `keyword = true`).
    Compare {
        field: Field,
        op: Operator,
        value: Literal,
    },
    /// `service(…)` / `edge(…)`, with the expression its block scopes.
    Scoped {
        selector: Selector,
        inner: Option<Box<Node>>,
    },
}

/// A comparison's right-hand value.
#[derive(Debug, Clone)]
enum Literal {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Literal {
    fn kind(&self) -> &'static str {
        match self {
            Self::Str(_) => "a string",
            Self::Num(_) => "a number",
            Self::Bool(_) => "a boolean",
        }
    }

    /// The literal a parsed operand carries, or `None` for one naming another
    /// field — a comparison between two fields is not evaluated here.
    fn of(operand: &Operand) -> Option<Self> {
        match operand {
            Operand::Text(text) => Some(Self::Str(text.clone())),
            Operand::Number(number) => Some(Self::Num(*number)),
            Operand::Boolean(value) => Some(Self::Bool(*value)),
            Operand::Field(_) => None,
        }
    }
}

/// Which entities a scope block's keywords read.
#[derive(Debug, Clone)]
enum Selector {
    /// `service("name")` / `service("name", "type")`.
    Service {
        name: String,
        r#type: Option<String>,
    },
    /// `edge("source", "destination")`.
    Edge { source: String, destination: String },
}

/// Compile one parsed node.
///
/// The recursion follows the parsed tree's height, which
/// [`super::filter_expression`] bounds.
fn compile(node: &filter_expression::Node) -> Result<Node, FilterError> {
    match node {
        filter_expression::Node::And(left, right) => Ok(Node::And(
            Box::new(compile(left)?),
            Box::new(compile(right)?),
        )),
        filter_expression::Node::Or(left, right) => Ok(Node::Or(
            Box::new(compile(left)?),
            Box::new(compile(right)?),
        )),
        filter_expression::Node::Not(inner) => Ok(Node::Not(Box::new(compile(inner)?))),
        filter_expression::Node::Predicate {
            path,
            arguments,
            block,
            comparison,
        } => compile_predicate(path, arguments, block.as_deref(), comparison.as_ref()),
    }
}

/// Compile one predicate: a scope, a comparison, or a bare boolean keyword.
fn compile_predicate(
    path: &str,
    arguments: &[filter_expression::Argument],
    block: Option<&filter_expression::Node>,
    comparison: Option<&(Operator, Operand)>,
) -> Result<Node, FilterError> {
    if path == "service" || path == "edge" {
        let selector = selector_of(path, arguments)?;
        let inner = match block {
            None => None,
            Some(inner) => Some(Box::new(compile(inner)?)),
        };
        return Ok(Node::Scoped { selector, inner });
    }
    if block.is_some() {
        return Err(unevaluated(format!(
            "`{path}` with a `{{ … }}` block is not evaluated by this emulator yet"
        )));
    }
    if !arguments.is_empty() {
        return Err(unevaluated(format!(
            "`{path}` with arguments is not evaluated by this emulator yet"
        )));
    }
    let field = field_of(path)?;
    match comparison {
        Some((op, operand)) => {
            let Some(value) = Literal::of(operand) else {
                return Err(unevaluated(
                    "a comparison between two keywords is not evaluated by this emulator yet",
                ));
            };
            check(path, &field, *op, &value)?;
            Ok(Node::Compare {
                field,
                op: *op,
                value,
            })
        }
        // A boolean keyword stands alone: `fault` is `fault = true`.
        None if field.value_type() == ValueType::Bool => Ok(Node::Compare {
            field,
            op: Operator::Equal,
            value: Literal::Bool(true),
        }),
        None => Err(FilterError::new(format!(
            "`{path}` is not a boolean, so it needs a comparison"
        ))),
    }
}

/// The scope a `service(…)` / `edge(…)` predicate selects.
fn selector_of(
    path: &str,
    arguments: &[filter_expression::Argument],
) -> Result<Selector, FilterError> {
    let names: Vec<&str> = arguments
        .iter()
        .filter_map(|argument| match &argument.value {
            Operand::Text(text) => Some(text.as_str()),
            Operand::Field(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if names.len() != arguments.len() {
        return Err(unevaluated(format!(
            "`{path}` is evaluated for named services only"
        )));
    }
    match (path, names.as_slice()) {
        ("service", [name]) => Ok(Selector::Service {
            name: (*name).to_string(),
            r#type: None,
        }),
        ("service", [name, r#type]) => Ok(Selector::Service {
            name: (*name).to_string(),
            r#type: Some((*r#type).to_string()),
        }),
        ("edge", [source, destination]) => Ok(Selector::Edge {
            source: (*source).to_string(),
            destination: (*destination).to_string(),
        }),
        ("service", found) => Err(FilterError::new(format!(
            "`service` takes a name and optionally a type, not {} arguments",
            found.len()
        ))),
        (_, found) => Err(FilterError::new(format!(
            "`edge` takes a source and a destination, not {} arguments",
            found.len()
        ))),
    }
}

/// A part of the language this emulator reads but does not evaluate. Named
/// rather than answered with silence, so a caller is not told "no traces match"
/// for a question that was never asked.
fn unevaluated(detail: impl std::fmt::Display) -> FilterError {
    FilterError::new(detail)
}

/// Which derived values `node`'s keywords read. Walked once at compile time, so
/// a trace derives only what the expression actually asks for.
fn note_reads(node: &Node, reads: &mut Reads) {
    match node {
        Node::And(left, right) | Node::Or(left, right) => {
            note_reads(left, reads);
            note_reads(right, reads);
        }
        Node::Not(inner) => note_reads(inner, reads),
        Node::Scoped { selector, inner } => {
            if matches!(selector, Selector::Edge { .. }) {
                reads.edges = true;
            }
            if let Some(inner) = inner {
                note_reads(inner, reads);
            }
        }
        Node::Compare { field, .. } => match field {
            Field::RootCause {
                kind: RootCauseKind::Fault,
                ..
            } => reads.fault_root_causes = true,
            Field::RootCause {
                kind: RootCauseKind::Error,
                ..
            } => reads.error_root_causes = true,
            Field::Annotation(_) => reads.annotations = true,
            _ => {}
        },
    }
}

// ---------------------------------------------------------------------------
// The fields this module can answer from, and their types
// ---------------------------------------------------------------------------

/// What one keyword reads.
#[derive(Debug, Clone)]
enum Field {
    Flag(Flag),
    Duration,
    ResponseTime,
    HttpStatus,
    Http(HttpField),
    User,
    ResourceArn,
    Annotation(String),
    RootCause {
        kind: RootCauseKind,
        part: RootCausePart,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Ok,
    Error,
    Fault,
    Throttle,
    Partial,
}

#[derive(Debug, Clone, Copy)]
enum HttpField {
    Url,
    Method,
    UserAgent,
    ClientIp,
}

#[derive(Debug, Clone, Copy)]
enum RootCausePart {
    ServiceName,
    ServiceType,
    ServiceInferred,
    EntityName,
    EntityRemote,
    ExceptionName,
    ExceptionMessage,
}

/// What type a keyword's value has, which is what a comparison is checked
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueType {
    Bool,
    Num,
    Str,
    /// An annotation: the submitted value decides, so any literal compares.
    Any,
}

impl Field {
    fn value_type(&self) -> ValueType {
        match self {
            Self::Flag(_) => ValueType::Bool,
            Self::Duration | Self::ResponseTime | Self::HttpStatus => ValueType::Num,
            Self::Http(_) | Self::User | Self::ResourceArn => ValueType::Str,
            Self::Annotation(_) => ValueType::Any,
            Self::RootCause { part, .. } => match part {
                RootCausePart::ServiceInferred | RootCausePart::EntityRemote => ValueType::Bool,
                _ => ValueType::Str,
            },
        }
    }
}

/// The subset of the indexed catalog this module can answer from.
///
/// The catalog of fields X-Ray INDEXES is [`super::filter_expression`]'s, and it
/// is wider than this one: it is what decides whether an expression is of the
/// language at all. A field the parser accepts and this does not is refused as
/// unevaluated rather than as unknown, so the refusal says which of the two
/// judged it.
fn field_of(keyword: &str) -> Result<Field, FilterError> {
    if let Some(key) = keyword.strip_prefix("annotation.") {
        if key.is_empty() {
            return Err(FilterError::new(
                "`annotation.` needs the name of an annotation",
            ));
        }
        return Ok(Field::Annotation(key.to_string()));
    }
    if let Some(rest) = keyword.strip_prefix("rootcause.") {
        return root_cause_field(rest);
    }
    match keyword {
        "ok" => Ok(Field::Flag(Flag::Ok)),
        "error" => Ok(Field::Flag(Flag::Error)),
        "fault" => Ok(Field::Flag(Flag::Fault)),
        "throttle" => Ok(Field::Flag(Flag::Throttle)),
        "partial" => Ok(Field::Flag(Flag::Partial)),
        "duration" => Ok(Field::Duration),
        "responsetime" => Ok(Field::ResponseTime),
        "http.status" => Ok(Field::HttpStatus),
        "http.url" => Ok(Field::Http(HttpField::Url)),
        "http.method" => Ok(Field::Http(HttpField::Method)),
        "http.useragent" => Ok(Field::Http(HttpField::UserAgent)),
        "http.clientip" => Ok(Field::Http(HttpField::ClientIp)),
        "user" => Ok(Field::User),
        "resource.arn" => Ok(Field::ResourceArn),
        other => Err(unevaluated(format!(
            "`{other}` is an indexed field this emulator does not evaluate yet"
        ))),
    }
}

/// `rootcause.<fault|error>.<service|entity|exception>.<member>`.
fn root_cause_field(rest: &str) -> Result<Field, FilterError> {
    let (class, selector) = rest.split_once('.').ok_or_else(|| {
        unevaluated(format!(
            "`rootcause.{rest}` is not a root cause member this emulator evaluates"
        ))
    })?;
    let kind = match class {
        "fault" => RootCauseKind::Fault,
        "error" => RootCauseKind::Error,
        // `ResponseTimeRootCauses` is derived from a response-time analysis of
        // the trace, which nothing in a submitted segment document states, so
        // this emulator derives none and says so rather than answering a filter
        // on them with silence.
        "responsetime" => {
            return Err(unevaluated(
                "`rootcause.responsetime` is not evaluated by this emulator: it reports no ResponseTimeRootCauses",
            ))
        }
        other => {
            return Err(unevaluated(format!(
                "`rootcause.{other}` is not a root cause class"
            )))
        }
    };
    let part = match selector {
        "service.name" => RootCausePart::ServiceName,
        "service.type" => RootCausePart::ServiceType,
        "service.inferred" => RootCausePart::ServiceInferred,
        "entity.name" => RootCausePart::EntityName,
        "entity.remote" => RootCausePart::EntityRemote,
        "exception.name" => RootCausePart::ExceptionName,
        "exception.message" => RootCausePart::ExceptionMessage,
        other => {
            return Err(unevaluated(format!(
                "`rootcause.{class}.{other}` is not a root cause member this emulator evaluates"
            )))
        }
    };
    Ok(Field::RootCause { kind, part })
}

/// Refuse a comparison whose operands cannot be compared at all.
fn check(keyword: &str, field: &Field, op: Operator, value: &Literal) -> Result<(), FilterError> {
    let expected = field.value_type();
    let compatible = matches!(
        (expected, value),
        (ValueType::Any, _)
            | (ValueType::Bool, Literal::Bool(_))
            | (ValueType::Num, Literal::Num(_))
            | (ValueType::Str, Literal::Str(_))
    );
    if !compatible {
        let wanted = match expected {
            ValueType::Bool => "a boolean",
            ValueType::Num => "a number",
            ValueType::Str => "a string",
            ValueType::Any => "a value",
        };
        return Err(FilterError::operands(format!(
            "`{keyword}` is {wanted} and cannot be compared to {}",
            value.kind()
        )));
    }
    if orders(op) && !matches!(value, Literal::Num(_)) {
        return Err(FilterError::operands(format!(
            "`{keyword}` cannot be ordered against {}",
            value.kind()
        )));
    }
    if textual(op) && !matches!(value, Literal::Str(_)) {
        return Err(FilterError::operands(format!(
            "a string operator cannot compare `{keyword}` to {}",
            value.kind()
        )));
    }
    Ok(())
}

/// Whether `op` orders its operands, which only numbers do.
fn orders(op: Operator) -> bool {
    matches!(
        op,
        Operator::Less | Operator::LessOrEqual | Operator::Greater | Operator::GreaterOrEqual
    )
}

/// Whether `op` is one of the string operators, which only strings take.
fn textual(op: Operator) -> bool {
    matches!(
        op,
        Operator::Contains | Operator::BeginsWith | Operator::EndsWith
    )
}

/// Apply an equality or ordering operator to an `Ordering`.
fn ordered(op: Operator, ordering: std::cmp::Ordering) -> bool {
    match op {
        Operator::Equal => ordering.is_eq(),
        Operator::NotEqual => ordering.is_ne(),
        Operator::Less => ordering.is_lt(),
        Operator::LessOrEqual => ordering.is_le(),
        Operator::Greater => ordering.is_gt(),
        Operator::GreaterOrEqual => ordering.is_ge(),
        // The string operators are applied in `matches_str`, which is the only
        // value kind that takes them.
        Operator::Contains | Operator::BeginsWith | Operator::EndsWith => false,
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Everything one trace contributes to an expression, derived ONCE per trace
/// before the tree is walked.
///
/// Without this, a node that reads an entity classification, an annotation or a
/// root cause would re-derive it from the whole segment tree every time it is
/// visited; with it, the per-trace cost is one pass over the trace plus, per
/// node, a lookup over the segments in scope.
struct Facts<'a> {
    trace: &'a TraceRecord,
    /// Per segment, the recorded outcomes of its own entity tree.
    outcomes: Vec<Vec<Outcome>>,
    /// Every annotation the trace carries, by key, with the segment that
    /// recorded it. Empty unless the expression reads an annotation.
    annotations: BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>>,
    /// Every entity id in the trace mapped to the segment that holds it, so an
    /// `edge(…)` scope resolves a `parent_id` by lookup instead of re-scanning
    /// the trace per segment. Empty unless the expression carries an edge.
    entity_owners: BTreeMap<&'a str, usize>,
    /// The trace's own outcome flags, each in the scope the model documents for
    /// it, so a keyword at trace scope answers exactly what `TraceSummary`
    /// reports.
    trace_flags: TraceFlags,
    /// The fault root causes, when the expression reads them.
    fault_causes: Vec<RootCauseView>,
    /// The error root causes, when the expression reads them.
    error_causes: Vec<RootCauseView>,
}

impl<'a> Facts<'a> {
    fn of(trace: &'a TraceRecord, reads: Reads, budget: &mut QueryBudget) -> Self {
        let outcomes: Vec<Vec<Outcome>> = trace
            .segments
            .iter()
            .map(SegmentRecord::outcomes)
            .collect();
        // One step per entity read: this is the pass over the trace, so it is
        // what a trace costs before a single node is visited.
        budget.spend(outcomes.iter().map(Vec::len).sum::<usize>().max(1));
        let mut annotations: BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>> =
            BTreeMap::new();
        if reads.annotations {
            for (at, segment) in trace.segments.iter().enumerate() {
                collect_annotations(at, &segment.annotations, &mut annotations);
                walk_annotations(at, &segment.subsegments, &mut annotations);
            }
            budget.spend(annotations.values().map(Vec::len).sum());
        }
        let entity_owners = if reads.edges {
            let owners = trace.entity_owners();
            budget.spend(owners.len());
            owners
        } else {
            BTreeMap::new()
        };
        let fault_causes = if reads.fault_root_causes {
            let causes = trace.root_causes(RootCauseKind::Fault);
            budget.spend(trace.segments.len() + causes.len());
            causes
        } else {
            Vec::new()
        };
        let error_causes = if reads.error_root_causes {
            let causes = trace.root_causes(RootCauseKind::Error);
            budget.spend(trace.segments.len() + causes.len());
            causes
        } else {
            Vec::new()
        };
        // `HasFault` and `HasError` are the ROOT segment document's and
        // `HasThrottle` is any document's, as `TraceSummary` documents each
        // (see `TraceRecord::root_document_has`), read here off the
        // classifications already derived above.
        let root_at = trace
            .segments
            .iter()
            .position(|segment| segment.parent_id.is_none());
        let root_outcomes: &[Outcome] = root_at.map_or(&[], |at| outcomes[at].as_slice());
        let trace_flags = TraceFlags {
            fault: root_outcomes
                .iter()
                .any(|outcome| outcome.class == Classification::Fault),
            // A 400-series error of the root document, which a throttled
            // request's own 429 status is one of — exactly what
            // `TraceRecord::has_error` reports, so the keyword and the summary
            // cannot disagree.
            error: root_outcomes.iter().any(|outcome| outcome.error_400),
            throttle: outcomes
                .iter()
                .flatten()
                .any(|outcome| outcome.class == Classification::Throttle),
        };
        Self {
            trace,
            outcomes,
            annotations,
            entity_owners,
            trace_flags,
            fault_causes,
            error_causes,
        }
    }

    /// The root causes of `kind`.
    fn root_causes(&self, kind: RootCauseKind) -> &[RootCauseView] {
        match kind {
            RootCauseKind::Fault => &self.fault_causes,
            RootCauseKind::Error => &self.error_causes,
        }
    }
}

/// A trace's own `HasFault` / `HasError` / `HasThrottle`.
#[derive(Debug, Clone, Copy)]
struct TraceFlags {
    fault: bool,
    error: bool,
    throttle: bool,
}

/// One entity's annotations, indexed by key.
fn collect_annotations<'a>(
    at: usize,
    source: &'a BTreeMap<String, serde_json::Value>,
    out: &mut BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>>,
) {
    for (key, value) in source {
        out.entry(key.as_str()).or_default().push((at, value));
    }
}

/// The annotations of a subsegment tree, indexed by key.
///
/// Walked with an explicit worklist rather than by recursion, so the nesting a
/// submitted document carries costs heap and not call stack (the ingest also
/// refuses a document nested deeper than [`super::MAX_SUBSEGMENT_DEPTH`]). The
/// order is the recursion's: a node, then its children left to right, which is
/// the order a key's values are indexed in.
fn walk_annotations<'a>(
    at: usize,
    subsegments: &'a [SubsegmentRecord],
    out: &mut BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>>,
) {
    let mut pending: Vec<&'a SubsegmentRecord> = subsegments.iter().rev().collect();
    while let Some(subsegment) = pending.pop() {
        collect_annotations(at, &subsegment.annotations, out);
        pending.extend(subsegment.subsegments.iter().rev());
    }
}

/// Which of a trace's segments the keywords in hand read.
///
/// Carries the selected indices for iteration AND a membership mask, so a check
/// of the form "is this segment in scope" is a lookup rather than a scan of the
/// selection.
struct Scope {
    /// Whether this is the whole trace: `duration`, `responsetime` and the
    /// `http` block are then the trace's own, as `TraceSummary` reports them.
    whole_trace: bool,
    /// The segment indices in scope.
    indices: Vec<usize>,
    /// Membership by segment index.
    member: Vec<bool>,
}

impl Scope {
    /// Every segment of a trace of `segments` segments.
    fn whole(segments: usize) -> Self {
        Self {
            whole_trace: true,
            indices: (0..segments).collect(),
            member: vec![true; segments],
        }
    }

    /// The segments one scope block selected, out of `segments` in the trace.
    fn selected(segments: usize, indices: Vec<usize>) -> Self {
        let mut member = vec![false; segments];
        for at in &indices {
            if let Some(slot) = member.get_mut(*at) {
                *slot = true;
            }
        }
        Self {
            whole_trace: false,
            indices,
            member,
        }
    }

    fn indices(&self) -> &[usize] {
        &self.indices
    }

    fn contains(&self, at: usize) -> bool {
        self.member.get(at).copied().unwrap_or(false)
    }

    fn is_whole_trace(&self) -> bool {
        self.whole_trace
    }
}

/// Walk the tree. The recursion follows the tree's height, which
/// [`super::filter_expression`]'s bounds cap — at `MAX_DEPTH` for nesting and at
/// one node per two characters of `MAX_LENGTH` for a chained `a OR b OR c …` —
/// so this cannot run off the stack. Every node visit spends from `budget`, so
/// the work one query does over the traces it reads is bounded too.
fn evaluate(node: &Node, facts: &Facts<'_>, scope: &Scope, budget: &mut QueryBudget) -> bool {
    if budget.exhausted() {
        return false;
    }
    budget.spend(1);
    match node {
        Node::And(left, right) => {
            evaluate(left, facts, scope, budget) && evaluate(right, facts, scope, budget)
        }
        Node::Or(left, right) => {
            evaluate(left, facts, scope, budget) || evaluate(right, facts, scope, budget)
        }
        Node::Not(inner) => !evaluate(inner, facts, scope, budget),
        Node::Compare { field, op, value } => {
            budget.spend(scope.indices().len());
            compare(field, *op, value, facts, scope)
        }
        Node::Scoped { selector, inner } => {
            budget.spend(facts.trace.segments.len());
            let selected = select(selector, facts);
            if selected.is_empty() {
                return false;
            }
            match inner {
                None => true,
                Some(inner) => evaluate(
                    inner,
                    facts,
                    &Scope::selected(facts.trace.segments.len(), selected),
                    budget,
                ),
            }
        }
    }
}

/// The segments a scope selector names.
///
/// An `edge(…)` resolves each candidate's `parent_id` through the entity index
/// [`Facts`] built once for the trace, so the selection is one pass over the
/// segments rather than a scan of the trace per segment.
fn select(selector: &Selector, facts: &Facts<'_>) -> Vec<usize> {
    let trace = facts.trace;
    match selector {
        Selector::Service { name, r#type } => trace
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| {
                &segment.name == name
                    && r#type
                        .as_ref()
                        .is_none_or(|wanted| segment.origin.as_ref() == Some(wanted))
            })
            .map(|(at, _)| at)
            .collect(),
        Selector::Edge {
            source,
            destination,
        } => trace
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| {
                &segment.name == destination
                    && segment.parent_id.as_deref().is_some_and(|parent| {
                        facts
                            .entity_owners
                            .get(parent)
                            .is_some_and(|owner| &trace.segments[*owner].name == source)
                    })
            })
            .map(|(at, _)| at)
            .collect(),
    }
}

/// Whether any value this keyword reads in `scope` satisfies the comparison.
fn compare(
    field: &Field,
    op: Operator,
    value: &Literal,
    facts: &Facts<'_>,
    scope: &Scope,
) -> bool {
    let trace = facts.trace;
    let segments = || scope.indices().iter().map(|at| &trace.segments[*at]);
    match field {
        Field::Flag(Flag::Partial) => {
            let partial = segments().any(|segment| segment.in_progress);
            matches_bool(partial, op, value)
        }
        Field::Flag(flag) => {
            // At trace scope a flag is exactly what `TraceSummary` reports, so
            // the keyword and the summary cannot disagree; inside a scope block
            // it is the outcome the scoped entities recorded.
            if scope.is_whole_trace() {
                let held = match flag {
                    Flag::Fault => facts.trace_flags.fault,
                    Flag::Error => facts.trace_flags.error,
                    Flag::Throttle => facts.trace_flags.throttle,
                    // `ok` is the absence of all three anywhere in the trace,
                    // and `partial` is answered by the arm above.
                    Flag::Ok => {
                        let mut outcomes = facts.outcomes.iter().flatten().peekable();
                        outcomes.peek().is_some()
                            && outcomes.all(|outcome| outcome.class == Classification::Ok)
                    }
                    Flag::Partial => false,
                };
                return matches_bool(held, op, value);
            }
            let mut outcomes = scope
                .indices()
                .iter()
                .flat_map(|at| facts.outcomes[*at].iter().copied())
                .peekable();
            let held = match flag {
                Flag::Ok => {
                    outcomes.peek().is_some()
                        && outcomes.all(|outcome| outcome.class == Classification::Ok)
                }
                // As at trace scope: a 400-series error, the 429 of a throttled
                // request included.
                Flag::Error => outcomes.any(|outcome| outcome.error_400),
                Flag::Fault => outcomes.any(|outcome| outcome.class == Classification::Fault),
                Flag::Throttle => {
                    outcomes.any(|outcome| outcome.class == Classification::Throttle)
                }
                Flag::Partial => false,
            };
            matches_bool(held, op, value)
        }
        Field::Duration => {
            let duration = if scope.is_whole_trace() {
                trace.duration()
            } else {
                scoped_duration(trace, scope)
            };
            duration.is_some_and(|duration| matches_num(duration, op, value))
        }
        Field::ResponseTime => {
            let response_time = if scope.is_whole_trace() {
                trace.response_time()
            } else {
                segments().find_map(|segment| Some(segment.end_time? - segment.start_time))
            };
            response_time.is_some_and(|value_seconds| matches_num(value_seconds, op, value))
        }
        Field::HttpStatus => http_segments(trace, scope)
            .iter()
            .filter_map(|segment| segment.http_status())
            .any(|status| matches_num(f64::from(status), op, value)),
        Field::Http(member) => http_segments(trace, scope)
            .iter()
            .filter_map(|segment| {
                let http = segment.http.as_ref()?;
                match member {
                    HttpField::Url => http.url.as_deref(),
                    HttpField::Method => http.http_method.as_deref(),
                    HttpField::UserAgent => http.user_agent.as_deref(),
                    HttpField::ClientIp => http.client_ip.as_deref(),
                }
            })
            .any(|found| matches_str(found, op, value)),
        Field::User => segments()
            .filter_map(|segment| segment.user.as_deref())
            .any(|found| matches_str(found, op, value)),
        Field::ResourceArn => segments()
            .filter_map(|segment| segment.resource_arn.as_deref())
            .any(|found| matches_str(found, op, value)),
        Field::Annotation(key) => facts
            .annotations
            .get(key.as_str())
            .is_some_and(|recorded| {
                recorded
                    .iter()
                    .filter(|(at, _)| scope.contains(*at))
                    .any(|(_, found)| matches_json(found, op, value))
            }),
        Field::RootCause { kind, part } => {
            root_cause_matches(facts.root_causes(*kind), *part, op, value)
        }
    }
}

/// The segments whose `http` block a keyword reads: the trace's own `Http` is
/// its root segment's, as `TraceSummary$Http` reports it, and a scope block
/// reads the scoped segments'.
fn http_segments<'a>(trace: &'a TraceRecord, scope: &Scope) -> Vec<&'a SegmentRecord> {
    if scope.is_whole_trace() {
        return trace.root().into_iter().collect();
    }
    scope
        .indices()
        .iter()
        .map(|at| &trace.segments[*at])
        .collect()
}

/// The span the scoped segments cover.
fn scoped_duration(trace: &TraceRecord, scope: &Scope) -> Option<f64> {
    let mut first = f64::INFINITY;
    let mut last = f64::NEG_INFINITY;
    for at in scope.indices() {
        let segment = &trace.segments[*at];
        first = first.min(segment.start_time);
        last = last.max(segment.end_time.unwrap_or(segment.start_time));
    }
    (first.is_finite() && last.is_finite()).then_some(last - first)
}

/// Whether any service, entity or exception of `causes` satisfies the
/// comparison.
fn root_cause_matches(
    causes: &[RootCauseView],
    part: RootCausePart,
    op: Operator,
    value: &Literal,
) -> bool {
    causes.iter().any(|cause| {
        cause.services.iter().any(|service| match part {
            RootCausePart::ServiceName => matches_str(&service.name, op, value),
            RootCausePart::ServiceType => service
                .r#type
                .as_ref()
                .is_some_and(|found| matches_str(found, op, value)),
            RootCausePart::ServiceInferred => matches_bool(service.inferred, op, value),
            RootCausePart::EntityName => service
                .entity_path
                .iter()
                .any(|entity| matches_str(&entity.name, op, value)),
            RootCausePart::EntityRemote => service
                .entity_path
                .iter()
                .any(|entity| matches_bool(entity.remote, op, value)),
            RootCausePart::ExceptionName => service.entity_path.iter().any(|entity| {
                entity.exceptions.iter().any(|exception| {
                    exception
                        .name
                        .as_ref()
                        .is_some_and(|found| matches_str(found, op, value))
                })
            }),
            RootCausePart::ExceptionMessage => service.entity_path.iter().any(|entity| {
                entity.exceptions.iter().any(|exception| {
                    exception
                        .message
                        .as_ref()
                        .is_some_and(|found| matches_str(found, op, value))
                })
            }),
        })
    })
}

fn matches_bool(found: bool, op: Operator, value: &Literal) -> bool {
    match value {
        Literal::Bool(wanted) => ordered(op, found.cmp(wanted)),
        _ => false,
    }
}

fn matches_num(found: f64, op: Operator, value: &Literal) -> bool {
    match value {
        Literal::Num(wanted) => found
            .partial_cmp(wanted)
            .is_some_and(|ordering| ordered(op, ordering)),
        _ => false,
    }
}

/// A string comparison, including the string operators the language carries
/// beside the six orderings.
fn matches_str(found: &str, op: Operator, value: &Literal) -> bool {
    let Literal::Str(wanted) = value else {
        return false;
    };
    match op {
        Operator::Contains => found.contains(wanted.as_str()),
        Operator::BeginsWith => found.starts_with(wanted.as_str()),
        Operator::EndsWith => found.ends_with(wanted.as_str()),
        other => ordered(other, found.cmp(wanted.as_str())),
    }
}

/// An annotation compares by the type the document stored it as.
fn matches_json(found: &serde_json::Value, op: Operator, value: &Literal) -> bool {
    match found {
        serde_json::Value::String(text) => matches_str(text, op, value),
        serde_json::Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| matches_num(number, op, value)),
        serde_json::Value::Bool(flag) => matches_bool(*flag, op, value),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trace of one complete root segment named `svc`, built through the same
    /// parser `PutTraceSegments` ingests with.
    fn trace() -> TraceRecord {
        let document = r#"{"trace_id":"1-58406520-a006649127e371903a2de979","id":"0000000000000001","name":"svc","start_time":1.0,"end_time":2.0}"#;
        let (trace_id, segment) =
            super::super::parse_segment_document(document).expect("the document is well formed");
        let mut record = TraceRecord {
            id: trace_id,
            first_start: segment.start_time,
            last_end: segment.end_time.unwrap_or(segment.start_time),
            ..TraceRecord::default()
        };
        record.segments.push(segment);
        record
    }

    /// The budget is the WHOLE query's, so a query that has spent it stops
    /// evaluating instead of reading further traces — which is what lets the
    /// handler refuse the request rather than report half a selection.
    #[test]
    fn a_query_that_has_spent_its_budget_stops_evaluating() {
        let expression = parse("service(\"svc\")").expect("the expression is readable");
        let trace = trace();

        let mut budget = QueryBudget::new();
        assert!(
            expression.matches(&trace, &mut budget),
            "the expression selects the trace it matches"
        );
        assert!(
            !budget.exhausted(),
            "one small trace must not spend a whole query's budget"
        );

        budget.spend(MAX_QUERY_STEPS);
        assert!(
            budget.exhausted(),
            "spending the whole budget leaves it exhausted"
        );
        assert!(
            !expression.matches(&trace, &mut budget),
            "a spent budget stops evaluation rather than reading the trace again"
        );
    }

    /// Every trace a query reads spends from the same budget, so the bound is on
    /// the request and not on one trace.
    #[test]
    fn each_trace_a_query_reads_spends_from_the_one_budget() {
        let expression =
            parse("annotation.stage = \"beta\" OR fault OR service(\"svc\")").expect("readable");
        let trace = trace();
        let mut budget = QueryBudget::new();
        expression.matches(&trace, &mut budget);
        let after_one = budget.remaining;
        expression.matches(&trace, &mut budget);
        assert!(
            budget.remaining < after_one,
            "the second trace spends further from the same budget: {} then {}",
            after_one,
            budget.remaining
        );
    }

    /// `AND` binds tighter than `OR`, which the one parser's fold decides and
    /// this module inherits: `fault OR ok AND partial` is
    /// `fault OR (ok AND partial)`, so a trace that is merely `ok` does not
    /// match.
    #[test]
    fn and_binds_tighter_than_or() {
        let trace = trace();
        let mut budget = QueryBudget::new();
        let expression = parse("fault OR ok AND partial").expect("the expression is readable");
        assert!(
            !expression.matches(&trace, &mut budget),
            "the trace is ok but not partial, so the conjunction fails and nothing else matches"
        );
        let grouped = parse("(fault OR ok) AND partial").expect("the expression is readable");
        assert!(
            !grouped.matches(&trace, &mut budget),
            "the grouped form also fails, for the same `partial`"
        );
        let satisfied = parse("fault OR ok AND NOT partial").expect("the expression is readable");
        assert!(
            satisfied.matches(&trace, &mut budget),
            "the trace is ok and not partial, so the conjunction holds"
        );
    }
}

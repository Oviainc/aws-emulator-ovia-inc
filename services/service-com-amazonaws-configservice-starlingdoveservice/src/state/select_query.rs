// SPDX-License-Identifier: CC0-1.0

//! The Config query language: the SQL `SELECT` command a select operation takes, and
//! the configuration-item documents it is run over.
//!
//! `SelectResourceConfig` "accepts a structured query language (SQL) SELECT command,
//! performs the corresponding search, and returns resource configurations matching the
//! properties" (the model doc for the operation), and declares
//! `InvalidExpressionException` — "The syntax of the query is incorrect" — for a command
//! it cannot read. The reading here therefore lives in state rather than in one handler:
//! `SelectAggregateResourceConfig` takes the same command over the aggregated
//! inventory, and a stored query (`PutStoredQuery$Expression`) is the same language
//! again.
//!
//! The command is PARSED, never matched: the `sqlparser` crate reads it, so an
//! expression is judged by the SQL it IS rather than by any text this emulator searches
//! for. What the Config query language accepts on top of being SQL is the documented
//! form of a Config query (the "Query components" section of the Config Developer
//! Guide):
//!
//! ```text
//! SELECT property [, ...] [ WHERE condition ] [ GROUP BY property ] [ ORDER BY property [ASC | DESC] [, ...] ]
//! ```
//!
//! so a command carrying anything that form has no place for — a `FROM` clause, a join,
//! a subquery, a set operation, a `HAVING` clause, a window — is not a Config query and
//! is refused the way a command the parser cannot read at all is. The PROPERTIES are the
//! members of the configuration item the query runs over
//! ([`configuration_item_document`]), which is why nothing here carries a list of field
//! names: a member of `ConfigurationItem` is selectable because the item has it.
//!
//! A command that cannot be read is refused as a SYNTAX ERROR at the position of the
//! submitted expression where the reading stopped ([`stopped_reading_at`]): which parser
//! read the text, and what it made of it, is an implementation detail of this emulator
//! and no part of the service's contract, so none of it travels in the refusal.
//!
//! The work one expression can cause is bounded before the parser is entered:
//! `Expression` is `@length(1, 4096)`, which the boundary validator enforces before any
//! handler runs, and [`MAX_EXPRESSION_DEPTH`] and [`MAX_EXPRESSION_TOKENS`] bound the
//! DEPTH of the tree that expression can ask for — in the parser, and in the drop of the
//! tree it builds.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sqlparser::ast::{
    BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr, Query,
    Select, SelectItem, SetExpr, Statement, UnaryOperator,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer};

use super::{ComAmazonawsConfigserviceStarlingdoveserviceState, ConfigurationAggregatorEntry};
use crate::ops::types::{ConfigurationItem, FieldInfo, QueryInfo};
/// The deepest BRACKETING one `Expression` may carry, counted before the parser is
/// entered.
///
/// This is the EMULATOR's own bound, and it is what keeps a caller-controlled expression
/// from turning into a deep recursion — in the parser, or in the DROP of the tree it
/// builds, which descends with that tree whatever the parser's own recursion limit
/// refused. A Config query of the documented form brackets a handful of levels (a
/// parenthesised `WHERE` over an `IN (…)` is two or three), so 32 is an order of
/// magnitude above any such query and far below a descent that could reach the end of a
/// worker thread's stack.
pub const MAX_EXPRESSION_DEPTH: usize = 32;

/// The most TOKENS one `Expression` may carry.
///
/// The general bound, for what the nesting brackets do NOT express: a chain of binary
/// operators nests the TREE at bracket depth zero, and the drop of that tree descends
/// with it. The widest Config query the documented form writes is a few dozen tokens, so
/// 2048 is far above it and still unwinds inside a worker thread's ordinary stack.
pub const MAX_EXPRESSION_TOKENS: usize = 2_048;

/// How many results one page of a select answers when the request does not ask for a
/// page size — `Limit` defaults to 0, which is "unspecified" rather than "no results".
/// 100 is `Limit`'s own maximum (range 0..100), so it is the largest page the model
/// lets one response carry.
pub const SELECT_RESULTS_PAGE_SIZE: usize = 100;

/// The aggregate functions a Config query can select
/// (the Config Developer Guide's "Query components": "AVG, COUNT, MAX, MIN, SUM").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    Avg,
    Count,
    Max,
    Min,
    Sum,
}

impl Aggregate {
    /// The function `name` names, if it names one. Matched case-insensitively, the way
    /// SQL names a function.
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "AVG" => Some(Self::Avg),
            "COUNT" => Some(Self::Count),
            "MAX" => Some(Self::Max),
            "MIN" => Some(Self::Min),
            "SUM" => Some(Self::Sum),
            _ => None,
        }
    }

    /// How the function is spelled in the name of the field it produces.
    fn spelling(self) -> &'static str {
        match self {
            Self::Avg => "AVG",
            Self::Count => "COUNT",
            Self::Max => "MAX",
            Self::Min => "MIN",
            Self::Sum => "SUM",
        }
    }
}

/// One item a query selects.
#[derive(Debug, Clone)]
enum Field {
    /// Every member of the configuration item (`SELECT *`).
    Everything,
    /// One property of the configuration item, as the path of member names that reaches
    /// it (`resourceId`, `configuration.state`).
    Property(Vec<String>),
    /// An aggregate over the items the query matched.
    Aggregated {
        function: Aggregate,
        /// The property the function is applied to, or `None` for `COUNT(*)`.
        property: Option<Vec<String>>,
    },
}

impl Field {
    /// The name this field is reported under, in `QueryInfo$SelectFields` and as the
    /// member of the result document: the property path as it was written, or the
    /// function applied to it.
    fn name(&self) -> String {
        match self {
            Self::Everything => "*".to_string(),
            Self::Property(path) => path.join("."),
            Self::Aggregated { function, property } => {
                let argument = match property {
                    None => "*".to_string(),
                    Some(path) => path.join("."),
                };
                format!("{}({argument})", function.spelling())
            }
        }
    }
}

/// How two values are compared by a `WHERE` condition.
#[derive(Debug, Clone, Copy)]
enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

/// One condition of a query's `WHERE` clause.
#[derive(Debug, Clone)]
enum Condition {
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Not(Box<Condition>),
    Compare {
        property: Vec<String>,
        comparison: Comparison,
        value: Value,
    },
    Between {
        property: Vec<String>,
        low: Value,
        high: Value,
        negated: bool,
    },
    In {
        property: Vec<String>,
        values: Vec<Value>,
        negated: bool,
    },
    Like {
        property: Vec<String>,
        pattern: String,
        negated: bool,
    },
    IsNull {
        property: Vec<String>,
        negated: bool,
    },
}

/// One `ORDER BY` key: the property or selected field to order by, and whether the
/// ordering descends.
#[derive(Debug, Clone)]
struct OrderKey {
    property: Vec<String>,
    descending: bool,
}

/// A Config query, read off an `Expression`.
#[derive(Debug, Clone)]
pub struct SelectQuery {
    fields: Vec<Field>,
    condition: Option<Condition>,
    group_by: Vec<Vec<String>>,
    order_by: Vec<OrderKey>,
    /// A `LIMIT` the expression itself carries, which bounds the rows the query answers
    /// however large a page the request asks for.
    limit: Option<usize>,
}

impl SelectQuery {
    /// The names of the fields this query selects, in the order it selected them —
    /// `QueryInfo$SelectFields`, "Returns a `FieldInfo` object" per selected field.
    pub fn field_names(&self) -> Vec<String> {
        self.fields.iter().map(Field::name).collect()
    }

    /// Whether this query aggregates: one that does answers one row per group rather
    /// than one row per matched configuration item.
    pub fn aggregates(&self) -> bool {
        !self.group_by.is_empty()
            || self
                .fields
                .iter()
                .any(|field| matches!(field, Field::Aggregated { .. }))
    }

    /// Run the query over `documents` — the configuration items it searches — and answer
    /// the result documents, in the order the query orders them.
    pub fn run(&self, documents: &[Value]) -> Vec<Value> {
        let matched: Vec<&Value> = documents
            .iter()
            .filter(|document| match &self.condition {
                None => true,
                Some(condition) => condition.matches(document),
            })
            .collect();

        let mut results = if self.aggregates() {
            self.aggregate(&matched)
        } else {
            matched
                .iter()
                .map(|document| self.project(document))
                .collect()
        };

        for key in self.order_by.iter().rev() {
            // A stable sort per key, least significant key first, orders by the whole
            // key list: equal rows keep the order the earlier keys put them in.
            results.sort_by(|left, right| {
                let ordering = compare_values(
                    value_at(left, &key.property).first().copied(),
                    value_at(right, &key.property).first().copied(),
                );
                if key.descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            });
        }

        if let Some(limit) = self.limit {
            results.truncate(limit);
        }
        results
    }

    /// One result document for `document`: the properties the query selected, each
    /// under the member path it was selected by.
    fn project(&self, document: &Value) -> Value {
        let mut result = Map::new();
        for field in &self.fields {
            match field {
                // Every member of the configuration item, as the item has them.
                Field::Everything => {
                    if let Value::Object(members) = document {
                        for (member, value) in members {
                            result.insert(member.clone(), value.clone());
                        }
                    }
                }
                Field::Property(path) => {
                    if let Some(value) = value_at(document, path).first() {
                        insert_at(&mut result, path, (*value).clone());
                    }
                }
                // An aggregate cannot be projected per item; `run` sends an aggregating
                // query to `aggregate` instead.
                Field::Aggregated { .. } => {}
            }
        }
        Value::Object(result)
    }

    /// One result document per group of matched items: the grouped properties and the
    /// aggregates over the items of that group.
    ///
    /// A query that aggregates without grouping has ONE group holding every matched
    /// item, which is what `SELECT COUNT(*)` answers.
    fn aggregate(&self, matched: &[&Value]) -> Vec<Value> {
        let mut groups: Vec<(Vec<Option<Value>>, Vec<&Value>)> = Vec::new();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        for document in matched {
            let key: Vec<Option<Value>> = self
                .group_by
                .iter()
                .map(|path| value_at(document, path).first().map(|value| (*value).clone()))
                .collect();
            let indexed = serde_json::to_string(&key).unwrap_or_default();
            match index.get(&indexed) {
                Some(at) => groups[*at].1.push(document),
                None => {
                    index.insert(indexed, groups.len());
                    groups.push((key, vec![document]));
                }
            }
        }
        if groups.is_empty() && self.group_by.is_empty() {
            // Nothing matched and nothing groups the results: the aggregate over no
            // items is still one row (`COUNT(*)` of nothing is zero).
            groups.push((Vec::new(), Vec::new()));
        }

        groups
            .into_iter()
            .map(|(key, members)| {
                let mut result = Map::new();
                for field in &self.fields {
                    match field {
                        Field::Property(path) => {
                            // A selected property of an aggregating query is one the
                            // query groups by, so its value is the group's own.
                            if let Some(Some(value)) = self
                                .group_by
                                .iter()
                                .position(|grouped| grouped == path)
                                .map(|at| key.get(at).cloned().flatten())
                            {
                                insert_at(&mut result, path, value);
                            }
                        }
                        Field::Aggregated { function, property } => {
                            if let Some(value) = aggregate_over(*function, property, &members) {
                                result.insert(field.name(), value);
                            }
                        }
                        // `SELECT *` cannot be aggregated; `read_expression` refuses it
                        // beside an aggregate.
                        Field::Everything => {}
                    }
                }
                Value::Object(result)
            })
            .collect()
    }
}

/// The value of `function` over `members`, or `None` when the group holds no value the
/// function applies to.
fn aggregate_over(
    function: Aggregate,
    property: &Option<Vec<String>>,
    members: &[&Value],
) -> Option<Value> {
    // `COUNT(*)` counts the items themselves; every other function reads one property
    // of each item, and an item the property is absent from contributes nothing.
    let Some(path) = property else {
        return (function == Aggregate::Count)
            .then(|| Value::from(u64::try_from(members.len()).unwrap_or_default()));
    };
    let values: Vec<Value> = members
        .iter()
        .filter_map(|document| value_at(document, path).first().map(|value| (*value).clone()))
        .filter(|value| !value.is_null())
        .collect();
    if function == Aggregate::Count {
        return Some(Value::from(u64::try_from(values.len()).unwrap_or_default()));
    }
    if values.is_empty() {
        return None;
    }
    match function {
        Aggregate::Min => values
            .into_iter()
            .min_by(|left, right| compare_values(Some(left), Some(right))),
        Aggregate::Max => values
            .into_iter()
            .max_by(|left, right| compare_values(Some(left), Some(right))),
        Aggregate::Sum | Aggregate::Avg => {
            let numbers: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
            if numbers.is_empty() {
                return None;
            }
            let sum: f64 = numbers.iter().sum();
            let total = if function == Aggregate::Sum {
                sum
            } else {
                sum / numbers.len() as f64
            };
            serde_json::Number::from_f64(total).map(Value::Number)
        }
        Aggregate::Count => None,
    }
}

impl Condition {
    /// Whether `document` satisfies this condition.
    fn matches(&self, document: &Value) -> bool {
        match self {
            Self::And(left, right) => left.matches(document) && right.matches(document),
            Self::Or(left, right) => left.matches(document) || right.matches(document),
            Self::Not(inner) => !inner.matches(document),
            // A condition on a property holds when ANY value at that path satisfies it:
            // a path reaching into a list of members (a relationship, a tag) is
            // satisfied by one element, the way a Config query over a list property is.
            Self::Compare {
                property,
                comparison,
                value,
            } => value_at(document, property)
                .into_iter()
                .any(|candidate| comparison.holds(candidate, value)),
            Self::Between {
                property,
                low,
                high,
                negated,
            } => {
                let inside = value_at(document, property).into_iter().any(|candidate| {
                    Comparison::GreaterOrEqual.holds(candidate, low)
                        && Comparison::LessOrEqual.holds(candidate, high)
                });
                inside != *negated
            }
            Self::In {
                property,
                values,
                negated,
            } => {
                let found = value_at(document, property).into_iter().any(|candidate| {
                    values
                        .iter()
                        .any(|value| Comparison::Equal.holds(candidate, value))
                });
                found != *negated
            }
            Self::Like {
                property,
                pattern,
                negated,
            } => {
                let matched = value_at(document, property)
                    .into_iter()
                    .filter_map(Value::as_str)
                    .any(|candidate| like_matches(candidate, pattern));
                matched != *negated
            }
            Self::IsNull { property, negated } => {
                let present = value_at(document, property)
                    .into_iter()
                    .any(|value| !value.is_null());
                present == *negated
            }
        }
    }
}

impl Comparison {
    /// Whether `candidate` stands in this relation to `value`.
    ///
    /// Two values compare only when they are of the same kind — a number against a
    /// number, a string against a string, a boolean against a boolean — which is why an
    /// ordering against a value of another kind holds for nothing rather than ordering
    /// the kinds against each other.
    fn holds(self, candidate: &Value, value: &Value) -> bool {
        let ordering = match (candidate, value) {
            (Value::Number(left), Value::Number(right)) => {
                match (left.as_f64(), right.as_f64()) {
                    (Some(left), Some(right)) => left.partial_cmp(&right),
                    _ => None,
                }
            }
            (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
            (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
            (Value::Null, Value::Null) => Some(::std::cmp::Ordering::Equal),
            _ => None,
        };
        let Some(ordering) = ordering else {
            // Values that do not compare are not equal either, so only `!=` holds.
            return matches!(self, Self::NotEqual);
        };
        match self {
            Self::Equal => ordering.is_eq(),
            Self::NotEqual => !ordering.is_eq(),
            Self::Less => ordering.is_lt(),
            Self::LessOrEqual => ordering.is_le(),
            Self::Greater => ordering.is_gt(),
            Self::GreaterOrEqual => ordering.is_ge(),
        }
    }
}

/// The values of `document` at `path`: the member reached by following the path, and —
/// where the path passes through a list — the member of each element of that list.
///
/// An absent member yields no value at all, which is what makes `IS NULL` and the
/// absence of a property the same question.
fn value_at<'a>(document: &'a Value, path: &[String]) -> Vec<&'a Value> {
    let mut current: Vec<&Value> = vec![document];
    for member in path {
        let mut next: Vec<&Value> = Vec::new();
        for value in current {
            match value {
                Value::Object(members) => next.extend(members.get(member)),
                Value::Array(items) => {
                    for item in items {
                        if let Value::Object(members) = item {
                            next.extend(members.get(member));
                        }
                    }
                }
                _ => {}
            }
        }
        current = next;
    }
    // A list reached by the whole path stands for its own elements, so a condition on
    // it is a condition on any element.
    let mut values: Vec<&Value> = Vec::new();
    for value in current {
        match value {
            Value::Array(items) => values.extend(items.iter()),
            other => values.push(other),
        }
    }
    values
}

/// Place `value` in `result` under `path`, creating the intermediate objects a nested
/// property is reported under: `configuration.state` is reported as
/// `{"configuration": {"state": …}}`, the way a nested property of a configuration item
/// is nested in the item itself.
fn insert_at(result: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((last, leading)) = path.split_last() else {
        return;
    };
    let mut current = result;
    for member in leading {
        let entry = current
            .entry(member.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = match entry {
            Value::Object(members) => members,
            _ => return,
        };
    }
    current.insert(last.clone(), value);
}

/// Order two values for an `ORDER BY` key: numbers numerically, strings
/// lexicographically, and a row the key is absent from after every row that has it.
fn compare_values(left: Option<&Value>, right: Option<&Value>) -> ::std::cmp::Ordering {
    use ::std::cmp::Ordering;
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => match (left, right) {
            (Value::Number(left), Value::Number(right)) => left
                .as_f64()
                .partial_cmp(&right.as_f64())
                .unwrap_or(Ordering::Equal),
            (Value::String(left), Value::String(right)) => left.cmp(right),
            (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
            // Values of different kinds have no ordering of their own; comparing their
            // renderings keeps the sort total and deterministic.
            _ => left.to_string().cmp(&right.to_string()),
        },
    }
}

/// Whether `candidate` matches a SQL `LIKE` `pattern`, in which `%` stands for any run
/// of characters (including none) and `_` for exactly one character.
///
/// Matched over CHARACTERS rather than bytes, so a multi-byte character is one `_`.
fn like_matches(candidate: &str, pattern: &str) -> bool {
    let text: Vec<char> = candidate.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    // The classic two-cursor match: `star` remembers the last `%` and how much of the
    // text it had consumed, so a failed match after one backtracks to consume one more
    // character. Linear in the text per `%`, and it allocates nothing.
    let (mut at_text, mut at_pattern) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while at_text < text.len() {
        if at_pattern < pattern.len()
            && (pattern[at_pattern] == '_' || pattern[at_pattern] == text[at_text])
        {
            at_text += 1;
            at_pattern += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '%' {
            star = Some((at_pattern, at_text));
            at_pattern += 1;
        } else if let Some((at_star, consumed)) = star {
            at_pattern = at_star + 1;
            at_text = consumed + 1;
            star = Some((at_star, consumed + 1));
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|token| *token == '%')
}

/// Why an `Expression` is refused, as the message `InvalidExpressionException` carries.
///
/// The one error code stands for every refusal of a Config query, so the message is what
/// tells a caller which refusal it got, and the two kinds say different things:
///
/// * a command this service cannot READ as a Config query is a SYNTAX error, and the
///   message reports WHERE in the submitted expression the reading stopped
///   ([`syntax_error_message`]): that position is the actionable part of such a refusal,
///   and it is the whole of it — which parser read the text, and what it made of it, is
///   an implementation detail of this emulator and no part of the contract, so the
///   readers below carry no prose of their own;
/// * a command that IS read and breaks a RULE of the query language carries the sentence
///   that rule is worded with ([`Self::broken_rule`]) — the one that names the fields
///   which broke the GROUP BY coverage rule, and otherwise the single sentence a query
///   the engine will not run is refused with ([`QUERY_IS_INVALID_MESSAGE`]).
#[derive(Debug, Clone)]
pub struct ExpressionRefusal(String);

impl ExpressionRefusal {
    /// The refusal of a command that could not be read as a Config query: a syntax error
    /// at the position of the submitted expression where the reading stopped.
    fn syntax(stopped_at: Position) -> Self {
        Self(syntax_error_message(stopped_at))
    }

    /// The refusal of a command that is read and breaks a rule of the query language:
    /// `message` is the whole sentence that rule is refused with.
    fn broken_rule(message: String) -> Self {
        Self(message)
    }

    /// The message the refusal is answered with.
    pub fn message(self) -> String {
        self.0
    }
}

/// A command one reading could not READ as a Config query.
///
/// It carries nothing: such a command is refused as a syntax error AT a position of the
/// submitted expression, and the position is computed by [`read_expression`] — the only
/// reader holding the whole command — so a reader that finds the fault inside one clause
/// of it has nothing to add.
#[derive(Debug, Clone, Copy)]
struct Unreadable;

/// Why one reading did not produce a Config query: the command could not be READ as one,
/// or it was read and breaks a RULE of the query language, which has its own sentence.
#[derive(Debug, Clone)]
enum Unread {
    Unreadable,
    BrokenRule(String),
}

/// A reading that only says the command is unreadable is the common case, so `?` lifts
/// one into the refusal it is reported under.
impl From<Unreadable> for Unread {
    fn from(_: Unreadable) -> Self {
        Self::Unreadable
    }
}

/// A position in a submitted `Expression`: a line and a column, both counted from one,
/// as Config reports where it stopped reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    line: u64,
    column: u64,
}

impl Position {
    /// The first position of any expression.
    const START: Self = Self { line: 1, column: 1 };

    /// The position after `character`, which this one is the position of.
    fn after(self, character: char) -> Self {
        if character == '\n' {
            Self {
                line: self.line + 1,
                column: 1,
            }
        } else {
            Self {
                line: self.line,
                column: self.column + 1,
            }
        }
    }
}

/// The tokenizer reports a token's position as its own [`Location`], counted from one on
/// both axes — the same way this one is.
impl From<Location> for Position {
    fn from(location: Location) -> Self {
        Self {
            line: location.line,
            column: location.column,
        }
    }
}

/// How Config words a command it could not read as a Config query: a syntax error at the
/// position the reading stopped at.
///
/// BUILT FROM THE REQUEST, and it is the actionable part of the refusal — a caller fixing
/// a query needs to know WHERE in the expression it sent the reading stopped. That
/// position is all the sentence carries: no prose of the reader, and no text of the
/// expression itself.
fn syntax_error_message(stopped_at: Position) -> String {
    format!(
        "syntax error at line {}, column {}",
        stopped_at.line, stopped_at.column
    )
}

/// How Config words the GROUP BY coverage rule of the query language.
///
/// A query that aggregates answers one row per GROUP, so a property it selects that the
/// GROUP BY list does not carry has no single value to answer for a group. The message
/// names the RULE and is BUILT FROM THE REQUEST: it lists the selected fields the GROUP
/// BY list does not carry, in the order the query selected them, so a caller knows which
/// fields to group by or to drop.
fn ungrouped_selected_fields_message(fields: &[String]) -> String {
    format!(
        "when a GROUP BY clause or Aggregate Function (AVG|COUNT|MAX|MIN|SUM) is present, \
         selected fields ({}) must appear in the GROUP BY field list",
        fields.join(", ")
    )
}

/// The members a configuration item carries, by the name a Config query selects them
/// with (`smithy_query(query="shape", shape="ConfigurationItem")`).
///
/// NOT ONE of them is a number: the shape declares strings, enums, timestamps, a map and
/// two lists, and the `configuration` member is a JSON-encoded document whose own members
/// the resource type defines rather than this model. The numbers an arithmetic aggregate
/// can compute over are therefore only ever INSIDE that document, never one of these
/// members.
const CONFIGURATION_ITEM_MEMBERS: [&str; 20] = [
    "accountId",
    "arn",
    "availabilityZone",
    "awsRegion",
    "configuration",
    "configurationItemCaptureTime",
    "configurationItemDeliveryTime",
    "configurationItemMD5Hash",
    "configurationItemStatus",
    "configurationStateId",
    "recordingFrequency",
    "relatedEvents",
    "relationships",
    "resourceCreationTime",
    "resourceId",
    "resourceName",
    "resourceType",
    "supplementaryConfiguration",
    "tags",
    "version",
];

/// The ONE sentence Config refuses a query it READ but will not RUN with.
///
/// Fixed, and it carries no part of the request: a rule of the query language that AWS
/// words itself ([`ungrouped_selected_fields_message`]) says which fields broke it, and
/// every other rule of the language carries no more than this.
const QUERY_IS_INVALID_MESSAGE: &str = "Query is invalid.";

/// How Config words a selected item that names no field of the configuration item.
///
/// A field is named by a path of member names, so a selected item that is not one is read
/// as a field NAME carrying characters a name cannot carry — the brackets of a function
/// the language does not have, for instance. BUILT FROM THE REQUEST: it quotes the item
/// as the query wrote it, which is the only way a caller can tell which of several
/// selected items it has to fix.
fn invalid_field_name_message(name: &str) -> String {
    format!("field name {name} contains invalid characters")
}

/// Whether the schema of a configuration item declares `path` as something other than a
/// number — which it does for every member it declares at all
/// ([`CONFIGURATION_ITEM_MEMBERS`]).
///
/// A path THROUGH a member (a tag, a member of the `configuration` document) is one the
/// schema does not type, so nothing is known about it here and nothing is refused for it:
/// those are exactly the properties a resource type's own configuration carries numbers
/// in.
fn declared_non_numeric(path: &[String]) -> bool {
    matches!(path, [member] if CONFIGURATION_ITEM_MEMBERS.contains(&member.as_str()))
}

/// Read `expression` as a Config query, or answer why it is not one
/// ([`ExpressionRefusal`]).
pub fn read_expression(expression: &str) -> Result<SelectQuery, ExpressionRefusal> {
    if let Some(stopped_at) = passes_a_bound(expression) {
        return Err(ExpressionRefusal::syntax(stopped_at));
    }
    match read_command(expression) {
        Ok(query) => Ok(query),
        Err(Unread::BrokenRule(message)) => Err(ExpressionRefusal::broken_rule(message)),
        // A command that could not be read is a syntax error at the position the reading
        // stopped at, which only a reader holding the WHOLE command can say.
        Err(Unread::Unreadable) => Err(ExpressionRefusal::syntax(stopped_reading_at(expression))),
    }
}

/// Read `expression` as a Config query, or say why that reading did not produce one.
///
/// The bounds [`read_expression`] checks are NOT checked here: this is also how a PREFIX
/// of a command is read ([`stopped_reading_at`]), and a prefix of an expression that
/// passed them passes them too.
fn read_command(expression: &str) -> Result<SelectQuery, Unread> {
    let statements =
        Parser::parse_sql(&GenericDialect {}, expression).map_err(|_| Unread::Unreadable)?;
    // Exactly one SQL SELECT command, and nothing else.
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(Unread::Unreadable);
    };
    read_query(query)
}

/// Read one parsed SQL query as a Config query.
fn read_query(query: &Query) -> Result<SelectQuery, Unread> {
    // A Config query carries no WITH clause, and is a single SELECT rather than a set
    // operation or a parenthesised query.
    if query.with.is_some() {
        return Err(Unread::Unreadable);
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(Unread::Unreadable);
    };
    let select: &Select = select.as_ref();
    // A Config query selects from the resources Config has recorded, so it carries no
    // FROM clause — nor a HAVING or a DISTINCT one, which the documented form of a Config
    // query has no place for either.
    if !select.from.is_empty() || select.having.is_some() || select.distinct.is_some() {
        return Err(Unread::Unreadable);
    }

    let mut fields: Vec<Field> = Vec::new();
    for item in &select.projection {
        fields.push(read_field(item)?);
    }
    // A Config query selects at least one property.
    if fields.is_empty() {
        return Err(Unread::Unreadable);
    }

    let group_by = match &select.group_by {
        GroupByExpr::Expressions(expressions, modifiers) if modifiers.is_empty() => {
            let mut grouped: Vec<Vec<String>> = Vec::new();
            for expression in expressions {
                // A Config query groups by properties of the configuration item.
                grouped.push(property_of(expression).ok_or(Unreadable)?);
            }
            grouped
        }
        _ => return Err(Unread::Unreadable),
    };

    let condition = match &select.selection {
        None => None,
        Some(expression) => Some(read_condition(expression, 0)?),
    };

    let order_by = read_order_by(query)?;
    let limit = read_limit(query)?;

    let aggregates = !group_by.is_empty()
        || fields
            .iter()
            .any(|field| matches!(field, Field::Aggregated { .. }));
    if aggregates {
        // A query that aggregates answers one row per group, and `*` is every member of
        // an ITEM: there is no group whose members it could name.
        if fields
            .iter()
            .any(|field| matches!(field, Field::Everything))
        {
            return Err(Unread::BrokenRule(QUERY_IS_INVALID_MESSAGE.to_string()));
        }
        // Every property this query selects has to be one it groups by, and the refusal
        // names ALL of those that are not, in the order they were selected: the rule is
        // about the projection as a whole, so a caller fixing one field at a time would
        // need one refusal per field to learn the same thing.
        let ungrouped: Vec<String> = fields
            .iter()
            .filter_map(|field| match field {
                Field::Property(path) if !group_by.iter().any(|grouped| grouped == path) => {
                    Some(path.join("."))
                }
                _ => None,
            })
            .collect();
        if !ungrouped.is_empty() {
            return Err(Unread::BrokenRule(ungrouped_selected_fields_message(
                &ungrouped,
            )));
        }

        // AVG and SUM compute over NUMBERS, so a field the configuration item schema
        // declares as something else carries nothing either of them can add up: that
        // query is refused rather than answered with a group the function is simply
        // missing from. COUNT counts values of any type and MIN and MAX order them, so
        // neither is bound by this.
        for field in &fields {
            if let Field::Aggregated {
                function,
                property: Some(path),
            } = field
            {
                if matches!(function, Aggregate::Avg | Aggregate::Sum)
                    && declared_non_numeric(path)
                {
                    return Err(Unread::BrokenRule(QUERY_IS_INVALID_MESSAGE.to_string()));
                }
            }
        }
    }

    // An ORDER BY of an aggregating query orders the GROUPS, so it can only name what
    // such a result carries: a grouped property, or a selected aggregate.
    if aggregates {
        let names: Vec<String> = fields.iter().map(Field::name).collect();
        for key in &order_by {
            let path = key.property.join(".");
            if !group_by.contains(&key.property) && !names.contains(&path) {
                return Err(Unread::BrokenRule(QUERY_IS_INVALID_MESSAGE.to_string()));
            }
        }
    }

    Ok(SelectQuery {
        fields,
        condition,
        group_by,
        order_by,
        limit,
    })
}

/// Read one selected item as a field of a Config query.
fn read_field(item: &SelectItem) -> Result<Field, Unread> {
    match item {
        SelectItem::Wildcard(_) => Ok(Field::Everything),
        SelectItem::UnnamedExpr(expression) => read_field_expression(expression),
        // A Config query selects properties of ONE configuration item: they carry no
        // alias, and a qualified wildcard names nothing.
        SelectItem::ExprWithAlias { .. } | SelectItem::QualifiedWildcard(..) => {
            Err(Unread::Unreadable)
        }
    }
}

/// Read one selected expression: a property of the configuration item, or an aggregate
/// over the items the query matched.
fn read_field_expression(expression: &Expr) -> Result<Field, Unread> {
    if let Expr::Function(function) = expression {
        let name = function
            .name
            .0
            .iter()
            .map(|part| part.to_string())
            .collect::<Vec<String>>()
            .join(".");
        let Some(aggregate) = Aggregate::parse(&name) else {
            // The language has no such function, so what was selected is not a call at
            // all: it is a FIELD NAME, and a field is named by a path of member names,
            // which the brackets of the call are not part of.
            return Err(Unread::BrokenRule(invalid_field_name_message(
                &expression.to_string(),
            )));
        };
        // An aggregate of the language takes exactly one argument.
        let FunctionArguments::List(arguments) = &function.args else {
            return Err(Unread::Unreadable);
        };
        let [argument] = arguments.args.as_slice() else {
            return Err(Unread::Unreadable);
        };
        let property = match argument {
            FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => None,
            FunctionArg::Unnamed(FunctionArgExpr::Expr(inner)) => {
                Some(property_of(inner).ok_or(Unreadable)?)
            }
            // An aggregate of a Config query takes a property of the configuration item,
            // or `*` for COUNT.
            _ => return Err(Unread::Unreadable),
        };
        if property.is_none() && aggregate != Aggregate::Count {
            return Err(Unread::Unreadable);
        }
        return Ok(Field::Aggregated {
            function: aggregate,
            property,
        });
    }
    // A Config query selects properties of the configuration item, aggregates over them,
    // or `*`.
    property_of(expression)
        .map(Field::Property)
        .ok_or(Unread::Unreadable)
}

/// The words the Config query language's own grammar is written with — the clause
/// keywords of `SELECT property [, ...] [WHERE condition] [GROUP BY property] [ORDER BY
/// property]` and the operators a condition uses.
///
/// An UNQUOTED identifier that is one of them names no property: an expression that
/// leaves one standing in a property position is a query its writer got wrong (`SELECT
/// WHERE`), not a selection of a member called `WHERE`, and SQL's own parser cannot tell
/// the two apart because every one of these is a legal column name in SQL.
const QUERY_LANGUAGE_KEYWORDS: [&str; 17] = [
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "BY", "LIMIT", "AND", "OR", "NOT", "IN", "LIKE",
    "BETWEEN", "IS", "NULL", "ASC", "DESC",
];

/// The property path `expression` names, if it names one: a member of the configuration
/// item, or a member reached through the members before it.
fn property_of(expression: &Expr) -> Option<Vec<String>> {
    let named = |identifier: &::sqlparser::ast::Ident| {
        let keyword = identifier.quote_style.is_none()
            && QUERY_LANGUAGE_KEYWORDS.contains(&identifier.value.to_ascii_uppercase().as_str());
        (!keyword).then(|| identifier.value.clone())
    };
    match expression {
        Expr::Identifier(identifier) => named(identifier).map(|member| vec![member]),
        Expr::CompoundIdentifier(identifiers) => {
            identifiers.iter().map(named).collect::<Option<Vec<String>>>()
        }
        Expr::Nested(inner) => property_of(inner),
        _ => None,
    }
}

/// Read a `WHERE` expression as a Config query's condition.
///
/// `depth` is how deep this reading already is, counted on the TREE rather than on the
/// brackets: a chain of `AND`s nests the tree without nesting a single bracket, so
/// [`MAX_EXPRESSION_DEPTH`] is enforced here as well as on the bytes. It bounds the
/// recursion of this reading, of the drop of the condition it builds, and of
/// [`Condition::matches`], which descends the same tree once per configuration item.
fn read_condition(expression: &Expr, depth: usize) -> Result<Condition, Unreadable> {
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(Unreadable);
    }
    let depth = depth + 1;
    match expression {
        Expr::Nested(inner) => read_condition(inner, depth),
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::And => Ok(Condition::And(
                Box::new(read_condition(left, depth)?),
                Box::new(read_condition(right, depth)?),
            )),
            BinaryOperator::Or => Ok(Condition::Or(
                Box::new(read_condition(left, depth)?),
                Box::new(read_condition(right, depth)?),
            )),
            BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Lt
            | BinaryOperator::LtEq
            | BinaryOperator::Gt
            | BinaryOperator::GtEq => {
                let comparison = match op {
                    BinaryOperator::Eq => Comparison::Equal,
                    BinaryOperator::NotEq => Comparison::NotEqual,
                    BinaryOperator::Lt => Comparison::Less,
                    BinaryOperator::LtEq => Comparison::LessOrEqual,
                    BinaryOperator::Gt => Comparison::Greater,
                    _ => Comparison::GreaterOrEqual,
                };
                // A condition compares a PROPERTY against a value, in either order, so
                // `'x' = resourceId` reads as `resourceId = 'x'`.
                if let (Some(property), Some(value)) = (property_of(left), literal_of(right)) {
                    return Ok(Condition::Compare {
                        property,
                        comparison,
                        value,
                    });
                }
                if let (Some(value), Some(property)) = (literal_of(left), property_of(right)) {
                    let comparison = match comparison {
                        Comparison::Less => Comparison::Greater,
                        Comparison::LessOrEqual => Comparison::GreaterOrEqual,
                        Comparison::Greater => Comparison::Less,
                        Comparison::GreaterOrEqual => Comparison::LessOrEqual,
                        other => other,
                    };
                    return Ok(Condition::Compare {
                        property,
                        comparison,
                        value,
                    });
                }
                // A condition of a Config query compares a property of the configuration
                // item against a value.
                Err(Unreadable)
            }
            // Any other operator is not one a condition of a Config query uses.
            _ => Err(Unreadable),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => Ok(Condition::Not(Box::new(read_condition(expr, depth)?))),
        Expr::IsNull(inner) => Ok(Condition::IsNull {
            property: property_of(inner).ok_or(Unreadable)?,
            negated: false,
        }),
        Expr::IsNotNull(inner) => Ok(Condition::IsNull {
            property: property_of(inner).ok_or(Unreadable)?,
            negated: true,
        }),
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let property = property_of(expr).ok_or(Unreadable)?;
            let mut values = Vec::new();
            for item in list {
                // An IN list of a Config query carries values.
                values.push(literal_of(item).ok_or(Unreadable)?);
            }
            Ok(Condition::In {
                property,
                values,
                negated: *negated,
            })
        }
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } => Ok(Condition::Between {
            property: property_of(expr).ok_or(Unreadable)?,
            // A BETWEEN of a Config query carries values.
            low: literal_of(low).ok_or(Unreadable)?,
            high: literal_of(high).ok_or(Unreadable)?,
            negated: *negated,
        }),
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        } => {
            let property = property_of(expr).ok_or(Unreadable)?;
            // A LIKE of a Config query carries a pattern.
            let pattern = literal_of(pattern)
                .and_then(|value| value.as_str().map(str::to_string))
                .ok_or(Unreadable)?;
            Ok(Condition::Like {
                property,
                pattern,
                negated: *negated,
            })
        }
        // Anything else is no condition of the Config query language. A condition that
        // does not name a property of the configuration item is not one either, which is
        // what every reading above refuses.
        _ => Err(Unreadable),
    }
}

/// The value `expression` is, if it is one: a string, a number, a boolean or NULL.
fn literal_of(expression: &Expr) -> Option<Value> {
    match expression {
        Expr::Nested(inner) => literal_of(inner),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => literal_of(expr).and_then(|value| {
            value
                .as_f64()
                .and_then(|number| serde_json::Number::from_f64(-number))
                .map(Value::Number)
        }),
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        } => literal_of(expr),
        Expr::Value(value) => literal_of_sql_value(value),
        _ => None,
    }
}

/// The JSON value one parsed SQL literal stands for.
fn literal_of_sql_value(value: &sqlparser::ast::ValueWithSpan) -> Option<Value> {
    use sqlparser::ast::Value as SqlValue;
    match &value.value {
        SqlValue::Number(digits, _) => digits.parse::<serde_json::Number>().ok().map(Value::Number),
        SqlValue::SingleQuotedString(text)
        | SqlValue::DoubleQuotedString(text)
        | SqlValue::EscapedStringLiteral(text)
        | SqlValue::UnicodeStringLiteral(text) => Some(Value::String(text.clone())),
        SqlValue::Boolean(flag) => Some(Value::Bool(*flag)),
        SqlValue::Null => Some(Value::Null),
        _ => None,
    }
}

/// Read a query's `ORDER BY` clause as the keys it orders by.
fn read_order_by(query: &Query) -> Result<Vec<OrderKey>, Unreadable> {
    use sqlparser::ast::OrderByKind;
    let Some(order_by) = query.order_by.as_ref() else {
        return Ok(Vec::new());
    };
    // A Config query orders by properties of the configuration item.
    let OrderByKind::Expressions(expressions) = &order_by.kind else {
        return Err(Unreadable);
    };
    let mut keys = Vec::new();
    for expression in expressions {
        let property = property_of(&expression.expr).ok_or(Unreadable)?;
        keys.push(OrderKey {
            property,
            // "ORDER BY property [DESC | ASC]": ascending unless the query says
            // otherwise, which is SQL's own default.
            descending: expression.options.asc == Some(false),
        });
    }
    Ok(keys)
}

/// Read a query's `LIMIT` clause as the number of rows it bounds the answer to.
fn read_limit(query: &Query) -> Result<Option<usize>, Unreadable> {
    use sqlparser::ast::LimitClause;
    let Some(clause) = query.limit_clause.as_ref() else {
        return Ok(None);
    };
    // A Config query carries a plain LIMIT: a number of rows, and no OFFSET or BY.
    let LimitClause::LimitOffset {
        limit,
        offset,
        limit_by,
    } = clause
    else {
        return Err(Unreadable);
    };
    if offset.is_some() || !limit_by.is_empty() {
        return Err(Unreadable);
    }
    let Some(limit) = limit else {
        return Ok(None);
    };
    let rows = literal_of(limit)
        .as_ref()
        .and_then(Value::as_u64)
        .ok_or(Unreadable)?;
    Ok(Some(usize::try_from(rows).unwrap_or(usize::MAX)))
}

/// Where `expression` passes a bound this emulator puts on the work ONE expression may
/// ask for, if it passes one.
///
/// [`MAX_EXPRESSION_DEPTH`] and [`MAX_EXPRESSION_TOKENS`] are the emulator's own bounds,
/// and this is one pass over the characters, so they cost less than the parse they guard
/// and nothing is parsed to find where one was passed — which is what keeps a
/// caller-controlled expression from turning into a deep recursion in the parser, or in
/// the drop of the tree it builds. Brackets and tokens inside a literal are counted too,
/// which can only make this OVER-count: an expression refused here is refused for
/// carrying that much, never served when it carries more.
fn passes_a_bound(expression: &str) -> Option<Position> {
    let mut open: usize = 0;
    let mut tokens: usize = 0;
    let mut inside_word = false;
    let mut at = Position::START;
    for character in expression.chars() {
        let here = at;
        at = at.after(character);
        // Each run of word characters, and each other non-whitespace character, counts as
        // one token.
        if character.is_alphanumeric() || matches!(character, '_' | '-' | ':' | '.') {
            if !inside_word {
                tokens += 1;
                inside_word = true;
            }
        } else {
            inside_word = false;
            if !character.is_whitespace() {
                tokens += 1;
            }
        }
        if tokens > MAX_EXPRESSION_TOKENS {
            return Some(here);
        }
        match character {
            '(' | '[' => {
                open += 1;
                if open > MAX_EXPRESSION_DEPTH {
                    return Some(here);
                }
            }
            ')' | ']' => open = open.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// The keyword a Config query opens with ("SELECT property [, ...] [ WHERE condition ]
/// [ GROUP BY property ] [ ORDER BY property [ASC | DESC] [, ...] ]").
const SELECT_KEYWORD: &str = "SELECT";

/// The most PREFIXES of one command [`stopped_reading_at`] reads to find where the reading
/// stopped.
///
/// Reading a prefix costs about what reading the whole command costs, so this is what keeps
/// refusing one expression a small multiple of reading it rather than a multiple of its
/// token count ([`MAX_EXPRESSION_TOKENS`] of them). The prefixes are read from the LONGEST
/// down and a Config query of the documented form carries a few dozen tokens, so the
/// reading of a command written in that form stops within this many; a command whose fault
/// is further than that from its end is answered at the token its reading began with, which
/// is where nothing of it was read.
const MAX_PREFIX_READINGS: usize = 64;

/// Where the reading of `expression` stopped: the position of the first token it could
/// not read as part of a Config query.
///
/// Found by READING, never by searching the text: the longest PREFIX of the command this
/// service can read as a Config query is the part it read, so the token that FOLLOWS that
/// prefix is the one the reading stopped at. Prefixes are cut at the token boundaries of
/// the submitted command — the tokenizer the parser itself uses supplies them, each with
/// the line and column it starts at — so the position names a token the caller wrote and
/// never a point inside one. The `SELECT` a Config query opens with is a beginning the
/// reading got past, so a command whose first selected property is already unreadable
/// stops there, while one that opens with anything else stops at its very first token.
fn stopped_reading_at(expression: &str) -> Position {
    let dialect = GenericDialect {};
    let tokenized = Tokenizer::new(&dialect, expression).tokenize_with_location();
    let tokens = match tokenized {
        Ok(tokens) => tokens,
        // The text does not split into tokens at all (an unterminated string literal,
        // say): the tokenizer itself reports where it stopped.
        Err(error) => return Position::from(error.location),
    };
    let significant: Vec<&TokenWithSpan> = tokens
        .iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_)))
        .collect();
    let starts: Vec<Position> = significant
        .iter()
        .map(|token| Position::from(token.span.start))
        .collect();
    let offsets = byte_offsets(expression);

    // Where the reading began: past the keyword a Config query opens with, or at the very
    // first token of a command that opens with anything else.
    let began_at = usize::from(
        significant
            .first()
            .is_some_and(|token| token.token.to_string().eq_ignore_ascii_case(SELECT_KEYWORD)),
    );
    for cut in (began_at + 1..starts.len()).rev().take(MAX_PREFIX_READINGS) {
        let prefix = offsets
            .get(&starts[cut])
            .and_then(|offset| expression.get(..*offset));
        if prefix.is_some_and(|prefix| read_command(prefix).is_ok()) {
            return starts[cut];
        }
    }
    match starts.get(began_at) {
        Some(stopped_at) => *stopped_at,
        // Nothing follows the beginning of the command, so the reading stopped where the
        // text ran out — the greatest position of the expression, one past its last
        // character.
        None if began_at > 0 => offsets
            .keys()
            .next_back()
            .copied()
            .unwrap_or(Position::START),
        None => starts.first().copied().unwrap_or(Position::START),
    }
}

/// The byte offset in `expression` of every position a token of it can start at.
fn byte_offsets(expression: &str) -> BTreeMap<Position, usize> {
    let mut offsets = BTreeMap::new();
    let mut at = Position::START;
    for (offset, character) in expression.char_indices() {
        offsets.insert(at, offset);
        at = at.after(character);
    }
    offsets.insert(at, expression.len());
    offsets
}

/// The configuration item `item` as the JSON document a Config query runs over.
///
/// The PROPERTIES a query selects and filters on are the members of `ConfigurationItem`,
/// spelled as the model spells them, so this rendering is what makes a member
/// selectable: a member the item does not carry is absent from the document, and a
/// condition on it holds for nothing. Two members are rendered rather than copied:
/// `configuration` is "a JSON-encoded string that contains the contents for the resource
/// configuration", which a query reaches INTO (`configuration.state`), so it is read as
/// the document it encodes; and a timestamp is rendered the way the protocol renders one,
/// so a condition compares it as the caller wrote it.
pub fn configuration_item_document(item: &ConfigurationItem) -> Value {
    let mut document = Map::new();
    let mut text = |member: &str, value: &Option<String>| {
        if let Some(value) = value {
            document.insert(member.to_string(), Value::String(value.clone()));
        }
    };
    text("version", &item.version);
    text("accountId", &item.account_id);
    text("configurationItemStatus", &item.configuration_item_status);
    text("configurationStateId", &item.configuration_state_id);
    text("configurationItemMD5Hash", &item.configuration_item_md5_hash);
    text("arn", &item.arn);
    text("resourceType", &item.resource_type);
    text("resourceId", &item.resource_id);
    text("resourceName", &item.resource_name);
    text("awsRegion", &item.aws_region);
    text("availabilityZone", &item.availability_zone);
    text("recordingFrequency", &item.recording_frequency);

    for (member, at) in [
        (
            "configurationItemCaptureTime",
            item.configuration_item_capture_time,
        ),
        ("resourceCreationTime", item.resource_creation_time),
        (
            "configurationItemDeliveryTime",
            item.configuration_item_delivery_time,
        ),
    ] {
        if let Some(rendered) = at.and_then(|at| {
            at.fmt(::aws_smithy_types::date_time::Format::DateTime)
                .ok()
        }) {
            document.insert(member.to_string(), Value::String(rendered));
        }
    }

    if let Some(tags) = &item.tags {
        document.insert(
            "tags".to_string(),
            Value::Object(
                tags.iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        );
    }
    if let Some(configuration) = &item.configuration {
        // A configuration document that is not JSON cannot be reached into, so it stays
        // the string it is rather than being reported as something it is not.
        let value = serde_json::from_str::<Value>(configuration)
            .unwrap_or_else(|_| Value::String(configuration.clone()));
        document.insert("configuration".to_string(), value);
    }
    if let Some(supplementary) = &item.supplementary_configuration {
        document.insert(
            "supplementaryConfiguration".to_string(),
            Value::Object(
                supplementary
                    .iter()
                    .map(|(key, value)| {
                        let value = serde_json::from_str::<Value>(value)
                            .unwrap_or_else(|_| Value::String(value.clone()));
                        (key.clone(), value)
                    })
                    .collect(),
            ),
        );
    }
    if let Some(events) = &item.related_events {
        document.insert(
            "relatedEvents".to_string(),
            Value::Array(events.iter().map(|id| Value::String(id.clone())).collect()),
        );
    }
    if let Some(relationships) = &item.relationships {
        document.insert(
            "relationships".to_string(),
            Value::Array(
                relationships
                    .iter()
                    .map(|relationship| {
                        let mut rendered = Map::new();
                        for (member, value) in [
                            ("resourceType", &relationship.resource_type),
                            ("resourceId", &relationship.resource_id),
                            ("resourceName", &relationship.resource_name),
                            ("relationshipName", &relationship.relationship_name),
                        ] {
                            if let Some(value) = value {
                                rendered.insert(member.to_string(), Value::String(value.clone()));
                            }
                        }
                        Value::Object(rendered)
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(document)
}

/// Every configuration item a resource query of this account's Config data searches: the
/// CURRENT configuration item of each resource the inventory holds, as the document a
/// query runs over.
///
/// A resource Config recorded the deletion of is not among them, for the same reason the
/// aggregate reads leave it out ("The API does not return results for deleted
/// resources"), and the items come in the inventory's own `(resourceType, resourceId)`
/// ordering, so a query that orders by nothing still answers in a stable order and a page
/// token names the same row twice.
pub fn resource_configuration_item_documents(
    store: &ComAmazonawsConfigserviceStarlingdoveserviceState,
    aws_region: &str,
) -> Vec<Value> {
    store
        .discovered_resources
        .values()
        .filter(|entry| entry.deleted_at.is_none())
        .map(|entry| recorded_resource_document(store, entry, aws_region))
        .collect()
}

/// The document a query runs over for ONE recorded resource: its CURRENT configuration
/// item, at the recording frequency the recording recorder records that resource type at.
///
/// The ONE mapping both readers below use, so the single-account and the aggregate query
/// paths cannot drift on what a document contains.
fn recorded_resource_document(
    store: &ComAmazonawsConfigserviceStarlingdoveserviceState,
    entry: &super::DiscoveredResourceEntry,
    aws_region: &str,
) -> Value {
    let item = entry.to_configuration_item(
        aws_region,
        store.recording_frequency_of(&entry.resource_type),
    );
    configuration_item_document(&item)
}

/// Every configuration item an AGGREGATE query of `aggregator` searches: the CURRENT
/// configuration item of each resource that aggregator takes in, as the document a
/// query runs over.
///
/// An aggregator answers only for the Config data it aggregates, so what it takes in is
/// decided by the same two questions the single-resource aggregate read asks — is the
/// source account and Region one of the aggregator's sources, and is the resource type
/// one it takes in ([`super::ConfigurationAggregatorEntry::aggregates_source`] and
/// `aggregates_resource_type`). This store stands for ONE account in ONE Region, so the
/// only source it can answer for is `account_id` in `aws_region`: an aggregator over
/// any other source aggregates data this store does not hold and its queries match
/// nothing.
pub fn aggregated_configuration_item_documents(
    store: &ComAmazonawsConfigserviceStarlingdoveserviceState,
    aggregator: &ConfigurationAggregatorEntry,
    account_id: &str,
    aws_region: &str,
) -> Vec<Value> {
    // Whether this store's own source is one the aggregator takes in is one question
    // for the whole query, so it is asked once rather than per resource.
    if !aggregator.aggregates_source(account_id, aws_region) {
        return Vec::new();
    }
    store
        .discovered_resources
        .values()
        .filter(|entry| entry.deleted_at.is_none())
        .filter(|entry| aggregator.aggregates_resource_type(&entry.resource_type))
        .map(|entry| recorded_resource_document(store, entry, aws_region))
        .collect()
}

/// How a select operation words `InvalidLimitException` ("The specified limit is outside
/// the allowable range") for the page size `requested` under the member `member`.
///
/// BUILT FROM THE REQUEST — it quotes the value sent, names the member that carried it
/// and states the bound it missed — and written once, so the two select operations cannot
/// word the same refusal differently.
pub fn invalid_limit_message(member: &str, requested: i32) -> String {
    format!(
        "Value '{requested}' at '{member}' failed to satisfy constraint: Member must be \
         between 0 and {SELECT_RESULTS_PAGE_SIZE}"
    )
}

/// The page size `requested` asks for, as the one rule both select operations apply:
/// `Some(size)` for a value inside the modelled range, `None` for the modelled default of
/// 0 — "unspecified", which leaves the page size to the operation's own documented
/// default — and `Err` carrying the message of the refusal for a value outside the range.
///
/// The modelled range (0..100) is refused at the request boundary where the boundary
/// enforces it; this check stands so a value that reaches a handler answers that
/// handler's own modelled `InvalidLimitException` either way.
pub fn requested_page_size(member: &str, requested: i32) -> Result<Option<usize>, String> {
    match usize::try_from(requested) {
        Ok(0) => Ok(None),
        Ok(size) if size <= SELECT_RESULTS_PAGE_SIZE => Ok(Some(size)),
        _ => Err(invalid_limit_message(member, requested)),
    }
}

/// Where in a query's results the page `next_token` names begins: 0 when the request
/// carries no token, and `None` for a token outside the opaque form a select operation
/// issues — which is the modelled `InvalidNextTokenException`.
///
/// The token is opaque to its holder: it names how many results of this query were
/// already answered, which is what makes the next page the rows after them.
pub fn select_resume_at(next_token: Option<&str>) -> Option<usize> {
    match next_token {
        None => Some(0),
        Some(token) => {
            super::decode_page_token(token).and_then(|resume_at| resume_at.parse::<usize>().ok())
        }
    }
}

/// The page of `results` that begins at `resume_at` and carries at most `page_size`
/// rows, and the token that names the page after it (`None` once the whole result set has
/// been answered).
///
/// Each result is one JSON document of the properties the query selected, which is what
/// `Results` carries on the wire.
pub fn select_results_page(
    results: &[Value],
    resume_at: usize,
    page_size: usize,
) -> (Vec<String>, Option<String>) {
    let page: Vec<String> = results
        .iter()
        .skip(resume_at)
        .take(page_size)
        .map(Value::to_string)
        .collect();
    let answered = resume_at.saturating_add(page.len());
    let next_token =
        (answered < results.len()).then(|| super::encode_page_token(&answered.to_string()));
    (page, next_token)
}

/// The `QueryInfo` a select answers for `query`: the fields it selected, in the order it
/// selected them (`QueryInfo$SelectFields`, "Returns a `FieldInfo` object" per field).
pub fn select_query_info(query: &SelectQuery) -> QueryInfo {
    QueryInfo {
        select_fields: Some(
            query
                .field_names()
                .into_iter()
                .map(|name| FieldInfo { name: Some(name) })
                .collect(),
        ),
    }
}

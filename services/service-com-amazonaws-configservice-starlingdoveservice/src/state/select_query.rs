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

use super::ComAmazonawsConfigserviceStarlingdoveserviceState;
use crate::ops::types::ConfigurationItem;

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
    fn aggregates(&self) -> bool {
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

/// Read `expression` as a Config query, or answer why it is not one.
///
/// The message is BUILT FROM THE REQUEST: it states what about the expression cannot be
/// read, so a caller can fix the query from the refusal alone.
pub fn read_expression(expression: &str) -> Result<SelectQuery, String> {
    if nesting_depth(expression) > MAX_EXPRESSION_DEPTH {
        return Err(format!(
            "the expression nests more than {MAX_EXPRESSION_DEPTH} levels of brackets"
        ));
    }
    if token_count(expression) > MAX_EXPRESSION_TOKENS {
        return Err(format!(
            "the expression carries more than {MAX_EXPRESSION_TOKENS} tokens"
        ));
    }

    let statements = Parser::parse_sql(&GenericDialect {}, expression)
        .map_err(|error| format!("the expression is not a SQL SELECT command: {error}"))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(
            "the expression must be exactly one SQL SELECT command and nothing else".to_string(),
        );
    };
    read_query(query)
}

/// Read one parsed SQL query as a Config query.
fn read_query(query: &Query) -> Result<SelectQuery, String> {
    if query.with.is_some() {
        return Err("a Config query carries no WITH clause".to_string());
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(
            "a Config query is a single SELECT, not a set operation or a parenthesised query"
                .to_string(),
        );
    };
    let select: &Select = select.as_ref();
    if !select.from.is_empty() {
        return Err(
            "a Config query selects from the resources Config has recorded, so it carries no \
             FROM clause"
                .to_string(),
        );
    }
    if select.having.is_some() {
        return Err("a Config query carries no HAVING clause".to_string());
    }
    if select.distinct.is_some() {
        return Err("a Config query carries no DISTINCT clause".to_string());
    }

    let mut fields: Vec<Field> = Vec::new();
    for item in &select.projection {
        fields.push(read_field(item)?);
    }
    if fields.is_empty() {
        return Err("a Config query selects at least one property".to_string());
    }

    let group_by = match &select.group_by {
        GroupByExpr::Expressions(expressions, modifiers) if modifiers.is_empty() => {
            let mut grouped: Vec<Vec<String>> = Vec::new();
            for expression in expressions {
                grouped.push(property_of(expression).ok_or_else(|| {
                    "a Config query groups by a property of the configuration item".to_string()
                })?);
            }
            grouped
        }
        _ => {
            return Err(
                "a Config query groups by properties of the configuration item".to_string(),
            )
        }
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
        for field in &fields {
            match field {
                Field::Everything => {
                    return Err(
                        "a Config query that aggregates selects properties it groups by, not *"
                            .to_string(),
                    )
                }
                Field::Property(path) if !group_by.iter().any(|grouped| grouped == path) => {
                    return Err(format!(
                        "the property '{}' is selected beside an aggregate, so the query must \
                         group by it",
                        path.join(".")
                    ))
                }
                _ => {}
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
                return Err(format!(
                    "the ORDER BY property '{path}' is neither grouped by nor selected by this \
                     query"
                ));
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
fn read_field(item: &SelectItem) -> Result<Field, String> {
    match item {
        SelectItem::Wildcard(_) => Ok(Field::Everything),
        SelectItem::UnnamedExpr(expression) => read_field_expression(expression),
        SelectItem::ExprWithAlias { .. } => Err(
            "a Config query selects properties of the configuration item, which carry no alias"
                .to_string(),
        ),
        SelectItem::QualifiedWildcard(..) => Err(
            "a Config query selects properties of one configuration item, so a qualified \
             wildcard names nothing"
                .to_string(),
        ),
    }
}

/// Read one selected expression: a property of the configuration item, or an aggregate
/// over the items the query matched.
fn read_field_expression(expression: &Expr) -> Result<Field, String> {
    if let Expr::Function(function) = expression {
        let name = function
            .name
            .0
            .iter()
            .map(|part| part.to_string())
            .collect::<Vec<String>>()
            .join(".");
        let Some(aggregate) = Aggregate::parse(&name) else {
            return Err(format!(
                "'{name}' is not one of the functions a Config query selects (AVG, COUNT, MAX, \
                 MIN, SUM)"
            ));
        };
        let FunctionArguments::List(arguments) = &function.args else {
            return Err(format!("{name} takes one argument"));
        };
        let [argument] = arguments.args.as_slice() else {
            return Err(format!("{name} takes exactly one argument"));
        };
        let property = match argument {
            FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => None,
            FunctionArg::Unnamed(FunctionArgExpr::Expr(inner)) => {
                Some(property_of(inner).ok_or_else(|| {
                    format!("{name} takes a property of the configuration item")
                })?)
            }
            _ => {
                return Err(format!(
                    "{name} takes a property of the configuration item, or * for COUNT"
                ))
            }
        };
        if property.is_none() && aggregate != Aggregate::Count {
            return Err(format!("{name} takes a property, not *"));
        }
        return Ok(Field::Aggregated {
            function: aggregate,
            property,
        });
    }
    property_of(expression)
        .map(Field::Property)
        .ok_or_else(|| {
            "a Config query selects properties of the configuration item, aggregates over them, \
             or *"
                .to_string()
        })
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
fn read_condition(expression: &Expr, depth: usize) -> Result<Condition, String> {
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(format!(
            "the condition nests more than {MAX_EXPRESSION_DEPTH} levels"
        ));
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
                Err(
                    "a condition of a Config query compares a property of the configuration item \
                     against a value"
                        .to_string(),
                )
            }
            other => Err(format!(
                "'{other}' is not an operator a condition of a Config query uses"
            )),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => Ok(Condition::Not(Box::new(read_condition(expr, depth)?))),
        Expr::IsNull(inner) => Ok(Condition::IsNull {
            property: property_of(inner).ok_or_else(condition_needs_property)?,
            negated: false,
        }),
        Expr::IsNotNull(inner) => Ok(Condition::IsNull {
            property: property_of(inner).ok_or_else(condition_needs_property)?,
            negated: true,
        }),
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let property = property_of(expr).ok_or_else(condition_needs_property)?;
            let mut values = Vec::new();
            for item in list {
                values.push(literal_of(item).ok_or_else(|| {
                    "an IN list of a Config query carries values".to_string()
                })?);
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
            property: property_of(expr).ok_or_else(condition_needs_property)?,
            low: literal_of(low)
                .ok_or_else(|| "a BETWEEN of a Config query carries values".to_string())?,
            high: literal_of(high)
                .ok_or_else(|| "a BETWEEN of a Config query carries values".to_string())?,
            negated: *negated,
        }),
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        } => {
            let property = property_of(expr).ok_or_else(condition_needs_property)?;
            let pattern = literal_of(pattern)
                .and_then(|value| value.as_str().map(str::to_string))
                .ok_or_else(|| "a LIKE of a Config query carries a pattern".to_string())?;
            Ok(Condition::Like {
                property,
                pattern,
                negated: *negated,
            })
        }
        other => Err(format!(
            "'{other}' is not a condition of the Config query language"
        )),
    }
}

/// The one sentence a condition that does not name a property is refused with.
fn condition_needs_property() -> String {
    "a condition of a Config query names a property of the configuration item".to_string()
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
fn read_order_by(query: &Query) -> Result<Vec<OrderKey>, String> {
    use sqlparser::ast::OrderByKind;
    let Some(order_by) = query.order_by.as_ref() else {
        return Ok(Vec::new());
    };
    let OrderByKind::Expressions(expressions) = &order_by.kind else {
        return Err("a Config query orders by properties of the configuration item".to_string());
    };
    let mut keys = Vec::new();
    for expression in expressions {
        let property = property_of(&expression.expr).ok_or_else(|| {
            "a Config query orders by properties of the configuration item".to_string()
        })?;
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
fn read_limit(query: &Query) -> Result<Option<usize>, String> {
    use sqlparser::ast::LimitClause;
    let Some(clause) = query.limit_clause.as_ref() else {
        return Ok(None);
    };
    let LimitClause::LimitOffset {
        limit,
        offset,
        limit_by,
    } = clause
    else {
        return Err("a Config query carries a plain LIMIT".to_string());
    };
    if offset.is_some() || !limit_by.is_empty() {
        return Err("a Config query carries a plain LIMIT".to_string());
    }
    let Some(limit) = limit else {
        return Ok(None);
    };
    let rows = literal_of(limit)
        .as_ref()
        .and_then(Value::as_u64)
        .ok_or_else(|| "the LIMIT of a Config query is a number of rows".to_string())?;
    Ok(Some(usize::try_from(rows).unwrap_or(usize::MAX)))
}

/// The depth of `expression`'s bracketing: the most brackets open at any one point.
///
/// One pass over the characters, so the bound costs less than the parse it guards.
/// Brackets inside a literal are counted too, which can only make this OVER-count: an
/// expression refused here is refused for carrying that many brackets, never served
/// when it nests deeper.
fn nesting_depth(expression: &str) -> usize {
    let mut depth: usize = 0;
    let mut deepest: usize = 0;
    for character in expression.chars() {
        match character {
            '(' | '[' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// The number of tokens `expression` carries: each run of word characters, and each
/// other non-whitespace character, counts once. It OVER-counts rather than
/// under-counts, which is the safe direction for a bound on depth.
fn token_count(expression: &str) -> usize {
    let mut tokens: usize = 0;
    let mut inside_word = false;
    for character in expression.chars() {
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
    }
    tokens
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
        .map(|entry| {
            let item = entry.to_configuration_item(
                aws_region,
                store.recording_frequency_of(&entry.resource_type),
            );
            configuration_item_document(&item)
        })
        .collect()
}

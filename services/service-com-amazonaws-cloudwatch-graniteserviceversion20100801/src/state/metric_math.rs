// SPDX-License-Identifier: CC0-1.0

//! Metric math: the expression language a `MetricDataQuery$Expression` carries, parsed once
//! and evaluated over the series this store holds.
//!
//! "A math expression can use the `Id` of the other metrics or queries to refer to those
//! metrics, and can also use the `Id` of other expressions to use the result of those
//! expressions." (`MetricDataQuery$Expression`) — so an expression is a tree over three kinds
//! of value: a plain number, ONE time series, and an ARRAY of time series (what a search or
//! `METRICS()` yields). Every function and operator below is typed in those three, which is
//! what decides whether a call is evaluated or refused.
//!
//! The language is CloudWatch's own and no grammar for it is published outside the service,
//! so it is read here with a tokeniser and a precedence-climbing parser rather than through a
//! parser crate — there is no crate that implements this dialect.
//!
//! Nothing in this module reaches the store: the series a `SEARCH` discovers come from the
//! [`SeriesSource`] its caller passes in, so the one reader of the metric store stays the
//! handler that holds its lock.

use std::collections::{BTreeMap, HashMap};

/// One time series an expression reads or produces: its label, and its `(time stamp in epoch
/// seconds, value)` points, oldest first.
#[derive(Debug, Clone, Default)]
pub struct MathSeries {
    /// The label the result carrying this series reports.
    pub label: String,
    /// The series' points, oldest first.
    pub points: Vec<(f64, f64)>,
    /// The metric and statistic this series was RETRIEVED for, when it is one metric's series
    /// rather than a value computed from others.
    ///
    /// It is what an anomaly detector is keyed by ([`SeriesSource::anomaly_model`]), so only a
    /// series that still IS a metric's series carries it: every series an operator or a
    /// function computes is a new series, which no detector was created for.
    pub metric: Option<SeriesMetric>,
}

/// The metric one series was retrieved for: the identity of the stored series, and the
/// statistic it was folded by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeriesMetric {
    /// The metric's namespace.
    pub namespace: String,
    /// The metric's name.
    pub metric_name: String,
    /// The metric's normalised dimension set.
    pub dimensions: BTreeMap<String, String>,
    /// The statistic the series' points were folded by.
    pub stat: String,
}

impl MathSeries {
    /// A series with `points` and no label of its own yet.
    pub fn new(label: impl Into<String>, points: Vec<(f64, f64)>) -> Self {
        Self {
            label: label.into(),
            points,
            metric: None,
        }
    }

    /// The same series, remembering the metric it was retrieved for.
    pub fn of_metric(mut self, metric: SeriesMetric) -> Self {
        self.metric = Some(metric);
        self
    }
}

/// What an expression evaluates to.
#[derive(Debug, Clone)]
pub enum MathValue {
    /// One number, which every point of the series it is combined with is combined with.
    Scalar(f64),
    /// One time series.
    Series(MathSeries),
    /// Several time series, as a search or `METRICS()` returns them.
    Array(Vec<MathSeries>),
}

/// Why an expression is not one this service evaluates: a malformed expression, a reference
/// to an id the request does not define, a function nothing here computes, or a call whose
/// operands are not the kinds of value it takes.
#[derive(Debug, Clone)]
pub struct MathError {
    /// What is wrong with the expression, in the words the refusal reports.
    pub message: String,
}

impl MathError {
    /// An error carrying `message`.
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// What an expression reads out of the store: the series a `SEARCH` discovers, and the
/// anomaly-detection model a band is drawn from.
///
/// An expression never reads the store itself: its caller implements this over the store it
/// already holds, so one query takes one lock however many searches its expressions carry.
pub trait SeriesSource {
    /// Every series the search `query` matches, each folded by `stat` into `period`-wide
    /// windows of the request's range, and what the search COST to answer — which its caller
    /// charges to the evaluation's budget.
    fn search(&self, query: &str, stat: &str, period: f64) -> Result<SearchResult, MathError>;

    /// The model the anomaly DETECTOR created for `series` expects its values by, or `None`
    /// when no detector was created for it.
    ///
    /// A band is the detector's: only a trained model says what a value is expected to be, so
    /// a series the lookup finds no detector for has no band — which is what
    /// `ANOMALY_DETECTION_BAND` then reports. The lookup is keyed on the metric the series was
    /// retrieved for ([`MathSeries::metric`]), because that is what a detector is created
    /// for.
    fn anomaly_model(&self, series: &MathSeries) -> Option<AnomalyModel>;

    /// How many stored data points answering [`Self::anomaly_model`] for `series` will VISIT:
    /// the history the model is trained on, which is the metric's own and not the span this
    /// expression reads.
    ///
    /// Reported separately so the caller charges that work to its budget BEFORE the fold runs
    /// ([`Evaluation::charge`]), the way every other loop over a series' points is charged.
    fn anomaly_training_datapoints(&self, series: &MathSeries) -> usize;
}

/// What an anomaly detector's trained model says a series is expected to be.
pub struct AnomalyModel {
    /// The value the model expects at each of the series' time stamps, oldest first.
    pub expected: Vec<(f64, f64)>,
    /// How far from the expected value one band width reaches at each of those time stamps.
    pub deviation: Vec<f64>,
}

/// What one search found, and what answering it cost.
pub struct SearchResult {
    /// The series the query matched.
    pub series: Vec<MathSeries>,
    /// How many of the store's series the query was matched against, so a search that matches
    /// nothing still costs the scan it took.
    pub scanned: usize,
}

/// A token of an expression.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A number literal.
    Number(f64),
    /// A quoted string — a search query, a label, or a fill mode.
    Text(String),
    /// A bare word: a query id, a function name, or a logical operator spelled out.
    Word(String),
    /// An operator symbol.
    Operator(Operator),
    /// `(`.
    Open,
    /// `)`.
    Close,
    /// `[`.
    OpenArray,
    /// `]`.
    CloseArray,
    /// `,`.
    Comma,
    /// A unary `-`, which the parser decides from position.
    Minus,
    /// A unary `!` / `NOT`.
    Bang,
}

/// A binary operator of the expression language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Modulo,
    /// `^`
    Power,
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterOrEqual,
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `AND` / `&&`
    And,
    /// `OR` / `||`
    Or,
}

impl Operator {
    /// How tightly this operator binds; a larger number binds tighter.
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Less
            | Self::LessOrEqual
            | Self::Greater
            | Self::GreaterOrEqual
            | Self::Equal
            | Self::NotEqual => 3,
            Self::Add | Self::Subtract => 4,
            Self::Multiply | Self::Divide | Self::Modulo => 5,
            Self::Power => 6,
        }
    }

    /// Whether the operator groups to the right, as exponentiation does.
    fn right_associative(self) -> bool {
        self == Self::Power
    }

    /// The operator applied to two numbers. A comparison and a logical operator answer 1 for
    /// true and 0 for false, which is what makes them operands of arithmetic and of `IF`.
    fn apply(self, left: f64, right: f64) -> f64 {
        let truth = |holds: bool| if holds { 1.0 } else { 0.0 };
        match self {
            Self::Add => left + right,
            Self::Subtract => left - right,
            Self::Multiply => left * right,
            Self::Divide => left / right,
            Self::Modulo => left % right,
            Self::Power => left.powf(right),
            Self::Less => truth(left < right),
            Self::LessOrEqual => truth(left <= right),
            Self::Greater => truth(left > right),
            Self::GreaterOrEqual => truth(left >= right),
            #[allow(clippy::float_cmp)]
            Self::Equal => truth(left == right),
            #[allow(clippy::float_cmp)]
            Self::NotEqual => truth(left != right),
            Self::And => truth(left != 0.0 && right != 0.0),
            Self::Or => truth(left != 0.0 || right != 0.0),
        }
    }
}

/// A parsed expression.
#[derive(Debug, Clone)]
pub enum Expression {
    /// A number literal.
    Number(f64),
    /// A quoted string literal.
    Text(String),
    /// A reference to another query's `Id`.
    Reference(String),
    /// A bare word one function reads as a KEYWORD rather than as a value: a fill mode, or a
    /// sort's statistic and order. It names no query, so nothing resolves it as one.
    Keyword(String),
    /// A function call.
    Call {
        /// The function's name, as the expression spelled it.
        name: String,
        /// The call's arguments, in order.
        arguments: Vec<Expression>,
    },
    /// An array literal: `[m1, m2]`.
    Array(Vec<Expression>),
    /// A negated expression.
    Negate(Box<Expression>),
    /// A logically negated expression.
    Not(Box<Expression>),
    /// Two expressions combined by an operator.
    Binary {
        /// The operator.
        operator: Operator,
        /// Its left operand.
        left: Box<Expression>,
        /// Its right operand.
        right: Box<Expression>,
    },
}

/// The query ids `expression` refers to, in the order it names them and without repeats.
pub fn references(expression: &Expression) -> Vec<String> {
    let mut found = Vec::new();
    collect_references(expression, &mut found);
    found
}

/// Every function this module computes, by the name an expression calls it under.
///
/// The one catalog: the evaluator resolves a call against it before evaluating any argument,
/// and the readers that only ANALYSE an expression ([`calls_function`],
/// [`returns_many_series`]) name their functions out of the same list.
const KNOWN_FUNCTIONS: &[&str] = &[
    "ABS",
    "ANOMALY_DETECTION_BAND",
    "AVG",
    "CEIL",
    "DATAPOINT_COUNT",
    "DIFF",
    "DIFF_TIME",
    "FILL",
    "FIRST",
    "FLOOR",
    "IF",
    "INTEGRAL",
    "LAST",
    "LOG",
    "LOG10",
    "MAX",
    "METRIC_COUNT",
    "METRICS",
    "MIN",
    "PERIOD",
    "RATE",
    "REMOVE_EMPTY",
    "RUNNING_SUM",
    "SEARCH",
    "SLICE",
    "SORT",
    "SQRT",
    "STDDEV",
    "SUM",
    "TIME_SERIES",
];

/// The functions whose value is an ARRAY of time series whatever they are given: a search and
/// `METRICS()` return every series they matched, and an anomaly band returns its two bounds.
const ARRAY_FUNCTIONS: &[&str] = &["SEARCH", "METRICS", "ANOMALY_DETECTION_BAND", "REMOVE_EMPTY"];

/// The functions whose value is an array exactly when their first argument's is: they order,
/// cut or transform what they are given without changing how many series it is.
const ARRAY_PRESERVING_FUNCTIONS: &[&str] = &[
    "SORT", "SLICE", "FILL", "ABS", "CEIL", "FLOOR", "LOG", "LOG10", "SQRT", "RATE", "DIFF",
    "DIFF_TIME", "RUNNING_SUM", "INTEGRAL",
];

/// Whether `expression` CALLS `function`, at any depth.
///
/// The one reader of this question: an alarm refuses an expression that calls `SEARCH` however
/// the call is wrapped, and the same parse answers it that the evaluator uses.
pub fn calls_function(expression: &Expression, function: &str) -> bool {
    match expression {
        Expression::Number(_)
        | Expression::Text(_)
        | Expression::Reference(_)
        | Expression::Keyword(_) => false,
        Expression::Call { name, arguments } => {
            name.eq_ignore_ascii_case(function)
                || arguments
                    .iter()
                    .any(|argument| calls_function(argument, function))
        }
        Expression::Array(items) => items.iter().any(|item| calls_function(item, function)),
        Expression::Negate(inner) | Expression::Not(inner) => calls_function(inner, function),
        Expression::Binary { left, right, .. } => {
            calls_function(left, function) || calls_function(right, function)
        }
    }
}

/// Whether the value of `expression` is MORE than one time series.
///
/// What decides it is the kind of value the outermost call produces, not what the expression
/// mentions: a function that returns every series it matched is many, the same call wrapped in
/// one that folds them to a series is one — which is why an alarm, whose expression "must
/// return a single time series", refuses the first and accepts the second.
pub fn returns_many_series(expression: &Expression) -> bool {
    match expression {
        Expression::Number(_)
        | Expression::Text(_)
        | Expression::Reference(_)
        | Expression::Keyword(_) => false,
        Expression::Array(_) => true,
        Expression::Call { name, arguments } => {
            let upper = name.to_ascii_uppercase();
            if ARRAY_FUNCTIONS.contains(&upper.as_str()) {
                return true;
            }
            ARRAY_PRESERVING_FUNCTIONS.contains(&upper.as_str())
                && arguments.first().is_some_and(returns_many_series)
        }
        Expression::Negate(inner) | Expression::Not(inner) => returns_many_series(inner),
        Expression::Binary { left, right, .. } => {
            returns_many_series(left) || returns_many_series(right)
        }
    }
}

/// The keyword a Metrics Insights query opens with ("Metrics Insights query components and
/// syntax", `MetricDataQuery$Expression`).
const SELECT_KEYWORD: &str = "SELECT";

/// Whether `expression` is a Metrics Insights query rather than a metric math expression: an
/// Insights query is a SELECT statement in a language of its own, so none of the metric math
/// above speaks about it.
pub fn is_metrics_insights_query(expression: &str) -> bool {
    expression
        .trim_start()
        .get(..SELECT_KEYWORD.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(SELECT_KEYWORD))
}

/// Append the ids `expression` refers to that `found` does not already carry.
fn collect_references(expression: &Expression, found: &mut Vec<String>) {
    match expression {
        Expression::Number(_) | Expression::Text(_) | Expression::Keyword(_) => {}
        Expression::Reference(id) => {
            if !found.iter().any(|seen| seen == id) {
                found.push(id.clone());
            }
        }
        Expression::Call { arguments, .. } | Expression::Array(arguments) => {
            for argument in arguments {
                collect_references(argument, found);
            }
        }
        Expression::Negate(inner) | Expression::Not(inner) => collect_references(inner, found),
        Expression::Binary { left, right, .. } => {
            collect_references(left, found);
            collect_references(right, found);
        }
    }
}

/// Parse an expression, or say why it is not one.
pub fn parse(expression: &str) -> Result<Expression, MathError> {
    // The length bound comes first: everything below walks the expression, and the walk's own
    // depth is charged against [`MAX_NESTING`], so neither grows with a longer request.
    if expression.chars().count() > MAX_EXPRESSION_CHARS {
        return Err(MathError::new(format!(
            "an expression is at most {MAX_EXPRESSION_CHARS} characters long"
        )));
    }
    let tokens = tokenize(expression)?;
    if tokens.is_empty() {
        return Err(MathError::new("the expression is empty"));
    }
    let mut parser = Parser {
        tokens,
        index: 0,
        depth: 0,
    };
    let parsed = parser.expression(0)?;
    if parser.index < parser.tokens.len() {
        return Err(MathError::new("Invalid syntax"));
    }
    Ok(parsed)
}

/// Split an expression into its tokens.
#[allow(clippy::too_many_lines)]
fn tokenize(expression: &str) -> Result<Vec<Token>, MathError> {
    let characters: Vec<char> = expression.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut index = 0;
    while let Some(&current) = characters.get(index) {
        if current.is_whitespace() {
            index += 1;
            continue;
        }
        match current {
            '\'' | '"' => {
                index += 1;
                let mut text = String::new();
                let mut closed = false;
                while let Some(&character) = characters.get(index) {
                    index += 1;
                    if character == current {
                        closed = true;
                        break;
                    }
                    text.push(character);
                }
                if !closed {
                    return Err(MathError::new("Invalid syntax"));
                }
                tokens.push(Token::Text(text));
            }
            '(' => {
                tokens.push(Token::Open);
                index += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                index += 1;
            }
            '[' => {
                tokens.push(Token::OpenArray);
                index += 1;
            }
            ']' => {
                tokens.push(Token::CloseArray);
                index += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                index += 1;
            }
            '+' => {
                tokens.push(Token::Operator(Operator::Add));
                index += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                index += 1;
            }
            '*' => {
                tokens.push(Token::Operator(Operator::Multiply));
                index += 1;
            }
            '/' => {
                tokens.push(Token::Operator(Operator::Divide));
                index += 1;
            }
            '%' => {
                tokens.push(Token::Operator(Operator::Modulo));
                index += 1;
            }
            '^' => {
                tokens.push(Token::Operator(Operator::Power));
                index += 1;
            }
            '<' | '>' | '=' | '!' => {
                let followed_by_equals = characters.get(index + 1) == Some(&'=');
                let operator = match (current, followed_by_equals) {
                    ('<', false) => Some(Operator::Less),
                    ('<', true) => Some(Operator::LessOrEqual),
                    ('>', false) => Some(Operator::Greater),
                    ('>', true) => Some(Operator::GreaterOrEqual),
                    ('=', _) => Some(Operator::Equal),
                    ('!', true) => Some(Operator::NotEqual),
                    ('!', false) => None,
                    _ => return Err(MathError::new("Invalid syntax")),
                };
                index += 1 + usize::from(followed_by_equals);
                match operator {
                    Some(operator) => tokens.push(Token::Operator(operator)),
                    None => tokens.push(Token::Bang),
                }
            }
            '&' | '|' => {
                if characters.get(index + 1) != Some(&current) {
                    return Err(MathError::new("Invalid syntax"));
                }
                index += 2;
                tokens.push(Token::Operator(if current == '&' {
                    Operator::And
                } else {
                    Operator::Or
                }));
            }
            _ if current.is_ascii_digit() || current == '.' => {
                let start = index;
                while characters
                    .get(index)
                    .is_some_and(|character| character.is_ascii_digit() || *character == '.')
                {
                    index += 1;
                }
                if characters
                    .get(index)
                    .is_some_and(|character| *character == 'e' || *character == 'E')
                {
                    let exponent = index;
                    index += 1;
                    if characters
                        .get(index)
                        .is_some_and(|character| *character == '+' || *character == '-')
                    {
                        index += 1;
                    }
                    if characters
                        .get(index)
                        .is_some_and(char::is_ascii_digit)
                    {
                        while characters.get(index).is_some_and(char::is_ascii_digit) {
                            index += 1;
                        }
                    } else {
                        // `1e` is not a number; the `e` starts a word instead.
                        index = exponent;
                    }
                }
                let text: String = characters
                    .get(start..index)
                    .unwrap_or_default()
                    .iter()
                    .collect();
                let number = text
                    .parse::<f64>()
                    .map_err(|_| MathError::new("Invalid syntax"))?;
                tokens.push(Token::Number(number));
            }
            _ if current.is_alphabetic() || current == '_' => {
                let start = index;
                while characters.get(index).is_some_and(|character| {
                    character.is_alphanumeric() || *character == '_'
                }) {
                    index += 1;
                }
                let word: String = characters
                    .get(start..index)
                    .unwrap_or_default()
                    .iter()
                    .collect();
                match word.to_ascii_uppercase().as_str() {
                    "AND" => tokens.push(Token::Operator(Operator::And)),
                    "OR" => tokens.push(Token::Operator(Operator::Or)),
                    "NOT" => tokens.push(Token::Bang),
                    _ => tokens.push(Token::Word(word)),
                }
            }
            _ => return Err(MathError::new("Invalid syntax")),
        }
    }
    Ok(tokens)
}

/// The precedence-climbing parser over an expression's tokens.
struct Parser {
    /// The tokens, in order.
    tokens: Vec<Token>,
    /// How many of them are consumed.
    index: usize,
    /// How deeply the term being parsed is nested, so a deeply parenthesised expression is
    /// refused rather than recursed into without bound.
    depth: usize,
}

/// How deeply an expression may nest: every recursion of the parse and of the walks over the
/// parsed tree is charged against this, so neither a parenthesised group, a chain of prefix
/// operators nor a right-associative chain can recurse without bound.
const MAX_NESTING: usize = 64;

/// The fill modes `FILL`'s second argument may name: "FILL(m1, REPEAT)".
const FILL_MODES: [&str; 2] = ["REPEAT", "LINEAR"];

/// The statistics `SORT`'s second argument may order by.
const SORT_STATISTICS: [&str; 5] = ["AVG", "MIN", "MAX", "SUM", "STDDEV"];

/// The orders `SORT`'s third argument may name.
const SORT_ORDERS: [&str; 2] = ["ASC", "DESC"];

/// The words the `position`th argument of `function` reads as a KEYWORD rather than as a
/// query id — the ONE table both the parse and the two functions that take them go by, so a
/// word in such a position cannot be read as a keyword by one reader and as a reference by
/// another. Every other word in every other position is a reference.
fn keywords_at(function: &str, position: usize) -> &'static [&'static str] {
    match (function.to_ascii_uppercase().as_str(), position) {
        ("FILL", 1) => &FILL_MODES,
        ("SORT", 1) => &SORT_STATISTICS,
        ("SORT", 2) => &SORT_ORDERS,
        _ => &[],
    }
}

/// How long an expression may be: "Expression (MetricExpression, len 1..2048)"
/// (`MetricDataQuery$Expression`). Stated here as well, so the bound the parse relies on does
/// not depend on the boundary validator having run.
const MAX_EXPRESSION_CHARS: usize = 2048;

impl Parser {
    /// Charge one level of nesting, refusing one level too deep.
    fn enter(&mut self) -> Result<(), MathError> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return Err(MathError::new("the expression nests too deeply"));
        }
        Ok(())
    }

    /// Release one level of nesting.
    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// The token the parser is looking at.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    /// Consume the token the parser is looking at when it is `token`.
    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.index += 1;
            return true;
        }
        false
    }

    /// Parse everything binding at least as tightly as `minimum`.
    ///
    /// Every recursion of the descent passes through here or through [`Self::unary`], so the
    /// nesting charged in both is what bounds the parser's depth — a parenthesised group, an
    /// argument, the right-hand side of an operator and a prefix chain alike.
    fn expression(&mut self, minimum: u8) -> Result<Expression, MathError> {
        self.enter()?;
        let parsed = self.binary(minimum);
        self.leave();
        parsed
    }

    /// Parse an operator chain whose nesting is already charged against [`MAX_NESTING`].
    fn binary(&mut self, minimum: u8) -> Result<Expression, MathError> {
        let mut left = self.unary()?;
        loop {
            // A `-` is subtraction here and negation in prefix position, which is what tells
            // `m1 - m2` from `-m1`; the tokeniser leaves the decision to this parser.
            let operator = match self.peek() {
                Some(&Token::Operator(operator)) => operator,
                Some(&Token::Minus) => Operator::Subtract,
                _ => break,
            };
            if operator.precedence() < minimum {
                break;
            }
            self.index += 1;
            let next = if operator.right_associative() {
                operator.precedence()
            } else {
                operator.precedence() + 1
            };
            let right = self.expression(next)?;
            left = Expression::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// Parse a prefixed term.
    fn unary(&mut self) -> Result<Expression, MathError> {
        self.enter()?;
        let parsed = self.prefixed();
        self.leave();
        parsed
    }

    /// Parse a prefixed term whose nesting is already charged against [`MAX_NESTING`].
    fn prefixed(&mut self) -> Result<Expression, MathError> {
        if self.eat(&Token::Minus) {
            return Ok(Expression::Negate(Box::new(self.unary()?)));
        }
        if self.eat(&Token::Operator(Operator::Add)) {
            return self.unary();
        }
        if self.eat(&Token::Bang) {
            return Ok(Expression::Not(Box::new(self.unary()?)));
        }
        self.term()
    }

    /// Parse one term: a literal, a reference, a call, an array or a parenthesised group.
    fn term(&mut self) -> Result<Expression, MathError> {
        let Some(token) = self.peek().cloned() else {
            return Err(MathError::new("Invalid syntax"));
        };
        self.index += 1;
        self.nested(token)
    }

    /// Parse one term whose nesting is already charged against [`MAX_NESTING`].
    fn nested(&mut self, token: Token) -> Result<Expression, MathError> {
        match token {
            Token::Number(number) => Ok(Expression::Number(number)),
            Token::Text(text) => Ok(Expression::Text(text)),
            Token::Word(word) => {
                if self.eat(&Token::Open) {
                    let mut arguments = self.arguments(&Token::Close)?;
                    // A bare word in one of this function's KEYWORD positions names no query:
                    // it is the fill mode, or the statistic and order a sort takes. Marking it
                    // here is what keeps every reader of the parse — the evaluator and the
                    // alarm's id check alike — from reading it as a reference.
                    for (position, argument) in arguments.iter_mut().enumerate() {
                        if let Expression::Reference(spelled) = argument {
                            if keywords_at(&word, position)
                                .iter()
                                .any(|keyword| keyword.eq_ignore_ascii_case(spelled))
                            {
                                *argument = Expression::Keyword(spelled.clone());
                            }
                        }
                    }
                    return Ok(Expression::Call {
                        name: word,
                        arguments,
                    });
                }
                Ok(Expression::Reference(word))
            }
            Token::Open => {
                let inner = self.expression(0)?;
                if !self.eat(&Token::Close) {
                    return Err(MathError::new("Invalid syntax"));
                }
                Ok(inner)
            }
            Token::OpenArray => Ok(Expression::Array(self.arguments(&Token::CloseArray)?)),
            _ => Err(MathError::new("Invalid syntax")),
        }
    }

    /// Parse a comma-separated argument list up to and including `closing`.
    fn arguments(&mut self, closing: &Token) -> Result<Vec<Expression>, MathError> {
        let mut arguments = Vec::new();
        if self.eat(closing) {
            return Ok(arguments);
        }
        loop {
            arguments.push(self.expression(0)?);
            if self.eat(closing) {
                return Ok(arguments);
            }
            if !self.eat(&Token::Comma) {
                return Err(MathError::new("Invalid syntax"));
            }
        }
    }
}

/// Everything an expression is evaluated against: the values of the queries it may refer to,
/// the series the request retrieves, where a search finds series, and the window the request
/// asked for.
pub struct MathScope<'a> {
    /// The value of every query id the expression may refer to.
    pub resolved: &'a HashMap<String, MathValue>,
    /// Every series the request RETRIEVES, in request order, each paired with the metric name
    /// it was retrieved for — what `METRICS()` returns and what its filter matches against.
    pub retrieved: &'a [(String, MathSeries)],
    /// Where a `SEARCH` call finds its series.
    pub source: &'a dyn SeriesSource,
    /// The first instant of the requested range, in epoch seconds.
    pub start: f64,
    /// The first instant after the requested range, in epoch seconds.
    pub end: f64,
    /// The granularity an expression's own points take, in seconds.
    pub period: f64,
}

/// One evaluation of one expression, which is also what counts the points it dropped and the
/// work it has spent.
pub struct Evaluation<'a> {
    /// What the expression is evaluated against.
    scope: &'a MathScope<'a>,
    /// How many points the arithmetic dropped because it produced a value that is not a
    /// finite number.
    dropped: usize,
    /// How many data points this evaluation has visited, against [`MAX_VISITED_POINTS`].
    visited: usize,
}

/// How many data points one expression may visit, over every series and every call it reads,
/// so the work one request can ask for is bounded whatever its calls nest.
const MAX_VISITED_POINTS: usize = 2_000_000;

/// The refusal for an expression that asks for more work than [`MAX_VISITED_POINTS`].
const BUDGET_SPENT: &str = "the expression reads more data points than one request evaluates";

impl<'a> Evaluation<'a> {
    /// An evaluation against `scope` that has spent none of the budget.
    pub fn new(scope: &'a MathScope<'a>) -> Self {
        Self::resumed(scope, 0)
    }

    /// An evaluation against `scope` carrying the work the REQUEST has already spent, so the
    /// budget bounds the request rather than each of its expressions.
    pub fn resumed(scope: &'a MathScope<'a>, spent: usize) -> Self {
        Self {
            scope,
            dropped: 0,
            visited: spent,
        }
    }

    /// How much of the budget the request has spent once this expression is evaluated.
    pub fn spent(&self) -> usize {
        self.visited
    }

    /// How many points this evaluation dropped for producing a value that is not a finite
    /// number: "the math expression could not be computed" for those time stamps.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Charge `points` to this evaluation's budget, answering whether it still has one.
    ///
    /// Every loop over a series' points charges what it is about to visit, so the work one
    /// expression can ask for is bounded however its calls nest — and a loop stops where the
    /// budget does rather than after it.
    fn charge(&mut self, points: usize) -> bool {
        self.visited = self.visited.saturating_add(points);
        self.visited <= MAX_VISITED_POINTS
    }

    /// Whether the budget is spent.
    fn exhausted(&self) -> bool {
        self.visited > MAX_VISITED_POINTS
    }

    /// The value of `expression`, or the reason it has none — including a budget the
    /// expression asked for more work than.
    pub fn value(&mut self, expression: &Expression) -> Result<MathValue, MathError> {
        if self.exhausted() {
            return Err(MathError::new(BUDGET_SPENT));
        }
        let value = self.evaluate(expression)?;
        if self.exhausted() {
            return Err(MathError::new(BUDGET_SPENT));
        }
        Ok(value)
    }

    /// The value of one node of the expression.
    fn evaluate(&mut self, expression: &Expression) -> Result<MathValue, MathError> {
        match expression {
            Expression::Number(number) => Ok(MathValue::Scalar(*number)),
            Expression::Text(text) => Err(MathError::new(format!(
                "the string '{text}' is not a metric math value"
            ))),
            Expression::Keyword(word) => Err(MathError::new(format!(
                "{word} is a keyword of the function that takes it, not a value"
            ))),
            Expression::Reference(id) => {
                // Resolving a reference COPIES the value the other query holds, so the copy is
                // charged: an expression that names the same id many times pays for each copy
                // and cannot amplify one stored series into unbounded memory.
                let value = self
                    .scope
                    .resolved
                    .get(id)
                    .cloned()
                    .ok_or_else(|| MathError::new(format!("ID '{id}' not found")))?;
                if !self.charge(points_in(&value)) {
                    return Err(MathError::new(BUDGET_SPENT));
                }
                Ok(value)
            }
            Expression::Array(items) => {
                let mut series = Vec::with_capacity(items.len());
                for item in items {
                    let value = self.value(item)?;
                    // The array's own value holds the item's points, so they are charged as
                    // they are appended — the count below bounds how many SERIES it holds, and
                    // this bounds how many POINTS they carry.
                    if !self.charge(points_in(&value)) {
                        return Err(MathError::new(BUDGET_SPENT));
                    }
                    match value {
                        MathValue::Series(one) => series.push(one),
                        MathValue::Array(many) => series.extend(many),
                        MathValue::Scalar(_) => {
                            return Err(MathError::new(
                                "an array of metrics holds time series, not numbers",
                            ))
                        }
                    }
                    // An array literal is the one place an expression can make MORE series
                    // than it read, so the count is checked as it grows rather than after: a
                    // nest of arrays of arrays cannot multiply without bound.
                    if series.len() > MAX_ARRAY_SERIES {
                        return Err(MathError::new(format!(
                            "an expression holds at most {MAX_ARRAY_SERIES} time series"
                        )));
                    }
                }
                Ok(MathValue::Array(series))
            }
            Expression::Negate(inner) => {
                let value = self.value(inner)?;
                self.map_points(value, |point| -point)
            }
            Expression::Not(inner) => {
                let value = self.value(inner)?;
                self.map_points(value, |point| if point == 0.0 { 1.0 } else { 0.0 })
            }
            Expression::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.value(left)?;
                let right = self.value(right)?;
                self.combine(*operator, left, right)
            }
            Expression::Call { name, arguments } => self.call(name, arguments),
        }
    }

    /// Apply `transform` to every number of `value`, dropping what it makes non-finite.
    fn map_points(
        &mut self,
        value: MathValue,
        transform: impl Fn(f64) -> f64 + Copy,
    ) -> Result<MathValue, MathError> {
        Ok(match value {
            MathValue::Scalar(number) => MathValue::Scalar(transform(number)),
            MathValue::Series(series) => MathValue::Series(self.transform_series(series, transform)),
            MathValue::Array(many) => MathValue::Array(
                many.into_iter()
                    .map(|series| self.transform_series(series, transform))
                    .collect(),
            ),
        })
    }

    /// `transform` over one series' points, dropping what it makes non-finite.
    fn transform_series(
        &mut self,
        series: MathSeries,
        transform: impl Fn(f64) -> f64,
    ) -> MathSeries {
        let mut points = Vec::with_capacity(series.points.len());
        for (timestamp, value) in series.points {
            if !self.charge(1) {
                break;
            }
            let computed = transform(value);
            if computed.is_finite() {
                points.push((timestamp, computed));
            } else {
                self.dropped += 1;
            }
        }
        MathSeries {
            label: series.label,
            points,
            // A transformed series is a new series, not the metric's own.
            metric: None,
        }
    }

    /// Combine two values with `operator`.
    fn combine(
        &mut self,
        operator: Operator,
        left: MathValue,
        right: MathValue,
    ) -> Result<MathValue, MathError> {
        match (left, right) {
            (MathValue::Scalar(left), MathValue::Scalar(right)) => {
                Ok(MathValue::Scalar(operator.apply(left, right)))
            }
            (MathValue::Series(series), MathValue::Scalar(number)) => Ok(MathValue::Series(
                self.transform_series(series, |value| operator.apply(value, number)),
            )),
            (MathValue::Scalar(number), MathValue::Series(series)) => Ok(MathValue::Series(
                self.transform_series(series, |value| operator.apply(number, value)),
            )),
            (MathValue::Array(many), MathValue::Scalar(number)) => Ok(MathValue::Array(
                many.into_iter()
                    .map(|series| self.transform_series(series, |value| operator.apply(value, number)))
                    .collect(),
            )),
            (MathValue::Scalar(number), MathValue::Array(many)) => Ok(MathValue::Array(
                many.into_iter()
                    .map(|series| self.transform_series(series, |value| operator.apply(number, value)))
                    .collect(),
            )),
            (MathValue::Series(left), MathValue::Series(right)) => {
                let aligned = self.index_of(&right)?;
                Ok(MathValue::Series(
                    self.zip_indexed(operator, left, &aligned, false),
                ))
            }
            (MathValue::Array(many), MathValue::Series(right)) => {
                // The other operand is indexed ONCE, before the walk over the array: an index
                // rebuilt per series would make the work the product of the array's length and
                // the series' rather than their sum.
                let aligned = self.index_of(&right)?;
                Ok(MathValue::Array(
                    many.into_iter()
                        .map(|series| self.zip_indexed(operator, series, &aligned, false))
                        .collect(),
                ))
            }
            (MathValue::Series(left), MathValue::Array(many)) => {
                let aligned = self.index_of(&left)?;
                Ok(MathValue::Array(
                    many.into_iter()
                        .map(|series| self.zip_indexed(operator, series, &aligned, true))
                        .collect(),
                ))
            }
            (MathValue::Array(_), MathValue::Array(_)) => Err(MathError::new(
                "an operator combines a time series or a number with an array of time series, \
                 not two arrays",
            )),
        }
    }

    /// One series' points keyed by time stamp, charged: building the index is what aligning
    /// two series costs, so no caller can obtain one for free.
    fn index_of(&mut self, series: &MathSeries) -> Result<BTreeMap<i64, f64>, MathError> {
        if !self.charge(series.points.len()) {
            return Err(MathError::new(BUDGET_SPENT));
        }
        Ok(point_map(series))
    }

    /// `operator` over the UNION of the time stamps `series` and the series `aligned` indexes
    /// carry: an operand with no datapoint at a time stamp the other one has is taken as 0, so
    /// the bucket still yields a value.
    ///
    /// CloudWatch answers an arithmetic expression over two metrics for every period in which
    /// ANY operand has data, with the ABSENT operand read as zero. The recorded expressions
    /// pin that from three sides: `m1 + m2` keeps the period in which only `m1` was published
    /// and over a metric nothing has ever written the sum is the other operand's own series,
    /// while `m1 * m2` answers that same period with ZERO rather than with the present
    /// operand's value (which is what ignoring the absent one would give) and rather than
    /// leaving the period out (which is what dropping the bucket would give). So the rule is
    /// the operand, not the operator: a period either side has data in is answered, and the
    /// side that has none contributes 0 to whatever the operator does with it — a product with
    /// it is zero, and a quotient by it is no finite number and so no datapoint at all.
    ///
    /// `indexed_is_left` says which side of the operator the INDEXED series is, so an operator
    /// that does not commute is applied the way the expression wrote it.
    fn zip_indexed(
        &mut self,
        operator: Operator,
        series: MathSeries,
        aligned: &BTreeMap<i64, f64>,
        indexed_is_left: bool,
    ) -> MathSeries {
        // The union of the two sides' time stamps, in time order: this series' own points,
        // plus the keys only the indexed series carries. A `BTreeMap` is what orders them,
        // oldest first, however the two series were spaced.
        let mut union: BTreeMap<i64, Option<f64>> = series
            .points
            .iter()
            .map(|(timestamp, value)| (time_key(*timestamp), Some(*value)))
            .collect();
        for key in aligned.keys() {
            union.entry(*key).or_insert(None);
        }
        let mut points = Vec::with_capacity(union.len());
        for (key, value) in union {
            if !self.charge(1) {
                break;
            }
            // The operand with no datapoint in this bucket counts as 0 on either side.
            let own = value.unwrap_or(0.0);
            let other = aligned.get(&key).copied().unwrap_or(0.0);
            let (left, right) = if indexed_is_left {
                (other, own)
            } else {
                (own, other)
            };
            let computed = operator.apply(left, right);
            if computed.is_finite() {
                points.push((key_time(key), computed));
            } else {
                self.dropped += 1;
            }
        }
        MathSeries {
            label: series.label,
            points,
            // A combined series is a new series, not the metric's own.
            metric: None,
        }
    }

    /// Evaluate one function call.
    #[allow(clippy::too_many_lines)]
    fn call(&mut self, name: &str, arguments: &[Expression]) -> Result<MathValue, MathError> {
        let upper = name.to_ascii_uppercase();
        // The NAME is resolved before any argument is evaluated, so a call to a function
        // nothing here computes costs only the parse — not the evaluation of everything it
        // was passed.
        if !KNOWN_FUNCTIONS.contains(&upper.as_str()) {
            return Err(MathError::new(unknown_function(name)));
        }
        // The functions that take a bare word or a STRING rather than a value are evaluated
        // from the argument expressions themselves.
        match upper.as_str() {
            "METRICS" => return self.metrics(arguments),
            "SEARCH" => return self.search(arguments),
            "FILL" => return self.fill(arguments),
            // `SORT`'s statistic and order are bare words rather than values, so only its
            // first argument is evaluated as one.
            "SORT" => return self.sort(arguments),
            _ => {}
        }

        let mut values = Vec::with_capacity(arguments.len());
        for argument in arguments {
            values.push(self.value(argument)?);
        }

        match upper.as_str() {
            "SUM" | "AVG" | "MIN" | "MAX" | "STDDEV" => {
                let aggregator = Aggregator::named(&upper)
                    .ok_or_else(|| MathError::new(unknown_function(name)))?;
                self.aggregate(aggregator, &upper, values)
            }
            "ABS" => self.one_point_function(&upper, values, f64::abs),
            "CEIL" => self.one_point_function(&upper, values, f64::ceil),
            "FLOOR" => self.one_point_function(&upper, values, f64::floor),
            "LOG" => self.one_point_function(&upper, values, f64::ln),
            "LOG10" => self.one_point_function(&upper, values, f64::log10),
            "SQRT" => self.one_point_function(&upper, values, f64::sqrt),
            "RATE" | "DIFF" | "DIFF_TIME" | "RUNNING_SUM" | "INTEGRAL" => {
                self.sequence_function(&upper, values)
            }
            "METRIC_COUNT" => {
                let value = one_argument(&upper, values)?;
                #[allow(clippy::cast_precision_loss)]
                Ok(MathValue::Scalar(match value {
                    MathValue::Array(many) => many.len() as f64,
                    MathValue::Series(_) => 1.0,
                    MathValue::Scalar(_) => {
                        return Err(wrong_operand(&upper));
                    }
                }))
            }
            "DATAPOINT_COUNT" => self.datapoint_count(&upper, values),
            "PERIOD" => {
                let value = one_argument(&upper, values)?;
                match value {
                    MathValue::Series(series) => Ok(MathValue::Scalar(period_of(
                        &series,
                        self.scope.period,
                    ))),
                    _ => Err(wrong_operand(&upper)),
                }
            }
            "FIRST" | "LAST" => {
                let value = one_argument(&upper, values)?;
                let last = upper == "LAST";
                match value {
                    MathValue::Array(mut many) => {
                        if many.is_empty() {
                            return Ok(MathValue::Array(Vec::new()));
                        }
                        let picked = if last {
                            many.pop()
                        } else {
                            Some(many.swap_remove(0))
                        };
                        picked.map(MathValue::Series).ok_or_else(|| {
                            MathError::new(format!("{upper} needs at least one time series"))
                        })
                    }
                    MathValue::Series(series) => {
                        let point = if last {
                            series.points.last()
                        } else {
                            series.points.first()
                        };
                        Ok(MathValue::Scalar(
                            point.map_or(f64::NAN, |(_, value)| *value),
                        ))
                    }
                    MathValue::Scalar(_) => Err(wrong_operand(&upper)),
                }
            }
            "REMOVE_EMPTY" => {
                let value = one_argument(&upper, values)?;
                match value {
                    MathValue::Array(many) => Ok(MathValue::Array(
                        many.into_iter()
                            .filter(|series| !series.points.is_empty())
                            .collect(),
                    )),
                    MathValue::Series(series) => Ok(MathValue::Array(
                        (!series.points.is_empty())
                            .then_some(vec![series])
                            .unwrap_or_default(),
                    )),
                    MathValue::Scalar(_) => Err(wrong_operand(&upper)),
                }
            }
            "TIME_SERIES" => {
                let value = one_argument(&upper, values)?;
                match value {
                    MathValue::Scalar(number) => {
                        let grid = self.grid();
                        let _ = self.charge(grid.len());
                        Ok(MathValue::Series(MathSeries::new(
                            String::new(),
                            grid.into_iter().map(|at| (at, number)).collect(),
                        )))
                    }
                    _ => Err(wrong_operand(&upper)),
                }
            }
            "IF" => self.conditional(values),
            "SLICE" => Self::slice(values),
            "ANOMALY_DETECTION_BAND" => self.anomaly_detection_band(values),
            _ => Err(MathError::new(unknown_function(name))),
        }
    }

    /// `METRICS()` — every series the request retrieves, optionally filtered by a string the
    /// metric name must contain.
    fn metrics(&mut self, arguments: &[Expression]) -> Result<MathValue, MathError> {
        let filter = match arguments {
            [] => None,
            [Expression::Text(filter)] => Some(filter.to_ascii_lowercase()),
            _ => {
                return Err(MathError::new(
                    "METRICS takes no argument, or one quoted filter",
                ))
            }
        };
        let series: Vec<MathSeries> = self
            .scope
            .retrieved
            .iter()
            .filter(|(metric_name, _)| {
                filter.as_ref().is_none_or(|filter| {
                    metric_name.to_ascii_lowercase().contains(filter.as_str())
                })
            })
            .map(|(_, series)| series.clone())
            .collect();
        let points: usize = series.iter().map(|one| one.points.len()).sum();
        if !self.charge(points) {
            return Err(MathError::new(BUDGET_SPENT));
        }
        Ok(MathValue::Array(series))
    }

    /// `SEARCH(query, stat, period)` — the series the store holds that the query matches.
    fn search(&mut self, arguments: &[Expression]) -> Result<MathValue, MathError> {
        let query = match arguments.first() {
            Some(Expression::Text(query)) => query.clone(),
            _ => {
                return Err(MathError::new(
                    "SEARCH takes a quoted search expression, a statistic and a period",
                ))
            }
        };
        let stat = match arguments.get(1) {
            None => DEFAULT_SEARCH_STAT.to_string(),
            Some(Expression::Text(stat)) => stat.clone(),
            Some(_) => return Err(MathError::new("SEARCH's statistic is a quoted name")),
        };
        let period = match arguments.get(2) {
            None => self.scope.period,
            Some(Expression::Number(period)) => *period,
            Some(_) => return Err(MathError::new("SEARCH's period is a number of seconds")),
        };
        if arguments.len() > 3 {
            return Err(MathError::new(
                "SEARCH takes a search expression, a statistic and a period",
            ));
        }
        let found = self.scope.source.search(&query, &stat, period)?;
        // A search is the one value an expression does not compute from what it already has,
        // so BOTH what it scanned and what it found are charged: an expression cannot pay
        // nothing for a walk over the store.
        let points: usize = found
            .series
            .iter()
            .map(|series| series.points.len())
            .sum();
        if !self.charge(found.scanned.saturating_add(points)) {
            return Err(MathError::new(BUDGET_SPENT));
        }
        Ok(MathValue::Array(found.series))
    }

    /// `FILL(series, value)` — the same series with the window's empty periods filled.
    fn fill(&mut self, arguments: &[Expression]) -> Result<MathValue, MathError> {
        let [source, filler] = arguments else {
            return Err(MathError::new("FILL takes a time series and a fill value"));
        };
        let value = self.value(source)?;
        // "FILL(m1, REPEAT)" — a fill mode is written as a bare word (which the parse marked
        // as the keyword it is) or as a quoted one; anything else is a value to fill with.
        let named = match filler {
            Expression::Text(mode) | Expression::Keyword(mode) => Some(mode.as_str()),
            _ => None,
        };
        let mode = match named {
            Some(name) => match FillMode::named(name) {
                Some(mode) => mode,
                None if matches!(filler, Expression::Text(_)) => {
                    return Err(MathError::new(format!("FILL cannot fill with '{name}'")))
                }
                None => self.fill_value(filler)?,
            },
            None => self.fill_value(filler)?,
        };
        let grid = self.grid();
        match value {
            MathValue::Series(series) => {
                // What the fill is about to visit is charged BEFORE it visits it: one grid
                // per series.
                if !self.charge(grid.len()) {
                    return Err(MathError::new(BUDGET_SPENT));
                }
                Ok(MathValue::Series(fill_series(&series, &grid, mode)))
            }
            MathValue::Array(many) => {
                if !self.charge(grid.len().saturating_mul(many.len())) {
                    return Err(MathError::new(BUDGET_SPENT));
                }
                Ok(MathValue::Array(
                    many.iter()
                        .map(|series| fill_series(series, &grid, mode))
                        .collect(),
                ))
            }
            MathValue::Scalar(_) => Err(wrong_operand("FILL")),
        }
    }

    /// The number a `FILL` call fills with, when it does not name a mode.
    fn fill_value(&mut self, filler: &Expression) -> Result<FillMode, MathError> {
        match self.value(filler)? {
            MathValue::Scalar(number) => Ok(FillMode::Value(number)),
            _ => Err(MathError::new(
                "FILL fills with a number, REPEAT or LINEAR",
            )),
        }
    }

    /// `SUM` / `AVG` / `MIN` / `MAX` / `STDDEV`: over one series' points it is a number, and
    /// over an array of series it is the series of that statistic at each time stamp.
    fn aggregate(
        &mut self,
        aggregator: Aggregator,
        name: &str,
        values: Vec<MathValue>,
    ) -> Result<MathValue, MathError> {
        match one_argument(name, values)? {
            MathValue::Series(series) => {
                let numbers: Vec<f64> = series.points.iter().map(|(_, value)| *value).collect();
                let _ = self.charge(numbers.len());
                Ok(MathValue::Scalar(
                    aggregator.of(&numbers).unwrap_or(f64::NAN),
                ))
            }
            MathValue::Array(many) => {
                let mut columns: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
                for series in &many {
                    for (timestamp, value) in &series.points {
                        if !self.charge(1) {
                            break;
                        }
                        columns
                            .entry(time_key(*timestamp))
                            .or_default()
                            .push(*value);
                    }
                }
                let mut points = Vec::with_capacity(columns.len());
                for (key, numbers) in columns {
                    if let Some(value) = aggregator.of(&numbers) {
                        if value.is_finite() {
                            points.push((key_time(key), value));
                        } else {
                            self.dropped += 1;
                        }
                    }
                }
                Ok(MathValue::Series(MathSeries::new(String::new(), points)))
            }
            MathValue::Scalar(_) => Err(wrong_operand(name)),
        }
    }

    /// A function that is applied to each point on its own.
    fn one_point_function(
        &mut self,
        name: &str,
        values: Vec<MathValue>,
        transform: impl Fn(f64) -> f64 + Copy,
    ) -> Result<MathValue, MathError> {
        let value = one_argument(name, values)?;
        self.map_points(value, transform)
    }

    /// A function whose value at a point depends on the point before it.
    fn sequence_function(
        &mut self,
        name: &str,
        values: Vec<MathValue>,
    ) -> Result<MathValue, MathError> {
        let value = one_argument(name, values)?;
        match value {
            MathValue::Series(series) => {
                Ok(MathValue::Series(self.sequence_series(name, &series)))
            }
            MathValue::Array(many) => Ok(MathValue::Array(
                many.iter()
                    .map(|series| self.sequence_series(name, series))
                    .collect(),
            )),
            MathValue::Scalar(_) => Err(wrong_operand(name)),
        }
    }

    /// One series under a function whose value at a point depends on the point before it.
    fn sequence_series(&mut self, name: &str, series: &MathSeries) -> MathSeries {
        let mut points = Vec::with_capacity(series.points.len());
        let mut running = 0.0;
        for (index, (timestamp, value)) in series.points.iter().enumerate() {
            if !self.charge(1) {
                break;
            }
            let previous = index.checked_sub(1).and_then(|before| series.points.get(before));
            let computed = match name {
                "RATE" => previous.map(|(before, earlier)| (value - earlier) / (timestamp - before)),
                "DIFF" => previous.map(|(_, earlier)| value - earlier),
                "DIFF_TIME" => previous.map(|(before, _)| timestamp - before),
                "RUNNING_SUM" => {
                    running += value;
                    Some(running)
                }
                // "The integral of the metric": the value accumulated over the time it covers,
                // which is the value times the span since the point before it.
                "INTEGRAL" => {
                    let span = previous.map_or(self.scope.period, |(before, _)| timestamp - before);
                    running += value * span;
                    Some(running)
                }
                _ => None,
            };
            // The first point of a difference has no point before it, so the function has no
            // value there — which is a gap, not a dropped computation.
            if let Some(computed) = computed {
                if computed.is_finite() {
                    points.push((*timestamp, computed));
                } else {
                    self.dropped += 1;
                }
            }
        }
        MathSeries {
            label: series.label.clone(),
            points,
            // A sequence function's series is a new series, not the metric's own.
            metric: None,
        }
    }

    /// `DATAPOINT_COUNT`: how many points one series carries, or how many series carry a
    /// point at each time stamp of an array.
    fn datapoint_count(
        &mut self,
        name: &str,
        values: Vec<MathValue>,
    ) -> Result<MathValue, MathError> {
        #[allow(clippy::cast_precision_loss)]
        match one_argument(name, values)? {
            MathValue::Series(series) => Ok(MathValue::Scalar(series.points.len() as f64)),
            MathValue::Array(many) => {
                let mut counts: BTreeMap<i64, f64> = BTreeMap::new();
                for series in &many {
                    for (timestamp, _) in &series.points {
                        if !self.charge(1) {
                            break;
                        }
                        *counts.entry(time_key(*timestamp)).or_default() += 1.0;
                    }
                }
                Ok(MathValue::Series(MathSeries::new(
                    String::new(),
                    counts
                        .into_iter()
                        .map(|(key, count)| (key_time(key), count))
                        .collect(),
                )))
            }
            MathValue::Scalar(_) => Err(wrong_operand(name)),
        }
    }

    /// `IF(condition, then, else)`, decided point by point.
    fn conditional(&mut self, values: Vec<MathValue>) -> Result<MathValue, MathError> {
        let [condition, when_true, when_false] = values.as_slice() else {
            return Err(MathError::new(
                "IF takes a condition, a value when it holds and a value when it does not",
            ));
        };
        match condition {
            MathValue::Scalar(number) => Ok(if *number == 0.0 {
                when_false.clone()
            } else {
                when_true.clone()
            }),
            MathValue::Series(series) => {
                // Each branch is indexed ONCE, before the walk over the condition's points:
                // a lookup per point over a freshly built index would make the work the
                // product of the two series' lengths rather than the sum.
                let on_true = branch_index(when_true);
                let on_false = branch_index(when_false);
                if !self.charge(on_true.points().saturating_add(on_false.points())) {
                    return Err(MathError::new(BUDGET_SPENT));
                }
                let mut points = Vec::with_capacity(series.points.len());
                for (timestamp, holds) in &series.points {
                    if !self.charge(1) {
                        break;
                    }
                    let branch = if *holds == 0.0 { &on_false } else { &on_true };
                    let Some(value) = branch.at(*timestamp) else {
                        continue;
                    };
                    if value.is_finite() {
                        points.push((*timestamp, value));
                    } else {
                        self.dropped += 1;
                    }
                }
                Ok(MathValue::Series(MathSeries::new(
                    series.label.clone(),
                    points,
                )))
            }
            MathValue::Array(_) => Err(MathError::new(
                "IF's condition is a time series or a number, not an array of time series",
            )),
        }
    }

    /// `ANOMALY_DETECTION_BAND(series, width)` — the two series that bound the values the
    /// anomaly DETECTOR created for the metric expects it in.
    ///
    /// The band is the detector's: only a trained model says what a value is expected to be,
    /// and the band reaches `width` of that model's deviation either side of it. So the model
    /// is LOOKED UP for the series ([`SeriesSource::anomaly_model`]) and a series no detector
    /// was created for has no band: its two bounds are reported under their labels with no
    /// points, which is what a band over a metric without a detector is.
    fn anomaly_detection_band(&mut self, values: Vec<MathValue>) -> Result<MathValue, MathError> {
        let (series, width) = match values.as_slice() {
            [MathValue::Series(series)] => (series, DEFAULT_BAND_WIDTH),
            [MathValue::Series(series), MathValue::Scalar(width)] => (series, *width),
            _ => {
                return Err(MathError::new(
                    "ANOMALY_DETECTION_BAND takes one time series and a band width",
                ))
            }
        };
        if width <= 0.0 {
            return Err(MathError::new(
                "ANOMALY_DETECTION_BAND's band width is greater than zero",
            ));
        }
        // The history the model is trained on is read out of the store, and reading it is a
        // walk over that metric's stored datapoints — so it is charged to this evaluation's
        // budget BEFORE it runs, the way every other loop over a series' points is, and an
        // expression that asks for more of them than one request evaluates is refused rather
        // than answered.
        let training = self.scope.source.anomaly_training_datapoints(series);
        if !self.charge(training) {
            return Err(MathError::new(BUDGET_SPENT));
        }
        let model = self.scope.source.anomaly_model(series);
        let (expected, deviation) = model.map_or_else(
            || (Vec::new(), Vec::new()),
            |model| (model.expected, model.deviation),
        );
        if !self.charge(expected.len()) {
            return Err(MathError::new(BUDGET_SPENT));
        }
        let bound = |sign: f64| -> Vec<(f64, f64)> {
            expected
                .iter()
                .zip(&deviation)
                .map(|((timestamp, value), reach)| (*timestamp, value + sign * width * reach))
                .filter(|(_, value)| value.is_finite())
                .collect()
        };
        Ok(MathValue::Array(vec![
            MathSeries::new(format!("{}{LOWER_BAND_SUFFIX}", series.label), bound(-1.0)),
            MathSeries::new(format!("{}{UPPER_BAND_SUFFIX}", series.label), bound(1.0)),
        ]))
    }

    /// `SORT(array, function, order, count)` — the array's series ordered by one statistic of
    /// each of them, and at most `count` of them.
    fn sort(&mut self, arguments: &[Expression]) -> Result<MathValue, MathError> {
        // The statistic and the order are keywords rather than values, so they are read from
        // the argument expressions themselves.
        let keyword = |index: usize| match arguments.get(index) {
            Some(Expression::Keyword(word) | Expression::Text(word)) => {
                Some(word.to_ascii_uppercase())
            }
            _ => None,
        };
        let (Some(statistic), Some(order)) = (keyword(1), keyword(2)) else {
            return Err(MathError::new(
                "SORT takes an array of time series, a statistic, an order and a count",
            ));
        };
        let Some(aggregator) = Aggregator::named(&statistic) else {
            return Err(MathError::new(format!(
                "SORT cannot order by {statistic}"
            )));
        };
        let descending = match order.as_str() {
            "DESC" => true,
            "ASC" => false,
            other => {
                return Err(MathError::new(format!(
                    "SORT's order is ASC or DESC, not {other}"
                )))
            }
        };
        let count = match arguments.get(3) {
            None => None,
            Some(Expression::Number(count)) if *count >= 0.0 => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(count.trunc() as usize)
            }
            Some(_) => return Err(MathError::new("SORT's count is a number of time series")),
        };
        if arguments.len() > 4 {
            return Err(MathError::new(
                "SORT takes an array of time series, a statistic, an order and a count",
            ));
        }
        let many = match arguments.first().map(|first| self.value(first)).transpose()? {
            Some(MathValue::Array(many)) => many,
            Some(MathValue::Series(series)) => vec![series],
            _ => return Err(wrong_operand("SORT")),
        };
        // One key per series, computed ONCE and charged: a key recomputed inside the
        // comparator would fold every series' points as many times as the sort compares it.
        let mut keyed: Vec<(f64, MathSeries)> = Vec::with_capacity(many.len());
        for series in many {
            if !self.charge(series.points.len()) {
                return Err(MathError::new(BUDGET_SPENT));
            }
            let numbers: Vec<f64> = series.points.iter().map(|(_, value)| *value).collect();
            let key = aggregator.of(&numbers).unwrap_or(f64::NEG_INFINITY);
            keyed.push((key, series));
        }
        keyed.sort_by(|(left, _), (right, _)| {
            let ordering = left.total_cmp(right);
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        let mut many: Vec<MathSeries> = keyed.into_iter().map(|(_, series)| series).collect();
        if let Some(count) = count {
            many.truncate(count);
        }
        Ok(MathValue::Array(many))
    }

    /// `SLICE(array, start, end)` — the series of the array from `start` up to but not
    /// including `end`.
    fn slice(values: Vec<MathValue>) -> Result<MathValue, MathError> {
        let (many, start, end) = match values.as_slice() {
            [MathValue::Array(many), MathValue::Scalar(start), MathValue::Scalar(end)] => {
                (many, *start, *end)
            }
            _ => {
                return Err(MathError::new(
                    "SLICE takes an array of time series and the two indexes to cut it at",
                ))
            }
        };
        if start < 0.0 || end < start {
            return Err(MathError::new(
                "SLICE's first index is not negative and its second is not before its first",
            ));
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (from, to) = (start.trunc() as usize, end.trunc() as usize);
        Ok(MathValue::Array(
            many.iter()
                .skip(from)
                .take(to.saturating_sub(from))
                .cloned()
                .collect(),
        ))
    }

    /// Every time stamp the requested window holds at its granularity: the periods a filled
    /// or a constant series carries a point at.
    fn grid(&self) -> Vec<f64> {
        let mut grid = Vec::new();
        if self.scope.period <= 0.0 {
            return grid;
        }
        let mut at = self.scope.start;
        while at < self.scope.end && grid.len() < MAX_GRID_POINTS {
            grid.push(at);
            at += self.scope.period;
        }
        grid
    }
}

/// The statistic a `SEARCH` that names none folds its series by. "If you omit the statistic,
/// Average is used." (`SEARCH`, Metric Math Syntax and Functions)
const DEFAULT_SEARCH_STAT: &str = "Average";

/// The band width `ANOMALY_DETECTION_BAND` uses when the call names none: "If you omit this,
/// the default of 2 is used."
const DEFAULT_BAND_WIDTH: f64 = 2.0;

/// What the lower bound of an anomaly detection band is labelled with.
///
/// The label is what tells the two bounds of a band apart wherever one is read — the alarm
/// whose threshold IS a band reads the bound it compares against by it — so the suffix is
/// stated once here rather than at each reader.
pub(super) const LOWER_BAND_SUFFIX: &str = " Low";

/// What the upper bound of an anomaly detection band is labelled with.
pub(super) const UPPER_BAND_SUFFIX: &str = " High";

/// How many periods a constructed series spans at most, so an expression over a wide range at
/// a one-second granularity cannot allocate without bound.
const MAX_GRID_POINTS: usize = 100_800;

/// How many time series one expression's value may hold: the same 500 metrics one request can
/// retrieve ("can retrieve as many as 500 different metrics in a single request",
/// `GetMetricData`).
const MAX_ARRAY_SERIES: usize = 500;

/// How a `FILL` call fills a window's empty periods.
#[derive(Debug, Clone, Copy)]
enum FillMode {
    /// With one number.
    Value(f64),
    /// With the value of the nearest earlier point.
    Repeat,
    /// With the straight line between the points on either side.
    Linear,
}

impl FillMode {
    /// The mode a quoted or bare-word argument names, out of the one table the parse marks
    /// those words by ([`FILL_MODES`]).
    fn named(mode: &str) -> Option<Self> {
        FILL_MODES
            .iter()
            .find(|known| known.eq_ignore_ascii_case(mode))
            .and_then(|known| match *known {
                "REPEAT" => Some(Self::Repeat),
                "LINEAR" => Some(Self::Linear),
                _ => None,
            })
    }
}

/// `series` with a point at every time stamp of `grid`.
///
/// ONE pass over the grid: the next time stamp the series holds a point at is found once, from
/// the back, so a sparse series over a wide window costs the grid's length rather than its
/// square.
fn fill_series(series: &MathSeries, grid: &[f64], mode: FillMode) -> MathSeries {
    let held = point_map(series);
    // `next_held[i]` is the first `(time stamp, value)` the series holds at or after `grid[i]`.
    let mut next_held: Vec<Option<(f64, f64)>> = vec![None; grid.len()];
    let mut ahead: Option<(f64, f64)> = None;
    for (slot, at) in next_held.iter_mut().zip(grid).rev() {
        if let Some(value) = held.get(&time_key(*at)) {
            ahead = Some((*at, *value));
        }
        *slot = ahead;
    }

    let mut points: Vec<(f64, f64)> = Vec::with_capacity(grid.len());
    for (index, at) in grid.iter().enumerate() {
        if let Some(value) = held.get(&time_key(*at)) {
            points.push((*at, *value));
            continue;
        }
        let filled = match mode {
            FillMode::Value(number) => Some(number),
            FillMode::Repeat => points.last().map(|(_, value)| *value),
            FillMode::Linear => {
                let earlier = points.last().copied();
                let later = next_held.get(index).copied().flatten();
                match (earlier, later) {
                    (Some((before, from)), Some((after, to))) if after > before => {
                        Some(from + (to - from) * (at - before) / (after - before))
                    }
                    _ => None,
                }
            }
        };
        if let Some(value) = filled.filter(|value: &f64| value.is_finite()) {
            points.push((*at, value));
        }
    }
    MathSeries {
        label: series.label.clone(),
        points,
        // A filled series is a new series, not the metric's own.
        metric: None,
    }
}

/// One branch of an `IF`, indexed once so the walk over the condition's points is a lookup
/// per point rather than a rebuild per point.
enum Branch {
    /// A number, which the branch takes at every time stamp.
    Everywhere(f64),
    /// A series, which the branch takes only where the series carries a point.
    Where(BTreeMap<i64, f64>),
    /// An array of series, which is not a value a branch can take.
    Nowhere,
}

impl Branch {
    /// The branch's value at `timestamp`.
    fn at(&self, timestamp: f64) -> Option<f64> {
        match self {
            Self::Everywhere(number) => Some(*number),
            Self::Where(points) => points.get(&time_key(timestamp)).copied(),
            Self::Nowhere => None,
        }
    }

    /// How many points the index holds, which is what building it cost.
    fn points(&self) -> usize {
        match self {
            Self::Where(points) => points.len(),
            Self::Everywhere(_) | Self::Nowhere => 0,
        }
    }
}

/// Index one branch of an `IF`.
fn branch_index(value: &MathValue) -> Branch {
    match value {
        MathValue::Scalar(number) => Branch::Everywhere(*number),
        MathValue::Series(series) => Branch::Where(point_map(series)),
        MathValue::Array(_) => Branch::Nowhere,
    }
}

/// One series' points keyed by time stamp, which is how two series are aligned.
fn point_map(series: &MathSeries) -> BTreeMap<i64, f64> {
    series
        .points
        .iter()
        .map(|(timestamp, value)| (time_key(*timestamp), *value))
        .collect()
}

/// How many data points a value holds — what copying it costs.
fn points_in(value: &MathValue) -> usize {
    match value {
        MathValue::Scalar(_) => 0,
        MathValue::Series(series) => series.points.len(),
        MathValue::Array(many) => many.iter().map(|series| series.points.len()).sum(),
    }
}

/// A time stamp as the key two series are aligned on: whole milliseconds, so two series folded
/// into the same windows align exactly.
#[allow(clippy::cast_possible_truncation)]
fn time_key(timestamp: f64) -> i64 {
    (timestamp * 1000.0).round() as i64
}

/// The time stamp an alignment key stands for.
#[allow(clippy::cast_precision_loss)]
fn key_time(key: i64) -> f64 {
    key as f64 / 1000.0
}

/// The granularity a series' points are spaced at, or `fallback` when it carries fewer than
/// two of them.
fn period_of(series: &MathSeries, fallback: f64) -> f64 {
    let mut spacing = f64::INFINITY;
    for pair in series.points.windows(2) {
        if let [(before, _), (after, _)] = pair {
            spacing = spacing.min(after - before);
        }
    }
    if spacing.is_finite() && spacing > 0.0 {
        spacing
    } else {
        fallback
    }
}

/// The one value a one-argument function takes.
fn one_argument(name: &str, values: Vec<MathValue>) -> Result<MathValue, MathError> {
    let mut values = values;
    if values.len() != 1 {
        return Err(MathError::new(format!(
            "{name} takes one argument, got {}",
            values.len()
        )));
    }
    Ok(values.remove(0))
}

/// What a call to a function nothing here computes is refused with: the name the expression
/// called, quoted, and that no such function was found.
///
/// The same form the refusal of an id no query defines carries (`ID '<id>' not found`), which
/// is how this service states a name an expression refers to that does not resolve.
fn unknown_function(name: &str) -> String {
    format!("Function '{name}' not found")
}

/// The refusal for a call whose operand is not a kind of value it takes.
fn wrong_operand(name: &str) -> MathError {
    MathError::new(format!(
        "{name} cannot be computed over this kind of value"
    ))
}

/// A statistic over a set of numbers, which is both how a series folds to one number and how
/// an array of series folds to one series.
#[derive(Debug, Clone, Copy)]
enum Aggregator {
    /// Their sum.
    Sum,
    /// Their mean.
    Avg,
    /// The smallest of them.
    Min,
    /// The largest of them.
    Max,
    /// Their standard deviation.
    StdDev,
}

impl Aggregator {
    /// The aggregator a function name denotes.
    fn named(name: &str) -> Option<Self> {
        match name {
            "SUM" => Some(Self::Sum),
            "AVG" => Some(Self::Avg),
            "MIN" => Some(Self::Min),
            "MAX" => Some(Self::Max),
            "STDDEV" => Some(Self::StdDev),
            _ => None,
        }
    }

    /// The statistic over `numbers`, or `None` over none at all.
    fn of(self, numbers: &[f64]) -> Option<f64> {
        if numbers.is_empty() {
            return match self {
                Self::Sum => Some(0.0),
                _ => None,
            };
        }
        #[allow(clippy::cast_precision_loss)]
        let count = numbers.len() as f64;
        let sum: f64 = numbers.iter().sum();
        Some(match self {
            Self::Sum => sum,
            Self::Avg => sum / count,
            Self::Min => numbers.iter().copied().fold(f64::INFINITY, f64::min),
            Self::Max => numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Self::StdDev => {
                let mean = sum / count;
                (numbers
                    .iter()
                    .map(|number| (number - mean) * (number - mean))
                    .sum::<f64>()
                    / count)
                    .sqrt()
            }
        })
    }
}

/// A search query as the store matches series against it.
///
/// "`SEARCH('{Namespace, DimensionName1, DimensionName2} MetricName=\"name\"', 'Stat',
/// period)`" — the optional `{…}` schema fixes the namespace and the EXACT set of dimension
/// names a matching series carries; each remaining term narrows the match further, and the
/// terms are combined by the `AND`, `OR` and `NOT` the search syntax carries ("You can use
/// AND, OR and NOT", Metric Math Syntax and Functions).
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// The namespace a matching series is published under, when the query's schema fixes one.
    namespace: Option<String>,
    /// The exact set of dimension names a matching series carries, when the query gives a
    /// schema.
    dimension_names: Option<Vec<String>>,
    /// The term groups: a series matches when EVERY term of ANY group holds for it, which is
    /// the `OR` of the `AND`s the query spells out.
    groups: Vec<Vec<SearchTerm>>,
}

impl SearchQuery {
    /// Whether the series this names is one the query matches.
    ///
    /// A schema fixes the namespace and the exact set of dimension NAMES; every other term
    /// narrows the match without fixing the rest of the series' identity, which is what makes
    /// a query naming one dimension match a series carrying more.
    pub fn matches(
        &self,
        namespace: &str,
        metric_name: &str,
        dimensions: &BTreeMap<String, String>,
    ) -> bool {
        if self
            .namespace
            .as_ref()
            .is_some_and(|wanted| wanted != namespace)
        {
            return false;
        }
        if let Some(names) = self.dimension_names.as_ref() {
            if dimensions.len() != names.len()
                || !names.iter().all(|name| dimensions.contains_key(name))
            {
                return false;
            }
        }
        if self.groups.is_empty() {
            return true;
        }
        self.groups.iter().any(|group| {
            group
                .iter()
                .all(|term| term.holds(namespace, metric_name, dimensions))
        })
    }
}

/// One term of a search query, and whether it is negated.
#[derive(Debug, Clone)]
struct SearchTerm {
    /// Whether the query asked for the series the matcher does NOT match.
    negated: bool,
    /// What the term matches against.
    matcher: Matcher,
}

impl SearchTerm {
    /// Whether this term holds for the series it names.
    fn holds(
        &self,
        namespace: &str,
        metric_name: &str,
        dimensions: &BTreeMap<String, String>,
    ) -> bool {
        let matched = match &self.matcher {
            Matcher::MetricName(term) => term.matches(metric_name),
            Matcher::Namespace(term) => term.matches(namespace),
            Matcher::Dimension(name, term) => dimensions
                .get(name)
                .is_some_and(|value| term.matches(value)),
            Matcher::Anywhere(term) => {
                term.matches(metric_name)
                    || term.matches(namespace)
                    || dimensions
                        .iter()
                        .any(|(name, value)| term.matches(name) || term.matches(value))
            }
        };
        matched != self.negated
    }
}

/// What one search term matches against.
#[derive(Debug, Clone)]
enum Matcher {
    /// The series' metric name.
    MetricName(Term),
    /// The series' namespace.
    Namespace(Term),
    /// The value of one of the series' dimensions, by the dimension's name.
    Dimension(String, Term),
    /// Anywhere in the series' identity: its metric name, its namespace, or one of its
    /// dimension names or values.
    Anywhere(Term),
}

/// One term's text: a quoted value matches exactly, an unquoted one matches any value that
/// contains it, without regard to case.
#[derive(Debug, Clone)]
struct Term {
    /// The text the term carries.
    text: String,
    /// Whether the term was quoted, and so matches exactly.
    exact: bool,
}

impl Term {
    /// Whether `candidate` matches this term.
    fn matches(&self, candidate: &str) -> bool {
        if self.exact {
            candidate == self.text
        } else {
            candidate
                .to_ascii_lowercase()
                .contains(&self.text.to_ascii_lowercase())
        }
    }
}

/// Parse a `SEARCH` query, or say why it is not one.
pub fn parse_search_query(query: &str) -> Result<SearchQuery, MathError> {
    let mut parsed = SearchQuery::default();
    let rest = match query.trim().strip_prefix('{') {
        None => query.trim(),
        Some(schema) => {
            let Some((schema, rest)) = schema.split_once('}') else {
                return Err(MathError::new(
                    "the search expression's schema is not closed",
                ));
            };
            let mut parts = schema.split(',').map(str::trim).filter(|part| !part.is_empty());
            let Some(namespace) = parts.next() else {
                return Err(MathError::new(
                    "the search expression's schema names a namespace",
                ));
            };
            parsed.namespace = Some(unquote(namespace).text);
            parsed.dimension_names =
                Some(parts.map(|name| unquote(name).text).collect::<Vec<String>>());
            rest.trim()
        }
    };

    // The terms of one group are ANDed, `OR` opens the next group, and `NOT` negates the term
    // after it. A query that spells out no term at all matches every series of its schema.
    let mut groups: Vec<Vec<SearchTerm>> = Vec::new();
    let mut group: Vec<SearchTerm> = Vec::new();
    let mut negated = false;
    for term in search_terms(rest) {
        match term.to_ascii_uppercase().as_str() {
            "AND" => continue,
            "OR" => {
                groups.push(std::mem::take(&mut group));
                continue;
            }
            "NOT" => {
                negated = true;
                continue;
            }
            _ => {}
        }
        let matcher = match term.split_once('=') {
            Some((key, value)) => {
                let value = unquote(value.trim());
                match key.trim() {
                    "MetricName" => Matcher::MetricName(value),
                    "Namespace" => Matcher::Namespace(value),
                    "" => return Err(MathError::new("a search term names what it matches")),
                    dimension => Matcher::Dimension(dimension.to_string(), value),
                }
            }
            None => Matcher::Anywhere(unquote(term.trim())),
        };
        group.push(SearchTerm { negated, matcher });
        negated = false;
    }
    groups.push(group);
    parsed.groups = groups.into_iter().filter(|group| !group.is_empty()).collect();
    Ok(parsed)
}

/// The terms of a search query: whitespace-separated, except inside a quoted value.
///
/// One pass over the query's bytes, so reading a long query costs its length rather than its
/// square.
fn search_terms(query: &str) -> Vec<&str> {
    let mut terms: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    for (offset, current) in query.char_indices() {
        match quote {
            Some(open) if current == open => quote = None,
            Some(_) => {}
            None if current == '\'' || current == '"' => quote = Some(current),
            None if current.is_whitespace() => {
                if offset > start {
                    terms.push(query.get(start..offset).unwrap_or_default());
                }
                start = offset + current.len_utf8();
            }
            None => {}
        }
    }
    if start < query.len() {
        terms.push(query.get(start..).unwrap_or_default());
    }
    terms
        .into_iter()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .collect()
}

/// One term's text, and whether it was quoted.
fn unquote(text: &str) -> Term {
    for quote in ['\'', '"'] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return Term {
                text: inner.to_string(),
                exact: true,
            };
        }
    }
    Term {
        text: text.to_string(),
        exact: false,
    }
}

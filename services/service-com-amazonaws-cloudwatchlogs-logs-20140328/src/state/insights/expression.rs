// SPDX-License-Identifier: CC0-1.0

//! The EXPRESSIONS a pipelined query carries: what a `filter` (Logs Insights QL) or `where`
//! (PPL) command selects records by, what a `stats` command aggregates over, and what a
//! `sort` command orders by.
//!
//! An expression is TOKENIZED and PARSED into a tree once per stage and then evaluated once
//! per record, so the query string is read once however many events the query reads, and a
//! regular expression it carries is compiled once rather than per event.
//!
//! What the grammar covers is what the query languages' own expressions are written with:
//! the boolean operators (`and`, `or`, `not`, and their `&&`/`||`/`!` spellings), the six
//! comparisons, membership (`in [..]`), the text matches (`like`, `not like`, `=~`), the four
//! arithmetic operators and the remainder, bracketed grouping, and calls — the aggregate
//! functions a `stats` command names and the scalar functions an expression may use
//! ([`SCALAR_FUNCTIONS`]).
//!
//! WHAT BOUNDS IT: the query string is `@length(0..10000)` (`QueryString`), the parse is a
//! single pass over its tokens with no backtracking, the tree it builds is bounded in DEPTH
//! by [`super::MAX_EXPRESSION_DEPTH`] and in NODE COUNT by [`super::MAX_EXPRESSION_NODES`] —
//! both, because a flat chain of same-precedence operators grows a tree without nesting one,
//! and that tree is walked recursively per record and dropped recursively at the end of the
//! stage. Every evaluation spends a step of the run's [`super::Budget`], so the stages, the
//! records and the nodes cannot multiply out past it, and a regular expression inside the
//! query is compiled once under an explicit size limit ([`MAX_COMPILED_REGEX_BYTES`]).

use super::{Budget, Record};

/// One value an expression evaluates to.
///
/// A record's fields are text, so text is what most expressions produce; a comparison or an
/// arithmetic operator reads a NUMBER out of that text where one is written there
/// ([`number_of`]), which is how `filter duration > 500` compares a parsed field against a
/// number.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A field the record does not carry, or a function with nothing to answer.
    Null,
    /// Text.
    Text(String),
    /// A number.
    Number(f64),
    /// A truth value, which is what a comparison answers.
    Bool(bool),
}

impl Value {
    /// This value as the text a result field would carry.
    pub fn rendered(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Text(text) => text.clone(),
            Self::Number(number) => render_number(*number),
            Self::Bool(flag) => flag.to_string(),
        }
    }

    /// The number this value is, if it is one: a number, or text that is written as one.
    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            Self::Text(text) => number_of(text),
            Self::Bool(_) | Self::Null => None,
        }
    }

    /// Whether this value selects a record: a truth value is itself, a number is true when it
    /// is not zero, text is true when it is not empty, and a missing field is false.
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Text(text) => !text.is_empty(),
            Self::Number(number) => *number != 0.0,
            Self::Bool(flag) => *flag,
        }
    }
}

/// How a number is rendered into a result field: as an integer when it is one, so a count or
/// a sum of whole numbers reads as a whole number rather than as a float.
pub fn render_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{:.0}", number)
    } else {
        format!("{number}")
    }
}

/// The number `text` is written as, if it is written as one.
pub fn number_of(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f64>().ok().filter(|number: &f64| number.is_finite())
}

/// How large a compiled expression a query string's regular expression may build. THE
/// EMULATOR'S OWN BOUND, the same one the query compiler holds a `parse` pattern to.
const MAX_COMPILED_REGEX_BYTES: usize = 1 << 20;

/// A parsed expression.
#[derive(Debug, Clone)]
pub enum Expr {
    /// The value of one field of the record.
    Field(String),
    /// A literal written in the query string.
    Literal(Value),
    /// A regular-expression literal, compiled once when the stage was parsed.
    Pattern(::regex::Regex),
    /// One operand under a prefix operator.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// What it applies to.
        expr: Box<Expr>,
    },
    /// Two operands under an infix operator.
    Binary {
        /// The operator.
        op: BinaryOp,
        /// Its left operand.
        left: Box<Expr>,
        /// Its right operand.
        right: Box<Expr>,
    },
    /// Membership in a list written in the query string.
    In {
        /// What is looked for.
        expr: Box<Expr>,
        /// Where it is looked for.
        list: Vec<Expr>,
        /// Whether the test is `not in`.
        negated: bool,
    },
    /// A call: an aggregate function of a `stats` command, or a scalar function.
    Call {
        /// The function's name, lowercased.
        name: String,
        /// Its arguments, in the order written.
        args: Vec<Expr>,
        /// Whether it was written `f(*)`, which is how `count` counts records.
        wildcard: bool,
    },
}

/// A prefix operator of the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `not`, `!`.
    Not,
    /// Arithmetic negation.
    Negate,
}

/// An infix operator of the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `and`, `&&`.
    And,
    /// `or`, `||`.
    Or,
    /// `=`, `==`.
    Equal,
    /// `!=`, `<>`.
    NotEqual,
    /// `<`.
    Less,
    /// `<=`.
    LessOrEqual,
    /// `>`.
    Greater,
    /// `>=`.
    GreaterOrEqual,
    /// `like`.
    Like,
    /// `not like`.
    NotLike,
    /// `=~`, which matches a regular expression.
    Matches,
    /// `+`.
    Add,
    /// `-`.
    Subtract,
    /// `*`.
    Multiply,
    /// `/`.
    Divide,
    /// `%`.
    Modulo,
}

/// The scalar functions an expression may call, each over the record it is evaluated
/// against: the string and arithmetic functions the query languages define for an expression
/// that is not an aggregate.
pub const SCALAR_FUNCTIONS: [&str; 9] = [
    "ispresent",
    "isempty",
    "isblank",
    "strlen",
    "strcontains",
    "lower",
    "upper",
    "abs",
    "coalesce",
];

/// The functions that group records into spans of time rather than computing a value, which
/// a `stats` command's `by` clause names and the caller of this module reads
/// ([`super::pipeline`]).
pub const GROUPING_FUNCTIONS: [&str; 2] = ["bin", "span"];

/// Whether `name` is a function this service can run: a scalar function, an aggregate or a
/// grouping function.
///
/// A call to anything else is refused where it is PARSED, so a query naming a function of the
/// language this service has not implemented is reported as the `Failed` status of an
/// execution that produced nothing — never as a result computed without it.
fn is_known_function(name: &str) -> bool {
    let lowercased = name.to_lowercase();
    SCALAR_FUNCTIONS.contains(&lowercased.as_str())
        || GROUPING_FUNCTIONS.contains(&lowercased.as_str())
        || AggregateFunction::parse(&lowercased).is_some()
}

/// Parse `text` as one expression, or say why it is not one.
pub fn parse(text: &str) -> Result<Expr, String> {
    let tokens = tokenize(text)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        nodes: 0,
    };
    let expression = parser.expression(0)?;
    if parser.at < parser.tokens.len() {
        return Err(format!(
            "the expression carries {} after its end",
            parser.tokens[parser.at].text()
        ));
    }
    Ok(expression)
}

/// Evaluate `expression` against `record`, spending `budget` as it goes.
///
/// A run that is over budget evaluates to nothing: every expression of every later record
/// answers [`Value::Null`], and the caller reports the run as the failure the budget records
/// ([`Budget::reason`]) rather than reporting rows it stopped computing half way through.
pub fn evaluate(expression: &Expr, record: &Record, budget: &Budget) -> Value {
    if !budget.step() {
        return Value::Null;
    }
    match expression {
        Expr::Field(name) => record
            .get(name)
            .map_or(Value::Null, |value| Value::Text(value.to_string())),
        Expr::Literal(value) => value.clone(),
        // A regular expression standing on its own is text; it is a MATCH only as the right
        // operand of one of the match operators, which read it as the pattern it is.
        Expr::Pattern(pattern) => Value::Text(pattern.as_str().to_string()),
        Expr::Unary { op, expr } => {
            let value = evaluate(expr, record, budget);
            match op {
                UnaryOp::Not => Value::Bool(!value.truthy()),
                UnaryOp::Negate => value.number().map_or(Value::Null, |number| {
                    Value::Number(-number)
                }),
            }
        }
        Expr::Binary { op, left, right } => binary(*op, left, right, record, budget),
        Expr::In {
            expr,
            list,
            negated,
        } => {
            let value = evaluate(expr, record, budget);
            let found = list.iter().any(|item| {
                compare(&value, &evaluate(item, record, budget)) == Some(std::cmp::Ordering::Equal)
            });
            Value::Bool(found != *negated)
        }
        Expr::Call {
            name,
            args,
            wildcard,
        } => call(name, args, *wildcard, record, budget),
    }
}

/// Evaluate one infix operator. `and` and `or` are evaluated lazily, so the right operand of
/// a decided boolean is never evaluated.
fn binary(op: BinaryOp, left: &Expr, right: &Expr, record: &Record, budget: &Budget) -> Value {
    match op {
        BinaryOp::And => {
            if !evaluate(left, record, budget).truthy() {
                return Value::Bool(false);
            }
            Value::Bool(evaluate(right, record, budget).truthy())
        }
        BinaryOp::Or => {
            if evaluate(left, record, budget).truthy() {
                return Value::Bool(true);
            }
            Value::Bool(evaluate(right, record, budget).truthy())
        }
        BinaryOp::Like | BinaryOp::NotLike | BinaryOp::Matches => {
            let subject = evaluate(left, record, budget).rendered();
            let matched = match right {
                // `like /re/` and `=~ /re/` match the expression; `like "text"` is the
                // substring test that command documents.
                Expr::Pattern(pattern) => pattern.is_match(&subject),
                other => subject.contains(evaluate(other, record, budget).rendered().as_str()),
            };
            Value::Bool(matched != matches!(op, BinaryOp::NotLike))
        }
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessOrEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterOrEqual => {
            let ordering = compare(
                &evaluate(left, record, budget),
                &evaluate(right, record, budget),
            );
            let Some(ordering) = ordering else {
                // Nothing to compare — a field the record does not carry — so the comparison
                // selects nothing, except as an inequality, which two incomparable values
                // satisfy.
                return Value::Bool(matches!(op, BinaryOp::NotEqual));
            };
            use std::cmp::Ordering;
            Value::Bool(match op {
                BinaryOp::Equal => ordering == Ordering::Equal,
                BinaryOp::NotEqual => ordering != Ordering::Equal,
                BinaryOp::Less => ordering == Ordering::Less,
                BinaryOp::LessOrEqual => ordering != Ordering::Greater,
                BinaryOp::Greater => ordering == Ordering::Greater,
                _ => ordering != Ordering::Less,
            })
        }
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo => {
            let (Some(left), Some(right)) = (
                evaluate(left, record, budget).number(),
                evaluate(right, record, budget).number(),
            ) else {
                return Value::Null;
            };
            let result = match op {
                BinaryOp::Add => left + right,
                BinaryOp::Subtract => left - right,
                BinaryOp::Multiply => left * right,
                BinaryOp::Divide => {
                    if right == 0.0 {
                        return Value::Null;
                    }
                    left / right
                }
                _ => {
                    if right == 0.0 {
                        return Value::Null;
                    }
                    left % right
                }
            };
            if result.is_finite() {
                Value::Number(result)
            } else {
                Value::Null
            }
        }
    }
}

/// Order two values, as a TOTAL order over everything a record can carry.
///
/// Two values written as numbers are ordered as numbers; everything else is ordered as text.
/// A value written as a number and one that is not are ordered by that distinction FIRST —
/// every number before every piece of text — because ordering them by their text instead
/// would make the order depend on the pair rather than on the values: `9` before `10` as
/// numbers, `"10"` before `"1a"` as text and `"1a"` before `"9"` as text is a cycle, and a
/// comparison that cycles is not an order a sort may be driven by.
///
/// A missing value compares with nothing, which is what makes a comparison against an absent
/// field select no record; a SORT orders those last ([`total_ordering`]).
pub fn compare(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    if matches!(left, Value::Null) || matches!(right, Value::Null) {
        return None;
    }
    match (left.number(), right.number()) {
        (Some(left), Some(right)) => Some(left.total_cmp(&right)),
        // The number-before-text tag, then the text itself.
        (left_number, right_number) => Some(
            left_number
                .is_none()
                .cmp(&right_number.is_none())
                .then_with(|| left.rendered().cmp(&right.rendered())),
        ),
    }
}

/// The order a SORT drives: [`compare`]'s total order over the values a record carries, with
/// a value the record does NOT carry ordered after every value it does.
///
/// Both query surfaces order by this one rule, so a sort of a pipeline and a SQL `ORDER BY`
/// place the same rows in the same order, and the comparison a sort is driven by is total —
/// reflexive, antisymmetric and transitive — over everything a caller's data can hold.
pub fn total_ordering(left: &Value, right: &Value) -> std::cmp::Ordering {
    match (matches!(left, Value::Null), matches!(right, Value::Null)) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => compare(left, right).unwrap_or(std::cmp::Ordering::Equal),
    }
}

/// Evaluate one scalar function call. An aggregate function (`count`, `sum`, …) is NOT
/// evaluated here: it is computed by the `stats` stage over a whole group of records, so a
/// call to one outside that stage has nothing to aggregate and answers nothing.
fn call(name: &str, args: &[Expr], wildcard: bool, record: &Record, budget: &Budget) -> Value {
    let argument = |index: usize| {
        args.get(index)
            .map_or(Value::Null, |expr| evaluate(expr, record, budget))
    };
    match name {
        "ispresent" => Value::Bool(match args.first() {
            Some(Expr::Field(field)) => record.get(field).is_some(),
            Some(other) => !matches!(evaluate(other, record, budget), Value::Null),
            None => false,
        }),
        "isempty" | "isblank" => Value::Bool(argument(0).rendered().trim().is_empty()),
        "strlen" => Value::Number(argument(0).rendered().chars().count() as f64),
        "strcontains" => Value::Bool(
            argument(0)
                .rendered()
                .contains(argument(1).rendered().as_str()),
        ),
        "lower" => Value::Text(argument(0).rendered().to_lowercase()),
        "upper" => Value::Text(argument(0).rendered().to_uppercase()),
        "abs" => argument(0)
            .number()
            .map_or(Value::Null, |number| Value::Number(number.abs())),
        "coalesce" => args
            .iter()
            .map(|expr| evaluate(expr, record, budget))
            .find(|value| !matches!(value, Value::Null))
            .unwrap_or(Value::Null),
        // A call this grammar does not define never reaches here: the parse refuses one
        // ([`is_known_function`]). `count(*)` reaches here only outside a `stats` stage,
        // where there is no group to count.
        _ => {
            let _ = wildcard;
            Value::Null
        }
    }
}

/// What one aggregate function computes over the values of a group.
///
/// The set is the one both query-language surfaces aggregate with: a `stats` command's
/// functions and a SQL `SELECT`'s, which name the same computations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFunction {
    /// How many values the group holds — which, over a record's own presence, is how many
    /// records it holds.
    Count,
    /// How many DISTINCT values the group holds.
    CountDistinct,
    /// Their total.
    Sum,
    /// Their mean.
    Average,
    /// The least of them.
    Minimum,
    /// The greatest.
    Maximum,
}

impl AggregateFunction {
    /// The aggregate `name` names, lowercased, or `None` when it names none.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "count" => Some(Self::Count),
            "count_distinct" | "distinct_count" | "dc" => Some(Self::CountDistinct),
            "sum" => Some(Self::Sum),
            "avg" | "average" | "mean" => Some(Self::Average),
            "min" => Some(Self::Minimum),
            "max" => Some(Self::Maximum),
            _ => None,
        }
    }
}

/// What one aggregate has taken in so far, folded record by record.
///
/// An aggregate is accumulated INCREMENTALLY — count, sum, mean, least and greatest each need
/// a fixed amount of state per group — so a group costs the same whether it holds one record
/// or every record of the scan. Only `count_distinct` has to remember values, which is
/// bounded by [`MAX_DISTINCT_VALUES`] and charged to the run's budget.
///
/// A value a record does not carry contributes to nothing — counting a field counts the
/// records that HAVE it — so a missing value is left out of every function, and a function
/// with nothing to compute over answers nothing.
#[derive(Debug, Default, Clone)]
pub struct Accumulator {
    /// How many values were taken in.
    count: u64,
    /// Their total, over those that are numbers.
    sum: f64,
    /// How many of them were numbers, so a mean over none answers nothing.
    numbers: u64,
    /// The least value seen, as a value rather than a number, so a minimum over text is the
    /// text that sorts first.
    least: Option<Value>,
    /// The greatest, on the same terms.
    greatest: Option<Value>,
    /// The distinct values seen, for a `count_distinct`.
    distinct: std::collections::BTreeSet<String>,
}

/// THE EMULATOR'S OWN CEILING on how many DISTINCT values one `count_distinct` remembers:
/// the only aggregate whose state grows with the values it sees, so it is the only one that
/// needs a ceiling of its own.
pub const MAX_DISTINCT_VALUES: usize = 100_000;

impl Accumulator {
    /// Take one value into this aggregate, spending the run's budget for anything it has to
    /// remember. `false` once the run is over budget.
    pub fn add(&mut self, function: AggregateFunction, value: &Value, budget: &Budget) -> bool {
        if matches!(value, Value::Null) {
            return !budget.exhausted();
        }
        self.count = self.count.saturating_add(1);
        if let Some(number) = value.number() {
            self.sum += number;
            self.numbers = self.numbers.saturating_add(1);
        }
        if self
            .least
            .as_ref()
            .is_none_or(|least| compare(value, least) == Some(std::cmp::Ordering::Less))
        {
            self.least = Some(value.clone());
        }
        if self
            .greatest
            .as_ref()
            .is_none_or(|greatest| compare(value, greatest) == Some(std::cmp::Ordering::Greater))
        {
            self.greatest = Some(value.clone());
        }
        if matches!(function, AggregateFunction::CountDistinct)
            && self.distinct.len() < MAX_DISTINCT_VALUES
        {
            let rendered = value.rendered();
            if !budget.store(rendered.len()) {
                return false;
            }
            self.distinct.insert(rendered);
        }
        !budget.exhausted()
    }

    /// What this aggregate answers for its group.
    pub fn answer(&self, function: AggregateFunction) -> Value {
        match function {
            AggregateFunction::Count => Value::Number(self.count as f64),
            AggregateFunction::CountDistinct => Value::Number(self.distinct.len() as f64),
            AggregateFunction::Sum => Value::Number(self.sum),
            AggregateFunction::Average => {
                if self.numbers == 0 {
                    return Value::Null;
                }
                Value::Number(self.sum / self.numbers as f64)
            }
            AggregateFunction::Minimum => self.least.clone().unwrap_or(Value::Null),
            AggregateFunction::Maximum => self.greatest.clone().unwrap_or(Value::Null),
        }
    }
}

/// One token of an expression.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A name: a field, a function or a keyword.
    Word(String),
    /// A number written in the query string.
    Number(f64),
    /// A quoted string.
    Text(String),
    /// A regular-expression literal, without its slashes.
    Pattern(String),
    /// An operator or a piece of punctuation.
    Symbol(String),
}

impl Token {
    /// This token as the query string wrote it, for a message that names where a parse
    /// stopped.
    fn text(&self) -> String {
        match self {
            Self::Word(word) => word.clone(),
            Self::Number(number) => render_number(*number),
            Self::Text(text) => format!("\"{text}\""),
            Self::Pattern(pattern) => format!("/{pattern}/"),
            Self::Symbol(symbol) => symbol.clone(),
        }
    }

    /// The symbol this token is, if it is one.
    fn symbol(&self) -> Option<&str> {
        match self {
            Self::Symbol(symbol) => Some(symbol.as_str()),
            _ => None,
        }
    }

    /// The keyword this token is, lowercased, if it is a word.
    fn keyword(&self) -> Option<String> {
        match self {
            Self::Word(word) => Some(word.to_lowercase()),
            _ => None,
        }
    }
}

/// The operators of the grammar that are written as symbols, longest first so a two-character
/// operator is never read as two one-character ones.
const SYMBOLS: [&str; 17] = [
    "<=", ">=", "!=", "<>", "==", "=~", "&&", "||", "=", "<", ">", "+", "-", "*", "/", "%", "!",
];

/// Split `text` into the tokens of one expression.
fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let characters: Vec<char> = text.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut index = 0usize;
    while index < characters.len() {
        let character = characters[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        // A `/` opens a regular-expression literal where a VALUE is expected, and is division
        // where one has just been read: the same rule a query compiler reads `filter @message
        // like /a|b/` by.
        if character == '/' && expects_value(&tokens) {
            let (pattern, next) = scan_until(&characters, index + 1, '/');
            tokens.push(Token::Pattern(pattern));
            index = next;
            continue;
        }
        if character == '"' || character == '\'' {
            let (literal, next) = scan_until(&characters, index + 1, character);
            tokens.push(Token::Text(literal));
            index = next;
            continue;
        }
        // A back-quoted identifier is a FIELD whose name the quoting lets carry anything,
        // which is how PPL writes a field name with a dot or a dash in it.
        if character == '`' {
            let (name, next) = scan_until(&characters, index + 1, '`');
            tokens.push(Token::Word(name));
            index = next;
            continue;
        }
        if character.is_ascii_digit() {
            let (token, next) = scan_number(&characters, index);
            tokens.push(token);
            index = next;
            continue;
        }
        if is_word_character(character) {
            let name: String = characters[index..]
                .iter()
                .take_while(|character| is_word_character(**character))
                .collect();
            index += name.chars().count();
            tokens.push(Token::Word(name));
            continue;
        }
        if let Some(symbol) = SYMBOLS.iter().find(|symbol| {
            characters[index..]
                .iter()
                .take(symbol.chars().count())
                .copied()
                .eq(symbol.chars())
        }) {
            tokens.push(Token::Symbol((*symbol).to_string()));
            index += symbol.chars().count();
            continue;
        }
        if matches!(character, '(' | ')' | '[' | ']' | ',') {
            tokens.push(Token::Symbol(character.to_string()));
            index += 1;
            continue;
        }
        return Err(format!("the expression carries {character}, which is not part of one"));
    }
    Ok(tokens)
}

/// Whether a VALUE is what comes next, which is how a `/` is told apart from division: at the
/// start of an expression, and after an operator, a keyword, an opening bracket or a comma.
fn expects_value(tokens: &[Token]) -> bool {
    match tokens.last() {
        None => true,
        Some(Token::Symbol(symbol)) => !matches!(symbol.as_str(), ")" | "]"),
        Some(Token::Word(word)) => KEYWORDS.contains(&word.to_lowercase().as_str()),
        _ => false,
    }
}

/// The words of the grammar that are OPERATORS rather than field names, so what follows one
/// is a value.
const KEYWORDS: [&str; 6] = ["and", "or", "not", "like", "in", "by"];

/// Whether `character` can be part of a field name or a keyword: a field may be a
/// `@`-prefixed generated field and may carry the dots of a flattened JSON path.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '@' | '_' | '.' | '$' | '#' | ':')
}

/// Scan the text that runs from `from` to the next `delimiter`, answering it and the index
/// just past the delimiter. An escaped delimiter is part of the text. Text that nothing
/// closes runs to the end, which is where the stage carrying it is judged.
fn scan_until(characters: &[char], from: usize, delimiter: char) -> (String, usize) {
    let mut text = String::new();
    let mut index = from;
    while index < characters.len() {
        let character = characters[index];
        if character == '\\' {
            if let Some(escaped) = characters.get(index + 1) {
                // The escape is kept for a regular expression, whose own syntax uses it, and
                // dropped for a quoted string, where it only escapes the delimiter.
                if delimiter == '/' {
                    text.push(character);
                }
                text.push(*escaped);
                index += 2;
                continue;
            }
        }
        if character == delimiter {
            return (text, index + 1);
        }
        text.push(character);
        index += 1;
    }
    (text, characters.len())
}

/// Scan the number that opens at `index`, answering it and the index just past it.
///
/// A number followed immediately by letters is a DURATION — `5m`, `1h`, the form a `bin`
/// command's width is written in — so it is read as the text it is written as rather than as
/// a number and a field name.
fn scan_number(characters: &[char], index: usize) -> (Token, usize) {
    let digits: String = characters[index..]
        .iter()
        .take_while(|character| character.is_ascii_digit() || **character == '.')
        .collect();
    let mut next = index + digits.chars().count();
    let unit: String = characters[next.min(characters.len())..]
        .iter()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect();
    if !unit.is_empty() {
        next += unit.chars().count();
        return (Token::Text(format!("{digits}{unit}")), next);
    }
    (
        digits
            .parse::<f64>()
            .map_or_else(|_| Token::Text(digits.clone()), Token::Number),
        next,
    )
}

/// A recursive-descent parser over one expression's tokens.
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    /// How many nodes the tree has been given so far, counted against
    /// [`super::MAX_EXPRESSION_NODES`]: a flat chain of operators adds a node per operator
    /// without nesting one, so the COUNT is bounded as well as the depth.
    nodes: usize,
}

impl Parser {
    /// Take one node of the expression's budget, or say the expression is larger than this
    /// service evaluates.
    fn node(&mut self) -> Result<(), String> {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > super::MAX_EXPRESSION_NODES {
            return Err(format!(
                "the expression holds more than {} parts",
                super::MAX_EXPRESSION_NODES
            ));
        }
        Ok(())
    }
    /// Parse one expression, operators binding as [`precedence`] says.
    fn expression(&mut self, depth: usize) -> Result<Expr, String> {
        if depth > super::MAX_EXPRESSION_DEPTH {
            return Err("the expression nests deeper than this service evaluates".to_string());
        }
        self.binary_expression(0, depth)
    }

    /// Parse an expression whose operators bind at least as tightly as `least`.
    fn binary_expression(&mut self, least: u8, depth: usize) -> Result<Expr, String> {
        let mut left = self.unary(depth + 1)?;
        while let Some((operator, binding, consumed)) = self.peek_operator() {
            if binding < least {
                break;
            }
            self.at += consumed;
            self.node()?;
            left = match operator {
                // `in` carries a LIST rather than one operand, so its right-hand side is
                // parsed on its own.
                Operator::Membership { negated } => self.membership(left, negated, depth + 1)?,
                Operator::Infix(op) => Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(self.binary_expression(binding + 1, depth + 1)?),
                },
            };
        }
        Ok(left)
    }

    /// The operator at the cursor: which one it is, how tightly it binds, and how many
    /// tokens it spans.
    fn peek_operator(&self) -> Option<(Operator, u8, usize)> {
        let token = self.tokens.get(self.at)?;
        if let Some(symbol) = token.symbol() {
            let op = match symbol {
                "&&" => BinaryOp::And,
                "||" => BinaryOp::Or,
                "=" | "==" => BinaryOp::Equal,
                "!=" | "<>" => BinaryOp::NotEqual,
                "<" => BinaryOp::Less,
                "<=" => BinaryOp::LessOrEqual,
                ">" => BinaryOp::Greater,
                ">=" => BinaryOp::GreaterOrEqual,
                "=~" => BinaryOp::Matches,
                "+" => BinaryOp::Add,
                "-" => BinaryOp::Subtract,
                "*" => BinaryOp::Multiply,
                "/" => BinaryOp::Divide,
                "%" => BinaryOp::Modulo,
                _ => return None,
            };
            return Some((Operator::Infix(op), precedence(op), 1));
        }
        match token.keyword()?.as_str() {
            "and" => Some((
                Operator::Infix(BinaryOp::And),
                precedence(BinaryOp::And),
                1,
            )),
            "or" => Some((Operator::Infix(BinaryOp::Or), precedence(BinaryOp::Or), 1)),
            "like" => Some((
                Operator::Infix(BinaryOp::Like),
                precedence(BinaryOp::Like),
                1,
            )),
            "in" => Some((
                Operator::Membership { negated: false },
                precedence(BinaryOp::Equal),
                1,
            )),
            "not" => {
                // `not like` and `not in` are the negated forms of the two operators; a bare
                // `not` is the prefix operator and is no infix operator at all.
                match self.tokens.get(self.at + 1).and_then(Token::keyword)?.as_str() {
                    "like" => Some((
                        Operator::Infix(BinaryOp::NotLike),
                        precedence(BinaryOp::Like),
                        2,
                    )),
                    "in" => Some((
                        Operator::Membership { negated: true },
                        precedence(BinaryOp::Equal),
                        2,
                    )),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Parse the list of an `in` / `not in` test, the operator having been consumed.
    fn membership(&mut self, left: Expr, negated: bool, depth: usize) -> Result<Expr, String> {
        let bracketed = self.tokens.get(self.at).and_then(Token::symbol) == Some("[");
        if bracketed {
            self.at += 1;
        }
        let mut list: Vec<Expr> = Vec::new();
        loop {
            if self.tokens.get(self.at).and_then(Token::symbol) == Some("]") {
                self.at += 1;
                break;
            }
            if self.at >= self.tokens.len() {
                if bracketed {
                    return Err("the in list is not closed".to_string());
                }
                break;
            }
            list.push(self.binary_expression(precedence(BinaryOp::Equal) + 1, depth + 1)?);
            if self.tokens.get(self.at).and_then(Token::symbol) == Some(",") {
                self.at += 1;
                continue;
            }
            if !bracketed {
                break;
            }
        }
        Ok(Expr::In {
            expr: Box::new(left),
            list,
            negated,
        })
    }

    /// Parse one operand, with any prefix operators it carries.
    fn unary(&mut self, depth: usize) -> Result<Expr, String> {
        if depth > super::MAX_EXPRESSION_DEPTH {
            return Err("the expression nests deeper than this service evaluates".to_string());
        }
        let token = self
            .tokens
            .get(self.at)
            .ok_or_else(|| "the expression ends where a value was required".to_string())?
            .clone();
        if token.symbol() == Some("!") || token.keyword().as_deref() == Some("not") {
            self.at += 1;
            self.node()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.unary(depth + 1)?),
            });
        }
        if token.symbol() == Some("-") {
            self.at += 1;
            self.node()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Negate,
                expr: Box::new(self.unary(depth + 1)?),
            });
        }
        self.primary(depth + 1)
    }

    /// Parse one primary: a bracketed expression, a literal, a call or a field.
    fn primary(&mut self, depth: usize) -> Result<Expr, String> {
        let token = self
            .tokens
            .get(self.at)
            .ok_or_else(|| "the expression ends where a value was required".to_string())?
            .clone();
        self.at += 1;
        self.node()?;
        match token {
            Token::Symbol(symbol) if symbol == "(" => {
                let inner = self.expression(depth + 1)?;
                if self.tokens.get(self.at).and_then(Token::symbol) != Some(")") {
                    return Err("a bracketed expression is not closed".to_string());
                }
                self.at += 1;
                Ok(inner)
            }
            Token::Number(number) => Ok(Expr::Literal(Value::Number(number))),
            Token::Text(text) => Ok(Expr::Literal(Value::Text(text))),
            Token::Pattern(pattern) => Ok(Expr::Pattern(compile_pattern(&pattern)?)),
            Token::Word(word) => {
                if self.tokens.get(self.at).and_then(Token::symbol) == Some("(") {
                    self.at += 1;
                    return self.call(&word, depth + 1);
                }
                match word.to_lowercase().as_str() {
                    "true" => Ok(Expr::Literal(Value::Bool(true))),
                    "false" => Ok(Expr::Literal(Value::Bool(false))),
                    "null" => Ok(Expr::Literal(Value::Null)),
                    _ => Ok(Expr::Field(word)),
                }
            }
            Token::Symbol(symbol) => Err(format!(
                "the expression carries {symbol} where a value was required"
            )),
        }
    }

    /// Parse a call's arguments, the opening bracket having been consumed.
    fn call(&mut self, name: &str, depth: usize) -> Result<Expr, String> {
        if !is_known_function(name) {
            return Err(format!(
                "the function {name} has no execution in this service"
            ));
        }
        let mut args: Vec<Expr> = Vec::new();
        let mut wildcard = false;
        loop {
            match self.tokens.get(self.at).and_then(Token::symbol) {
                Some(")") => {
                    self.at += 1;
                    break;
                }
                Some(",") => {
                    self.at += 1;
                    continue;
                }
                // `count(*)` counts the records of the group rather than the values of a
                // field, which is the one place a `*` stands for an argument.
                Some("*") if args.is_empty() => {
                    self.at += 1;
                    wildcard = true;
                    continue;
                }
                _ => {}
            }
            if self.at >= self.tokens.len() {
                return Err(format!("the call to {name} is not closed"));
            }
            args.push(self.expression(depth + 1)?);
        }
        Ok(Expr::Call {
            name: name.to_lowercase(),
            args,
            wildcard,
        })
    }
}

/// What a parser read at an operator position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    /// One of the infix operators, taking one operand on each side.
    Infix(BinaryOp),
    /// `in` or `not in`, whose right-hand side is a list.
    Membership {
        /// Whether the test is `not in`.
        negated: bool,
    },
}

/// How tightly one operator binds: `or` loosest, then `and`, then the comparisons and
/// matches, then addition, then multiplication — the precedence an expression language gives
/// them.
fn precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessOrEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterOrEqual
        | BinaryOp::Like
        | BinaryOp::NotLike
        | BinaryOp::Matches => 3,
        BinaryOp::Add | BinaryOp::Subtract => 4,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 5,
    }
}

/// Compile one regular-expression literal of a query string, under the size bound this
/// service holds a caller's expression to.
pub fn compile_pattern(pattern: &str) -> Result<::regex::Regex, String> {
    ::regex::RegexBuilder::new(pattern)
        .size_limit(MAX_COMPILED_REGEX_BYTES)
        .build()
        .map_err(|error| format!("the regular expression /{pattern}/ is not valid: {error}"))
}

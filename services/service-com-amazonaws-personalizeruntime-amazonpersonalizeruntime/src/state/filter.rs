// SPDX-License-Identifier: CC0-1.0

//! Amazon Personalize **filter expressions**: the parser and the evaluator the three
//! runtime operations share.
//!
//! A filter is a control-plane resource (`CreateFilter`) whose `filterExpression` the
//! runtime applies to a recommendation request. The expression language is
//! Personalize's own — no crates.io crate parses it — so it is parsed here, as a
//! small recursive-descent pass over the grammar AWS documents for
//! "Filtering recommendations and user segments":
//!
//! ```text
//! expression := ("INCLUDE" | "EXCLUDE") subject "WHERE" clause { ("AND" | "OR") clause }
//! subject    := identifier                       -- "ItemID" or "ActionID"
//! clause     := dataset "." column predicate
//! predicate  := ["NOT"] "IN" "(" value { "," value } ")"
//!             | ("=" | "<>" | "<" | "<=" | ">" | ">=") value
//! value      := "$" parameter | '"' literal '"' | bare-literal
//! ```
//!
//! `AND` and `OR` are evaluated left to right with equal precedence: the documented
//! examples never mix them, and the expression language has no parentheses around
//! clauses, so there is no precedence to honour.
//!
//! # What the expression is evaluated against
//!
//! A clause names a dataset (`Items.GENRE`) and this emulator stores exactly the
//! Items and Actions attributes a registered catalog carries, so
//! [`parse`] accepts only those two datasets: an expression over `Interactions` would
//! have to be evaluated against interaction history this emulator does not store, and
//! silently ignoring it would make the filter claim to filter when it does not.
//!
//! A categorical attribute may hold several values separated by `|`, the separator
//! Personalize's own Items CSV uses, so a predicate matches when ANY of the attribute's
//! values matches, and a negated one when none does.
//!
//! # What a comparison compares
//!
//! `IN`, `NOT IN`, `=` and `<>` all test EQUALITY of the value(s) an entry stores,
//! whether that metadata is categorical or numerical: two values that both name a
//! finite number are compared as numbers, and anything else is compared as text, so a
//! clause over a categorical column is applied rather than silently satisfied by
//! nothing, and set membership agrees with `=` on when two spellings name one quantity.
//! The ORDERING operators (`<`, `<=`, `>`, `>=`) are defined over numerical metadata
//! only, so an operand that names no number leaves the clause unapplicable and is
//! refused where it becomes known — at [`parse`] for a literal written into the
//! expression, and at [`bind`] for a `$placeholder` the request binds — never by
//! applying a clause nothing can satisfy.
//!
//! A clause is evaluated against the value the entry STORES for its column, so an entry
//! that stores none satisfies no clause of any predicate — `NOT IN` included — and such
//! an entry is therefore kept by an `EXCLUDE` expression and dropped by an `INCLUDE`
//! one, whatever shape the predicates take. The clause evaluator is where that is
//! decided.
//!
//! # What a request binds a `$placeholder` to
//!
//! A request's `filterValues` entry holds the value(s) of ONE placeholder, separated by
//! commas, with a string value enclosed in double quotes and a numeric or timestamp
//! value written bare: the quotes delimit the value, so a clause compares the stored
//! attribute against the text between them. [`split_values`] is where that is read.

use std::collections::{BTreeMap, HashMap};

/// Whether a filter keeps the entries its expression matches, or removes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    /// `INCLUDE`: only matching entries survive.
    Include,
    /// `EXCLUDE`: matching entries are removed.
    Exclude,
}

/// The kind of catalog entry a filter is applied to, and therefore the only dataset
/// name whose clauses can be evaluated for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    /// An Items-dataset entry, filtered by `Items.<COLUMN>` clauses.
    Items,
    /// An Actions-dataset entry, filtered by `Actions.<COLUMN>` clauses.
    Actions,
}

impl Entity {
    /// The dataset name whose clauses apply to this entity.
    pub fn dataset(self) -> &'static str {
        match self {
            Self::Items => DATASET_ITEMS,
            Self::Actions => DATASET_ACTIONS,
        }
    }
}

/// The Items dataset name, as a filter expression spells it.
pub const DATASET_ITEMS: &str = "Items";
/// The Actions dataset name, as a filter expression spells it.
pub const DATASET_ACTIONS: &str = "Actions";

/// How two clauses combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// `AND`
    And,
    /// `OR`
    Or,
}

/// A comparison a clause can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// `=`
    Equal,
    /// `<>`
    NotEqual,
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterOrEqual,
}

impl Comparison {
    /// Whether this operator ORDERS its operands, and is therefore defined over
    /// numerical metadata only — as opposed to `=` and `<>`, which test equality of
    /// categorical or numerical values alike.
    fn orders(self) -> bool {
        matches!(
            self,
            Self::Less | Self::LessOrEqual | Self::Greater | Self::GreaterOrEqual
        )
    }

    /// How the operator is spelled in an expression, for a message that names it.
    fn spelling(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "<>",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
        }
    }

    /// Whether `left <op> right` holds.
    fn holds(self, left: f64, right: f64) -> bool {
        match self {
            Self::Equal => left == right,
            Self::NotEqual => left != right,
            Self::Less => left < right,
            Self::LessOrEqual => left <= right,
            Self::Greater => left > right,
            Self::GreaterOrEqual => left >= right,
        }
    }
}

/// One operand of a clause: either a `$placeholder` the request binds through
/// `filterValues`, or a literal written into the expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    /// `$PARAM` — bound from the request's `filterValues`.
    Parameter(String),
    /// A literal value.
    Literal(String),
}

/// What a clause asserts about one attribute.
#[derive(Debug, Clone)]
pub enum Predicate {
    /// `IN (...)` / `NOT IN (...)`.
    In {
        /// The operands of the set.
        values: Vec<Operand>,
        /// True for `NOT IN`.
        negated: bool,
    },
    /// A comparison against a single operand.
    Compare {
        /// The comparison operator.
        op: Comparison,
        /// The right-hand operand.
        operand: Operand,
    },
}

/// One clause of a filter expression.
#[derive(Debug, Clone)]
pub struct Clause {
    /// The dataset the clause reads — `Items` or `Actions`.
    pub dataset: String,
    /// The column within that dataset.
    pub column: String,
    /// What the clause asserts.
    pub predicate: Predicate,
}

/// A parsed filter expression.
#[derive(Debug, Clone)]
pub struct FilterExpression {
    /// `INCLUDE` or `EXCLUDE`.
    pub kind: FilterKind,
    /// The subject the expression selects — `ItemID` or `ActionID`.
    pub subject: String,
    /// The clauses, each with the combinator that joins it to the one before. The
    /// first entry's combinator is [`Combinator::And`] and is never read.
    pub clauses: Vec<(Combinator, Clause)>,
}

impl FilterExpression {
    /// Every `$placeholder` the expression references, sorted and de-duplicated.
    ///
    /// This is what `filterValues` has to bind for an `INCLUDE` expression
    /// ("you must provide values for all parameters that are defined in the
    /// expression").
    pub fn referenced_parameters(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for (_, clause) in &self.clauses {
            let operands: Vec<&Operand> = match &clause.predicate {
                Predicate::In { values, .. } => values.iter().collect(),
                Predicate::Compare { operand, .. } => vec![operand],
            };
            for operand in operands {
                if let Operand::Parameter(name) = operand {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
        }
        names.sort();
        names
    }
}

/// One token of a filter expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word: a keyword, a qualified column, a `$placeholder` or a literal.
    Word(String),
    /// A double-quoted literal.
    Quoted(String),
    /// `(`
    Open,
    /// `)`
    Close,
    /// `,`
    Comma,
    /// A run of comparison characters (`=`, `<`, `>`, `!`).
    Operator(String),
}

/// Characters that start a comparison operator.
const OPERATOR_CHARS: [char; 4] = ['=', '<', '>', '!'];

/// Split `expression` into tokens.
fn tokenize(expression: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = expression.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else if ch == '"' {
            chars.next();
            let mut literal = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(next) => literal.push(next),
                    None => return Err("filter expression has an unterminated string".to_string()),
                }
            }
            tokens.push(Token::Quoted(literal));
        } else if ch == '(' {
            chars.next();
            tokens.push(Token::Open);
        } else if ch == ')' {
            chars.next();
            tokens.push(Token::Close);
        } else if ch == ',' {
            chars.next();
            tokens.push(Token::Comma);
        } else if OPERATOR_CHARS.contains(&ch) {
            let mut operator = String::new();
            while chars.peek().is_some_and(|next| OPERATOR_CHARS.contains(next)) {
                operator.push(chars.next().unwrap_or_default());
            }
            tokens.push(Token::Operator(operator));
        } else {
            let mut word = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_whitespace()
                    || next == '('
                    || next == ')'
                    || next == ','
                    || next == '"'
                    || OPERATOR_CHARS.contains(&next)
                {
                    break;
                }
                word.push(next);
                chars.next();
            }
            tokens.push(Token::Word(word));
        }
    }
    Ok(tokens)
}

/// A cursor over the token stream.
struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        if token.is_some() {
            self.at += 1;
        }
        token
    }

    /// The next token as a bare word, or an error naming `expected`.
    fn word(&mut self, expected: &str) -> Result<String, String> {
        match self.next() {
            Some(Token::Word(word)) => Ok(word),
            other => Err(format!(
                "filter expression expected {expected}, found {}",
                describe(other.as_ref())
            )),
        }
    }

    /// Consume the next word and require it to equal `keyword`, case-insensitively.
    fn keyword(&mut self, keyword: &str) -> Result<(), String> {
        let word = self.word(keyword)?;
        if word.eq_ignore_ascii_case(keyword) {
            Ok(())
        } else {
            Err(format!(
                "filter expression expected {keyword}, found '{word}'"
            ))
        }
    }

    /// Consume the next token and require it to be `token`.
    fn exact(&mut self, token: &Token, expected: &str) -> Result<(), String> {
        match self.next() {
            Some(found) if &found == token => Ok(()),
            other => Err(format!(
                "filter expression expected {expected}, found {}",
                describe(other.as_ref())
            )),
        }
    }

    /// One operand: `$parameter`, a quoted literal, or a bare literal.
    fn operand(&mut self) -> Result<Operand, String> {
        match self.next() {
            Some(Token::Quoted(literal)) => Ok(Operand::Literal(literal)),
            Some(Token::Word(word)) => match word.strip_prefix('$') {
                Some(name) if !name.is_empty() => Ok(Operand::Parameter(name.to_string())),
                Some(_) => Err("filter expression has a '$' with no parameter name".to_string()),
                None => Ok(Operand::Literal(word)),
            },
            other => Err(format!(
                "filter expression expected a value, found {}",
                describe(other.as_ref())
            )),
        }
    }

    /// One clause.
    fn clause(&mut self) -> Result<Clause, String> {
        let qualified = self.word("a <Dataset>.<COLUMN> reference")?;
        let (dataset, column) = qualified.split_once('.').ok_or_else(|| {
            format!("filter expression expected a <Dataset>.<COLUMN> reference, found '{qualified}'")
        })?;
        if dataset != DATASET_ITEMS && dataset != DATASET_ACTIONS {
            return Err(format!(
                "filter expression references the {dataset} dataset; this emulator evaluates \
                 filter expressions over {DATASET_ITEMS} and {DATASET_ACTIONS} only"
            ));
        }
        if column.is_empty() {
            return Err(format!(
                "filter expression reference '{qualified}' names no column"
            ));
        }
        let predicate = self.predicate()?;
        orderable(dataset, column, &predicate)?;
        Ok(Clause {
            dataset: dataset.to_string(),
            column: column.to_string(),
            predicate,
        })
    }

    /// The predicate of a clause.
    fn predicate(&mut self) -> Result<Predicate, String> {
        match self.peek().cloned() {
            Some(Token::Operator(operator)) => {
                self.at += 1;
                let op = match operator.as_str() {
                    "=" | "==" => Comparison::Equal,
                    "<>" | "!=" => Comparison::NotEqual,
                    "<" => Comparison::Less,
                    "<=" => Comparison::LessOrEqual,
                    ">" => Comparison::Greater,
                    ">=" => Comparison::GreaterOrEqual,
                    other => {
                        return Err(format!(
                            "filter expression has an unknown comparison operator '{other}'"
                        ))
                    }
                };
                Ok(Predicate::Compare {
                    op,
                    operand: self.operand()?,
                })
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("NOT") => {
                self.at += 1;
                self.keyword("IN")?;
                Ok(Predicate::In {
                    values: self.value_set()?,
                    negated: true,
                })
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("IN") => {
                self.at += 1;
                Ok(Predicate::In {
                    values: self.value_set()?,
                    negated: false,
                })
            }
            other => Err(format!(
                "filter expression expected IN, NOT IN or a comparison, found {}",
                describe(other.as_ref())
            )),
        }
    }

    /// `( value, value, ... )`.
    fn value_set(&mut self) -> Result<Vec<Operand>, String> {
        self.exact(&Token::Open, "'('")?;
        let mut values = vec![self.operand()?];
        while self.peek() == Some(&Token::Comma) {
            self.at += 1;
            values.push(self.operand()?);
        }
        self.exact(&Token::Close, "')'")?;
        Ok(values)
    }
}

/// How an unexpected token reads in an error message.
fn describe(token: Option<&Token>) -> String {
    match token {
        None => "the end of the expression".to_string(),
        Some(Token::Word(word)) => format!("'{word}'"),
        Some(Token::Quoted(literal)) => format!("\"{literal}\""),
        Some(Token::Open) => "'('".to_string(),
        Some(Token::Close) => "')'".to_string(),
        Some(Token::Comma) => "','".to_string(),
        Some(Token::Operator(operator)) => format!("'{operator}'"),
    }
}

/// The finite number a value names, or `None` when its text names none.
///
/// Surrounding whitespace is not part of a value, and neither infinity nor a
/// not-a-number is a quantity a dataset column holds, so only a finite number counts as
/// one: a value that is not one is text, and a clause that needs a number cannot be
/// applied to it.
fn numeric(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

/// Whether two values are equal: as numbers when both name one — so the same quantity
/// written two ways is one value — and as the text they hold otherwise.
fn equal_values(left: &str, right: &str) -> bool {
    match (numeric(left), numeric(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

/// The refusal an ordering comparison earns when the value it would compare against
/// names no number: naming the clause, the operator and the value, plus the
/// `filterValues` parameter the value came from when it came from one.
fn unorderable(
    dataset: &str,
    column: &str,
    op: Comparison,
    value: &str,
    parameter: Option<&str>,
) -> String {
    let source = match parameter {
        Some(name) => format!("filterValues binds the filter expression parameter ${name} to "),
        None => "filter expression compares ".to_string(),
    };
    format!(
        "{source}the non-numeric value '{value}', which the {dataset}.{column} {} comparison \
         cannot apply: {} compares numbers only",
        op.spelling(),
        op.spelling()
    )
}

/// `Err` when a clause orders `dataset`.`column` against a literal that names no
/// number.
///
/// Checked where the expression is parsed, so a filter whose own text carries such a
/// comparison is refused when it is CREATED rather than stored as a clause nothing can
/// satisfy. A `$placeholder` operand is checked where a request binds it ([`bind`]).
fn orderable(dataset: &str, column: &str, predicate: &Predicate) -> Result<(), String> {
    match predicate {
        Predicate::Compare {
            op,
            operand: Operand::Literal(literal),
        } if op.orders() && numeric(literal).is_none() => {
            Err(unorderable(dataset, column, *op, literal, None))
        }
        _ => Ok(()),
    }
}

/// Parse a `filterExpression`.
///
/// `Err` carries a message naming what was wrong, for the caller that registered the
/// filter: the real service validates the expression when the filter is CREATED, so a
/// stored filter is always one this evaluator can apply.
pub fn parse(expression: &str) -> Result<FilterExpression, String> {
    let mut parser = Parser {
        tokens: tokenize(expression)?,
        at: 0,
    };
    let action = parser.word("INCLUDE or EXCLUDE")?;
    let kind = if action.eq_ignore_ascii_case("INCLUDE") {
        FilterKind::Include
    } else if action.eq_ignore_ascii_case("EXCLUDE") {
        FilterKind::Exclude
    } else {
        return Err(format!(
            "filter expression must start with INCLUDE or EXCLUDE, found '{action}'"
        ));
    };
    let subject = parser.word("the filtered subject (ItemID or ActionID)")?;
    parser.keyword("WHERE")?;

    let mut clauses = vec![(Combinator::And, parser.clause()?)];
    while let Some(token) = parser.peek().cloned() {
        let Token::Word(word) = token else {
            return Err(format!(
                "filter expression expected AND or OR, found {}",
                describe(parser.peek())
            ));
        };
        let combinator = if word.eq_ignore_ascii_case("AND") {
            Combinator::And
        } else if word.eq_ignore_ascii_case("OR") {
            Combinator::Or
        } else {
            return Err(format!(
                "filter expression expected AND or OR, found '{word}'"
            ));
        };
        parser.at += 1;
        clauses.push((combinator, parser.clause()?));
    }

    Ok(FilterExpression {
        kind,
        subject,
        clauses,
    })
}

/// A clause whose `$placeholders` are already bound to the request's `filterValues`.
#[derive(Debug, Clone)]
struct BoundClause {
    column: String,
    predicate: BoundPredicate,
}

/// A predicate with every operand resolved to a concrete value.
///
/// A comparison is bound as whichever of the two things it asserts: `=` and `<>` are an
/// equality test over a value of either kind, while an ordering operator carries the
/// NUMBER its operand named — so binding is where a value that names none is refused,
/// and the evaluator has nothing left to answer "never true" for.
#[derive(Debug, Clone)]
enum BoundPredicate {
    In {
        values: Vec<String>,
        negated: bool,
    },
    /// `=` (`negated: false`) or `<>` (`negated: true`) against one value.
    Equals {
        value: String,
        negated: bool,
    },
    /// `<`, `<=`, `>` or `>=` against the number the operand named.
    Order {
        op: Comparison,
        value: f64,
    },
}

/// A filter expression bound to one request: ready to decide, per catalog entry,
/// whether it survives.
///
/// Binding happens ONCE per request, so a parameter the request failed to bind is
/// reported once — not once per catalog entry — and the per-entry decision is a pure
/// function of the entry's stored attributes.
#[derive(Debug, Clone)]
pub struct BoundFilter {
    kind: FilterKind,
    clauses: Vec<(Combinator, BoundClause)>,
}

/// Bind `expression` to a request's `filter_values` for entries of kind `entity`.
///
/// Follows the documented asymmetry between the two filter kinds: for an `INCLUDE`
/// expression "you must provide values for all parameters that are defined in the
/// expression", so an unbound parameter is an error; for an `EXCLUDE` expression "you
/// can omit the filter-values. In this case, Amazon Personalize doesn't use that
/// portion of the expression to filter recommendations", so the clause is dropped.
///
/// A parameter is unbound whenever the request names no value for it, whatever
/// predicate the clause carries: the key may be missing, or present with a value that
/// names no value at all ([`resolve`]). That decision is taken in ONE place —
/// [`unbound_parameter`] — so the two kinds differ only in what the rule does, never in
/// which requests reach it.
///
/// A clause over the other dataset cannot be evaluated against this entity's stored
/// attributes at all, which is an error for either kind: the caller applied an Items
/// filter to action recommendations, or the reverse. So is a value the request DID
/// supply that the clause cannot compare — an ordering operator against text: the
/// asymmetry above is about a value the request OMITS, and a supplied value the clause
/// cannot apply is reported here rather than filtering nothing.
pub fn bind(
    expression: &FilterExpression,
    entity: Entity,
    filter_values: Option<&HashMap<String, String>>,
) -> Result<BoundFilter, String> {
    let mut clauses: Vec<(Combinator, BoundClause)> = Vec::new();
    for (combinator, clause) in &expression.clauses {
        if clause.dataset != entity.dataset() {
            return Err(format!(
                "the filter applies to the {} dataset, which does not describe the {} this \
                 operation recommends",
                clause.dataset,
                entity.dataset()
            ));
        }
        let bound = match &clause.predicate {
            Predicate::In { values, negated } => {
                let mut bound_values: Vec<String> = Vec::new();
                let mut unbound: Option<String> = None;
                for operand in values {
                    match resolve(operand, filter_values) {
                        Some(resolved) => bound_values.extend(resolved),
                        None => {
                            if let Operand::Parameter(name) = operand {
                                unbound = Some(name.clone());
                            }
                        }
                    }
                }
                match unbound {
                    Some(name) => {
                        unbound_parameter(expression.kind, &name)?;
                        continue;
                    }
                    None => BoundPredicate::In {
                        values: bound_values,
                        negated: *negated,
                    },
                }
            }
            // A comparison asserts something about ONE value, so a parameter bound to
            // several is compared against the first.
            Predicate::Compare { op, operand } => {
                match resolve(operand, filter_values).and_then(|resolved| resolved.into_iter().next())
                {
                    Some(value) => {
                        bound_comparison(*op, &clause.dataset, &clause.column, operand, value)?
                    }
                    None => {
                        // Only a parameter can go unbound: a literal operand stands for
                        // itself.
                        if let Operand::Parameter(name) = operand {
                            unbound_parameter(expression.kind, name)?;
                        }
                        continue;
                    }
                }
            }
        };
        clauses.push((
            *combinator,
            BoundClause {
                column: clause.column.clone(),
                predicate: bound,
            },
        ));
    }
    Ok(BoundFilter {
        kind: expression.kind,
        clauses,
    })
}

/// One comparison of `clause`, bound to the `value` its operand named.
///
/// `=` and `<>` test equality, which a value of either kind answers. An ordering
/// operator compares numbers, so a value naming none leaves the clause unapplicable:
/// `Err` reports it — naming the `filterValues` parameter it came from — rather than
/// keeping a clause that could hold for no entry.
fn bound_comparison(
    op: Comparison,
    dataset: &str,
    column: &str,
    operand: &Operand,
    value: String,
) -> Result<BoundPredicate, String> {
    if !op.orders() {
        return Ok(BoundPredicate::Equals {
            value,
            negated: op == Comparison::NotEqual,
        });
    }
    match numeric(&value) {
        Some(number) => Ok(BoundPredicate::Order { op, value: number }),
        None => Err(unorderable(
            dataset,
            column,
            op,
            &value,
            match operand {
                Operand::Parameter(name) => Some(name.as_str()),
                Operand::Literal(_) => None,
            },
        )),
    }
}

/// What an expression of `kind` does with a clause whose parameter `name` the request
/// left unbound: refuse the request, or drop the clause.
///
/// `Err` is the refusal an `INCLUDE` expression owes; `Ok(())` means the caller drops
/// the clause, which is what an `EXCLUDE` expression does with a portion of the
/// expression it does not use.
fn unbound_parameter(kind: FilterKind, name: &str) -> Result<(), String> {
    if kind == FilterKind::Include {
        return Err(format!(
            "filterValues provides no value for the filter expression parameter \
             ${name}, which the INCLUDE expression requires"
        ));
    }
    Ok(())
}

/// One operand's concrete values — at least one — or `None` when the request did not
/// bind it to any value.
///
/// A parameter's value may carry several values: "Separate multiple values for one
/// parameter with a comma." A request that supplies the parameter with text that names
/// no value at all — nothing, only whitespace, or only separators, all of which
/// [`split_values`] reads as naming no value — binds it no more than an absent key
/// does, so it resolves the same way. An explicitly DELIMITED empty value is a value
/// the caller wrote, and binds the parameter to the empty string.
fn resolve(operand: &Operand, filter_values: Option<&HashMap<String, String>>) -> Option<Vec<String>> {
    match operand {
        Operand::Literal(literal) => Some(vec![literal.clone()]),
        Operand::Parameter(name) => {
            let raw = filter_values?.get(name)?;
            let values = split_values(raw);
            (!values.is_empty()).then_some(values)
        }
    }
}

/// The character that delimits a STRING value in a `filterValues` entry. The documented
/// request format writes it escaped for JSON (`"GENRES": "\"Comedy\",\"Drama\""`), so
/// what reaches a handler is the quote character itself.
const VALUE_QUOTE: char = '"';

/// The separator between the values of one `filterValues` entry.
const VALUE_SEPARATOR: char = ',';

/// The values one `filterValues` entry carries, with the delimiters removed.
///
/// "Provide the parameter name (in matching case) as a key and the filter value(s) as
/// the corresponding value. Separate multiple values for one parameter with a comma."
/// A string value is written enclosed in double quotes; a numeric or timestamp value is
/// written bare. The quotes DELIMIT the value and are not part of it, so what a clause
/// compares against a stored attribute is the text BETWEEN them — a separator between
/// them belongs to the value — while a bare value stands for itself, without the
/// whitespace that surrounds a separator.
fn split_values(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut values: Vec<String> = Vec::new();
    let mut at = 0usize;
    loop {
        while chars.get(at).is_some_and(|ch| ch.is_whitespace()) {
            at += 1;
        }
        let mut value = String::new();
        let quoted = starts_quote(&chars, at);
        if quoted {
            at += quote_width(&chars, at);
            while let Some(&ch) = chars.get(at) {
                if starts_quote(&chars, at) {
                    break;
                }
                value.push(ch);
                at += 1;
            }
            at += quote_width(&chars, at);
            // Whatever sits between the closing quote and the separator is not part of
            // the value either.
            while chars.get(at).is_some_and(|&ch| ch != VALUE_SEPARATOR) {
                at += 1;
            }
        } else {
            while let Some(&ch) = chars.get(at) {
                if ch == VALUE_SEPARATOR {
                    break;
                }
                value.push(ch);
                at += 1;
            }
            value = value.trim().to_string();
        }
        // A quoted value is kept even when it is empty — the caller wrote it — but an
        // entry that holds nothing between two separators names no value at all.
        if quoted || !value.is_empty() {
            values.push(value);
        }
        if chars.get(at).is_none() {
            return values;
        }
        // Step over the separator and read the next value.
        at += 1;
    }
}

/// Whether position `at` begins a value delimiter: the quote character, or the
/// backslash-escaped quote of a caller that passed the documented request format
/// through without undoing its JSON escaping.
fn starts_quote(chars: &[char], at: usize) -> bool {
    chars.get(at) == Some(&VALUE_QUOTE)
        || (chars.get(at) == Some(&'\\') && chars.get(at + 1) == Some(&VALUE_QUOTE))
}

/// How many characters the delimiter at `at` occupies. One, unless it still carries the
/// backslash of its JSON escaping.
fn quote_width(chars: &[char], at: usize) -> usize {
    if chars.get(at) == Some(&'\\') {
        2
    } else {
        1
    }
}

impl BoundFilter {
    /// Whether an entry carrying `attributes` survives this filter.
    pub fn retains(&self, attributes: &BTreeMap<String, String>) -> bool {
        let matched = self.matches(attributes);
        match self.kind {
            FilterKind::Include => matched,
            FilterKind::Exclude => !matched,
        }
    }

    /// Whether the expression's clauses match an entry carrying `attributes`.
    ///
    /// No clauses left to evaluate means nothing matches — reachable only for an
    /// `EXCLUDE` expression whose every clause the request left unbound, which the
    /// documentation says removes nothing.
    fn matches(&self, attributes: &BTreeMap<String, String>) -> bool {
        let mut clauses = self.clauses.iter();
        let Some((_, first)) = clauses.next() else {
            return false;
        };
        let mut holds = evaluate(first, attributes);
        for (combinator, clause) in clauses {
            let next = evaluate(clause, attributes);
            holds = match combinator {
                Combinator::And => holds && next,
                Combinator::Or => holds || next,
            };
        }
        holds
    }

    /// How many clauses this filter will actually evaluate — 0 when the request left
    /// every clause of an `EXCLUDE` expression unbound.
    pub fn bound_clause_count(&self) -> usize {
        self.clauses.len()
    }
}

/// Whether one bound clause holds for an entry carrying `attributes`.
///
/// A clause asserts something about the value the entry STORES for its column, so an
/// entry that stores no value for that column satisfies it under NO predicate: there is
/// nothing to compare, and negating the comparison does not turn an absent value into a
/// match any more than it does in SQL, where a predicate over a missing value is
/// unknown rather than true. That is decided here, once, before the predicate is read —
/// so `NOT IN`, `IN` and every comparison answer an attribute-less entry the same way,
/// and an entry is kept or removed only by the attributes it actually holds.
///
/// A column may hold SEVERAL values, so every predicate reads the ones the entry holds
/// the same way: the clause holds when any of them satisfies it, and a negated one when
/// none does.
fn evaluate(clause: &BoundClause, attributes: &BTreeMap<String, String>) -> bool {
    let Some(stored) = attributes.get(&clause.column) else {
        return false;
    };
    let held = || stored.split('|').map(str::trim);
    match &clause.predicate {
        BoundPredicate::In { values, negated } => {
            let matched = held().any(|held| values.iter().any(|value| equal_values(held, value)));
            matched != *negated
        }
        BoundPredicate::Equals { value, negated } => {
            let matched = held().any(|held| equal_values(held, value));
            matched != *negated
        }
        BoundPredicate::Order { op, value } => held()
            .any(|held| numeric(held).is_some_and(|number| op.holds(number, *value))),
    }
}

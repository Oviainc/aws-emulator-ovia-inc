// SPDX-License-Identifier: CC0-1.0

//! The PostgreSQL front end of this service's data plane.
//!
//! The Data API runs its statements against Aurora, and the engine this emulator
//! answers for is Aurora PostgreSQL, so a statement is PARSED AS POSTGRESQL
//! (`sqlparser`'s PostgreSQL dialect) before the SQL engine behind it is asked to
//! run anything. The parse decides four things:
//!
//! * the statement the engine runs — the same statement, rewritten into the form
//!   the engine's own grammar accepts. `value::type` is PostgreSQL's spelling of
//!   `CAST(value AS type)`, which is the spelling the engine takes;
//! * the PostgreSQL type of every result column the statement's own text
//!   determines (a cast, a literal), which is what `columnMetadata` reports for a
//!   computed column and what decides which `Field` member carries its values;
//! * which columns a `CREATE TABLE` declares with a sequence (PostgreSQL's
//!   `serial` family), so the engine assigns their values when an INSERT leaves
//!   them out, the way the sequence default does;
//! * whether the statement changes rows, which is what `numberOfRecordsUpdated`
//!   counts.
//!
//! Nothing here inspects SQL text: the only input to every decision is the parsed
//! statement, and the statement the engine runs is this module's rendering of that
//! same parse.
//!
//! ## The dialect a statement is written in
//!
//! The Data API serves Aurora MySQL DB clusters as well as Aurora PostgreSQL ones, so a
//! statement reaching it may be written in MySQL's dialect. Where the two dialects spell
//! ONE statement differently, the rewrite below renders it as the statement the engine
//! runs: MySQL's upsert (`INSERT … ON DUPLICATE KEY UPDATE`) is the engine's own
//! `INSERT … ON CONFLICT (<key>) DO UPDATE SET` ([`plan_upsert`]), with the key the
//! conflict is on named from the table's own declaration — which only the database holds,
//! so the caller of [`plan`] supplies it.
//!
//! ## The type catalog
//!
//! [`pg_type`] maps a type as a declaration or a cast spells it to the type
//! PostgreSQL's catalog holds, and to what the Data API reports for it. The type
//! codes are `java.sql.Types`' constants, which is what `ColumnMetadata.type`
//! carries; the names are PostgreSQL's own catalog names, and the accepted
//! spellings are the aliases PostgreSQL's "Data Types" chapter documents for each
//! one (`integer`, `int` and `int4` are one type). A precision is the number of
//! decimal digits the type's documented range holds (`smallint` spans ±32767, so
//! 5; `integer` ±2147483647, so 10; `bigint` 19; `double precision` carries 17
//! significant digits; `boolean` 1), a length-parameterised type reports the
//! length its declaration gave, and a type with no length limit reports the
//! largest value the member can carry.

use std::ops::ControlFlow;

use sqlparser::ast::{
    CastKind, ColumnOption, ColumnOptionDef, ConflictTarget, DataType, DoUpdate, ExactNumberInfo,
    Expr, FunctionArg, FunctionArgExpr, FunctionArguments, Ident, ObjectNamePart, ObjectType,
    OnConflict, OnConflictAction, OnInsert, SelectItem, SetExpr, Statement, TableConstraint,
    TableObject, Value, visit_expressions_mut,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::{MAX_DECLARED_ARGUMENT, SqlError};

/// How a name the caller wrote is rendered for the engine.
mod names;

/// The precision a type with no length limit reports: the largest length the
/// member can carry, which is what a client reads as "unlimited".
const UNLIMITED_LENGTH: i32 = i32::MAX;

/// How deeply one statement may nest an expression before the PARSE refuses it.
///
/// A bound on the parser itself, and so on the work one request can ask of it:
/// what it costs to parse a nested call grows with the nesting, so the nesting is
/// capped — well above what a client writes by hand (a statement a person writes
/// nests a handful of levels) and far below where the parse becomes expensive.
/// The engine carries its own, separate expression-depth limit for what it is
/// then asked to run.
const MAX_PARSE_DEPTH: usize = 16;

/// How many queries one statement may combine with `UNION`, `INTERSECT` or
/// `EXCEPT`.
///
/// The ENGINE's own compound-select limit, read from where it is set on the
/// connection, so this refuses nothing the engine would have run — and it bounds
/// a chain the PARSE does not: a combined query is parsed iteratively, so its
/// depth grows with the request text, while the tree it builds is walked and
/// dropped by recursion.
const MAX_SET_OPERATIONS: usize = super::MAX_COMPOUND_SELECT as usize;

/// How many of a statement's tokens may each make the statement tree one level
/// deeper (`a AND b AND …`, `a || b || …`, `a::text::text::…`).
///
/// Bounded for the same reason as the set operations above, and well above what
/// a client writes: a statement a person writes carries a few dozen such tokens,
/// and a generated one lists its values — which are not counted — rather than
/// chaining operators over them.
const MAX_CHAINED_LEVELS: usize = 1_024;

/// How many levels a statement's text could ask the tree to carry, as its tokens
/// show: the queries it combines, and the tokens that could each deepen it.
///
/// Counted on the TOKENS rather than on the tree, so the answer is reached
/// without building the tree it is about.
fn chained_levels(tokens: &[Token]) -> (usize, usize) {
    let mut set_operations = 0;
    let mut levels = 0;
    for token in tokens {
        if let Token::Word(word) = token {
            if matches!(
                word.keyword,
                Keyword::UNION | Keyword::INTERSECT | Keyword::EXCEPT | Keyword::MINUS
            ) {
                set_operations += 1;
            }
        }
        if deepens_the_tree(token) {
            levels += 1;
        }
    }
    (set_operations, levels)
}

/// Whether a token could make the tree one level deeper without the parse
/// recursing — an operator, a keyword that joins two operands, a cast, a
/// subscript.
///
/// CLOSED, and so an over-count rather than an exact one: what this names is
/// what CANNOT — the separators, the parentheses whose nesting the parser's own
/// recursion limit already counts, and the literals a chain is built OUT of.
/// EVERY other token counts, so an operator a later release of the parser or of
/// this dialect accepts is bounded by default rather than missed by a list
/// nobody updated.
fn deepens_the_tree(token: &Token) -> bool {
    !matches!(
        token,
        Token::Whitespace(_)
            | Token::Comma
            | Token::SemiColon
            | Token::LParen
            | Token::RParen
            | Token::Number(..)
            | Token::Char(_)
            | Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::EscapedStringLiteral(_)
            | Token::UnicodeStringLiteral(_)
            | Token::NationalStringLiteral(_)
            | Token::HexStringLiteral(_)
            | Token::Placeholder(_)
            | Token::EOF
    )
}

/// `java.sql.Types`' constants — the codes `ColumnMetadata.type` carries.
mod jdbc {
    pub const BIT: i32 = -7;
    pub const BIGINT: i32 = -5;
    pub const BINARY: i32 = -2;
    pub const CHAR: i32 = 1;
    pub const NUMERIC: i32 = 2;
    pub const INTEGER: i32 = 4;
    pub const SMALLINT: i32 = 5;
    pub const REAL: i32 = 7;
    pub const DOUBLE: i32 = 8;
    pub const VARCHAR: i32 = 12;
    pub const DATE: i32 = 91;
    pub const TIME: i32 = 92;
    pub const TIMESTAMP: i32 = 93;
    pub const ARRAY: i32 = 2003;
    /// A type with no `java.sql.Types` constant of its own.
    pub const OTHER: i32 = 1111;
}

/// What the Data API tells a caller whose `sql` carries more than one statement: a call
/// runs one statement, and this is the sentence that says so.
pub(super) const MULTISTATEMENT_REFUSAL: &str = "Multistatements aren't supported.";

/// What the modelled `UnsupportedResultException` says about a result whose type
/// it cannot carry. The model's own wording for the condition: "It contained a
/// multidimensional array."
pub(super) const MULTIDIMENSIONAL_ARRAY: &str = "The result contains a multidimensional array.";

/// What the Data API says about a floating value that is not a number at all: JSON has no
/// form for `NaN` or `Infinity`, so a result set carrying one cannot be rendered as the
/// single JSON string `formatRecordsAs=JSON` asks for, and the modelled
/// `UnsupportedResultException` says so in AWS's own words.
pub(super) const NON_FINITE_FLOAT: &str = "NaN and Infinity values are not supported";

/// How the Data API renders an error the DATABASE raised: the engine's own message,
/// introduced the way PostgreSQL introduces an error, and closed with the SQLSTATE of the
/// condition — which is what a client classifies the failure by.
fn postgresql_error(message: &str, sqlstate: &str) -> String {
    format!("ERROR: {message}; SQLState: {sqlstate}")
}

/// How PostgreSQL states that a statement reached a transaction a previous statement of
/// it already failed in: "current transaction is aborted, commands ignored until end of
/// transaction block", under the SQLSTATE of that condition (class 25, in-failed-SQL-
/// transaction).
///
/// An error the DATABASE raises for the statement, which is why it is reported as the
/// database's error and not as the transaction being gone: the transaction is still there,
/// and a commit or a rollback still ends it.
pub(super) fn transaction_aborted() -> String {
    postgresql_error(
        "current transaction is aborted, commands ignored until end of transaction block",
        SQLSTATE_IN_FAILED_TRANSACTION,
    )
}

/// How PostgreSQL states a statement it cannot parse: "syntax error at or near" the token
/// it stopped on, the POSITION of that token in the statement, and the SQLSTATE of a
/// syntax error.
///
/// The token and the place come from the PARSER's own answer — which names the token it
/// found and the line and column it found it at — so nothing here reads the caller's SQL
/// to decide what is wrong with it; the statement is read only to count the characters
/// before that line and column, which is the POSITION PostgreSQL reports.
///
/// A parser answer that names no such token keeps the parser's own text: there is nothing
/// to state PostgreSQL's sentence from.
pub(super) fn syntax_error(sql: &str, parser_message: &str) -> String {
    let Some((token, place)) = parser_message
        .split_once(PARSER_FOUND)
        .and_then(|(_, rest)| rest.split_once(PARSER_AT_LINE))
    else {
        return parser_message.to_string();
    };
    let Some((line, column)) = place.split_once(PARSER_COLUMN) else {
        return parser_message.to_string();
    };
    let (Ok(line), Ok(column)) = (
        line.trim().trim_end_matches(',').parse::<usize>(),
        column.trim().parse::<usize>(),
    ) else {
        return parser_message.to_string();
    };
    postgresql_error(
        &format!(
            "syntax error at or near \"{}\"; Position: {}",
            token.trim(),
            character_position(sql, line, column)
        ),
        SQLSTATE_SYNTAX_ERROR,
    )
}

/// How the parser's own answer introduces the token it stopped on and the place it found
/// it at.
const PARSER_FOUND: &str = " found: ";
const PARSER_AT_LINE: &str = " at Line: ";
const PARSER_COLUMN: &str = " Column: ";

/// The 1-based CHARACTER position of (`line`, `column`) in `sql`, which is what
/// PostgreSQL's `Position` reports — where the parser answers a line and a column.
fn character_position(sql: &str, line: usize, column: usize) -> usize {
    let before: usize = sql
        .split('\n')
        .take(line.saturating_sub(1))
        // The newline that ends each line counts as one character.
        .map(|earlier| earlier.chars().count() + 1)
        .sum();
    before + column.max(1)
}

/// The SQLSTATEs of the conditions this front end words, from PostgreSQL's "Error Codes"
/// appendix: a unique violation, a not-null violation, a statement in a transaction that
/// has already failed, a statement that cannot be parsed, and the two catalog lookups
/// that find nothing.
const SQLSTATE_UNIQUE_VIOLATION: &str = "23505";
const SQLSTATE_NOT_NULL_VIOLATION: &str = "23502";
const SQLSTATE_IN_FAILED_TRANSACTION: &str = "25P02";
const SQLSTATE_SYNTAX_ERROR: &str = "42601";
const SQLSTATE_UNDEFINED_TABLE: &str = "42P01";
const SQLSTATE_UNDEFINED_COLUMN: &str = "42703";

/// How the SQL engine behind this front end introduces each condition in its own
/// diagnostics, which is what is read to recognise the condition. Nothing is matched on
/// the statement's text: this reads the ENGINE's answer, not the caller's SQL.
const ENGINE_UNIQUE_VIOLATION: &str = "UNIQUE constraint failed: ";
const ENGINE_NOT_NULL_VIOLATION: &str = "NOT NULL constraint failed: ";
const ENGINE_UNDEFINED_TABLE: &str = "no such table: ";
const ENGINE_UNDEFINED_COLUMN: &str = "no such column: ";
/// The engine's other spelling of an undefined column, which names the table too: "table
/// <table> has no column named <column>".
const ENGINE_UNDEFINED_COLUMN_OF: &str = "table ";
const ENGINE_UNDEFINED_COLUMN_OF_MIDDLE: &str = " has no column named ";

/// The NAME the engine's diagnostic is about, read out of the text that follows its
/// prefix: the engine adds a hint and the statement it was parsing to some of its
/// messages, where PostgreSQL's own sentence carries the name alone.
fn named_in(rest: &str) -> String {
    let head = rest.split(" - ").next().unwrap_or(rest).trim();
    let head = head.split_whitespace().next().unwrap_or(head);
    head.trim_matches('"').to_string()
}

/// One column a constraint covers, as the engine names it: `table.column`.
fn split_qualified(qualified: &str) -> Option<(&str, &str)> {
    qualified.trim().rsplit_once('.')
}

/// The diagnostic the DATABASE THIS API ADVERTISES words an engine error with, or `None`
/// for a condition this front end does not word — whose own text the caller sees.
///
/// The Data API answers for Aurora PostgreSQL, so an error the database raised reaches the
/// caller in PostgreSQL's words and with PostgreSQL's SQLSTATE, whatever engine actually
/// ran the statement: that text and that code are what a client branches on, so an
/// engine-specific sentence in their place is a sentence no client can classify.
///
/// `primary_key_of` answers the columns a table's PRIMARY KEY covers, which is what
/// decides the NAME of the violated constraint: PostgreSQL names a table's primary-key
/// constraint `<table>_pkey` and a unique constraint over other columns
/// `<table>_<column…>_key`.
pub(super) fn postgresql_diagnostic(
    engine_message: &str,
    primary_key_of: &dyn Fn(&str) -> Vec<String>,
) -> Option<String> {
    if let Some(columns) = engine_message.strip_prefix(ENGINE_UNIQUE_VIOLATION) {
        let qualified: Vec<(&str, &str)> = columns.split(',').filter_map(split_qualified).collect();
        let (table, _) = *qualified.first()?;
        let violated: Vec<String> = qualified
            .iter()
            .map(|(_, column)| (*column).to_string())
            .collect();
        let constraint = if covers_the_same_columns(&primary_key_of(table), &violated) {
            format!("{table}_pkey")
        } else {
            format!("{table}_{}_key", violated.join("_"))
        };
        return Some(postgresql_error(
            &format!("duplicate key value violates unique constraint \"{constraint}\""),
            SQLSTATE_UNIQUE_VIOLATION,
        ));
    }
    if let Some(column) = engine_message.strip_prefix(ENGINE_NOT_NULL_VIOLATION) {
        let (table, column) = split_qualified(column)?;
        return Some(postgresql_error(
            &format!(
                "null value in column \"{column}\" of relation \"{table}\" violates not-null \
                 constraint"
            ),
            SQLSTATE_NOT_NULL_VIOLATION,
        ));
    }
    if let Some(table) = engine_message.strip_prefix(ENGINE_UNDEFINED_TABLE) {
        return Some(postgresql_error(
            &format!("relation \"{}\" does not exist", named_in(table)),
            SQLSTATE_UNDEFINED_TABLE,
        ));
    }
    if let Some(column) = engine_message.strip_prefix(ENGINE_UNDEFINED_COLUMN) {
        return Some(postgresql_error(
            &format!("column \"{}\" does not exist", named_in(column)),
            SQLSTATE_UNDEFINED_COLUMN,
        ));
    }
    if let Some((table, column)) = engine_message
        .strip_prefix(ENGINE_UNDEFINED_COLUMN_OF)
        .and_then(|rest| rest.split_once(ENGINE_UNDEFINED_COLUMN_OF_MIDDLE))
    {
        return Some(postgresql_error(
            &format!(
                "column \"{}\" of relation \"{}\" does not exist",
                named_in(column),
                named_in(table)
            ),
            SQLSTATE_UNDEFINED_COLUMN,
        ));
    }
    None
}

/// Whether two lists of column names cover the same columns, compared the way the engine
/// and the catalog spell them (without regard to case) and without regard to order.
fn covers_the_same_columns(one: &[String], other: &[String]) -> bool {
    if one.is_empty() || one.len() != other.len() {
        return false;
    }
    let fold = |names: &[String]| {
        let mut folded: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
        folded.sort_unstable();
        folded
    };
    fold(one) == fold(other)
}

/// What a value of a type is, as far as the `Field` union is concerned: the
/// member a value comes back in, and whether a number is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeClass {
    /// `booleanValue`.
    Boolean,
    /// `longValue`, subject to `longReturnType`.
    Integer,
    /// `doubleValue`.
    Float,
    /// An exact number, whose representation `decimalReturnType` controls.
    Decimal,
    /// `stringValue`.
    Text,
    /// A date, a time or a timestamp: `stringValue`, and not a case-sensitive
    /// value for comparison.
    Temporal,
    /// `blobValue`.
    Binary,
    /// `arrayValue`: one value that is a list of values of the element type.
    Array,
    /// A type the catalog does not name, whose values are reported as the engine
    /// holds them.
    Other,
}

/// A PostgreSQL type, as the catalog holds it and as `columnMetadata` reports it.
#[derive(Debug, Clone)]
pub struct PgType {
    /// The catalog name, lower case (`int4`, `varchar`, `serial`).
    pub name: String,
    pub class: TypeClass,
    /// The `java.sql.Types` code.
    pub jdbc_type: i32,
    pub precision: i32,
    pub scale: i32,
    /// The scale the declaration itself gave (`numeric(10,2)`), when it gave one.
    /// A `numeric` with no declared scale carries each value's own scale, so its
    /// values are rendered as they are and not padded to a scale nobody declared.
    pub declared_scale: Option<i32>,
    pub signed: bool,
    pub case_sensitive: bool,
    /// The declaration carries a sequence, so the engine assigns the column's
    /// value when an INSERT leaves it out.
    pub auto_increment: bool,
    /// The element type, for an array type.
    pub element: Option<Box<PgType>>,
    /// Why a result column of this type cannot be returned, when it cannot: the
    /// message the modelled `UnsupportedResultException` carries. `None` for a
    /// type whose values have a `Field` member to come back in.
    pub refusal: Option<String>,
}

impl PgType {
    /// The catalog's entry for a type, with the defaults its class implies: only
    /// a number is signed, and a value compared as text is case-sensitive.
    fn of(name: &str, class: TypeClass, jdbc_type: i32, precision: i32, scale: i32) -> Self {
        Self {
            name: name.to_string(),
            class,
            jdbc_type,
            precision,
            scale,
            declared_scale: None,
            signed: matches!(
                class,
                TypeClass::Integer | TypeClass::Float | TypeClass::Decimal
            ),
            case_sensitive: matches!(class, TypeClass::Text | TypeClass::Binary | TypeClass::Other),
            auto_increment: false,
            element: None,
            refusal: None,
        }
    }

    /// The same type, under the name a `serial` declaration gives it, with the
    /// sequence default that makes the engine assign its values.
    fn sequence(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self.auto_increment = true;
        self
    }

    /// A type whose values the Data API has no member to carry them in.
    fn unreturnable(mut self) -> Self {
        self.refusal = Some(format!(
            "The result contains the unsupported data type {}.",
            self.name.to_ascii_uppercase()
        ));
        self
    }

    /// The element type of an array type.
    pub fn element(&self) -> Option<&PgType> {
        self.element.as_deref()
    }
}

/// The array type whose elements are `element`.
///
/// PostgreSQL's catalog names an array type after its element (`text` ->
/// `_text`), and `java.sql.Types` has one code for every array. The digits, the
/// sign and the comparison are the ELEMENT's, because that is what an element of
/// the array carries.
///
/// An array OF arrays is the multidimensional array the model names as a result
/// the service cannot return, and an array whose element type has no member to
/// come back in cannot be returned either.
fn array_of(element: PgType) -> PgType {
    let refusal = if element.class == TypeClass::Array {
        Some(MULTIDIMENSIONAL_ARRAY.to_string())
    } else {
        element.refusal.clone()
    };
    PgType {
        name: format!("_{}", element.name),
        class: TypeClass::Array,
        jdbc_type: jdbc::ARRAY,
        precision: element.precision,
        scale: element.scale,
        declared_scale: element.declared_scale,
        signed: element.signed,
        case_sensitive: element.case_sensitive,
        auto_increment: false,
        refusal,
        element: Some(Box::new(element)),
    }
}

/// What one statement is, as the data plane needs to know it.
#[derive(Debug)]
pub struct Plan {
    /// The statement to run, in the engine's own grammar.
    pub sql: String,
    /// Whether the statement changes rows, so its row count is what
    /// `numberOfRecordsUpdated` reports. Only a data-manipulation statement has
    /// one; DDL and a read report none, as PostgreSQL's own command tags do.
    pub changes_rows: bool,
    /// The PostgreSQL type of each result column the statement's text determines,
    /// in the order the statement lists them. Empty when the statement's own text
    /// decides nothing (a wildcard, a statement with no result set).
    pub result_types: Vec<Option<PgType>>,
    /// The sequence-carrying columns this statement declares, which the store
    /// remembers so a later read of them reports the type they were declared
    /// with.
    pub sequence_columns: Vec<SequenceColumn>,
    /// Statements that complete this one, run after it: the sequence a
    /// declaration needs and the engine does not keep itself, and the further
    /// tables one statement empties.
    pub follow_on: Vec<String>,
    /// The tables this statement drops, whose remembered sequences go with them.
    pub dropped_tables: Vec<String>,
}

/// A column whose PostgreSQL declaration carries a sequence.
///
/// The column is declared to the engine as an integer, so that is the type the
/// engine reports for it; this is what remembers the name the declaration used
/// (`serial`, `bigserial`, `smallserial`), which is the name `columnMetadata`
/// reports and the one its precision and type code come from.
#[derive(Debug, Clone)]
pub struct SequenceColumn {
    /// The table, lower-cased: an unquoted SQL identifier is case-insensitive.
    pub table: String,
    /// The column, lower-cased.
    pub column: String,
    /// The catalog name of the declared type.
    pub type_name: String,
}

impl Plan {
    /// The type the statement's text gives result column `index`, when it gives
    /// one. The list is used only when it covers the result set exactly: a
    /// wildcard expands to columns the text does not list, so a mismatched count
    /// means nothing can be read off a position.
    pub fn result_type(&self, index: usize, columns: usize) -> Option<&PgType> {
        if self.result_types.len() != columns {
            return None;
        }
        self.result_types.get(index).and_then(Option::as_ref)
    }
}

/// Parse one PostgreSQL statement and plan it.
///
/// A call runs ONE statement — the model's `sql` member is "The SQL statement" —
/// so a request carrying several is refused rather than having all but the first
/// silently dropped.
///
/// The statement is CHECKED BEFORE IT IS PARSED, by [`chained_levels`], for how
/// many levels it could ask the statement tree to carry: the parse itself is
/// bounded by its recursion limit, but an operator chain is parsed ITERATIVELY,
/// so such a chain's depth grows with the request and no parser bound sees it —
/// while the tree it builds is walked and dropped by recursion. The check runs on
/// the tokens, which is a pass over the text that allocates nothing per level, so
/// a statement past the bound is refused before a tree of it exists at all.
pub fn plan(sql: &str, key_columns: &dyn Fn(&str) -> Vec<String>) -> Result<Plan, SqlError> {
    let dialect = PostgreSqlDialect {};
    let tokens = Tokenizer::new(&dialect, sql)
        .tokenize()
        .map_err(|error| SqlError::Engine(syntax_error(sql, &error.to_string())))?;
    let (set_operations, levels) = chained_levels(&tokens);
    if set_operations > MAX_SET_OPERATIONS {
        return Err(SqlError::InvalidRequest(format!(
            "this statement combines {set_operations} queries; a statement combines at most \
             {MAX_SET_OPERATIONS}"
        )));
    }
    if levels > MAX_CHAINED_LEVELS {
        return Err(SqlError::InvalidRequest(format!(
            "this statement is larger than one call runs: {levels} of its tokens could each make \
             it one level deeper, and at most {MAX_CHAINED_LEVELS} may"
        )));
    }
    let mut statements = Parser::new(&dialect)
        .with_recursion_limit(MAX_PARSE_DEPTH)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|error| SqlError::Engine(syntax_error(sql, &error.to_string())))?;
    if statements.len() != 1 {
        // "Multistatements aren't supported." — a call runs ONE statement (`sql` is "The
        // SQL statement"), and this is the sentence the service refuses a string carrying
        // several with, whatever it carries.
        return Err(SqlError::InvalidRequest(
            MULTISTATEMENT_REFUSAL.to_string(),
        ));
    }
    let mut statement = statements.remove(0);
    // The types the statement's own text gives its result columns are read from
    // the parse BEFORE it is rewritten, because the rewrite replaces a cast's
    // PostgreSQL target type with the engine type that holds its values.
    let result_types = result_types(&statement);
    normalise(&mut statement);
    // An upsert the caller wrote in MySQL's spelling is the same upsert the engine
    // spells with the conflicting key named, which only the table's own declaration
    // knows; done before the names are delimited, so the columns this builds are
    // rendered the way the caller's own are.
    plan_upsert(&mut statement, key_columns);
    // The names the statement uses are rendered as delimited identifiers, so a
    // word PostgreSQL admits as a name is one the engine's grammar reads as that
    // name too. Done AFTER the rewrites above, so the expressions they build are
    // rendered the same way as the ones the caller wrote.
    names::quote_identifiers(&mut statement);
    let (sequence_columns, mut follow_on) = plan_sequences(&mut statement);
    let (sql, emptied) = rendered(&statement);
    follow_on.extend(emptied);
    Ok(Plan {
        changes_rows: changes_rows(&statement),
        dropped_tables: dropped_tables(&statement),
        sql,
        result_types,
        sequence_columns,
        follow_on,
    })
}

/// The statement as the engine's grammar spells it, and the statements that
/// follow it.
///
/// `TRUNCATE` empties a table, which the engine spells as the delete of every
/// row in it — one delete per table it names, because the engine runs one
/// statement at a time. The row count the call reports is still none, because
/// emptying a table is not a statement that reports the rows it changed.
fn rendered(statement: &Statement) -> (String, Vec<String>) {
    match statement {
        Statement::Truncate { table_names, .. } if !table_names.is_empty() => {
            let mut deletes = table_names
                .iter()
                .map(|target| format!("DELETE FROM {}", target.name));
            let first = deletes.next().unwrap_or_else(|| statement.to_string());
            (first, deletes.collect())
        }
        _ => (statement.to_string(), Vec::new()),
    }
}

/// Rewrite the statement into the form the engine's grammar accepts: the same
/// statement, spelled the way the engine spells it.
///
/// * PostgreSQL's `value::type` and standard `CAST(value AS type)` are one and
///   the same cast, and the engine spells it only the second way; the cast's
///   TARGET becomes the engine type that holds a PostgreSQL value of that type —
///   text for a character, document, date or time type (so a date keeps its
///   digits and separators instead of being read as the number it starts with),
///   an integer, a real or an exact number for the numeric types, and a blob for
///   binary. The PostgreSQL type the cast named is still what the result column
///   reports: it is read off the parse before this runs.
/// * a typed literal, an interval, an array constructor and a clock reading are
///   each the same value written in the engine's own spelling (see the arms
///   below).
/// * `ILIKE` is a case-insensitive `LIKE`, which is what the engine's own `LIKE`
///   is for the ASCII letters it compares.
fn normalise(statement: &mut Statement) {
    // A locking clause asks the engine to hold the rows it read for the
    // transaction; this data plane runs one statement at a time against one
    // database, so the read it names is that same read, with nothing left for
    // the clause to say.
    if let Statement::Query(query) = statement {
        query.locks.clear();
    }
    let _ = visit_expressions_mut(statement, |expr| {
        match expr {
            Expr::Cast {
                kind, data_type, ..
            } => {
                if matches!(kind, CastKind::DoubleColon) {
                    *kind = CastKind::Cast;
                }
                *data_type = engine_type(&pg_type(&data_type.to_string()));
            }
            // A typed literal (`DATE '2024-01-01'`, `TIMESTAMP '...'`) is the
            // same value as the cast of that string to that type, which is the
            // form the engine takes.
            Expr::TypedString(typed) => {
                let engine = engine_type(&pg_type(&typed.data_type.to_string()));
                let value = &typed.value;
                if let Some(built) = parse_expression(&format!("CAST({value} AS {engine})")) {
                    *expr = built;
                }
            }
            // A span of time is held as the text it names, the way every other
            // type the engine has no storage class of its own for is.
            Expr::Interval(interval) => {
                if let Some(built) =
                    parse_expression(&format!("CAST({} AS TEXT)", interval.value))
                {
                    *expr = built;
                }
            }
            // An array constructor builds one value out of its elements, which
            // this data plane holds as the JSON document of those elements —
            // the form the engine itself builds and reads.
            Expr::Array(array) => {
                let elements = array
                    .elem
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(built) = parse_expression(&format!("json_array({elements})")) {
                    *expr = built;
                }
            }
            // `now()` is the statement's own clock reading, which the engine
            // spells as the standard keyword.
            Expr::Function(function) if is_now(&function.name.to_string()) => {
                if let Some(built) = parse_expression("CURRENT_TIMESTAMP") {
                    *expr = built;
                }
            }
            // A case-insensitive match, in the engine's spelling of it.
            Expr::ILike {
                negated,
                expr: value,
                pattern,
                escape_char,
                ..
            } => {
                let not = if *negated { " NOT" } else { "" };
                let escape = match escape_char {
                    Some(character) => format!(" ESCAPE '{character}'"),
                    None => String::new(),
                };
                if let Some(built) =
                    parse_expression(&format!("{value}{not} LIKE {pattern}{escape}"))
                {
                    *expr = built;
                }
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
}

/// Whether a function call is PostgreSQL's reading of the statement's clock.
fn is_now(name: &str) -> bool {
    name.eq_ignore_ascii_case("now")
}

/// MySQL's own name for the value an upsert's insert half would have stored, which
/// the engine spells as a column of the `excluded` row.
const MYSQL_INSERTED_VALUE_FUNCTION: &str = "VALUES";

/// The row an upsert's update half reads the values it would have inserted from, as
/// the engine names it.
const EXCLUDED_ROW: &str = "excluded";

/// Rewrite an upsert the caller wrote in MySQL's spelling into the one the engine
/// spells.
///
/// The Data API serves Aurora MySQL DB clusters as well as Aurora PostgreSQL ones, so
/// a statement reaching it may be written in MySQL's dialect, and
/// `INSERT … ON DUPLICATE KEY UPDATE <assignments>` is MySQL's UPSERT: the row is
/// inserted, and where it would duplicate a key the named columns of the row already
/// there are updated instead. That is the same statement as the engine's
/// `INSERT … ON CONFLICT (<key>) DO UPDATE SET <assignments>` — and PostgreSQL's own
/// spelling of it — with two differences this closes:
///
/// * the engine NAMES the key the conflict is on, which MySQL leaves implicit ("any
///   unique key"). The key comes from the table's own declaration, which only the
///   database holds, so the caller supplies it (`key_columns`). A table whose
///   declaration carries no key at all can raise no duplicate-key condition, so the
///   clause can never fire: it is DROPPED, and the insert runs as the insert MySQL runs
///   for such a table;
/// * MySQL's `VALUES(<column>)` inside an assignment is "the value that would have
///   been inserted", which the engine reads off the `excluded` row.
///
/// What this does NOT carry over is MySQL's affected-row COUNT: MySQL reports two rows
/// affected for a row its clause UPDATED and one for a row it inserted, while
/// `numberOfRecordsUpdated` reports what the engine behind this data plane counted, which
/// is one either way.
///
/// Nothing here inspects SQL text: the parse is rewritten and re-rendered, like every
/// other rewrite in this module.
fn plan_upsert(statement: &mut Statement, key_columns: &dyn Fn(&str) -> Vec<String>) {
    let Statement::Insert(insert) = statement else {
        return;
    };
    if !matches!(insert.on, Some(OnInsert::DuplicateKeyUpdate(_))) {
        return;
    }
    let TableObject::TableName(name) = &insert.table else {
        return;
    };
    let Some(table) = name.0.last().and_then(|part| match part {
        ObjectNamePart::Identifier(identifier) => Some(identifier.value.clone()),
        _ => None,
    }) else {
        return;
    };
    let keys = key_columns(&table);
    let Some(OnInsert::DuplicateKeyUpdate(mut assignments)) = insert.on.take() else {
        return;
    };
    if keys.is_empty() {
        // No key, so nothing the insert stores can duplicate one and the update half is
        // unreachable: the statement is the insert alone, which is what the engine is
        // handed.
        return;
    }
    for assignment in &mut assignments {
        read_inserted_values_off_the_excluded_row(&mut assignment.value);
    }
    insert.on = Some(OnInsert::OnConflict(OnConflict {
        conflict_target: Some(ConflictTarget::Columns(
            keys.into_iter().map(Ident::new).collect(),
        )),
        action: OnConflictAction::DoUpdate(DoUpdate {
            assignments,
            selection: None,
        }),
    }));
}

/// The columns of the FIRST unique key one table's own `CREATE TABLE` declares, and empty
/// for a declaration that carries none.
///
/// MySQL's upsert conflicts on any unique key, and a UNIQUE declaration is the half of
/// that the engine's permitted schema introspection does not report (`pragma_table_info`
/// reports the primary key alone), so it is read out of the declaration the engine stored
/// — PARSED, like every other statement this module reads, never inspected as text. A
/// single-column `UNIQUE` and a table-level `UNIQUE (a, b)` are both keys; the primary key
/// is not reported here, because its own reading already covers it. The FIRST such key is
/// the answer, for the reason [`crate::state::conflict_key_columns`] gives: the engine
/// spells an upsert with one conflict target.
#[must_use]
pub fn unique_key_columns(declaration: &str) -> Vec<String> {
    let Some(Statement::CreateTable(create)) = Parser::new(&PostgreSqlDialect {})
        .with_recursion_limit(MAX_PARSE_DEPTH)
        .try_with_sql(declaration)
        .and_then(|mut parser| parser.parse_statement())
        .ok()
    else {
        return Vec::new();
    };
    for column in &create.columns {
        if column.options.iter().any(|option| {
            matches!(
                option.option,
                ColumnOption::Unique {
                    is_primary: false,
                    ..
                }
            )
        }) {
            return vec![column.name.value.clone()];
        }
    }
    for constraint in &create.constraints {
        if let TableConstraint::Unique { columns, .. } = constraint {
            let named: Vec<String> = columns
                .iter()
                .filter_map(|column| match &column.column.expr {
                    Expr::Identifier(name) => Some(name.value.clone()),
                    _ => None,
                })
                .collect();
            if !named.is_empty() {
                return named;
            }
        }
    }
    Vec::new()
}

/// Rewrite every `VALUES(<column>)` in one assignment into the engine's reading of
/// that same value: the column of the `excluded` row.
fn read_inserted_values_off_the_excluded_row(value: &mut Expr) {
    let _ = visit_expressions_mut(value, |expr| {
        if let Expr::Function(function) = expr {
            if function
                .name
                .to_string()
                .eq_ignore_ascii_case(MYSQL_INSERTED_VALUE_FUNCTION)
            {
                if let Some(column) = sole_column_argument(&function.args) {
                    *expr = Expr::CompoundIdentifier(vec![Ident::new(EXCLUDED_ROW), column]);
                }
            }
        }
        ControlFlow::<()>::Continue(())
    });
}

/// The one column a `VALUES(<column>)` names, or `None` for any other argument list —
/// which is left as the caller wrote it, for the engine to answer.
fn sole_column_argument(args: &FunctionArguments) -> Option<Ident> {
    let FunctionArguments::List(list) = args else {
        return None;
    };
    match list.args.as_slice() {
        [FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Identifier(column)))] => {
            Some(column.clone())
        }
        _ => None,
    }
}

/// Parse one expression this module built, back into the tree the statement
/// carries. The input is this module's own rendering, so a failure to parse it is
/// this module's bug and leaves the expression as the caller wrote it, for the
/// engine to accept or refuse.
fn parse_expression(sql: &str) -> Option<Expr> {
    let mut parser = Parser::new(&PostgreSqlDialect {})
        .with_recursion_limit(MAX_PARSE_DEPTH)
        .try_with_sql(sql)
        .ok()?;
    parser.parse_expr().ok()
}

/// The engine type that holds a PostgreSQL value of this type.
fn engine_type(pg: &PgType) -> DataType {
    match pg.class {
        TypeClass::Integer => DataType::Integer(None),
        TypeClass::Float => DataType::Real,
        // A boolean is held as the 1 or 0 the engine compares and stores.
        TypeClass::Boolean | TypeClass::Decimal => DataType::Numeric(ExactNumberInfo::None),
        TypeClass::Binary => DataType::Blob(None),
        // An array is held as the document of its elements, which is text.
        TypeClass::Text | TypeClass::Temporal | TypeClass::Array | TypeClass::Other => {
            DataType::Text
        }
    }
}

/// Whether the statement manipulates rows, and so has a row count to report.
fn changes_rows(statement: &Statement) -> bool {
    matches!(
        statement,
        Statement::Insert(_) | Statement::Update { .. } | Statement::Delete(_)
    )
}

/// The PostgreSQL type of each column the statement's own text determines.
fn result_types(statement: &Statement) -> Vec<Option<PgType>> {
    let projection = match statement {
        Statement::Query(query) => leftmost_projection(&query.body),
        Statement::Insert(insert) => insert.returning.as_deref(),
        Statement::Delete(delete) => delete.returning.as_deref(),
        Statement::Update { returning, .. } => returning.as_deref(),
        _ => None,
    };
    projection
        .unwrap_or_default()
        .iter()
        .map(selected_type)
        .collect()
}

/// The projection whose columns a query's result set takes its names and types
/// from: the leftmost branch of a set operation, which is the one PostgreSQL
/// names the result after.
///
/// Walked ITERATIVELY, because a combined query's depth grows with the request
/// text (see [`chained_levels`]) and a recursive walk of it would grow the stack
/// with it.
fn leftmost_projection(body: &SetExpr) -> Option<&[SelectItem]> {
    let mut body = body;
    loop {
        match body {
            SetExpr::Select(select) => return Some(&select.projection),
            SetExpr::Query(query) => body = &query.body,
            SetExpr::SetOperation { left, .. } => body = left,
            _ => return None,
        }
    }
}

/// The type one selected expression has, when its own form says.
fn selected_type(item: &SelectItem) -> Option<PgType> {
    match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
            expression_type(expr)
        }
        // A wildcard stands for columns the statement does not list; the engine
        // reports their declared types.
        _ => None,
    }
}

/// The type an expression has when its own form determines it: a cast names its
/// target type, and a literal has the type PostgreSQL gives that literal. Any
/// other expression is left to the engine, which reports the declared type of a
/// stored column and the storage class of a computed one.
fn expression_type(expr: &Expr) -> Option<PgType> {
    match expr {
        Expr::Cast { data_type, .. } => Some(pg_type(&data_type.to_string())),
        // A typed literal names its own type, and a span of time is an interval.
        Expr::TypedString(typed) => Some(pg_type(&typed.data_type.to_string())),
        Expr::Interval(_) => Some(pg_type("interval")),
        Expr::Nested(inner) => expression_type(inner),
        Expr::UnaryOp { expr, .. } => expression_type(expr),
        Expr::Value(value) => literal_type(&value.value),
        // A clock reading is a timestamp with a time zone.
        Expr::Function(function) if is_now(&function.name.to_string()) => {
            Some(pg_type("timestamptz"))
        }
        // An array constructor is an array of whatever its elements are, and an
        // array of arrays is the multidimensional array the model names.
        Expr::Array(array) => Some(array_of(
            array
                .elem
                .iter()
                .find_map(expression_type)
                .unwrap_or_else(|| pg_type("text")),
        )),
        _ => None,
    }
}

/// The type PostgreSQL gives a literal: an integer literal is the narrowest
/// integer type that holds it, a fractional literal is exact (`numeric`), a
/// quoted string is `text` and `true`/`false` is `bool`.
fn literal_type(value: &Value) -> Option<PgType> {
    match value {
        Value::Boolean(_) => Some(pg_type("bool")),
        Value::Number(digits, _) => Some(match digits.parse::<i32>() {
            Ok(_) => pg_type("int4"),
            Err(_) => match digits.parse::<i64>() {
                Ok(_) => pg_type("int8"),
                Err(_) => pg_type("numeric"),
            },
        }),
        Value::SingleQuotedString(_) | Value::DollarQuotedString(_) => Some(pg_type("text")),
        _ => None,
    }
}

/// The sequence of every `serial` column a `CREATE TABLE` declares: the columns
/// the store remembers, and the statements that install a sequence the engine
/// does not keep itself.
///
/// A `serial` declaration is "an autoincrementing integer column": the sequence
/// supplies the column's value when an INSERT leaves it out. The engine assigns
/// an integer PRIMARY KEY's value itself, during the insert, so a `serial`
/// primary key is declared to it as that — which is also what makes a `RETURNING`
/// clause report the assigned value, as the `generatedFields` documentation says
/// to read it. The key is declared AUTOINCREMENT as well, so the engine keeps the
/// sequence's own high-water mark rather than re-deriving it from the rows that
/// are still there: a KEY's sequence never hands out a value twice, so a value it
/// supplied for a row since deleted is not supplied again.
///
/// A `serial` column that is NOT its table's key is given its value by a trigger
/// on the table — the next value above the largest the column holds — because
/// only the key is assigned by the engine itself, and only the key's counter is
/// one the engine keeps: a non-key sequence therefore does re-use the value of a
/// row that has since been deleted.
fn plan_sequences(statement: &mut Statement) -> (Vec<SequenceColumn>, Vec<String>) {
    let Statement::CreateTable(create) = statement else {
        return (Vec::new(), Vec::new());
    };
    let table = create.name.to_string();
    let table_key = create
        .name
        .0
        .last()
        .map(|part| part.to_string())
        .unwrap_or_default()
        .trim_matches('"')
        .to_ascii_lowercase();
    let scope = if create.temporary { "TEMP " } else { "" };
    let mut columns = Vec::new();
    let mut sequences = Vec::new();
    for column in &mut create.columns {
        let declared = pg_type(&column.data_type.to_string());
        if !declared.auto_increment {
            continue;
        }
        let key = column
            .options
            .iter()
            .position(|option| matches!(option.option, ColumnOption::Unique { is_primary: true, .. }));
        column.data_type = DataType::Integer(None);
        match key {
            // The engine's own key counter, which it keeps where the key is
            // declared: directly after it, because that is where its grammar
            // takes it.
            Some(position) => column.options.insert(
                position + 1,
                ColumnOptionDef {
                    name: None,
                    option: ColumnOption::DialectSpecific(vec![Token::make_keyword(
                        "AUTOINCREMENT",
                    )]),
                },
            ),
            None => {
                let name = column.name.to_string();
                let trigger = quoted(&format!("{table_key}_{}_nextval", column.name.value));
                sequences.push(format!(
                    "CREATE {scope}TRIGGER IF NOT EXISTS {trigger} AFTER INSERT ON {table} \
                     WHEN NEW.{name} IS NULL \
                     BEGIN UPDATE {table} SET {name} = \
                     (SELECT IFNULL(MAX({name}), 0) + 1 FROM {table}) WHERE rowid = NEW.rowid; END"
                ));
            }
        }
        columns.push(SequenceColumn {
            table: table_key.clone(),
            column: column.name.value.to_ascii_lowercase(),
            type_name: declared.name.clone(),
        });
    }
    (columns, sequences)
}

/// The tables a `DROP TABLE` drops, lower-cased.
fn dropped_tables(statement: &Statement) -> Vec<String> {
    match statement {
        Statement::Drop {
            object_type: ObjectType::Table,
            names,
            ..
        } => names
            .iter()
            .map(|name| {
                name.0
                    .last()
                    .map(|part| part.to_string())
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_ascii_lowercase()
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// An identifier, quoted so that whatever it holds is one identifier.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The PostgreSQL type a declaration or a cast spells `declared`.
///
/// The name is matched against the catalog's own names and the aliases
/// PostgreSQL documents for them, so `integer`, `int` and `int4` are one type and
/// all three report `int4`. A type the catalog does not name is reported under the
/// name it was written with, with no `java.sql.Types` code of its own.
pub fn pg_type(declared: &str) -> PgType {
    let (name, arguments) = split_declared_type(declared);
    // An array type is named after its element, in either spelling the parser
    // renders (`text[]`, `ARRAY<TEXT>`).
    if let Some(element) = array_element_name(&name) {
        let element = match array_element_name(element) {
            // An array OF arrays is already a result this service cannot
            // return, so the nesting is not followed further than it takes to
            // see it: however deeply a caller nests it, this looks twice.
            Some(_) => PgType::of(element, TypeClass::Array, jdbc::ARRAY, UNLIMITED_LENGTH, 0),
            None => pg_type(&with_arguments(element, &arguments)),
        };
        return array_of(element);
    }
    let length = arguments.first().copied();
    let scale = arguments.get(1).copied();
    match name.as_str() {
        "bool" | "boolean" => PgType::of("bool", TypeClass::Boolean, jdbc::BIT, 1, 0),
        "int2" | "smallint" => PgType::of("int2", TypeClass::Integer, jdbc::SMALLINT, 5, 0),
        "int4" | "int" | "integer" => PgType::of("int4", TypeClass::Integer, jdbc::INTEGER, 10, 0),
        "int8" | "bigint" => PgType::of("int8", TypeClass::Integer, jdbc::BIGINT, 19, 0),
        "oid" => PgType::of("oid", TypeClass::Integer, jdbc::BIGINT, 10, 0),
        "float4" | "real" => PgType::of("float4", TypeClass::Float, jdbc::REAL, 8, 8),
        "float8" | "double precision" => {
            PgType::of("float8", TypeClass::Float, jdbc::DOUBLE, 17, 17)
        }
        "numeric" | "decimal" | "dec" => {
            let mut numeric = PgType::of(
                "numeric",
                TypeClass::Decimal,
                jdbc::NUMERIC,
                length.unwrap_or(0),
                scale.unwrap_or(0),
            );
            numeric.declared_scale = scale;
            numeric
        }
        "text" => PgType::of("text", TypeClass::Text, jdbc::VARCHAR, UNLIMITED_LENGTH, 0),
        "varchar" | "character varying" => PgType::of(
            "varchar",
            TypeClass::Text,
            jdbc::VARCHAR,
            length.unwrap_or(UNLIMITED_LENGTH),
            0,
        ),
        "bpchar" | "char" | "character" => PgType::of(
            "bpchar",
            TypeClass::Text,
            jdbc::CHAR,
            length.unwrap_or(1),
            0,
        ),
        // The type PostgreSQL's catalog uses for an identifier, whose documented
        // length is 63 bytes.
        "name" => PgType::of("name", TypeClass::Text, jdbc::VARCHAR, 63, 0),
        "uuid" => PgType::of("uuid", TypeClass::Text, jdbc::OTHER, UNLIMITED_LENGTH, 0),
        "json" => PgType::of("json", TypeClass::Text, jdbc::OTHER, UNLIMITED_LENGTH, 0),
        "jsonb" => PgType::of("jsonb", TypeClass::Text, jdbc::OTHER, UNLIMITED_LENGTH, 0),
        "bytea" => PgType::of(
            "bytea",
            TypeClass::Binary,
            jdbc::BINARY,
            UNLIMITED_LENGTH,
            0,
        ),
        // A date and time type reports the width of the value it displays:
        // `YYYY-MM-DD` is 13 with room for an era, a time adds the clock and a
        // fraction, a timestamp both, and a zone offset six more.
        "date" => PgType::of("date", TypeClass::Temporal, jdbc::DATE, 13, 0),
        "time" | "time without time zone" => {
            PgType::of("time", TypeClass::Temporal, jdbc::TIME, 15, 0)
        }
        "timetz" | "time with time zone" => {
            PgType::of("timetz", TypeClass::Temporal, jdbc::TIME, 21, 0)
        }
        "timestamp" | "timestamp without time zone" => {
            PgType::of("timestamp", TypeClass::Temporal, jdbc::TIMESTAMP, 29, 0)
        }
        "timestamptz" | "timestamp with time zone" => {
            PgType::of("timestamptz", TypeClass::Temporal, jdbc::TIMESTAMP, 35, 0)
        }
        "serial" | "serial4" => {
            PgType::of("int4", TypeClass::Integer, jdbc::INTEGER, 10, 0).sequence("serial")
        }
        "bigserial" | "serial8" => {
            PgType::of("int8", TypeClass::Integer, jdbc::BIGINT, 19, 0).sequence("bigserial")
        }
        "smallserial" | "serial2" => {
            PgType::of("int2", TypeClass::Integer, jdbc::SMALLINT, 5, 0).sequence("smallserial")
        }
        // A span of time is neither a number nor a string: the `Field` union has
        // no member for one, so a result column of that type is refused rather
        // than reported as something it is not.
        "interval" => PgType::of("interval", TypeClass::Other, jdbc::OTHER, 49, 0).unreturnable(),
        _ => PgType::of(&name, TypeClass::Other, jdbc::OTHER, UNLIMITED_LENGTH, 0),
    }
}

/// The element a type name names, when the name is an array type's.
fn array_element_name(name: &str) -> Option<&str> {
    name.strip_suffix("[]").or_else(|| {
        name.strip_prefix("array<")
            .and_then(|rest| rest.strip_suffix('>'))
    })
}

/// A type name with the arguments the declaration gave it put back, so an
/// element type carries the length its array's declaration named
/// (`varchar(8)[]`).
fn with_arguments(name: &str, arguments: &[i32]) -> String {
    if arguments.is_empty() {
        return name.to_string();
    }
    let arguments = arguments
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("{name}({arguments})")
}

/// The type a result column has when neither its declaration nor the statement's
/// text names one: the PostgreSQL type of the storage class the engine returned
/// its values in.
pub fn type_of_storage_class(class: Option<&str>) -> PgType {
    match class {
        Some("INTEGER") => pg_type("int8"),
        Some("REAL") => pg_type("float8"),
        Some("BLOB") => pg_type("bytea"),
        _ => pg_type("text"),
    }
}

/// Split a type as it was written into its name and its arguments:
/// `VARCHAR(32)` -> (`varchar`, [32]), `NUMERIC(10,2)` -> (`numeric`, [10, 2]),
/// `TIMESTAMP(3) WITH TIME ZONE` -> (`timestamp with time zone`, [3]). The
/// arguments are clamped, so a declaration cannot put an arbitrary number into
/// `columnMetadata` or into arithmetic over it.
fn split_declared_type(declared: &str) -> (String, Vec<i32>) {
    let lower = declared.to_ascii_lowercase();
    let (head, arguments, tail) = match lower.split_once('(') {
        // Whatever follows the arguments is part of the name
        // (`WITH TIME ZONE`), so the three parts are kept apart.
        Some((head, rest)) => match rest.split_once(')') {
            Some((arguments, tail)) => (head, arguments, tail),
            None => (head, rest, ""),
        },
        None => (lower.as_str(), "", ""),
    };
    let numbers = arguments
        .split(',')
        .filter_map(|argument| argument.trim().parse::<i32>().ok())
        .map(|number| number.clamp(0, MAX_DECLARED_ARGUMENT))
        .collect();
    let name = collapse_whitespace(&format!("{head} {tail}"));
    (name, numbers)
}

/// A type name with its words separated by exactly one space, so the multi-word
/// spellings PostgreSQL documents (`double precision`) match the catalog however
/// they were laid out.
fn collapse_whitespace(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

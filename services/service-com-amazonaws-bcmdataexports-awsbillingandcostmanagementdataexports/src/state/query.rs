// SPDX-License-Identifier: CC0-1.0

//! The `DataQuery` a submitted `Export` carries: the tables its `QueryStatement` names,
//! the columns it selects and filters on, and the refusals for a query this service cannot
//! serve.
//!
//! `CreateExport`'s own modelled documentation states what a query may say: "The
//! QueryStatement is an SQL statement. Data Exports only supports a limited subset of the
//! SQL syntax. … To view the available tables and columns, see the Data Exports table
//! dictionary" (smithy_query shape `CreateExportRequest`, member `Export`, quoted in
//! `src/ops/create_export.rs`). The tables AND COLUMNS that dictionary publishes are
//! exactly [`super::TABLE_CATALOG`] — the catalog `ListTables` lists and `GetTable`
//! describes — so a statement whose `FROM` names anything else, or which selects or
//! filters on a name no table it reads publishes, selects data this service does not hold,
//! and is refused rather than persisted.
//!
//! The statement is READ, not matched: it is parsed with the `sqlparser` crate and the
//! relations and the column references are taken off the parsed statement, so a table or
//! column named in a join, a sub-query or behind an alias is found exactly as one named in
//! a bare `FROM` or select list is.
//!
//! # Which columns a statement may name
//!
//! A table's published column set is not fixed: a table property "[changes] the data and
//! schema of a table" (the modelled doc for `GetTableRequest$TableProperties`), so the
//! columns a table publishes are the ones in force under the table configuration THE
//! REQUEST states — [`super::TableSpec::columns_in_force`], the same filter `GetTable`
//! reports a schema through. A statement may therefore name a column exactly when some
//! table it reads publishes that column under the properties this request puts in force:
//! a name no such table publishes selects a column the export could never deliver (and a
//! name a table publishes only at another value of a gating property is such a name), so
//! it is refused with the modelled `ValidationException` like an unpublished table is.
//!
//! Two kinds of name are NOT column references, and neither is checked:
//!
//! * a name the statement ITSELF defines, WHERE THAT NAME IS IN SCOPE — the alias of a
//!   select item, which a later clause of the same statement, or a query reading that
//!   one, can name. A select list is not such a clause: the names a select list defines
//!   are not in scope in the expressions of that same select list, so an identifier
//!   standing in one of them is a column reference like any other and is checked against
//!   the published columns — which is what keeps an unpublished name aliased to itself
//!   (`SELECT c AS c`) from resolving against its own definition;
//! * the parts of a qualified reference other than the one that resolves: the parts before
//!   the column qualify a relation (by table name or by alias) and the parts after index
//!   into the column's own value, and the catalog publishes neither. A qualified reference
//!   resolves when ONE of its parts is a published column, which is also why a qualifier is
//!   not resolved to a single relation: data exports publishes one catalog, and a statement
//!   may qualify by an alias of a derived relation the catalog never names.
//!
//! # What the published subset is, and what that makes of a CTE
//!
//! "Data Exports only supports a limited subset of the SQL syntax": the subset the data
//! query documentation publishes is a SELECT over the catalog's tables (a column list,
//! `FROM`, `WHERE`, and the joins between published tables), so a `QueryStatement` has to
//! be ONE statement, that statement a query, and every relation it names a published
//! table. That is checked against the PARSED statement in [`query_shape`], which is why
//! all three refusals below say the same thing about a statement this service cannot read
//! as a data query — text the parser cannot read at all, a statement that is not a query
//! (`DELETE`, `DROP`, `INSERT`, `EXPLAIN`, …), more than one statement, and a query that
//! reads no table — and none of them is decided from the statement's text.
//!
//! The direction matters: the generic dialect this parse uses is WIDER than the published
//! subset, so what it cannot read as a single query is outside the subset too and is
//! refused, while a shape it reads and the subset does not mention (a set operation, a
//! `GROUP BY`) is served as long as every relation it names is published — this service
//! refuses what no data export could deliver, not everything the documentation does not
//! spell out.
//!
//! A common table expression is outside the subset for the relation rule rather than the
//! statement rule: `WITH t AS (…) SELECT * FROM t` names `t`, the catalog publishes no
//! `t`, and the statement is refused like any other naming an unpublished table (a row of
//! `tests/all/export_definition_validation.rs` pins that). If a recording ever shows AWS
//! accepting a CTE, the fix is to register each `WITH` alias as a relation this walk does
//! not resolve — not to accept unresolved relation names generally.
//!
//! # The bounds on the work a statement can cause
//!
//! A recursive-descent parse, the walk over the tree it builds and the DROP of that tree
//! all descend with the statement's structure, and an over-deep descent is a
//! process-wide failure rather than a slow request — so the structure is bounded before
//! the parser is entered, and the recursion then runs on a stack sized for those bounds.
//! Four bounds, each stated where it is enforced:
//!
//! * LENGTH — the model's (`@length(1, 36000)`, enforced by the boundary validator before
//!   any handler runs);
//! * BRACKETING — [`MAX_QUERY_NESTING_DEPTH`], an early-out for the nesting a statement
//!   writes with brackets;
//! * TOKENS — [`MAX_QUERY_TOKENS`], which bounds the DEPTH of the tree whatever its
//!   shape: a descent consumes at least one token per level, so no AST can be deeper than
//!   the statement's token count. This is the bound that covers the nesting brackets do
//!   NOT express — a chain of binary or unary operators, of set operations, or of joins,
//!   all of which nest at bracket depth zero;
//! * STACK — [`QUERY_PARSE_STACK_BYTES`]: the parse, the relation walk and the drop run
//!   on a thread whose stack is sized for [`MAX_QUERY_TOKENS`] levels, so the depth those
//!   bounds still admit cannot reach the end of a worker thread's ordinary stack.
//!
//! Beside them, [`MAX_QUERY_RELATIONS`] bounds the catalog work the resolved relations
//! cause.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::ControlFlow;

use sqlparser::ast::{
    Expr, ObjectName, Query, SelectItem, SetExpr, Statement, Visit, Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use super::{clip_for_message, field_validation_failed, table_spec, TableSpec};
use crate::ops::types::{DataQuery, InternalServerException, ValidationException};

/// Why a submitted `DataQuery` was refused: because of the REQUEST, or because this
/// process could not read it.
///
/// The two are different answers to a caller — a 400 says "fix the statement", a 500 says
/// "try again later" — so they are kept apart here rather than folded into one, and each
/// operation lifts them into its own modelled error.
#[derive(Debug)]
pub enum QueryRefusal {
    /// The statement is at fault: the modelled `ValidationException` (HTTP 400).
    Invalid(ValidationException),
    /// This process could not read the statement at all — a thread it could not start, or
    /// a parse that failed in a way the parser does not report as an error: the modelled
    /// `InternalServerException` (HTTP 500). It says nothing about the statement, so it
    /// must not be reported as a bad request.
    Internal(InternalServerException),
}

impl From<ValidationException> for QueryRefusal {
    fn from(invalid: ValidationException) -> Self {
        Self::Invalid(invalid)
    }
}

/// The modelled input member a query refusal names (`DataQuery$QueryStatement`, reached
/// through `Export$DataQuery`).
const QUERY_STATEMENT_FIELD: &str = "Export.DataQuery.QueryStatement";

/// The modelled input member a table-configuration refusal names
/// (`DataQuery$TableConfigurations`).
///
/// `pub(crate)` because a refusal of a table-property VALUE this service resolves against
/// another service ([`super::referenced_resources`]) names the same member rather than a
/// second spelling of it.
pub(crate) const TABLE_CONFIGURATIONS_FIELD: &str = "Export.DataQuery.TableConfigurations";

/// The most DISTINCT tables one `QueryStatement` may name.
///
/// The statement itself is bounded by the model (`QueryStatement` is `@length(1, 36000)`
/// — smithy_query shape `DataQuery`, member `QueryStatement`), which the boundary
/// validator enforces before any handler runs, so the parse below is over at most 36 000
/// characters. This second bound is the EMULATOR's choice, and it is what keeps the work
/// of resolving the parsed relations finite and small whatever is inside those
/// characters: a statement selecting over the whole published catalog names a handful of
/// tables, while a crafted one can name thousands. A statement past the bound is refused
/// with the modelled `ValidationException` rather than resolved.
const MAX_QUERY_RELATIONS: usize = 64;

/// The deepest BRACKETING one `QueryStatement` may carry, counted before the parser is
/// entered.
///
/// An early-out on the nesting a statement writes with brackets — a sub-query, a
/// parenthesised predicate, a call's arguments — which is the nesting a query of the
/// published subset actually uses. It is NOT the general depth bound: a chain of binary or
/// unary operators, of set operations or of joins nests the tree at bracket depth zero, and
/// [`MAX_QUERY_TOKENS`] is what bounds those.
///
/// The value is the EMULATOR's choice. The published subset — a SELECT over the catalog's
/// tables with a column list, a `WHERE` and joins — brackets a handful of levels: a filter
/// over an `IN (SELECT …)` with parenthesised predicates inside it is 3 or 4. 32 is an
/// order of magnitude above that.
const MAX_QUERY_NESTING_DEPTH: usize = 32;

/// The most TOKENS one `QueryStatement` may carry.
///
/// This is the general depth bound: a recursive-descent parse descends at most one level
/// per token it consumes, and the tree it builds has one node per token at most, so the
/// token count is an upper bound on the depth of the parse, of the relation walk and of
/// the drop — whatever shape the nesting takes, bracketed or not.
///
/// The value is the EMULATOR's choice. The widest query the published catalog admits — a
/// SELECT naming every column of the largest table (140-odd), with a `WHERE` and a join —
/// is a few hundred tokens, so 4096 is an order of magnitude above any real query and
/// small enough for [`QUERY_PARSE_STACK_BYTES`] to cover with margin.
const MAX_QUERY_TOKENS: usize = 4096;

/// The stack the parse, the relation walk and the drop of the parsed statement run on.
///
/// Those three recursions descend with the tree, whose depth [`MAX_QUERY_TOKENS`] bounds,
/// so the stack is sized for that bound rather than left to the worker thread's ordinary
/// one (2 MiB on a Tokio worker): 4096 levels at a generous 2 KiB per frame is 8 MiB, and
/// 16 MiB leaves the same margin again. A thread's stack is RESERVED, not committed, so
/// this costs nothing a statement does not actually use, and the two operations that reach
/// it are rare control-plane calls.
///
/// `sqlparser` carries its own guard for the parse — it depends on the `recursive` crate,
/// which grows the stack for a deep descent (see this workspace's `Cargo.lock` entry for
/// `sqlparser 0.59.0`) — but the walk and the drop are this code's own recursions over the
/// tree, so the sizing here is what covers them.
const QUERY_PARSE_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The name the parse thread carries, so a stack trace or a `top` says what it is.
const QUERY_PARSE_THREAD_NAME: &str = "bcm-data-exports-query-parse";

/// The number of tokens `statement` carries: each run of identifier characters, and each
/// other non-whitespace character, counts once.
///
/// A single pass over the characters, so computing the bound costs less than the parse it
/// guards, and it OVER-counts rather than under-counts the parser's own tokens (a
/// two-character operator counts twice, the characters of a quoted literal count
/// individually) — which is the safe direction for a bound on depth.
fn token_count(statement: &str) -> usize {
    let mut tokens: usize = 0;
    let mut inside_word = false;
    for character in statement.chars() {
        if character.is_alphanumeric() || character == '_' || character == '$' {
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

/// The depth of `statement`'s bracketing: the most `(` open at any one point.
///
/// A single pass over the characters, so computing the bound costs less than the parse it
/// guards. It counts the nesting a statement writes with brackets — a sub-query, a
/// parenthesised predicate, a function call's arguments.
///
/// Brackets inside a string literal or a comment are counted too, which can only make this
/// over-count: a statement refused here is refused for carrying that many brackets, never
/// served when it nests deeper. At a bound of [`MAX_QUERY_NESTING_DEPTH`] that costs
/// nothing a real query would notice (a published-subset query carries a handful of
/// brackets, literal ones included); skipping quoted spans is what to add if the bound is
/// ever tightened towards what a query really uses.
fn nesting_depth(statement: &str) -> usize {
    let mut depth: usize = 0;
    let mut deepest: usize = 0;
    for character in statement.chars() {
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

/// What one `QueryStatement` NAMES: the relations it reads, the column references it
/// makes, and the names it defines itself.
///
/// Read off the PARSED statement rather than its text, so a name in a join, a sub-query or
/// behind an alias is reported exactly as one in a bare `FROM` or select list is.
#[derive(Debug, Default)]
pub struct QueryShape {
    /// Every relation the statement names, in walk order, each once.
    pub tables: Vec<String>,
    /// What the statement's OUTERMOST select list projects, in the order it writes it: the
    /// columns a delivery of this definition writes into the export file (see
    /// [`delivered_columns`]).
    pub projection: Vec<Projected>,
    /// Every column reference the statement makes, in walk order: each once per scope it
    /// was read in, so a reference written in a select list is kept both as that list reads
    /// it and as the statement at large does.
    columns: Vec<ColumnReference>,
    /// The names the statement itself DEFINES: the alias of a select item, which a later
    /// clause of the same statement may name although no table publishes it.
    defined: BTreeSet<String>,
    /// The aliases of each SELECT LIST the statement writes, in the order they were read:
    /// the names that list defines, which its own expressions cannot name because a select
    /// list defines them for the clauses that come after it. A reference written in one
    /// carries that list's index.
    select_lists: Vec<BTreeSet<String>>,
}

impl QueryShape {
    /// Is `name` — a name the statement defines — in scope where `reference` stands?
    ///
    /// It is everywhere except in the expressions of the select list that defines it.
    fn defined_name_in_scope(&self, reference: &ColumnReference, name: &str) -> bool {
        let Some(list) = reference.select_list else {
            // Anywhere but a select list, every name the statement defines is in scope.
            return true;
        };
        // The index was handed out by the walk that recorded the list, so a missing entry
        // is unreachable; reading it as a list that defines nothing keeps this total
        // rather than indexing.
        self.select_lists
            .get(list)
            .is_none_or(|defined| !defined.contains(name))
    }
}

/// ONE item of a statement's outermost select list: what the export file carries in that
/// position.
///
/// Read off the PARSED statement, like everything else here, so a column selected behind an
/// alias or a qualifier is reported as the statement selects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Projected {
    /// A wildcard (`*`, or one qualified by a relation): every column in force for the
    /// tables the statement reads.
    AllColumns,
    /// A reference to a name, by its identifier parts: the part that resolves to a
    /// published column is the column this position carries (see [`resolve_data_query`]
    /// for why a reference's other parts are not columns).
    Reference(Vec<String>),
    /// A position whose name the statement itself states: the alias of a select item, or an
    /// unaliased expression, named as the statement writes it.
    Named(String),
}

/// One column reference a statement makes.
#[derive(Debug)]
struct ColumnReference {
    /// Every identifier part of the reference, in order: one part for a bare column name,
    /// and for a qualified one the relation (or the alias) it is qualified by, the column,
    /// and whatever indexes into the column's value.
    parts: Vec<String>,
    /// The reference as the statement writes it, which is what a refusal names.
    written: String,
    /// The select list this reference is written in, as an index into
    /// [`QueryShape::select_lists`], whose aliases are therefore not in scope for it.
    /// `None` for a reference standing anywhere else, where every name the statement
    /// defines is in scope.
    select_list: Option<usize>,
}

/// The relations and column references of `statement`.
///
/// `Err` is [`QueryRefusal::Invalid`] — the modelled 400 — for a statement past one of the
/// structural bounds (its bracketing deeper than [`MAX_QUERY_NESTING_DEPTH`] or its token
/// count past [`MAX_QUERY_TOKENS`], both refused BEFORE the parser is entered), for one
/// this service cannot read AS A DATA QUERY — text that does not parse, a statement that
/// is not a query, more than one statement, or a query reading no table at all, none of
/// which selects rows this service could ever deliver — and for one naming more distinct
/// relations than [`MAX_QUERY_RELATIONS`]; and [`QueryRefusal::Internal`] — the modelled
/// 500 — when this process could not run the parse at all.
pub fn query_shape(statement: &str) -> Result<QueryShape, QueryRefusal> {
    // Both structural bounds are applied to the TEXT, so no descent over this statement
    // has begun when it is refused.
    if nesting_depth(statement) > MAX_QUERY_NESTING_DEPTH {
        return Err(too_deeply_nested().into());
    }
    if token_count(statement) > MAX_QUERY_TOKENS {
        return Err(too_many_tokens().into());
    }
    // The parse, the walk over the tree it builds and the DROP of that tree all recurse
    // with the tree's depth, which the token bound above caps — so all three run on a
    // thread whose stack is sized for that cap rather than on a worker thread's ordinary
    // one. The work is bounded and this thread is joined before returning, so nothing is
    // left running behind the request.
    //
    // A scoped thread rather than `tokio::task::spawn_blocking`: the SIZED stack is the
    // whole point here, and a blocking-pool task takes the runtime's stack size, which no
    // caller of this function can set. The cost is that the calling worker is parked for
    // the parse — bounded by the token cap above — which is the trade this makes.
    std::thread::scope(|scope| {
        let spawned = ::std::thread::Builder::new()
            .name(QUERY_PARSE_THREAD_NAME.to_string())
            .stack_size(QUERY_PARSE_STACK_BYTES)
            .spawn_scoped(scope, || shape_of(statement));
        match spawned {
            // The parse ran: its own answer stands, whether that is the shape it read or
            // the 400 for a statement it could not read.
            Ok(handle) => handle.join().unwrap_or_else(|_| {
                // The parse PANICKED. That is this process failing, not the caller's
                // statement being wrong, so it is the modelled server error.
                Err(QueryRefusal::Internal(InternalServerException {
                    message: "The query statement could not be read.".to_string(),
                }))
            }),
            // A thread this process could not start: likewise a server fault.
            Err(_) => Err(QueryRefusal::Internal(InternalServerException {
                message: "The query statement could not be read.".to_string(),
            })),
        }
    })
}

/// The shape of `statement`, parsed and walked. Called on the sized stack
/// [`query_shape`] sets up; the bounds it states are already applied.
fn shape_of(statement: &str) -> Result<QueryShape, QueryRefusal> {
    let parsed = Parser::parse_sql(&GenericDialect {}, statement)
        .map_err(|_| unreadable_query(statement))?;
    // THE PUBLISHED SUBSET, checked on the parsed statement: a data query says which rows
    // of the catalog's tables an export delivers, so it is ONE statement and that
    // statement is a query. Anything else the generic dialect happens to read — a `DELETE`
    // or `DROP` naming a published table, an `INSERT`, an `EXPLAIN`, two statements
    // separated by a semicolon — selects nothing this service can export (and the first
    // two ask it to CHANGE the catalog, which no data export does), so it is refused here
    // rather than persisted.
    let [Statement::Query(query)] = parsed.as_slice() else {
        return Err(not_a_single_query(statement).into());
    };
    // ONE walk over the parsed tree reports the relations, the column references and the
    // names the statement defines itself, so nothing is read out of the statement's text.
    let mut walk = QueryWalk::default();
    if query.as_ref().visit(&mut walk).is_break() {
        return Err(too_many_relations().into());
    }
    // A query reading NO relation (`SELECT 1`, `VALUES (1)`) is a query over the request's
    // own literals, not over the catalog: there is no table for the export to deliver, so
    // it is outside the subset too.
    if walk.shape.tables.is_empty() {
        return Err(reads_no_table(statement).into());
    }
    // WHAT THE STATEMENT PROJECTS, read off the outermost query body rather than off the
    // walk: the walk reports every select list the statement writes, while the file a
    // delivery produces carries the columns of the one the statement ANSWERS with.
    walk.shape.projection = projection_of(&query.body);
    Ok(walk.shape)
}

/// What the outermost select list of `body` projects.
///
/// A set operation projects what its first arm does: every arm of a `UNION` answers the
/// same columns, so the first one names them. A body this parse reads as neither a select
/// nor a set operation names no position of its own, so the whole of the published columns
/// is what it answers with.
fn projection_of(body: &SetExpr) -> Vec<Projected> {
    match body {
        SetExpr::Select(select) => select.projection.iter().map(projected_item).collect(),
        SetExpr::Query(query) => projection_of(&query.body),
        SetExpr::SetOperation { left, .. } => projection_of(left),
        _ => vec![Projected::AllColumns],
    }
}

/// One select item as the position it projects.
fn projected_item(item: &SelectItem) -> Projected {
    match item {
        // A wildcard, qualified or not, selects the columns themselves.
        SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(..) => Projected::AllColumns,
        // An alias NAMES the position: "SELECT c AS total" answers a column called `total`.
        SelectItem::ExprWithAlias { alias, .. } => Projected::Named(alias.value.clone()),
        SelectItem::UnnamedExpr(expr) => match identifier_parts(expr) {
            Some(parts) => Projected::Reference(parts),
            // An unaliased EXPRESSION — `SUM(c)`, `a + b` — is a position the statement
            // names only by writing it, which is also how SQL labels such a column, so that
            // is what the delivered file calls it. It is not refused: the definition
            // carrying it was accepted (every column reference inside it resolves, which is
            // what [`resolve_data_query`] judges), and a definition this service persisted
            // has to be deliverable.
            None => Projected::Named(expr.to_string()),
        },
    }
}

/// The most columns one delivered export file carries.
///
/// A BOUND ON THE AMPLIFICATION, not on either factor: the select list is the caller's and
/// a single `*` in it expands to the whole published column set of every table the statement
/// reads, so the names a delivery builds are the PRODUCT of the two — a statement inside
/// `QueryStatement`'s own `@length 1..36000` can write enough wildcards to multiply the
/// catalog into hundreds of thousands of columns, each of which would become a header field
/// and (for Parquet) a schema leaf. This caps that product, so the file one delivery encodes
/// is bounded whatever the statement says. It is set well above the widest schema the
/// catalog publishes, so every wildcard over a real table still delivers that table's whole
/// schema; what it refuses is a projection no export file could carry.
pub const MAX_DELIVERED_COLUMNS: usize = 1024;

/// The columns a delivery of `query` writes into the export file, in the order the
/// statement projects them.
///
/// A data export delivers the ROWS a `DataQuery` selects from the tables the catalog
/// publishes, so the file's columns are that statement's own select list — resolved against
/// the columns in force for the tables it reads, under the table configuration the
/// definition is stored with ([`TableSpec::columns_in_force`], the same filter `GetTable`
/// reports a schema through). A wildcard therefore answers the published columns of those
/// tables, in catalog order, and a named position answers the one name it states.
///
/// `Err` is what [`query_shape`] refuses — the statement is read here exactly as it was read
/// when the definition was accepted, under the same bounds, so a definition this service
/// persisted resolves and nothing else does — and the modelled 400 for a projection past
/// [`MAX_DELIVERED_COLUMNS`], which is refused BEFORE the names are built rather than after.
pub fn delivered_columns(query: &DataQuery) -> Result<Vec<String>, QueryRefusal> {
    let shape = query_shape(&query.query_statement)?;
    // The columns in force for the tables the statement reads, in catalog order, each once:
    // what a wildcard projects, and the set a reference resolves against.
    let mut published: Vec<&'static str> = Vec::new();
    for table in &shape.tables {
        let Some(spec) = table_spec(table) else {
            continue;
        };
        let requested = query
            .table_configurations
            .as_ref()
            .and_then(|configured| configured.get(table));
        // The stored configuration was judged when the definition was accepted, so the
        // properties in force are the ones it carries; a map this read cannot resolve
        // leaves the table's gated columns out rather than failing the delivery.
        let properties = spec
            .effective_properties(requested, QUERY_STATEMENT_FIELD)
            .unwrap_or_default();
        for column in spec.columns_in_force(&properties) {
            if !published.contains(&column.name) {
                published.push(column.name);
            }
        }
    }

    let mut columns: Vec<String> = Vec::new();
    for projected in &shape.projection {
        // How many positions THIS item contributes, decided before any of them is built: a
        // wildcard contributes the whole published set and every other item exactly one. The
        // bound is therefore applied to the product of the select list and the catalog, and
        // the walk stops at the first item that would pass it.
        let contributes = match projected {
            Projected::AllColumns => published.len(),
            Projected::Named(_) | Projected::Reference(_) => 1,
        };
        if columns.len() + contributes > MAX_DELIVERED_COLUMNS {
            return Err(too_many_delivered_columns().into());
        }
        match projected {
            Projected::AllColumns => {
                columns.extend(published.iter().map(|column| (*column).to_string()));
            }
            Projected::Named(name) => columns.push(name.clone()),
            Projected::Reference(parts) => columns.push(
                parts
                    .iter()
                    .find(|part| published.contains(&part.as_str()))
                    .cloned()
                    // A reference the published set does not resolve is carried as the
                    // statement writes it; such a statement is refused before it is ever
                    // stored, so this is the form a reference takes nowhere else.
                    .unwrap_or_else(|| parts.join(".")),
            ),
        }
    }
    Ok(columns)
}

/// The modelled 400 for a select list that projects more columns than an export file
/// carries.
///
/// The sentence states the bound rather than echoing the statement: what the caller has to
/// change is how many positions the select list names, and the statement itself is already
/// what they sent.
fn too_many_delivered_columns() -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!(
            "The query statement projects more than {MAX_DELIVERED_COLUMNS} columns, which is \
             more than a data export file carries."
        ),
        format!("Must project at most {MAX_DELIVERED_COLUMNS} columns."),
    )
}

/// The walk that reads a parsed query's shape.
///
/// It stops at [`MAX_QUERY_RELATIONS`] — the only thing that breaks the walk, which is why
/// [`shape_of`] reads a break as that refusal.
#[derive(Debug, Default)]
struct QueryWalk {
    /// What the walk has read so far.
    shape: QueryShape,
    /// The relations already reported, so de-duplicating them costs
    /// O(relations log relations) and not O(relations^2).
    seen_tables: BTreeSet<String>,
    /// The column references already reported, for the same reason — each under the select
    /// list it was read in, so one text read once in a select list's own scope and once in
    /// the wider scope of the statement is reported under both and neither reading is lost.
    seen_columns: BTreeSet<(String, Option<usize>)>,
}

impl QueryWalk {
    /// Report one column reference, `select_list` being the select list it is written in
    /// (as an index into [`QueryShape::select_lists`]) and `None` anywhere else.
    fn record_reference(&mut self, parts: Vec<String>, select_list: Option<usize>) {
        let reference = ColumnReference {
            written: parts.join("."),
            parts,
            select_list,
        };
        let key = (reference.written.clone(), select_list);
        if self.seen_columns.insert(key) {
            self.shape.columns.push(reference);
        }
    }

    /// Read every SELECT of `body`: the names its select list DEFINES, and the references
    /// the expressions of that same select list make — which those names are not in scope
    /// for, because a select list defines them for the clauses that come AFTER it.
    ///
    /// Recurses with the set expression's own structure, which [`MAX_QUERY_TOKENS`] bounds
    /// the depth of, on the stack [`query_shape`] sized for that bound. Each select list is
    /// read ONCE more than the single walk over the tree, and reports at most one reference
    /// per identifier it encloses, so the extra work and the references kept stay bounded
    /// by the statement's own token count times its nesting depth — both capped before the
    /// parser is entered.
    fn read_select_lists(&mut self, body: &SetExpr) {
        match body {
            SetExpr::Select(select) => {
                let defined: BTreeSet<String> = select
                    .projection
                    .iter()
                    .filter_map(|item| match item {
                        SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.clone()),
                        _ => None,
                    })
                    .collect();
                let list = self.shape.select_lists.len();
                self.shape.select_lists.push(defined.clone());
                for item in &select.projection {
                    let expr = match item {
                        SelectItem::UnnamedExpr(expr)
                        | SelectItem::ExprWithAlias { expr, .. } => expr,
                        // A wildcard names no column, qualified or not.
                        _ => continue,
                    };
                    self.read_select_item(expr, list);
                }
                self.shape.defined.extend(defined);
            }
            SetExpr::Query(query) => self.read_select_lists(&query.body),
            SetExpr::SetOperation { left, right, .. } => {
                self.read_select_lists(left);
                self.read_select_lists(right);
            }
            _ => {}
        }
    }

    /// Report the references ONE select item's expression makes, as written in select list
    /// `list`.
    fn read_select_item(&mut self, expr: &Expr, list: usize) {
        // The expression ITSELF, and then everything inside it: whether a walk started at
        // an expression reports that expression is the visitor's business, and a reference
        // reported twice in one select list is recorded once.
        if let Some(parts) = identifier_parts(expr) {
            self.record_reference(parts, Some(list));
        }
        let mut inside = SelectItemReferences::default();
        let _ = expr.visit(&mut inside);
        for parts in inside.references {
            self.record_reference(parts, Some(list));
        }
    }
}

impl Visitor for QueryWalk {
    /// The walk stops only at the relation bound.
    type Break = ();

    /// Every relation of the statement — `FROM`, each join, and each sub-query's own
    /// `FROM`.
    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<()> {
        // A relation's name is its single identifier's VALUE, so a quoted name
        // (`"COST_AND_USAGE_REPORT"`) resolves like a bare one. A QUALIFIED name
        // (`other_db.COST_AND_USAGE_REPORT`) keeps every part, so it does not resolve
        // to the published table its last part happens to spell: data exports
        // publishes one catalog, not a schema a caller can qualify into.
        let named = match relation.0.as_slice() {
            [single] => single
                .as_ident()
                .map_or_else(|| single.to_string(), |ident| ident.value.clone()),
            _ => relation.to_string(),
        };
        if self.seen_tables.insert(named.clone()) {
            if self.shape.tables.len() >= MAX_QUERY_RELATIONS {
                return ControlFlow::Break(());
            }
            self.shape.tables.push(named);
        }
        ControlFlow::Continue(())
    }

    /// Every column reference of the statement: an identifier used as a value, wherever it
    /// stands — a select item, a predicate, a join condition, a function argument, a
    /// grouping or an ordering. A function's NAME, a type name, an alias and a relation are
    /// not identifier expressions, so none of them is read here.
    ///
    /// Read in the scope of the WHOLE statement, where every name the statement defines is
    /// in scope; the expressions of a select list are read a second time, in the narrower
    /// scope of that list, by [`QueryWalk::read_select_lists`].
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if let Some(parts) = identifier_parts(expr) {
            self.record_reference(parts, None);
        }
        ControlFlow::Continue(())
    }

    /// The names each query of the statement DEFINES for itself, and the references its
    /// select lists make in their own scope.
    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        self.read_select_lists(&query.body);
        ControlFlow::Continue(())
    }
}

/// The identifier parts `expr` is written with, when it is a reference to a name: one part
/// for a bare identifier, every part of a compound one. `None` for an expression that
/// names nothing (a literal, a call, an operator).
fn identifier_parts(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Identifier(ident) => Some(vec![ident.value.clone()]),
        Expr::CompoundIdentifier(idents) => {
            Some(idents.iter().map(|ident| ident.value.clone()).collect())
        }
        _ => None,
    }
}

/// Collects the references ONE expression of a select list makes, so [`QueryWalk`] can
/// report them under that select list's own scope.
///
/// A sub-query written inside the expression is descended into like anything else, which
/// reports its names in this scope as well; the sub-query's own select list is read in its
/// own scope when the outer walk reaches that query, so a name it defines for itself never
/// resolves the expression that defines it.
#[derive(Debug, Default)]
struct SelectItemReferences {
    /// The identifier parts of each reference, in walk order.
    references: Vec<Vec<String>>,
}

impl Visitor for SelectItemReferences {
    /// Nothing stops this walk: it is over one expression, which the statement's own
    /// bounds already cover.
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if let Some(parts) = identifier_parts(expr) {
            self.references.push(parts);
        }
        ControlFlow::Continue(())
    }
}

/// The modelled `TableConfigurations` map: for each table name, that table's properties
/// by name — the shape `DataQuery$TableConfigurations` carries (smithy_query shape
/// `DataQuery`, member `TableConfigurations`).
pub type TableConfigurations = HashMap<String, HashMap<String, String>>;

/// Judge `query` and report what it is STORED with, from ONE read of its statement: the
/// verdict on the definition and the `TableConfigurations` in force for it.
///
/// THE VERDICT. Every table the statement names is one this service publishes AND every
/// column it names is one a table it reads publishes under the table configuration the
/// request states. Both halves are the same published dictionary — "To view the available
/// tables and columns, see the Data Exports table dictionary" (the modelled doc for
/// `CreateExport`'s `Export`) — and the column half is read under the properties IN FORCE,
/// because a table property "[changes] the data and schema of a table" (the modelled doc
/// for `GetTableRequest$TableProperties`): a column the request's own configuration
/// switches off is as unservable as one no table publishes at all. See this module's
/// header for the names that are not column references.
///
/// `require_table_configurations` additionally demands, for each named table that
/// publishes table properties, that `TableConfigurations` carries that table's entry —
/// what `UpdateExport` owes, because it "[updates] an existing data export by overwriting
/// all export parameters. All export parameters must be specified in the UpdateExport
/// request" (smithy_query operation `UpdateExport`), so a per-table property the request
/// leaves unsaid is not kept but unspecified.
///
/// WHAT IS STORED. For every table the query names, and every table the request configures
/// on its own, each table property that table publishes — at the requested value where the
/// request names it and at the published default otherwise. RECORDED AWS behaviour: a
/// reader gets the export's EFFECTIVE table configuration. The recorded `GetExport` for a
/// created export reports the queried table's property set filled in, not only the
/// properties the create happened to name — the same rule `GetTable` already applies to a
/// table request ("Each table property has a default value that it assumes if not
/// specified" — the modelled doc for `GetTableRequest$TableProperties`). The effective
/// configuration is therefore what the definition is STORED with, so every later reader
/// reports it and a definition read back through `GetExport` and handed straight to
/// `UpdateExport` keeps those properties (which is also what satisfies the restatement
/// rule above for such a round trip).
///
/// `Ok(None)` when no table the definition names publishes any property — there is no
/// configuration to report then ("tables are not required to have any TableProperties" —
/// the modelled doc for `CreateExport`).
///
/// ONE pass, because the verdict and the stored configuration are read off the same
/// resolved tables: each table is resolved exactly once, by [`resolve_table`], so the
/// statement is parsed once per submitted definition and no caller can be told that a
/// definition is acceptable and then have a table of it refused.
pub fn resolve_data_query(
    query: &DataQuery,
    require_table_configurations: bool,
) -> Result<Option<TableConfigurations>, QueryRefusal> {
    let shape = query_shape(&query.query_statement)?;
    let requested = query.table_configurations.as_ref();
    let mut effective: TableConfigurations = HashMap::new();
    // The columns the statement may name: every column in force for the tables it reads,
    // under the properties THIS request puts in force.
    let mut published: BTreeSet<&'static str> = BTreeSet::new();
    for table in &shape.tables {
        // This table's name was submitted in the statement, so a refusal about the NAME
        // names that member.
        let (spec, properties) = resolve_table(
            table,
            QUERY_STATEMENT_FIELD,
            requested,
            require_table_configurations,
        )?;
        published.extend(spec.columns_in_force(&properties).map(|column| column.name));
        record_configuration(table, properties, &mut effective);
    }
    for reference in &shape.columns {
        // A reference resolves when one of its parts is a published column, or a name the
        // statement defines that is IN SCOPE where the reference stands: the other parts
        // qualify a relation or index into the column's value, and the catalog publishes
        // neither. A select list's own aliases are out of scope in that list's
        // expressions, so an unpublished name aliased to itself does not resolve against
        // its own definition.
        let resolved = reference.parts.iter().any(|part| {
            published.contains(part.as_str())
                || (shape.defined.contains(part.as_str())
                    && shape.defined_name_in_scope(reference, part))
        });
        if !resolved {
            return Err(invalid_query_column(&reference.written).into());
        }
    }
    // A table the request configures but the statement does not name is still the
    // caller's, so its table and its properties are resolved — and therefore checked and
    // reported — exactly as a queried table's are.
    let queried: BTreeSet<&str> = shape.tables.iter().map(String::as_str).collect();
    let configured_only: BTreeSet<&str> = requested
        .into_iter()
        .flatten()
        .map(|(table, _)| table.as_str())
        .filter(|table| !queried.contains(table))
        .collect();
    for table in configured_only {
        // This table's name was submitted as a KEY of `TableConfigurations` and nowhere
        // else, so a refusal about the NAME names that member.
        let (_, properties) = resolve_table(
            table,
            TABLE_CONFIGURATIONS_FIELD,
            requested,
            require_table_configurations,
        )?;
        record_configuration(table, properties, &mut effective);
    }
    Ok((!effective.is_empty()).then_some(effective))
}

/// Resolve ONE table of a submitted definition: the catalog entry its name names, and the
/// properties in force for it under the configuration the request states.
///
/// The single place a table name becomes a catalog entry and a requested property map
/// becomes an effective one, for every table of every submitted definition — so the
/// refusals are stated once (a table outside the catalog, an entry an update owes and
/// left unsaid, a property the table does not publish, or a value outside the property's
/// published values, each the modelled 400) and the effective properties the stored
/// configuration reports are the very ones the verdict was read from.
///
/// `name_field` is the member path of the submitted definition that CARRIED THIS NAME —
/// the statement for a table the query reads, the configuration map for one the request
/// only configures — because `ValidationExceptionField$Name` is "[t]he field name where
/// the invalid entry was detected" (smithy_query shape `ValidationExceptionField`), and
/// an unpublished name is an invalid entry of whichever member the request wrote it at.
fn resolve_table(
    table: &str,
    name_field: &str,
    requested: Option<&TableConfigurations>,
    require_table_configurations: bool,
) -> Result<(&'static TableSpec, BTreeMap<String, String>), QueryRefusal> {
    let spec = table_spec(table).ok_or_else(|| invalid_query_table(name_field, table))?;
    if require_table_configurations && !spec.properties.is_empty() {
        let configured =
            requested.is_some_and(|configurations| configurations.contains_key(spec.name));
        if !configured {
            return Err(missing_table_configurations(spec.name).into());
        }
    }
    // The refusals a property map earns name the member the SUBMITTED definition carried
    // it at, which on this path is `Export.DataQuery.TableConfigurations` and not the
    // member `GetTable` reads one at.
    let properties = spec.effective_properties(
        requested.and_then(|configured| configured.get(table)),
        TABLE_CONFIGURATIONS_FIELD,
    )?;
    Ok((spec, properties))
}

/// Record `table`'s effective `properties` in the configuration a definition is stored
/// with. A table publishing no property contributes no entry: there is nothing to report
/// for it.
fn record_configuration(
    table: &str,
    properties: BTreeMap<String, String>,
    into: &mut TableConfigurations,
) {
    if properties.is_empty() {
        return;
    }
    into.insert(table.to_string(), properties.into_iter().collect());
}

/// The modelled 400 for a `QueryStatement` this service cannot read as a data query,
/// naming `Export.DataQuery.QueryStatement` and what that member must carry.
///
/// The sentence is built from the request — it echoes the statement, clipped to the
/// message's own modelled bound — because the statement is the only part of the request
/// the caller has to change. RECORDED AWS wording for the same class of refusal, from the
/// unpublished-table rule next door: `Table name <name> is invalid`.
fn invalid_query_statement(statement: &str, requirement: &str) -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!("QueryStatement {} is invalid", clip_for_message(statement)),
        requirement.to_string(),
    )
}

/// The modelled 400 for a `QueryStatement` that is not readable SQL.
fn unreadable_query(statement: &str) -> ValidationException {
    invalid_query_statement(
        statement,
        "Must be an SQL statement over the tables data exports publishes.",
    )
}

/// The modelled 400 for a `QueryStatement` that parses but is not ONE query: a statement
/// of another kind (`DELETE`, `DROP`, `INSERT`, `EXPLAIN`, …), several statements, or no
/// statement at all.
fn not_a_single_query(statement: &str) -> ValidationException {
    invalid_query_statement(
        statement,
        "Must be a single SELECT statement over the tables data exports publishes.",
    )
}

/// The modelled 400 for a query that reads no relation, so it names no table of the
/// catalog for the export to deliver.
fn reads_no_table(statement: &str) -> ValidationException {
    invalid_query_statement(
        statement,
        "Must select from at least one table data exports publishes; call ListTables for them.",
    )
}

/// The modelled 400 for a `QueryStatement` nested deeper than this service reads.
fn too_deeply_nested() -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!(
            "QueryStatement must not be nested more than {MAX_QUERY_NESTING_DEPTH} levels deep"
        ),
        format!("Must not nest more than {MAX_QUERY_NESTING_DEPTH} levels of brackets."),
    )
}

/// The modelled 400 for a `QueryStatement` carrying more tokens than this service reads.
fn too_many_tokens() -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!("QueryStatement must not carry more than {MAX_QUERY_TOKENS} tokens"),
        format!("Must not carry more than {MAX_QUERY_TOKENS} tokens."),
    )
}

/// The modelled 400 for a `QueryStatement` naming more distinct relations than this
/// service resolves.
fn too_many_relations() -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!("QueryStatement must not name more than {MAX_QUERY_RELATIONS} tables"),
        format!(
            "Must name at most {MAX_QUERY_RELATIONS} tables of the data exports catalog."
        ),
    )
}

/// The modelled 400 for a submitted definition naming a table outside the catalog,
/// naming the caller's own member path `field` as the place the invalid entry was
/// detected: the statement for a table the query reads, the configuration map for a table
/// the request only configures.
///
/// The sentence is built from the request and names the offending table, which is the
/// only part of the request the caller has to change; the table it must be one of is what
/// `ListTables` reports. RECORDED AWS wording for that rule: `Table name <name> is
/// invalid`.
fn invalid_query_table(field: &str, table: &str) -> ValidationException {
    field_validation_failed(
        field,
        format!("Table name {} is invalid", clip_for_message(table)),
        "Must name a table data exports publishes; call ListTables for them.".to_string(),
    )
}

/// The modelled 400 for a `QueryStatement` naming a column no table it reads publishes
/// under the table configuration the request states.
///
/// The sentence is built from the request and names the offending column, which is the
/// only part of the request the caller has to change; the columns it must be one of are
/// what `GetTable` reports for the queried table under the same table properties. It is
/// worded as the unpublished-table refusal beside it, whose recorded AWS form is `Table
/// name <name> is invalid`.
fn invalid_query_column(column: &str) -> ValidationException {
    field_validation_failed(
        QUERY_STATEMENT_FIELD,
        format!("Column name {} is invalid", clip_for_message(column)),
        "Must name a column the queried table publishes under the table properties the \
         request states; call GetTable for them."
            .to_string(),
    )
}

/// The modelled 400 for an update whose query names `table` without restating that
/// table's `TableConfigurations`.
fn missing_table_configurations(table: &str) -> ValidationException {
    field_validation_failed(
        TABLE_CONFIGURATIONS_FIELD,
        format!("'TableConfigurations' must be provided when updating a {table} export"),
        format!(
            "Must carry the {table} entry: an update overwrites every export parameter."
        ),
    )
}

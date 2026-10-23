// SPDX-License-Identifier: CC0-1.0

//! The `DataQuery` a submitted `Export` carries: the tables its `QueryStatement` names,
//! and the refusals for a query this service cannot serve.
//!
//! `CreateExport`'s own modelled documentation states what a query may say: "The
//! QueryStatement is an SQL statement. Data Exports only supports a limited subset of the
//! SQL syntax. … To view the available tables and columns, see the Data Exports table
//! dictionary" (smithy_query shape `CreateExportRequest`, member `Export`, quoted in
//! `src/ops/create_export.rs`). The tables that dictionary publishes are exactly
//! [`super::TABLE_CATALOG`] — the catalog `ListTables` lists and `GetTable` describes — so
//! a statement whose `FROM` names anything else selects data this service does not hold,
//! and is refused rather than persisted.
//!
//! The statement is READ, not matched: it is parsed with the `sqlparser` crate and the
//! relations are taken off the parsed statement, so a table named in a join, a sub-query
//! or behind an alias is found exactly as one named in a bare `FROM` is.
//!
//! # What the published subset is, and what that makes of a CTE
//!
//! "Data Exports only supports a limited subset of the SQL syntax": the subset the data
//! query documentation publishes is a SELECT over the catalog's tables (a column list,
//! `FROM`, `WHERE`, and the joins between published tables), so every relation a statement
//! names has to BE a published table. A common table expression is therefore outside the
//! subset: `WITH t AS (…) SELECT * FROM t` names `t`, the catalog publishes no `t`, and the
//! statement is refused like any other naming an unpublished table (a row of
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

use std::collections::{BTreeSet, HashMap};
use std::ops::ControlFlow;

use sqlparser::ast::visit_relations;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use super::{clip_for_message, table_spec, VALIDATION_REASON_FIELD_FAILED};
use crate::ops::types::{
    DataQuery, InternalServerException, ValidationException, ValidationExceptionField,
};

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
const TABLE_CONFIGURATIONS_FIELD: &str = "Export.DataQuery.TableConfigurations";

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

/// Every relation `statement` names, in the order the parse walk finds them, each once.
///
/// `Err` is [`QueryRefusal::Invalid`] — the modelled 400 — for a statement past one of the
/// structural bounds (its bracketing deeper than [`MAX_QUERY_NESTING_DEPTH`] or its token
/// count past [`MAX_QUERY_TOKENS`], both refused BEFORE the parser is entered), for one
/// that is not SQL this service can read — a query it cannot parse names no table, so
/// there is nothing to resolve against the catalog and nothing it could ever deliver — and
/// for one naming more distinct relations than [`MAX_QUERY_RELATIONS`]; and
/// [`QueryRefusal::Internal`] — the modelled 500 — when this process could not run the
/// parse at all.
pub fn query_tables(statement: &str) -> Result<Vec<String>, QueryRefusal> {
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
            .spawn_scoped(scope, || relations_of(statement));
        match spawned {
            // The parse ran: its own answer stands, whether that is the relations or the
            // 400 for a statement it could not read.
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

/// The relations of `statement`, parsed and walked. Called on the sized stack
/// [`query_tables`] sets up; the bounds it states are already applied.
fn relations_of(statement: &str) -> Result<Vec<String>, QueryRefusal> {
    let parsed = Parser::parse_sql(&GenericDialect {}, statement)
        .map_err(|_| unreadable_query(statement))?;
    let mut tables: Vec<String> = Vec::new();
    // Membership is a set lookup rather than a scan of `tables`, so de-duplicating the
    // relations costs O(relations log relations) and not O(relations^2).
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for one in &parsed {
        // The walk reports every relation of the statement — `FROM`, each join, and each
        // sub-query's own `FROM` — so nothing is read out of the statement's text. It
        // stops at the relation bound, and the statement is then refused.
        let walked: ControlFlow<()> = visit_relations(one, |relation| {
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
            if seen.insert(named.clone()) {
                if tables.len() >= MAX_QUERY_RELATIONS {
                    return ControlFlow::Break(());
                }
                tables.push(named);
            }
            ControlFlow::Continue(())
        });
        if walked.is_break() {
            return Err(too_many_relations().into());
        }
    }
    Ok(tables)
}

/// Refuse `query` unless every table it names is one this service publishes.
///
/// `require_table_configurations` additionally demands, for each named table that
/// publishes table properties, that `TableConfigurations` carries that table's entry —
/// what `UpdateExport` owes, because it "[updates] an existing data export by overwriting
/// all export parameters. All export parameters must be specified in the UpdateExport
/// request" (smithy_query operation `UpdateExport`), so a per-table property the request
/// leaves unsaid is not kept but unspecified.
pub fn validate_data_query(
    query: &DataQuery,
    require_table_configurations: bool,
) -> Result<(), QueryRefusal> {
    for table in query_tables(&query.query_statement)? {
        let spec = table_spec(&table).ok_or_else(|| invalid_query_table(&table))?;
        if !require_table_configurations || spec.properties.is_empty() {
            continue;
        }
        let configured = query
            .table_configurations
            .as_ref()
            .is_some_and(|configurations| configurations.contains_key(spec.name));
        if !configured {
            return Err(missing_table_configurations(spec.name).into());
        }
    }
    Ok(())
}

/// The modelled `TableConfigurations` map: for each table name, that table's properties
/// by name — the shape `DataQuery$TableConfigurations` carries (smithy_query shape
/// `DataQuery`, member `TableConfigurations`).
pub type TableConfigurations = HashMap<String, HashMap<String, String>>;

/// The `TableConfigurations` an export definition is STORED with: for every table the
/// query names, and every table the request configures, each table property that table
/// publishes — at the requested value where the request names it and at the published
/// default otherwise.
///
/// RECORDED AWS behaviour: a reader gets the export's EFFECTIVE table configuration. The
/// recorded `GetExport` for a created export reports the queried table's property set
/// filled in, not only the properties the create happened to name — the same rule
/// `GetTable` already applies to a table request ("Each table property has a default
/// value that it assumes if not specified" — the modelled doc for
/// `GetTableRequest$TableProperties`). The effective configuration is therefore what the
/// definition is STORED with, so every later reader reports it and a definition read back
/// through `GetExport` and handed straight to `UpdateExport` keeps those properties
/// (which is also what satisfies [`validate_data_query`]'s restatement rule for such a
/// round trip).
///
/// `None` when no table the definition names publishes any property — there is no
/// configuration to report then ("tables are not required to have any TableProperties" —
/// the modelled doc for `CreateExport`).
///
/// Called AFTER [`validate_data_query`], whose refusals it repeats rather than hides: a
/// table outside the catalog, a property the table does not publish, or a value outside
/// the property's published values is the modelled 400 either way.
pub fn effective_table_configurations(
    query: &DataQuery,
) -> Result<Option<TableConfigurations>, QueryRefusal> {
    let requested = query.table_configurations.as_ref();
    // The tables the statement reads, plus any table the request configures on its own:
    // a configuration for a table the statement does not name is still the caller's, and
    // its table and properties are still checked and reported.
    let mut tables: BTreeSet<String> = query_tables(&query.query_statement)?
        .into_iter()
        .collect();
    for table in requested.into_iter().flatten().map(|(name, _)| name) {
        tables.insert(table.clone());
    }

    let mut effective: TableConfigurations = HashMap::new();
    for table in &tables {
        let spec = table_spec(table).ok_or_else(|| invalid_query_table(table))?;
        let properties =
            spec.effective_properties(requested.and_then(|configured| configured.get(table)))?;
        if properties.is_empty() {
            continue;
        }
        effective.insert(table.clone(), properties.into_iter().collect());
    }
    Ok((!effective.is_empty()).then_some(effective))
}

/// The modelled 400 for a `QueryStatement` that is not readable SQL.
fn unreadable_query(statement: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "QueryStatement {} is invalid",
            clip_for_message(statement)
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: QUERY_STATEMENT_FIELD.to_string(),
            message: "Must be an SQL statement over the tables data exports publishes."
                .to_string(),
        }]),
    }
}

/// The modelled 400 for a `QueryStatement` nested deeper than this service reads.
fn too_deeply_nested() -> ValidationException {
    ValidationException {
        message: format!(
            "QueryStatement must not be nested more than {MAX_QUERY_NESTING_DEPTH} levels deep"
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: QUERY_STATEMENT_FIELD.to_string(),
            message: format!(
                "Must not nest more than {MAX_QUERY_NESTING_DEPTH} levels of brackets."
            ),
        }]),
    }
}

/// The modelled 400 for a `QueryStatement` carrying more tokens than this service reads.
fn too_many_tokens() -> ValidationException {
    ValidationException {
        message: format!("QueryStatement must not carry more than {MAX_QUERY_TOKENS} tokens"),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: QUERY_STATEMENT_FIELD.to_string(),
            message: format!("Must not carry more than {MAX_QUERY_TOKENS} tokens."),
        }]),
    }
}

/// The modelled 400 for a `QueryStatement` naming more distinct relations than this
/// service resolves.
fn too_many_relations() -> ValidationException {
    ValidationException {
        message: format!(
            "QueryStatement must not name more than {MAX_QUERY_RELATIONS} tables"
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: QUERY_STATEMENT_FIELD.to_string(),
            message: format!(
                "Must name at most {MAX_QUERY_RELATIONS} tables of the data exports catalog."
            ),
        }]),
    }
}

/// The modelled 400 for a `QueryStatement` naming a table outside the catalog.
///
/// The sentence is built from the request and names the offending table, which is the
/// only part of the request the caller has to change; the table it must be one of is what
/// `ListTables` reports. RECORDED AWS wording for that rule: `Table name <name> is
/// invalid`.
fn invalid_query_table(table: &str) -> ValidationException {
    ValidationException {
        message: format!("Table name {} is invalid", clip_for_message(table)),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: QUERY_STATEMENT_FIELD.to_string(),
            message: "Must name a table data exports publishes; call ListTables for them."
                .to_string(),
        }]),
    }
}

/// The modelled 400 for an update whose query names `table` without restating that
/// table's `TableConfigurations`.
fn missing_table_configurations(table: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "'TableConfigurations' must be provided when updating a {table} export"
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: TABLE_CONFIGURATIONS_FIELD.to_string(),
            message: format!(
                "Must carry the {table} entry: an update overwrites every export parameter."
            ),
        }]),
    }
}

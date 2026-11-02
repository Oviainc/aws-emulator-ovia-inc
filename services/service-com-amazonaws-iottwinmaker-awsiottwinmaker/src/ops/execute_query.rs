// SPDX-License-Identifier: CC0-1.0

//! @op ExecuteQuery
//! Runs one knowledge-graph query against a workspace and reports the rows it selects —
//! "Run queries to access information from your knowledge graph of entities within
//! individual workspaces".
//!
//! The statement is a real query, so it is really parsed and really evaluated: a
//! tokenizer plus a recursive-descent parser ([`tokenize`], [`Parser`]) turn it into the
//! syntax tree of the service's graph query language, [`Plan`] resolves the variables the
//! `MATCH` clause declares, and [`Matcher`] enumerates the bindings of those variables
//! over the graph [`Graph::build`] derives from the workspace's STORED entities. Nothing
//! is keyed on the text of a particular statement, and no row is synthesised: every row
//! is a projection of entities and relationships this service persisted, so a query over
//! an empty workspace reports no rows and a query run again after a `CreateEntity`
//! reports the new one.
//!
//! The language is IoT TwinMaker's own PartiQL-derived graph dialect
//! (`SELECT … FROM EntityGraph MATCH … WHERE …`), which no crates.io parser implements —
//! `sqlparser` and the SQL dialects it covers have no `MATCH` clause — so the grammar is
//! written out here, the way this repo's other proprietary expression languages are
//! (DynamoDB's `KeyConditionExpression`, X-Ray's filter expressions).
//!
//! The graph:
//!
//! * a NODE is a stored entity of the workspace, labelled with the `componentTypeId` of
//!   each of its components — its top-level components and the composite components held
//!   inside them alike, a composite component being a component of the entity too — so
//!   `(e:com.example.type)` selects the entities carrying a component of that type at any
//!   path;
//! * an EDGE is a stored relationship between two entities of the workspace, of EITHER
//!   kind this service stores one:
//!   * the entity HIERARCHY, which an entity's stored parent records: an edge from the
//!     child to its parent, labelled [`HIERARCHY_RELATIONSHIP_NAME`], so a hierarchy hop
//!     matches without the caller having modelled the hierarchy as a property;
//!   * a property of ANY of the entity's components, at any path, whose value carries a
//!     `relationshipValue` naming a target entity of the same workspace, labelled with
//!     the property's name. A component's properties are the ones it stores itself AND
//!     the ones the component type it instantiates gives it, and a property's value is
//!     the one a read of it reports ([`component_property_values`]), so a relationship a
//!     component only inherits — and one the time-series data plane wrote last — is as
//!     much an edge as one the entity's own create set.
//!
//!   The label is what `-[:isLocationOf]->` matches on, and a relationship whose other
//!   end is not an entity of this workspace (a target outside it, or the root sentinel a
//!   top-level entity names as its parent) is no edge, because the node it would end at
//!   does not exist.
//!
//! What the handler refuses, and with which modelled error:
//!
//! * a statement the grammar does not accept, a `FROM` naming anything but `EntityGraph`,
//!   a variable no `MATCH` declares, an unknown workspace (the model declares no
//!   `ResourceNotFoundException` here, so an unknown workspace is a bad request like it
//!   is for `ListEntities`) and a `nextToken` not minted for this query — a 400
//!   `ValidationException`. The workspace is the SCOPE of the request, so it is resolved
//!   before the statement and the `nextToken` are judged: a query naming a workspace this
//!   service does not hold is refused for that workspace whatever else it carries, the
//!   order every other workspace-scoped operation of this service resolves them in;
//! * a query whose evaluation exceeds [`MATCH_STEP_BUDGET`] candidate bindings or
//!   [`QUERY_DEADLINE`] of wall clock — the modelled 400 `QueryTimeoutException`, which
//!   is what the service answers for a query it will not finish;
//! * a query selecting more than [`MAX_RESULT_ROWS`] rows — the modelled 402
//!   `ServiceQuotaExceededException`, rather than a silently truncated answer.
//!
//! Evaluation is PartiQL's: a path that names no member of the thing it is applied to is
//! MISSING, so a comparison against it is UNKNOWN and the row is not selected, and
//! `AND`/`OR`/`NOT` combine the three values the Kleene way.
//!
//! Reading is all this operation does, and it reads NARROWLY: the handler takes the
//! store's lock only to snapshot the workspace's entities, releases it, and evaluates
//! the query against that snapshot on the blocking pool
//! (`tokio::task::spawn_blocking(..).await`, a real await point). So a query at the top
//! of its budget neither holds the store's lock — which would serialise every other
//! operation of this service — nor occupies the async worker that serves the rest of the
//! emulator, and it writes nothing.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ExecuteQuery::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ExecuteQuery`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json")
//! ```
//!
//! Run queries to access information from your knowledge graph of entities within
//! individual workspaces. The ExecuteQuery action only works with Amazon Web Services Java
//! SDK2. ExecuteQuery will not work with any Amazon Web Services Java SDK version < 2.x.
//!
//! ## Input
//!
//! * `maxResults` (QueryServiceMaxResults, range 1..100) — The maximum number of results to
//!   return at one time. The default is 50.
//! * `nextToken` (NextToken, len 0..17880, pattern .*) — The string that specifies the next
//!   page of results.
//! * `queryStatement` (QueryStatement, len 1..1000, pattern ^[\s\S]+$) **required** — The
//!   query statement.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace.
//!
//! ## Output
//!
//! * `columnDescriptions` (ColumnDescriptions) — A list of ColumnDescription objects.
//! * `nextToken` (NextToken, len 0..17880, pattern .*) — The string that specifies the next
//!   page of results.
//! * `rows` (Rows) — Represents a single row in the query results.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access is denied.
//! * `InternalServerException` (HTTP 500) — An unexpected error has occurred.
//! * `QueryTimeoutException` (HTTP 400) — The query timeout exception.
//! * `ServiceQuotaExceededException` (HTTP 402) — The service quota was exceeded.
//! * `ThrottlingException` (HTTP 429) — The rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — Failed

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime};

use ::aws_smithy_types::{Document, Number};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ExecuteQueryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ExecuteQueryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iottwinmaker#ExecuteQuery")]
#[derive(Debug)]
pub enum ExecuteQueryErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    QueryTimeoutException(QueryTimeoutException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::execute_query::ExecuteQueryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ExecuteQueryError = crate::scaffolded::Scaffolded<ExecuteQueryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ExecuteQueryErrorModeled> for ExecuteQueryError {
    fn from(modeled: ExecuteQueryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ExecuteQuery` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ExecuteQuery<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ExecuteQuery<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ExecuteQuery<Eng>
{
    type Input = ExecuteQueryInput;
    type Output = ExecuteQueryOutput;
    type Error = ExecuteQueryError;

    const NAME: &'static str = "ExecuteQuery";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXECUTE_QUERY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXECUTE_QUERY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every call is recorded, successful or refused: the real service logs
        // control-plane access, and a query is a read of tenant state. The statement
        // itself is caller data, so it is not logged.
        ::tracing::info!(
            operation = "ExecuteQuery",
            workspace_id = %input.workspace_id,
            "serving a request"
        );
        // The Region this request is signed for picks the store that answers
        // it: a workspace created through another Region's endpoint is not in
        // it, so its entities are not what this query walks.
        let region = crate::state::request_region(ctx.parts());

        let invalid = |message: String| {
            ExecuteQueryErrorModeled::ValidationException(ValidationException {
                message: Some(message),
            })
        };

        // The SCOPE is resolved FIRST, before any member of the request is judged, so a
        // query against a workspace this service does not hold is refused for that
        // workspace whatever else the request carries — the order `ListEntities` and
        // every other workspace-scoped operation of this service resolve them in.
        //
        // The store is read under its lock and the lock is RELEASED before anything is
        // evaluated: the walk below is caller-chosen work with a wall-clock bound, and
        // holding the lock for it would serialise every other operation of this service
        // behind one query. What the query reads is the snapshot taken here, which is
        // also what makes the walk a consistent read of one instant — and the statement
        // and the token below are judged OUTSIDE the lock, so resolving the scope first
        // does not widen what the lock is held for.
        //
        // The WHOLE workspace is snapshotted, not its entities alone: naming a
        // `componentTypeId` is what gives a component that type's properties, so the
        // properties a component has and the value each of them reads as are resolved
        // against the workspace's component types the way every other property reader of
        // this service resolves them.
        let workspace: crate::state::Workspace = {
            let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
                ExecuteQueryErrorModeled::InternalServerException(InternalServerException {
                    message: Some("the iottwinmaker store is unavailable".to_string()),
                })
            })?;
            let state = state.lock();
            let state = state.region(&region);
            // The model declares no `ResourceNotFoundException` for this operation, so an
            // unknown workspace is refused the way `ListEntities` refuses one.
            let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
                return Err(invalid(crate::state::workspace_not_found_message(
                    &input.workspace_id,
                ))
                .into());
            };
            workspace.clone()
        };

        // Parsed and planned once the scope holds, and still before the walk: a
        // statement that is not a query needs no state to be refused.
        let query = parse_query(&input.query_statement)
            .map_err(|reason| invalid(format!("{INVALID_STATEMENT}: {reason}")))?;
        let plan = Plan::build(&query)
            .map_err(|reason| invalid(format!("{INVALID_STATEMENT}: {reason}")))?;
        // The columns are a property of the statement, not of the rows it matches, so
        // they are known here — and are reported whether or not it selected a row.
        let column_descriptions = plan.column_descriptions();

        // A `nextToken` carries the row offset the page resumes at, and is bound to the
        // Region, the workspace and the statement it was minted for: the row set is
        // recomputed from state on every call — out of the store of the Region the request
        // is signed for, each holding its own independent workspaces — so resuming one
        // query's paging inside another would report rows from a listing the caller never
        // asked for. Only the offset's SPELLING can be judged here — whether it names a
        // row this query selects is judged against the row set itself, below.
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => Some(
                decode_next_token(token, &region, &input.workspace_id, &input.query_statement)
                    .ok_or_else(|| invalid(crate::state::invalid_next_token_message()))?,
            ),
        };
        // Where a request names no token the walk starts at the first row.
        let skip = resume_at.unwrap_or(0);

        // "The maximum number of results to return at one time. The default is 50" — the
        // `maxResults` member's own documentation. The modelled range is 1..100, so the
        // boundary validator has already refused anything outside it; the clamp keeps
        // this handler's own bound local rather than depending on that.
        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, MAX_MAX_RESULTS);

        // The match is bounded (see `MATCH_STEP_BUDGET` and `QUERY_DEADLINE`) but
        // CPU-bound and caller-chosen, so it runs on the blocking pool rather than on the
        // async worker: `spawn_blocking(..).await` is a real await point, so a query at
        // the top of its budget cannot starve the worker that serves health checks and
        // every other request, and a cancellation or a timeout has somewhere to land.
        let walked = ::tokio::task::spawn_blocking(move || {
            let graph = Graph::build(&workspace);
            let bindings = Matcher::new(&graph, &query, &plan).run()?;
            let rows: Vec<Row> = bindings
                .iter()
                .skip(skip)
                .take(page_size)
                .map(|binding| plan.project(&graph, binding))
                .collect();
            Ok::<(Vec<Row>, usize), QueryFault>((rows, bindings.len()))
        })
        .await;

        let (rows, selected) = match walked {
            Ok(Ok(answer)) => answer,
            Ok(Err(fault)) => {
                return Err(match fault {
                    QueryFault::Timeout(message) => {
                        ExecuteQueryErrorModeled::QueryTimeoutException(QueryTimeoutException {
                            message: Some(message),
                        })
                    }
                    QueryFault::TooManyRows(message) => {
                        ExecuteQueryErrorModeled::ServiceQuotaExceededException(
                            ServiceQuotaExceededException {
                                message: Some(message),
                            },
                        )
                    }
                }
                .into())
            }
            // A panic or a cancellation on the pool says nothing about the query, so it
            // is reported as the server fault it is rather than as an empty answer.
            Err(error) => {
                ::tracing::error!(
                    operation = Self::NAME,
                    error = %error,
                    "the query did not complete"
                );
                return Err(ExecuteQueryErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some("the query did not complete".to_string()),
                    },
                )
                .into());
            }
        };

        // The row count is only known once the row set is recomputed, so this is where
        // an offset past the rows this query selects is refused: a page is only minted
        // at a row that exists, so such an offset is one this operation never wrote.
        if resume_at.is_some_and(|resume_at| {
            !crate::state::ordinal_resumes_a_selected_row(resume_at, selected)
        }) {
            return Err(invalid(crate::state::invalid_next_token_message()).into());
        }

        let served_through = skip.saturating_add(rows.len());
        let next_token = (served_through < selected).then(|| {
            encode_next_token(
                served_through,
                &region,
                &input.workspace_id,
                &input.query_statement,
            )
        });

        Ok(ExecuteQueryOutput {
            column_descriptions: Some(column_descriptions),
            rows: Some(rows),
            next_token,
        })
    }
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// The page size served when a request names no `maxResults`, and the largest one it
/// can ask for. Both come from the member's own documentation ("The maximum number of
/// results to return at one time. The default is 50") and its modelled range (1..100).
const DEFAULT_MAX_RESULTS: usize = 50;
/// See [`DEFAULT_MAX_RESULTS`].
const MAX_MAX_RESULTS: usize = 100;

/// The most rows one query may select before it is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// The real service's query limits are account-level and adjustable, so no fixed number
/// is AWS's; this is this emulator's own declared ceiling, so a cross join over a large
/// workspace cannot materialise an unbounded row set in memory. A refusal is served
/// rather than a truncated answer, so no caller is handed rows that look complete.
const MAX_RESULT_ROWS: usize = 10_000;

/// The most candidate bindings [`Matcher`] explores, and the most wall clock it spends,
/// before the query is refused with the modelled `QueryTimeoutException`.
///
/// A pattern's cost is exponential in its number of hops, and both the pattern and the
/// workspace are caller-chosen, so the walk is bounded by work and by time rather than
/// trusted to finish.
const MATCH_STEP_BUDGET: u64 = 2_000_000;
/// See [`MATCH_STEP_BUDGET`].
const QUERY_DEADLINE: Duration = Duration::from_millis(2_000);
/// How often the walk looks at the clock: often enough to honour [`QUERY_DEADLINE`],
/// rarely enough that the syscall is not the cost of the walk.
const STEPS_PER_CLOCK_CHECK: u64 = 4_096;

/// The most tokens one statement may hold. `QueryStatement` is modelled `@length
/// 1..1000`, so this is a ceiling the statement's own size already implies; it keeps the
/// token vector's bound local to the tokenizer.
const MAX_STATEMENT_TOKENS: usize = 1_000;

/// How deeply a `WHERE` expression may nest, counting both a parenthesised group and a
/// `NOT`. The nesting is caller-chosen, so without a cap the number of parser stack
/// frames would be too (and a stack overflow aborts the process rather than failing one
/// request).
const MAX_EXPRESSION_DEPTH: usize = 32;

/// The prefix every `ValidationException` for an unusable statement carries, so the
/// refusals that are about the statement read alike: the service names the member it
/// could not use, which is the `queryStatement` the request carried.
const INVALID_STATEMENT: &str = "Invalid QueryStatement";

/// The one source this language's `FROM` clause can name.
const ENTITY_GRAPH: &str = "EntityGraph";

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// One lexical token of a query statement.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A bare word: a keyword, a variable, a label, or a dotted property path.
    Word(String),
    /// A quoted string literal.
    Text(String),
    /// A numeric literal.
    Number(f64),
    Open,
    Close,
    OpenBracket,
    CloseBracket,
    Colon,
    Comma,
    Dash,
    Arrow,
    LeftArrow,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

/// Which characters make up a bare word. `.` is one of them, so a property path
/// (`e.entityName`) and a dotted component type id (`com.example.type`) each arrive as a
/// single word and are split where the grammar needs them split.
fn is_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '.'
}

/// Split a query statement into tokens.
fn tokenize(statement: &str) -> Result<Vec<Token>, String> {
    let characters: Vec<char> = statement.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0usize;
    while at < characters.len() {
        if tokens.len() >= MAX_STATEMENT_TOKENS {
            return Err(format!(
                "a statement may hold at most {MAX_STATEMENT_TOKENS} tokens"
            ));
        }
        let character = characters[at];
        match character {
            c if c.is_whitespace() => at += 1,
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            '[' => {
                tokens.push(Token::OpenBracket);
                at += 1;
            }
            ']' => {
                tokens.push(Token::CloseBracket);
                at += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                at += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                at += 1;
            }
            '=' => {
                tokens.push(Token::Equal);
                at += 1;
            }
            '!' => {
                if characters.get(at + 1) == Some(&'=') {
                    tokens.push(Token::NotEqual);
                    at += 2;
                } else {
                    // The token the lexer was matching is the `!` AND the
                    // character that failed to complete it, which is as much of
                    // the statement as it had consumed when the match failed.
                    return Err(invalid_token(&characters, at, 2));
                }
            }
            '<' => {
                let (token, width) = match characters.get(at + 1) {
                    Some('=') => (Token::LessOrEqual, 2),
                    Some('>') => (Token::NotEqual, 2),
                    Some('-') => (Token::LeftArrow, 2),
                    _ => (Token::Less, 1),
                };
                tokens.push(token);
                at += width;
            }
            '>' => {
                let (token, width) = match characters.get(at + 1) {
                    Some('=') => (Token::GreaterOrEqual, 2),
                    _ => (Token::Greater, 1),
                };
                tokens.push(token);
                at += width;
            }
            '-' => {
                let (token, width) = match characters.get(at + 1) {
                    Some('>') => (Token::Arrow, 2),
                    _ => (Token::Dash, 1),
                };
                tokens.push(token);
                at += width;
            }
            '\'' | '"' => {
                // A quoted literal. The quote character is doubled to be included in
                // the value, the way SQL and PartiQL escape one.
                let quote = character;
                let mut text = String::new();
                at += 1;
                loop {
                    match characters.get(at) {
                        None => {
                            return Err(format!(
                                "a {quote} quoted literal is not closed before the end of the \
                                 statement"
                            ))
                        }
                        Some(&found) if found == quote => {
                            if characters.get(at + 1) == Some(&quote) {
                                text.push(quote);
                                at += 2;
                            } else {
                                at += 1;
                                break;
                            }
                        }
                        Some(&found) => {
                            text.push(found);
                            at += 1;
                        }
                    }
                }
                tokens.push(Token::Text(text));
            }
            c if c.is_ascii_digit() => {
                let start = at;
                while characters
                    .get(at)
                    .is_some_and(|found| found.is_ascii_digit() || *found == '.')
                {
                    at += 1;
                }
                let text: String = characters[start..at].iter().collect();
                let value = text
                    .parse::<f64>()
                    .map_err(|_| format!("{text} is not a number"))?;
                tokens.push(Token::Number(value));
            }
            c if is_word_char(c) => {
                let start = at;
                while characters.get(at).is_some_and(|found| is_word_char(*found)) {
                    at += 1;
                }
                tokens.push(Token::Word(characters[start..at].iter().collect()));
            }
            _ => return Err(invalid_token(&characters, at, 1)),
        }
    }
    if tokens.is_empty() {
        return Err("a statement must hold a query".to_string());
    }
    Ok(tokens)
}

/// Why a run of characters the lexer cannot recognise as a token refuses the
/// statement, as the reason the `ValidationException`'s message carries after
/// [`INVALID_STATEMENT`].
///
/// Built from the request: it names WHERE in the statement the lexer stopped —
/// as a 1-based line and column, which is how a statement's own text is
/// addressed, so an editor or a console can point at the character — and quotes
/// the text it had consumed for the token it could not finish (`at` is the index
/// of that token's first character and `width` how many characters the attempt
/// spanned, clamped to the end of the statement).
fn invalid_token(characters: &[char], at: usize, width: usize) -> String {
    let (line, column) = line_and_column(characters, at);
    let end = at.saturating_add(width).min(characters.len());
    let token: String = characters[at.min(end)..end].iter().collect();
    format!(
        "at line {line}, column {column}: invalid token, token recognition error at: '{token}'"
    )
}

/// Where the character at index `at` sits in the statement: its 1-BASED line and
/// column, counting a line per `\n`. A statement is one line unless the caller
/// wrote it across several, and the first character of a line is column 1.
fn line_and_column(characters: &[char], at: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for character in &characters[..at.min(characters.len())] {
        if *character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

// ---------------------------------------------------------------------------
// Syntax tree
// ---------------------------------------------------------------------------

/// One parsed statement.
#[derive(Debug)]
struct Query {
    select: Vec<SelectItem>,
    patterns: Vec<PathPattern>,
    filter: Option<Expr>,
}

/// One item of the `SELECT` list: a variable, or a member of what a variable is bound to.
#[derive(Debug)]
struct SelectItem {
    /// The item as it was written, which is what names the reported column.
    text: String,
    variable: String,
    field: Option<String>,
}

/// One node of a `MATCH` pattern: `(variable:label)`, either part optional.
#[derive(Debug)]
struct NodePattern {
    variable: Option<String>,
    label: Option<String>,
}

/// One relationship of a `MATCH` pattern: `-[variable:label]->`, either part optional.
#[derive(Debug)]
struct EdgePattern {
    variable: Option<String>,
    label: Option<String>,
    direction: Direction,
}

/// Which way a relationship pattern points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// `-[..]->`: the left node is the relationship's source.
    Outgoing,
    /// `<-[..]-`: the left node is its target.
    Incoming,
    /// `-[..]-`: either way round.
    Either,
}

/// One comma-separated pattern of a `MATCH` clause: a node, then a hop per relationship.
#[derive(Debug)]
struct PathPattern {
    start: NodePattern,
    hops: Vec<(EdgePattern, NodePattern)>,
}

/// One `WHERE` expression.
#[derive(Debug)]
enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Compare {
        left: Operand,
        operator: Comparison,
        right: Operand,
    },
    /// `operand [NOT] LIKE 'pattern'`, the pattern already compiled.
    Like {
        value: Operand,
        pattern: ::regex::Regex,
        negated: bool,
    },
    /// `operand IS [NOT] NULL`, which is how a statement asks whether a path is MISSING.
    IsNull {
        value: Operand,
        negated: bool,
    },
}

/// One side of a comparison.
#[derive(Debug)]
enum Operand {
    /// A member of what a variable is bound to.
    Path {
        variable: String,
        field: String,
    },
    Literal(Value),
}

/// The comparison operators the language spells `=`, `!=`/`<>`, `<`, `<=`, `>`, `>=`.
#[derive(Debug, Clone, Copy)]
enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

/// A comparable value: what a literal holds, and what a path evaluates to when the
/// member it names is a scalar.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Text(String),
    Number(f64),
    Boolean(bool),
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// The words the grammar reserves, so none of them is mistaken for a variable or a
/// label.
const RESERVED_WORDS: [&str; 12] = [
    "SELECT", "FROM", "MATCH", "WHERE", "AND", "OR", "NOT", "LIKE", "IS", "NULL", "TRUE", "FALSE",
];

fn is_reserved(word: &str) -> bool {
    RESERVED_WORDS
        .iter()
        .any(|reserved| word.eq_ignore_ascii_case(reserved))
}

/// Whether `text` is a variable name: a letter or `_`, then letters, digits and `_`.
fn is_variable_name(text: &str) -> bool {
    text.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_')
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Parse one statement.
fn parse_query(statement: &str) -> Result<Query, String> {
    let tokens = tokenize(statement)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        depth: 0,
    };
    let query = parser.query()?;
    if parser.at != tokens.len() {
        return Err(format!(
            "unexpected input after the query, at token {}",
            parser.at + 1
        ));
    }
    Ok(query)
}

/// Recursive-descent parser over one statement's tokens.
///
/// The grammar it accepts:
///
/// ```text
/// query        := SELECT select_list FROM EntityGraph [MATCH pattern_list] [WHERE expr]
/// select_list  := select_item (',' select_item)*
/// select_item  := variable ['.' field]
/// pattern_list := path (',' path)*
/// path         := node (relationship node)*
/// node         := '(' [variable] [':' label] ')'
/// relationship := '-' ['[' [variable] [':' label] ']'] ('->' | '-')
///               | '<-' ['[' [variable] [':' label] ']'] '-'
///               | '->'
/// expr         := and_expr (OR and_expr)*
/// and_expr     := not_expr (AND not_expr)*
/// not_expr     := NOT not_expr | '(' expr ')' | predicate
/// predicate    := operand (('=' | '!=' | '<' | '<=' | '>' | '>=') operand
///                          | [NOT] LIKE string | IS [NOT] NULL)
/// operand      := variable '.' field | string | number | TRUE | FALSE
/// ```
#[derive(Debug)]
struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn bump(&mut self) -> Result<&Token, String> {
        let token = self
            .tokens
            .get(self.at)
            .ok_or_else(|| "the query ends before it is complete".to_string())?;
        self.at += 1;
        Ok(token)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == Some(expected) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token, what: &str) -> Result<(), String> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(format!("expected {what}, found {}", self.describe()))
        }
    }

    /// The next token as text, for an error message.
    fn describe(&self) -> String {
        match self.peek() {
            None => "the end of the statement".to_string(),
            Some(Token::Word(word)) => format!("{word:?}"),
            Some(Token::Text(text)) => format!("the literal {text:?}"),
            Some(Token::Number(number)) => format!("the number {number}"),
            Some(other) => format!("{other:?}"),
        }
    }

    /// Consume the next token when it is the keyword `word`.
    fn eat_keyword(&mut self, word: &str) -> bool {
        match self.peek() {
            Some(Token::Word(found)) if found.eq_ignore_ascii_case(word) => {
                self.at += 1;
                true
            }
            _ => false,
        }
    }

    fn expect_keyword(&mut self, word: &str) -> Result<(), String> {
        if self.eat_keyword(word) {
            Ok(())
        } else {
            Err(format!("expected {word}, found {}", self.describe()))
        }
    }

    /// Consume the next token when it is a word that is not a reserved one.
    fn eat_name(&mut self) -> Option<String> {
        match self.peek() {
            Some(Token::Word(found)) if !is_reserved(found) => {
                let found = found.clone();
                self.at += 1;
                Some(found)
            }
            _ => None,
        }
    }

    fn query(&mut self) -> Result<Query, String> {
        self.expect_keyword("SELECT")?;
        let select = self.select_list()?;
        self.expect_keyword("FROM")?;
        let source = self
            .eat_name()
            .ok_or_else(|| format!("expected {ENTITY_GRAPH} after FROM"))?;
        if !source.eq_ignore_ascii_case(ENTITY_GRAPH) {
            return Err(format!(
                "FROM accepts only {ENTITY_GRAPH}, the graph of a workspace's entities, \
                 found {source:?}"
            ));
        }
        let patterns = if self.eat_keyword("MATCH") {
            self.pattern_list()?
        } else {
            Vec::new()
        };
        let filter = if self.eat_keyword("WHERE") {
            Some(self.expression()?)
        } else {
            None
        };
        Ok(Query {
            select,
            patterns,
            filter,
        })
    }

    fn select_list(&mut self) -> Result<Vec<SelectItem>, String> {
        let mut items = vec![self.select_item()?];
        while self.eat(&Token::Comma) {
            items.push(self.select_item()?);
        }
        Ok(items)
    }

    fn select_item(&mut self) -> Result<SelectItem, String> {
        let text = self
            .eat_name()
            .ok_or_else(|| format!("expected a selected variable, found {}", self.describe()))?;
        let (variable, field) = split_path(&text)?;
        Ok(SelectItem {
            text,
            variable,
            field,
        })
    }

    fn pattern_list(&mut self) -> Result<Vec<PathPattern>, String> {
        let mut patterns = vec![self.path()?];
        while self.eat(&Token::Comma) {
            patterns.push(self.path()?);
        }
        Ok(patterns)
    }

    fn path(&mut self) -> Result<PathPattern, String> {
        let start = self.node_pattern()?;
        let mut hops = Vec::new();
        while matches!(
            self.peek(),
            Some(Token::Dash) | Some(Token::Arrow) | Some(Token::LeftArrow)
        ) {
            let edge = self.relationship_pattern()?;
            let node = self.node_pattern()?;
            hops.push((edge, node));
        }
        Ok(PathPattern { start, hops })
    }

    fn node_pattern(&mut self) -> Result<NodePattern, String> {
        self.expect(&Token::Open, "a node pattern, which opens with (")?;
        let variable = self.pattern_variable()?;
        let label = self.pattern_label()?;
        self.expect(&Token::Close, "the ) closing a node pattern")?;
        Ok(NodePattern { variable, label })
    }

    fn relationship_pattern(&mut self) -> Result<EdgePattern, String> {
        // `->` on its own is an anonymous outgoing relationship; otherwise the pattern
        // opens with `-` or `<-` and closes with `->`, `-`, matching the direction.
        let opens_incoming = match self.bump()? {
            Token::Arrow => {
                return Ok(EdgePattern {
                    variable: None,
                    label: None,
                    direction: Direction::Outgoing,
                })
            }
            Token::Dash => false,
            Token::LeftArrow => true,
            other => {
                return Err(format!(
                    "expected a relationship pattern, found {other:?}"
                ))
            }
        };
        let (variable, label) = if self.eat(&Token::OpenBracket) {
            let variable = self.pattern_variable()?;
            let label = self.pattern_label()?;
            self.expect(
                &Token::CloseBracket,
                "the ] closing a relationship pattern",
            )?;
            (variable, label)
        } else {
            (None, None)
        };
        let direction = match (opens_incoming, self.bump()?) {
            (false, Token::Arrow) => Direction::Outgoing,
            (false, Token::Dash) => Direction::Either,
            (true, Token::Dash) => Direction::Incoming,
            (_, other) => {
                return Err(format!(
                    "a relationship pattern ends in -> or -, found {other:?}"
                ))
            }
        };
        Ok(EdgePattern {
            variable,
            label,
            direction,
        })
    }

    /// The optional variable of a node or relationship pattern.
    fn pattern_variable(&mut self) -> Result<Option<String>, String> {
        match self.peek() {
            Some(Token::Word(found)) if !is_reserved(found) => {
                let found = found.clone();
                if !is_variable_name(&found) {
                    return Err(format!(
                        "{found:?} is not a variable name: a variable is a letter or _ followed \
                         by letters, digits and _"
                    ));
                }
                self.at += 1;
                Ok(Some(found))
            }
            _ => Ok(None),
        }
    }

    /// The optional `:label` of a node or relationship pattern: a component type id on a
    /// node, a relationship name on an edge.
    fn pattern_label(&mut self) -> Result<Option<String>, String> {
        if !self.eat(&Token::Colon) {
            return Ok(None);
        }
        let label = self
            .eat_name()
            .ok_or_else(|| format!("expected a label after :, found {}", self.describe()))?;
        Ok(Some(label))
    }

    fn expression(&mut self) -> Result<Expr, String> {
        let mut left = self.conjunction()?;
        while self.eat_keyword("OR") {
            let right = self.conjunction()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn conjunction(&mut self) -> Result<Expr, String> {
        let mut left = self.negation()?;
        while self.eat_keyword("AND") {
            let right = self.negation()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn negation(&mut self) -> Result<Expr, String> {
        if self.depth >= MAX_EXPRESSION_DEPTH {
            return Err(format!(
                "a WHERE expression may nest at most {MAX_EXPRESSION_DEPTH} deep"
            ));
        }
        if self.eat_keyword("NOT") {
            self.depth += 1;
            let inner = self.negation()?;
            self.depth -= 1;
            return Ok(Expr::Not(Box::new(inner)));
        }
        if self.peek() == Some(&Token::Open) {
            self.at += 1;
            self.depth += 1;
            let inner = self.expression()?;
            self.depth -= 1;
            self.expect(&Token::Close, "the ) closing a grouped condition")?;
            return Ok(inner);
        }
        self.predicate()
    }

    fn predicate(&mut self) -> Result<Expr, String> {
        let left = self.operand()?;
        if self.eat_keyword("IS") {
            let negated = self.eat_keyword("NOT");
            self.expect_keyword("NULL")?;
            return Ok(Expr::IsNull {
                value: left,
                negated,
            });
        }
        if self.eat_keyword("NOT") {
            self.expect_keyword("LIKE")?;
            return self.like(left, true);
        }
        if self.eat_keyword("LIKE") {
            return self.like(left, false);
        }
        let operator = match self.bump()? {
            Token::Equal => Comparison::Equal,
            Token::NotEqual => Comparison::NotEqual,
            Token::Less => Comparison::Less,
            Token::LessOrEqual => Comparison::LessOrEqual,
            Token::Greater => Comparison::Greater,
            Token::GreaterOrEqual => Comparison::GreaterOrEqual,
            other => {
                return Err(format!(
                    "expected a comparison operator, LIKE or IS NULL, found {other:?}"
                ))
            }
        };
        let right = self.operand()?;
        Ok(Expr::Compare {
            left,
            operator,
            right,
        })
    }

    fn like(&mut self, value: Operand, negated: bool) -> Result<Expr, String> {
        let pattern = match self.bump()? {
            Token::Text(text) => text.clone(),
            other => {
                return Err(format!(
                    "LIKE compares against a quoted pattern, found {other:?}"
                ))
            }
        };
        Ok(Expr::Like {
            value,
            pattern: like_matcher(&pattern)?,
            negated,
        })
    }

    fn operand(&mut self) -> Result<Operand, String> {
        let negate = self.eat(&Token::Dash);
        match self.bump()? {
            Token::Text(text) if !negate => Ok(Operand::Literal(Value::Text(text.clone()))),
            Token::Number(number) => Ok(Operand::Literal(Value::Number(if negate {
                -*number
            } else {
                *number
            }))),
            Token::Word(word) if !negate && word.eq_ignore_ascii_case("TRUE") => {
                Ok(Operand::Literal(Value::Boolean(true)))
            }
            Token::Word(word) if !negate && word.eq_ignore_ascii_case("FALSE") => {
                Ok(Operand::Literal(Value::Boolean(false)))
            }
            Token::Word(word) if !negate && !is_reserved(word) => {
                let (variable, field) = split_path(word)?;
                let field = field.ok_or_else(|| {
                    format!(
                        "a condition reads a member of a variable, as in {word}.entityName, not \
                         the variable {word} itself"
                    )
                })?;
                Ok(Operand::Path { variable, field })
            }
            other => Err(format!(
                "expected a property path or a literal, found {other:?}"
            )),
        }
    }
}

/// Split a written path into its variable and the member it names.
fn split_path(text: &str) -> Result<(String, Option<String>), String> {
    let mut parts = text.split('.');
    let variable = parts.next().unwrap_or_default().to_string();
    let field = parts.next().map(str::to_string);
    if parts.next().is_some() {
        return Err(format!(
            "{text:?} is not a supported path: a path names one member of a variable, as in \
             e.entityName"
        ));
    }
    if !is_variable_name(&variable) {
        return Err(format!(
            "{variable:?} is not a variable name: a variable is a letter or _ followed by \
             letters, digits and _"
        ));
    }
    if field.as_ref().is_some_and(|field| field.is_empty()) {
        return Err(format!("{text:?} names no member after its ."));
    }
    Ok((variable, field))
}

/// Compile a SQL `LIKE` pattern: `%` stands for any run of characters and `_` for
/// exactly one; every other character matches itself.
fn like_matcher(pattern: &str) -> Result<::regex::Regex, String> {
    let mut expression = String::from("^");
    for character in pattern.chars() {
        match character {
            '%' => expression.push_str("(?s:.)*"),
            '_' => expression.push_str("(?s:.)"),
            other => expression.push_str(&::regex::escape(other.encode_utf8(&mut [0u8; 4]))),
        }
    }
    expression.push('$');
    ::regex::Regex::new(&expression).map_err(|error| format!("{pattern:?} is not a pattern: {error}"))
}

// ---------------------------------------------------------------------------
// Plan: the variables a statement declares, and the columns it reports
// ---------------------------------------------------------------------------

/// What a variable is bound to, which is what decides the reported column type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Node,
    Edge,
}

impl VarKind {
    /// The modelled `ColumnType` for a column that reports this whole binding
    /// (`ColumnType` is `NODE | EDGE | VALUE`).
    fn column_type(self) -> &'static str {
        match self {
            Self::Node => "NODE",
            Self::Edge => "EDGE",
        }
    }
}

/// One declared variable: where its binding lives in a row, and what it binds to.
#[derive(Debug, Clone, Copy)]
struct Variable {
    slot: usize,
    kind: VarKind,
}

/// One reported column: where it reads from and what it projects.
#[derive(Debug)]
struct Projection {
    name: String,
    slot: usize,
    kind: VarKind,
    field: Option<String>,
}

/// A statement's variables resolved against its `MATCH` clause.
#[derive(Debug)]
struct Plan {
    variables: BTreeMap<String, Variable>,
    projections: Vec<Projection>,
}

impl Plan {
    fn build(query: &Query) -> Result<Self, String> {
        let mut variables: BTreeMap<String, Variable> = BTreeMap::new();
        let mut declare = |name: &str, kind: VarKind| -> Result<(), String> {
            let slot = variables.len();
            match variables.get(name) {
                Some(existing) if existing.kind != kind => Err(format!(
                    "the variable {name:?} is matched both as a node and as a relationship"
                )),
                Some(_) => Ok(()),
                None => {
                    variables.insert(name.to_string(), Variable { slot, kind });
                    Ok(())
                }
            }
        };
        for pattern in &query.patterns {
            if let Some(name) = &pattern.start.variable {
                declare(name, VarKind::Node)?;
            }
            for (edge, node) in &pattern.hops {
                if let Some(name) = &edge.variable {
                    declare(name, VarKind::Edge)?;
                }
                if let Some(name) = &node.variable {
                    declare(name, VarKind::Node)?;
                }
            }
        }

        let mut projections = Vec::new();
        for item in &query.select {
            let variable = variables.get(&item.variable).ok_or_else(|| {
                format!(
                    "the variable {:?} is selected but no MATCH pattern declares it",
                    item.variable
                )
            })?;
            projections.push(Projection {
                name: item.text.clone(),
                slot: variable.slot,
                kind: variable.kind,
                field: item.field.clone(),
            });
        }
        let plan = Self {
            variables,
            projections,
        };
        if let Some(filter) = &query.filter {
            plan.check_filter(filter)?;
        }
        Ok(plan)
    }

    /// Refuse a `WHERE` clause that reads a variable no pattern declares, rather than
    /// silently treating it as MISSING.
    fn check_filter(&self, filter: &Expr) -> Result<(), String> {
        let mut operands: Vec<&Operand> = Vec::new();
        let mut pending: Vec<&Expr> = vec![filter];
        while let Some(expr) = pending.pop() {
            match expr {
                Expr::Or(left, right) | Expr::And(left, right) => {
                    pending.push(left);
                    pending.push(right);
                }
                Expr::Not(inner) => pending.push(inner),
                Expr::Compare { left, right, .. } => {
                    operands.push(left);
                    operands.push(right);
                }
                Expr::Like { value, .. } | Expr::IsNull { value, .. } => operands.push(value),
            }
        }
        for operand in operands {
            if let Operand::Path { variable, .. } = operand {
                if !self.variables.contains_key(variable) {
                    return Err(format!(
                        "the variable {variable:?} is used in WHERE but no MATCH pattern \
                         declares it"
                    ));
                }
            }
        }
        Ok(())
    }

    /// The columns this statement reports, in the order it selects them.
    fn column_descriptions(&self) -> Vec<ColumnDescription> {
        self.projections
            .iter()
            .map(|projection| ColumnDescription {
                name: Some(projection.name.clone()),
                // A column that projects a whole binding reports what that binding is
                // (`NODE`/`EDGE`); one that projects a member of it reports `VALUE`.
                r#type: Some(
                    match projection.field {
                        None => projection.kind.column_type(),
                        Some(_) => "VALUE",
                    }
                    .to_string(),
                ),
            })
            .collect()
    }

    /// One row: each selected column, in order.
    fn project(&self, graph: &Graph<'_>, binding: &Binding) -> Row {
        let row_data = self
            .projections
            .iter()
            .map(|projection| {
                let Some(bound) = binding[projection.slot] else {
                    return Document::Null;
                };
                match &projection.field {
                    None => graph.document(bound),
                    Some(field) => graph.member(bound, field).unwrap_or(Document::Null),
                }
            })
            .collect();
        Row {
            row_data: Some(row_data),
        }
    }
}

// ---------------------------------------------------------------------------
// The graph a workspace's stored entities form
// ---------------------------------------------------------------------------

/// The name of the relationship the SERVICE itself keeps between an entity and the
/// parent entity it was created or moved under — the label `-[:isChildOf]->` matches.
///
/// The hierarchy is service state in its own right: a create or an update sets the
/// entity's parent and every read of the entity reports it, so it is a relationship the
/// knowledge graph holds whether or not any component property models one.
const HIERARCHY_RELATIONSHIP_NAME: &str = "isChildOf";

/// One relationship of the graph: either the stored parent of an entity, or a property
/// of one of its components whose value points at another entity of the same workspace.
#[derive(Debug)]
struct GraphEdge<'a> {
    source: usize,
    target: usize,
    /// The relationship's own name, which is the label `-[:name]->` matches: the name of
    /// the property holding it, or [`HIERARCHY_RELATIONSHIP_NAME`] for the hierarchy.
    ///
    /// OWNED, because a property a component only INHERITS from the component type it
    /// instantiates is named by that type rather than by anything the entity stores.
    relationship_name: String,
    /// The component holding the relationship, and `None` for the hierarchy, which is
    /// held between the entities themselves rather than on a component of either. A
    /// composite component is named by its own name, the last segment of its path, the
    /// way `GetEntity` keys it.
    component_name: Option<&'a str>,
    /// The component at the far end the relationship names, owned for the same reason
    /// the name is: it comes out of the value a read of the property RESOLVED, which a
    /// default declared on the component type contributes to.
    target_component_name: Option<String>,
}

/// The nodes and edges of one workspace, derived fresh per call from a snapshot of it —
/// so a query never reads a cached projection of state.
#[derive(Debug)]
struct Graph<'a> {
    /// The snapshotted workspace the nodes and edges below are derived from, kept so a
    /// projection of a component's properties resolves them against the component types
    /// the components instantiate ([`component_property_values`]).
    workspace: &'a crate::state::Workspace,
    /// The workspace's entities in `entityId` order (the order the snapshot's map gave
    /// them in), so a row set (and therefore a page boundary) is the same for the same
    /// state.
    entities: Vec<&'a crate::state::Entity>,
    /// Each entity's node labels: the `componentTypeId` of each of its components, at
    /// every path the entity holds one at.
    labels: Vec<BTreeSet<&'a str>>,
    edges: Vec<GraphEdge<'a>>,
    /// Edge indices by source entity, and by target entity.
    outgoing: Vec<Vec<usize>>,
    incoming: Vec<Vec<usize>>,
}

impl<'a> Graph<'a> {
    /// Build the graph from a SNAPSHOT of the workspace, taken under the store's lock and
    /// read here with the lock already released.
    fn build(snapshot: &'a crate::state::Workspace) -> Self {
        let entities: Vec<&crate::state::Entity> = snapshot.entities.values().collect();
        let index_of: BTreeMap<&str, usize> = entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.entity_id.as_str(), index))
            .collect();
        // Every component the entity holds, walked through the one scan every
        // component-scoped reader of this crate shares
        // ([`crate::state::Entity::components_at_every_path`]): a composite component is a
        // component of the entity too, with a `componentTypeId` and properties of its
        // own, so the type it instantiates labels the node just as a top-level
        // component's does.
        let labels: Vec<BTreeSet<&str>> = entities
            .iter()
            .copied()
            .map(|entity| {
                entity
                    .components_at_every_path()
                    .filter_map(|component| component.component_type_id())
                    .collect()
            })
            .collect();

        let mut edges = Vec::new();
        let mut outgoing = vec![Vec::new(); entities.len()];
        let mut incoming = vec![Vec::new(); entities.len()];
        for (source, entity) in entities.iter().copied().enumerate() {
            // The hierarchy relationship, from the child to the parent entity it is
            // stored under. An entity at the top of the hierarchy names the root
            // sentinel rather than an entity, so it has no such edge — the same rule as
            // for a property relationship pointing outside the workspace.
            if let Some(parent) = index_of.get(entity.parent_entity_id.as_str()).copied() {
                let index = edges.len();
                edges.push(GraphEdge {
                    source,
                    target: parent,
                    relationship_name: HIERARCHY_RELATIONSHIP_NAME.to_string(),
                    component_name: None,
                    target_component_name: None,
                });
                outgoing[source].push(index);
                incoming[parent].push(index);
            }
            for component in entity.components_at_every_path() {
                // Every property the component HAS, each with the value a read of it
                // reports — so a relationship a component only inherits from its
                // component type, and one the time-series data plane wrote last, is an
                // edge here exactly as it is a relationship to every other reader.
                for (property_name, value) in component_property_values(snapshot, &component) {
                    let Some(relationship) = value
                        .as_ref()
                        .and_then(|value| value.relationship_value.as_ref())
                    else {
                        continue;
                    };
                    // A relationship naming an entity this workspace does not hold is
                    // no edge: the node it would end at does not exist.
                    let Some(target) = relationship
                        .target_entity_id
                        .as_deref()
                        .and_then(|target| index_of.get(target).copied())
                    else {
                        continue;
                    };
                    let index = edges.len();
                    edges.push(GraphEdge {
                        source,
                        target,
                        relationship_name: property_name,
                        component_name: Some(component.component_name()),
                        target_component_name: relationship.target_component_name.clone(),
                    });
                    outgoing[source].push(index);
                    incoming[target].push(index);
                }
            }
        }
        Self {
            workspace: snapshot,
            entities,
            labels,
            edges,
            outgoing,
            incoming,
        }
    }

    /// Whether the entity at `index` carries the node label `label`.
    fn has_label(&self, index: usize, label: &str) -> bool {
        self.labels[index].contains(label)
    }

    /// The whole of what a binding is bound to, as the row data document for a `NODE` or
    /// `EDGE` column.
    fn document(&self, bound: Bound) -> Document {
        let members = match bound {
            Bound::Node(index) => NODE_MEMBERS
                .iter()
                .filter_map(|name| {
                    node_member(self.workspace, self.entities[index], name)
                        .map(|value| ((*name).to_string(), value))
                })
                .collect(),
            Bound::Edge(index) => EDGE_MEMBERS
                .iter()
                .filter_map(|name| {
                    self.edge_member(&self.edges[index], name)
                        .map(|value| ((*name).to_string(), value))
                })
                .collect(),
        };
        Document::Object(members)
    }

    /// One member of what a binding is bound to, or `None` when it names no member of it
    /// — PartiQL's MISSING, which is what makes a comparison against it UNKNOWN.
    fn member(&self, bound: Bound, field: &str) -> Option<Document> {
        match bound {
            Bound::Node(index) => node_member(self.workspace, self.entities[index], field),
            Bound::Edge(index) => self.edge_member(&self.edges[index], field),
        }
    }

    fn edge_member(&self, edge: &GraphEdge<'_>, field: &str) -> Option<Document> {
        match field {
            "relationshipName" => Some(text(&edge.relationship_name)),
            "sourceEntityId" => Some(text(&self.entities[edge.source].entity_id)),
            "sourceComponentName" => edge.component_name.map(text),
            "targetEntityId" => Some(text(&self.entities[edge.target].entity_id)),
            "targetComponentName" => edge.target_component_name.as_deref().map(text),
            _ => None,
        }
    }
}

/// The members a `NODE` row carries, which are also the paths `e.<member>` reads.
///
/// Each is the stored entity's corresponding member, named as this service names it on
/// the entity itself (`GetEntity`/`EntitySummary`: `entityId`, `entityName`, `arn`,
/// `description`), beside the entity's creation and update instants and its components.
const NODE_MEMBERS: [&str; 7] = [
    "arn",
    "components",
    "creationDate",
    "description",
    "entityId",
    "entityName",
    "lastUpdateDate",
];

/// The members an `EDGE` row carries, which are also the paths `r.<member>` reads: the
/// relationship's name, and the entity and component at each of its ends.
const EDGE_MEMBERS: [&str; 5] = [
    "relationshipName",
    "sourceComponentName",
    "sourceEntityId",
    "targetComponentName",
    "targetEntityId",
];

fn node_member(
    workspace: &crate::state::Workspace,
    entity: &crate::state::Entity,
    field: &str,
) -> Option<Document> {
    match field {
        "arn" => Some(text(&entity.arn)),
        "entityId" => Some(text(&entity.entity_id)),
        "entityName" => Some(text(&entity.entity_name)),
        // Present even when empty: an entity always carries a description (the empty
        // string when its create named none), the way every other read reports it.
        "description" => Some(text(&entity.description)),
        "creationDate" => Some(epoch_seconds(entity.creation_date_time)),
        "lastUpdateDate" => Some(epoch_seconds(entity.update_date_time)),
        "components" => Some(components_document(workspace, entity)),
        _ => None,
    }
}

/// The entity's stored components, each with every property it HAS and the value that
/// property reads as ([`component_property_values`]): every component the entity holds,
/// at every path it holds one at, so a composite component is reported as the component
/// of the entity it is. A composite component carries the `componentPath` it is addressed
/// by, the member this service reports one by (`ComponentSummary::componentPath`), which
/// a top-level component — addressed by name alone — does not have.
fn components_document(
    workspace: &crate::state::Workspace,
    entity: &crate::state::Entity,
) -> Document {
    Document::Array(
        entity
            .components_at_every_path()
            .map(|component| {
                let mut members = HashMap::from([(
                    "componentName".to_string(),
                    text(component.component_name()),
                )]);
                if let Some(component_type_id) = component.component_type_id() {
                    members.insert("componentTypeId".to_string(), text(component_type_id));
                }
                if let Some(component_path) = component.component_path() {
                    members.insert("componentPath".to_string(), text(component_path));
                }
                if let Some(description) = component.description() {
                    members.insert("description".to_string(), text(description));
                }
                members.insert(
                    "properties".to_string(),
                    Document::Array(
                        component_property_values(workspace, &component)
                            .into_iter()
                            .map(|(property_name, value)| {
                                let mut property_members = HashMap::from([(
                                    "propertyName".to_string(),
                                    text(&property_name),
                                )]);
                                if let Some(value) = value {
                                    property_members
                                        .insert("propertyValue".to_string(), data_value(&value, 0));
                                }
                                Document::Object(property_members)
                            })
                            .collect(),
                    ),
                );
                Document::Object(members)
            })
            .collect(),
    )
}

/// Every property one component of an entity HAS, in property-name order, each with the
/// value a read of it reports — `None` for a property that reads as no value at all.
///
/// The two projections every property reader of this crate shares, composed: the
/// component's EFFECTIVE property set
/// ([`crate::state::Workspace::component_property_names`]), which is what naming a
/// `componentTypeId` gives the component on top of the records the entity stores itself,
/// and the value each of those properties reads as
/// ([`crate::state::latest_property_value`]) — the latest value written to it through
/// either plane, else the `defaultValue` its effective definition declares
/// ([`crate::state::Workspace::effective_property_definition`], which is what makes a
/// default declared only on the component TYPE apply to a component that restates
/// nothing).
///
/// Both of this operation's property projections go through here — the `properties` array
/// of a `NODE` row and the relationships [`Graph::build`] derives its edges from — so one
/// property of one component says the same thing in a query as it does through
/// `GetEntity`, `ListProperties` and `GetPropertyValue`.
///
/// The cost is bounded by the component's own properties plus the definitions on its
/// inheritance chain, which
/// [`crate::state::Workspace::component_type_chain`] walks at most once per component type
/// reached.
fn component_property_values(
    workspace: &crate::state::Workspace,
    component: &crate::state::ComponentAtPath<'_>,
) -> Vec<(String, Option<DataValue>)> {
    let component_type_id = component.component_type_id();
    let properties = component.properties();
    workspace
        .component_property_names(component_type_id, properties)
        .into_iter()
        .map(|property_name| {
            let definition = workspace.effective_property_definition(
                component_type_id,
                properties,
                &property_name,
            );
            let value = crate::state::latest_property_value(
                properties.get(&property_name),
                definition.as_ref(),
            );
            (property_name, value)
        })
        .collect()
}

/// How deeply a stored `DataValue`'s list and map nesting is projected before the
/// projection stops. A `DataValue` nests through `listValue`/`mapValue`, and its depth
/// comes from what a write stored rather than from this request, so the projection is
/// bounded for the same reason the parser is: the number of stack frames must not be
/// something another call gets to choose (a stack overflow aborts the process rather
/// than failing one request). Past the bound the member reads as `null`.
const MAX_VALUE_DEPTH: usize = 32;

/// One stored `DataValue` as a row data document: the member it carries, in the order
/// the shape declares them, so a value set as a string reads back as a string.
///
/// `depth` is how far inside a nested list or map this value sits; see
/// [`MAX_VALUE_DEPTH`].
fn data_value(value: &DataValue, depth: usize) -> Document {
    if depth >= MAX_VALUE_DEPTH {
        return Document::Null;
    }
    if let Some(found) = value.boolean_value {
        return Document::Bool(found);
    }
    if let Some(found) = value.double_value {
        return Document::Number(Number::Float(found));
    }
    if let Some(found) = value.integer_value {
        return Document::Number(whole_number(i64::from(found)));
    }
    if let Some(found) = value.long_value {
        return Document::Number(whole_number(found));
    }
    if let Some(found) = &value.string_value {
        return text(found);
    }
    if let Some(found) = &value.list_value {
        return Document::Array(
            found
                .iter()
                .map(|entry| data_value(entry, depth + 1))
                .collect(),
        );
    }
    if let Some(found) = &value.map_value {
        return Document::Object(
            found
                .iter()
                .map(|(key, entry)| (key.clone(), data_value(entry, depth + 1)))
                .collect(),
        );
    }
    if let Some(found) = &value.relationship_value {
        let mut members = HashMap::new();
        if let Some(target_entity_id) = &found.target_entity_id {
            members.insert("targetEntityId".to_string(), text(target_entity_id));
        }
        if let Some(target_component_name) = &found.target_component_name {
            members.insert(
                "targetComponentName".to_string(),
                text(target_component_name),
            );
        }
        return Document::Object(members);
    }
    if let Some(found) = &value.expression {
        return text(found);
    }
    Document::Null
}

fn text(value: &str) -> Document {
    Document::String(value.to_string())
}

/// A stored whole number as the document number variant it fits: `PosInt` for a
/// non-negative value, `NegInt` for a negative one.
fn whole_number(value: i64) -> Number {
    match u64::try_from(value) {
        Ok(unsigned) => Number::PosInt(unsigned),
        Err(_) => Number::NegInt(value),
    }
}

/// A stored instant as the seconds since the epoch a row data document carries.
fn epoch_seconds(at: SystemTime) -> Document {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Document::Number(Number::Float(since_epoch.as_secs_f64()))
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// What one variable is bound to in one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    /// An entity, by its index in [`Graph::entities`].
    Node(usize),
    /// A relationship, by its index in [`Graph::edges`].
    Edge(usize),
}

/// One row's bindings, by variable slot.
type Binding = Vec<Option<Bound>>;

/// Why a query was refused mid-evaluation.
#[derive(Debug)]
enum QueryFault {
    /// The walk ran out of its step budget or its wall clock.
    Timeout(String),
    /// The query selected more rows than this emulator serves for one query.
    TooManyRows(String),
}

/// The evaluator: enumerates every binding of the statement's variables that its
/// patterns and its `WHERE` clause admit.
///
/// A relationship is used at most once per row (relationship isomorphism, as graph
/// pattern languages define it), so an undirected hop cannot walk back along the edge it
/// just came in on.
#[derive(Debug)]
struct Matcher<'a> {
    graph: &'a Graph<'a>,
    query: &'a Query,
    plan: &'a Plan,
    deadline: Instant,
    steps: u64,
    rows: Vec<Binding>,
}

impl<'a> Matcher<'a> {
    fn new(graph: &'a Graph<'a>, query: &'a Query, plan: &'a Plan) -> Self {
        Self {
            graph,
            query,
            plan,
            deadline: Instant::now() + QUERY_DEADLINE,
            steps: 0,
            rows: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Vec<Binding>, QueryFault> {
        let mut binding: Binding = vec![None; self.plan.variables.len()];
        let mut used_edges: BTreeSet<usize> = BTreeSet::new();
        self.solve(0, &mut binding, &mut used_edges)?;
        Ok(self.rows)
    }

    /// Charge one unit of the walk's budget.
    fn charge(&mut self) -> Result<(), QueryFault> {
        self.steps += 1;
        if self.steps > MATCH_STEP_BUDGET {
            return Err(QueryFault::Timeout(format!(
                "the query did not finish within {MATCH_STEP_BUDGET} candidate matches"
            )));
        }
        if self.steps.is_multiple_of(STEPS_PER_CLOCK_CHECK) && Instant::now() >= self.deadline {
            return Err(QueryFault::Timeout(format!(
                "the query did not finish within {} ms",
                QUERY_DEADLINE.as_millis()
            )));
        }
        Ok(())
    }

    /// Match the pattern at `pattern_index`, then the ones after it.
    fn solve(
        &mut self,
        pattern_index: usize,
        binding: &mut Binding,
        used_edges: &mut BTreeSet<usize>,
    ) -> Result<(), QueryFault> {
        let query = self.query;
        if pattern_index == query.patterns.len() {
            return self.collect(binding);
        }
        let path = &query.patterns[pattern_index];
        for candidate in self.node_candidates(&path.start, binding) {
            self.charge()?;
            let bound_slot = bind(binding, self.slot_of(path.start.variable.as_deref()), candidate);
            self.walk(pattern_index, 0, candidate, binding, used_edges)?;
            unbind(binding, bound_slot);
        }
        Ok(())
    }

    /// Walk the hops of one pattern, from the node bound at `current`.
    fn walk(
        &mut self,
        pattern_index: usize,
        hop: usize,
        current: usize,
        binding: &mut Binding,
        used_edges: &mut BTreeSet<usize>,
    ) -> Result<(), QueryFault> {
        let query = self.query;
        let graph = self.graph;
        let path = &query.patterns[pattern_index];
        if hop == path.hops.len() {
            return self.solve(pattern_index + 1, binding, used_edges);
        }
        let (edge_pattern, node_pattern) = &path.hops[hop];
        // An undirected hop looks both ways; a relationship that is both (a self
        // relationship) is still one candidate, not two.
        let candidates: Vec<usize> = match edge_pattern.direction {
            Direction::Outgoing => graph.outgoing[current].clone(),
            Direction::Incoming => graph.incoming[current].clone(),
            Direction::Either => graph.outgoing[current]
                .iter()
                .chain(graph.incoming[current].iter())
                .copied()
                .collect::<BTreeSet<usize>>()
                .into_iter()
                .collect(),
        };
        let edge_slot = self.slot_of(edge_pattern.variable.as_deref());
        let node_slot = self.slot_of(node_pattern.variable.as_deref());
        for edge_index in candidates {
            self.charge()?;
            if used_edges.contains(&edge_index) {
                continue;
            }
            let edge = &graph.edges[edge_index];
            if edge_pattern
                .label
                .as_deref()
                .is_some_and(|label| label != edge.relationship_name)
            {
                continue;
            }
            match edge_slot.and_then(|slot| binding[slot]) {
                Some(Bound::Edge(already)) if already != edge_index => continue,
                Some(Bound::Node(_)) => continue,
                _ => {}
            }
            let other = if edge.source == current {
                edge.target
            } else {
                edge.source
            };
            if !self.node_admits(node_pattern, other, binding) {
                continue;
            }
            let bound_edge = bind_edge(binding, edge_slot, edge_index);
            let bound_node = bind(binding, node_slot, other);
            used_edges.insert(edge_index);
            self.walk(pattern_index, hop + 1, other, binding, used_edges)?;
            used_edges.remove(&edge_index);
            unbind(binding, bound_node);
            unbind(binding, bound_edge);
        }
        Ok(())
    }

    /// A completed binding: report it when the `WHERE` clause selects it.
    fn collect(&mut self, binding: &Binding) -> Result<(), QueryFault> {
        let selected = match &self.query.filter {
            None => true,
            // UNKNOWN does not select a row, only TRUE does.
            Some(filter) => self.evaluate(filter, binding) == Some(true),
        };
        if !selected {
            return Ok(());
        }
        if self.rows.len() >= MAX_RESULT_ROWS {
            return Err(QueryFault::TooManyRows(format!(
                "the query selects more than {MAX_RESULT_ROWS} rows; narrow it with a WHERE \
                 clause or a more specific MATCH pattern"
            )));
        }
        self.rows.push(binding.clone());
        Ok(())
    }

    fn slot_of(&self, variable: Option<&str>) -> Option<usize> {
        Some(self.plan.variables.get(variable?)?.slot)
    }

    /// The entities the node pattern can bind, given what is already bound.
    fn node_candidates(&self, pattern: &NodePattern, binding: &Binding) -> Vec<usize> {
        match self
            .slot_of(pattern.variable.as_deref())
            .and_then(|slot| binding[slot])
        {
            // Already bound by an earlier pattern: the only candidate is that binding,
            // which is how two patterns sharing a variable join.
            Some(Bound::Node(index)) => {
                if self.label_admits(pattern, index) {
                    vec![index]
                } else {
                    Vec::new()
                }
            }
            Some(Bound::Edge(_)) => Vec::new(),
            None => (0..self.graph.entities.len())
                .filter(|index| self.label_admits(pattern, *index))
                .collect(),
        }
    }

    fn label_admits(&self, pattern: &NodePattern, index: usize) -> bool {
        pattern
            .label
            .as_deref()
            .is_none_or(|label| self.graph.has_label(index, label))
    }

    fn node_admits(&self, pattern: &NodePattern, index: usize, binding: &Binding) -> bool {
        if !self.label_admits(pattern, index) {
            return false;
        }
        match self
            .slot_of(pattern.variable.as_deref())
            .and_then(|slot| binding[slot])
        {
            Some(bound) => bound == Bound::Node(index),
            None => true,
        }
    }

    // --- the WHERE clause, in PartiQL's three-valued logic ---

    fn evaluate(&self, expr: &Expr, binding: &Binding) -> Option<bool> {
        match expr {
            Expr::Or(left, right) => {
                match (self.evaluate(left, binding), self.evaluate(right, binding)) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                }
            }
            Expr::And(left, right) => {
                match (self.evaluate(left, binding), self.evaluate(right, binding)) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                }
            }
            Expr::Not(inner) => self.evaluate(inner, binding).map(|value| !value),
            Expr::Compare {
                left,
                operator,
                right,
            } => compare(
                &self.operand_value(left, binding)?,
                *operator,
                &self.operand_value(right, binding)?,
            ),
            Expr::Like {
                value,
                pattern,
                negated,
            } => match self.operand_value(value, binding)? {
                Value::Text(found) => Some(pattern.is_match(&found) != *negated),
                // LIKE applies to text only; against anything else it is UNKNOWN.
                _ => None,
            },
            // `IS NULL` is the one predicate that is DEFINITE about a missing path.
            Expr::IsNull { value, negated } => {
                Some(self.operand_value(value, binding).is_none() != *negated)
            }
        }
    }

    /// One operand's value, or `None` for PartiQL's MISSING: a path naming no member of
    /// its binding, or one naming a member that is not a scalar.
    fn operand_value(&self, operand: &Operand, binding: &Binding) -> Option<Value> {
        match operand {
            Operand::Literal(value) => Some(value.clone()),
            Operand::Path { variable, field } => {
                let bound = binding[self.plan.variables.get(variable)?.slot]?;
                match self.graph.member(bound, field)? {
                    Document::String(found) => Some(Value::Text(found)),
                    Document::Bool(found) => Some(Value::Boolean(found)),
                    Document::Number(Number::PosInt(found)) => {
                        Some(Value::Number(found as f64))
                    }
                    Document::Number(Number::NegInt(found)) => {
                        Some(Value::Number(found as f64))
                    }
                    Document::Number(Number::Float(found)) => Some(Value::Number(found)),
                    Document::Null | Document::Array(_) | Document::Object(_) => None,
                }
            }
        }
    }
}

/// Compare two values. Comparing across types is UNKNOWN rather than false, so
/// `NOT (e.entityName = 1)` does not select a row either.
fn compare(left: &Value, operator: Comparison, right: &Value) -> Option<bool> {
    let ordering = match (left, right) {
        (Value::Text(left), Value::Text(right)) => left.cmp(right),
        (Value::Number(left), Value::Number(right)) => left.partial_cmp(right)?,
        (Value::Boolean(left), Value::Boolean(right)) => left.cmp(right),
        _ => return None,
    };
    Some(match operator {
        Comparison::Equal => ordering.is_eq(),
        Comparison::NotEqual => !ordering.is_eq(),
        Comparison::Less => ordering.is_lt(),
        Comparison::LessOrEqual => ordering.is_le(),
        Comparison::Greater => ordering.is_gt(),
        Comparison::GreaterOrEqual => ordering.is_ge(),
    })
}

/// Bind a node, reporting the slot to undo — and `None` when the variable was already
/// bound (to this same node, which is checked before) or when the pattern is anonymous.
fn bind(binding: &mut Binding, slot: Option<usize>, index: usize) -> Option<usize> {
    bind_bound(binding, slot, Bound::Node(index))
}

fn bind_edge(binding: &mut Binding, slot: Option<usize>, index: usize) -> Option<usize> {
    bind_bound(binding, slot, Bound::Edge(index))
}

fn bind_bound(binding: &mut Binding, slot: Option<usize>, value: Bound) -> Option<usize> {
    let slot = slot?;
    if binding[slot].is_some() {
        return None;
    }
    binding[slot] = Some(value);
    Some(slot)
}

fn unbind(binding: &mut Binding, slot: Option<usize>) {
    if let Some(slot) = slot {
        binding[slot] = None;
    }
}

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// The envelope a `nextToken` of this operation carries, so a token minted elsewhere —
/// by another listing, or by hand — is recognisably not one of ours. The token itself is
/// the shape every paginated operation of this service mints
/// ([`crate::state::encode_page_token`]), scoped to the Region, the workspace AND the
/// statement the offset belongs to: the row set is recomputed per call, out of one
/// Region's own store, so an offset only means something for the same query in the same
/// Region. The statement goes last, where the envelope lets a scope part carry newlines of
/// its own.
const NEXT_TOKEN_PREFIX: &str = "iottwinmaker:query:1:";

/// The opaque `nextToken` for the page of this query's rows starting at `offset`.
fn encode_next_token(offset: usize, region: &str, workspace_id: &str, statement: &str) -> String {
    crate::state::encode_ordinal_page_token(
        NEXT_TOKEN_PREFIX,
        region,
        &[workspace_id, statement],
        offset,
    )
}

/// The row offset inside a `nextToken` minted in this Region for this workspace and
/// statement, or `None` when the token is not one of ours or belongs to another query.
///
/// The offset is one of the positions [`encode_next_token`] writes
/// ([`crate::state::decode_ordinal_page_token`]); whether it names a row this query
/// still selects is decided against the recomputed row set
/// ([`crate::state::ordinal_resumes_a_selected_row`]).
fn decode_next_token(
    token: &str,
    region: &str,
    workspace_id: &str,
    statement: &str,
) -> Option<usize> {
    crate::state::decode_ordinal_page_token(
        NEXT_TOKEN_PREFIX,
        region,
        &[workspace_id, statement],
        token,
    )
}

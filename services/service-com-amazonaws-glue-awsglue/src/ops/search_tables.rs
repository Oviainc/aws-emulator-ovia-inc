// SPDX-License-Identifier: CC0-1.0

//! @op SearchTables
//!
//! This is the ONLY file to touch for this operation.
//!
//! Searches EVERY table of one Data Catalog — across all of its databases, not inside
//! one named database the way `GetTables` does — and returns the stored tables whose
//! metadata, or whose parent database's metadata, satisfies the request's free-text
//! search and every one of its property predicates.
//!
//! What a table's searchable metadata is: its own name, the name of the database that
//! holds it, its description, owner and table type, its parameter keys and values, its
//! storage location, input/output formats, and its column and partition-key names,
//! types and comments — plus the parent database's name, description, location and
//! parameters, because the operation searches "properties in the table metadata as well
//! as on the parent database".
//!
//! How a value matches, from the model's own account of the match under
//! `SearchTablesRequest$Filters`: a stored value is split on punctuation into tokens and
//! each token is compared for equality with the wanted value, so `link` selects
//! `customer-link` and `xx-link-yy` but not `xxlinkyy`; the whole value counts as a
//! match too, so a wanted value that itself carries punctuation is still reachable.
//! Comparison is ASCII-case-insensitive, because the catalog folds database and table
//! names to lowercase on the way in ([`crate::state::fold_name`]) and a search that was
//! case-sensitive could not find a table by the name its client sent.
//!
//! `SearchText` is that match over the WHOLE searchable metadata: unquoted, it is split
//! on whitespace into terms and every term must match some value (a table satisfying
//! only part of the text is not a hit); wrapped in double quotes, it instead requires
//! one searchable value to equal the quoted text exactly — "specifying a value in quotes
//! filters based on an exact match to the value".
//!
//! `Filters` narrows by one named property each. A predicate's `Key` names a table
//! property — `Name`, `DatabaseName`, `Description`, `Owner`, `TableType`, `Location`,
//! the four time fields below, or, failing those, a key of the table's `Parameters` map,
//! which is where the catalog keeps the rest of a table's metadata properties. A
//! predicate carrying no `Key` matches when ANY searchable value matches, which is what
//! a predicate that names no property can mean. A predicate carrying no `Value` is
//! refused: there is nothing to compare against.
//!
//! `Comparator` is "used only for time fields, and can be omitted for other field
//! types", so it orders a comparison against `CreateTime`, `UpdateTime`,
//! `LastAccessTime` or `LastAnalyzedTime` (defaulting to `EQUALS`); one sent beside a
//! predicate that names no time field orders nothing and is refused rather than ignored.
//! A time predicate's `Value` is read as epoch seconds or as an RFC 3339 instant;
//! anything else is refused with `InvalidInputException`.
//!
//! `SortCriteria` orders the result by one property — the same key namespace the
//! predicates use — ascending by default, and `DESC` is the exact reverse of that order.
//! Tables that carry no value for the sort field come last in ascending order. The
//! default order, and the tie-break under a sort field, is by database name then table
//! name, so one total order exists to page over. `NextToken` carries the ordering key of
//! the last table served, so the next call resumes at the first table ordered strictly
//! after it, which stays correct across tables created or deleted between two calls.
//!
//! Members this slice does not act on, and why: `IncludeStatusDetails` asks for the
//! status of a Data Catalog view create or update, which this slice does not keep, so
//! ASKING for it is refused rather than answered without the detail — the same refusal
//! `GetTables` makes. `ResourceShareType` IS acted on: `FEDERATED` selects only tables
//! that reference an entity outside the catalog (a stored table carrying a
//! `FederatedTable`), `FOREIGN` selects only tables shared with this account from
//! another, of which a single-account store holds none, and `ALL` selects both those and
//! the local ones; absent, only this account's own tables are searched. A value outside
//! that enum is judged by this handler rather than by the framework boundary
//! ([`crate::service::resource_share_type_not_valid`]), so it earns the catalog's own
//! `InvalidInputException` naming the value the request sent.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `SearchTables::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `SearchTables`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")
//! ```
//!
//! Searches a set of tables based on properties in the table metadata as well as on the
//! parent database. You can search against text or filter conditions. You can only get
//! tables that you have access to based on the security policies defined in Lake Formation.
//! You need at least a read-only access to the table for it to be returned. If you do not
//! have access to all the columns in the table, these columns will not be searched against
//! when returning the list of tables back to you. If you have access to the columns but not
//! the data in the columns, those columns and the associated metadata for those columns
//! will be included in the search.
//!
//! ## Input
//!
//! * `CatalogId` (CatalogIdString, len 1..255, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$) — A unique identifier,
//!   consisting of account_id .
//! * `Filters` (SearchPropertyPredicates) — A list of key-value pairs, and a comparator used
//!   to filter the search results. Returns all entities matching the predicate. The
//!   Comparator member of the PropertyPredicate struct is used only for time fields, and can
//!   be omitted for other field types. Also, when comparing string values, such as when
//!   Key=Name, a fuzzy match algorithm is used. The Key field (for example, the value of the
//!   Name field) is split on certain punctuation characters, for example, -, :, #, etc. into
//!   tokens. Then each token is exact-match compared with the Value member of
//!   PropertyPredicate. For example, if Key=Name and Value=link, tables named customer-link
//!   and xx-link-yy are returned, but xxlinkyy is not returned.
//! * `IncludeStatusDetails` (BooleanNullable) — Specifies whether to include status details
//!   related to a request to create or update an Glue Data Catalog view.
//! * `MaxResults` (PageSize, range 1..1000) — The maximum number of tables to return in a
//!   single response.
//! * `NextToken` (Token) — A continuation token, included if this is a continuation call.
//! * `ResourceShareType` (ResourceShareType, one of: ALL | FEDERATED | FOREIGN) — Allows you
//!   to specify that you want to search the tables shared with your account. The allowable
//!   values are FOREIGN or ALL. If set to FOREIGN, will search the tables shared with your
//!   account. If set to ALL, will search the tables shared with your account, as well as the
//!   tables in yor local account.
//! * `SearchText` (ValueString, len 1..1024) — A string used for a text search. Specifying a
//!   value in quotes filters based on an exact match to the value.
//! * `SortCriteria` (SortCriteria, len 0..1) — A list of criteria for sorting the results by
//!   a field name, in an ascending or descending order.
//!
//! ## Output
//!
//! * `NextToken` (Token) — A continuation token, present if the current list segment is not
//!   the last.
//! * `TableList` (TableList) — A list of the requested Table objects. The SearchTables
//!   response returns only the tables that you have access to.
//!
//! ## Errors
//!
//! * `InternalServiceException` (HTTP 500) — An internal service error occurred.
//! * `InvalidInputException` (HTTP 400) — The input provided was not valid.
//! * `OperationTimeoutException` (HTTP 400) — The operation timed out.

#[allow(unused_imports)]
use super::types::*;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::state::{DatabaseEntry, TableEntry};

/// The largest page this operation serves, and the page size it uses when the request
/// names no `MaxResults`. Source: the model's `MaxResults` target `PageSize`, whose
/// range is 1..1000.
const MAX_PAGE_SIZE: i32 = 1000;

/// Ceiling on the predicates one request may carry.
///
/// This emulator's own bound, not an AWS quota: every predicate is evaluated against
/// every stored table, so the work one request can buy is bounded here rather than left
/// to grow with the unbounded `SearchPropertyPredicates` list. A request past it is
/// refused with `InvalidInputException`.
pub const MAX_FILTERS: usize = 64;

/// Ceiling on the terms one `SearchText` may carry, for the same reason as
/// [`MAX_FILTERS`]: each term is matched against every searchable value of every stored
/// table.
pub const MAX_SEARCH_TERMS: usize = 64;

/// The separator between the parts of an ordering key (and so of a `NextToken`).
///
/// `U+0001` cannot appear in a name, a description or any other value the catalog sorts
/// by: every such member's modelled pattern admits `\u0020` and up, so the separator can
/// never be confused with a character inside one of the parts it joins.
const KEY_SEPARATOR: char = '\u{1}';

/// The time properties of a stored table, under the field names a request names them
/// with, ASCII-folded. Source: the `Table` members `CreateTime`, `UpdateTime`,
/// `LastAccessTime` and `LastAnalyzedTime` (`smithy_query shape=Table`), the only
/// timestamp members a stored table carries — these are the "time fields" a
/// `PropertyPredicate`'s `Comparator` applies to.
const TIME_FIELDS: [&str; 4] = [
    "createtime",
    "updatetime",
    "lastaccesstime",
    "lastanalyzedtime",
];

/// The sentence this search refuses a `Comparator` sent beside a predicate that names no
/// time field with.
///
/// RECORDED constant, and the whole message: it names neither the comparator the request
/// sent nor the key it was sent beside, so one wording answers every such predicate.
const COMPARATOR_ONLY_TIME_FIELDS: &str = "Comparator can only apply to time fields";

/// `at` as milliseconds since the epoch, the one scale every time comparison here uses.
fn system_time_millis(at: SystemTime) -> i64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => {
            let millis = i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX);
            -millis
        }
    }
}

/// The same scale for a client-supplied timestamp member.
fn date_time_millis(at: &::aws_smithy_types::DateTime) -> i64 {
    at.secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(at.subsec_nanos() / 1_000_000))
}

/// A predicate's `Value` read as an instant: epoch seconds (whole or fractional) or an
/// RFC 3339 date-time. `None` means the text is neither, so the predicate cannot be
/// evaluated and the request is refused.
fn parse_timestamp(value: &str) -> Option<i64> {
    use ::aws_smithy_types::date_time::Format;
    [Format::EpochSeconds, Format::DateTime]
        .into_iter()
        .find_map(|format| ::aws_smithy_types::DateTime::from_str(value, format).ok())
        .map(|at| date_time_millis(&at))
}

/// `millis` rendered so that lexicographic order over the rendering is the order of the
/// instants themselves, for every `i64`: the value is biased past zero and zero-padded,
/// so no comparison of ordering keys has to parse a number back out.
fn ordered_millis(millis: i64) -> String {
    let biased = i128::from(millis) - i128::from(i64::MIN);
    format!("{biased:020}")
}

/// Every searchable value of `table`: its own metadata and that of the database holding
/// it, which is what `SearchText` and a key-less predicate are matched against.
fn searchable_values<'a>(table: &'a TableEntry, database: &'a DatabaseEntry) -> Vec<&'a str> {
    let mut values: Vec<&str> = vec![table.name.as_str(), database.name.as_str()];
    values.extend(table.description.as_deref());
    values.extend(table.owner.as_deref());
    values.extend(table.table_type.as_deref());
    values.extend(database.description.as_deref());
    values.extend(database.location_uri.as_deref());
    for parameters in [table.parameters.as_ref(), database.parameters.as_ref()]
        .into_iter()
        .flatten()
    {
        for (key, value) in parameters {
            values.push(key.as_str());
            values.push(value.as_str());
        }
    }
    if let Some(descriptor) = table.storage_descriptor.as_ref() {
        values.extend(descriptor.location.as_deref());
        values.extend(descriptor.input_format.as_deref());
        values.extend(descriptor.output_format.as_deref());
        values.extend(
            descriptor
                .columns
                .iter()
                .flatten()
                .flat_map(column_values)
                .collect::<Vec<&str>>(),
        );
    }
    values.extend(
        table
            .partition_keys
            .iter()
            .flatten()
            .flat_map(column_values)
            .collect::<Vec<&str>>(),
    );
    values
}

/// The searchable text of one column: its name, its type and its comment.
fn column_values(column: &Column) -> Vec<&str> {
    let mut values: Vec<&str> = vec![column.name.as_str()];
    values.extend(column.r#type.as_deref());
    values.extend(column.comment.as_deref());
    values
}

/// Whether `value` matches `wanted` (already ASCII-folded) under the catalog's fuzzy
/// match: `value` is split on punctuation into tokens and a token must equal `wanted`,
/// or the whole folded value must.
fn text_matches(value: &str, wanted: &str) -> bool {
    let folded = crate::state::fold_name(value);
    folded == wanted
        || folded
            .split(|character: char| !character.is_alphanumeric())
            .any(|token| token == wanted)
}

/// The text values of the property `key` names, or `None` when the catalog holds no such
/// property for this table. An EMPTY list means the property exists but this table
/// carries no value for it.
fn text_property<'a>(
    key: &str,
    table: &'a TableEntry,
    database: &'a DatabaseEntry,
) -> Option<Vec<&'a str>> {
    let values: Vec<&str> = match crate::state::fold_name(key).as_str() {
        "name" => vec![table.name.as_str()],
        "databasename" => vec![database.name.as_str()],
        "description" => table.description.as_deref().into_iter().collect(),
        "owner" => table.owner.as_deref().into_iter().collect(),
        "tabletype" => table.table_type.as_deref().into_iter().collect(),
        "location" => table
            .storage_descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.location.as_deref())
            .into_iter()
            .collect(),
        // The rest of a table's metadata properties live in its `Parameters` map, whose
        // keys are case-sensitive, so the key is looked up exactly as it was sent.
        _ => {
            return table
                .parameters
                .as_ref()
                .and_then(|parameters| parameters.get(key))
                .map(|value| vec![value.as_str()])
        }
    };
    Some(values)
}

/// The instant of the time property `key` names: `None` when `key` is not a time field,
/// `Some(None)` when it is one this table carries no value for.
fn time_property(key: &str, table: &TableEntry) -> Option<Option<i64>> {
    match crate::state::fold_name(key).as_str() {
        "createtime" => Some(Some(system_time_millis(table.create_time))),
        "updatetime" => Some(Some(system_time_millis(table.update_time))),
        "lastaccesstime" => Some(table.last_access_time.as_ref().map(date_time_millis)),
        "lastanalyzedtime" => Some(table.last_analyzed_time.as_ref().map(date_time_millis)),
        _ => None,
    }
}

/// Whether `key` names one of the table's [`TIME_FIELDS`], which is what decides whether
/// a predicate's `Comparator` is used at all.
fn is_time_field(key: &str) -> bool {
    TIME_FIELDS.contains(&crate::state::fold_name(key).as_str())
}

/// A `SearchText` as the search evaluates it.
#[derive(Debug)]
enum TextQuery {
    /// Quoted text: one searchable value must equal it exactly.
    Exact(String),
    /// Unquoted text: every term must match some searchable value.
    Terms(Vec<String>),
}

impl TextQuery {
    /// Read the request's `SearchText`. Text wrapped in double quotes is the exact-match
    /// form the model documents; anything else is split into terms on whitespace.
    fn parse(text: &str) -> Self {
        let trimmed = text.trim();
        // Stripped rather than sliced by index, so request text can never drive an
        // out-of-range slice.
        if let Some(inner) = trimmed
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            return Self::Exact(crate::state::fold_name(inner));
        }
        Self::Terms(
            trimmed
                .split_whitespace()
                .map(crate::state::fold_name)
                .collect(),
        )
    }

    /// How many terms this query is matched by, which is what [`MAX_SEARCH_TERMS`]
    /// bounds.
    fn term_count(&self) -> usize {
        match self {
            Self::Exact(_) => 1,
            Self::Terms(terms) => terms.len(),
        }
    }

    /// Whether a table whose searchable metadata is `values` satisfies this query.
    fn matches(&self, values: &[&str]) -> bool {
        match self {
            Self::Exact(wanted) => values
                .iter()
                .any(|value| crate::state::fold_name(value) == *wanted),
            Self::Terms(terms) => terms
                .iter()
                .all(|term| values.iter().any(|value| text_matches(value, term))),
        }
    }
}

/// One `PropertyPredicate` as the search evaluates it.
#[derive(Debug)]
enum Predicate {
    /// A predicate that names no property: any searchable value may match.
    AnyProperty(String),
    /// A predicate on a text property: the key as sent, and the folded wanted value.
    Text { key: String, wanted: String },
    /// A predicate on a time property, ordered by its comparator.
    Time {
        key: String,
        comparator: TimeComparator,
        millis: i64,
    },
}

/// How a time predicate compares the stored instant to the one it carries. Source: the
/// `Comparator` enum (`EQUALS | GREATER_THAN | LESS_THAN | GREATER_THAN_EQUALS |
/// LESS_THAN_EQUALS`).
#[derive(Debug, Clone, Copy)]
enum TimeComparator {
    Equals,
    GreaterThan,
    LessThan,
    GreaterThanEquals,
    LessThanEquals,
}

impl TimeComparator {
    /// Whether `stored` stands in this relation to `wanted`.
    fn admits(self, stored: i64, wanted: i64) -> bool {
        match self {
            Self::Equals => stored == wanted,
            Self::GreaterThan => stored > wanted,
            Self::LessThan => stored < wanted,
            Self::GreaterThanEquals => stored >= wanted,
            Self::LessThanEquals => stored <= wanted,
        }
    }
}

impl Predicate {
    /// Whether `table` satisfies this predicate. `values` is the table's searchable
    /// metadata, already collected for the free-text match.
    fn matches(&self, table: &TableEntry, database: &DatabaseEntry, values: &[&str]) -> bool {
        match self {
            Self::AnyProperty(wanted) => values.iter().any(|value| text_matches(value, wanted)),
            Self::Text { key, wanted } => text_property(key, table, database)
                .is_some_and(|values| values.iter().any(|value| text_matches(value, wanted))),
            Self::Time {
                key,
                comparator,
                millis,
            } => time_property(key, table)
                .flatten()
                .is_some_and(|stored| comparator.admits(stored, *millis)),
        }
    }
}

/// The ordering key of one table: the sort field's value (tables carrying none come
/// last), then the database name and the table name, which together are unique — so
/// every search has one total order to page over.
fn ordering_key(
    sort_field: Option<&str>,
    table: &TableEntry,
    database: &DatabaseEntry,
) -> String {
    let sort_value = sort_field.and_then(|field| sort_value(field, table, database));
    let (presence, value) = match sort_value {
        Some(value) => ('0', value),
        None => ('1', String::new()),
    };
    format!(
        "{presence}{value}{KEY_SEPARATOR}{}{KEY_SEPARATOR}{}",
        database.name, table.name
    )
}

/// The value `field` sorts this table by, rendered so that lexicographic order over the
/// rendering is the order of the values themselves.
fn sort_value(field: &str, table: &TableEntry, database: &DatabaseEntry) -> Option<String> {
    if let Some(instant) = time_property(field, table) {
        return instant.map(ordered_millis);
    }
    text_property(field, table, database)?
        .first()
        .map(|value| crate::state::fold_name(value))
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SearchTablesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-glue-awsglue.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SearchTablesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.glue#SearchTables",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum SearchTablesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServiceException(InternalServiceException),
    InvalidInputException(InvalidInputException),
    OperationTimeoutException(OperationTimeoutException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::search_tables::SearchTablesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type SearchTablesError = crate::scaffolded::Scaffolded<SearchTablesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SearchTablesErrorModeled> for SearchTablesError {
    fn from(modeled: SearchTablesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// Refuse the request as invalid, with the operation's own modelled client error.
fn invalid_input(message: String) -> SearchTablesError {
    SearchTablesErrorModeled::InvalidInputException(InvalidInputException {
        message: Some(message),
        from_federation_source: None,
    })
    .into()
}

/// Read one `PropertyPredicate` into the form the search evaluates, refusing a predicate
/// that cannot be evaluated at all.
fn read_predicate(predicate: &PropertyPredicate) -> Result<Predicate, SearchTablesError> {
    let Some(wanted) = predicate.value.as_deref() else {
        return Err(invalid_input(
            "Value is required on every PropertyPredicate in Filters".to_string(),
        ));
    };
    let folded = crate::state::fold_name(wanted);
    let key = predicate.key.as_deref();
    // "The `Comparator` member of the `PropertyPredicate` struct is used only for time
    // fields, and can be omitted for other field types": a comparator sent beside a
    // predicate that names no time field — a text property, or no property at all —
    // orders nothing, so it is REFUSED rather than ignored, which is the only answer that
    // tells the caller its filter was not applied the way it asked.
    if predicate.comparator.is_some() && !key.is_some_and(is_time_field) {
        return Err(invalid_input(COMPARATOR_ONLY_TIME_FIELDS.to_string()));
    }
    let Some(key) = key else {
        return Ok(Predicate::AnyProperty(folded));
    };
    if !is_time_field(key) {
        return Ok(Predicate::Text {
            key: key.to_string(),
            wanted: folded,
        });
    }
    let comparator = match predicate.comparator.as_deref() {
        // A time predicate that names no comparator asks for the instant itself.
        None | Some("EQUALS") => TimeComparator::Equals,
        Some("GREATER_THAN") => TimeComparator::GreaterThan,
        Some("LESS_THAN") => TimeComparator::LessThan,
        Some("GREATER_THAN_EQUALS") => TimeComparator::GreaterThanEquals,
        Some("LESS_THAN_EQUALS") => TimeComparator::LessThanEquals,
        Some(other) => {
            return Err(invalid_input(format!(
                "Comparator {other} is not one of EQUALS, GREATER_THAN, LESS_THAN, \
                 GREATER_THAN_EQUALS, LESS_THAN_EQUALS"
            )))
        }
    };
    let Some(millis) = parse_timestamp(wanted) else {
        return Err(invalid_input(format!(
            "Value {wanted} is not a timestamp, which the time field {key} is compared against"
        )));
    };
    Ok(Predicate::Time {
        key: key.to_string(),
        comparator,
        millis,
    })
}

/// Which tables of the catalog the request searches at all, from `ResourceShareType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Share {
    /// This account's own tables, which is what a request naming no share type searches.
    Local,
    /// Only tables that reference an entity outside the Glue Data Catalog.
    Federated,
    /// Only tables shared with this account from another one.
    Foreign,
    /// Both the shared and the local ones.
    All,
}

impl Share {
    /// Whether this share type admits a stored table. A stored table is federated
    /// exactly when it carries a `FederatedTable`, and no stored table is foreign: the
    /// store holds one account's catalog, so there is nothing in it shared from another
    /// account.
    fn admits(self, table: &TableEntry) -> bool {
        match self {
            Self::Local | Self::All => true,
            Self::Federated => table.federated_table.is_some(),
            Self::Foreign => false,
        }
    }
}

/// The `SearchTables` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SearchTables<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SearchTables<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SearchTables<Eng>
{
    type Input = SearchTablesInput;
    type Output = SearchTablesOutput;
    type Error = SearchTablesError;

    const NAME: &'static str = "SearchTables";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SEARCH_TABLES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SEARCH_TABLES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `IncludeStatusDetails` asks for the status of a Data Catalog VIEW create or
        // update, which this slice does not keep, so ASKING for it is refused; not
        // asking for it is served normally — the same answer `GetTables` gives.
        if input.include_status_details == Some(true) {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "SearchTables with IncludeStatusDetails",
            ));
        }

        let share = match input.resource_share_type.as_deref() {
            None => Share::Local,
            Some("ALL") => Share::All,
            Some("FEDERATED") => Share::Federated,
            Some("FOREIGN") => Share::Foreign,
            // A share type outside the enum this member targets is judged here rather than
            // at the framework boundary (`skip_validation` on `SearchTablesRequest`), so it
            // earns the catalog's own sentence naming the value the request sent.
            Some(other) => {
                return Err(invalid_input(
                    crate::service::resource_share_type_not_valid(other),
                ))
            }
        };

        let filters = input.filters.as_deref().unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(invalid_input(format!(
                "Filters carries {} predicates, more than the {MAX_FILTERS} this catalog \
                 evaluates",
                filters.len()
            )));
        }
        let predicates = filters
            .iter()
            .map(read_predicate)
            .collect::<Result<Vec<Predicate>, SearchTablesError>>()?;

        let query = input.search_text.as_deref().map(TextQuery::parse);
        if let Some(query) = query.as_ref() {
            if query.term_count() > MAX_SEARCH_TERMS {
                return Err(invalid_input(format!(
                    "SearchText carries {} terms, more than the {MAX_SEARCH_TERMS} this catalog \
                     matches",
                    query.term_count()
                )));
            }
        }

        // `SortCriteria` holds at most one criterion (the model's own length 0..1), so
        // the result is ordered by at most one field.
        let criterion = input
            .sort_criteria
            .as_deref()
            .unwrap_or_default()
            .first()
            .cloned();
        let descending = match criterion.as_ref().and_then(|one| one.sort.as_deref()) {
            None | Some("ASC") => false,
            Some("DESC") => true,
            Some(other) => {
                return Err(invalid_input(format!(
                    "Sort {other} is not one of ASC, DESC"
                )))
            }
        };
        let sort_field = criterion
            .as_ref()
            .and_then(|one| one.field_name.as_deref())
            .filter(|field| !field.is_empty());

        // Absent `MaxResults`, page at the largest value the model accepts for it
        // (`PageSize`, range 1..1000). The clamp re-states that range in the handler so
        // the page size is never 0 even if the boundary validator ever stops enforcing
        // it.
        let page_size = usize::try_from(input.max_results.unwrap_or(MAX_PAGE_SIZE))
            .unwrap_or(MAX_PAGE_SIZE as usize)
            .clamp(1, MAX_PAGE_SIZE as usize);

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(SearchTablesErrorModeled::InternalServiceException(
                InternalServiceException {
                    message: Some("the Glue Data Catalog store is unavailable".into()),
                },
            )
            .into());
        };
        let catalog_id = crate::state::resolve_catalog_id(input.catalog_id.as_ref());
        // Whether the caller may search this catalog at all is decided BEFORE anything is
        // read out of it: a `CatalogId` of another account is reachable only through that
        // catalog's own resource policy (see `crate::state::foreign_catalog_refusal`), so
        // a cross-account search is refused rather than answered with an empty page.
        if let Some(message) = crate::state::foreign_catalog_refusal(
            &catalog_id,
            &crate::state::request_region(ctx.parts()),
            Self::NAME,
        ) {
            return Err(SearchTablesErrorModeled::AccessDeniedException(
                AccessDeniedException {
                    message: Some(message),
                },
            )
            .into());
        }

        let store = state.lock();
        // A catalog this store does not hold holds no tables, so the search over it is
        // empty rather than a refusal: the operation models no not-found error, and a
        // search that matches nothing is a legitimate empty answer.
        let databases = store
            .catalogs
            .get(&catalog_id)
            .map(|catalog| catalog.databases.values().collect::<Vec<&DatabaseEntry>>())
            .unwrap_or_default();

        // The search walks EVERY database of the catalog, not one named database: that
        // is what makes a table of the same name in several databases a hit in each.
        let mut matching: Vec<(String, &TableEntry, &DatabaseEntry)> = Vec::new();
        for database in databases {
            for table in database.tables.values() {
                if !share.admits(table) {
                    continue;
                }
                let values = searchable_values(table, database);
                if query.as_ref().is_some_and(|query| !query.matches(&values)) {
                    continue;
                }
                if !predicates
                    .iter()
                    .all(|predicate| predicate.matches(table, database, &values))
                {
                    continue;
                }
                matching.push((ordering_key(sort_field, table, database), table, database));
            }
        }
        matching.sort_by(|left, right| left.0.cmp(&right.0));
        if descending {
            // The ordering key is unique per table, so reversing the ascending order is
            // exactly the descending one.
            matching.reverse();
        }

        // The token is the ordering key of the last table of the previous page, so this
        // page resumes at the first table ordered strictly after it — which stays
        // correct across tables created or deleted between two calls.
        let start = match input.next_token.as_deref() {
            Some(cursor) => matching.partition_point(|(key, _, _)| {
                if descending {
                    key.as_str() >= cursor
                } else {
                    key.as_str() <= cursor
                }
            }),
            None => 0,
        };
        let end = start.saturating_add(page_size).min(matching.len());
        // `get` rather than an index, so no arithmetic on a request-derived page size
        // can slice out of range.
        let page = matching.get(start..end).unwrap_or_default();

        // Each hit is served in the SAME projection the single-table read serves, so a
        // client that works from a search is never less informed than one that re-reads
        // each table.
        let table_list: Vec<Table> = page
            .iter()
            .map(|(_, table, database)| table.to_table(&database.database_id))
            .collect();
        let next_token = if end < matching.len() {
            page.last().map(|(key, _, _)| key.clone())
        } else {
            None
        };

        Ok(SearchTablesOutput {
            table_list: Some(table_list),
            next_token,
        })
    }
}

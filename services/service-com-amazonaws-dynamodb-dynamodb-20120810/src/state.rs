// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dynamodb-dynamodb-20120810 service.
//!
//! This is the DynamoDB table + item store every handler in `src/ops/` reads and
//! writes, plus the pure helpers several of them share (key extraction, the
//! order-preserving key encoding, item sizing, capacity accounting and the
//! `TableDescription` projection). Nothing here is a canned response: every value an
//! operation reports is computed from what a previous request wrote.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AttributeDefinition, AttributeValue, BillingModeSummary, Capacity, ConsumedCapacity,
    GlobalSecondaryIndexDescription, ItemCollectionMetrics, KeySchemaElement,
    LocalSecondaryIndexDescription, Projection, ProvisionedThroughput,
    ProvisionedThroughputDescription, Tag, TableClassSummary, TableDescription,
};

/// A DynamoDB item: the attribute-name -> attribute-value map a client put.
pub type Item = HashMap<String, AttributeValue>;

/// A resource's tag set: at most ONE value per key — "Each DynamoDB table can only
/// have up to one tag with the same key" (the `Tag$Key` doc) — kept in key order so
/// every reader reports the same list for the same stored set.
pub type TagSet = BTreeMap<String, String>;

/// How long a table stays in a transient status before the clock makes the next
/// status due.
///
/// `CreateTable` and `DeleteTable` are asynchronous — the model's own doc for
/// `CreateTable` says "Upon receiving a CreateTable request, DynamoDB immediately
/// returns a response with a TableStatus of CREATING. After the table is created,
/// DynamoDB sets the TableStatus to ACTIVE" — so a table is reported `CREATING`
/// (then `DELETING`) for a span rather than flipping instantly. Real AWS takes as
/// long as it takes; this emulator CHOOSES 500 ms: non-zero, so a caller that acts
/// immediately after the call still observes the transient status and the state
/// errors that go with it, and short enough that a client's waiter settles quickly.
/// The due instant is stored on the table and materialised when a later request
/// reads it — nothing sleeps.
pub const STATUS_TRANSITION: Duration = Duration::from_millis(500);

/// The maximum size of one item, 400 KB ("Service, Account, and Table Quotas" in the
/// Amazon DynamoDB Developer Guide: "The maximum item size in DynamoDB is 400 KB").
pub const MAX_ITEM_SIZE_BYTES: i64 = 400 * 1024;

/// How a write of an item past [`MAX_ITEM_SIZE_BYTES`] is refused: AWS's FIXED
/// sentence for that quota ("Item size has exceeded the maximum allowed size" —
/// the Developer Guide's "Error handling" entry for `ValidationException`), with
/// neither the ceiling nor the offending size appended. Both figures are
/// internal to the check; the caller is told which quota it crossed.
pub const ITEM_SIZE_EXCEEDED_MESSAGE: &str = "Item size has exceeded the maximum allowed size";

/// How a `Key` member that is not the table's primary key is refused: AWS's
/// FIXED sentence for that mismatch, with nothing about WHICH element differs
/// appended — a caller learns its key does not match without the refusal
/// reporting this table's schema back to it.
pub const KEY_SCHEMA_MISMATCH_MESSAGE: &str =
    "The provided key element does not match the schema";

/// How an `Item` that omits a key attribute the schema declares is refused:
/// AWS's FIXED sentence for that case, again naming no attribute.
pub const MISSING_KEY_VALUE_MESSAGE: &str = "One of the required keys was not given a value";

/// Which request member a primary key is being read out of.
///
/// The walk over the key schema is the same for both; only the WORDING of a
/// missing or wrongly-typed element differs, which is what this carries. A
/// `Key` IS the primary key, so any way it differs from the schema is the one
/// mismatch; an `Item` is the whole item, so a key attribute it does not carry
/// is reported as the value it did not give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
    Item,
    Key,
}

impl KeySource {
    /// How this source words a key attribute the schema declares and the
    /// request did not carry.
    fn absent(self) -> RequestError {
        match self {
            Self::Item => RequestError::Validation(MISSING_KEY_VALUE_MESSAGE.to_string()),
            Self::Key => self.mismatch(),
        }
    }

    /// The one sentence a key that is not this table's primary key is refused
    /// with.
    fn mismatch(self) -> RequestError {
        RequestError::Validation(KEY_SCHEMA_MISMATCH_MESSAGE.to_string())
    }
}

/// How deeply an attribute value may nest: "DynamoDB supports nested attributes up to
/// 32 levels deep" ("Service, Account, and Table Quotas"). A value deeper than this
/// is refused, so no walk over a request's attributes is driven by caller-chosen
/// depth.
pub const MAX_ATTRIBUTE_DEPTH: usize = 32;

/// The most bytes a KEY attribute's value may carry, by the role it plays in a key
/// schema: "The maximum length of any partition key value is 2048 bytes" and "The
/// maximum length of any sort key value is 1024 bytes" ("Service, Account, and Table
/// Quotas" in the Amazon DynamoDB Developer Guide). A value past the limit for its
/// role is refused before it reaches state, on the read path as well as the write
/// path, because a key that cannot be stored cannot be looked up either.
pub const MAX_PARTITION_KEY_BYTES: i64 = 2048;
pub const MAX_SORT_KEY_BYTES: i64 = 1024;

/// The most bytes a resource-based policy document may carry: "The maximum size
/// supported for a resource-based policy document is 20 KB. DynamoDB counts
/// whitespaces when calculating the size of a policy against this limit"
/// (`PutResourcePolicy$Policy`).
pub const MAX_RESOURCE_POLICY_BYTES: usize = 20 * 1024;

/// The most tables this emulator will hold IN ONE REGION: "There is a soft account
/// quota of 2,500 tables" (the `LimitExceededException` doc), a quota AWS applies per
/// account and region. Reaching it answers that modelled error instead of growing
/// without end.
pub const MAX_TABLES: usize = 2500;

/// How many distinct regions this store keeps tables for.
///
/// [`MAX_TABLES`] alone would not bound the store: the region keying it is the
/// request's own SigV4 credential-scope region, which this emulator does not
/// authenticate, so without this cap a caller could open a fresh per-region bucket per
/// request. With it the store holds at most `MAX_REGIONS` buckets, and so at most
/// `MAX_REGIONS * MAX_TABLES` tables in total. AWS operates on the order of 36
/// commercial regions, so this bound is this emulator's and is comfortably above the
/// real surface; reaching it answers the same modelled `LimitExceededException` AWS
/// answers when a create would exceed an allowance.
pub const MAX_REGIONS: usize = 64;

/// The most tags one resource may carry: "You can add up to 50 tags to a single
/// DynamoDB table" (the `Tag` shape's doc). A request carrying more, or a merge that
/// would push a resource past it, is answered with the modelled
/// `LimitExceededException` rather than stored.
pub const MAX_TAGS_PER_TABLE: usize = 50;

/// The tag-key prefix a caller may not assign: "Amazon Web Services-assigned tag
/// names and values are automatically assigned the `aws:` prefix, which the user
/// cannot assign" (the `Tag` doc).
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The most global secondary indexes one table may carry: "One or more global
/// secondary indexes (the maximum is 20) to be created on the table" (the
/// `CreateTable$GlobalSecondaryIndexes` doc).
pub const MAX_GLOBAL_SECONDARY_INDEXES: usize = 20;

/// The most local secondary indexes one table may carry: "You can define a maximum of
/// 5 local secondary indexes per table" ("Service, Account, and Table Quotas" in the
/// Amazon DynamoDB Developer Guide). A local index is defined with the table and can
/// never be added later, so this is the only place the count is checked.
pub const MAX_LOCAL_SECONDARY_INDEXES: usize = 5;

/// The most `HASH` and the most `RANGE` elements one global secondary index's key
/// schema may carry: "Each global secondary index supports up to 4 partition keys and
/// up to 4 sort keys" (`CreateTable$GlobalSecondaryIndexes`).
pub const MAX_INDEX_PARTITION_KEYS: usize = 4;
pub const MAX_INDEX_SORT_KEYS: usize = 4;

/// The most key elements one index's key schema may carry, from the two counts above.
pub const MAX_INDEX_KEY_ELEMENTS: usize = MAX_INDEX_PARTITION_KEYS + MAX_INDEX_SORT_KEYS;

/// The most attribute definitions one table may declare.
///
/// "The attributes in KeySchema must also be defined in the AttributeDefinitions
/// array", and a definition no key schema uses is refused, so a table's definitions can
/// never outnumber the key attributes of its own key schema (one `HASH` and at most one
/// `RANGE`) plus those of its indexes. A request declaring more than that is already
/// invalid, and clamping it up front is what bounds the work matching definitions
/// against key schemas: both counts come from the request body.
pub const MAX_ATTRIBUTE_DEFINITIONS: usize = 2
    + MAX_GLOBAL_SECONDARY_INDEXES * MAX_INDEX_KEY_ELEMENTS
    + MAX_LOCAL_SECONDARY_INDEXES * MAX_INDEX_KEY_ELEMENTS;

/// What one table, and the whole store, may hold in items.
///
/// Real DynamoDB puts no ceiling on a table's size — it has a fleet behind it, and
/// this emulator has one process's memory — so these are the EMULATOR's own budgets,
/// not a documented AWS quota: large enough for the data a client's test writes,
/// small enough that a runaway writer is refused with a `ValidationException` naming
/// the budget rather than exhausting the host. Admission control measures against
/// [`Table::stored_bytes`], a running total each table keeps, so the check costs
/// O(tables) and never a walk over every item.
pub const MAX_ITEMS_PER_TABLE: usize = 50_000;
pub const MAX_TABLE_SIZE_BYTES: i64 = 16 * 1024 * 1024;
pub const MAX_STORE_SIZE_BYTES: i64 = 64 * 1024 * 1024;

/// The significant digits an `N` value may carry, and the decimal exponent range its
/// magnitude may fall in: "Number: ... can have up to 38 digits of precision.
/// Exponent range is 10^-130 to 10^125" ("Service, Account, and Table Quotas").
/// [`NumberKey`] normalises a value to `0.<digits> * 10^point`, so a magnitude below
/// 10^125 bounds `point` at 126 and one at or above 10^-130 bounds it at -129.
const MAX_SIGNIFICANT_DIGITS: usize = 38;
const MIN_POINT: i32 = -129;
const MAX_POINT: i32 = 126;

/// The most DIGITS one `N` value's text may carry, before normalisation strips its
/// leading and trailing zeros. This is the EMULATOR's own input-size limit, not an AWS
/// quota: AWS bounds the significant digits and the exponent (above) and normalises the
/// zeros away, so it accepts a text this refuses.
///
/// The quota above bounds the SIGNIFICANT digits and the exponent, not the text: a run
/// of zeros normalises to the value zero however long it is, so without this bound a
/// caller could store an arbitrarily long numeric text whose accounted size
/// ([`attribute_value_size`], which sizes a number from its significant digits) is one
/// byte — growing what the process retains without moving the budgets measured against
/// it. A text with more digits than 38 significant ones plus the whole exponent span
/// denotes no value inside the documented range that a shorter text does not also
/// denote, so refusing it costs a caller nothing it could not spell another way; the
/// refusal is the `ValidationException` every caller of [`NumberKey::parse`] renders,
/// and its message names this limit.
const MAX_NUMBER_TEXT_DIGITS: usize =
    MAX_SIGNIFICANT_DIGITS + (MAX_POINT - MIN_POINT) as usize;

/// One write capacity unit covers a write of up to 1 KB (DynamoDB Developer Guide,
/// "Read/write capacity mode").
const WRITE_UNIT_BYTES: i64 = 1024;

/// One read capacity unit covers one strongly consistent read of up to 4 KB; an
/// eventually consistent read of the same size costs half as much (DynamoDB
/// Developer Guide, "Read/write capacity mode").
const READ_UNIT_BYTES: i64 = 4096;

/// How a handler should refuse a request this store cannot serve.
///
/// Returned by the shared helpers below so each operation can lift it into the error
/// its own model declares (every operation here models `ResourceNotFoundException`;
/// `Validation` becomes the 400 `ValidationException` the scaffold channel renders).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// 400 `ResourceNotFoundException` — no table by that name is readable.
    NotFound(String),
    /// 400 `ValidationException` — the request contradicts the stored table.
    Validation(String),
}

impl RequestError {
    /// The message to put on the rendered error.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message) | Self::Validation(message) => message,
        }
    }
}

// =============================================================================
// How this service WORDS a refusal
//
// Three wording rules, each stated exactly once so no handler spells a refusal
// itself and no two of them drift:
//
//   * a constraint violation on an input member is reported inside the
//     `<N> validation error(s) detected: ` envelope ([`validation_message`]),
//     over one clause per violated member ([`constraint_clause`]);
//   * a clause states only facts of the REQUEST — the member's dotted input
//     path, the constraint it violated, and the rejected value where AWS echoes
//     one — and never an emulator-internal figure (a configured maximum, a byte
//     count, the operation's name);
//   * a refusal whose text AWS fixes (the item-size ceiling, the data-plane
//     `ResourceNotFoundException`) carries exactly that text and nothing
//     appended.
//
// `src/service.rs` words the framework's own constraint refusals through the
// same functions, so a bound the model declares and a bound this service checks
// itself read alike.
//
// WHY THESE PHRASES LIVE HERE rather than being borrowed from the framework.
// `aws_smithy_schema_server::constraints` owns the same sentences
// (`Violation::requirement`, `Violation::message`, `envelope`), and
// `src/service.rs` DOES call `Violation::message` for the two clause forms that
// render identically. The rest cannot be borrowed:
//
//   * `Violation::requirement` is private to that crate and `vendor/` is
//     read-only here, so the phrase cannot be reached without the sentence
//     around it;
//   * the sentence around it always interpolates a value — `Value '<v>'` when
//     the violation carries one and `Value null` when it does not — while the
//     clause AWS states for a size or magnitude bound on this service echoes
//     NO value, which is a third form that renderer cannot produce;
//   * the bounds this service checks itself (the `TableName` form, the item and
//     nesting ceilings) are not model traits at all, so they have no
//     `Violation` to render from, and `envelope` takes violations rather than
//     clauses.
//
// A violation class this crate has no phrase for is DECLINED in `service.rs`
// (`ViolationKind` is `#[non_exhaustive]`), which keeps the framework's own
// wording instead of a stale phrase from here.
// =============================================================================

/// The `ValidationException` envelope: `1 validation error detected: <clause>`
/// for one clause, `<N> validation errors detected: <clause>; <clause>` for
/// several (the plural and the `; ` join are AWS's, and the count is the number
/// of clauses stated).
///
/// Returns `None` when there is nothing to report, so a caller cannot render an
/// empty refusal.
pub fn validation_message<I>(clauses: I) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    let clauses: Vec<String> = clauses.into_iter().collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// One clause of the envelope: the member that was wrong, named by its dotted
/// input path, and the constraint it failed.
///
/// `echoed` carries the rejected value for the violations whose recorded AWS
/// wording interpolates one, and is `None` for the violations whose wording
/// names the member and the constraint only — AWS echoes a value in some
/// clauses and not others, and the choice belongs to the call site that knows
/// which member was refused, not to this renderer.
pub fn constraint_clause(path: &str, echoed: Option<&str>, requirement: &str) -> String {
    match echoed {
        Some(value) => {
            format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
        }
        None => format!("Value at '{path}' failed to satisfy constraint: {requirement}"),
    }
}

/// The requirement phrase for a `@length` minimum, worded as AWS words it. The
/// phrases below are the ONE place each requirement is spelled: `src/service.rs`
/// words the framework's violations through them too, so a bound the model
/// declares and a bound this service checks itself cannot drift apart.
pub fn min_length_requirement(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement phrase for a `@length` maximum.
pub fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement phrase for a `@range` minimum. The bound is interpolated as
/// the text it displays with, so an integral bound reads `1` and not `1.0`.
pub fn min_value_requirement(min: &str) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase for a `@range` maximum.
pub fn max_value_requirement(max: &str) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement phrase for `@uniqueItems`.
pub fn unique_items_requirement() -> String {
    "Member must have unique values".to_string()
}

/// The requirement phrase for a `@pattern`, stating the expression the value
/// must satisfy.
///
/// The expression is reported WITHOUT the `^` / `$` the model spells it with: a
/// service states the pattern a value must satisfy, not the anchoring that makes
/// the match total, so a client that re-validates locally with the expression it
/// read out of the message uses the one AWS states.
pub fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not
/// the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// A refusal of ONE input member: the envelope over a single clause.
pub fn invalid_member(path: &str, echoed: Option<&str>, requirement: &str) -> RequestError {
    refused(constraint_clause(path, echoed, requirement))
}

/// The refusal a single clause makes: the envelope over that clause, as the 400
/// `ValidationException` every caller renders.
fn refused(clause: String) -> RequestError {
    // One clause is never zero clauses, so the envelope is always rendered; the
    // bare clause is the unreachable fallback rather than a second wording.
    RequestError::Validation(validation_message([clause.clone()]).unwrap_or(clause))
}

/// The refusal for a `ReturnValues` an item write does not accept.
///
/// `PutItem` and `DeleteItem` accept only `NONE` and `ALL_OLD` of the five
/// values the `ReturnValue` enum declares ("Use ReturnValues if you want to get
/// the item attributes as they appeared before they were updated" — the
/// `PutItem$ReturnValues` / `DeleteItem$ReturnValues` docs, which list
/// `NONE | ALL_OLD`). AWS states it inside the validation envelope with its own
/// fixed clause, which names the member and the values it accepts and nothing
/// of the rejected value or the refusing operation.
pub fn return_values_must_be_old_or_none() -> RequestError {
    refused("ReturnValues can only be ALL_OLD or NONE".to_string())
}

/// The refusal for an item whose attributes nest deeper than
/// [`MAX_ATTRIBUTE_DEPTH`]: AWS's fixed clause for that quota, inside the same
/// envelope, naming the quota and nothing of which attribute reached it.
pub fn nesting_levels_exceeded() -> RequestError {
    refused(
        "Nesting Levels have exceeded supported limits: Attributes in the item have nested \
         levels beyond supported limit"
            .to_string(),
    )
}

/// The refusal for a key attribute whose value is empty.
///
/// A key attribute's value must be non-empty whatever type the table declared
/// for it — DynamoDB accepts an empty string or binary in a NON-key attribute
/// and not in a key one — and the refusal names the key it was supplied for.
/// Not a constraint-envelope refusal: AWS states it as a parameter-value
/// sentence of its own.
fn empty_key_attribute(attribute: &str, kind: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values are not valid. The AttributeValue for a key attribute \
         cannot contain an empty {kind} value. Key: {attribute}"
    ))
}

/// A table's lifecycle status, as the `TableStatus` enum the model declares names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableStatus {
    Creating,
    Updating,
    Active,
    Deleting,
}

impl TableStatus {
    /// The wire value, one of the model's `TableStatus` enum values.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Updating => "UPDATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }

    /// Whether an item operation may run against a table in this status.
    ///
    /// "You can perform read and write operations only on an ACTIVE table" (the
    /// `CreateTable` doc) rules out `CREATING` and `DELETING`; an `UpdateTable` in
    /// flight only rules out another table-level change ("While it's UPDATING, you
    /// can't issue another UpdateTable request" — the `UpdateTable` doc), so a table
    /// being updated keeps serving its items.
    pub fn serves_items(self) -> bool {
        matches!(self, Self::Active | Self::Updating)
    }
}

/// A secondary index's lifecycle status, as the model's `IndexStatus` enum names it.
///
/// `CREATING`, `UPDATING` and `DELETING` are the transient statuses the
/// `GlobalSecondaryIndexDescription$IndexStatus` doc lists ("CREATING - The index is
/// being created. UPDATING - The index is being updated. DELETING - The index is being
/// deleted. ACTIVE - The index is ready for use"); which transient status an index is
/// in is recorded on it, and the move to `ACTIVE` is owed by the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStatus {
    Creating,
    Updating,
    Active,
}

impl IndexStatus {
    /// The wire value, one of the model's `IndexStatus` enum values.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Updating => "UPDATING",
            Self::Active => "ACTIVE",
        }
    }
}

/// One component of a primary key, encoded so that `Ord` is DynamoDB's own ordering
/// for that attribute type: UTF-8 byte order for `S`, numeric order for `N`,
/// unsigned byte order for `B` (the `Condition` doc: "For Binary, DynamoDB treats
/// each byte of the binary data as unsigned when it compares binary values").
///
/// A key attribute's type is fixed by the table's `AttributeDefinitions`, so two keys
/// of one table never compare across variants; the derived cross-variant order is
/// only there to make `Ord` total.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttributeValueKey {
    B(Vec<u8>),
    N(NumberKey),
    S(String),
}

/// A primary key: the partition key value, and the sort key value when the table
/// declares a `RANGE` key.
///
/// `Ord` compares the partition component first, so a `BTreeMap<ItemKey, Item>`
/// iterates each partition's items contiguously in sort-key order — which is the
/// order `Query` returns and the order `ExclusiveStartKey` pagination resumes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ItemKey {
    pub partition: AttributeValueKey,
    pub sort: Option<AttributeValueKey>,
}

/// The role one attribute plays in a key schema, and the documented size limit that
/// goes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyRole {
    Partition,
    Sort,
}

impl KeyRole {
    /// The documented maximum size of a value in this role.
    fn max_bytes(self) -> i64 {
        match self {
            Self::Partition => MAX_PARTITION_KEY_BYTES,
            Self::Sort => MAX_SORT_KEY_BYTES,
        }
    }

    /// How DynamoDB words the refusal of an oversize value in this role.
    ///
    /// The two roles have DIFFERENT wordings, both the service's own, both naming the
    /// documented limit for the role — the partition-key one includes DynamoDB's own
    /// missing space before the number, and the sort-key one speaks of the aggregate
    /// because a table's range keys are sized together with its local indexes'.
    fn oversize_message(self) -> String {
        let limit = self.max_bytes();
        match self {
            Self::Partition => {
                format!("Size of hashkey has exceeded the maximum size limit of{limit} bytes")
            }
            Self::Sort => format!(
                "Aggregated size of all range keys has exceeded the size limit of {limit} bytes"
            ),
        }
    }
}

/// A DynamoDB `N` attribute value, parsed into sign + significant digits + decimal
/// exponent so that comparison is NUMERIC rather than textual.
///
/// DynamoDB sends numbers across the wire as strings with up to 38 significant
/// digits and an exponent, and compares them numerically, so `"10"` sorts after
/// `"9"` and `"1e2"` equals `"100"`. The value is `±0.<digits> * 10^point`, with
/// `digits` holding no leading or trailing zero; zero is `digits == []`, `point == 0`
/// and `negative == false`, so `"-0"`, `"0"` and `"0.00"` are one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberKey {
    negative: bool,
    point: i32,
    digits: Vec<u8>,
}

/// Why an `N` attribute value's text is not a number DynamoDB stores.
///
/// Each variant names the RULE the text breaks rather than lumping every bad number
/// into one refusal, so the `ValidationException` a caller gets names the violated
/// concept ([`number_rejection`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberFault {
    /// Not a number at all: the text is not `[+-]?digits[.digits][eE[+-]?digits]`.
    NotNumeric,
    /// More significant digits than the documented 38.
    Precision,
    /// A magnitude above the documented exponent range.
    Overflow,
    /// A non-zero magnitude below the documented exponent range.
    Underflow,
    /// More digits of TEXT than this emulator accepts ([`MAX_NUMBER_TEXT_DIGITS`]).
    TextTooLong,
}

impl NumberKey {
    /// Parse the textual form of an `N` attribute value, or `None` when it is not a
    /// value DynamoDB stores. Use [`Self::try_parse`] where the reason matters.
    pub fn parse(text: &str) -> Option<Self> {
        Self::try_parse(text).ok()
    }

    /// Parse the textual form of an `N` attribute value, naming the rule a refused
    /// text breaks.
    pub fn try_parse(text: &str) -> Result<Self, NumberFault> {
        let (negative, unsigned) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(at) => (
                &unsigned[..at],
                unsigned[at + 1..]
                    .parse::<i32>()
                    .map_err(|_| NumberFault::NotNumeric)?,
            ),
            None => (unsigned, 0),
        };
        let (integral, fractional) = match mantissa.find('.') {
            Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
            None => (mantissa, ""),
        };
        if integral.is_empty() && fractional.is_empty() {
            return Err(NumberFault::NotNumeric);
        }
        if !integral.bytes().chain(fractional.bytes()).all(|b| b.is_ascii_digit()) {
            return Err(NumberFault::NotNumeric);
        }
        // The TEXT is bounded before anything is normalised: the digits a longer one
        // carries cannot change which documented value it denotes, and the parsed
        // value's accounted size is computed from its significant digits, not its
        // text. Both terms come from the request.
        if integral.len().saturating_add(fractional.len()) > MAX_NUMBER_TEXT_DIGITS {
            return Err(NumberFault::TextTooLong);
        }
        let mut digits: Vec<u8> = integral
            .bytes()
            .chain(fractional.bytes())
            .map(|b| b - b'0')
            .collect();
        // The value is 0.<digits> * 10^point before normalisation: the decimal point
        // sits after the integral digits, shifted by the exponent. Both terms come
        // from the request, so every step is checked: an exponent that leaves the
        // documented range is refused as an over- or underflow, whichever direction
        // it left it in.
        let shifted = i32::try_from(integral.len())
            .ok()
            .and_then(|len| len.checked_add(exponent));
        let mut point = match shifted {
            Some(point) => point,
            None if exponent > 0 => return Err(NumberFault::Overflow),
            None => return Err(NumberFault::Underflow),
        };
        let leading_zeros = digits.iter().take_while(|digit| **digit == 0).count();
        digits.drain(..leading_zeros);
        point = match i32::try_from(leading_zeros)
            .ok()
            .and_then(|zeros| point.checked_sub(zeros))
        {
            Some(point) => point,
            None => return Err(NumberFault::Underflow),
        };
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            // Every spelling of zero normalises to the same value, sign included.
            return Ok(Self {
                negative: false,
                point: 0,
                digits,
            });
        }
        if digits.len() > MAX_SIGNIFICANT_DIGITS {
            return Err(NumberFault::Precision);
        }
        if point > MAX_POINT {
            return Err(NumberFault::Overflow);
        }
        if point < MIN_POINT {
            return Err(NumberFault::Underflow);
        }
        Ok(Self {
            negative,
            point,
            digits,
        })
    }

    /// Compare magnitudes (both treated as non-negative).
    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        match (self.digits.is_empty(), other.digits.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            // A larger decimal exponent means a larger magnitude once both are
            // normalised to 0.<digits>; equal exponents fall back to the digits,
            // whose lexicographic order IS their numeric order.
            (false, false) => self
                .point
                .cmp(&other.point)
                .then_with(|| self.digits.cmp(&other.digits)),
        }
    }

    /// The number of significant digits, which is what an `N` value's stored size is
    /// computed from.
    fn significant_digits(&self) -> i64 {
        i64::try_from(self.digits.len()).unwrap_or(i64::MAX)
    }

    /// This number's CANONICAL text — the one form every read of it reports.
    ///
    /// An `N` value travels as a string ("Numbers are sent across the network to
    /// DynamoDB as strings, to maximize compatibility across languages and libraries.
    /// However, DynamoDB treats them as number type attributes" — `AttributeValue$N`),
    /// so what is stored is the NUMBER and what a read reports is that number rendered
    /// once, not the lexeme a writer happened to send. The rendering is plain decimal
    /// over the normalised digits — never an exponent, no sign on zero, and a leading
    /// or trailing zero only where the decimal point needs one — so every spelling of
    /// one value reads back identically.
    pub fn canonical_text(&self) -> String {
        if self.digits.is_empty() {
            return "0".to_string();
        }
        let digits: String = self
            .digits
            .iter()
            .map(|digit| char::from(b'0' + digit))
            .collect();
        // The value is 0.<digits> * 10^point, so `point` is where the decimal point
        // falls among the digits: left of them all (a leading "0." and the zeros the
        // exponent asks for), right of them all (trailing zeros), or between two.
        let magnitude = match usize::try_from(self.point) {
            Err(_) | Ok(0) => {
                let zeros = usize::try_from(self.point.unsigned_abs()).unwrap_or(0);
                format!("0.{}{digits}", "0".repeat(zeros))
            }
            Ok(at) if at >= self.digits.len() => {
                format!("{digits}{}", "0".repeat(at - self.digits.len()))
            }
            Ok(at) => format!("{}.{}", &digits[..at], &digits[at..]),
        };
        if self.negative {
            format!("-{magnitude}")
        } else {
            magnitude
        }
    }
}

impl Ord for NumberKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, false) => self.magnitude_cmp(other),
            (true, true) => other.magnitude_cmp(self),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

impl PartialOrd for NumberKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One stored global secondary index: its configuration, its lifecycle instants and
/// the throughput a later `UpdateTable` can move.
///
/// An index holds no items of its own — "attributes that are copied (projected) from
/// the table into the global secondary index" (the `GlobalSecondaryIndex$Projection`
/// doc) — so `ItemCount` and `IndexSizeBytes` are DERIVED from the table's items here
/// rather than stored and bumped.
#[derive(Debug, Clone)]
pub struct GlobalSecondaryIndex {
    pub name: String,
    pub arn: String,
    pub key_schema: Vec<KeySchemaElement>,
    pub projection: Projection,
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    /// The transient status this index is in until [`Self::active_at`] falls due.
    pub transient_status: IndexStatus,
    /// When the clock owes this index its transient -> `ACTIVE` transition.
    pub active_at: SystemTime,
    /// When the last throughput increase and decrease were applied, and how many
    /// decreases have been applied today, as `ProvisionedThroughputDescription`
    /// reports them.
    pub throughput_history: ThroughputHistory,
}

impl GlobalSecondaryIndex {
    /// The status at `now`, derived from the stored instants.
    pub fn status(&self, now: SystemTime) -> IndexStatus {
        if now >= self.active_at {
            IndexStatus::Active
        } else {
            self.transient_status
        }
    }

    /// Whether `item` is projected into this index.
    fn covers(&self, item: &Item) -> bool {
        index_covers(&self.key_schema, item)
    }

    /// The bytes this index stores for one covered item.
    fn projected_size(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> i64 {
        index_projected_size(&self.key_schema, &self.projection, item, table_key_schema)
    }
}

/// One stored local secondary index: an index that shares the table's partition key
/// and orders each partition by a different sort key.
///
/// A local index is defined WITH the table and cannot be added, changed or removed
/// afterwards (`UpdateTable` carries no local-index member), and the model's
/// `LocalSecondaryIndexDescription` declares no `IndexStatus` or throughput — so unlike
/// [`GlobalSecondaryIndex`] there is no per-index lifecycle or capacity to record here.
/// Like a global index it holds no items of its own, so its `ItemCount` and
/// `IndexSizeBytes` are DERIVED from the table's items.
#[derive(Debug, Clone)]
pub struct LocalSecondaryIndex {
    pub name: String,
    pub arn: String,
    pub key_schema: Vec<KeySchemaElement>,
    pub projection: Projection,
}

impl LocalSecondaryIndex {
    /// Whether `item` is projected into this index.
    fn covers(&self, item: &Item) -> bool {
        index_covers(&self.key_schema, item)
    }

    /// The bytes this index stores for one covered item.
    fn projected_size(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> i64 {
        index_projected_size(&self.key_schema, &self.projection, item, table_key_schema)
    }
}

/// Whether `item` reaches an index with this key schema: an item is projected into a
/// secondary index only when it carries every attribute of the index's key schema
/// ("Not all items will qualify: For example, a partition key cannot have any duplicate
/// values" — the `Backfilling` doc). One rule for both index kinds, so an item is
/// counted, sized and charged for by the same test wherever it is projected.
fn index_covers(key_schema: &[KeySchemaElement], item: &Item) -> bool {
    key_schema
        .iter()
        .all(|element| item.contains_key(&element.attribute_name))
}

/// The bytes an index stores for one covered item: the attributes the index projects,
/// which are the index and table key attributes plus whatever `Projection` adds.
fn index_projected_size(
    key_schema: &[KeySchemaElement],
    projection: &Projection,
    item: &Item,
    table_key_schema: &[KeySchemaElement],
) -> i64 {
    let projection_type = projection.projection_type.as_deref().unwrap_or("ALL");
    if projection_type == "ALL" {
        return item_size(item);
    }
    let non_key = projection.non_key_attributes.as_deref().unwrap_or_default();
    let projected: Item = item
        .iter()
        .filter(|(name, _)| {
            key_schema
                .iter()
                .chain(table_key_schema)
                .any(|element| &element.attribute_name == *name)
                || (projection_type == "INCLUDE" && non_key.iter().any(|kept| kept == *name))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    item_size(&projected)
}

/// What a write costs ONE index, or `None` when that index is not affected at all.
///
/// The Developer Guide gives the rule under "Provisioned throughput considerations for
/// local secondary indexes", and the global indexes follow the same accounting:
///
/// * "If you write a new item to the table that defines an indexed attribute, or you
///   update an existing item to define a previously undefined indexed attribute, one
///   write operation is required to put the item into the index."
/// * "If an update to the table changes the value of an indexed key attribute (from A to
///   B), two writes are required, one to delete the previous item from the index and
///   another write to put the new item into the index."
/// * "If an item was present in the index, but a table write caused the indexed
///   attribute to be deleted, one write is required to delete the old item projection
///   out of the index."
/// * "If an item is not present in the index before or after the update, there is no
///   additional write cost for the index."
/// * "If an update to the table changes only the value of projected attributes ... but
///   does not change the value of any indexed key attribute, then one write is required
///   to update the values of the projected attributes into the index" — charged, like the
///   table itself, on the larger of the two projections.
fn index_write_units<'a>(
    key_schema: &[KeySchemaElement],
    projection: &Projection,
    table_key_schema: &[KeySchemaElement],
    before: Option<&'a Item>,
    after: Option<&'a Item>,
) -> Option<f64> {
    let entry = |version: Option<&'a Item>| version.filter(|item| index_covers(key_schema, item));
    let size =
        |item: &Item| index_projected_size(key_schema, projection, item, table_key_schema);
    match (entry(before), entry(after)) {
        (None, None) => None,
        (None, Some(item)) | (Some(item), None) => Some(write_capacity_units(size(item))),
        (Some(old), Some(new)) => {
            if index_key_changed(key_schema, old, new) {
                Some(write_capacity_units(size(old)) + write_capacity_units(size(new)))
            } else {
                Some(write_capacity_units(size(old).max(size(new))))
            }
        }
    }
}

/// Whether the two versions of an item differ in any of this index's key values.
fn index_key_changed(key_schema: &[KeySchemaElement], before: &Item, after: &Item) -> bool {
    key_schema.iter().any(|element| {
        !same_key_value(
            before.get(&element.attribute_name),
            after.get(&element.attribute_name),
        )
    })
}

/// Whether two values of one key attribute are the same key.
///
/// An index key attribute is always a scalar (`CreateTable` refuses a key attribute
/// whose `AttributeType` is not `S`, `N` or `B`), and an `N` key is compared NUMERICALLY
/// — `"1e2"` and `"100"` are one key, the way [`NumberKey`] orders them — so a rewrite
/// that respells a number has not changed the index key.
fn same_key_value(before: Option<&AttributeValue>, after: Option<&AttributeValue>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(AttributeValue::S(before)), Some(AttributeValue::S(after))) => before == after,
        (Some(AttributeValue::B(before)), Some(AttributeValue::B(after))) => before == after,
        (Some(AttributeValue::N(before)), Some(AttributeValue::N(after))) => {
            match (NumberKey::parse(before), NumberKey::parse(after)) {
                (Some(before), Some(after)) => before == after,
                // Neither denotes a number at all, so there is nothing to compare but
                // the text they were written as.
                _ => before == after,
            }
        }
        _ => false,
    }
}

/// What a `ProvisionedThroughputDescription` reports about past capacity changes: the
/// instants of the last increase and decrease, and the number of decreases applied in
/// the current UTC day ("The number of provisioned throughput decreases for this table
/// during this UTC calendar day").
///
/// The day's count is held as ONE `(day, count)` pair rather than a list of instants,
/// so recording a decrease is O(1) and the record cannot grow with the number of
/// decreases a caller makes: a decrease on a later day replaces the pair instead of
/// appending to it.
#[derive(Debug, Clone, Default)]
pub struct ThroughputHistory {
    pub last_increase: Option<SystemTime>,
    pub last_decrease: Option<SystemTime>,
    /// The UTC day the count below belongs to, and how many decreases were applied on
    /// it. A read on a later day reports zero without anything having had to run at
    /// midnight.
    decreases_day: i64,
    decreases_on_day: i64,
}

impl ThroughputHistory {
    /// Record a move from `before` to `after` at `now`.
    pub fn record(
        &mut self,
        now: SystemTime,
        before: &ProvisionedThroughput,
        after: &ProvisionedThroughput,
    ) {
        // A single call can raise one dimension and lower the other, so both sides are
        // tested rather than one being the negation of the other.
        if after.read_capacity_units > before.read_capacity_units
            || after.write_capacity_units > before.write_capacity_units
        {
            self.last_increase = Some(now);
        }
        if after.read_capacity_units < before.read_capacity_units
            || after.write_capacity_units < before.write_capacity_units
        {
            self.last_decrease = Some(now);
            let day = utc_day(now);
            if self.decreases_day == day {
                self.decreases_on_day = self.decreases_on_day.saturating_add(1);
            } else {
                self.decreases_day = day;
                self.decreases_on_day = 1;
            }
        }
    }

    /// How many decreases fall in the same UTC calendar day as `now`.
    pub fn decreases_today(&self, now: SystemTime) -> i64 {
        if self.decreases_day == utc_day(now) {
            self.decreases_on_day
        } else {
            0
        }
    }

    /// The description members a `ProvisionedThroughputDescription` carries.
    fn describe(&self, now: SystemTime) -> (Option<i64>, Option<::aws_smithy_types::DateTime>, Option<::aws_smithy_types::DateTime>) {
        let decreases = self.decreases_today(now);
        (
            // `NumberOfDecreasesToday` is modelled `PositiveLongObject` (range min 1),
            // so a table that has had none reports no value rather than zero.
            (decreases > 0).then_some(decreases),
            self.last_increase.map(::aws_smithy_types::DateTime::from),
            self.last_decrease.map(::aws_smithy_types::DateTime::from),
        )
    }
}

/// The UTC day `instant` falls in, as a whole number of days since the epoch.
fn utc_day(instant: SystemTime) -> i64 {
    const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs() / SECONDS_PER_DAY).unwrap_or(i64::MAX)
        })
}

/// One stored table: its configuration, its lifecycle instants, and its items.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub arn: String,
    /// The region this table lives in. A table name is unique only within a region
    /// ("table names must be unique within each Region" — the `CreateTable` doc), so
    /// the store keys by region and the table remembers which one it answers for.
    pub region: String,
    pub key_schema: Vec<KeySchemaElement>,
    pub attribute_definitions: Vec<AttributeDefinition>,
    /// `PROVISIONED` or `PAY_PER_REQUEST`, as resolved at CreateTable.
    pub billing_mode: String,
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    /// The table's own past capacity changes, as `ProvisionedThroughputDescription`
    /// reports them.
    pub throughput_history: ThroughputHistory,
    /// The global secondary indexes defined on this table, in the order they were
    /// declared, exactly as the request gave them — `DescribeTable` projects each into a
    /// `GlobalSecondaryIndexDescription` and never stores one.
    pub global_secondary_indexes: Vec<GlobalSecondaryIndex>,
    /// The local secondary indexes defined on this table, in the order they were
    /// declared. Like the global ones they are projected into a description on every
    /// read and never stored as one; unlike them they can only be defined here, at
    /// `CreateTable`.
    pub local_secondary_indexes: Vec<LocalSecondaryIndex>,
    pub table_class: Option<String>,
    pub deletion_protection_enabled: bool,
    /// The tags this table carries: the set `CreateTable` seeded and every later
    /// `TagResource`/`UntagResource` edited, and the one thing
    /// `ListTagsOfResource` reports.
    pub tags: TagSet,
    pub creation_date_time: SystemTime,
    /// When the clock owes this table its `CREATING` -> `ACTIVE` transition.
    pub active_at: SystemTime,
    /// When the clock owes this table the end of an `UpdateTable` — its `UPDATING` ->
    /// `ACTIVE` transition. `None` once no update is in flight, which is also the case
    /// for an update that asks for no capacity work at all (see `UpdateTable`).
    pub updated_at: Option<SystemTime>,
    /// When `PAY_PER_REQUEST` TAKES EFFECT as this table's read/write capacity mode,
    /// which is what `BillingModeSummary.LastUpdateToPayPerRequestDateTime` reports:
    /// "Represents the time when `PAY_PER_REQUEST` was last set as the read/write
    /// capacity mode" (that member's doc). `None` for a table that has never been
    /// on-demand.
    ///
    /// The instant stored is when the mode takes effect — the end of the `CreateTable`
    /// or `UpdateTable` that set it, not the instant that call was accepted — so a
    /// description taken while the table is still becoming on-demand reports no
    /// timestamp yet ([`Table::describe`] withholds a future one), and every later
    /// description reports the same instant. It survives a switch back to
    /// `PROVISIONED`, because the member records when the mode was LAST SET rather than
    /// which mode is current.
    pub pay_per_request_at: Option<SystemTime>,
    /// When the clock owes this table its removal, set by `DeleteTable`.
    pub removed_at: Option<SystemTime>,
    /// The resource-based policy attached to this table, parsed once when it was
    /// attached, or `None` while the table carries none. This is what the data plane
    /// evaluates before it reads or writes an item, and what `GetResourcePolicy`
    /// reports.
    pub resource_policy: Option<AttachedPolicy>,
    pub items: BTreeMap<ItemKey, Item>,
    /// The summed size of `items`, kept current as items are written and removed.
    ///
    /// This is admission-control bookkeeping ONLY: what a response reports as
    /// `TableSizeBytes` is derived from `items` by [`Table::table_size_bytes`], so a
    /// client is never told this number. Keeping it lets `PutItem` check its budget
    /// without walking the table.
    pub stored_bytes: i64,
}

impl Table {
    /// The status at `now`, derived from the stored instants.
    pub fn status(&self, now: SystemTime) -> TableStatus {
        if self.removed_at.is_some() {
            TableStatus::Deleting
        } else if now < self.active_at {
            TableStatus::Creating
        } else if self.updated_at.is_some_and(|due| now < due) {
            TableStatus::Updating
        } else {
            TableStatus::Active
        }
    }

    /// Whether the clock has already made this table's removal due.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removed_at.is_some_and(|due| now >= due)
    }

    /// How many tags this table would carry once `incoming` were merged in: a key it
    /// already holds is overwritten rather than added, so only new keys grow the set.
    pub fn tag_count_after_merging(&self, incoming: &TagSet) -> usize {
        self.tags.len()
            + incoming
                .keys()
                .filter(|key| !self.tags.contains_key(*key))
                .count()
    }

    /// Merge `incoming` into this table's tag set: "If you try to add an existing tag
    /// (same key), the existing tag value will be updated to the new value" (the
    /// `Tag$Key` doc), and every key the request does not name keeps its value.
    pub fn merge_tags(&mut self, incoming: TagSet) {
        self.tags.extend(incoming);
    }

    /// Drop exactly the tag keys named, leaving every other one alone. A key this
    /// table does not carry is nothing to remove, so the call is idempotent.
    pub fn remove_tags<'k>(&mut self, keys: impl IntoIterator<Item = &'k str>) {
        for key in keys {
            self.tags.remove(key);
        }
    }

    /// The name of the `HASH` key attribute.
    pub fn partition_key_name(&self) -> &str {
        self.key_schema
            .iter()
            .find(|element| element.key_type == "HASH")
            .map_or("", |element| element.attribute_name.as_str())
    }

    /// The name of the `RANGE` key attribute, when the table declares one.
    pub fn sort_key_name(&self) -> Option<&str> {
        self.key_schema
            .iter()
            .find(|element| element.key_type == "RANGE")
            .map(|element| element.attribute_name.as_str())
    }

    /// The `AttributeType` (`S`/`N`/`B`) declared for `attribute`.
    fn declared_type(&self, attribute: &str) -> Option<&str> {
        self.attribute_definitions
            .iter()
            .find(|definition| definition.attribute_name == attribute)
            .map(|definition| definition.attribute_type.as_str())
    }

    /// Encode one key attribute, refusing a value whose type is not the one the
    /// table declared for it, whose `S`/`B` value is empty, or whose size is past the
    /// documented limit for the role the attribute plays
    /// ([`Self::check_key_limits`]).
    pub fn key_value(
        &self,
        attribute: &str,
        value: &AttributeValue,
    ) -> Result<AttributeValueKey, RequestError> {
        // CreateTable refuses a key schema whose attributes are not all defined, so
        // a key attribute always has a declared type; an undefined one is a bad
        // request rather than something to guess a type for.
        let declared = self.declared_type(attribute).ok_or_else(|| {
            RequestError::Validation(format!(
                "No AttributeDefinition was provided for key attribute {attribute}"
            ))
        })?;
        let encoded = match (declared, value) {
            // "The AttributeValue for a key attribute cannot contain an empty
            // string value" / an empty binary value: a key attribute's value
            // must be non-empty, however the table declared its type, and the
            // refusal names the key it was supplied for.
            ("S", AttributeValue::S(text)) if text.is_empty() => {
                return Err(empty_key_attribute(attribute, "string"))
            }
            ("B", AttributeValue::B(bytes)) if bytes.is_empty() => {
                return Err(empty_key_attribute(attribute, "binary"))
            }
            ("S", AttributeValue::S(text)) => AttributeValueKey::S(text.clone()),
            ("B", AttributeValue::B(bytes)) => AttributeValueKey::B(bytes.clone()),
            ("N", AttributeValue::N(text)) => AttributeValueKey::N(
                // The numeric rules (38 significant digits, the documented exponent
                // range, and this emulator's own numeric-text limit) are the same ones
                // every `N` value in a request is held to; the refusal names the one
                // this text broke.
                NumberKey::try_parse(text).map_err(|fault| number_rejection(text, fault))?,
            ),
            (_, actual) => {
                return Err(RequestError::Validation(format!(
                    "Type mismatch for key {attribute} expected: {declared} actual: {}",
                    attribute_value_type(actual)
                )))
            }
        };
        self.check_key_limits(attribute, value)?;
        Ok(encoded)
    }

    /// The role `attribute` plays in a key schema of this table — its own, or one of
    /// its global secondary indexes — if it plays one at all.
    ///
    /// The table's own schema is consulted first: an attribute that is this table's
    /// sort key and some index's partition key is bounded as the sort key it is here.
    fn key_role(&self, attribute: &str) -> Option<KeyRole> {
        let role_in = |schema: &[KeySchemaElement]| {
            schema
                .iter()
                .find(|element| element.attribute_name == attribute)
                .map(|element| {
                    if element.key_type == "HASH" {
                        KeyRole::Partition
                    } else {
                        KeyRole::Sort
                    }
                })
        };
        role_in(&self.key_schema).or_else(|| {
            self.global_secondary_indexes
                .iter()
                .find_map(|index| role_in(&index.key_schema))
                .or_else(|| {
                    self.local_secondary_indexes
                        .iter()
                        .find_map(|index| role_in(&index.key_schema))
                })
        })
    }

    /// Refuse a key attribute value past the documented key-value size limit.
    ///
    /// The limit is the one for the role the attribute plays in a key schema
    /// ([`MAX_PARTITION_KEY_BYTES`] / [`MAX_SORT_KEY_BYTES`]). An attribute that is no
    /// key at all is the item's own business and is not bounded here; a key's empty
    /// `S`/`B` value is refused by [`Self::key_value`], which is the other half of the
    /// documented key-value contract.
    pub fn check_key_limits(
        &self,
        attribute: &str,
        value: &AttributeValue,
    ) -> Result<(), RequestError> {
        let Some(role) = self.key_role(attribute) else {
            return Ok(());
        };
        let size = attribute_value_size(value);
        if size > role.max_bytes() {
            return Err(RequestError::Validation(format!(
                "One or more parameter values were invalid: {}",
                role.oversize_message()
            )));
        }
        Ok(())
    }

    /// The primary key of an item being written: every key attribute the schema
    /// declares must be present, with its declared type. Non-key attributes are the
    /// item's own business.
    ///
    /// A missing key attribute is refused the way the service words it for an ITEM —
    /// "One of the required keys was not given a value" — which is not how it words the
    /// same gap in a `Key`-shaped member (see [`Self::key_of_key_map`]).
    pub fn key_of_item(&self, item: &Item) -> Result<ItemKey, RequestError> {
        self.key_from(item, KeySource::Item)
    }

    /// The primary key named by a `Key` member: exactly the key attributes, no more.
    ///
    /// Unlike an `Item`, a `Key` IS the primary key, so every way it can differ
    /// from the schema — an attribute the schema does not declare, a key
    /// attribute it omits, a key attribute of another type — is the one mismatch
    /// AWS states with [`KEY_SCHEMA_MISMATCH_MESSAGE`].
    pub fn key_of_key_map(&self, key: &Item) -> Result<ItemKey, RequestError> {
        for name in key.keys() {
            if name != self.partition_key_name() && Some(name.as_str()) != self.sort_key_name() {
                return Err(KeySource::Key.mismatch());
            }
        }
        self.key_from(key, KeySource::Key)
    }

    /// The one walk over the key schema both `Key` and `Item` are read through:
    /// each declared key attribute is taken from `supplied` and encoded, and
    /// `source` decides only how a missing or wrongly-typed element is WORDED.
    fn key_from(&self, supplied: &Item, source: KeySource) -> Result<ItemKey, RequestError> {
        let mut partition = None;
        let mut sort = None;
        for element in &self.key_schema {
            let name = element.attribute_name.as_str();
            let value = supplied.get(name).ok_or_else(|| source.absent())?;
            // Read out of a `Key`, an element of the wrong type does not name
            // this key either, so it is the same mismatch rather than a report
            // of the declared and the supplied type.
            if matches!(source, KeySource::Key)
                && self
                    .declared_type(name)
                    .is_some_and(|declared| declared != attribute_value_type(value))
            {
                return Err(source.mismatch());
            }
            let encoded = self.key_value(name, value)?;
            if element.key_type == "HASH" {
                partition = Some(encoded);
            } else {
                sort = Some(encoded);
            }
        }
        Ok(ItemKey {
            partition: partition.ok_or_else(|| KeySource::Key.mismatch())?,
            sort,
        })
    }

    /// Hold every INDEX key attribute the item carries to the same key-value limits.
    ///
    /// The documented limits are stated for a key attribute "for a table or index", and
    /// an item that carries an index's key attributes is projected into that index, so
    /// an oversize index key is refused on the same write the table's own key is
    /// checked on. An item that does not carry the attribute is simply not projected
    /// and is not checked for it.
    pub fn check_index_key_limits(&self, item: &Item) -> Result<(), RequestError> {
        let schemas = self
            .global_secondary_indexes
            .iter()
            .map(|index| &index.key_schema)
            .chain(
                self.local_secondary_indexes
                    .iter()
                    .map(|index| &index.key_schema),
            );
        for schema in schemas {
            for element in schema {
                if let Some(value) = item.get(&element.attribute_name) {
                    self.check_key_limits(&element.attribute_name, value)?;
                }
            }
        }
        Ok(())
    }

    /// The key attributes of a stored item, as the `Key`-shaped map
    /// `LastEvaluatedKey` reports (and a later `ExclusiveStartKey` resumes from).
    pub fn key_attributes(&self, item: &Item) -> Item {
        self.key_schema
            .iter()
            .filter_map(|element| {
                item.get_key_value(&element.attribute_name)
                    .map(|(name, value)| (name.clone(), value.clone()))
            })
            .collect()
    }

    /// The number of items stored, which is what `ItemCount` reports.
    pub fn item_count(&self) -> i64 {
        i64::try_from(self.items.len()).unwrap_or(i64::MAX)
    }

    /// The stored size of every item, which is what `TableSizeBytes` reports.
    pub fn table_size_bytes(&self) -> i64 {
        self.items.values().map(item_size).sum()
    }

    /// The index by that name, as `UpdateTable` and the index describes need it.
    pub fn index(&self, name: &str) -> Option<&GlobalSecondaryIndex> {
        self.global_secondary_indexes
            .iter()
            .find(|index| index.name == name)
    }

    /// The same, mutably, for the operations that move an index's throughput.
    pub fn index_mut(&mut self, name: &str) -> Option<&mut GlobalSecondaryIndex> {
        self.global_secondary_indexes
            .iter_mut()
            .find(|index| index.name == name)
    }

    /// Project one global secondary index into the description `DescribeTable` and the
    /// table-mutating operations report. `ItemCount` and `IndexSizeBytes` are computed
    /// from the table's items, never stored.
    fn describe_index(
        &self,
        index: &GlobalSecondaryIndex,
        now: SystemTime,
    ) -> GlobalSecondaryIndexDescription {
        let covered = self.items.values().filter(|item| index.covers(item));
        let item_count = i64::try_from(covered.clone().count()).unwrap_or(i64::MAX);
        let index_size_bytes = covered
            .map(|item| index.projected_size(item, &self.key_schema))
            .sum();
        let (number_of_decreases_today, last_increase_date_time, last_decrease_date_time) =
            index.throughput_history.describe(now);
        GlobalSecondaryIndexDescription {
            index_name: Some(index.name.clone()),
            index_arn: Some(index.arn.clone()),
            key_schema: Some(index.key_schema.clone()),
            projection: Some(index.projection.clone()),
            index_status: Some(index.status(now).as_str().to_string()),
            // "For indexes that were created during a CreateTable operation, the
            // Backfilling attribute does not appear in the DescribeTable output" (the
            // `Backfilling` doc), and this emulator projects an index's items from the
            // table on every read, so no index is ever backfilling.
            backfilling: None,
            provisioned_throughput: Some(ProvisionedThroughputDescription {
                read_capacity_units: Some(
                    index
                        .provisioned_throughput
                        .as_ref()
                        .map_or(0, |throughput| throughput.read_capacity_units),
                ),
                write_capacity_units: Some(
                    index
                        .provisioned_throughput
                        .as_ref()
                        .map_or(0, |throughput| throughput.write_capacity_units),
                ),
                number_of_decreases_today,
                last_increase_date_time,
                last_decrease_date_time,
            }),
            item_count: Some(item_count),
            index_size_bytes: Some(index_size_bytes),
            ..GlobalSecondaryIndexDescription::default()
        }
    }

    /// Project one local secondary index into the description `DescribeTable` and the
    /// table-mutating operations report. `ItemCount` and `IndexSizeBytes` are computed
    /// from the table's items by the same rule the global indexes use.
    fn describe_local_index(&self, index: &LocalSecondaryIndex) -> LocalSecondaryIndexDescription {
        let covered = self.items.values().filter(|item| index.covers(item));
        let item_count = i64::try_from(covered.clone().count()).unwrap_or(i64::MAX);
        let index_size_bytes = covered
            .map(|item| index.projected_size(item, &self.key_schema))
            .sum();
        LocalSecondaryIndexDescription {
            index_name: Some(index.name.clone()),
            index_arn: Some(index.arn.clone()),
            key_schema: Some(index.key_schema.clone()),
            projection: Some(index.projection.clone()),
            item_count: Some(item_count),
            index_size_bytes: Some(index_size_bytes),
        }
    }

    /// The capacity ONE write consumes on this table and on each secondary index the
    /// write affects, from the item that stood at the key `before` it and the item that
    /// stands there `after`.
    ///
    /// The table's own units are one per 1 KB of the item, weighing BOTH versions and
    /// charging for the larger: "PutItem — Writes a single item to a table. If an item
    /// with the same primary key exists in the table, the operation replaces the item.
    /// For calculating provisioned throughput consumption, the item size that matters is
    /// the larger of the two" ("Capacity unit consumption for write operations" in the
    /// Developer Guide). A delete weighs the removed item alone, since nothing stands at
    /// the key afterwards, and a delete that found nothing weighs neither — it still
    /// costs the table the one-unit minimum and touches no index.
    ///
    /// Each index is charged by [`index_write_units`], so an index the write leaves
    /// untouched gets no entry at all.
    pub fn write_capacity(&self, before: Option<&Item>, after: Option<&Item>) -> WriteCapacity {
        let mut capacity = WriteCapacity {
            table: write_capacity_units(before.map_or(0, item_size).max(after.map_or(0, item_size))),
            ..WriteCapacity::default()
        };
        for index in &self.local_secondary_indexes {
            if let Some(units) = index_write_units(
                &index.key_schema,
                &index.projection,
                &self.key_schema,
                before,
                after,
            ) {
                capacity
                    .local_secondary_indexes
                    .insert(index.name.clone(), units);
            }
        }
        for index in &self.global_secondary_indexes {
            if let Some(units) = index_write_units(
                &index.key_schema,
                &index.projection,
                &self.key_schema,
                before,
                after,
            ) {
                capacity
                    .global_secondary_indexes
                    .insert(index.name.clone(), units);
            }
        }
        capacity
    }

    /// The `ItemCollectionKey` of an item: "The partition key value of the item
    /// collection. This value is the same as the partition key value of the item"
    /// (`ItemCollectionMetrics$ItemCollectionKey`) — so it is the item's own partition
    /// key attribute, name and value, and nothing else.
    fn item_collection_key(&self, item: &Item) -> Item {
        item.get_key_value(self.partition_key_name())
            .map(|(name, value)| (name.clone(), value.clone()))
            .into_iter()
            .collect()
    }

    /// The bytes one item collection holds: "the size of all the items in the table,
    /// plus the size of all attributes projected into all of the local secondary indexes
    /// on that table" (`ItemCollectionMetrics$SizeEstimateRangeGB`), summed over the
    /// items that share `partition` — which is what an item collection is.
    ///
    /// One partition's items are contiguous in the store, so this is a range scan over
    /// that collection rather than a walk over the table.
    fn item_collection_bytes(&self, partition: &AttributeValueKey) -> i64 {
        let first = ItemKey {
            partition: partition.clone(),
            sort: None,
        };
        sum_of(
            self.items
                .range(first..)
                .take_while(|(key, _)| &key.partition == partition)
                .map(|(_, item)| {
                    sum_of(
                        self.local_secondary_indexes
                            .iter()
                            .filter(|index| index.covers(item))
                            .map(|index| index.projected_size(item, &self.key_schema)),
                    )
                    .saturating_add(item_size(item))
                }),
        )
    }

    /// The `ItemCollectionMetrics` for the collection the key of `item` belongs to, or
    /// `None` when this table has no local secondary index: "If the table does not have
    /// any local secondary indexes, this information is not returned in the response"
    /// (`ItemCollectionMetrics`).
    ///
    /// `item` may be a whole item or just a `Key` map — only its key attributes are
    /// read — so one rule serves every write operation.
    pub fn item_collection_metrics(&self, item: &Item) -> Option<ItemCollectionMetrics> {
        if self.local_secondary_indexes.is_empty() {
            return None;
        }
        let key = self.key_of_item(item).ok()?;
        Some(ItemCollectionMetrics {
            item_collection_key: Some(self.item_collection_key(item)),
            size_estimate_range_gb: Some(size_estimate_range_gb(
                self.item_collection_bytes(&key.partition),
            )),
        })
    }

    /// Project this table into the `TableDescription` every table operation returns.
    /// `ItemCount` and `TableSizeBytes` are derived from `items` here, never stored.
    pub fn describe(&self, now: SystemTime) -> TableDescription {
        let (number_of_decreases_today, last_increase_date_time, last_decrease_date_time) =
            self.throughput_history.describe(now);
        TableDescription {
            table_name: Some(self.name.clone()),
            table_arn: Some(self.arn.clone()),
            key_schema: Some(self.key_schema.clone()),
            attribute_definitions: Some(self.attribute_definitions.clone()),
            table_status: Some(self.status(now).as_str().to_string()),
            creation_date_time: Some(::aws_smithy_types::DateTime::from(self.creation_date_time)),
            item_count: Some(self.item_count()),
            table_size_bytes: Some(self.table_size_bytes()),
            billing_mode_summary: Some(BillingModeSummary {
                billing_mode: Some(self.billing_mode.clone()),
                // Reported only once the mode it records is in effect: the stored
                // instant is when the call that set `PAY_PER_REQUEST` settles, so while
                // that call is still settling there is nothing to report yet.
                last_update_to_pay_per_request_date_time: self
                    .pay_per_request_at
                    .filter(|in_effect_at| now >= *in_effect_at)
                    .map(::aws_smithy_types::DateTime::from),
            }),
            provisioned_throughput: Some(ProvisionedThroughputDescription {
                // A PAY_PER_REQUEST table reports zeroes: it has no provisioned
                // throughput, and no increase or decrease has ever been applied.
                read_capacity_units: Some(
                    self.provisioned_throughput
                        .as_ref()
                        .map_or(0, |throughput| throughput.read_capacity_units),
                ),
                write_capacity_units: Some(
                    self.provisioned_throughput
                        .as_ref()
                        .map_or(0, |throughput| throughput.write_capacity_units),
                ),
                number_of_decreases_today,
                last_increase_date_time,
                last_decrease_date_time,
            }),
            // A table with no secondary index reports no list at all, which is what
            // DescribeTable does for a table that has none.
            global_secondary_indexes: (!self.global_secondary_indexes.is_empty()).then(|| {
                self.global_secondary_indexes
                    .iter()
                    .map(|index| self.describe_index(index, now))
                    .collect()
            }),
            local_secondary_indexes: (!self.local_secondary_indexes.is_empty()).then(|| {
                self.local_secondary_indexes
                    .iter()
                    .map(|index| self.describe_local_index(index))
                    .collect()
            }),
            table_class_summary: self.table_class.as_ref().map(|class| TableClassSummary {
                table_class: Some(class.clone()),
                last_update_date_time: None,
            }),
            deletion_protection_enabled: Some(self.deletion_protection_enabled),
            ..TableDescription::default()
        }
    }
}

/// Per-service state for com-amazonaws-dynamodb-dynamodb-20120810. A handler reaches it through [`service_state`].
///
/// Tables are keyed by REGION and then by name, because that is the scope a DynamoDB
/// table name is unique in: "In an Amazon Web Services account, table names must be
/// unique within each Region. That is, you can have two tables with same name if you
/// create the tables in different Regions" (the `CreateTable` doc). The account is not
/// part of the key because this emulator serves exactly one account — the canonical
/// account [`::engine::emulator_arn`] mints every ARN for — so every request's tables
/// already belong to it.
#[derive(Debug, Default)]
pub struct ComAmazonawsDynamodbDynamodb20120810State {
    /// Every table this emulator holds, keyed by region and then by table name.
    pub regions: HashMap<String, HashMap<String, Table>>,
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Materialise the status changes the clock has already made due: a table whose
    /// `DeleteTable` span has elapsed is gone.
    ///
    /// This is the only write a reporting operation makes — it finishes a transition
    /// that was owed at this instant whether or not anybody called.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        for tables in self.regions.values_mut() {
            tables.retain(|_, table| !table.removal_due(now));
        }
        self.regions.retain(|_, tables| !tables.is_empty());
    }

    /// The tables of one region, which is the only scope a table name resolves in.
    pub fn tables_in(&self, region: &str) -> Option<&HashMap<String, Table>> {
        self.regions.get(region)
    }

    /// How many tables that region already holds — what the per-region table quota is
    /// measured against.
    pub fn table_count_in(&self, region: &str) -> usize {
        self.tables_in(region).map_or(0, HashMap::len)
    }

    /// The names of that region's tables, sorted, as `ListTables` reports them.
    pub fn table_names_in(&self, region: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .tables_in(region)
            .map(|tables| tables.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Whether that region already holds a table by that name.
    pub fn contains_table(&self, region: &str, name: &str) -> bool {
        self.tables_in(region)
            .is_some_and(|tables| tables.contains_key(name))
    }

    /// Whether storing a table in `region` would open a new per-region bucket beyond
    /// [`MAX_REGIONS`]. The region comes from the request's own credential scope, so
    /// the number of buckets is capped as well as the tables in each.
    pub fn region_bucket_exhausted(&self, region: &str) -> bool {
        !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS
    }

    /// Store a new table in its own region.
    pub fn insert_table(&mut self, table: Table) {
        self.regions
            .entry(table.region.clone())
            .or_default()
            .insert(table.name.clone(), table);
    }

    /// The table by that name in that region, whatever its status — what
    /// `DescribeTable` reports.
    pub fn table(&self, region: &str, name: &str) -> Result<&Table, RequestError> {
        self.tables_in(region)
            .and_then(|tables| tables.get(name))
            .ok_or_else(|| not_found(name))
    }

    /// The same, mutably, for the operations that change a table's configuration.
    pub fn table_mut(&mut self, region: &str, name: &str) -> Result<&mut Table, RequestError> {
        self.regions
            .get_mut(region)
            .and_then(|tables| tables.get_mut(name))
            .ok_or_else(|| not_found(name))
    }

    /// The table by that name in that region, which must be `ACTIVE`.
    ///
    /// An item operation on a table that is still `CREATING`, or already `DELETING`,
    /// is answered as if the table were not there: the `CreateTable` doc says "You
    /// can perform read and write operations only on an ACTIVE table".
    ///
    /// The refusal is the ITEM one ([`item_not_found`]), which names no table.
    pub fn active_table(
        &self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<&Table, RequestError> {
        let table = self
            .table(region, name)
            .map_err(|_| item_resource_not_found())?;
        if table.status(now).serves_items() {
            Ok(table)
        } else {
            Err(item_resource_not_found())
        }
    }

    /// The same, mutably, for the operations that write items.
    pub fn active_table_mut(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<&mut Table, RequestError> {
        match self.table_mut(region, name) {
            Ok(table) if table.status(now).serves_items() => Ok(table),
            _ => Err(item_resource_not_found()),
        }
    }

    /// The bytes every table together holds — the store-wide figure admission
    /// control measures against. O(tables), because each table keeps its own total.
    pub fn stored_bytes(&self) -> i64 {
        self.regions
            .values()
            .flat_map(HashMap::values)
            .map(|table| table.stored_bytes)
            .fold(0, i64::saturating_add)
    }

    /// Write one item into `name`, replacing whatever that key held.
    ///
    /// Numbers are canonicalized HERE, the store's only item-write site, so what is
    /// stored is the number each `N` denotes rather than the lexeme a writer sent, and
    /// every read path reports that one form without normalising anything itself
    /// ([`canonicalize_item`]). It runs before this method's own budget checks and
    /// after the caller has resolved the table — every handler reaches this through
    /// [`Self::active_table`]/[`Table::key_of_item`] first — so a write naming a table
    /// that is absent or not `ACTIVE` is still answered with that refusal rather than
    /// with a number's.
    ///
    /// This is the store's only growth site, so it is also where the budgets are
    /// enforced: a write that would push the table past
    /// [`MAX_ITEMS_PER_TABLE`]/[`MAX_TABLE_SIZE_BYTES`], or the store past
    /// [`MAX_STORE_SIZE_BYTES`], is refused instead of accepted. Replacing an
    /// existing item frees that item's bytes, so a steady overwrite loop is never
    /// refused.
    pub fn put_item(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
        key: ItemKey,
        mut item: Item,
    ) -> Result<Option<Item>, RequestError> {
        canonicalize_item(&mut item)?;
        let size = item_size(&item);
        let stored = self.stored_bytes();
        let table = self.active_table_mut(region, name, now)?;
        // Whether this write INSERTS is whether the key is held, not whether the item
        // it holds sizes to zero — an item can legitimately size to nothing.
        let replaced = table.items.get(&key);
        let replaced_size = replaced.map_or(0, item_size);
        if replaced.is_none() && table.items.len() >= MAX_ITEMS_PER_TABLE {
            return Err(RequestError::Validation(format!(
                "Table {name} already holds this emulator's maximum of \
                 {MAX_ITEMS_PER_TABLE} items"
            )));
        }
        let table_bytes = table
            .stored_bytes
            .saturating_sub(replaced_size)
            .saturating_add(size);
        if table_bytes > MAX_TABLE_SIZE_BYTES {
            return Err(RequestError::Validation(format!(
                "Table {name} would exceed this emulator's maximum stored size of \
                 {MAX_TABLE_SIZE_BYTES} bytes"
            )));
        }
        if stored.saturating_sub(replaced_size).saturating_add(size) > MAX_STORE_SIZE_BYTES {
            return Err(RequestError::Validation(format!(
                "The write would exceed this emulator's maximum stored size of \
                 {MAX_STORE_SIZE_BYTES} bytes across all tables"
            )));
        }
        table.stored_bytes = table_bytes;
        Ok(table.items.insert(key, item))
    }

    /// Remove one item from `name`, returning it, and give its bytes back to the
    /// table's budget.
    pub fn delete_item(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
        key: &ItemKey,
    ) -> Result<Option<Item>, RequestError> {
        let table = self.active_table_mut(region, name, now)?;
        let removed = table.items.remove(key);
        table.stored_bytes = table
            .stored_bytes
            .saturating_sub(removed.as_ref().map_or(0, item_size));
        Ok(removed)
    }
}

/// The `ResourceNotFoundException` message the CONTROL plane answers with: the
/// table-level operations (`DescribeTable`, `UpdateTable`, the tagging surface)
/// name the resource they could not find, because the caller addressed that
/// table by name and the describing surface reports it back.
fn not_found(name: &str) -> RequestError {
    RequestError::NotFound(format!("Requested resource not found: Table: {name} not found"))
}

/// The `ResourceNotFoundException` message the DATA plane answers with.
///
/// An item operation (`GetItem`, `PutItem`, `DeleteItem`, `Query`, …) against a
/// table that is absent — or present but not yet serving items — is refused with
/// AWS's FIXED text, with no resource detail appended: the one message is all a
/// client sees for "the table you addressed is not there", so nothing about this
/// store's contents (which name was looked up, whether it exists in another
/// status) leaks through the data-plane refusal.
fn item_resource_not_found() -> RequestError {
    RequestError::NotFound("Requested resource not found".to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDynamodbDynamodb20120810State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDynamodbDynamodb20120810State>)
}

/// The region this request was signed for, which is the region the ARNs it mints
/// carry. An unsigned request (authentication off) falls back to `us-east-1`, the
/// region a generated emulator defaults to.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// `arn:aws:dynamodb:<region>:<account>:table/<name>`, built through the core's ARN
/// helper so the account segment is the canonical emulator account.
pub fn table_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("dynamodb", region, &format!("table/{name}"))
}

/// `arn:aws:dynamodb:<region>:<account>:table/<table>/index/<index>` — the form the
/// `GlobalSecondaryIndexDescription$IndexArn` member carries, and the resource id
/// Application Auto Scaling names an index scalable target by.
pub fn index_arn(region: &str, table: &str, index: &str) -> String {
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!("table/{table}/index/{index}"),
    )
}

/// The form a table name must take: 3..=255 characters of
/// `[a-zA-Z0-9_.-]` (the `CreateTable$TableName` doc's own length and pattern —
/// the data-plane members target `TableArn`, which widens the modelled bound to
/// 1..=1024 so an ARN fits, while the name inside it keeps these).
const MIN_TABLE_NAME_LENGTH: usize = 3;
const MAX_TABLE_NAME_LENGTH: usize = 255;
const TABLE_NAME_PATTERN: &str = "^[a-zA-Z0-9_.-]+$";

/// How AWS's refusals name the table-name member: the `tableName` path its own
/// request validator reports this constraint against, with the rejected name
/// echoed.
///
/// NOT the wire spelling `src/service.rs` restores for a violation of a bound
/// the MODEL declares, and deliberately so: the public model does not carry this
/// bound at all (the data-plane members target `TableArn`, length 1..=1024), so
/// this is a check the service itself makes, and the recorded AWS refusals for
/// it state the validator's own form — `Value 'ab' at 'tableName' failed to
/// satisfy constraint: Member must have length greater than or equal to 3` for
/// a two-character name on `GetItem`. The same clause is stated for every
/// operation that resolves a table name, and the echoed value is the name the
/// caller sent, never a figure of this emulator's.
const TABLE_NAME_MEMBER_PATH: &str = "tableName";

/// The `TableName` member accepts a bare table name or the table's ARN ("You can
/// also provide the Amazon Resource Name (ARN) of the table in this parameter" —
/// the `CreateTable$TableName` doc). Resolve either to the bare name, refusing a
/// name outside the documented 3..=255 `[a-zA-Z0-9_.-]+` form.
///
/// A name outside that form is refused the way DynamoDB's request validator words a
/// constraint violation ([`invalid_member`]), because that is what this check is: the
/// member, the value and the constraint it broke.
pub fn resolve_table_name(table_name: &str) -> Result<String, RequestError> {
    let name = match table_name.strip_prefix("arn:") {
        Some(rest) => rest
            .splitn(5, ':')
            .nth(4)
            .and_then(|resource| resource.strip_prefix("table/"))
            .and_then(|path| path.split('/').next())
            .ok_or_else(|| {
                invalid_member(
                    TABLE_NAME_MEMBER_PATH,
                    Some(table_name),
                    &pattern_requirement(TABLE_NAME_PATTERN),
                )
            })?,
        None => table_name,
    };
    let length = name.chars().count();
    if length < MIN_TABLE_NAME_LENGTH {
        return Err(invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(name),
            &min_length_requirement(MIN_TABLE_NAME_LENGTH),
        ));
    }
    if length > MAX_TABLE_NAME_LENGTH {
        return Err(invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(name),
            &max_length_requirement(MAX_TABLE_NAME_LENGTH),
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    {
        return Err(invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(name),
            &pattern_requirement(TABLE_NAME_PATTERN),
        ));
    }
    Ok(name.to_string())
}

/// The DynamoDB resource a tagging operation's `ResourceArn` names: the region it
/// lives in and the table it is.
///
/// A tagging call carries no `TableName`, so the ARN is the caller's whole selector
/// and it names the region too — which is the scope a table name resolves in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    pub region: String,
    pub table: String,
}

/// Resolve a tagging operation's `ResourceArn` to the table it names.
///
/// The one place an ARN is classified for the tagging surface, so `TagResource`,
/// `UntagResource` and `ListTagsOfResource` share a single rule: the value must be an
/// ARN of THIS service (`arn:<partition>:dynamodb:<region>:<account>:table/<name>`)
/// naming a table and nothing deeper — a sub-resource ARN (`.../index/<i>`,
/// `.../stream/<label>`) is not a resource this emulator keeps a tag set for, so it
/// is refused rather than silently tagged as its table. A value that is not such an
/// ARN is a bad request, not a missing resource.
pub fn resolve_resource_arn(resource_arn: &str) -> Result<TaggedResource, RequestError> {
    let invalid = || {
        RequestError::Validation(format!(
            "Invalid ResourceArn: {resource_arn} is not a DynamoDB table ARN of the form \
             arn:<partition>:dynamodb:<region>:<account>:table/<name>"
        ))
    };
    // `arn`, partition, service, region, account, resource — the resource field is
    // the last one and may itself carry `/`, so the split stops at six.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let (partition, service, region, resource) = match fields.as_slice() {
        ["arn", partition, service, region, _account, resource] => {
            (*partition, *service, *region, *resource)
        }
        _ => return Err(invalid()),
    };
    if partition.is_empty() || service != "dynamodb" || region.is_empty() {
        return Err(invalid());
    }
    let path = resource.strip_prefix("table/").ok_or_else(invalid)?;
    if path.contains('/') {
        return Err(invalid());
    }
    Ok(TaggedResource {
        region: region.to_string(),
        table: resolve_table_name(path).map_err(|_| invalid())?,
    })
}

/// Fold a request's `TagList` into the stored [`TagSet`].
///
/// A key repeated inside one request keeps its LAST value, by the same rule that
/// makes a second `TagResource` overwrite the first: "If you try to add an existing
/// tag (same key), the existing tag value will be updated to the new value"
/// (`Tag$Key`). A key in the reserved `aws:` namespace is refused — the user "cannot
/// assign" it — so no stored key is ever one AWS alone mints.
pub fn tag_set_from_request(tags: &[Tag]) -> Result<TagSet, RequestError> {
    let mut set = TagSet::new();
    for tag in tags {
        if tag
            .key
            .to_ascii_lowercase()
            .starts_with(RESERVED_TAG_PREFIX)
        {
            return Err(RequestError::Validation(format!(
                "Invalid tag key {}: the {RESERVED_TAG_PREFIX} prefix is reserved for Amazon \
                 Web Services-assigned tags and cannot be assigned by a caller",
                tag.key
            )));
        }
        set.insert(tag.key.clone(), tag.value.clone());
    }
    Ok(set)
}

/// A stored tag set as the modelled `TagList` carries it, in key order.
pub fn tag_list(tags: &TagSet) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The `AttributeValue` union member name of a value, for a type-mismatch message.
pub fn attribute_value_type(value: &AttributeValue) -> &'static str {
    match value {
        AttributeValue::S(_) => "S",
        AttributeValue::N(_) => "N",
        AttributeValue::B(_) => "B",
        AttributeValue::Ss(_) => "SS",
        AttributeValue::Ns(_) => "NS",
        AttributeValue::Bs(_) => "BS",
        AttributeValue::M(_) => "M",
        AttributeValue::L(_) => "L",
        AttributeValue::Null(_) => "NULL",
        AttributeValue::Bool(_) => "BOOL",
    }
}

/// The stored size of one attribute value, by the rule the DynamoDB Developer Guide
/// gives under "Item sizes and formats": a string or binary value is its raw byte
/// length, a number is "approximately 1 byte per two significant digits + 1 byte", a
/// boolean or null is 1 byte, and a list or map is "3 bytes of overhead, plus the
/// size of each element or key-value pair" and 1 byte per element.
/// The walk is ITERATIVE, over an explicit stack: nesting in a request is chosen by
/// the caller, so recursing on it would make stack use a function of the request.
/// Sizes accumulate saturatingly, so the arithmetic here cannot overflow either.
pub fn attribute_value_size(value: &AttributeValue) -> i64 {
    let mut total: i64 = 0;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let own = match value {
            AttributeValue::S(text) => byte_len(text.len()),
            AttributeValue::B(bytes) => byte_len(bytes.len()),
            AttributeValue::N(text) => number_size(text),
            AttributeValue::Bool(_) | AttributeValue::Null(_) => 1,
            AttributeValue::Ss(values) => sum_of(values.iter().map(|text| byte_len(text.len()))),
            AttributeValue::Ns(values) => sum_of(values.iter().map(|text| number_size(text))),
            AttributeValue::Bs(values) => sum_of(values.iter().map(|bytes| byte_len(bytes.len()))),
            AttributeValue::L(values) => {
                // One byte per element, plus the elements themselves, which the
                // stack picks up rather than a recursive call.
                pending.extend(values.iter());
                3i64.saturating_add(byte_len(values.len()))
            }
            AttributeValue::M(entries) => {
                let names = sum_of(
                    entries
                        .keys()
                        .map(|name| byte_len(name.len()).saturating_add(1)),
                );
                pending.extend(entries.values());
                3i64.saturating_add(names)
            }
        };
        total = total.saturating_add(own);
    }
    total
}

/// Whether an attribute value nests deeper than `limit` levels, walked iteratively
/// for the same reason [`attribute_value_size`] is.
pub fn attribute_depth_exceeds(value: &AttributeValue, limit: usize) -> bool {
    let mut pending = vec![(value, 1usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > limit {
            return true;
        }
        match value {
            AttributeValue::L(values) => {
                pending.extend(values.iter().map(|value| (value, depth + 1)));
            }
            AttributeValue::M(entries) => {
                pending.extend(entries.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// The stored size of one item: every attribute's name plus its value.
pub fn item_size(item: &Item) -> i64 {
    sum_of(
        item.iter()
            .map(|(name, value)| byte_len(name.len()).saturating_add(attribute_value_size(value))),
    )
}

/// The `ValidationException` a bad `N` value earns, naming the rule its text breaks.
///
/// The clauses are DynamoDB's own for the documented rules — the precision one inside
/// the validation envelope ([`validation_message`]) the service states it in, the
/// magnitude ones as the standalone sentences the service answers them with. Only the
/// numeric-text limit is this emulator's, and its message says so.
pub fn number_rejection(text: &str, fault: NumberFault) -> RequestError {
    match fault {
        NumberFault::NotNumeric => RequestError::Validation(format!(
            "One or more parameter values were invalid: The parameter cannot be converted to a \
             numeric value: {}",
            clipped(text)
        )),
        NumberFault::Precision => refused(format!(
            "Attempting to store more than {MAX_SIGNIFICANT_DIGITS} significant digits in a Number"
        )),
        NumberFault::Overflow => RequestError::Validation(
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        NumberFault::Underflow => RequestError::Validation(
            "Number underflow. Attempting to store a number with magnitude smaller than \
             supported range"
                .to_string(),
        ),
        NumberFault::TextTooLong => RequestError::Validation(format!(
            "One or more parameter values were invalid: A Number may carry at most \
             {MAX_NUMBER_TEXT_DIGITS} digits of text, this emulator's input-size limit"
        )),
    }
}

/// A request value echoed back in a refusal, bounded.
///
/// The text is the caller's, so a message that embedded all of it would grow with the
/// request; the head of it is what identifies the offending value to a human.
fn clipped(text: &str) -> String {
    const KEEP: usize = 48;
    if text.chars().count() <= KEEP {
        return text.to_string();
    }
    let head: String = text.chars().take(KEEP).collect();
    format!("{head}…")
}

// THE DOCUMENTED VALUE RULES, which the canonicalizing walk below
// ([`canonicalize_attribute_value`]) enforces as it goes. None of them is expressible
// in the Smithy model, so the boundary validator passes a request that breaks them:
//
//   * a number carries at most 38 significant digits inside the documented exponent
//     range ("Number: ... can have up to 38 digits of precision" — "Service, Account,
//     and Table Quotas"), wherever it appears: an `N`, a member of an `NS`, or either
//     nested in an `L` or `M`;
//   * a set is not empty ("Set type attributes cannot be empty. Invalid Requests with
//     empty values will be rejected with a ValidationException exception" — the
//     `PutItem` doc);
//   * a set's members are distinct ("the values within a set must be unique" and "No
//     two elements can be equal" — the `AttributeValue$SS`/`NS`/`BS` docs and the
//     Developer Guide's set rules), compared the way the type compares: `NS` members
//     numerically, so `1` and `1.0` are one member.

/// Which set type a value is, for the refusals whose wording differs between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetType {
    String,
    Number,
    Binary,
}

impl SetType {
    /// The clause DynamoDB refuses an EMPTY set of this type with — including the
    /// double space where the service's template leaves the attribute path empty, and
    /// the different sentence it uses for binary sets.
    fn empty_clause(self) -> String {
        match self {
            Self::String => "An string set  may not be empty".to_string(),
            Self::Number => "An number set  may not be empty".to_string(),
            Self::Binary => "Binary sets should not be empty".to_string(),
        }
    }
}

/// Refuse an empty set, inside the same validation envelope ([`validation_message`])
/// every other clause is stated in.
fn check_set_not_empty(set_type: SetType, members: usize) -> Result<(), RequestError> {
    if members == 0 {
        return Err(refused(format!(
            "One or more parameter values were invalid: {}",
            set_type.empty_clause()
        )));
    }
    Ok(())
}

/// Refuse a set that lists one member twice.
///
/// The three set types have three wordings, each DynamoDB's own, so the rendered
/// `collection` is the caller's:
///
/// * `SS`: `Input collection ["a", "a"] contains duplicates.`
/// * `NS`: `Input collection contains duplicates.`
/// * `BS`: `Input collection ["YQ==","YQ=="]of type BS contains duplicates.`
///
/// — down to the comma spacing each uses and the missing space after the bracket in
/// the binary one.
fn duplicate_set_member(collection: String) -> RequestError {
    refused(format!(
        "One or more parameter values were invalid: Input collection \
         {collection}contains duplicates."
    ))
}

/// The collection as the `SS` message echoes it: quoted members, `, `-separated.
fn echoed_strings(values: &[String]) -> String {
    format!(
        "[{}] ",
        joined(
            values.iter().map(|member| format!("\"{}\"", clipped(member))),
            ", "
        )
    )
}

/// The collection as the `BS` message echoes it: base64 members, `,`-separated, and the
/// type named with no space after the bracket.
fn echoed_blobs(values: &[Vec<u8>]) -> String {
    format!(
        "[{}]of type BS ",
        joined(
            values.iter().map(|member| format!(
                "\"{}\"",
                clipped(&::aws_smithy_types::base64::encode(member))
            )),
            ","
        )
    )
}

/// Join at most the first few rendered members, eliding the rest.
///
/// The set's length is the caller's choice, so a message that echoed every member
/// would grow with the request.
fn joined(members: impl Iterator<Item = String>, separator: &str) -> String {
    const MAX_ECHOED_MEMBERS: usize = 8;
    let mut rendered: Vec<String> = members.take(MAX_ECHOED_MEMBERS + 1).collect();
    if rendered.len() > MAX_ECHOED_MEMBERS {
        rendered.truncate(MAX_ECHOED_MEMBERS);
        rendered.push("…".to_string());
    }
    rendered.join(separator)
}

/// Add up sizes without overflowing.
fn sum_of(sizes: impl Iterator<Item = i64>) -> i64 {
    sizes.fold(0i64, i64::saturating_add)
}

fn byte_len(len: usize) -> i64 {
    i64::try_from(len).unwrap_or(i64::MAX)
}

/// A number's stored size: 1 byte per two significant digits, plus 1 byte. An
/// unparseable value is sized by its text, which is all there is to store.
fn number_size(text: &str) -> i64 {
    match NumberKey::parse(text) {
        Some(number) => number.significant_digits() / 2 + 1,
        None => byte_len(text.len()),
    }
}

/// One `N` value's canonical stored text, or the refusal for a text that denotes no
/// number this store can hold — which names the rule the text broke
/// ([`number_rejection`]), the way the service's own refusals do.
///
/// DynamoDB stores the number, so a write normalises the lexeme ONCE here and every
/// later read reports that form ([`NumberKey::canonical_text`]).
///
/// Crate-visible: the write path reaches it through [`canonicalize_item`], and a
/// handler that needs one value canonicalized (an `UpdateItem` attribute, say) is in
/// this crate too, so nothing outside it needs this.
pub(crate) fn canonical_number_text(text: &str) -> Result<String, RequestError> {
    NumberKey::try_parse(text)
        .map(|number| number.canonical_text())
        .map_err(|fault| number_rejection(text, fault))
}

/// Canonicalize every number inside one attribute value, and hold every set to the
/// documented set rules.
///
/// The rules are about the value TYPE, not about where a value sits, so this reaches
/// the `N` of a scalar, every member of an `NS`, and every number or set nested in an
/// `L` or an `M` at any depth: a number is refused unless it is one the documented
/// precision and exponent range can hold ([`canonical_number_text`]), and a set is
/// refused if it is empty ([`check_set_not_empty`]) or lists one value twice
/// ([`duplicate_set_member`]). Set distinctness is checked AFTER canonicalization, so
/// two spellings of one number are the duplicate member DynamoDB refuses rather than
/// two members of the stored set. The walk is ITERATIVE, over an explicit stack, for
/// the reason [`attribute_value_size`]'s is: how deeply a value nests is the caller's
/// choice, so recursing on it would make stack use a function of the request.
pub(crate) fn canonicalize_attribute_value(
    value: &mut AttributeValue,
) -> Result<(), RequestError> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            AttributeValue::N(text) => *text = canonical_number_text(text)?,
            AttributeValue::Ns(members) => {
                check_set_not_empty(SetType::Number, members.len())?;
                for member in members.iter_mut() {
                    *member = canonical_number_text(member)?;
                }
                if has_duplicate(members) {
                    // The number-set wording names no members at all.
                    return Err(duplicate_set_member(String::new()));
                }
            }
            AttributeValue::Ss(members) => {
                check_set_not_empty(SetType::String, members.len())?;
                if has_duplicate(members) {
                    return Err(duplicate_set_member(echoed_strings(members)));
                }
            }
            AttributeValue::Bs(members) => {
                check_set_not_empty(SetType::Binary, members.len())?;
                if has_duplicate(members) {
                    return Err(duplicate_set_member(echoed_blobs(members)));
                }
            }
            AttributeValue::L(members) => pending.extend(members.iter_mut()),
            AttributeValue::M(entries) => pending.extend(entries.values_mut()),
            AttributeValue::S(_)
            | AttributeValue::B(_)
            | AttributeValue::Bool(_)
            | AttributeValue::Null(_) => {}
        }
    }
    Ok(())
}

/// Whether a set lists one value twice: a set holds each of its values once, so a
/// request naming one twice is a `ValidationException` rather than a stored duplicate.
///
/// Members are compared as the values they are — numerically for `NS`, because its
/// members are canonicalized first — and the check sorts BORROWED members, so it costs
/// O(n log n) and copies nothing.
fn has_duplicate<T: Ord>(members: &[T]) -> bool {
    let mut sorted: Vec<&T> = members.iter().collect();
    sorted.sort_unstable();
    sorted.windows(2).any(|pair| pair[0] == pair[1])
}

/// Canonicalize every number in one item and hold its values to the documented rules,
/// which is what a write does before the item lands.
///
/// The attributes are walked in NAME order rather than in the hash map's own order,
/// and the walk STOPS at the first refusal: two identical requests are therefore
/// refused with identical messages, and a refused item is left with the attributes
/// after the failing one untouched. The refusal is passed through as the value rule
/// worded it, because that is how the service words these: its messages name the rule
/// the value broke, not the attribute that carried it.
pub fn canonicalize_item(item: &mut Item) -> Result<(), RequestError> {
    let mut names: Vec<String> = item.keys().cloned().collect();
    names.sort_unstable();
    for name in names {
        let Some(value) = item.get_mut(&name) else {
            continue;
        };
        canonicalize_attribute_value(value)?;
    }
    Ok(())
}

/// The capacity units a write of `size` bytes consumes: one unit per 1 KB, and never
/// less than one unit.
pub fn write_capacity_units(size: i64) -> f64 {
    units_of(size, WRITE_UNIT_BYTES)
}

/// The capacity units a read of `size` bytes consumes: one unit per 4 KB when the
/// read is strongly consistent, half that when it is eventually consistent.
pub fn read_capacity_units(size: i64, consistent_read: bool) -> f64 {
    let units = units_of(size, READ_UNIT_BYTES);
    if consistent_read {
        units
    } else {
        units / 2.0
    }
}

/// How many whole units of `unit_bytes` cover `size` bytes, never fewer than one.
fn units_of(size: i64, unit_bytes: i64) -> f64 {
    let size = size.max(0);
    let partial_unit = i64::from(size % unit_bytes != 0);
    let whole_units = (size / unit_bytes + partial_unit).max(1);
    f64::from(u32::try_from(whole_units).unwrap_or(u32::MAX))
}

/// What ONE write consumed, broken down the way `ConsumedCapacity` reports it: the
/// units the table itself took, and the units each secondary index the write affected
/// took.
///
/// The per-index maps are ordered, so a write reports its indexes in one stable order
/// whatever order the table declared them in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WriteCapacity {
    /// The units consumed on the table itself.
    pub table: f64,
    /// The units consumed on each local secondary index the write affected.
    pub local_secondary_indexes: BTreeMap<String, f64>,
    /// The units consumed on each global secondary index the write affected.
    pub global_secondary_indexes: BTreeMap<String, f64>,
}

impl WriteCapacity {
    /// The aggregate `CapacityUnits`: "The total number of capacity units consumed by
    /// the operation" (`ConsumedCapacity$CapacityUnits`) — the table's units plus those
    /// of every index the write fanned out to.
    pub fn total(&self) -> f64 {
        self.local_secondary_indexes
            .values()
            .chain(self.global_secondary_indexes.values())
            .fold(self.table, |total, units| total + units)
    }
}

/// The `ConsumedCapacity` a WRITE that asked for it gets back, or `None` when it asked
/// for `NONE` (or did not ask at all).
///
/// `TOTAL` reports "only the aggregate ConsumedCapacity for the operation"; `INDEXES`
/// reports "the aggregate ConsumedCapacity for the operation, together with
/// ConsumedCapacity for each table and secondary index that was accessed" (the
/// `ReturnConsumedCapacity` doc), so the `Table` and per-index breakdown is returned
/// only then — and only for the indexes the write actually affected, since an index it
/// did not reach consumed nothing on it.
pub fn consumed_write_capacity(
    requested: Option<&str>,
    table_name: &str,
    capacity: &WriteCapacity,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    let breakdown = |units: &BTreeMap<String, f64>| -> Option<HashMap<String, Capacity>> {
        (detail && !units.is_empty()).then(|| {
            units
                .iter()
                .map(|(name, units)| {
                    (
                        name.clone(),
                        Capacity {
                            capacity_units: Some(*units),
                            ..Capacity::default()
                        },
                    )
                })
                .collect()
        })
    };
    Some(ConsumedCapacity {
        table_name: Some(table_name.to_string()),
        capacity_units: Some(capacity.total()),
        table: detail.then(|| Capacity {
            capacity_units: Some(capacity.table),
            ..Capacity::default()
        }),
        local_secondary_indexes: breakdown(&capacity.local_secondary_indexes),
        global_secondary_indexes: breakdown(&capacity.global_secondary_indexes),
        ..ConsumedCapacity::default()
    })
}

/// Whether a write reports item collection metrics: "If set to SIZE, the response
/// includes statistics about item collections, if any, that were modified during the
/// operation ... If set to NONE (the default), no statistics are returned"
/// (`PutItem$ReturnItemCollectionMetrics`). A value the model's
/// `ReturnItemCollectionMetrics` enum does not name is a bad request rather than a
/// silent `NONE`.
pub fn returns_item_collection_metrics(requested: Option<&str>) -> Result<bool, RequestError> {
    match requested {
        None | Some("NONE") => Ok(false),
        Some("SIZE") => Ok(true),
        Some(other) => Err(RequestError::Validation(format!(
            "ReturnItemCollectionMetrics can only be SIZE or NONE, got {other}"
        ))),
    }
}

/// A gigabyte, as `SizeEstimateRangeGB` denominates its bounds. DynamoDB's size quotas
/// are decimal ("The maximum size of any item collection is 10 GB" — "Service, Account,
/// and Table Quotas"), so a gigabyte here is 10^9 bytes.
const BYTES_PER_GB: i64 = 1_000_000_000;

/// The two-element `[lower, upper]` bound, in GB, that brackets `bytes`.
///
/// "This value is a two-element array containing a lower bound and an upper bound for
/// the estimate ... The estimate is subject to change over time; therefore, do not rely
/// on the precision or accuracy of the estimate"
/// (`ItemCollectionMetrics$SizeEstimateRangeGB`). The bracket reported here is the whole
/// gigabyte at or below the measured size and the next one above it, so the measured
/// size always lies inside the reported range and a collection under a gigabyte reports
/// `[0.0, 1.0]`.
pub fn size_estimate_range_gb(bytes: i64) -> Vec<f64> {
    let whole_gb = bytes.max(0) / BYTES_PER_GB;
    let lower = f64::from(u32::try_from(whole_gb).unwrap_or(u32::MAX));
    vec![lower, lower + 1.0]
}

/// The `ConsumedCapacity` a READ that asked for it gets back, or `None` when it
/// asked for `NONE` (or did not ask at all).
///
/// `INDEXES` asks for the per-table breakdown as well as the aggregate; on a base
/// table there is no index to break out, so the table's own capacity is the whole of
/// it (the `ReturnConsumedCapacity` doc: "specifying INDEXES will only return
/// ConsumedCapacity information for table(s)").
pub fn consumed_capacity(
    requested: Option<&str>,
    table_name: &str,
    units: f64,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    Some(ConsumedCapacity {
        table_name: Some(table_name.to_string()),
        capacity_units: Some(units),
        table: detail.then(|| Capacity {
            capacity_units: Some(units),
            ..Capacity::default()
        }),
        ..ConsumedCapacity::default()
    })
}

// ---------------------------------------------------------------------------
// Resource-based policies
// ---------------------------------------------------------------------------

/// Why a resource-based policy document is not one this emulator can attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyFault {
    /// Past the documented size limit, carrying the size the request sent — the
    /// modelled `LimitExceededException`.
    TooLarge(usize),
    /// Not a policy document at all, carrying what is wrong with it — a
    /// `ValidationException`.
    Invalid(String),
}

/// A resource-based policy attached to a table.
///
/// The document is kept verbatim, because `GetResourcePolicy` "returns the
/// resource-based policy document attached to the resource ... in JSON format", and
/// its statements are parsed ONCE here so evaluating a data-plane request never
/// re-parses it. The document is capped at [`MAX_RESOURCE_POLICY_BYTES`] before it is
/// parsed, which is what bounds the statements, values and patterns one request can
/// ask the matcher below for.
#[derive(Debug, Clone)]
pub struct AttachedPolicy {
    /// The document exactly as it was attached.
    pub document: String,
    /// The revision id, DERIVED from the document: "PutResourcePolicy is an idempotent
    /// operation; running it multiple times on the same resource using the same policy
    /// document will return the same revision id" (the `PutResourcePolicy` doc). A
    /// different document therefore gets a different id, and the same one re-attached
    /// gets the one it had.
    pub revision_id: String,
    /// When the clock owes this policy its application to the data plane.
    ///
    /// "PutResourcePolicy is an asynchronous operation" and "the policy application is
    /// eventually consistent" (the `PutResourcePolicy` doc), so a request made straight
    /// after the attach is still authorized as it was — while a policy that arrives
    /// WITH the table is applied at once, because "when you attach a resource-based
    /// policy while creating a table, the policy application is strongly consistent"
    /// (`CreateTable$ResourcePolicy`). The attaching operation chooses which of the two
    /// it is; nothing sleeps for the span.
    pub effective_at: SystemTime,
    /// The parsed statements, in document order.
    statements: Vec<PolicyStatement>,
}

impl AttachedPolicy {
    /// Parse `document` into the policy to attach, or say why it cannot be attached.
    ///
    /// `effective_at` is when the data plane starts honouring it (see the field).
    pub fn attach(document: &str, effective_at: SystemTime) -> Result<Self, PolicyFault> {
        if document.len() > MAX_RESOURCE_POLICY_BYTES {
            return Err(PolicyFault::TooLarge(document.len()));
        }
        let parsed: serde_json::Value = serde_json::from_str(document).map_err(|error| {
            PolicyFault::Invalid(format!("Resource-based policy is not valid JSON: {error}"))
        })?;
        let object = parsed.as_object().ok_or_else(|| {
            PolicyFault::Invalid("Resource-based policy must be a JSON object".to_string())
        })?;
        let listed: Vec<&serde_json::Value> = match object.get("Statement") {
            Some(serde_json::Value::Array(list)) => list.iter().collect(),
            Some(single @ serde_json::Value::Object(_)) => vec![single],
            _ => {
                return Err(PolicyFault::Invalid(
                    "Resource-based policy must carry a Statement".to_string(),
                ))
            }
        };
        let mut statements = Vec::with_capacity(listed.len());
        for statement in listed {
            statements.push(PolicyStatement::parse(statement)?);
        }
        Ok(Self {
            document: document.to_string(),
            revision_id: policy_revision_id(document),
            effective_at,
            statements,
        })
    }

    /// Whether this policy EXPLICITLY DENIES `action` on `resource_arn` to `caller`,
    /// and whether the statement that denied it carried a `Condition` the emulator did
    /// not evaluate.
    ///
    /// Only a Deny decides anything here. The caller is in the resource's own account —
    /// the only account this emulator serves — so a resource-based policy's Allow
    /// grants it nothing it does not already hold, while "an explicit deny in any
    /// policy overrides any allow" (the IAM policy evaluation logic). A Deny that
    /// matches the request is therefore the one outcome a resource policy changes.
    pub fn denies(&self, caller: &str, action: &str, resource_arn: &str) -> Option<bool> {
        self.statements
            .iter()
            .find(|statement| statement.denies(caller, action, resource_arn))
            .map(PolicyStatement::conditioned)
    }
}

/// The revision id of a policy document: a digest of the document, so the same
/// document always gets the same id and a different one never does.
fn policy_revision_id(document: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(document.as_bytes()))
}

/// One statement of a resource-based policy, reduced to what decides a request.
#[derive(Debug, Clone)]
struct PolicyStatement {
    /// `Effect: Deny`. An Allow statement is parsed and kept so the document round
    /// trips, but it changes no outcome (see [`AttachedPolicy::denies`]).
    deny: bool,
    principal: Selector,
    action: Selector,
    resource: Selector,
    /// Whether the statement carries a `Condition`.
    ///
    /// A condition is matched against the REQUEST CONTEXT (the source VPC, the time of
    /// day, a tag on the caller), and this emulator has no identity or network context
    /// to supply, so it cannot evaluate one. A Deny that carries one is therefore
    /// applied anyway — FAIL CLOSED: the emulator never serves a request the attached
    /// policy might refuse, which is the error that matters for a policy whose whole
    /// job is to refuse. The flag is kept so the refusal can say the condition was not
    /// evaluated.
    conditioned: bool,
}

impl PolicyStatement {
    /// Parse one statement, refusing a document this emulator would otherwise store
    /// and then not honour.
    fn parse(value: &serde_json::Value) -> Result<Self, PolicyFault> {
        let object = value.as_object().ok_or_else(|| {
            PolicyFault::Invalid("Each Statement must be a JSON object".to_string())
        })?;
        let conditioned = object.contains_key("Condition");
        let effect = object
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                PolicyFault::Invalid(
                    "Each Statement must carry an Effect of Allow or Deny".to_string(),
                )
            })?;
        let deny = if effect.eq_ignore_ascii_case("Deny") {
            true
        } else if effect.eq_ignore_ascii_case("Allow") {
            false
        } else {
            return Err(PolicyFault::Invalid(format!(
                "Statement Effect must be Allow or Deny, got {}",
                clipped(effect)
            )));
        };
        // "Resource-based policies ... require a Principal" (the IAM user guide's
        // policy grammar), so a statement without one is not a resource policy.
        let principal = Selector::parse(object, "Principal", "NotPrincipal", principal_values)?
            .ok_or_else(|| {
                PolicyFault::Invalid(
                    "Each Statement of a resource-based policy must carry a Principal".to_string(),
                )
            })?;
        let action = Selector::parse(object, "Action", "NotAction", string_values)?.ok_or_else(
            || PolicyFault::Invalid("Each Statement must carry an Action".to_string()),
        )?;
        // A resource-based policy is attached to ONE resource, so a statement that
        // names no Resource applies to the resource it is attached to.
        let resource = Selector::parse(object, "Resource", "NotResource", string_values)?
            .unwrap_or_else(Selector::everything);
        Ok(Self {
            deny,
            principal,
            action,
            resource,
            conditioned,
        })
    }

    /// Whether this statement denies the request.
    ///
    /// A `Condition` is not evaluated (see the field's doc) and does not stop a Deny
    /// from applying.
    fn denies(&self, caller: &str, action: &str, resource_arn: &str) -> bool {
        self.deny
            && self
                .principal
                .matches(|value| principal_names(value, caller))
            && self.action.matches(|pattern| action_matches(pattern, action))
            && self
                .resource
                .matches(|pattern| resource_matches(pattern, resource_arn))
    }

    /// Whether this statement's decision rests on a `Condition` the emulator did not
    /// evaluate, so a refusal can say so.
    fn conditioned(&self) -> bool {
        self.conditioned
    }
}

/// One statement element: the values it lists, and whether the statement applies to
/// those values (`Action`) or to everything BUT them (`NotAction`).
#[derive(Debug, Clone)]
struct Selector {
    values: Vec<String>,
    negated: bool,
}

impl Selector {
    /// Read `positive` or its `Not…` counterpart out of a statement. A statement that
    /// carries both forms of one element is not a policy document.
    fn parse(
        object: &serde_json::Map<String, serde_json::Value>,
        positive: &'static str,
        negative: &'static str,
        extract: fn(&serde_json::Value) -> Option<Vec<String>>,
    ) -> Result<Option<Self>, PolicyFault> {
        let (raw, negated) = match (object.get(positive), object.get(negative)) {
            (Some(_), Some(_)) => {
                return Err(PolicyFault::Invalid(format!(
                    "A Statement may carry {positive} or {negative}, not both"
                )))
            }
            (Some(value), None) => (value, false),
            (None, Some(value)) => (value, true),
            (None, None) => return Ok(None),
        };
        let values = extract(raw).ok_or_else(|| {
            PolicyFault::Invalid(format!(
                "{positive} must be a string, a list of strings, or a map of principal kinds"
            ))
        })?;
        Ok(Some(Self { values, negated }))
    }

    /// The element a statement that names none of its own carries: everything.
    fn everything() -> Self {
        Self {
            values: vec!["*".to_string()],
            negated: false,
        }
    }

    /// Whether the request's value satisfies this element, honouring the negation.
    fn matches(&self, matched: impl Fn(&str) -> bool) -> bool {
        self.values.iter().any(|value| matched(value)) != self.negated
    }
}

/// A statement element given as a string or a list of strings.
fn string_values(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(text) => Some(vec![text.clone()]),
        serde_json::Value::Array(list) => list
            .iter()
            .map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// A `Principal` element: `"*"`, or a map from principal kind to one or more values.
///
/// A kind other than `AWS` — `Service`, `Federated`, `CanonicalUser` — names no IAM
/// identity, so its values are kept WITH their kind and can match no caller rather
/// than being silently dropped.
fn principal_values(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(_) | serde_json::Value::Array(_) => string_values(value),
        serde_json::Value::Object(map) => {
            let mut values = Vec::new();
            for (kind, listed) in map {
                let listed = string_values(listed)?;
                if kind.eq_ignore_ascii_case("AWS") {
                    values.extend(listed);
                } else {
                    values.extend(listed.into_iter().map(|value| format!("{kind}:{value}")));
                }
            }
            Some(values)
        }
        _ => None,
    }
}

/// The identity every request to this emulator carries: the root of the single account
/// it serves.
///
/// The emulator verifies a request's signature but has no IAM identity behind the
/// credential it was signed with, and it serves exactly one account (the store does not
/// even key by account), so the one principal a policy can be evaluated against is
/// that account's root.
pub fn caller_principal() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// Whether a policy `Principal` value names the identity this emulator serves.
///
/// An ACCOUNT-level principal names the caller: `*`, a bare account id, or an
/// `…:root` ARN. An identity-specific principal — a user, a role, an assumed role, a
/// service — does not, because this emulator cannot tell which identity is calling.
/// The account id itself is not compared: every resource here belongs to the one
/// account whatever account id the document spells.
fn principal_names(value: &str, caller: &str) -> bool {
    if wildcard_match(value, caller) {
        return true;
    }
    let account_id = value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_digit());
    account_id || (value.starts_with("arn:") && value.ends_with(":root"))
}

/// Whether a policy `Action` value names the action being performed. IAM action names
/// are case-insensitive.
fn action_matches(pattern: &str, action: &str) -> bool {
    wildcard_match(
        &pattern.to_ascii_lowercase(),
        &action.to_ascii_lowercase(),
    )
}

/// Whether a policy `Resource` value names the resource the request names.
///
/// The partition, region and account segments of an ARN are not compared: this
/// emulator serves one partition and one account, and a policy is attached to a table
/// in a known region, so what distinguishes one resource from another here is the
/// SERVICE and the resource path (`table/<name>`, `table/<name>/index/<index>`).
fn resource_matches(pattern: &str, resource_arn: &str) -> bool {
    if wildcard_match(pattern, resource_arn) {
        return true;
    }
    match (arn_tail(pattern), arn_tail(resource_arn)) {
        (Some(pattern), Some(actual)) => wildcard_match(&pattern, &actual),
        _ => false,
    }
}

/// The service and resource path of an ARN — `dynamodb:table/x` — or `None` when the
/// value is not an ARN at all.
fn arn_tail(arn: &str) -> Option<String> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let (_partition, service) = (fields.next()?, fields.next()?);
    let (_region, _account) = (fields.next()?, fields.next()?);
    let resource = fields.next()?;
    Some(format!("{service}:{resource}"))
}

/// IAM's glob matching: `*` stands for any run of characters and `?` for exactly one.
///
/// Iterative with a single backtrack point, so the work is bounded by the lengths of
/// the two strings rather than by their structure; both come from a document already
/// capped at [`MAX_RESOURCE_POLICY_BYTES`].
fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut at_pattern, mut at_value) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;
    while at_value < value.len() {
        if at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == value[at_value])
        {
            at_pattern += 1;
            at_value += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some(at_pattern);
            resume = at_value;
            at_pattern += 1;
        } else if let Some(at_star) = star {
            at_pattern = at_star + 1;
            resume += 1;
            at_value = resume;
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|glob| *glob == '*')
}

/// The `AccessDeniedException` message a data-plane request earns when the table's
/// resource-based policy explicitly denies the caller that action, or `None` when
/// nothing denies it.
///
/// `action` is the IAM action the operation performs (`dynamodb:GetItem`). A table
/// with no policy attached — or one whose application is not due yet — denies nothing,
/// which is why this is the whole check an operation makes.
pub fn policy_denial(table: &Table, action: &str, now: SystemTime) -> Option<String> {
    let policy = table.resource_policy.as_ref()?;
    // A policy whose application is not due yet authorizes nothing: the attach was
    // asynchronous (see `AttachedPolicy::effective_at`).
    if now < policy.effective_at {
        return None;
    }
    let caller = caller_principal();
    let conditioned = policy.denies(&caller, action, &table.arn)?;
    let mut message = format!(
        "User: {caller} is not authorized to perform: {action} on resource: {} with an \
         explicit deny in a resource-based policy",
        table.arn
    );
    if conditioned {
        // Say what was not evaluated rather than imply the condition matched.
        message.push_str(
            " (the statement carries a Condition, which this emulator does not evaluate: \
             it refuses rather than serves what the policy may deny)",
        );
    }
    Some(message)
}


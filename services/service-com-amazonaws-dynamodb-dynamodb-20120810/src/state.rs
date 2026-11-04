// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dynamodb-dynamodb-20120810 service.
//!
//! This is the DynamoDB table + item store every handler in `src/ops/` reads and
//! writes, plus the pure helpers several of them share (key extraction, the
//! order-preserving key encoding, item sizing, capacity accounting, the expression
//! language the item writes share and the `TableDescription` projection). Nothing here
//! is a canned response: every value an operation reports is computed from what a
//! previous request wrote.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bigdecimal::BigDecimal;

use crate::ops::types::{
    AttributeDefinition, AttributeValue, AttributeValueUpdate, BackupDescription, BackupDetails,
    BackupSummary, BillingModeSummary, CancellationReason, Capacity, Condition,
    ConsumedCapacity, ContinuousBackupsDescription, ContributorInsightsSummary,
    ExpectedAttributeValue,
    ExportSummary,
    GlobalSecondaryIndexDescription, GlobalSecondaryIndexInfo,
    GlobalSecondaryIndexWarmThroughputDescription,
    ImportSummary,
    ItemCollectionMetrics, KeySchemaElement, KinesisDataStreamDestination,
    LocalSecondaryIndexDescription,
    LocalSecondaryIndexInfo,
    PointInTimeRecoveryDescription, Projection,
    ProvisionedThroughput, ProvisionedThroughputDescription, RestoreSummary, S3BucketSource,
    ProvisionedThroughputExceededException,
    SearchSchemaElement,
    SourceTableDetails,
    OnDemandThroughput,
    SourceTableFeatureDetails, StreamSpecification, Tag,
    TableClassSummary, TableDescription, TableThroughputModeSummary,
    TableWarmThroughputDescription, TimeToLiveDescription, VectorAttributeDefinition,
    ThrottlingReason,
    VectorIndexDescription,
    VectorIndexInfo,
    WarmThroughput,
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
/// long as it takes and the model states no figure, so the span is this emulator's:
/// it is the ONE span every clock-owed transition of this project takes, three
/// seconds, which is long enough that a caller walking a whole sequence of calls
/// after the one that started the transition still observes the transient status and
/// the state errors that go with it — a `DELETING` table stays describable while the
/// caller polls it and retries the delete — rather than vanishing between two of its
/// own requests. The due instant is stored on the table and materialised when a later
/// request reads it: nothing sleeps for it.
pub const STATUS_TRANSITION: Duration = Duration::from_secs(3);

/// The maximum size of one item, 400 KB ("Service, Account, and Table Quotas" in the
/// Amazon DynamoDB Developer Guide: "The maximum item size in DynamoDB is 400 KB").
pub const MAX_ITEM_SIZE_BYTES: i64 = 400 * 1024;

/// The most data ONE page of a read that pages carries, 1 MB — the documented
/// dataset limit of a `Query` ("A single Query operation can retrieve a maximum of 1
/// MB of data" — `Query$Limit` and "Paginating table query results") and of a `Scan`
/// ("If the total size of scanned items exceeds the maximum dataset size limit of 1
/// MB, the scan completes and results are returned to the user" — the `Scan` doc).
///
/// A page that stops here reports a `LastEvaluatedKey`, so the caller pages on and
/// one request cannot amplify into a response the size of a table. It lives here
/// because it is a property of this STORE rather than of either operation, so the two
/// readers cannot drift apart.
pub const MAX_PAGE_BYTES: i64 = 1024 * 1024;

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

/// How an `ExclusiveStartKey` that is not a key of the schema being paged is
/// refused. A resume token is not an item selector: a caller gets it from the
/// previous page's `LastEvaluatedKey`, so AWS states the token INVALID rather
/// than reporting the key-schema mismatch it words for a `Key` member.
pub const START_KEY_INVALID_MESSAGE: &str = "The provided starting key is invalid";

/// Which request member a primary key is being read out of.
///
/// The walk over the key schema is the same for all three; only the WORDING of a
/// missing or wrongly-typed element differs, which is what this carries. A
/// `Key` IS the primary key, so any way it differs from the schema is the one
/// mismatch; an `Item` is the whole item, so a key attribute it does not carry
/// is reported as the value it did not give; an `ExclusiveStartKey` is a resume
/// token, so any way it differs from the schema makes it an invalid token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
    Item,
    Key,
    StartKey,
}

impl KeySource {
    /// How this source words a key attribute the schema declares and the
    /// request did not carry.
    fn absent(self) -> RequestError {
        match self {
            Self::Item => RequestError::Validation(MISSING_KEY_VALUE_MESSAGE.to_string()),
            Self::Key | Self::StartKey => self.mismatch(),
        }
    }

    /// The one sentence a key that is not this table's primary key is refused
    /// with, in the terms the member it was read out of is stated in.
    fn mismatch(self) -> RequestError {
        match self {
            Self::StartKey => RequestError::Validation(START_KEY_INVALID_MESSAGE.to_string()),
            Self::Item | Self::Key => {
                RequestError::Validation(KEY_SCHEMA_MISMATCH_MESSAGE.to_string())
            }
        }
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

/// The most non-key attributes a table's secondary indexes may project BETWEEN THEM:
/// "the total count of NonKeyAttributes summed across all of the secondary indexes, must
/// not exceed 100. If you project the same attribute into two different indexes, this
/// counts as two distinct attributes when determining the total"
/// (`Projection$NonKeyAttributes`).
pub const MAX_PROJECTED_NON_KEY_ATTRIBUTES: usize = 100;

/// Hold a table's secondary-index projections to that budget.
///
/// Counted the way the doc counts: one entry per NAME PER INDEX, so a name projected
/// into two indexes spends two of the budget. `ALL` and `KEYS_ONLY` projections name no
/// attributes and spend none — "This limit only applies when you specify the
/// ProjectionType of INCLUDE".
pub fn check_projection_budget<'p>(
    projections: impl IntoIterator<Item = &'p Projection>,
) -> Result<(), RequestError> {
    let projected: usize = projections
        .into_iter()
        .map(|projection| {
            projection
                .non_key_attributes
                .as_ref()
                .map_or(0, |attributes| attributes.len())
        })
        .sum();
    if projected > MAX_PROJECTED_NON_KEY_ATTRIBUTES {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: Number of projected attributes in all \
             indexes exceeds limit of {MAX_PROJECTED_NON_KEY_ATTRIBUTES}, number of projected \
             attributes:{projected}"
        )));
    }
    Ok(())
}

/// The most `HASH` and the most `RANGE` elements a TABLE's primary key may carry: "A
/// simple primary key would be represented by one `KeySchemaElement` (for the partition
/// key). A composite primary key would require one `KeySchemaElement` for the partition
/// key, and another `KeySchemaElement` for the sort key" (the `KeySchemaElement` doc).
pub const MAX_TABLE_PARTITION_KEYS: usize = 1;
pub const MAX_TABLE_SORT_KEYS: usize = 1;

/// The most key elements a table's primary key may carry, from the two counts above.
pub const MAX_TABLE_KEY_ELEMENTS: usize = MAX_TABLE_PARTITION_KEYS + MAX_TABLE_SORT_KEYS;

/// The most `HASH` and the most `RANGE` elements one global secondary index's key
/// schema may carry: "Each global secondary index supports up to 4 partition keys and
/// up to 4 sort keys" (`CreateTable$GlobalSecondaryIndexes`).
pub const MAX_INDEX_PARTITION_KEYS: usize = 4;
pub const MAX_INDEX_SORT_KEYS: usize = 4;

/// The most key elements one index's key schema may carry, from the two counts above.
pub const MAX_INDEX_KEY_ELEMENTS: usize = MAX_INDEX_PARTITION_KEYS + MAX_INDEX_SORT_KEYS;

/// The most vector indexes one table may carry.
///
/// AWS documents no ceiling for `CreateTable$VectorIndexes`, so this one is this
/// emulator's own: the global secondary index quota over again
/// ([`MAX_GLOBAL_SECONDARY_INDEXES`]), since a vector index is another index on the same
/// table. A request past it is refused rather than storing a list whose length is the
/// request's to choose.
pub const MAX_VECTOR_INDEXES: usize = MAX_GLOBAL_SECONDARY_INDEXES;

/// The most elements one vector index's `SearchSchema` may carry.
///
/// The model bounds that list below ("@length min 1") and not above, so this ceiling is
/// this emulator's own: the widest key schema this service accepts for an index
/// ([`MAX_INDEX_KEY_ELEMENTS`]), because a search schema plays the same part for a vector
/// index — "the search schema that defines partition key and inline filter attributes"
/// (`VectorIndex$SearchSchema`). It is what bounds the work of matching a request's
/// attribute definitions against its vector indexes, since both counts come from the
/// request body.
pub const MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS: usize = MAX_INDEX_KEY_ELEMENTS;

/// The most attribute definitions one table may declare.
///
/// "The attributes in KeySchema must also be defined in the AttributeDefinitions
/// array", and a definition no schema uses is refused, so a table's definitions can
/// never outnumber the key attributes of its own key schema (one `HASH` and at most one
/// `RANGE`) plus those of its indexes, plus the attributes its vector indexes consume. A
/// request declaring more than that is already invalid, and clamping it up front is what
/// bounds the work matching definitions against those schemas: both counts come from the
/// request body.
pub const MAX_ATTRIBUTE_DEFINITIONS: usize = MAX_TABLE_KEY_ELEMENTS
    + MAX_GLOBAL_SECONDARY_INDEXES * MAX_INDEX_KEY_ELEMENTS
    + MAX_LOCAL_SECONDARY_INDEXES * MAX_INDEX_KEY_ELEMENTS
    + MAX_VECTOR_INDEXES * (1 + MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS);

/// What the whole store, and any one table, may hold in items.
///
/// Real DynamoDB puts no ceiling on a table's size or on how many items it holds — it has
/// a fleet behind it — so no legal item write is refused for how much data a table already
/// carries: the only size a write earns a refusal for is the documented per-item limit
/// ([`MAX_ITEM_SIZE_BYTES`]). This emulator has one process's memory, and the item store is
/// the one thing a caller can grow without limit, so these are its own HOST-PROTECTION
/// ceilings: deliberately far above the data any test stores — sixty-four times the 16 MiB
/// a table could hold before, which is what made the old ceiling reachable by a legal
/// workload — and low enough that a runaway writer is refused rather than taking the
/// machine down with it. They are measured against [`Table::stored_bytes`] and
/// [`ComAmazonawsDynamodbDynamodb20120810State::stored_bytes`], running totals each table
/// keeps, so the check costs O(tables) and never a walk over every item, and they are
/// enforced at the store's ONE growth site
/// ([`ComAmazonawsDynamodbDynamodb20120810State::put_item`]) so every write path — the item
/// API, the batch and transactional APIs, PartiQL and a restore — is bounded before it
/// allocates.
pub const MAX_ITEMS_PER_TABLE: usize = 1_000_000;
pub const MAX_TABLE_SIZE_BYTES: i64 = 1024 * 1024 * 1024;
pub const MAX_STORE_SIZE_BYTES: i64 = 2 * 1024 * 1024 * 1024;

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

/// The most CHARACTERS one `N` value's EXPONENT text may carry, sign included: the
/// widest decimal spelling of the `i32` the exponent is read into.
///
/// The same input-size limit as [`MAX_NUMBER_TEXT_DIGITS`], for the same reason, applied
/// to the other half of the lexeme: an exponent is read with `parse::<i32>`, which
/// accepts any run of leading zeros, so a bound on the exponent's VALUE (the documented
/// range, enforced below as an over- or underflow) bounds no amount of exponent TEXT. An
/// exponent spelled wider than an `i32` denotes a magnitude far outside the documented
/// range whatever the mantissa shifts it by, so refusing it costs a caller nothing it
/// could not spell another way — and it keeps the text one value retains bounded, which
/// is what the byte budgets measured against a number's accounted size
/// ([`attribute_value_size`], which sizes a number from its significant digits) rely on.
const MAX_EXPONENT_TEXT_CHARS: usize = 11;

/// How often the figures a description reports as `ItemCount` and
/// `TableSizeBytes`/`IndexSizeBytes` are recomputed from what is stored.
///
/// These members are a METERING SNAPSHOT, not a live count: "DynamoDB updates this
/// value approximately every six hours. Recent changes might not be reflected in this
/// value" (the `TableDescription$ItemCount`, `$TableSizeBytes` and
/// `GlobalSecondaryIndexDescription$IndexSizeBytes` docs). The interval is the
/// documented one, so a client is never handed an exact count the real service does not
/// give; the refresh is materialised by [`ComAmazonawsDynamodbDynamodb20120810State::apply_due_transitions`]
/// when the clock has already made it due, and nothing on the write path moves it.
pub const COUNTER_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The warm throughput a resource with NO provisioned capacity keeps warm until a
/// request asks for another: "All newly created DynamoDB tables and global secondary
/// indexes have a default warm throughput value of 12,000 read units per second and
/// 4,000 write units per second" (DynamoDB Developer Guide, "Warm throughput"). A
/// resource that IS provisioned keeps its provisioned capacity warm instead — see
/// [`derived_warm_throughput`].
pub const DEFAULT_WARM_READ_UNITS_PER_SECOND: i64 = 12_000;
pub const DEFAULT_WARM_WRITE_UNITS_PER_SECOND: i64 = 4_000;

/// The provisioned-capacity quotas an account holds in one Region BEFORE it asks for an
/// increase, in capacity units.
///
/// "Per account – 80,000 read request units and 80,000 write request units" and "Per
/// table – 40,000 read request units and 40,000 write request units" ("Service, Account,
/// and Table Quotas in Amazon DynamoDB", "Throughput default quotas"). The per-table pair
/// restricts "the sum of the provisioned capacity of the new table itself and all its
/// global secondary indexes", and the per-account pair the "aggregate provisioned capacity
/// over all your tables and GSIs" (the `DescribeLimits` doc) — which is what
/// [`ComAmazonawsDynamodbDynamodb20120810State::capacity_quotas_in`] measures against what
/// a Region has already provisioned.
pub const DEFAULT_ACCOUNT_MAX_READ_CAPACITY_UNITS: i64 = 80_000;
pub const DEFAULT_ACCOUNT_MAX_WRITE_CAPACITY_UNITS: i64 = 80_000;
pub const DEFAULT_TABLE_MAX_READ_CAPACITY_UNITS: i64 = 40_000;
pub const DEFAULT_TABLE_MAX_WRITE_CAPACITY_UNITS: i64 = 40_000;

/// How much unused capacity a provisioned table keeps for a burst: "DynamoDB currently
/// retains up to 300 seconds (5 minutes) of unused read and write capacity" (DynamoDB
/// Developer Guide, "Using burst capacity effectively"). It is the CEILING on what the
/// budget in [`CapacityBudget`] can hold, so a table that has been idle admits a burst
/// of this many seconds' worth of writes at once and no more.
pub const BURST_CAPACITY_WINDOW: Duration = Duration::from_secs(300);

/// The write capacity ONE partition key value can consume per second: "a single
/// partition can support a maximum of 3,000 read capacity units or 1,000 write capacity
/// units" (DynamoDB Developer Guide, "Designing partition keys to distribute your
/// workload evenly"), and every item that shares a partition key value lives in one
/// partition — so this is the ceiling a hot key hits however much capacity its table
/// has.
///
/// It is an INTERNAL limit of the storage layer rather than a table setting, so it
/// bounds a table in either capacity mode.
pub const PARTITION_WRITE_UNITS_PER_SECOND: i64 = 1000;

/// Over how long a window the per-partition-key ceiling above is measured.
///
/// CHOSEN by this emulator, not documented: the ceiling is stated per second and no
/// published rule promises a partition-level burst credit (the documented 300-second
/// credit in [`BURST_CAPACITY_WINDOW`] is a TABLE-level allowance), so the smallest
/// window the ceiling is stated over is the one enforced. A key is therefore admitted
/// [`PARTITION_WRITE_UNITS_PER_SECOND`] units of writes in any one second and refused
/// past that, while a key that has been idle for a second starts again with the whole
/// ceiling behind it.
pub const PARTITION_BURST_WINDOW: Duration = Duration::from_secs(1);

/// How far ahead of its own refill a write budget may be drawn when a batch write must
/// make progress ([`Table::admit_item_write_with_progress`]).
///
/// CHOSEN by this emulator, not documented: it is the capacity the storage layer behind
/// ONE partition can serve in a second ([`PARTITION_WRITE_UNITS_PER_SECOND`] over
/// [`PARTITION_BURST_WINDOW`]), which is the most work a single partition can do ahead
/// of the provisioning that pays for it. Borrowed units are charged and repay at the
/// rate in force, so this bounds only the burst a progress allowance can add, never the
/// sustained rate a table admits.
fn progress_borrowing_allowance() -> f64 {
    burst_ceiling(PARTITION_WRITE_UNITS_PER_SECOND, PARTITION_BURST_WINDOW)
}

/// How many partition key values one table keeps a write budget for at a time.
///
/// Admission-control bookkeeping has to be bounded like everything else this store
/// holds: a budget that has gone a whole [`PARTITION_BURST_WINDOW`] without a write has
/// refilled to the ceiling and is indistinguishable from a key never written to, so
/// those entries are dropped once the map reaches this size ([`KeyWriteBudgets`]).
const MAX_TRACKED_WRITE_KEYS: usize = 4096;

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
//     clause AWS states for a size or shape bound on this service echoes
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

/// The requirement phrase for a `@range` minimum, over the bound the model
/// declares (held as the text it displays with, so an integral bound reads `1`
/// rather than `1.0`).
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

/// The requirement phrase for a member whose value must come from an enumerated
/// set, worded as AWS's request validator words it, over the values the model
/// declares for that member in the order it declares them.
pub fn enum_value_requirement(values: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
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

/// How a `Query` that states neither of the two members a key condition can be
/// written in is refused: AWS's fixed sentence for the pair, which states the two
/// member names and that the request must carry one of them.
pub const KEY_CONDITION_REQUIRED_MESSAGE: &str = "Either the KeyConditions or \
     KeyConditionExpression parameter must be specified in the request.";

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

/// The word a refusal names the KIND of an empty key value by, for the value
/// types that can be empty at all (`S` and `B`); `None` for every other value,
/// which cannot be empty in the sense the key rule speaks about.
///
/// The one place the two words are chosen: [`empty_key_attribute`] and
/// [`empty_index_key_attribute`] state the same word for a table key and for a
/// secondary-index key.
fn empty_key_value_kind(value: &AttributeValue) -> Option<&'static str> {
    match value {
        AttributeValue::S(text) if text.is_empty() => Some("string"),
        AttributeValue::B(bytes) if bytes.is_empty() => Some("binary"),
        _ => None,
    }
}

/// The refusal for an item whose SECONDARY-INDEX key attribute is empty.
///
/// "Attribute values of type String and Binary must have a length greater than
/// zero if the attribute is used as a key attribute for a table or index" (the
/// `PutItem$Item` doc), so an empty value is refused for an index key exactly as
/// it is for the table's own key — but the sentence names the INDEX whose key
/// schema the attribute belongs to and the attribute's role in it, because that
/// is what tells the caller which projection it broke.
fn empty_index_key_attribute(attribute: &str, kind: &str, index_name: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values are not valid. A value specified for a secondary index \
         key is not supported. The AttributeValue for a key attribute cannot contain an empty \
         {kind} value. IndexName: {index_name}, IndexKey: {attribute}"
    ))
}

/// The refusal for an item whose SECONDARY-INDEX key attribute carries a type
/// other than the one the table declared for it.
///
/// "If you specify any attributes that are part of an index key, then the data
/// types for those attributes must match those of the schema in the table's
/// attribute definition" (the `PutItem$Item` doc), so the write is refused
/// rather than landing an item the index could not hold. Like
/// [`empty_key_attribute`] this is a parameter-value sentence of its own rather
/// than a constraint-envelope clause, and it states only facts of the REQUEST
/// and of the table it was addressed to: the index key attribute, the type the
/// table declares for it, the type the item supplied, and the index whose key
/// schema the attribute belongs to.
fn index_key_type_mismatch(
    attribute: &str,
    expected: &str,
    actual: &str,
    index_name: &str,
) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Type mismatch for Index Key {attribute} \
         Expected: {expected} Actual: {actual} IndexName: {index_name}"
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

/// Where one item sits in the order a read of a SECONDARY INDEX walks it: the index's
/// own key, and then the table's primary key.
///
/// `Ord` compares the index key first, so sorting by this orders an index's entries the
/// way a read of that index reports them — each index partition contiguous, in index
/// sort-key order — and the table's primary key breaks the tie, because nothing makes a
/// secondary index's key unique and two items that share one are still two entries. It
/// is also exactly the pair of keys an index read reports as `LastEvaluatedKey` and
/// resumes from ([`Table::index_key_attributes`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndexItemKey {
    /// The index's key values: the `HASH` elements it declares, then the `RANGE` ones,
    /// each group in the order the index declared it. An index may declare several of
    /// each ([`MAX_INDEX_PARTITION_KEYS`]), so this is a list rather than a pair.
    pub index: Vec<AttributeValueKey>,
    /// The primary key of the table item this entry projects.
    pub table: ItemKey,
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
            Some(at) => {
                let text = &unsigned[at + 1..];
                // The exponent's TEXT is bounded before it is parsed, exactly as the
                // mantissa's is below: `parse::<i32>` reads through any number of
                // leading zeros, so nothing else bounds how much text one exponent can
                // carry ([`MAX_EXPONENT_TEXT_CHARS`]).
                if text.len() > MAX_EXPONENT_TEXT_CHARS {
                    return Err(NumberFault::TextTooLong);
                }
                (
                    &unsigned[..at],
                    text.parse::<i32>().map_err(|_| NumberFault::NotNumeric)?,
                )
            }
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
    /// once, not the lexeme a writer happened to send. It is also the form a value
    /// COMPUTED by the service carries (an `ADD`ed or `SET`-arithmetic number), so one
    /// value has one spelling however the operands were written. The rendering is plain
    /// decimal over the normalised digits — never an exponent, no sign on zero, and a
    /// leading or trailing zero only where the decimal point needs one — so every
    /// spelling of one value reads back identically.
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
/// doc) — so its `ItemCount` and `IndexSizeBytes` are computed from the TABLE's items,
/// on the metering schedule [`MeteredCounters`] describes rather than per write.
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
    /// The warm throughput in force for this index, which its description reports as
    /// `WarmThroughput`. No request can name one per index (`CreateTable`/`UpdateTable`
    /// refuse that member with the scaffold's 501), so it follows the index's own
    /// provisioned capacity until that capacity is withdrawn — see
    /// [`WarmThroughputSettings::tracking_capacity`].
    pub warm_throughput: WarmThroughputSettings,
    /// The metered figures this index's `ItemCount` and `IndexSizeBytes` report,
    /// refreshed with the table's on the documented schedule.
    pub reported_counters: MeteredCounters,
    /// This index's own contributor insights setting: an index is named by the
    /// contributor insights operations through `IndexName` beside the table, so each one
    /// carries its own ([`ContributorInsights`]).
    pub contributor_insights: ContributorInsights,
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
}

/// The `SearchSchemaElementType` of a search-schema element that PARTITIONS a vector
/// index: "HASH - A partition key that partitions the vector index for independent
/// scaling. When specified, you must provide this attribute's value in the
/// SearchConditionExpression" (`SearchSchemaElement$SearchSchemaElementType`).
///
/// An element typed this way is a key of the index, so only an item that carries it
/// reaches the index; the other value, `INLINE_FILTER`, is "an attribute projected into
/// the vector index for filtering at the storage layer during search", which an item may
/// leave out.
pub const SEARCH_SCHEMA_PARTITION_KEY: &str = "HASH";

/// One stored vector index: the vector attribute it indexes, the schema it is searched
/// through, and its lifecycle instants.
///
/// Like a secondary index it holds no items of its own — "attributes that are copied
/// (projected) from the table into the vector index" (`VectorIndex$Projection`) — so its
/// `ItemCount` and `IndexSizeBytes` are metered from the TABLE's items on the schedule
/// [`MeteredCounters`] describes rather than per write.
#[derive(Debug, Clone)]
pub struct VectorIndex {
    pub name: String,
    pub arn: String,
    /// "The vector attribute configuration for the index" (`VectorIndex$VectorAttribute`):
    /// the attribute whose value is the vector each indexed item is found by.
    pub vector_attribute: VectorAttributeDefinition,
    /// "The search schema that defines partition key and inline filter attributes for the
    /// vector index" (`VectorIndex$SearchSchema`), in the order the request declared it.
    pub search_schema: Vec<SearchSchemaElement>,
    pub projection: Projection,
    /// "The number of dimensions in each vector" (`VectorIndex$Dimensions`).
    pub dimensions: i64,
    /// "The distance function used to calculate similarity between vectors"
    /// (`VectorIndex$DistanceFunction`).
    pub distance_function: String,
    /// The transient status this index is in until [`Self::active_at`] falls due.
    pub transient_status: IndexStatus,
    /// When the clock owes this index its transient -> `ACTIVE` transition.
    pub active_at: SystemTime,
    /// The metered figures this index's `ItemCount` and `IndexSizeBytes` report,
    /// refreshed with the table's on the documented schedule.
    pub reported_counters: MeteredCounters,
}

impl VectorIndex {
    /// The status at `now`, derived from the stored instants, by the same rule a
    /// secondary index follows.
    pub fn status(&self, now: SystemTime) -> IndexStatus {
        if now >= self.active_at {
            IndexStatus::Active
        } else {
            self.transient_status
        }
    }

    /// Every attribute of the TABLE this index consumes: the vector attribute, and each
    /// attribute its search schema names.
    ///
    /// This is what makes an `AttributeDefinitions` entry used by a vector index, and what
    /// the index stores for an item beside whatever its `Projection` adds.
    pub fn consumed_attributes(&self) -> impl Iterator<Item = &str> {
        ::std::iter::once(self.vector_attribute.attribute_name.as_str()).chain(
            self.search_schema
                .iter()
                .map(|element| element.attribute_name.as_str()),
        )
    }

    /// Whether `item` reaches this index: it must carry the vector attribute, and the
    /// value of every attribute the search schema partitions the index by
    /// ([`SEARCH_SCHEMA_PARTITION_KEY`]). An `INLINE_FILTER` attribute is optional, so an
    /// item that leaves one out is still indexed.
    pub fn covers(&self, item: &Item) -> bool {
        item.contains_key(&self.vector_attribute.attribute_name)
            && self
                .search_schema
                .iter()
                .filter(|element| element.search_schema_element_type == SEARCH_SCHEMA_PARTITION_KEY)
                .all(|element| item.contains_key(&element.attribute_name))
    }

    /// Which of a covered item's attributes this index STORES, or `None` when it stores
    /// every one of them.
    ///
    /// An index holds "attributes that are copied (projected) from the table into the
    /// vector index" (`VectorIndex$Projection`): the attributes it consumes (its vector
    /// attribute and its search schema), the table's own key attributes, and whatever its
    /// `Projection` adds by the same rule a secondary index is projected by
    /// ([`index_projects`]). `ALL` stores the whole item, which is `None` here.
    pub fn stored_attributes(
        &self,
        table_key_schema: &[KeySchemaElement],
    ) -> Option<BTreeSet<String>> {
        if projects_all(&self.projection) {
            return None;
        }
        let mut names: BTreeSet<String> = self
            .consumed_attributes()
            .map(ToString::to_string)
            .collect();
        // Which of the TABLE's attributes a non-`ALL` projection adds is
        // [`index_projects`]'s rule to state, so the candidates are filtered through it
        // rather than re-derived here.
        names.extend(
            table_key_schema
                .iter()
                .map(|element| element.attribute_name.as_str())
                .chain(
                    self.projection
                        .non_key_attributes
                        .iter()
                        .flatten()
                        .map(String::as_str),
                )
                .filter(|name| index_projects(&[], &self.projection, table_key_schema, name))
                .map(ToString::to_string),
        );
        Some(names)
    }

    /// The entry this index holds for one covered item: just the attributes it stores
    /// ([`Self::stored_attributes`]), which is what a search through it can report.
    pub fn index_entry(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> Item {
        match self.stored_attributes(table_key_schema) {
            None => item.clone(),
            Some(stored) => item
                .iter()
                .filter(|(name, _)| stored.contains(name.as_str()))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        }
    }

    /// The bytes this index stores for one covered item: the size of the entry it holds
    /// for it, so what the index is sized by and what a search through it reports are the
    /// same projection.
    fn projected_size(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> i64 {
        item_size(&self.index_entry(item, table_key_schema))
    }

    /// Project this index into the `VectorIndexDescription` every table description
    /// reports it as. `ItemCount` and `IndexSizeBytes` are the metered figures of the
    /// last refresh, never the live projection.
    fn describe(&self, now: SystemTime) -> VectorIndexDescription {
        VectorIndexDescription {
            index_name: Some(self.name.clone()),
            index_arn: Some(self.arn.clone()),
            vector_attribute: Some(self.vector_attribute.clone()),
            search_schema: (!self.search_schema.is_empty()).then(|| self.search_schema.clone()),
            projection: Some(self.projection.clone()),
            dimensions: Some(self.dimensions),
            distance_function: Some(self.distance_function.clone()),
            index_status: Some(self.status(now).as_str().to_string()),
            // "Specifies whether the index is currently backfilling"
            // (`VectorIndexDescription$Backfilling`): an index defined WITH the table has
            // no table data to backfill from, and this emulator projects an index's items
            // from the table on every read, so none ever is.
            backfilling: None,
            item_count: Some(self.reported_counters.item_count),
            index_size_bytes: Some(self.reported_counters.size_bytes),
        }
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
/// `IndexSizeBytes` are computed from the TABLE's items, on the metering schedule
/// [`MeteredCounters`] describes rather than per write.
#[derive(Debug, Clone)]
pub struct LocalSecondaryIndex {
    pub name: String,
    pub arn: String,
    pub key_schema: Vec<KeySchemaElement>,
    pub projection: Projection,
    /// The metered figures this index's `ItemCount` and `IndexSizeBytes` report,
    /// refreshed with the table's on the documented schedule.
    pub reported_counters: MeteredCounters,
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

/// One secondary index a READ can be served from, whichever kind it is: the key schema
/// and the projection that decide what the index holds, and whether it is global.
///
/// The two stored kinds ([`GlobalSecondaryIndex`], [`LocalSecondaryIndex`]) differ in
/// what can be done TO them — a global index has its own lifecycle and throughput — but
/// a read of either is decided by the same three facts, so a reader resolves an
/// `IndexName` to this once ([`Table::secondary_index`]) and never branches on the kind
/// again.
#[derive(Debug, Clone, Copy)]
pub struct SecondaryIndex<'a> {
    /// The index's own name, as the table declared it.
    pub name: &'a str,
    /// The key schema the index orders its entries by.
    pub key_schema: &'a [KeySchemaElement],
    /// Which of a covered item's attributes the index stores.
    pub projection: &'a Projection,
    /// Whether this is a GLOBAL secondary index. "The ConsistentRead parameter is not
    /// supported on global secondary indexes" and a read of one "cannot fetch attributes
    /// from the parent table" (`Scan$ConsistentRead`, `Scan$Select`), so a reader needs
    /// to know which kind it resolved even though it walks both the same way.
    pub global: bool,
}

impl SecondaryIndex<'_> {
    /// Whether this index holds an entry for `item` at all ([`index_covers`]).
    pub fn covers(&self, item: &Item) -> bool {
        index_covers(self.key_schema, item)
    }

    /// Whether this index STORES the attribute `name` of an item it covers
    /// ([`index_projects`]).
    pub fn projects(&self, table_key_schema: &[KeySchemaElement], name: &str) -> bool {
        index_projects(self.key_schema, self.projection, table_key_schema, name)
    }

    /// Whether this index projects EVERY attribute of the items it covers: "ALL - All of
    /// the table attributes are projected into the index" (`Projection$ProjectionType`),
    /// which is the only projection a read of every item attribute can be served from.
    pub fn projects_all(&self) -> bool {
        projects_all(self.projection)
    }

    /// The entry this index holds for one item it covers ([`index_entry`]).
    pub fn entry(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> Item {
        index_entry(self.key_schema, self.projection, table_key_schema, item)
    }
}

/// Whether an index with this projection stores every attribute of the items it covers.
///
/// The default is `ALL`, which is what `CreateTable` stores for an index whose request
/// named no `ProjectionType`, so the default is stated once here.
fn projects_all(projection: &Projection) -> bool {
    projection.projection_type.as_deref().unwrap_or("ALL") == "ALL"
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

/// Whether an index with this key schema and projection STORES the attribute `name` of
/// an item it covers: "KEYS_ONLY - Only the index and primary key attributes are
/// projected ... INCLUDE - ... the attributes that are projected are those in
/// NonKeyAttributes ... ALL - All of the table attributes are projected into the index"
/// (`Projection$ProjectionType`).
///
/// One predicate for every question about an index's stored entry — its size and whether
/// a write changed it — so both answers are decided by the same projection rule.
fn index_projects(
    key_schema: &[KeySchemaElement],
    projection: &Projection,
    table_key_schema: &[KeySchemaElement],
    name: &str,
) -> bool {
    let projection_type = projection.projection_type.as_deref().unwrap_or("ALL");
    if projection_type == "ALL" {
        return true;
    }
    let is_key = key_schema
        .iter()
        .chain(table_key_schema)
        .any(|element| element.attribute_name == name);
    is_key
        || (projection_type == "INCLUDE"
            && projection
                .non_key_attributes
                .iter()
                .flatten()
                .any(|kept| kept == name))
}

/// The entry an index with this key schema and projection HOLDS for one covered item:
/// the attributes it projects and no others ([`index_projects`]).
///
/// One rendering for every question about that entry — what a read of the index answers,
/// how big it is, and whether a write changed it — so the three answers cannot drift
/// apart.
fn index_entry(
    key_schema: &[KeySchemaElement],
    projection: &Projection,
    table_key_schema: &[KeySchemaElement],
    item: &Item,
) -> Item {
    if projects_all(projection) {
        return item.clone();
    }
    item.iter()
        .filter(|(name, _)| index_projects(key_schema, projection, table_key_schema, name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The bytes an index stores for one covered item: the size of the entry it holds for it
/// ([`index_entry`]), so what an index is sized by and what a read of it reports are the
/// same projection.
fn index_projected_size(
    key_schema: &[KeySchemaElement],
    projection: &Projection,
    item: &Item,
    table_key_schema: &[KeySchemaElement],
) -> i64 {
    if projects_all(projection) {
        return item_size(item);
    }
    item_size(&index_entry(key_schema, projection, table_key_schema, item))
}

/// Whether a write CHANGES what an index stores for an item it covers both before and
/// after: the index keeps only the attributes it projects, so a write that leaves every
/// one of them as it was leaves the index entry as it was.
///
/// An attribute is compared as the VALUE it is, by the same rule a condition compares
/// values with ([`attribute_values_equal`]) — a number respelt is the same number — and
/// an attribute one version carries and the other does not is a change, whichever side
/// carries it.
fn index_entry_changed(
    key_schema: &[KeySchemaElement],
    projection: &Projection,
    table_key_schema: &[KeySchemaElement],
    before: &Item,
    after: &Item,
) -> bool {
    let differs = |source: &Item, other: &Item| {
        source
            .iter()
            .filter(|(name, _)| index_projects(key_schema, projection, table_key_schema, name))
            .any(|(name, value)| {
                other
                    .get(name)
                    .is_none_or(|stored| !attribute_values_equal(value, stored))
            })
    };
    differs(before, after) || differs(after, before)
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
///
/// Every case the Guide charges for is a CHANGE to what the index stores, so a write that
/// leaves this index's entry exactly as it stood costs nothing on it: the attributes it
/// does not project cannot change it, which is why projecting fewer attributes is what
/// keeps an index's write cost down. The table itself is charged either way — it stores
/// the item the write carried whether or not the item differs.
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
            } else if index_entry_changed(key_schema, projection, table_key_schema, old, new) {
                Some(write_capacity_units(size(old).max(size(new))))
            } else {
                None
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

    /// Record that the table's provisioned capacity was WITHDRAWN at `now` — the table
    /// went on-demand, so the capacity a description reports drops to zero.
    ///
    /// That is the last time the reported capacity went down, so it moves
    /// `LastDecreaseDateTime`. It does NOT move the day's count: what
    /// `NumberOfDecreasesToday` counts is the capacity decreases the documented daily
    /// allowance counts — "For every table and global secondary index in an UpdateTable
    /// operation, you can decrease ReadCapacityUnits or WriteCapacityUnits (or both)"
    /// (`ProvisionedThroughputDescription$NumberOfDecreasesToday`'s "current maximums on
    /// provisioned throughput decreases") — and a table with no provisioned capacity at
    /// all has not spent one of them.
    pub fn record_capacity_withdrawal(&mut self, now: SystemTime) {
        self.last_decrease = Some(now);
    }

    /// The description members a `ProvisionedThroughputDescription` carries.
    fn describe(
        &self,
        now: SystemTime,
    ) -> (
        Option<i64>,
        Option<::aws_smithy_types::DateTime>,
        Option<::aws_smithy_types::DateTime>,
    ) {
        (
            // `NumberOfDecreasesToday` is always reported, as the count of decreases
            // applied in this UTC calendar day — zero for a table that has had none.
            // The `PositiveLongObject` range (min 1) the model puts on the member
            // constrains what a REQUEST may carry; what a description reports is the
            // day's count, and a client polling it to decide whether a decrease has
            // landed needs the member to be there before the first one.
            Some(self.decreases_today(now)),
            self.last_increase.map(::aws_smithy_types::DateTime::from),
            self.last_decrease.map(::aws_smithy_types::DateTime::from),
        )
    }
}

/// A table's attribute definitions as a description reports them: ordered by attribute
/// name, which is the order DynamoDB echoes them in whatever order they were submitted
/// in, so two creates differing only in that order describe the same table shape.
///
/// Sorted in the PROJECTION rather than on the way in: the stored table keeps exactly
/// what the request declared.
fn described_definitions(definitions: &[AttributeDefinition]) -> Vec<AttributeDefinition> {
    let mut described = definitions.to_vec();
    described.sort_by(|left, right| left.attribute_name.cmp(&right.attribute_name));
    described
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

/// The DynamoDB Streams stream a table publishes its item-level changes to.
///
/// A stream is a resource of the STREAMS service, which owns the registry its own
/// consumers read ([`register_table_stream`]); what the table keeps is the identity
/// of its latest stream — "the ARN that uniquely identifies the latest stream for
/// this table" (`TableDescription$LatestStreamArn`) — and the specification that
/// stream was created from, which is what every `TableDescription` reports.
#[derive(Debug, Clone)]
pub struct TableStream {
    /// `TableDescription$LatestStreamArn`, as the streams service minted it.
    pub arn: String,
    /// `TableDescription$LatestStreamLabel`: "a timestamp, in ISO 8601 format, for
    /// this stream".
    pub label: String,
    /// The wire `StreamViewType` the stream captures records in.
    pub view_type: String,
    /// Whether the stream is still ON. A disabled stream keeps its ARN and label:
    /// AWS reports the LATEST stream of the table, and the stream itself outlives
    /// the disable (it is deleted 24 hours later).
    pub enabled: bool,
}

impl TableStream {
    /// The `StreamSpecification` a description reports for this stream, or `None`
    /// where the stream is OFF.
    ///
    /// A specification describes a stream that EXISTS, so it is reported only while
    /// the table's stream is on: a table whose stream has been turned off carries no
    /// specification member at all, rather than one with the flag false. That
    /// presence is how a client tells a streaming table from a quiet one — the member
    /// is there or it is not — and a table that never enabled a stream reports none
    /// for the same reason.
    pub fn specification(&self) -> Option<StreamSpecification> {
        self.enabled.then(|| StreamSpecification {
            stream_enabled: true,
            stream_view_type: Some(self.view_type.clone()),
        })
    }
}

/// One item-level change a landed write owes the table's stream.
///
/// The data plane writes items while holding THIS service's store lock, and the
/// records live in the STREAMS service's store, so a write records what it changed
/// here and the handler hands the queue over at the end of the request
/// ([`publish_item_changes`]). Which change it was is derived from the images rather
/// than stored beside them: an item that was not there before is an `INSERT`, one that
/// was and still is a `MODIFY`, and one that is gone a `REMOVE` (`OperationType`).
#[derive(Debug, Clone)]
pub struct PendingStreamRecord {
    /// The stream the record belongs to, as the streams registry minted its ARN.
    pub stream_arn: String,
    /// "The primary key attribute(s) for the DynamoDB item that was modified"
    /// (`StreamRecord$Keys`), read off the item through the table's own key schema.
    pub keys: Item,
    /// The item before the change, for a write that replaced or removed one.
    pub old_image: Option<Item>,
    /// The item after the change, for a write that stored one.
    pub new_image: Option<Item>,
}

/// The most changes this store queues for the streams service before a handler hands
/// them over.
///
/// The queue is drained at the end of every operation that writes an item, so its
/// length is bounded by ONE request's writes — a batch write carries at most
/// [`MAX_BATCH_WRITE_REQUESTS`] of them. The cap is
/// what keeps that true even if a future write path forgets to drain it: past it a
/// change is dropped and the drop is logged, rather than letting the
/// queue grow with the writes of a whole process lifetime.
pub const MAX_PENDING_STREAM_RECORDS: usize = 1024;

/// The METERED figures a table or index description reports as `ItemCount` and
/// `TableSizeBytes`/`IndexSizeBytes`.
///
/// They are a snapshot of what was stored when the figures were last computed, NOT the
/// live store: "DynamoDB updates this value approximately every six hours. Recent
/// changes might not be reflected in this value" (the `TableDescription$ItemCount` doc,
/// and the same sentence on `$TableSizeBytes` and
/// `GlobalSecondaryIndexDescription$IndexSizeBytes`). So a write never moves them, and a
/// client cannot use them as an exact count the real service never gives; the next
/// refresh falls due [`COUNTER_REFRESH_INTERVAL`] after the last one.
#[derive(Debug, Clone)]
pub struct MeteredCounters {
    /// What the last refresh counted, which is what `ItemCount` reports.
    pub item_count: i64,
    /// What the last refresh measured, which is what `TableSizeBytes`/`IndexSizeBytes`
    /// reports.
    pub size_bytes: i64,
    /// When those two figures were computed. Held so the next refresh can be recognised
    /// as due from the clock alone, with nothing having to run on a timer.
    refreshed_at: SystemTime,
}

impl MeteredCounters {
    /// The figures a table or index created at `now` starts life with: it holds nothing
    /// yet, and its first refresh is owed one interval later.
    pub fn new(now: SystemTime) -> Self {
        Self {
            item_count: 0,
            size_bytes: 0,
            refreshed_at: now,
        }
    }

    /// Whether the clock has already made the next refresh due.
    fn refresh_due(&self, now: SystemTime) -> bool {
        now.duration_since(self.refreshed_at)
            .is_ok_and(|elapsed| elapsed >= COUNTER_REFRESH_INTERVAL)
    }

    /// Record the figures a refresh at `now` computed.
    fn refresh(&mut self, now: SystemTime, item_count: i64, size_bytes: i64) {
        self.item_count = item_count;
        self.size_bytes = size_bytes;
        self.refreshed_at = now;
    }
}

/// What is left of the capacity a PROVISIONED table is provisioned for, and when that
/// figure was last advanced.
///
/// "Provisioned throughput is the maximum amount of capacity that an application can
/// consume from a table or index" per second (DynamoDB Developer Guide, "Read/write
/// capacity mode"), and "DynamoDB currently retains up to 300 seconds (5 minutes) of
/// unused read and write capacity" for a burst — so the budget refills at the
/// provisioned rate and never holds more than [`BURST_CAPACITY_WINDOW`] of it. The
/// rate is PASSED IN rather than stored, because the capacity in force is the table's
/// own and an `UpdateTable` moves it.
///
/// Only the figure and the instant are kept, so nothing runs on a timer: how much has
/// refilled is derived from the clock when the next write asks for it.
#[derive(Debug, Clone)]
pub struct CapacityBudget {
    /// What was left at [`Self::measured_at`], or `None` while nothing has been spent:
    /// an untouched budget holds the whole ceiling, whatever rate is in force by the
    /// time it is first spent from.
    remaining: Option<f64>,
    /// When `remaining` was measured.
    measured_at: SystemTime,
}

impl CapacityBudget {
    /// The budget a resource created at `now` starts life with: it has consumed
    /// nothing, so the first write it is asked to admit has the whole burst ceiling
    /// behind it.
    pub fn new(now: SystemTime) -> Self {
        Self {
            remaining: None,
            measured_at: now,
        }
    }

    /// Take `units` from the budget a rate of `units_per_second` refills, reporting
    /// whether the budget covered them.
    ///
    /// A request the budget cannot cover takes NOTHING from it: capacity is consumed by
    /// the work that was done, so a throttled write leaves the budget where it was.
    pub fn take(&mut self, units_per_second: i64, units: f64, now: SystemTime) -> bool {
        self.take_within(units_per_second, BURST_CAPACITY_WINDOW, units, now)
    }

    /// The same, over a stated burst window — what the per-partition-key ceiling is
    /// measured over ([`PARTITION_BURST_WINDOW`]), so one implementation serves both
    /// budgets a write is admitted from.
    pub fn take_within(
        &mut self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) -> bool {
        let available = self.available(units_per_second, window, now);
        self.measured_at = now;
        let covered = available >= units;
        self.remaining = Some(if covered { available - units } else { available });
        covered
    }

    /// Whether the budget would cover `units` at `now`, WITHOUT taking them: what lets a
    /// write be held to two budgets at once and debit neither when either refuses it.
    pub fn covers_within(
        &self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) -> bool {
        self.available(units_per_second, window, now) >= units
    }

    /// Whether the budget can serve `units` by BORROWING against its own refill: the
    /// draw may leave the budget in deficit, but no deeper than `allowance`.
    ///
    /// Reports it without taking anything, so a write held to two budgets at once
    /// debits neither when either of them refuses it.
    pub fn covers_borrowing(
        &self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
        allowance: f64,
    ) -> bool {
        self.available(units_per_second, window, now) - units >= -allowance
    }

    /// Take `units` the budget may be SHORT of, leaving it in deficit by the shortfall.
    ///
    /// The units are charged in full, so nothing is served from capacity that is never
    /// accounted: the deficit repays at the rate in force, which leaves the rate this
    /// budget bounds exactly what it was. What borrowing changes is only how far ahead
    /// of its own refill one draw may be served (see
    /// [`Table::admit_item_write_with_progress`]), and
    /// [`CapacityBudget::covers_borrowing`] is what bounds that.
    pub fn take_borrowing(
        &mut self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) {
        let available = self.available(units_per_second, window, now);
        self.measured_at = now;
        self.remaining = Some(available - units);
    }

    /// What has refilled by `now`: the figure last measured plus the rate over the time
    /// since, held at the ceiling the burst window allows.
    fn available(&self, units_per_second: i64, window: Duration, now: SystemTime) -> f64 {
        let rate = units_as_f64(units_per_second);
        let ceiling = burst_ceiling(units_per_second, window);
        let elapsed = now
            .duration_since(self.measured_at)
            .unwrap_or_default()
            .as_secs_f64();
        match self.remaining {
            None => ceiling,
            Some(remaining) => (remaining + elapsed * rate).min(ceiling),
        }
    }

    /// Whether this budget has gone a whole `window` without being spent from, which
    /// means it has refilled to the ceiling and holds no more information than a budget
    /// that was never spent from at all.
    fn refilled(&self, window: Duration, now: SystemTime) -> bool {
        self.remaining.is_none()
            || now
                .duration_since(self.measured_at)
                .is_ok_and(|idle| idle >= window)
    }
}

/// The MOST a budget refilled at `units_per_second` can ever hold: the rate over the
/// whole window it accumulates credit over.
///
/// It is the ceiling [`CapacityBudget::available`] holds a refill at, so it is also the
/// largest single draw that budget can ever cover however long the caller waits — which
/// is what tells a write that is merely waiting for capacity from one no amount of
/// waiting admits.
pub fn burst_ceiling(units_per_second: i64, window: Duration) -> f64 {
    units_as_f64(units_per_second) * window.as_secs_f64()
}

/// What is left of the per-partition-key write capacity of one table: one budget per
/// partition key value that has been written to recently.
///
/// Admission-control bookkeeping ONLY — no response reports it — and bounded: entries
/// that have refilled are dropped once the map reaches [`MAX_TRACKED_WRITE_KEYS`], so a
/// table that is written across many keys does not accumulate a budget per key it ever
/// held.
#[derive(Debug, Clone, Default)]
pub struct KeyWriteBudgets {
    budgets: BTreeMap<AttributeValueKey, CapacityBudget>,
}

impl KeyWriteBudgets {
    /// Whether the key's own ceiling covers `units` at `now`, without taking them.
    fn covers(&self, partition: &AttributeValueKey, units: f64, now: SystemTime) -> bool {
        self.budgets.get(partition).is_none_or(|budget| {
            budget.covers_within(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
            )
        })
    }

    /// Take `units` from that key's budget, reporting whether it covered them.
    ///
    /// The map is held at [`MAX_TRACKED_WRITE_KEYS`] entries BEFORE a new one is added,
    /// so the bookkeeping one table carries is a constant of this store and not a
    /// function of how many keys have been written: the budgets that have refilled go
    /// first (they hold no more than a key never written to), and if every entry is
    /// still live the least recently spent from gives way, because it is the closest to
    /// having refilled.
    fn take(&mut self, partition: &AttributeValueKey, units: f64, now: SystemTime) -> bool {
        self.make_room_for(partition, now);
        self.budgets
            .entry(partition.clone())
            .or_insert_with(|| CapacityBudget::new(now))
            .take_within(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
            )
    }

    /// Whether that key's budget can serve `units` by borrowing against its own refill,
    /// no deeper than `allowance` ([`CapacityBudget::covers_borrowing`]).
    fn covers_borrowing(
        &self,
        partition: &AttributeValueKey,
        units: f64,
        now: SystemTime,
        allowance: f64,
    ) -> bool {
        self.budgets.get(partition).is_none_or(|budget| {
            budget.covers_borrowing(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
                allowance,
            )
        })
    }

    /// Take `units` from that key's budget, leaving it in deficit by whatever it was
    /// short of ([`CapacityBudget::take_borrowing`]).
    fn take_borrowing(&mut self, partition: &AttributeValueKey, units: f64, now: SystemTime) {
        self.make_room_for(partition, now);
        self.budgets
            .entry(partition.clone())
            .or_insert_with(|| CapacityBudget::new(now))
            .take_borrowing(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
            );
    }

    /// Hold the map at [`MAX_TRACKED_WRITE_KEYS`] entries before an entry for
    /// `partition` is added to it.
    fn make_room_for(&mut self, partition: &AttributeValueKey, now: SystemTime) {
        if self.budgets.len() >= MAX_TRACKED_WRITE_KEYS && !self.budgets.contains_key(partition) {
            self.budgets
                .retain(|_, budget| !budget.refilled(PARTITION_BURST_WINDOW, now));
            while self.budgets.len() >= MAX_TRACKED_WRITE_KEYS {
                let Some(oldest) = self
                    .budgets
                    .iter()
                    .min_by_key(|(_, budget)| budget.measured_at)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                self.budgets.remove(&oldest);
            }
        }
    }
}

/// Why the write path refused a write: WHICH of the ceilings a write is admitted from
/// was the binding one, which is what the throttle reports back to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteThrottle {
    /// The write capacity the TABLE is provisioned for was exhausted.
    TableProvisioned,
    /// The table had capacity left, but the request rate aimed at this one partition
    /// key value was past the internal partition-level ceiling
    /// ([`PARTITION_WRITE_UNITS_PER_SECOND`]).
    KeyRange,
}

impl WriteThrottle {
    /// The `ThrottlingReason$reason`: "The throttling reason follows a specific format:
    /// `ResourceType+OperationType+LimitType`" with the resource `Table`, the operation
    /// `Write` and the limit type of the ceiling that bound (that member's doc).
    pub fn reason(self) -> &'static str {
        match self {
            Self::TableProvisioned => "TableWriteProvisionedThroughputExceeded",
            Self::KeyRange => "TableWriteKeyRangeThroughputExceeded",
        }
    }

    /// The sentence DynamoDB states for this refusal: the provisioning one for a table
    /// that has spent what it is provisioned for, and the hot-key one for a single
    /// partition key value past the internal ceiling — which is the case that doc points
    /// a caller at its key design for.
    pub fn message(self) -> &'static str {
        match self {
            Self::TableProvisioned => THROUGHPUT_EXCEEDED_MESSAGE,
            Self::KeyRange => KEY_RANGE_THROUGHPUT_EXCEEDED_MESSAGE,
        }
    }

    /// The error code a surface that reports a per-slot failure (`BatchStatementError`)
    /// carries for it.
    pub fn code(self) -> &'static str {
        "ProvisionedThroughputExceeded"
    }
}

/// Why the read path refused a read: the ceiling that bound it.
///
/// The read counterpart of [`WriteThrottle`], reported the same way — a
/// `ProvisionedThroughputExceededException` carrying the sentence and the
/// `ThrottlingReason` of the ceiling ([`throughput_exceeded`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadThrottle {
    /// The read capacity the TABLE is provisioned for was exhausted.
    TableProvisioned,
}

impl ReadThrottle {
    /// The `ThrottlingReason$reason`: "The throttling reason follows a specific format:
    /// `ResourceType+OperationType+LimitType`" with the resource `Table`, the operation
    /// `Read` and the limit type of the ceiling that bound (that member's doc).
    pub fn reason(self) -> &'static str {
        match self {
            Self::TableProvisioned => "TableReadProvisionedThroughputExceeded",
        }
    }

    /// The sentence DynamoDB states for this refusal — the same provisioning sentence a
    /// write past the table's capacity earns, because it is the same ceiling in the other
    /// direction.
    pub fn message(self) -> &'static str {
        match self {
            Self::TableProvisioned => THROUGHPUT_EXCEEDED_MESSAGE,
        }
    }

    /// The error code a surface that reports a per-slot failure (`BatchStatementError`)
    /// carries for it.
    pub fn code(self) -> &'static str {
        "ProvisionedThroughputExceeded"
    }
}

/// How a write past the capacity its table is provisioned for is reported: the sentence
/// DynamoDB states for that refusal, naming the table's provisioning and the call that
/// raises it.
pub const THROUGHPUT_EXCEEDED_MESSAGE: &str =
    "The level of configured provisioned throughput for the table was exceeded. Consider \
     increasing your provisioning level with the UpdateTable API.";

/// How a write past the internal per-partition-key ceiling is reported: the sentence
/// DynamoDB states when the throughput a single key is offered exceeds the capacity of
/// the partition holding it, which is the diagnosis that refusal carries.
pub const KEY_RANGE_THROUGHPUT_EXCEEDED_MESSAGE: &str =
    "Throughput exceeds the current capacity of your table or index. DynamoDB is \
     automatically scaling your table or index so please try again shortly. If exceptions \
     persist, check if you have a hot key.";


/// A capacity figure the store holds as an integer, as the arithmetic above takes it.
fn units_as_f64(units: i64) -> f64 {
    f64::from(u32::try_from(units.max(0)).unwrap_or(u32::MAX))
}

/// One table's or index's warm throughput: the read and write units per second it keeps
/// warm, and when the pair in force became (or becomes) `ACTIVE`.
///
/// "Warm throughput ... specifies the minimum provisioned capacity maintained for
/// immediate data access" (the `TableWarmThroughputDescription` doc), and every table
/// has one — the request's values when `CreateTable`/`UpdateTable` named them, otherwise
/// the capacity the resource is provisioned for, and the documented default pair
/// ([`DEFAULT_WARM_READ_UNITS_PER_SECOND`] / [`DEFAULT_WARM_WRITE_UNITS_PER_SECOND`])
/// for a resource created with no provisioned capacity at all.
///
/// The pair is STORED rather than derived on every read: it is set when a request names
/// one and when the resource's provisioned capacity is set ([`Self::tracking_capacity`]),
/// and WITHDRAWING that capacity leaves it alone — what the resource was keeping warm
/// stays warm. `Status` is the status of this SETTING, not of the table: `UPDATING` while
/// an `UpdateTable` that changed it is still in flight, `ACTIVE` once the pair is in
/// force.
#[derive(Debug, Clone)]
pub struct WarmThroughputSettings {
    pub read_units_per_second: i64,
    pub write_units_per_second: i64,
    /// Whether a REQUEST named this pair. A pair nobody named follows the capacity the
    /// resource is provisioned for, so provisioning it differently moves the pair along;
    /// a named one stands until another request names a new one.
    pub named: bool,
    /// When the clock makes this pair `ACTIVE`. Equal to the creation instant for the
    /// pair a table is created with, so a table never reports a warm throughput that is
    /// not yet in force.
    pub active_at: SystemTime,
}

impl WarmThroughputSettings {
    /// The settings of a resource no request has named a warm throughput for: from `now`
    /// it keeps warm the capacity it is provisioned for, or the documented default pair
    /// when it is provisioned for none.
    pub fn tracking(provisioned: Option<&ProvisionedThroughput>, now: SystemTime) -> Self {
        let (read_units_per_second, write_units_per_second) = derived_warm_throughput(provisioned);
        Self {
            read_units_per_second,
            write_units_per_second,
            named: false,
            active_at: now,
        }
    }

    /// The pair in force: the one these settings hold.
    pub fn in_force(&self) -> (i64, i64) {
        (self.read_units_per_second, self.write_units_per_second)
    }

    /// These settings after the resource's provisioned capacity was SET to `provisioned`.
    ///
    /// A pair nobody named follows the capacity, because warm throughput is "the minimum
    /// provisioned capacity maintained for immediate data access" (the
    /// `TableWarmThroughputDescription` doc) — the recorded description of a provisioned
    /// table and of its index report their own units. A capacity WITHDRAWN (`None`, which
    /// is what a switch to `PAY_PER_REQUEST` leaves table and index with) changes nothing:
    /// the capacity that was being kept warm stays warm, so the resource goes on
    /// reporting the pair last in force rather than the pair a resource created on-demand
    /// starts with. A named pair is untouched either way.
    pub fn tracking_capacity(&self, provisioned: Option<&ProvisionedThroughput>) -> Self {
        match (self.named, provisioned) {
            (false, Some(capacity)) => Self {
                read_units_per_second: capacity.read_capacity_units,
                write_units_per_second: capacity.write_capacity_units,
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    /// The status this setting reports at `now`.
    fn status(&self, now: SystemTime) -> TableStatus {
        if now >= self.active_at {
            TableStatus::Active
        } else {
            TableStatus::Updating
        }
    }
}

/// The warm throughput a `CreateTable`/`UpdateTable` request leaves in force: the
/// members it names, falling back to `current` for the ones it does not, becoming
/// `ACTIVE` at `active_at` when it changes anything.
///
/// `current` is the pair in force once the request's CAPACITY has been applied (see
/// [`WarmThroughputSettings::tracking_capacity`]), so the half of the pair a request
/// leaves out is the half the table keeps warm after that move.
///
/// The FLOOR applied here is the model's: the description reports both members as
/// `PositiveLongObject` (range min 1), so a value below 1 is one no description could
/// carry and is refused. The Developer Guide's "Warm throughput" 12,000 read / 4,000
/// write pair is the value a table created with no provisioned capacity STARTS with — a
/// default, which is why [`derived_warm_throughput`] uses it — and no sentence in the
/// model or the guide makes it a minimum a request may not go under, so this stores a
/// lower value the caller asks for and reports it back rather than refusing on an
/// unsourced rule.
pub fn resolved_warm_throughput(
    requested: Option<&WarmThroughput>,
    current: &WarmThroughputSettings,
    active_at: SystemTime,
) -> Result<WarmThroughputSettings, RequestError> {
    let Some(requested) = requested else {
        return Ok(current.clone());
    };
    // A request that names one member leaves the other at the pair in force.
    let (in_force_read, in_force_write) = current.in_force();
    let read = requested.read_units_per_second.unwrap_or(in_force_read);
    let write = requested.write_units_per_second.unwrap_or(in_force_write);
    for (member, value) in [
        ("ReadUnitsPerSecond", read),
        ("WriteUnitsPerSecond", write),
    ] {
        if value < 1 {
            return Err(RequestError::Validation(format!(
                "Invalid WarmThroughput: {member} must be at least 1, got {value}"
            )));
        }
    }
    // A request that asks for the values already in force asks for no CHANGE, so the
    // pair keeps the instant it became ACTIVE rather than going UPDATING again. The
    // values it reports are the requested ones either way: naming them fixes the pair,
    // so it no longer moves with the resource's capacity.
    let unchanged = read == in_force_read && write == in_force_write;
    Ok(WarmThroughputSettings {
        read_units_per_second: read,
        write_units_per_second: write,
        named: true,
        active_at: if unchanged {
            current.active_at
        } else {
            active_at
        },
    })
}

/// The warm throughput a resource that no request has named one for keeps warm.
///
/// Warm throughput is "the minimum provisioned capacity maintained for immediate data
/// access" (the `TableWarmThroughputDescription` doc), so a resource that IS provisioned
/// keeps exactly that capacity warm — the recorded descriptions of a provisioned table
/// and of its global secondary index report their own provisioned units, not the default
/// pair — while one with no provisioned capacity at all (on-demand mode) keeps warm the
/// documented default ([`DEFAULT_WARM_READ_UNITS_PER_SECOND`] /
/// [`DEFAULT_WARM_WRITE_UNITS_PER_SECOND`]).
///
/// One rule for both resource kinds: a table's unnamed pair and every global secondary
/// index's pair are computed here from the capacity each of them carries.
pub fn derived_warm_throughput(provisioned: Option<&ProvisionedThroughput>) -> (i64, i64) {
    provisioned.map_or(
        (
            DEFAULT_WARM_READ_UNITS_PER_SECOND,
            DEFAULT_WARM_WRITE_UNITS_PER_SECOND,
        ),
        |capacity| {
            (
                capacity.read_capacity_units,
                capacity.write_capacity_units,
            )
        },
    )
}

/// The capacity pair a `ProvisionedThroughputDescription` reports for a table or index:
/// the stored one, or zeroes when there is none — "For a table or global secondary index
/// that uses on-demand capacity mode (`PAY_PER_REQUEST`), this value is `0`, because
/// on-demand mode does not use provisioned throughput"
/// (`ProvisionedThroughputDescription$ReadCapacityUnits`).
///
/// This is also the pair a capacity MOVE is measured against, so going on-demand reads as
/// a decrease of both dimensions to zero, exactly as the reported description shows it.
pub fn reported_capacity(throughput: Option<&ProvisionedThroughput>) -> ProvisionedThroughput {
    ProvisionedThroughput {
        read_capacity_units: throughput.map_or(0, |throughput| throughput.read_capacity_units),
        write_capacity_units: throughput.map_or(0, |throughput| throughput.write_capacity_units),
    }
}

/// The `OnDemandThroughput` member a request supplies, in the order the model declares
/// the two of them, or `None` for a ceiling that names neither.
///
/// A refusal names the member the caller actually sent, so the first one present is the
/// one reported.
pub fn named_on_demand_member(throughput: &OnDemandThroughput) -> Option<&'static str> {
    if throughput.max_read_request_units.is_some() {
        return Some("MaxReadRequestUnits");
    }
    if throughput.max_write_request_units.is_some() {
        return Some("MaxWriteRequestUnits");
    }
    None
}

/// Refuse an on-demand capacity ceiling named for a resource whose capacity mode is
/// PROVISIONED.
///
/// `MaxReadRequestUnits`/`MaxWriteRequestUnits` cap what an ON-DEMAND table serves —
/// "Sets the maximum number of read and write units for the specified on-demand table"
/// (the `OnDemandThroughput` doc) — and "you must use either OnDemandThroughput or
/// ProvisionedThroughput based on your table's capacity mode"
/// (`GlobalSecondaryIndex$ProvisionedThroughput`). So a ceiling named for a table whose
/// billing mode is the provisioned one asks for a setting that capacity model has
/// nowhere to put: a permanently invalid request, whatever the table's status, rather
/// than a conflict to retry or a gap to report as unimplemented.
///
/// `billing_mode` is the mode IN FORCE for the request — the one it names where it names
/// one, otherwise the table's stored mode — and the ceilings are the table's own and any
/// named per index, since an index's capacity mode is the table's. The message states the
/// member the request supplied and that mode, and nothing else.
pub fn check_on_demand_ceilings<'t>(
    billing_mode: &str,
    ceilings: impl IntoIterator<Item = &'t OnDemandThroughput>,
) -> Result<(), RequestError> {
    if billing_mode != "PROVISIONED" {
        return Ok(());
    }
    for ceiling in ceilings {
        if let Some(member) = named_on_demand_member(ceiling) {
            return Err(RequestError::Validation(format!(
                "One or more parameter values were invalid: {member} for OnDemandThroughput \
                 cannot be specified when the table BillingMode is {billing_mode}"
            )));
        }
    }
    Ok(())
}

/// A new table's `TableId`: "A unique identifier for the table, in UUID format,
/// generated by DynamoDB when the table is created" (the `TableDescription$TableId`
/// doc), in the pattern that member declares
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
///
/// Drawn from the `uuid` crate's v4 generator (OS randomness) ONCE, at create, and
/// stored on the table: every later description echoes the same value, so a client that
/// keys its own records by `TableId` keeps resolving them.
pub fn new_table_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The tags a resource carries, and the set an EVENTUALLY CONSISTENT tag read can see
/// yet.
///
/// A tag write lands on the resource at once and becomes visible to the tag reader a
/// span later: "TagResource is an asynchronous operation. If you issue a
/// ListTagsOfResource request immediately after a TagResource request, DynamoDB might
/// return your previous tag set, if there was one, or an empty tag set. This is because
/// ListTagsOfResource uses an eventually consistent query, and the metadata for your
/// tags or table might not be available at that moment. ... The application or removal
/// of tags using TagResource and UntagResource APIs is eventually consistent.
/// ListTagsOfResource API will only reflect the changes after a few seconds" (the
/// `TagResource` doc, repeated on `UntagResource` and `ListTagsOfResource`). AWS
/// publishes no figure for "a few seconds", so the span is this emulator's CHOSEN one —
/// the same [`STATUS_TRANSITION`] every other asynchronous change here takes: non-zero,
/// so a caller that reads straight after a tag write still observes the set that was
/// visible before it, and short enough that a client that re-polls settles quickly.
///
/// Each write records the set it left WITH ITS OWN due instant, so a later write never
/// postpones an earlier one's visibility and the reader always reports the newest set
/// whose span has elapsed. Nothing sleeps for the span: the propagation is materialised
/// by [`Self::apply_due_propagation`], which
/// [`ComAmazonawsDynamodbDynamodb20120810State::apply_due_transitions`] finishes on
/// every request, whether or not anybody asked for the tags.
#[derive(Debug, Default)]
pub struct ResourceTags {
    /// What the writes have left on the resource: the set the next write merges into or
    /// removes from, and what the tag ceiling is measured against.
    stored: TagSet,
    /// The newest set whose propagation has fallen due, which is the one a tag read
    /// reports.
    visible: TagSet,
    /// One entry per tag write still propagating — the set that write left, and when the
    /// clock owes its visibility — oldest first, so they become visible in the order
    /// they were written.
    propagating: VecDeque<(SystemTime, TagSet)>,
}

impl ResourceTags {
    /// The tag set a `CreateTable` seeds. It propagates like any other tag change, so a
    /// table's tags are readable once the create has propagated — the instant the table
    /// itself becomes `ACTIVE`.
    pub fn seeded(tags: TagSet, now: SystemTime) -> Self {
        let mut seeded = Self::default();
        if !tags.is_empty() {
            seeded.stored = tags;
            seeded.record(now);
        }
        seeded
    }

    /// The set the writes have left on the resource. This is what a WRITE reads — never
    /// what a reporting operation reports, which is [`Self::visible`].
    pub fn stored(&self) -> &TagSet {
        &self.stored
    }

    /// The set an eventually consistent tag read reports.
    pub fn visible(&self) -> &TagSet {
        &self.visible
    }

    /// How many tags the resource would carry once `incoming` were merged in: a key it
    /// already holds is overwritten rather than added, so only new keys grow the set.
    pub fn count_after_merging(&self, incoming: &TagSet) -> usize {
        self.stored.len()
            + incoming
                .keys()
                .filter(|key| !self.stored.contains_key(*key))
                .count()
    }

    /// Merge `incoming` into the stored set — "If you try to add an existing tag (same
    /// key), the existing tag value will be updated to the new value" (the `Tag$Key`
    /// doc), and every key the request does not name keeps its value — and start the
    /// propagation that makes the result visible.
    pub fn merge(&mut self, incoming: TagSet, now: SystemTime) {
        self.stored.extend(incoming);
        self.record(now);
    }

    /// Drop exactly the tag keys named, leaving every other one alone, and start the
    /// propagation of what is left. A key the resource does not carry is nothing to
    /// remove, so the call is idempotent.
    pub fn remove<'k>(&mut self, keys: impl IntoIterator<Item = &'k str>, now: SystemTime) {
        for key in keys {
            self.stored.remove(key);
        }
        self.record(now);
    }

    /// Materialise every propagation the clock has already made due, oldest first, so
    /// the visible set ends up the one the newest elapsed write left.
    pub fn apply_due_propagation(&mut self, now: SystemTime) {
        while self
            .propagating
            .front()
            .is_some_and(|(due, _)| now >= *due)
        {
            if let Some((_, propagated)) = self.propagating.pop_front() {
                self.visible = propagated;
            }
        }
    }

    /// Record the set the write at `now` left, due to be visible one span later.
    fn record(&mut self, now: SystemTime) {
        self.propagating
            .push_back((now + STATUS_TRANSITION, self.stored.clone()));
    }
}

/// The `ContinuousBackupsStatus` a table reports: "Continuous backups are ENABLED on
/// all tables at table creation" (the `UpdateContinuousBackups` doc), and nothing in
/// this API turns them off — `PointInTimeRecoverySpecification` carries only the
/// point-in-time recovery flag — so every table reports the same one of the two values
/// the model declares for that member
/// ([`crate::ops::types::CONTINUOUS_BACKUPS_STATUS_VALUES`]).
pub const CONTINUOUS_BACKUPS_STATUS: &str = "ENABLED";

/// The two values `PointInTimeRecoveryStatus` takes: "ENABLED - Point in time recovery
/// is enabled. DISABLED - Point in time recovery is disabled"
/// (`PointInTimeRecoveryDescription$PointInTimeRecoveryStatus`), which are the model's
/// own value table ([`crate::ops::types::POINT_IN_TIME_RECOVERY_STATUS_VALUES`]) named
/// here one per meaning, so a reader of this store sees which one a setting reports.
pub const POINT_IN_TIME_RECOVERY_ENABLED: &str = "ENABLED";
pub const POINT_IN_TIME_RECOVERY_DISABLED: &str = "DISABLED";

/// The recovery period an enable that names none settles on: "This parameter is
/// optional. If no value is provided, the value will default to 35"
/// (`PointInTimeRecoverySpecification$RecoveryPeriodInDays`).
pub const DEFAULT_RECOVERY_PERIOD_IN_DAYS: i32 = 35;

/// How far behind the present the newest restorable point sits:
/// "`LatestRestorableDateTime` is typically 5 minutes before the current time" (the
/// `UpdateContinuousBackups` doc and
/// `PointInTimeRecoveryDescription$LatestRestorableDateTime`).
pub const RESTORE_WINDOW_LAG: Duration = Duration::from_secs(5 * 60);

/// One day, the unit `RecoveryPeriodInDays` counts the recovery window in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A table's point-in-time recovery settings: what `UpdateContinuousBackups` writes and
/// what both it and `DescribeContinuousBackups` report.
///
/// Only the SETTINGS are stored. The restorable window is DERIVED from them at every
/// read, because it moves with the clock rather than with any call: its newest point is
/// one [`RESTORE_WINDOW_LAG`] behind now and its oldest the configured period behind
/// that, bounded below by the instant recovery started — before which there is nothing
/// to restore from.
#[derive(Debug, Clone, Default)]
pub struct PointInTimeRecovery {
    /// When point-in-time recovery was last turned on, or `None` while it is off (which
    /// is how every table starts: this API is the only way to turn it on, and
    /// `CreateTable` carries no `PointInTimeRecoverySpecification`).
    enabled_at: Option<SystemTime>,
    /// The recovery period in force while recovery is on, in days.
    recovery_period_in_days: i32,
}

impl PointInTimeRecovery {
    /// Apply one `PointInTimeRecoverySpecification`.
    ///
    /// Turning recovery on while it is ALREADY on does not restart the window — the
    /// recoverable period is as old as the recovery that has been running — so only the
    /// period is re-stated; turning it off forgets where the window started, so a later
    /// enable starts a new one.
    pub fn apply(&mut self, enabled: bool, recovery_period_in_days: Option<i32>, now: SystemTime) {
        if enabled {
            self.enabled_at = Some(self.enabled_at.unwrap_or(now));
            self.recovery_period_in_days =
                recovery_period_in_days.unwrap_or(DEFAULT_RECOVERY_PERIOD_IN_DAYS);
        } else {
            self.enabled_at = None;
            self.recovery_period_in_days = 0;
        }
    }

    /// Whether point-in-time recovery is on, which is what a restore from a point in
    /// time is served against ([`Self::restorable_window`] derives the window such a
    /// restore is held to, and answers `None` for exactly the tables this answers
    /// `false` for).
    pub fn enabled(&self) -> bool {
        self.enabled_at.is_some()
    }

    /// The restorable window at `now` — `(earliest, latest)` — while recovery is on,
    /// and `None` while it is off, because a table with recovery off has no point in
    /// time to restore to.
    ///
    /// DERIVED at every read rather than stored, because it moves with the clock: the
    /// newest point is one [`RESTORE_WINDOW_LAG`] behind now ("LatestRestorableDateTime
    /// is typically 5 minutes before the current time") and the oldest the configured
    /// period behind now, each held at or after the instant recovery started — before
    /// which there is nothing to restore from. The bounds are ordered by construction,
    /// so the window is never empty.
    pub fn restorable_window(&self, now: SystemTime) -> Option<(SystemTime, SystemTime)> {
        let enabled_at = self.enabled_at?;
        let latest = now
            .checked_sub(RESTORE_WINDOW_LAG)
            .unwrap_or(now)
            .max(enabled_at);
        let period = u32::try_from(self.recovery_period_in_days).unwrap_or_default();
        let earliest = now
            .checked_sub(DAY * period)
            .unwrap_or(now)
            .clamp(enabled_at, latest);
        // Reported at the precision a timestamp carries on the wire, so each bound a
        // client reads back is exactly the instant it can send in again.
        Some((
            at_timestamp_precision(earliest),
            at_timestamp_precision(latest),
        ))
    }

    /// The continuous-backups settings as the two continuous-backups operations report
    /// them: the table's continuous-backups status, and the point-in-time recovery
    /// description — the window only while recovery is on, since a table with recovery
    /// off has no point in time to restore to.
    pub fn describe(&self, now: SystemTime) -> ContinuousBackupsDescription {
        let recovery = match self.restorable_window(now) {
            Some((earliest, latest)) => PointInTimeRecoveryDescription {
                point_in_time_recovery_status: Some(POINT_IN_TIME_RECOVERY_ENABLED.to_string()),
                recovery_period_in_days: Some(self.recovery_period_in_days),
                earliest_restorable_date_time: Some(::aws_smithy_types::DateTime::from(earliest)),
                latest_restorable_date_time: Some(::aws_smithy_types::DateTime::from(latest)),
            },
            None => PointInTimeRecoveryDescription {
                point_in_time_recovery_status: Some(POINT_IN_TIME_RECOVERY_DISABLED.to_string()),
                ..PointInTimeRecoveryDescription::default()
            },
        };
        ContinuousBackupsDescription {
            continuous_backups_status: CONTINUOUS_BACKUPS_STATUS.to_string(),
            point_in_time_recovery_description: Some(recovery),
        }
    }
}

// =============================================================================
// CONTRIBUTOR INSIGHTS
//
// A table and each of its global secondary indexes carry their own setting: what
// `UpdateContributorInsights` writes, and what `DescribeContributorInsights` and
// `ListContributorInsights` report back.
// =============================================================================

/// The statuses a contributor insights setting is reported in, named one per meaning out
/// of the model's own value table
/// ([`crate::ops::types::CONTRIBUTOR_INSIGHTS_STATUS_VALUES`]).
///
/// `FAILED` is not among them: that status stands for a failure of the CloudWatch rules
/// behind the feature ("the most common exceptions for a FAILED status are:
/// LimitExceededException ... AccessDeniedException ..." — the `FailureException` doc),
/// and this store runs none, so no setting is ever reported in it.
pub const CONTRIBUTOR_INSIGHTS_ENABLING: &str = "ENABLING";
pub const CONTRIBUTOR_INSIGHTS_ENABLED: &str = "ENABLED";
pub const CONTRIBUTOR_INSIGHTS_DISABLING: &str = "DISABLING";
pub const CONTRIBUTOR_INSIGHTS_DISABLED: &str = "DISABLED";

/// The two values `ContributorInsightsAction` declares
/// ([`crate::ops::types::CONTRIBUTOR_INSIGHTS_ACTION_VALUES`]), named one per meaning.
pub const CONTRIBUTOR_INSIGHTS_ENABLE: &str = "ENABLE";
pub const CONTRIBUTOR_INSIGHTS_DISABLE: &str = "DISABLE";

/// The mode an enable that names none settles on: the wider of the two the model
/// declares, which tracks "all access and throttled events" rather than "throttled
/// events only" (`UpdateContributorInsights$ContributorInsightsMode`) — an enable that
/// asked for no narrowing asked for neither half to be dropped.
pub const DEFAULT_CONTRIBUTOR_INSIGHTS_MODE: &str = "ACCESSED_AND_THROTTLED_KEYS";

/// The separator between the table name and the index name in the key one contributor
/// insights summary pages by ([`contributor_insights_key`]).
///
/// A table name and an index name are `[a-zA-Z0-9_.-]` only (the `TableName` and
/// `IndexName` patterns), so this character can occur in neither: it cannot be mistaken
/// for part of a name, and it sorts below every character one can carry, which keeps a
/// table's own summary immediately ahead of its indexes' in the one ascending order
/// [`page_by_name`] walks.
const CONTRIBUTOR_INSIGHTS_KEY_SEPARATOR: char = '!';

/// The name one contributor insights summary is ordered and resumed by: the table it
/// belongs to, and the index after it where the summary is an index's.
pub fn contributor_insights_key(table: &str, index: Option<&str>) -> String {
    match index {
        Some(index) => format!("{table}{CONTRIBUTOR_INSIGHTS_KEY_SEPARATOR}{index}"),
        None => table.to_string(),
    }
}

/// The contributor insights setting of ONE table or index: what the last accepted
/// `UpdateContributorInsights` asked for, and when.
///
/// Only the REQUEST is stored; the status is derived at every read, because turning
/// insights on or off is asynchronous — the call leaves the setting `ENABLING` /
/// `DISABLING` and the clock owes it the move to `ENABLED` / `DISABLED` one
/// [`STATUS_TRANSITION`] later. A resource no request has ever named is `DISABLED` with
/// no timestamp at all: this API is the only way to turn insights on, and `CreateTable`
/// carries no contributor insights member.
#[derive(Debug, Clone, Default)]
pub struct ContributorInsights {
    /// Whether the last accepted action was `ENABLE`.
    enabled: bool,
    /// When that action was accepted, or `None` while no request has named this
    /// resource.
    requested_at: Option<SystemTime>,
    /// When the clock owes the move out of the transient status.
    settles_at: Option<SystemTime>,
    /// The mode in force while insights are on, as the request named it (or
    /// [`DEFAULT_CONTRIBUTOR_INSIGHTS_MODE`] where it named none). A `DISABLE` clears
    /// it: a setting that tracks nothing is tracking neither kind of event.
    mode: Option<String>,
}

impl ContributorInsights {
    /// Apply one `ContributorInsightsAction`, with the mode an enable names.
    ///
    /// The action is accepted whether or not it changes anything — re-stating `ENABLE`
    /// re-states the mode and starts the span again — so what a later read reports is
    /// always the last request's.
    pub fn apply(&mut self, enable: bool, mode: Option<&str>, now: SystemTime) {
        self.enabled = enable;
        self.requested_at = Some(now);
        self.settles_at = Some(now + STATUS_TRANSITION);
        self.mode = enable.then(|| {
            mode.unwrap_or(DEFAULT_CONTRIBUTOR_INSIGHTS_MODE)
                .to_string()
        });
    }

    /// The status at `now`: the transient one until the clock has made the move due,
    /// then the settled one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        let settled = self.settles_at.is_none_or(|due| now >= due);
        match (self.enabled, settled) {
            (true, true) => CONTRIBUTOR_INSIGHTS_ENABLED,
            (true, false) => CONTRIBUTOR_INSIGHTS_ENABLING,
            (false, true) => CONTRIBUTOR_INSIGHTS_DISABLED,
            (false, false) => CONTRIBUTOR_INSIGHTS_DISABLING,
        }
    }

    /// "Timestamp of the last time the status was changed"
    /// (`DescribeContributorInsightsOutput$LastUpdateDateTime`): the instant the status
    /// this read reports was reached — the call's own instant while the move is still
    /// owed, and the instant it fell due once it has. `None` for a resource no request
    /// has named, whose status has never changed.
    pub fn last_update(&self, now: SystemTime) -> Option<SystemTime> {
        match self.settles_at {
            Some(due) if now >= due => Some(due),
            _ => self.requested_at,
        }
    }

    /// The mode in force, which a setting that is off carries none of.
    pub fn mode(&self) -> Option<String> {
        self.mode.clone()
    }

    /// Whether this setting is one `ListContributorInsights` reports a summary for.
    ///
    /// The listing enumerates the contributor insights CONFIGURATIONS that exist — the
    /// resources insights are being turned on for, are on for, or are still being turned
    /// off for. A resource whose status has settled `DISABLED` has no configuration, so
    /// it carries no summary: it drops out of the listing once a `DISABLE` settles, and a
    /// resource no update has ever named was never in it. `DescribeContributorInsights`
    /// is what answers for a named resource either way, and reports `DISABLED` for it.
    pub fn listed(&self, now: SystemTime) -> bool {
        self.status(now) != CONTRIBUTOR_INSIGHTS_DISABLED
    }
}

// =============================================================================
// TIME TO LIVE
//
// One setting per table: whether items expire, and the attribute whose epoch-seconds
// value says when. `UpdateTimeToLive` writes it, `DescribeTimeToLive` reports it, and
// the store's own sweep ([`Table::apply_due_expirations`]) removes the items whose
// deletion the clock has already made due.
// =============================================================================

/// The four values `TimeToLiveStatus` declares
/// ([`crate::ops::types::TIME_TO_LIVE_STATUS_VALUES`]), named one per meaning: turning
/// a table's TTL on or off is asynchronous ("It can take up to one hour for the change
/// to fully process" — the `UpdateTimeToLive` doc), so the setting is reported in a
/// transient status until the clock has settled it.
pub const TIME_TO_LIVE_ENABLING: &str = "ENABLING";
pub const TIME_TO_LIVE_ENABLED: &str = "ENABLED";
pub const TIME_TO_LIVE_DISABLING: &str = "DISABLING";
pub const TIME_TO_LIVE_DISABLED: &str = "DISABLED";

/// How long after an item has expired its deletion falls due, and the shortest span
/// between two sweeps of one table.
///
/// DynamoDB states no fixed figure for this one: "DynamoDB deletes expired items on a
/// best-effort basis ... DynamoDB typically deletes expired items within two days of
/// expiration. The exact duration within which an item gets deleted after expiration is
/// specific to the nature of the workload" (the `UpdateTimeToLive` doc), which is an
/// upper bound on a best-effort sweep rather than a duration to keep. This emulator
/// therefore CHOOSES 500 ms: non-zero, so an item that has just expired is still served
/// to a reader ("Items that have expired and not been deleted will still show up in
/// reads, queries, and scans" — the same doc), and short enough that a client that waits
/// sees the deletion. It doubles as the sweep interval, so the walk one request can
/// trigger over a table's items runs at most once per span however many requests arrive.
pub const TTL_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How a call that turns TTL on while it is already on is refused: AWS's FIXED sentence
/// for that case, which names neither the table nor the attribute.
///
/// This is the refusal for a call that asks for the setting the table ALREADY has — the
/// same attribute, enabled again. A call naming another attribute is a different
/// refusal, which reports the attribute in force
/// ([`time_to_live_attribute_mismatch`]).
pub const TIME_TO_LIVE_ALREADY_ENABLED_MESSAGE: &str = "TimeToLive is already enabled";

/// The same for a call that turns TTL off while it is already off.
pub const TIME_TO_LIVE_ALREADY_DISABLED_MESSAGE: &str = "TimeToLive is already disabled";

/// How a specification that names an attribute OTHER than the one TTL is in force for is
/// refused: AWS's sentence for that case, whose variable part is the attribute this
/// table's items are currently expired by, read from the table's stored setting.
///
/// The TTL attribute of a table whose TTL is on is not something this API moves: a
/// specification naming another attribute asks for a TTL over an attribute that is not
/// the one in force, so it is refused and the stored one is reported back — whether the
/// specification was enabling TTL or disabling it, because either way the attribute it
/// names is not this table's.
pub fn time_to_live_attribute_mismatch(attribute_in_force: &str) -> String {
    format!(
        "TimeToLive is active on a different AttributeName: current AttributeName is \
         {attribute_in_force}"
    )
}

/// How a call that arrives while the last TTL change is still processing is refused:
/// "Any additional UpdateTimeToLive calls for the same table during this one hour
/// duration result in a ValidationException" (the `UpdateTimeToLive` doc). The span this
/// emulator processes a change over is [`STATUS_TRANSITION`], the one every other
/// transient status here is reported for.
pub const TIME_TO_LIVE_MODIFIED_TOO_OFTEN_MESSAGE: &str =
    "Time to live has been modified multiple times within a fixed interval";

/// One table's Time to Live setting: what the last accepted `UpdateTimeToLive` asked
/// for, when it was accepted, and when this table's items were last swept.
///
/// Only the REQUEST is stored; the status is derived at every read, for the reason
/// [`ContributorInsights`]'s is — the change is asynchronous, so the call leaves the
/// setting `ENABLING` / `DISABLING` and the clock owes it the move to `ENABLED` /
/// `DISABLED` one [`STATUS_TRANSITION`] later. A table no `UpdateTimeToLive` has named
/// is `DISABLED` with no attribute at all: this API is the only way to turn TTL on, and
/// `CreateTable` carries no TTL member.
#[derive(Debug, Clone, Default)]
pub struct TimeToLive {
    /// Whether the last accepted specification turned TTL on.
    enabled: bool,
    /// The attribute that specification named — the one whose value items expire by
    /// while TTL is on, and the one being withdrawn while it is going off. `None` until
    /// a request has named one.
    attribute_name: Option<String>,
    /// When the clock owes the move out of the transient status.
    settles_at: Option<SystemTime>,
    /// When this table's items were last swept for expiries, or `None` while they never
    /// have been.
    swept_at: Option<SystemTime>,
}

impl TimeToLive {
    /// Apply one `TimeToLiveSpecification`: the attribute it names comes into force (or
    /// starts being withdrawn) and the transient span starts again.
    pub fn apply(&mut self, enabled: bool, attribute_name: &str, now: SystemTime) {
        self.enabled = enabled;
        self.attribute_name = Some(attribute_name.to_string());
        self.settles_at = Some(now + STATUS_TRANSITION);
    }

    /// Whether the clock has already settled the last change this setting was given.
    pub fn settled(&self, now: SystemTime) -> bool {
        self.settles_at.is_none_or(|due| now >= due)
    }

    /// The status at `now`: the transient one until the clock has made the move due,
    /// then the settled one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match (self.enabled, self.settled(now)) {
            (true, true) => TIME_TO_LIVE_ENABLED,
            (true, false) => TIME_TO_LIVE_ENABLING,
            (false, true) => TIME_TO_LIVE_DISABLED,
            (false, false) => TIME_TO_LIVE_DISABLING,
        }
    }

    /// The attribute this table's items are expired by RIGHT NOW: the one the settled
    /// enable named, and `None` for a table whose TTL is off or still settling.
    pub fn attribute_in_force(&self, now: SystemTime) -> Option<&str> {
        match self.status(now) {
            TIME_TO_LIVE_ENABLED => self.attribute_name.as_deref(),
            _ => None,
        }
    }

    /// Project this setting into the `TimeToLiveDescription` the TTL reader reports:
    /// the status, and the attribute it is in force for.
    ///
    /// A setting that has settled OFF reports no attribute: nothing expires items on
    /// that table, so there is no TTL attribute for it to name — which is also what a
    /// table that has never had TTL reports.
    pub fn describe(&self, now: SystemTime) -> TimeToLiveDescription {
        let status = self.status(now);
        TimeToLiveDescription {
            time_to_live_status: Some(status.to_string()),
            attribute_name: match status {
                TIME_TO_LIVE_DISABLED => None,
                _ => self.attribute_name.clone(),
            },
        }
    }

    /// The attribute a sweep that is DUE at `now` expires items by, or `None` when this
    /// table has nothing to sweep yet.
    ///
    /// A sweep is due only while TTL is in force — a change that is still settling
    /// expires nothing, because the attribute it names is not the one items are being
    /// deleted by yet — and at most once per [`TTL_DELETION_DELAY`].
    fn due_sweep_attribute(&self, now: SystemTime) -> Option<String> {
        let in_force = self.status(now) == TIME_TO_LIVE_ENABLED;
        let due = self
            .swept_at
            .is_none_or(|last| now >= last + TTL_DELETION_DELAY);
        (in_force && due)
            .then(|| self.attribute_name.clone())
            .flatten()
    }

    /// Record that this table's items have just been swept.
    fn mark_swept(&mut self, now: SystemTime) {
        self.swept_at = Some(now);
    }
}

/// The epoch-seconds instant an item's TTL attribute must not be LATER than for its
/// deletion to be due at `now`: one [`TTL_DELETION_DELAY`] back, as an `N` value's text,
/// so the comparison runs through the same number parser a stored `N` was canonicalized
/// by ([`NumberKey`]).
fn expiry_cutoff(now: SystemTime) -> Option<NumberKey> {
    let delay = i128::try_from(TTL_DELETION_DELAY.as_millis()).unwrap_or(i128::MAX);
    let millis = epoch_millis(now).saturating_sub(delay);
    let sign = if millis < 0 { "-" } else { "" };
    let magnitude = millis.unsigned_abs();
    NumberKey::parse(&format!(
        "{sign}{}.{:03}",
        magnitude / 1_000,
        magnitude % 1_000
    ))
}

/// Whether one item's TTL attribute value has already made the item's deletion due.
///
/// "TTL compares the current time in epoch time format to the time stored in the TTL
/// attribute of an item. If the epoch time value stored in the attribute is less than
/// the current time, the item is marked as expired" (the `UpdateTimeToLive` doc): the
/// value is a NUMBER of seconds since the epoch, so an attribute of any other type
/// expires nothing — a string, a set or a boolean states no epoch time to compare.
fn expiry_due(value: &AttributeValue, cutoff: &NumberKey) -> bool {
    match value {
        AttributeValue::N(text) => {
            NumberKey::parse(text).is_some_and(|expires_at| expires_at <= *cutoff)
        }
        _ => false,
    }
}

// =============================================================================
// KINESIS DATA STREAM DESTINATIONS
//
// A table replicates its item changes to a Kinesis data stream through a
// DESTINATION record it carries: the enable command registers one, the update
// command changes the timestamp precision of a registered one, the disable
// command stops it, and the describing operation reports every one the table
// holds. Each record below is what those operations read and report; no value
// they answer with is canned.
// =============================================================================

/// The `DestinationStatus` values a destination record moves between
/// ([`crate::ops::types::DESTINATION_STATUS_VALUES`]), named one per meaning.
///
/// Registering, updating and stopping replication are all asynchronous, so a
/// destination is reported in a transient status (`ENABLING`, `UPDATING`,
/// `DISABLING`) for a span and in the settled one (`ACTIVE`, `DISABLED`) after it —
/// the same clock-owed move every other status in this store makes
/// ([`STATUS_TRANSITION`]).
pub const DESTINATION_STATUS_ENABLING: &str = "ENABLING";
pub const DESTINATION_STATUS_ACTIVE: &str = "ACTIVE";
pub const DESTINATION_STATUS_UPDATING: &str = "UPDATING";
pub const DESTINATION_STATUS_DISABLING: &str = "DISABLING";
pub const DESTINATION_STATUS_DISABLED: &str = "DISABLED";

/// How DynamoDB opens its refusal of a command against a table's Kinesis streaming
/// destination that the table's state cannot serve; the reason follows the colon.
const KINESIS_DESTINATION_INVALID_STATE: &str =
    "Table is not in a valid state to enable Kinesis Streaming Destination";

/// How a command that addresses ONE of a table's Kinesis streaming destinations is
/// refused when the table holds NO destination record for that stream at all: the stream
/// ARN the request named and the table it named it against, under the invalid-state
/// sentence above.
///
/// This is the lookup miss proper — the table has no record of ever replicating to that
/// stream. A record that EXISTS but is not `ACTIVE` is a state refusal instead
/// ([`kinesis_destination_not_active_to_update`]), because the two are different causes
/// for the caller: one is terminal, the other is waited out or re-enabled.
pub fn kinesis_destination_not_found(table: &str, stream_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: No streaming destination with streamArn: \
         {stream_arn} found for table with tableName: {table}"
    ))
}

/// How the command that STOPS replication is refused when the destination it addresses is
/// not `ACTIVE`: replication can only be stopped once it is running, and the refusal is
/// stated over that required status alone — it names neither the table nor the stream.
///
/// The same sentence answers every status that is not `ACTIVE`, a table that holds no
/// record for the stream included: in each case there is no running replication to stop.
pub fn kinesis_destination_not_active_to_disable() -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: KinesisStreamingDestination must be ACTIVE to \
         perform DISABLE operation."
    ))
}

/// How the command that CHANGES a destination's configuration is refused when the
/// destination it addresses exists but is not `ACTIVE`: the status the change requires,
/// over the table name and the Kinesis stream ARN the request addressed the destination
/// by (`kdsArn`).
pub fn kinesis_destination_not_active_to_update(table: &str, stream_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: Kinesis streaming is not in ACTIVE state. Updates \
         are only allowed in ACTIVE state. TableName: {table}, kdsArn: {stream_arn}"
    ))
}

/// How the command that STARTS replication is refused when the table already replicates
/// to that stream: the same invalid-state sentence, over the stream and table the
/// request named. Replication that is already on cannot be started again.
pub fn kinesis_destination_already_enabled(table: &str, stream_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "{KINESIS_DESTINATION_INVALID_STATE}: Streaming destination with streamArn: \
         {stream_arn} is already enabled for table with tableName: {table}"
    ))
}

/// How many Kinesis data stream destination records ONE table keeps.
///
/// The list grows by one per DISTINCT stream a command registers — a stream the table
/// already holds a record for is re-registered in place — and the stopped records are
/// the replication history the describing operation reports. AWS publishes no figure for
/// that history, so this store CHOOSES 64: room for a table's replication history to be
/// reported in full, and a bound, so no sequence of commands grows one table's record
/// list without end.
pub const MAX_KINESIS_DESTINATIONS_PER_TABLE: usize = 64;

/// The form a Kinesis data stream's ARN takes
/// (`arn:<partition>:kinesis:<region>:<account>:stream/<name>`), answering the stream it
/// names.
///
/// The `StreamArn` member is modelled as a length-bounded string only, so this is the
/// check the service itself makes: a value that names no Kinesis data stream addresses
/// no destination at all, which is an INPUT failure rather than a lookup miss, worded
/// the way this service words an ARN member it cannot read ([`check_table_arn`]).
pub fn check_stream_arn(stream_arn: &str) -> Result<String, RequestError> {
    let invalid = || {
        RequestError::Validation(format!(
            "Invalid StreamArn: Invalid StreamArn provided as input {stream_arn}"
        ))
    };
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and may itself carry `/`, so the split stops at six.
    let fields: Vec<&str> = stream_arn.splitn(6, ':').collect();
    let (partition, service, region, resource) = match fields.as_slice() {
        ["arn", partition, service, region, _account, resource] => {
            (*partition, *service, *region, *resource)
        }
        _ => return Err(invalid()),
    };
    if partition.is_empty() || service != "kinesis" || region.is_empty() {
        return Err(invalid());
    }
    let name = resource.strip_prefix("stream/").ok_or_else(invalid)?;
    if name.is_empty() || name.contains('/') {
        return Err(invalid());
    }
    Ok(name.to_string())
}

/// One Kinesis data stream destination of a table: the stream it replicates to, where
/// that replication is in its lifecycle, and the timestamp precision in force.
///
/// Only the last accepted command is stored; the status is derived at every read from
/// the clock, exactly as [`ContributorInsights`] derives its own.
#[derive(Debug, Clone)]
pub struct KinesisDestination {
    /// "The ARN for a specific Kinesis data stream"
    /// (`KinesisDataStreamDestination$StreamArn`) — the identity a command addresses
    /// this record by.
    pub stream_arn: String,
    /// The status this record settles on once the span has elapsed.
    settled_status: &'static str,
    /// The status it reports until then.
    transient_status: &'static str,
    /// When the clock owes the move from the transient status to the settled one.
    settles_at: SystemTime,
    /// "The precision of the Kinesis data stream timestamp"
    /// (`KinesisDataStreamDestination$ApproximateCreationDateTimePrecision`), or `None`
    /// while no command has named one: the member is absent until one does.
    precision: Option<String>,
}

impl KinesisDestination {
    /// Register one destination, as the command that starts replication leaves it:
    /// `ENABLING` for the span, then `ACTIVE`.
    pub fn enabling(stream_arn: &str, precision: Option<&str>, now: SystemTime) -> Self {
        Self {
            stream_arn: stream_arn.to_string(),
            settled_status: DESTINATION_STATUS_ACTIVE,
            transient_status: DESTINATION_STATUS_ENABLING,
            settles_at: now + STATUS_TRANSITION,
            precision: precision.map(str::to_string),
        }
    }

    /// The status at `now`: the transient one until the clock has made the move due,
    /// then the settled one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.settles_at {
            self.transient_status
        } else {
            self.settled_status
        }
    }

    /// Whether this record still REPLICATES — the destination a command that addresses
    /// a table's live replication to this stream acts on. A record whose replication has
    /// stopped, or is stopping, is history: it is still reported, and no longer
    /// addressable.
    pub fn replicating(&self, now: SystemTime) -> bool {
        let status = self.status(now);
        status != DESTINATION_STATUS_DISABLED && status != DESTINATION_STATUS_DISABLING
    }

    /// Whether this record is `ACTIVE` at `now` — replication running, with no
    /// asynchronous change of its own still owed.
    ///
    /// This is the status a command that CHANGES the destination requires: while a
    /// registration, a configuration change or a stop is still in flight the destination
    /// is in a transient status, and the change is refused on that status.
    pub fn active(&self, now: SystemTime) -> bool {
        self.status(now) == DESTINATION_STATUS_ACTIVE
    }

    /// Apply one update of this destination's configuration: the record is `UPDATING`
    /// for the span and `ACTIVE` after it, carrying the precision the command named.
    ///
    /// A command that names no precision changes none — it asks for no configuration
    /// change — and still restarts the span, because the service accepted the command.
    pub fn apply_update(&mut self, precision: Option<&str>, now: SystemTime) {
        if let Some(precision) = precision {
            self.precision = Some(precision.to_string());
        }
        self.settled_status = DESTINATION_STATUS_ACTIVE;
        self.transient_status = DESTINATION_STATUS_UPDATING;
        self.settles_at = now + STATUS_TRANSITION;
    }

    /// The timestamp precision in force, which a destination no command has named one
    /// for carries none of.
    pub fn precision(&self) -> Option<String> {
        self.precision.clone()
    }

    /// Apply the command that STOPS replication: the record is `DISABLING` for the span
    /// and `DISABLED` after it, keeping the stream it replicated to and the precision it
    /// replicated under — a stopped destination is still reported, as this table's
    /// replication history.
    pub fn apply_disable(&mut self, now: SystemTime) {
        self.settled_status = DESTINATION_STATUS_DISABLED;
        self.transient_status = DESTINATION_STATUS_DISABLING;
        self.settles_at = now + STATUS_TRANSITION;
    }

    /// Project this record into the `KinesisDataStreamDestination` the describing
    /// operation reports.
    ///
    /// `DestinationStatusDescription` is "the human-readable string that corresponds to
    /// the replica status" — a description of a FAILURE, which a record that replicates
    /// has none of, so the member stays absent.
    pub fn describe(&self, now: SystemTime) -> KinesisDataStreamDestination {
        KinesisDataStreamDestination {
            stream_arn: Some(self.stream_arn.clone()),
            destination_status: Some(self.status(now).to_string()),
            destination_status_description: None,
            approximate_creation_date_time_precision: self.precision(),
        }
    }
}

// =============================================================================
// ON-DEMAND BACKUPS
//
// A backup is a resource of its own: `CreateBackup` catalogues one from a table,
// `ListBackups` reports the ones a region holds, and `DescribeBackup`/`DeleteBackup`
// address one by the `BackupArn` minted for it. Everything the four of them report is
// read from the records below.
// =============================================================================

/// The `BackupStatus` a backup is in while it is still being taken: "the backup is
/// created asynchronously" (the `CreateBackup` doc), so a backup is `CREATING` for a
/// span before it is ready.
pub const BACKUP_STATUS_CREATING: &str = "CREATING";

/// The `BackupStatus` a backup is in once that span has elapsed and it can be restored
/// from.
pub const BACKUP_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `BackupStatus` reported for the backup a `DeleteBackup` has just removed — the
/// third and last value the model's `BackupStatus` enum declares
/// ([`crate::ops::types::BACKUP_STATUS_VALUES`]).
pub const BACKUP_STATUS_DELETED: &str = "DELETED";

/// The `BackupType` of a backup a CALLER asked for: "USER - On-demand backup created by
/// you" (`ListBackups$BackupType`). Every backup this API catalogues is one.
pub const BACKUP_TYPE_USER: &str = "USER";

/// The `BackupType` of a backup the service took by itself: "SYSTEM - On-demand backup
/// automatically created by DynamoDB" (`ListBackups$BackupType`). Nothing here takes a
/// backup on its own, so no stored backup carries it — it is named only by the filter
/// rule below.
pub const BACKUP_TYPE_SYSTEM: &str = "SYSTEM";

/// The `BackupTypeFilter` a list that names none is answered for: "USER - On-demand
/// backup created by you. (The default setting if no other backup types are specified.)"
/// (`ListBackups$BackupType`).
pub const DEFAULT_BACKUP_TYPE_FILTER: &str = "USER";

/// The `BackupTypeFilter` that spans several types at once: "ALL - All types of
/// on-demand backups (USER and SYSTEM)" (`ListBackups$BackupType`).
pub const BACKUP_TYPE_FILTER_ALL: &str = "ALL";

/// The most backups this emulator will hold IN ONE REGION.
///
/// AWS states no ceiling — "There is no limit to the number of on-demand backups that
/// can be taken" (the `CreateBackup` doc) — so this one is this emulator's own, and it
/// is the per-region table quota over again ([`MAX_TABLES`]): a backup is catalogued
/// from a table, and this keeps the catalogue of one region bounded by the number of
/// tables that region may hold. Reaching it answers the modelled
/// `LimitExceededException` rather than growing without end.
///
/// It bounds the whole catalogue too: a backup is only ever catalogued from a table of
/// its own region, and the table store holds at most [`MAX_REGIONS`] regions, so the
/// records here can never outnumber `MAX_REGIONS * MAX_BACKUPS_PER_REGION`.
pub const MAX_BACKUPS_PER_REGION: usize = MAX_TABLES;

/// How many bytes of ITEM DATA the whole backup catalogue may hold.
///
/// A backup keeps the items its source table held at the instant it was taken — that is
/// what a restore writes into the new table — so the catalogue is a growth site of its
/// own, beside the tables themselves. AWS states no ceiling here either, so this is the
/// table store's own budget over again ([`MAX_STORE_SIZE_BYTES`]): a `CreateBackup` that
/// would push the catalogue past it answers the modelled `LimitExceededException`
/// instead of growing the process without end.
pub const MAX_BACKUP_CATALOGUE_BYTES: i64 = MAX_STORE_SIZE_BYTES;

/// How many restores may be in flight at once: "Any number of users can execute up to 50
/// concurrent restores (any type of restore) in a given account" (the
/// `RestoreTableFromBackup` doc, repeated on `RestoreTableToPointInTime`). A restore past
/// it answers the modelled `LimitExceededException`.
pub const MAX_CONCURRENT_RESTORES: usize = 50;

/// Whether a stored `BackupType` is one a request's `BackupTypeFilter` selects.
///
/// One rule for every filter value the model declares: `ALL` spans the on-demand types
/// this API catalogues, and any other value selects exactly the type it names. A backup
/// "made with Amazon Web Services Backup" is listed by that service's own API rather
/// than this one (the `ListBackups` doc), so none is ever stored here to select.
pub fn backup_type_selected(filter: &str, stored: &str) -> bool {
    match filter {
        BACKUP_TYPE_FILTER_ALL => stored == BACKUP_TYPE_USER || stored == BACKUP_TYPE_SYSTEM,
        named => named == stored,
    }
}

/// Whether `instant` falls inside the time range a request named: at or after the lower
/// bound and strictly before the upper one — "In the request, start time is inclusive,
/// but end time is exclusive" (the `ListBackups` doc). A bound the request omits bounds
/// nothing.
///
/// Both sides are compared at [`at_timestamp_precision`], the precision a reported
/// timestamp carries, so a bound a client formed from an instant this service reported
/// selects exactly what it names rather than landing a fraction either side of it.
pub fn within_time_range(
    instant: SystemTime,
    lower: Option<&::aws_smithy_types::DateTime>,
    upper: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let instant = epoch_millis(instant);
    lower.is_none_or(|bound| instant >= wire_millis(bound))
        && upper.is_none_or(|bound| instant < wire_millis(bound))
}

/// How precise an instant this service reports and compares is: whole milliseconds, the
/// precision an AWS timestamp carries on the wire — and the one DynamoDB's own backup
/// identifier is built from, since a `BackupArn`'s last segment is the epoch millisecond
/// of the request.
///
/// This returns `instant` at that precision, which is what a record stores: the instant
/// it reports is then exactly the instant a client can send back.
pub fn at_timestamp_precision(instant: SystemTime) -> SystemTime {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => {
            UNIX_EPOCH
                + Duration::from_millis(u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        }
        // An instant before the epoch is not one this store mints; it is left as it is
        // rather than moved.
        Err(_) => instant,
    }
}

/// `instant` at the NEAREST reported timestamp, for an instant a client ECHOED BACK.
///
/// [`at_timestamp_precision`] truncates, which is right for an instant this service is
/// MINTING. A value that has travelled out through the wire's decimal form and back can
/// land a fraction either side of the instant it names, so one being read is rounded
/// instead: a client that sends back a timestamp this service reported then names exactly
/// that instant rather than the millisecond below it.
pub fn at_nearest_timestamp_precision(instant: SystemTime) -> SystemTime {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => {
            let millis = since.as_nanos().saturating_add(500_000) / 1_000_000;
            // An instant so far ahead that rounding it cannot be represented is left as
            // it is rather than wrapped: the value arrives from a request, so no caller
            // can steer this into an overflow.
            UNIX_EPOCH
                .checked_add(Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX)))
                .unwrap_or(instant)
        }
        Err(_) => instant,
    }
}

fn epoch_millis(instant: SystemTime) -> i128 {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => i128::try_from(since.as_millis()).unwrap_or(i128::MAX),
        Err(before) => i128::try_from(before.duration().as_millis())
            .map_or(i128::MIN, |millis| -millis),
    }
}

/// The same figure for a timestamp a request carried.
fn wire_millis(timestamp: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(timestamp.secs()) * 1_000 + i128::from(timestamp.subsec_nanos() / 1_000_000)
}

/// A new backup's `BackupArn`:
/// `arn:aws:dynamodb:<region>:<account>:table/<table>/backup/<millis>-<id>`, built
/// through the same core ARN helper every other identifier here is minted with
/// ([`table_arn`]).
///
/// The last segment is the instant the backup was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix drawn from the `uuid` crate's v4 generator
/// (OS randomness): the instant alone does not identify a backup, because two calls can
/// land in the same millisecond. Minted ONCE, at create, and stored on the backup, so
/// every later read reports the same ARN — and because the fixed-width instant sits
/// inside it, ascending ARN order IS ascending (table, request instant) order, which is
/// what the `ExclusiveStartBackupArn` cursor pages through.
pub fn new_backup_arn(region: &str, table: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "table/{table}/backup/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// One stored on-demand backup: what the request asked for, when it was catalogued, and
/// the snapshot of the source table taken at that instant.
///
/// A backup is a RESOURCE OF ITS OWN rather than part of a table: it is addressed by its
/// own ARN, it outlives the table it was taken from, and every member a backup operation
/// reports is read from here.
#[derive(Debug, Clone)]
pub struct Backup {
    /// The `BackupArn` minted for this backup at create ([`new_backup_arn`]), which is
    /// also the key the store holds it under.
    pub arn: String,
    /// The `BackupName` the request named.
    pub name: String,
    /// The region this backup lives in — the one its source table lives in, which is the
    /// scope every backup operation resolves inside.
    pub region: String,
    pub table_name: String,
    pub table_id: String,
    pub table_arn: String,
    /// `USER` for every backup catalogued through `CreateBackup` ([`BACKUP_TYPE_USER`]).
    pub backup_type: String,
    /// The bytes the source table held at the instant of the request, which is what this
    /// backup holds: "the entire table data is backed up" (the `CreateBackup` doc).
    pub size_bytes: i64,
    /// When the backup was requested: "a time marker of the request is cataloged" (the
    /// `CreateBackup` doc), which is the `BackupCreationDateTime` every read reports and
    /// the instant `ListBackups`'s time range is measured against ("these boundaries are
    /// for the time at which the original backup was requested" — the `ListBackups` doc).
    pub created_at: SystemTime,
    /// When the clock owes this backup its `CREATING` -> `AVAILABLE` transition, one
    /// [`STATUS_TRANSITION`] after the request: the backup "is created asynchronously"
    /// and becomes "available for restore" afterwards (the `CreateBackup` doc). Derived
    /// from the stored instant on every read, so nothing sleeps and no read writes.
    pub available_at: SystemTime,
    /// When this backup expires by itself, for a kind that does: "Time at which the
    /// automatic on-demand backup created by DynamoDB will expire" — a `SYSTEM` backup's
    /// own member (`BackupDetails$BackupExpiryDateTime`). A backup a caller asked for
    /// does not expire, so this is `None` for every backup this API catalogues.
    pub expires_at: Option<SystemTime>,
    /// When the restore in flight from this backup finishes, while one is: a backup "is
    /// either being created, deleted or restored to a table" (`BackupInUseException`),
    /// so for that span the backup is in use and no second backup control-plane call may
    /// have it. `None` once no restore is in flight; whether the span has elapsed is
    /// derived from the clock on every read, so nothing sleeps.
    pub restored_until: Option<SystemTime>,
    /// The source table as it stood when the backup was requested.
    pub source: SourceTableSnapshot,
    /// The ITEMS the source table held at the instant the backup was requested, which is
    /// the data this backup holds: "the entire table data is backed up" (the
    /// `CreateBackup` doc). Copied HERE rather than read off the table later, because a
    /// backup outlives the table it was taken from — and they are what
    /// `RestoreTableFromBackup` writes into the table it creates.
    pub items: BTreeMap<ItemKey, Item>,
}

/// The source table as a backup records it: the configuration and figures
/// `SourceTableDetails` and `SourceTableFeatureDetails` report for the backup, frozen at
/// the instant the backup was requested rather than read off the table later — the table
/// may have moved on, or be gone.
#[derive(Debug, Clone)]
pub struct SourceTableSnapshot {
    pub key_schema: Vec<KeySchemaElement>,
    /// The attribute definitions the source table declared. No backup reader reports
    /// them — `SourceTableDetails` carries none — but they are part of the table's
    /// schema, so a table restored from this backup declares the same ones.
    pub attribute_definitions: Vec<AttributeDefinition>,
    pub table_creation_date_time: SystemTime,
    pub billing_mode: String,
    /// The capacity the table was provisioned for, as a description reports it: zeroes
    /// for an on-demand table ([`reported_capacity`]).
    pub provisioned_throughput: ProvisionedThroughput,
    pub item_count: i64,
    pub table_size_bytes: i64,
    pub global_secondary_indexes: Vec<GlobalSecondaryIndexInfo>,
    pub local_secondary_indexes: Vec<LocalSecondaryIndexInfo>,
    /// The vector indexes the source table carried, as `SourceTableFeatureDetails`
    /// reports them — and the configuration a table restored from this backup rebuilds
    /// them from, since "all vector indexes from the backup are restored"
    /// (`RestoreTableFromBackup$VectorIndexOverride`).
    pub vector_indexes: Vec<VectorIndexInfo>,
    /// The table's stream configuration, for a table that had a stream.
    pub stream_specification: Option<StreamSpecification>,
    /// The table's Time to Live setting as it stood then, which is what
    /// `SourceTableFeatureDetails$TimeToLiveDescription` reports for this backup.
    pub time_to_live_description: TimeToLiveDescription,
}

impl Backup {
    /// The backup a `CreateBackup` at `now` catalogues from `table`.
    pub fn taken_from(table: &Table, backup_name: &str, now: SystemTime) -> Self {
        // The time marker this backup is catalogued under, at the precision it is
        // reported in, so the instant in its ARN, the instant it reports and the instant
        // a client can send back as a range bound are all the same one.
        let created_at = at_timestamp_precision(now);
        Self {
            arn: new_backup_arn(&table.region, &table.name, created_at),
            name: backup_name.to_string(),
            region: table.region.clone(),
            table_name: table.name.clone(),
            table_id: table.id.clone(),
            table_arn: table.arn.clone(),
            backup_type: BACKUP_TYPE_USER.to_string(),
            size_bytes: table.stored_bytes,
            created_at,
            available_at: now + STATUS_TRANSITION,
            expires_at: None,
            restored_until: None,
            source: table.backup_snapshot(now),
            items: table.items.clone(),
        }
    }

    /// The status at `now`, derived from the stored instants.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.available_at {
            BACKUP_STATUS_CREATING
        } else {
            BACKUP_STATUS_AVAILABLE
        }
    }

    /// Whether the clock still owes this backup its `CREATING` -> `AVAILABLE`
    /// transition, which is what a conflicting backup operation on the table is.
    pub fn still_creating(&self, now: SystemTime) -> bool {
        now < self.available_at
    }

    /// Whether a restore from this backup is still in flight at `now`.
    pub fn still_restoring(&self, now: SystemTime) -> bool {
        self.restored_until.is_some_and(|until| now < until)
    }

    /// Whether another backup control-plane operation still has this backup, so a call
    /// that needs it to itself is refused: "The backup is either being created, deleted
    /// or restored to a table" (`BackupInUseException`).
    pub fn in_use(&self, now: SystemTime) -> bool {
        self.still_creating(now) || self.still_restoring(now)
    }

    /// How a backup another operation still has is refused, naming the operation that
    /// has it — ONE sentence per in-flight operation, so the two callers that report
    /// `BackupInUseException` cannot word the same conflict differently. A backup that is
    /// in use for neither reason has nothing to report.
    pub fn in_use_message(&self, now: SystemTime) -> Option<String> {
        if self.still_creating(now) {
            return Some(format!("Backup is being created: {}", self.arn));
        }
        if self.still_restoring(now) {
            return Some(format!("Backup is being used to restore table: {}", self.arn));
        }
        None
    }

    /// Project this backup into the `BackupDetails` every backup operation reports,
    /// in the status the CALLING operation reports it in.
    pub fn details(&self, status: &str) -> BackupDetails {
        BackupDetails {
            backup_arn: self.arn.clone(),
            backup_name: self.name.clone(),
            backup_size_bytes: Some(self.size_bytes),
            backup_status: status.to_string(),
            backup_type: self.backup_type.clone(),
            backup_creation_date_time: ::aws_smithy_types::DateTime::from(self.created_at),
            backup_expiry_date_time: self.expires_at.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Project this backup into the `BackupSummary` a list reports, as it stands at
    /// `now`.
    pub fn summary(&self, now: SystemTime) -> BackupSummary {
        BackupSummary {
            table_name: Some(self.table_name.clone()),
            table_id: Some(self.table_id.clone()),
            table_arn: Some(self.table_arn.clone()),
            backup_arn: Some(self.arn.clone()),
            backup_name: Some(self.name.clone()),
            backup_creation_date_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            backup_expiry_date_time: self.expires_at.map(::aws_smithy_types::DateTime::from),
            backup_status: Some(self.status(now).to_string()),
            backup_type: Some(self.backup_type.clone()),
            backup_size_bytes: Some(self.size_bytes),
        }
    }

    /// Project this backup into the `BackupDescription` the two single-backup operations
    /// report: the backup itself, plus the source table as this backup recorded it.
    pub fn describe(&self, status: &str) -> BackupDescription {
        BackupDescription {
            backup_details: Some(self.details(status)),
            source_table_details: Some(SourceTableDetails {
                table_name: self.table_name.clone(),
                table_id: self.table_id.clone(),
                table_arn: Some(self.table_arn.clone()),
                table_size_bytes: Some(self.source.table_size_bytes),
                key_schema: self.source.key_schema.clone(),
                table_creation_date_time: ::aws_smithy_types::DateTime::from(
                    self.source.table_creation_date_time,
                ),
                provisioned_throughput: self.source.provisioned_throughput.clone(),
                on_demand_throughput: None,
                item_count: Some(self.source.item_count),
                billing_mode: Some(self.source.billing_mode.clone()),
            }),
            source_table_feature_details: Some(SourceTableFeatureDetails {
                // A table that had no secondary index of a kind reports no list for it,
                // exactly as its own description does.
                local_secondary_indexes: (!self.source.local_secondary_indexes.is_empty())
                    .then(|| self.source.local_secondary_indexes.clone()),
                global_secondary_indexes: (!self.source.global_secondary_indexes.is_empty())
                    .then(|| self.source.global_secondary_indexes.clone()),
                vector_indexes: (!self.source.vector_indexes.is_empty())
                    .then(|| self.source.vector_indexes.clone()),
                stream_description: self.source.stream_specification.clone(),
                time_to_live_description: Some(self.source.time_to_live_description.clone()),
                ..SourceTableFeatureDetails::default()
            }),
        }
    }
}

/// The `BackupNotFoundException` message a backup operation answers an ARN it holds no
/// backup for with: built from the request — the ARN the caller addressed — and nothing
/// about what the catalogue does hold.
pub fn backup_not_found(backup_arn: &str) -> RequestError {
    RequestError::NotFound(format!("Backup not found: {backup_arn}"))
}

/// How a write this emulator's own host-protection ceilings cannot admit is refused
/// ([`MAX_ITEMS_PER_TABLE`], [`MAX_TABLE_SIZE_BYTES`], [`MAX_STORE_SIZE_BYTES`]).
///
/// NOT an AWS quota, and not a value the request got wrong: DynamoDB bounds ONE item and
/// not what a table holds, and these ceilings sit far above the data any workload stores.
/// So the message names THE EMULATOR as what ran out, and says the service itself imposes
/// no such limit, rather than sending a client to fix its own input; it is the one refusal
/// this store answers for its own exhaustion.
fn store_exhausted(table: &str) -> RequestError {
    RequestError::Validation(format!(
        "This emulator cannot hold more data for table {table}: its in-process item store \
         is full (at most {MAX_ITEMS_PER_TABLE} items and {MAX_TABLE_SIZE_BYTES} bytes per \
         table, and {MAX_STORE_SIZE_BYTES} bytes across all tables). DynamoDB itself \
         imposes no such limit; delete items to make room."
    ))
}

/// Hold an ARN-addressed BACKUP operation's `BackupArn` to the form such an ARN takes in
/// the scope the request itself was made in.
///
/// The same two-part rule [`resolve_resource_arn`] states for a table ARN, over the
/// deeper form a backup carries
/// (`arn:<partition>:dynamodb:<region>:<account>:table/<table>/backup/<id>`): the value
/// must be an ARN of this service naming a backup, and it must address the partition,
/// account and REGION this request reached — which is exactly the prefix [`table_arn`]
/// mints for a table of this region. Anything else names no backup whose existence could
/// be looked up, so it is an INPUT failure ([`RequestError::Validation`]) rather than a
/// missing resource, and only a well-formed, in-scope ARN reaches the catalogue.
pub fn check_backup_arn(backup_arn: &str, request_region: &str) -> Result<(), RequestError> {
    let invalid = || {
        RequestError::Validation(format!(
            "Invalid BackupArn: Invalid BackupArn provided as input {backup_arn}"
        ))
    };
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and carries `/`, so the split stops at six.
    let fields: Vec<&str> = backup_arn.splitn(6, ':').collect();
    let resource = match fields.as_slice() {
        ["arn", _partition, "dynamodb", _region, _account, resource] => *resource,
        _ => return Err(invalid()),
    };
    let (table, backup) = resource
        .strip_prefix("table/")
        .and_then(|path| path.split_once("/backup/"))
        .ok_or_else(invalid)?;
    if backup.is_empty() || backup.contains('/') {
        return Err(invalid());
    }
    let table = resolve_table_name(table).map_err(|_| invalid())?;
    if backup_arn != format!("{}/backup/{backup}", table_arn(request_region, &table)) {
        return Err(invalid());
    }
    Ok(())
}

// =============================================================================
// Imports of table data: the records the import surface reports
// =============================================================================

/// How far back a listing of imports reaches: "Lists completed imports within the past
/// 90 days" (the `ListImports` doc), so a record older than that window is no longer
/// reported. Measured on every read from the instant the import finished, and from the
/// instant it began while it is still running, so nothing sleeps and no read writes.
pub const IMPORT_HISTORY_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// A new import's `ImportArn`:
/// `arn:aws:dynamodb:<region>:<account>:table/<table>/import/<millis>-<id>`, built
/// through the same core ARN helper every other identifier here is minted with
/// ([`table_arn`]), in the same construction as [`new_backup_arn`].
///
/// The last segment is the instant the import was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix: the instant alone does not identify an
/// import, because two calls can land in the same millisecond. Minted ONCE, when the
/// import is recorded, and stored on it — and because the fixed-width instant sits
/// inside it, ascending ARN order IS ascending (table, request instant) order, which is
/// the order a listing reports and the `NextToken` cursor pages through.
pub fn new_import_arn(region: &str, table: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "table/{table}/import/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// One recorded import of table data: the source it read, the table it wrote into, and
/// how far it got.
///
/// An import is a RESOURCE OF ITS OWN rather than part of a table: it is addressed by its
/// own ARN, it outlives the table it imported into, and every member the import readers
/// report is read from here.
#[derive(Debug, Clone)]
pub struct Import {
    /// The `ImportArn` minted for this import when it was recorded
    /// ([`new_import_arn`]), which is also the key the store holds it under.
    pub arn: String,
    /// The region this import lives in — the one the table it imported into lives in,
    /// which is the scope every import operation resolves inside.
    pub region: String,
    /// The name of the table this import wrote into. The `TableArn` a reader reports is
    /// minted from it and this region ([`table_arn`]), so one stored table identity
    /// cannot drift from the ARN the table reports for itself.
    pub table_name: String,
    /// Where this import stands: one of the `ImportStatus` values the model declares.
    pub status: String,
    /// "The S3 bucket that provides the source for the import"
    /// (`ImportTable$S3BucketSource`), as the request named it.
    pub s3_bucket_source: S3BucketSource,
    /// The log group the import wrote its errors to, where one was made for it
    /// (`ImportSummary$CloudWatchLogGroupArn`).
    pub cloud_watch_log_group_arn: Option<String>,
    /// "The format of the source data" (`ImportSummary$InputFormat`): one of the
    /// `InputFormat` values the model declares.
    pub input_format: String,
    /// "The time at which this import task began" (`ImportSummary$StartTime`).
    pub started_at: SystemTime,
    /// "The time at which this import task ended" (`ImportSummary$EndTime`) — `None`
    /// while the import is still running, so no reader reports an end for work that has
    /// not finished.
    pub ended_at: Option<SystemTime>,
}

impl Import {
    /// This import as `ListImports` reports it: every member read from the record.
    pub fn summary(&self) -> ImportSummary {
        ImportSummary {
            import_arn: Some(self.arn.clone()),
            import_status: Some(self.status.clone()),
            table_arn: Some(table_arn(&self.region, &self.table_name)),
            s3_bucket_source: Some(self.s3_bucket_source.clone()),
            cloud_watch_log_group_arn: self.cloud_watch_log_group_arn.clone(),
            input_format: Some(self.input_format.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Whether this record still falls inside the window a listing reaches back over
    /// ([`IMPORT_HISTORY_WINDOW`]), measured at `now` against the instant the import
    /// finished — or, while it is still running, the instant it began.
    pub fn within_history_window(&self, now: SystemTime) -> bool {
        let marker = self.ended_at.unwrap_or(self.started_at);
        match now.duration_since(marker) {
            Ok(age) => age < IMPORT_HISTORY_WINDOW,
            // Recorded at or after `now`: inside any window measured back from it.
            Err(_) => true,
        }
    }
}

// =============================================================================
// Exports of table data: the records the export surface reports
// =============================================================================

/// How far back a listing of exports reaches: "Lists completed exports within the past
/// 90 days" (the `ListExports` doc). The same span [`IMPORT_HISTORY_WINDOW`] carries,
/// named once and shared, so the two listings of this service cannot drift apart.
/// Measured on every read from the instant the export finished, and from the instant it
/// began while it is still running, so nothing sleeps and no read writes.
pub const EXPORT_HISTORY_WINDOW: Duration = IMPORT_HISTORY_WINDOW;

/// A new export's `ExportArn`:
/// `arn:aws:dynamodb:<region>:<account>:table/<table>/export/<millis>-<id>`, built
/// through the same core ARN helper every other identifier here is minted with
/// ([`table_arn`]), in the same construction as [`new_import_arn`].
///
/// The last segment is the instant the export was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix: the instant alone does not identify an
/// export, because two calls can land in the same millisecond. Minted ONCE, when the
/// export is recorded, and stored on it — and because the fixed-width instant sits
/// inside it, descending ARN order IS descending (table, request instant) order, which
/// is the "reverse alphanumeric order of ExportArn" a listing reports (the
/// `ListExports` doc) and the order its `NextToken` cursor pages through.
pub fn new_export_arn(region: &str, table: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "table/{table}/export/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// One recorded export of a table's data: the table it read, what kind of export it
/// was, and how far it got.
///
/// An export is a RESOURCE OF ITS OWN rather than part of a table, for the reason
/// [`Import`] is: it is addressed by its own ARN, it outlives the table it was taken
/// from, and every member the export readers report is read from here.
#[derive(Debug, Clone)]
pub struct Export {
    /// The `ExportArn` minted for this export when it was recorded
    /// ([`new_export_arn`]), which is also the key the store holds it under.
    pub arn: String,
    /// The region this export lives in — the one the table it read lives in, which is
    /// the scope every export operation resolves inside.
    pub region: String,
    /// The name of the table this export read. The `TableArn` an export request selects
    /// by is minted from it and this region ([`table_arn`]), so one stored table
    /// identity cannot drift from the ARN the table reports for itself.
    pub table_name: String,
    /// Where this export stands: one of the `ExportStatus` values the model declares.
    /// "Export can be in one of the following states: IN_PROGRESS, COMPLETED, or FAILED"
    /// (`ExportSummary$ExportStatus`), so a record that has not finished is still a
    /// record a reader reports.
    pub status: String,
    /// "The type of export that was performed" (`ExportSummary$ExportType`): one of the
    /// `ExportType` values the model declares.
    pub export_type: String,
    /// The instant this export task began (`ExportDescription$StartTime`).
    pub started_at: SystemTime,
    /// The instant this export task ended (`ExportDescription$EndTime`) — `None` while
    /// the export is still running, so no reader reports an end for work that has not
    /// finished.
    pub ended_at: Option<SystemTime>,
}

impl Export {
    /// This export as `ListExports` reports it: every member read from the record.
    pub fn summary(&self) -> ExportSummary {
        ExportSummary {
            export_arn: Some(self.arn.clone()),
            export_status: Some(self.status.clone()),
            export_type: Some(self.export_type.clone()),
        }
    }

    /// Whether this record still falls inside the window a listing reaches back over
    /// ([`EXPORT_HISTORY_WINDOW`]), measured at `now` against the instant the export
    /// finished — or, while it is still running, the instant it began.
    pub fn within_history_window(&self, now: SystemTime) -> bool {
        let marker = self.ended_at.unwrap_or(self.started_at);
        match now.duration_since(marker) {
            Ok(age) => age < EXPORT_HISTORY_WINDOW,
            // Recorded at or after `now`: inside any window measured back from it.
            Err(_) => true,
        }
    }
}

/// Hold an ARN-addressed TABLE member to the form such an ARN takes in the scope the
/// request itself was made in, answering the table it names.
///
/// For a member the model calls `TableArn` and documents as "The Amazon Resource Name
/// (ARN) associated with the table" (`ListImports$TableArn`) — not one of the `TableName`
/// members that "can also" carry an ARN — so the value is held to the same two-part
/// SHAPE and SCOPE rule [`resolve_resource_arn`] states, through that one function.
/// Anything else addresses no table from this endpoint, so it is an INPUT failure rather
/// than a lookup miss, stated over the member the request actually carries.
pub fn check_table_arn(table_arn: &str, request_region: &str) -> Result<String, RequestError> {
    resolve_resource_arn(table_arn, request_region)
        .map(|resource| resource.table)
        .map_err(|_| {
            RequestError::Validation(format!(
                "Invalid TableArn: Invalid TableArn provided as input {table_arn}"
            ))
        })
}

/// One stored table: its configuration, its lifecycle instants, and its items.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub arn: String,
    /// This table's `TableId`, minted by [`new_table_id`] when the table was created and
    /// never changed after: "A unique identifier for the table, in UUID format,
    /// generated by DynamoDB when the table is created" (`TableDescription$TableId`).
    pub id: String,
    /// The region this table lives in. A table name is unique only within a region
    /// ("table names must be unique within each Region" — the `CreateTable` doc), so
    /// the store keys by region and the table remembers which one it answers for.
    pub region: String,
    pub key_schema: Vec<KeySchemaElement>,
    pub attribute_definitions: Vec<AttributeDefinition>,
    /// `PROVISIONED` or `PAY_PER_REQUEST`, as resolved at CreateTable and moved by
    /// `UpdateTable`.
    pub billing_mode: String,
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    /// The warm throughput in force, which every description reports as
    /// `WarmThroughput`.
    pub warm_throughput: WarmThroughputSettings,
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
    /// The vector indexes defined on this table, in the order they were declared. Like
    /// the secondary indexes they are projected into a description on every read and
    /// never stored as one.
    pub vector_indexes: Vec<VectorIndex>,
    pub table_class: Option<String>,
    /// When the table's class was last UPDATED, which is what
    /// `TableClassSummary.LastUpdateDateTime` reports: "the date and time at which the
    /// table class was last updated" (that member's doc). `None` while the class is
    /// still the one the table was created with, which no call has updated.
    pub table_class_updated_at: Option<SystemTime>,
    pub deletion_protection_enabled: bool,
    /// This table's continuous-backups settings: whether point-in-time recovery is on
    /// and for how long a period, as `UpdateContinuousBackups` left them and both
    /// continuous-backups operations report them.
    pub point_in_time_recovery: PointInTimeRecovery,
    /// The tags this table carries: the set `CreateTable` seeded and every later
    /// `TagResource`/`UntagResource` edited, together with the set the eventually
    /// consistent `ListTagsOfResource` can see yet (see [`ResourceTags`]).
    pub tags: ResourceTags,
    pub creation_date_time: SystemTime,
    /// When the clock owes this table its `CREATING` -> `ACTIVE` transition.
    pub active_at: SystemTime,
    /// When the clock owes this table the end of an `UpdateTable` — its `UPDATING` ->
    /// `ACTIVE` transition. `None` once no update is in flight, which is also the case
    /// for an update the service applies with the call itself (see `UpdateTable`).
    pub updated_at: Option<SystemTime>,
    /// When `PAY_PER_REQUEST` was LAST SET as this table's read/write capacity mode,
    /// which is what `BillingModeSummary.LastUpdateToPayPerRequestDateTime` reports:
    /// "Represents the time when `PAY_PER_REQUEST` was last set as the read/write
    /// capacity mode" (that member's doc). `None` for a table that has never been
    /// on-demand — which is also what makes `BillingModeSummary` itself absent
    /// ([`Table::billing_mode_summary`]).
    ///
    /// The instant stored is the one the CALL that set the mode was made at — the
    /// create that created the table on-demand, which is that table's creation instant,
    /// or the `UpdateTable` that switched it — so every description from then on reports
    /// the same instant, including one taken while the capacity move that switch asked
    /// for is still settling. It survives a switch back to `PROVISIONED`, because the
    /// member records when the mode was LAST SET rather than which mode is current.
    pub pay_per_request_at: Option<SystemTime>,
    /// When the clock owes this table its removal, set by `DeleteTable`.
    pub removed_at: Option<SystemTime>,
    /// The resource-based policy attached to this table, parsed once when it was
    /// attached, or `None` while the table carries none. This is what the data plane
    /// evaluates before it reads or writes an item, and what `GetResourcePolicy`
    /// reports.
    pub resource_policy: Option<AttachedPolicy>,
    /// The stream this table publishes to, or `None` for a table that has never had
    /// one. A table whose stream was disabled keeps it here, disabled, because
    /// `LatestStreamArn`/`LatestStreamLabel` report the table's latest stream
    /// whether or not it is currently on.
    pub stream: Option<TableStream>,
    pub items: BTreeMap<ItemKey, Item>,
    /// The metered figures `ItemCount` and `TableSizeBytes` report — refreshed on the
    /// documented schedule by [`Table::refresh_due_counters`], never on the write path.
    pub reported_counters: MeteredCounters,
    /// The summed size of `items`, kept current as items are written and removed.
    ///
    /// This is admission-control bookkeeping ONLY: what a response reports as
    /// `TableSizeBytes` is [`Table::reported_counters`], the metered snapshot
    /// [`Table::refresh_due_counters`] maintains, so a client is never told this number.
    /// Keeping it lets `PutItem` check its budget without walking the table.
    pub stored_bytes: i64,
    /// What is left of the write capacity this table is provisioned for
    /// ([`Table::admit_write`]). Like [`Table::stored_bytes`] it is admission-control
    /// bookkeeping: no response reports it.
    pub write_budget: CapacityBudget,
    /// What is left of the READ capacity this table is provisioned for
    /// ([`Table::admit_read`]). Admission-control bookkeeping like
    /// [`Table::write_budget`]: no response reports it.
    pub read_budget: CapacityBudget,
    /// What is left of the write capacity of each partition key value recently written
    /// to ([`Table::admit_item_write`]). Admission-control bookkeeping like
    /// [`Table::write_budget`]: no response reports it.
    pub key_write_budgets: KeyWriteBudgets,
    /// This table's contributor insights setting, as the contributor insights
    /// operations wrote and report it ([`ContributorInsights`]). Its indexes carry
    /// their own.
    pub contributor_insights: ContributorInsights,
    /// This table's Time to Live setting, as `UpdateTimeToLive` wrote it and
    /// `DescribeTimeToLive` reports it ([`TimeToLive`]) — and what
    /// [`Table::apply_due_expirations`] expires this table's items by.
    pub time_to_live: TimeToLive,
    /// The Kinesis data stream destinations this table replicates its item changes to,
    /// in the order they were registered ([`KinesisDestination`]).
    ///
    /// A destination whose replication has stopped stays here, so the describing
    /// operation keeps reporting the table's replication history and a later command
    /// can tell a stream the table once replicated to from one it never did.
    pub kinesis_destinations: Vec<KinesisDestination>,
    /// What this table was RESTORED from, for a table a restore created, or `None` for a
    /// table `CreateTable` created: "Contains details for the restore"
    /// (`TableDescription$RestoreSummary`), which is what every description of a restored
    /// table reports ([`TableRestore`]).
    pub restore: Option<TableRestore>,
}

/// Where a restored table came from, as `RestoreSummary` reports it: the backup it was
/// restored from, the table that backup was taken of, and when the restore was asked
/// for.
///
/// Written ONCE, by the restore that created the table, and never changed after — so
/// every later description reports the same three values. Whether the restore is still
/// in progress is NOT stored here: it is the table's own `CREATING` span, derived from
/// the clock like every other transient status ([`Table::status`]).
#[derive(Debug, Clone)]
pub struct TableRestore {
    /// "The Amazon Resource Name (ARN) of the backup from which the table was restored"
    /// (`RestoreSummary$SourceBackupArn`) — `None` for a restore that was not from a
    /// backup.
    pub source_backup_arn: Option<String>,
    /// "The ARN of the source table of the backup that is being restored"
    /// (`RestoreSummary$SourceTableArn`).
    pub source_table_arn: Option<String>,
    /// "Point in time or source backup time" (`RestoreSummary$RestoreDateTime`).
    pub restore_date_time: SystemTime,
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

    /// How long the clock still owes this table its `CREATING` -> `ACTIVE` transition at
    /// `now`, or `None` for a table whose creation has already fallen due.
    ///
    /// The creation is the transition that makes a table usable at all — "You can perform
    /// read and write operations only on an ACTIVE table" (the `CreateTable` doc) — so it
    /// is the one a client blocks on before it does anything else, with a poller whose
    /// interval is far longer than the span this emulator compresses the creation into
    /// ([`STATUS_TRANSITION`]). [`crate::ops::describe_table`] reports the table once this
    /// is no longer owed, so the first poll a client sends observes the settled table
    /// instead of waiting out its whole polling interval for a transition that was already
    /// a fraction of a second away. A table being deleted or updated is reported as it
    /// stands: it already serves (or has served) requests, so nothing is gated on it.
    pub fn creation_settles_in(&self, now: SystemTime) -> Option<Duration> {
        self.active_at
            .duration_since(now)
            .ok()
            .filter(|left| !left.is_zero())
    }

    /// The read and write capacity units this table has PROVISIONED: its own, plus that
    /// of every global secondary index on it.
    ///
    /// That sum is the unit both documented throughput quotas are stated in — the
    /// per-table one restricts "the sum of the provisioned capacity of the new table
    /// itself and all its global secondary indexes" (the `DescribeLimits` doc) — so one
    /// function answers it and every reader measures the same thing. A local secondary
    /// index shares the table's own capacity and so is already counted; a table in
    /// `PAY_PER_REQUEST` mode provisions no capacity at all ("you don't need to specify
    /// how much read and write throughput you expect" — `BillingMode`), and contributes
    /// nothing.
    pub fn provisioned_capacity(&self) -> (i64, i64) {
        ::std::iter::once(self.provisioned_throughput.as_ref())
            .chain(
                self.global_secondary_indexes
                    .iter()
                    .map(|index| index.provisioned_throughput.as_ref()),
            )
            .flatten()
            .fold((0_i64, 0_i64), |(read, write), throughput| {
                (
                    read.saturating_add(throughput.read_capacity_units),
                    write.saturating_add(throughput.write_capacity_units),
                )
            })
    }

    /// The ARN of the stream this table currently publishes to, for a table whose
    /// stream is ON.
    ///
    /// A DISABLED stream "takes no further records" — the table keeps reporting it as
    /// its latest stream, and a consumer can still drain what it captured — so a write
    /// publishes nothing to it.
    pub fn enabled_stream_arn(&self) -> Option<String> {
        self.stream
            .as_ref()
            .filter(|stream| stream.enabled)
            .map(|stream| stream.arn.clone())
    }

    /// "The primary key attribute(s) for the DynamoDB item that was modified"
    /// (`StreamRecord$Keys`): the key attributes of `item`, read through this table's
    /// own key schema, and nothing else.
    ///
    /// Read off the ITEM rather than rebuilt from the encoded [`ItemKey`], so the
    /// values a consumer sees are the ones the table stores.
    pub fn stream_keys(&self, item: &Item) -> Item {
        self.key_schema
            .iter()
            .filter_map(|element| {
                item.get_key_value(&element.attribute_name)
                    .map(|(name, value)| (name.clone(), value.clone()))
            })
            .collect()
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
        // "The AttributeValue for a key attribute cannot contain an empty string
        // value" / an empty binary value: a key attribute's value must be non-empty,
        // however the table declared its type, and the refusal names the key it was
        // supplied for. A value of ANOTHER type than the one declared is the type
        // mismatch below, whether or not it is also empty.
        if declared == attribute_value_type(value) {
            if let Some(kind) = empty_key_value_kind(value) {
                return Err(empty_key_attribute(attribute, kind));
            }
        }
        let encoded = match (declared, value) {
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
                    "{INVALID_PARAMETER_CLAUSE}: Type mismatch for key {attribute} expected: \
                     {declared} actual: {}",
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
        self.key_of_map(key, KeySource::Key)
    }

    /// The primary key a paging member (`ExclusiveStartKey`) resumes from: read
    /// through the same key schema as a `Key`, and refused as the INVALID RESUME
    /// TOKEN it is when it is not a key of that schema ([`START_KEY_INVALID_MESSAGE`]).
    pub fn key_of_start_key(&self, key: &Item) -> Result<ItemKey, RequestError> {
        self.key_of_map(key, KeySource::StartKey)
    }

    /// The one walk a `Key`-shaped map is read through, whichever member carried it:
    /// exactly the key attributes the schema declares, with their declared types.
    fn key_of_map(&self, key: &Item, source: KeySource) -> Result<ItemKey, RequestError> {
        for name in key.keys() {
            if name != self.partition_key_name() && Some(name.as_str()) != self.sort_key_name() {
                return Err(source.mismatch());
            }
        }
        self.key_from(key, source)
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
            if !matches!(source, KeySource::Item)
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
            partition: partition.ok_or_else(|| source.mismatch())?,
            sort,
        })
    }

    /// Hold every SECONDARY-INDEX key attribute the item carries to the type the
    /// table declared for it, to the non-emptiness a key value must have, and to the
    /// same key-value limits — the three rules below, in that order.
    ///
    /// Every write path that stores an item goes through here. The transactional
    /// write, whose answer depends on WHICH of the rules was broken, calls
    /// [`Self::check_index_key_types`] and [`Self::check_index_key_values`]
    /// separately instead.
    pub fn check_index_keys(&self, item: &Item) -> Result<(), RequestError> {
        self.check_index_key_types(item)?;
        self.check_index_key_values(item)
    }

    /// The TYPE rule for the item's secondary-index key attributes: "If you specify
    /// any attributes that are part of an index key, then the data types for those
    /// attributes must match those of the schema in the table's attribute definition"
    /// (the `PutItem$Item` doc), so a value whose `AttributeValue` member is not the
    /// `AttributeType` this table's `AttributeDefinitions` declare for that attribute
    /// is refused ([`index_key_type_mismatch`]) and the item is never written.
    ///
    /// This is decided against the TABLE's schema rather than against the request
    /// alone, which is why the transactional write reports it as the refusal of the
    /// element that carries the item.
    pub fn check_index_key_types(&self, item: &Item) -> Result<(), RequestError> {
        for (index_name, attribute, value) in self.index_key_values(item) {
            let actual = attribute_value_type(value);
            if let Some(declared) = self.declared_type(attribute) {
                if declared != actual {
                    return Err(index_key_type_mismatch(
                        attribute, declared, actual, index_name,
                    ));
                }
            }
        }
        Ok(())
    }

    /// The VALUE rules for the item's secondary-index key attributes:
    ///
    /// * the EMPTINESS: "Attribute values of type String and Binary must have a
    ///   length greater than zero if the attribute is used as a key attribute for a
    ///   table or index" (the `PutItem$Item` doc), so an empty `S`/`B` index key is
    ///   refused ([`empty_index_key_attribute`]);
    /// * the SIZE: the documented key-value limits are stated for a key attribute
    ///   "for a table or index", and an item that carries an index's key attributes
    ///   is projected into that index, so an oversize index key is refused on the
    ///   same write the table's own key is checked on.
    ///
    /// Both speak about the value the request carried rather than about the table's
    /// schema, so the transactional write answers them as the request-level refusals
    /// they are.
    pub fn check_index_key_values(&self, item: &Item) -> Result<(), RequestError> {
        for (index_name, attribute, value) in self.index_key_values(item) {
            if let Some(kind) = empty_key_value_kind(value) {
                return Err(empty_index_key_attribute(attribute, kind, index_name));
            }
            self.check_key_limits(attribute, value)?;
        }
        Ok(())
    }

    /// Every secondary-index key attribute `item` carries, as `(index name,
    /// attribute, value)`, over the global indexes first and then the local ones.
    ///
    /// An attribute the item does not carry is simply not projected into that index
    /// and yields nothing. An attribute that is part of the TABLE's own key schema is
    /// left out too: it is refused as the table key it is by [`Self::key_of_item`] —
    /// the attribute plays both roles whenever a local secondary index is defined,
    /// and the refusal AWS states names the role the table gave it.
    fn index_key_values<'a>(
        &'a self,
        item: &'a Item,
    ) -> impl Iterator<Item = (&'a str, &'a str, &'a AttributeValue)> + 'a {
        self.global_secondary_indexes
            .iter()
            .map(|index| (index.name.as_str(), &index.key_schema))
            .chain(
                self.local_secondary_indexes
                    .iter()
                    .map(|index| (index.name.as_str(), &index.key_schema)),
            )
            .flat_map(move |(index_name, schema)| {
                schema.iter().filter_map(move |element| {
                    let attribute = element.attribute_name.as_str();
                    if self.is_table_key(attribute) {
                        return None;
                    }
                    item.get(attribute)
                        .map(|value| (index_name, attribute, value))
                })
            })
    }

    /// Whether `attribute` is one of this table's OWN key attributes.
    ///
    /// `pub`: a `Query`'s `FilterExpression` "does not allow key attributes", so the
    /// handler judges the attributes an expression reads against this one reading of
    /// the key schema rather than a second copy of it.
    pub fn is_table_key(&self, attribute: &str) -> bool {
        self.key_schema
            .iter()
            .any(|element| element.attribute_name == attribute)
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

    /// The secondary index an `IndexName` names, whichever kind it is, or `None` when
    /// this table declares no index by that name.
    ///
    /// "This index can be any local secondary index or global secondary index"
    /// (`Scan$IndexName`), so both lists answer; an index name is unique across the two
    /// of them, because a create refuses a name either list already holds.
    pub fn secondary_index(&self, name: &str) -> Option<SecondaryIndex<'_>> {
        let global = self
            .global_secondary_indexes
            .iter()
            .find(|index| index.name == name)
            .map(|index| SecondaryIndex {
                name: index.name.as_str(),
                key_schema: &index.key_schema,
                projection: &index.projection,
                global: true,
            });
        global.or_else(|| {
            self.local_secondary_indexes
                .iter()
                .find(|index| index.name == name)
                .map(|index| SecondaryIndex {
                    name: index.name.as_str(),
                    key_schema: &index.key_schema,
                    projection: &index.projection,
                    global: false,
                })
        })
    }

    /// One index key schema's elements in the order the index orders its entries by:
    /// every `HASH` element the schema declares, then every `RANGE` one, each group in
    /// the order the schema declared it — "HASH - partition key ... RANGE - sort key"
    /// (`KeySchemaElement$KeyType`), and an index may declare several of each
    /// ([`MAX_INDEX_PARTITION_KEYS`]).
    fn index_key_elements(
        key_schema: &[KeySchemaElement],
    ) -> impl Iterator<Item = &KeySchemaElement> {
        key_schema
            .iter()
            .filter(|element| element.key_type == "HASH")
            .chain(key_schema.iter().filter(|element| element.key_type != "HASH"))
    }

    /// Where `item`, stored under the primary key `key`, sits in the order a read of
    /// `index` walks it — or `None` when the index holds no entry for it.
    ///
    /// Every index key value was held to this table's declared type for it, to
    /// non-emptiness and to the documented key-value limits when the item was written
    /// ([`Self::check_index_keys`]), so encoding one cannot fail for a stored item; one
    /// this table could not encode is not in the index, exactly as an item that leaves an
    /// index key attribute out is not ([`index_covers`]).
    pub fn index_item_key(
        &self,
        index: &SecondaryIndex<'_>,
        key: &ItemKey,
        item: &Item,
    ) -> Option<IndexItemKey> {
        let mut values = Vec::with_capacity(index.key_schema.len());
        for element in Self::index_key_elements(index.key_schema) {
            let name = element.attribute_name.as_str();
            values.push(self.key_value(name, item.get(name)?).ok()?);
        }
        Some(IndexItemKey {
            index: values,
            table: key.clone(),
        })
    }

    /// The key attributes a read of `index` reports as `LastEvaluatedKey`, and resumes
    /// from: the index's own key attributes together with this table's, which is what
    /// addresses one entry of a secondary index — the index key alone does not, since
    /// nothing makes it unique.
    pub fn index_key_attributes(&self, index: &SecondaryIndex<'_>, item: &Item) -> Item {
        Self::index_key_elements(index.key_schema)
            .chain(self.key_schema.iter())
            .filter_map(|element| {
                item.get_key_value(&element.attribute_name)
                    .map(|(name, value)| (name.clone(), value.clone()))
            })
            .collect()
    }

    /// The place in `index`'s order an `ExclusiveStartKey` resumes a read from.
    ///
    /// A resume token is read back through the two schemas the read reported it from, so
    /// a map that names an attribute neither declares, leaves one of them out, or carries
    /// one with a type other than the table declared is the INVALID RESUME TOKEN it is
    /// ([`START_KEY_INVALID_MESSAGE`]) — the same way a base-table token is
    /// ([`Self::key_of_start_key`]).
    pub fn index_key_of_start_key(
        &self,
        index: &SecondaryIndex<'_>,
        key: &Item,
    ) -> Result<IndexItemKey, RequestError> {
        let named: BTreeSet<&str> = Self::index_key_elements(index.key_schema)
            .chain(self.key_schema.iter())
            .map(|element| element.attribute_name.as_str())
            .collect();
        if key.len() != named.len() || key.keys().any(|name| !named.contains(name.as_str())) {
            return Err(KeySource::StartKey.mismatch());
        }
        let mut values = Vec::with_capacity(index.key_schema.len());
        for element in Self::index_key_elements(index.key_schema) {
            let name = element.attribute_name.as_str();
            let value = key.get(name).ok_or_else(|| KeySource::StartKey.mismatch())?;
            if self
                .declared_type(name)
                .is_some_and(|declared| declared != attribute_value_type(value))
            {
                return Err(KeySource::StartKey.mismatch());
            }
            values.push(
                self.key_value(name, value)
                    .map_err(|_| KeySource::StartKey.mismatch())?,
            );
        }
        // The table's own half of the token is read through the one walk every
        // `ExclusiveStartKey` is read through, over just the attributes it declares.
        let table = self.key_of_start_key(&self.key_attributes(key))?;
        Ok(IndexItemKey {
            index: values,
            table,
        })
    }

    /// The number of items stored right now.
    fn live_item_count(&self) -> i64 {
        i64::try_from(self.items.len()).unwrap_or(i64::MAX)
    }

    /// The stored size of every item right now.
    fn live_table_size_bytes(&self) -> i64 {
        self.items.values().map(item_size).sum()
    }

    /// The items one index projects, and the bytes they take there, right now. One rule
    /// for both index kinds, from the key schema and projection each of them carries, in
    /// a single pass over the items.
    fn live_index_figures(
        &self,
        key_schema: &[KeySchemaElement],
        projection: &Projection,
    ) -> (i64, i64) {
        self.items
            .values()
            .filter(|item| index_covers(key_schema, item))
            .fold((0i64, 0i64), |(count, bytes), item| {
                (
                    count.saturating_add(1),
                    bytes.saturating_add(index_projected_size(
                        key_schema,
                        projection,
                        item,
                        &self.key_schema,
                    )),
                )
            })
    }

    /// Materialise the metering refresh the clock has already made due for this table
    /// and every index on it.
    ///
    /// This is the ONLY thing that moves what `ItemCount` and
    /// `TableSizeBytes`/`IndexSizeBytes` report: "DynamoDB updates this value
    /// approximately every six hours. Recent changes might not be reflected in this
    /// value" (`TableDescription$ItemCount`). A refresh owed at this instant was owed
    /// whether or not anybody called, so a reporting operation may finish it; a write is
    /// never allowed to.
    pub fn refresh_due_counters(&mut self, now: SystemTime) {
        if !self.reported_counters.refresh_due(now) {
            return;
        }
        self.meter_counters(now);
    }

    /// Remove every item of this table whose TTL deletion the clock has already made
    /// due, returning the items removed so their tables' streams can carry the change.
    ///
    /// "If the epoch time value stored in the attribute is less than the current time,
    /// the item is marked as expired and subsequently deleted" (the `UpdateTimeToLive`
    /// doc): the deletion was owed at this instant whether or not anybody called, so a
    /// reporting operation may finish it. Nothing here is a filter — an item that is
    /// expired but whose deletion is not due yet is still STORED and still served,
    /// because "items that have expired and not been deleted will still show up in
    /// reads, queries, and scans" (the same doc).
    ///
    /// A table whose TTL is off, or whose last TTL change is still settling, has nothing
    /// to sweep, and a table that was swept within the last [`TTL_DELETION_DELAY`] is
    /// left alone, so the walk over this table's items runs at most once per span.
    pub fn apply_due_expirations(&mut self, now: SystemTime) -> Vec<Item> {
        let Some(attribute) = self.time_to_live.due_sweep_attribute(now) else {
            return Vec::new();
        };
        let Some(cutoff) = expiry_cutoff(now) else {
            return Vec::new();
        };
        self.time_to_live.mark_swept(now);
        let due: Vec<ItemKey> = self
            .items
            .iter()
            .filter(|(_, item)| {
                item.get(&attribute)
                    .is_some_and(|value| expiry_due(value, &cutoff))
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut removed = Vec::with_capacity(due.len());
        for key in due {
            if let Some(item) = self.items.remove(&key) {
                self.stored_bytes = self.stored_bytes.saturating_sub(item_size(&item));
                removed.push(item);
            }
        }
        removed
    }

    /// Meter this table's figures and every index's, at `now`, whatever the schedule.
    ///
    /// The walk [`Self::refresh_due_counters`] performs when the clock has made a refresh
    /// due, reached directly by the one caller whose table ALREADY HOLDS data when it is
    /// created — a table a restore wrote the backup's items into, whose figures at
    /// creation are those of that data rather than zeroes. Nothing else may call it: a
    /// write never moves what `ItemCount`/`TableSizeBytes` report.
    pub fn meter_counters(&mut self, now: SystemTime) {
        let item_count = self.live_item_count();
        let size_bytes = self.live_table_size_bytes();
        self.reported_counters.refresh(now, item_count, size_bytes);
        // Each index's figures are metered from the table's items on the same schedule,
        // computed first (they read `self`) and then recorded.
        let global: Vec<(i64, i64)> = self
            .global_secondary_indexes
            .iter()
            .map(|index| self.live_index_figures(&index.key_schema, &index.projection))
            .collect();
        let local: Vec<(i64, i64)> = self
            .local_secondary_indexes
            .iter()
            .map(|index| self.live_index_figures(&index.key_schema, &index.projection))
            .collect();
        // A vector index is metered from the same items, by its own coverage and
        // projection rules ([`VectorIndex::covers`]).
        let vector: Vec<(i64, i64)> = self
            .vector_indexes
            .iter()
            .map(|index| {
                self.items
                    .values()
                    .filter(|item| index.covers(item))
                    .fold((0i64, 0i64), |(count, bytes), item| {
                        (
                            count.saturating_add(1),
                            bytes.saturating_add(index.projected_size(item, &self.key_schema)),
                        )
                    })
            })
            .collect();
        for (index, (item_count, size_bytes)) in
            self.global_secondary_indexes.iter_mut().zip(global)
        {
            index.reported_counters.refresh(now, item_count, size_bytes);
        }
        for (index, (item_count, size_bytes)) in self.local_secondary_indexes.iter_mut().zip(local)
        {
            index.reported_counters.refresh(now, item_count, size_bytes);
        }
        for (index, (item_count, size_bytes)) in self.vector_indexes.iter_mut().zip(vector) {
            index.reported_counters.refresh(now, item_count, size_bytes);
        }
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

    /// The contributor insights setting the request addressed: this table's own, or the
    /// one of the global secondary index it named.
    ///
    /// An `IndexName` the table does not carry names no resource whose setting could be
    /// read, so it is the same miss as a table that is not there ("The operation tried
    /// to access a nonexistent table or index" — the `ResourceNotFoundException` doc).
    pub fn contributor_insights(
        &self,
        index_name: Option<&str>,
    ) -> Result<&ContributorInsights, RequestError> {
        match index_name {
            None => Ok(&self.contributor_insights),
            Some(name) => self
                .index(name)
                .map(|index| &index.contributor_insights)
                .ok_or_else(|| index_not_found(&self.name, name)),
        }
    }

    /// The same, mutably, for the operation that writes the setting.
    pub fn contributor_insights_mut(
        &mut self,
        index_name: Option<&str>,
    ) -> Result<&mut ContributorInsights, RequestError> {
        match index_name {
            None => Ok(&mut self.contributor_insights),
            Some(name) => {
                // The miss is reported over the table's stored name, so the borrow of
                // the index below cannot overlap reading it.
                let table = self.name.clone();
                self.index_mut(name)
                    .map(|index| &mut index.contributor_insights)
                    .ok_or_else(|| index_not_found(&table, name))
            }
        }
    }

    /// The destination the command addressed, mutably: this table's LIVE replication to
    /// the stream that `stream_arn` names, or `None` when the table replicates to no
    /// such stream — a stream it has never replicated to, and one it has stopped
    /// replicating to, alike ([`KinesisDestination::replicating`]).
    pub fn replicating_kinesis_destination_mut(
        &mut self,
        stream_arn: &str,
        now: SystemTime,
    ) -> Option<&mut KinesisDestination> {
        self.kinesis_destinations
            .iter_mut()
            .find(|destination| destination.stream_arn == stream_arn && destination.replicating(now))
    }

    /// The destination record this table holds for the stream that `stream_arn` names,
    /// mutably, WHATEVER status it is in — `None` only when the table has no record of
    /// that stream at all.
    ///
    /// This is the lookup a command that refuses a destination on its STATUS needs: a
    /// record it does not see cannot be told apart from one that was never registered.
    pub fn kinesis_destination_mut(&mut self, stream_arn: &str) -> Option<&mut KinesisDestination> {
        self.kinesis_destinations
            .iter_mut()
            .find(|destination| destination.stream_arn == stream_arn)
    }

    /// The destination the command addressed, mutably, only while it is `ACTIVE`
    /// ([`KinesisDestination::active`]) — the status a command that changes a
    /// destination requires of it.
    pub fn active_kinesis_destination_mut(
        &mut self,
        stream_arn: &str,
        now: SystemTime,
    ) -> Option<&mut KinesisDestination> {
        self.kinesis_destination_mut(stream_arn)
            .filter(|destination| destination.active(now))
    }

    /// Whether this table already holds as many destination records as it keeps and
    /// `stream_arn` is not one of them — the registration that would grow the list past
    /// [`MAX_KINESIS_DESTINATIONS_PER_TABLE`].
    pub fn kinesis_destination_slots_exhausted(&self, stream_arn: &str) -> bool {
        self.kinesis_destinations.len() >= MAX_KINESIS_DESTINATIONS_PER_TABLE
            && !self
                .kinesis_destinations
                .iter()
                .any(|destination| destination.stream_arn == stream_arn)
    }

    /// Register replication to `stream_arn`, as the command that starts it leaves the
    /// record: `ENABLING` for the span and `ACTIVE` after it, under the precision the
    /// command named and under none where it named none.
    ///
    /// A stream this table once replicated to is re-registered IN PLACE, so one stream
    /// has one record whatever number of times replication to it is started and stopped,
    /// and it keeps its place in the order the describing operation reports.
    pub fn register_kinesis_destination(
        &mut self,
        stream_arn: &str,
        precision: Option<&str>,
        now: SystemTime,
    ) -> &KinesisDestination {
        let registered = KinesisDestination::enabling(stream_arn, precision, now);
        let index = match self
            .kinesis_destinations
            .iter()
            .position(|destination| destination.stream_arn == stream_arn)
        {
            Some(index) => {
                self.kinesis_destinations[index] = registered;
                index
            }
            None => {
                self.kinesis_destinations.push(registered);
                self.kinesis_destinations.len() - 1
            }
        };
        &self.kinesis_destinations[index]
    }

    /// Every Kinesis data stream destination this table carries, in registration order,
    /// as the describing operation reports them — the stopped ones included, which is
    /// what makes that report the table's replication history.
    pub fn describe_kinesis_destinations(
        &self,
        now: SystemTime,
    ) -> Vec<KinesisDataStreamDestination> {
        self.kinesis_destinations
            .iter()
            .map(|destination| destination.describe(now))
            .collect()
    }

    /// Every contributor insights summary this table reports: its own first, then one
    /// per global secondary index, in the order the indexes are stored —
    /// "ContributorInsightsSummary for a table and all its global secondary indexes"
    /// (the `ListContributorInsights` doc).
    ///
    /// Only a resource whose setting the listing reports ([`ContributorInsights::listed`])
    /// is in it, so a table whose insights are off has no summary and drops out once a
    /// `DISABLE` settles.
    ///
    /// A local secondary index has none: the feature is addressed through `IndexName`
    /// against the global indexes an `UpdateContributorInsights` can name.
    pub fn contributor_insights_summaries(&self, now: SystemTime) -> Vec<ContributorInsightsSummary> {
        ::std::iter::once(ContributorInsightsSummary {
            table_name: Some(self.name.clone()),
            index_name: None,
            contributor_insights_status: Some(self.contributor_insights.status(now).to_string()),
            contributor_insights_mode: self.contributor_insights.mode(),
        })
        .filter(|_| self.contributor_insights.listed(now))
        .chain(
            self.global_secondary_indexes
                .iter()
                .filter(|index| index.contributor_insights.listed(now))
                .map(|index| ContributorInsightsSummary {
                    table_name: Some(self.name.clone()),
                    index_name: Some(index.name.clone()),
                    contributor_insights_status: Some(
                        index.contributor_insights.status(now).to_string(),
                    ),
                    contributor_insights_mode: index.contributor_insights.mode(),
                }),
        )
        .collect()
    }

    /// Project one global secondary index into the description `DescribeTable` and the
    /// table-mutating operations report. `ItemCount` and `IndexSizeBytes` are the
    /// metered figures of the last refresh, never the live projection.
    fn describe_index(
        &self,
        index: &GlobalSecondaryIndex,
        now: SystemTime,
    ) -> GlobalSecondaryIndexDescription {
        let (number_of_decreases_today, last_increase_date_time, last_decrease_date_time) =
            index.throughput_history.describe(now);
        let capacity = reported_capacity(index.provisioned_throughput.as_ref());
        // An index keeps warm the capacity it is provisioned for — or, once that capacity
        // has been withdrawn, the pair it was keeping warm — by the same rule the table
        // follows; no request can name one per index (`CreateTable`/`UpdateTable` refuse
        // that member with the scaffold's 501), so none is ever in flight and the status
        // is `ACTIVE` — "The status can only be UPDATING or ACTIVE"
        // (`GlobalSecondaryIndexWarmThroughputDescription$Status`).
        let (warm_read_units, warm_write_units) = index.warm_throughput.in_force();
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
                read_capacity_units: Some(capacity.read_capacity_units),
                write_capacity_units: Some(capacity.write_capacity_units),
                number_of_decreases_today,
                last_increase_date_time,
                last_decrease_date_time,
            }),
            item_count: Some(index.reported_counters.item_count),
            index_size_bytes: Some(index.reported_counters.size_bytes),
            warm_throughput: Some(GlobalSecondaryIndexWarmThroughputDescription {
                read_units_per_second: Some(warm_read_units),
                write_units_per_second: Some(warm_write_units),
                status: Some(IndexStatus::Active.as_str().to_string()),
            }),
            ..GlobalSecondaryIndexDescription::default()
        }
    }

    /// Project one local secondary index into the description `DescribeTable` and the
    /// table-mutating operations report. `ItemCount` and `IndexSizeBytes` are the metered
    /// figures of the last refresh, by the same rule the global indexes follow.
    fn describe_local_index(&self, index: &LocalSecondaryIndex) -> LocalSecondaryIndexDescription {
        LocalSecondaryIndexDescription {
            index_name: Some(index.name.clone()),
            index_arn: Some(index.arn.clone()),
            key_schema: Some(index.key_schema.clone()),
            projection: Some(index.projection.clone()),
            item_count: Some(index.reported_counters.item_count),
            index_size_bytes: Some(index.reported_counters.size_bytes),
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
    /// untouched gets no entry at all — neither one the item does not reach, nor one
    /// whose stored entry this write does not change.
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

    /// Admit a write that consumes `units` write capacity units, reporting whether the
    /// capacity this table is provisioned for covered it.
    ///
    /// "Provisioned throughput is the maximum amount of capacity that an application can
    /// consume from a table or index" per second (DynamoDB Developer Guide, "Read/write
    /// capacity mode"), so a PROVISIONED table admits its provisioned write units each
    /// second plus whatever of them went unused inside [`BURST_CAPACITY_WINDOW`], and a
    /// write past that is throttled rather than applied. A table on-demand
    /// (`PAY_PER_REQUEST`) is provisioned for no capacity to exceed, so every write of
    /// one is admitted.
    ///
    /// Only writes are admitted from a budget: a read in this emulator leaves nothing
    /// behind it, so what a later caller is told never depends on how much has been
    /// read.
    pub fn admit_write(&mut self, units: f64, now: SystemTime) -> bool {
        let Some(provisioned) = self.provisioned_write_rate() else {
            return true;
        };
        self.write_budget.take(provisioned, units, now)
    }

    /// Admit a read that consumes `units` read capacity units, reporting the throttle
    /// when the capacity this table is provisioned for did not cover it.
    ///
    /// The read side of [`Self::admit_write`], and for the same documented reason:
    /// "provisioned throughput is the maximum amount of capacity that an application can
    /// consume from a table or index" per second, in reads as in writes, so a
    /// PROVISIONED table serves its provisioned read units each second plus whatever of
    /// them went unused inside [`BURST_CAPACITY_WINDOW`], and a read past that is
    /// refused with `ProvisionedThroughputExceededException` rather than served. A table
    /// on-demand (`PAY_PER_REQUEST`) is provisioned for no capacity to exceed, so every
    /// read of one is served.
    ///
    /// What the read SEES is untouched by this: the items, and everything a later caller
    /// reads back, are exactly what they were. What it spends is the capacity the service
    /// itself bills a read for — the figure the same call reports as
    /// `ConsumedCapacity.ReadCapacityUnits` — which is why a read is held to it at all.
    ///
    /// A read the budget cannot cover takes nothing from it, so a throttled read is not
    /// charged for work it was refused.
    pub fn admit_read(&mut self, units: f64, now: SystemTime) -> Result<(), ReadThrottle> {
        let Some(provisioned) = self.provisioned_read_rate() else {
            return Ok(());
        };
        if self.read_budget.take(provisioned, units, now) {
            Ok(())
        } else {
            Err(ReadThrottle::TableProvisioned)
        }
    }

    /// Whether the read capacity this table is provisioned for would cover `units` at
    /// `now`, WITHOUT taking them.
    ///
    /// What lets a read that spans SEVERAL tables be admitted all-or-nothing — a
    /// transactional get is one read, and "the entire transaction is rejected" when any
    /// part of it cannot be served, so no table is debited unless every one of them has
    /// the capacity ([`Self::admit_read`] is what takes it once they all do).
    pub fn read_admits(&self, units: f64, now: SystemTime) -> Result<(), ReadThrottle> {
        let Some(provisioned) = self.provisioned_read_rate() else {
            return Ok(());
        };
        if self
            .read_budget
            .covers_within(provisioned, BURST_CAPACITY_WINDOW, units, now)
        {
            Ok(())
        } else {
            Err(ReadThrottle::TableProvisioned)
        }
    }

    /// The read units per second this table is PROVISIONED for, or `None` for a table
    /// on-demand (`PAY_PER_REQUEST`), which is provisioned for no capacity to exceed.
    fn provisioned_read_rate(&self) -> Option<i64> {
        match (
            self.billing_mode.as_str(),
            self.provisioned_throughput.as_ref(),
        ) {
            ("PROVISIONED", Some(throughput)) if throughput.read_capacity_units > 0 => {
                Some(throughput.read_capacity_units)
            }
            _ => None,
        }
    }

    /// Admit one ITEM write — a put, an update or a delete of the item whose partition
    /// key value is `partition` — against BOTH ceilings a write is held to, reporting
    /// which one refused it.
    ///
    /// Beneath the table's own provisioned capacity sits the storage layer's
    /// per-partition-key ceiling ([`PARTITION_WRITE_UNITS_PER_SECOND`]): every item that
    /// shares a partition key value lives in one partition, so a stream of writes aimed
    /// at one key is bounded by that partition whatever the table is provisioned for,
    /// while the same rate spread over many keys is not. The table ceiling is checked
    /// first, so a table that is out of capacity reports its own provisioning rather
    /// than blaming the key.
    ///
    /// Neither budget is debited unless BOTH cover the write: a throttled write does no
    /// work, so it consumes no capacity anywhere.
    pub fn admit_item_write(
        &mut self,
        partition: &AttributeValueKey,
        capacity: &WriteCapacity,
        now: SystemTime,
    ) -> Result<(), WriteThrottle> {
        let total = capacity.total();
        let provisioned = self.provisioned_write_rate();
        if let Some(provisioned) = provisioned {
            if !self
                .write_budget
                .covers_within(provisioned, BURST_CAPACITY_WINDOW, total, now)
            {
                return Err(WriteThrottle::TableProvisioned);
            }
        }
        // The partition holds the TABLE's items: an index entry this write fans out to
        // is a write to that index's own partitions, so the key's ceiling is measured
        // against the units the table itself took.
        if !self.key_write_budgets.covers(partition, capacity.table, now) {
            return Err(WriteThrottle::KeyRange);
        }
        if let Some(provisioned) = provisioned {
            self.write_budget.take(provisioned, total, now);
        }
        self.key_write_budgets
            .take(partition, capacity.table, now);
        Ok(())
    }

    /// Whether the ceilings a write is admitted from could EVER cover this write — that
    /// is, whether waiting for capacity to refill could admit it at all.
    ///
    /// A budget never holds more than its whole burst window's worth of credit
    /// ([`burst_ceiling`]), so a write that asks for more than that is not waiting for
    /// capacity: no later attempt at it is admitted either, whatever the caller does.
    /// That is what tells a request worth handing back for a retry from one a retry
    /// could never get through.
    pub fn can_ever_admit_item_write(&self, capacity: &WriteCapacity) -> bool {
        if let Some(provisioned) = self.provisioned_write_rate() {
            if capacity.total() > burst_ceiling(provisioned, BURST_CAPACITY_WINDOW) {
                return false;
            }
        }
        capacity.table <= burst_ceiling(PARTITION_WRITE_UNITS_PER_SECOND, PARTITION_BURST_WINDOW)
    }

    /// Admit one item write that must make PROGRESS: the same two ceilings as
    /// [`Self::admit_item_write`], except that a write they are merely SHORT of capacity
    /// for may be served by BORROWING against their own refill, which leaves them in
    /// deficit by the shortfall.
    ///
    /// What this serves is the batch write surface, whose residue a caller is documented
    /// to resend ("you can provide this value directly to a subsequent BatchWriteItem
    /// operation"): the capacity a table has left decides HOW MANY of a batch's requests
    /// land, not WHETHER the call got anything done, so a batch that could otherwise
    /// store nothing stores the first request it can and hands the rest back — which is
    /// what keeps that retry loop converging.
    ///
    /// The units borrowed are charged in full and repay at the rate in force, so the
    /// SUSTAINED rate either ceiling admits is exactly the rate it is provisioned for;
    /// and the borrowing is bounded ([`progress_borrowing_allowance`]), so a caller that
    /// keeps writing past what its table is provisioned for runs the allowance out and
    /// is refused from then on. Two writes are refused here as they are there: one past
    /// what the ceilings can ever hold ([`Self::can_ever_admit_item_write`]), because no
    /// later attempt at it would be admitted either, and one whose shortfall is deeper
    /// than the allowance.
    pub fn admit_item_write_with_progress(
        &mut self,
        partition: &AttributeValueKey,
        capacity: &WriteCapacity,
        now: SystemTime,
    ) -> Result<(), WriteThrottle> {
        let Err(throttle) = self.admit_item_write(partition, capacity, now) else {
            return Ok(());
        };
        if !self.can_ever_admit_item_write(capacity) {
            return Err(throttle);
        }
        // Neither budget is borrowed from unless BOTH have the room to lend: a write
        // that is refused did no work, so it owes no capacity anywhere.
        let total = capacity.total();
        let allowance = progress_borrowing_allowance();
        let provisioned = self.provisioned_write_rate();
        if let Some(provisioned) = provisioned {
            if !self.write_budget.covers_borrowing(
                provisioned,
                BURST_CAPACITY_WINDOW,
                total,
                now,
                allowance,
            ) {
                return Err(WriteThrottle::TableProvisioned);
            }
        }
        if !self
            .key_write_budgets
            .covers_borrowing(partition, capacity.table, now, allowance)
        {
            return Err(WriteThrottle::KeyRange);
        }
        if let Some(provisioned) = provisioned {
            self.write_budget
                .take_borrowing(provisioned, BURST_CAPACITY_WINDOW, total, now);
        }
        self.key_write_budgets
            .take_borrowing(partition, capacity.table, now);
        Ok(())
    }

    /// The write units per second this table is PROVISIONED for, or `None` for a table
    /// on-demand (`PAY_PER_REQUEST`), which is provisioned for no capacity to exceed.
    fn provisioned_write_rate(&self) -> Option<i64> {
        match (
            self.billing_mode.as_str(),
            self.provisioned_throughput.as_ref(),
        ) {
            ("PROVISIONED", Some(throughput)) if throughput.write_capacity_units > 0 => {
                Some(throughput.write_capacity_units)
            }
            _ => None,
        }
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
    ///
    /// Every member here comes from what a previous request wrote: the id minted at
    /// create, the metered counters of the last refresh, the billing mode and the
    /// instant `PAY_PER_REQUEST` was last set, the throughput and its history, and the
    /// warm throughput in force. `AttributeDefinitions` is reported in the order
    /// When `PAY_PER_REQUEST` was LAST SET as this table's read/write mode:
    /// "Represents the time when `PAY_PER_REQUEST` was last set as the read/write
    /// capacity mode" (`BillingModeSummary$LastUpdateToPayPerRequestDateTime`).
    ///
    /// The instant of the CALL that set the mode — the create that created the table
    /// on-demand, which is that table's own creation instant, or the `UpdateTable` that
    /// switched it — and `None` for a table that has never been on-demand. Every summary
    /// member that reports the switch reads it from here, so they all report the same
    /// instant, and a description taken while the switch is still settling reports the
    /// instant the switch was asked for rather than nothing.
    fn pay_per_request_set_at(&self) -> Option<::aws_smithy_types::DateTime> {
        self.pay_per_request_at.map(::aws_smithy_types::DateTime::from)
    }

    /// The `BillingModeSummary` a description carries, or `None` for a table whose
    /// read/write mode has never been on-demand.
    ///
    /// "You may need to switch to on-demand mode at least once in order to return a
    /// `BillingModeSummary` response" (that shape's own note): the summary records the
    /// `PAY_PER_REQUEST` switch, so a table that has only ever been `PROVISIONED` has
    /// nothing to report under it and the member is absent. The mode of EVERY table is
    /// still reported, through `TableThroughputModeSummary`, which carries no such note.
    ///
    /// A table that HAS been on-demand carries the summary from then on: the mode in
    /// force — which may be `PROVISIONED` again, for a table that has switched back —
    /// beside the instant `PAY_PER_REQUEST` was last set.
    fn billing_mode_summary(&self) -> Option<BillingModeSummary> {
        let last_set = self.pay_per_request_set_at()?;
        Some(BillingModeSummary {
            billing_mode: Some(self.billing_mode.clone()),
            last_update_to_pay_per_request_date_time: Some(last_set),
        })
    }

    /// Project this table into the `TableDescription` every table operation returns.
    ///
    /// Every member here comes from what a previous request wrote: the id minted at
    /// create, the metered counters of the last refresh, the billing mode and the
    /// instant `PAY_PER_REQUEST` was last set, the throughput and its history, and the
    /// warm throughput in force. `AttributeDefinitions` is reported in the order
    /// [`described_definitions`] gives it, which is by attribute name.
    pub fn describe(&self, now: SystemTime) -> TableDescription {
        let (number_of_decreases_today, last_increase_date_time, last_decrease_date_time) =
            self.throughput_history.describe(now);
        let capacity = reported_capacity(self.provisioned_throughput.as_ref());
        let (warm_read_units, warm_write_units) = self.warm_throughput.in_force();
        TableDescription {
            table_name: Some(self.name.clone()),
            table_arn: Some(self.arn.clone()),
            table_id: Some(self.id.clone()),
            key_schema: Some(self.key_schema.clone()),
            attribute_definitions: Some(described_definitions(&self.attribute_definitions)),
            table_status: Some(self.status(now).as_str().to_string()),
            creation_date_time: Some(::aws_smithy_types::DateTime::from(self.creation_date_time)),
            item_count: Some(self.reported_counters.item_count),
            table_size_bytes: Some(self.reported_counters.size_bytes),
            billing_mode_summary: self.billing_mode_summary(),
            // The SAME read/write mode, reported through the newer summary member every
            // recorded table description carries beside `BillingModeSummary`: one stored
            // mode and one stored switch instant feed both, so the two can never disagree
            // about which mode a table is in.
            table_throughput_mode_summary: Some(TableThroughputModeSummary {
                table_throughput_mode: Some(self.billing_mode.clone()),
                last_update_to_pay_per_request_date_time: self.pay_per_request_set_at(),
            }),
            provisioned_throughput: Some(ProvisionedThroughputDescription {
                // A PAY_PER_REQUEST table reports zeroes: on-demand mode does not use
                // provisioned throughput.
                read_capacity_units: Some(capacity.read_capacity_units),
                write_capacity_units: Some(capacity.write_capacity_units),
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
            // "The vector indexes, if any, on the table" (`TableDescription$VectorIndexes`):
            // reported only for a table that carries one, like both secondary index lists.
            vector_indexes: (!self.vector_indexes.is_empty()).then(|| {
                self.vector_indexes
                    .iter()
                    .map(|index| index.describe(now))
                    .collect()
            }),
            table_class_summary: self.table_class.as_ref().map(|class| TableClassSummary {
                table_class: Some(class.clone()),
                // "The date and time at which the table class was last updated"
                // (`TableClassSummary$LastUpdateDateTime`): the instant of the
                // `UpdateTable` that last set the class, and absent on a table whose
                // class has only ever been the one its create named — that class was
                // never updated.
                last_update_date_time: self
                    .table_class_updated_at
                    .map(::aws_smithy_types::DateTime::from),
            }),
            // Every table has a warm throughput, so every description carries one: the
            // pair in force — the one a request named, or the capacity the table keeps
            // warm by being provisioned for it — and the status of that pair rather than
            // of the table.
            warm_throughput: Some(TableWarmThroughputDescription {
                read_units_per_second: Some(warm_read_units),
                write_units_per_second: Some(warm_write_units),
                status: Some(self.warm_throughput.status(now).as_str().to_string()),
            }),
            deletion_protection_enabled: Some(self.deletion_protection_enabled),
            // "The current DynamoDB Streams configuration for the table": reported
            // only for a table whose stream is ON ([`TableStream::specification`]),
            // which is what DescribeTable does both for a table that never enabled
            // one and for one whose stream has been turned off. The table keeps
            // naming its LATEST stream either way.
            stream_specification: self.stream.as_ref().and_then(TableStream::specification),
            latest_stream_arn: self.stream.as_ref().map(|stream| stream.arn.clone()),
            latest_stream_label: self.stream.as_ref().map(|stream| stream.label.clone()),
            // "Contains details for the restore": reported only for a table a restore
            // created, which is what DescribeTable does for a table `CreateTable`
            // created. `RestoreInProgress` is the table's own transient status — the
            // restore is what is creating it — so the one stored span answers both
            // members and they cannot disagree.
            restore_summary: self.restore.as_ref().map(|restore| RestoreSummary {
                source_backup_arn: restore.source_backup_arn.clone(),
                source_table_arn: restore.source_table_arn.clone(),
                restore_date_time: ::aws_smithy_types::DateTime::from(restore.restore_date_time),
                restore_in_progress: self.status(now) == TableStatus::Creating,
            }),
            ..TableDescription::default()
        }
    }

    /// Snapshot this table as a backup taken from it records its source
    /// ([`SourceTableSnapshot`]).
    ///
    /// The figures are counted HERE, at the instant the backup is requested, rather than
    /// taken from the table's periodically metered counters: a backup catalogues the data
    /// committed up to its own time marker (the `CreateBackup` doc), so what it reports
    /// is what the table held then — and the table itself may be changed or gone by the
    /// time the backup is read.
    pub fn backup_snapshot(&self, now: SystemTime) -> SourceTableSnapshot {
        SourceTableSnapshot {
            key_schema: self.key_schema.clone(),
            attribute_definitions: self.attribute_definitions.clone(),
            table_creation_date_time: self.creation_date_time,
            billing_mode: self.billing_mode.clone(),
            provisioned_throughput: reported_capacity(self.provisioned_throughput.as_ref()),
            item_count: i64::try_from(self.items.len()).unwrap_or(i64::MAX),
            table_size_bytes: self.stored_bytes,
            global_secondary_indexes: self
                .global_secondary_indexes
                .iter()
                .map(|index| GlobalSecondaryIndexInfo {
                    index_name: Some(index.name.clone()),
                    key_schema: Some(index.key_schema.clone()),
                    projection: Some(index.projection.clone()),
                    provisioned_throughput: index.provisioned_throughput.clone(),
                    on_demand_throughput: None,
                })
                .collect(),
            local_secondary_indexes: self
                .local_secondary_indexes
                .iter()
                .map(|index| LocalSecondaryIndexInfo {
                    index_name: Some(index.name.clone()),
                    key_schema: Some(index.key_schema.clone()),
                    projection: Some(index.projection.clone()),
                })
                .collect(),
            // The vector indexes the table carried, recorded with the backup so a restore
            // from it rebuilds them and `DescribeBackup` reports them.
            vector_indexes: self
                .vector_indexes
                .iter()
                .map(|index| VectorIndexInfo {
                    index_name: Some(index.name.clone()),
                    vector_attribute: Some(index.vector_attribute.clone()),
                    search_schema: (!index.search_schema.is_empty())
                        .then(|| index.search_schema.clone()),
                    projection: Some(index.projection.clone()),
                    dimensions: Some(index.dimensions),
                    distance_function: Some(index.distance_function.clone()),
                })
                .collect(),
            // The stream settings the table had when the backup was taken, by the same
            // rule its own description reports them: a stream that was OFF then is no
            // setting to carry, so the member is absent.
            stream_specification: self.stream.as_ref().and_then(TableStream::specification),
            // The TTL setting in force at this instant, projected exactly as the TTL
            // reader projects it, so the backup records what the table reported then.
            time_to_live_description: self.time_to_live.describe(now),
        }
    }
}

/// How a listing operation pages over entries already in ascending name order:
/// truncate `entries` to one page and answer the name the page stopped on, when more
/// remain.
///
/// The three members DynamoDB's listings carry mean the same thing on each of them —
/// `ListTables`'s `ExclusiveStartTableName`/`Limit`/`LastEvaluatedTableName` and
/// `ListGlobalTables`'s `ExclusiveStartGlobalTableName`/`Limit`/
/// `LastEvaluatedGlobalTableName` — so ONE function decides all three and the readers
/// cannot drift apart: entries up to and including `exclusive_start` are dropped ("the
/// first table name that this operation will evaluate" is the one AFTER it), what is
/// left is truncated at `limit` (`default_limit` where the request named none), and the
/// name it stopped on is reported only where entries remain beyond the page, "to apply
/// in a subsequent operation to the `ExclusiveStart…Name` parameter".
///
/// `name_of` is how an entry's name is read, so a listing whose entries are more than a
/// name pages by the same rule as one whose entries are the names themselves.
pub fn page_by_name<T>(
    entries: &mut Vec<T>,
    exclusive_start: Option<&str>,
    limit: Option<i32>,
    default_limit: i32,
    name_of: impl Fn(&T) -> &str,
) -> Option<String> {
    if let Some(start) = exclusive_start {
        entries.retain(|entry| name_of(entry) > start);
    }
    let limit = usize::try_from(limit.unwrap_or(default_limit)).unwrap_or(0);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    truncated
        .then(|| entries.last().map(|entry| name_of(entry).to_string()))
        .flatten()
}

/// What one region has provisioned, counted two ways: the aggregate over every table and
/// global secondary index in it, and the largest single table's own plus its indexes'
/// capacity — the two figures [`ComAmazonawsDynamodbDynamodb20120810State::capacity_quotas_in`]
/// measures the per-account and the per-table throughput quotas against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProvisionedCapacity {
    pub aggregate_read: i64,
    pub aggregate_write: i64,
    pub largest_table_read: i64,
    pub largest_table_write: i64,
}

/// The provisioned-capacity quotas in force for one account in one region: the four
/// figures `DescribeLimits` reports, in capacity units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityQuotas {
    pub account_max_read_capacity_units: i64,
    pub account_max_write_capacity_units: i64,
    pub table_max_read_capacity_units: i64,
    pub table_max_write_capacity_units: i64,
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
    /// The transactions already applied under a `ClientRequestToken`, keyed by the
    /// REGION the call was signed for and that token — a table name resolves inside one
    /// region, so a token stands for the transaction it ran in that region and nowhere
    /// else ([`CompletedTransaction`]). Entries past
    /// [`TRANSACTION_IDEMPOTENCY_WINDOW`] are dropped by
    /// [`Self::apply_due_transitions`], because "after 10 minutes, any request with
    /// the same client token is treated as a new request".
    pub transactions: HashMap<(String, String), CompletedTransaction>,
    /// The replication group of every GLOBAL table this store holds, keyed by the
    /// global table's name: the regions that hold a replica of it.
    ///
    /// A global table is ONE resource spanning several regions, so it is not one of the
    /// per-region entries in [`Self::regions`]: each region of a group holds its own
    /// replica table under the group's name, and this map is the relation between them —
    /// what the global-table readers report ("Lists all global tables that have a replica
    /// in the specified Region" — the `ListGlobalTables` doc). A table created on its
    /// own joins no group, so it is not a global table and no reader reports it as one.
    ///
    /// A group is established by the REPLICA surface — `CreateGlobalTable`,
    /// `UpdateGlobalTable`, `UpdateTable`'s `ReplicaUpdates` and `CreateTable`'s
    /// `GlobalTableSourceArn` — none of which this slice serves yet (each answers the
    /// scaffold's 501 for the member or the whole operation), so a store driven through
    /// this slice's own API holds no group and the readers report nothing, exactly as
    /// they do for an account that has created no global table.
    pub global_table_groups: BTreeMap<String, BTreeSet<String>>,
    /// Every on-demand backup this emulator holds, keyed by its own `BackupArn`.
    ///
    /// ONE map rather than one per region, because a `BackupArn` already carries the
    /// region the backup lives in and is the whole selector `DescribeBackup` and
    /// `DeleteBackup` address it by; each record remembers its region, which is what
    /// scopes a list to the region its request reached. Ordered by that ARN, so the
    /// `ExclusiveStartBackupArn` cursor a list pages through is a comparison over the
    /// keys rather than a sort per request ([`new_backup_arn`]).
    ///
    /// Backups are NOT held on the table they were taken from: a backup outlives it
    /// (nothing in `DeleteTable` removes a table's on-demand backups), and this is the
    /// only record of one.
    pub backups: BTreeMap<String, Backup>,
    /// Every import of table data this emulator holds, keyed by its own `ImportArn`.
    ///
    /// ONE map rather than one per region, for the reason [`Self::backups`] is one: an
    /// `ImportArn` already carries the region the import lives in, and each record
    /// remembers that region, which is what scopes a listing to the region its request
    /// reached. Ordered by that ARN, so the `NextToken` cursor a listing pages through is
    /// a comparison over the keys rather than a sort per request ([`new_import_arn`]).
    ///
    /// A record is written by the IMPORT surface — `ImportTable` — which this slice does
    /// not serve yet (it answers the scaffold's 501), so a store driven through this
    /// slice's own API holds no import and the listing reports none, exactly as it does
    /// for an account that has run no import.
    pub imports: BTreeMap<String, Import>,
    /// Every export of table data this emulator holds, keyed by its own `ExportArn`.
    ///
    /// ONE map rather than one per region, for the reason [`Self::backups`] is one: an
    /// `ExportArn` already carries the region the export lives in, and each record
    /// remembers that region, which is what scopes a listing to the region its request
    /// reached. Ordered by that ARN, so the "reverse alphanumeric order of ExportArn" a
    /// listing reports, and the `NextToken` cursor it pages through, are a walk back over
    /// the keys rather than a sort per request ([`new_export_arn`]).
    ///
    /// A record is written by the EXPORT surface — `ExportTableToPointInTime` — which
    /// this slice does not serve yet (it answers the scaffold's 501), so a store driven
    /// through this slice's own API holds no export and the listing reports none, exactly
    /// as it does for an account that has run no export.
    pub exports: BTreeMap<String, Export>,
    /// The item-level changes landed writes owe their tables' streams, in the order
    /// they were made.
    ///
    /// NOT part of a table: the records are the STREAMS service's resources, and a
    /// write can only hand them over once the request that made it is done deciding
    /// (a transactional write that is cancelled part-way through rolls its queued
    /// changes back with the items themselves). Bounded by
    /// [`MAX_PENDING_STREAM_RECORDS`]; drained by [`publish_item_changes`].
    pub pending_stream_records: Vec<PendingStreamRecord>,
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Materialise the changes the clock has already made due: a table whose
    /// `DeleteTable` span has elapsed is gone, an item whose TTL expiry has fallen due
    /// is deleted, a table whose metering interval has elapsed reports freshly counted
    /// `ItemCount`/`TableSizeBytes` figures, a tag write whose propagation span has
    /// elapsed becomes visible to the tag reader, and a `ClientRequestToken` whose
    /// idempotency window has elapsed no longer stands for the transaction it named.
    ///
    /// These are the only writes a reporting operation makes — each finishes something
    /// that was owed at this instant whether or not anybody called.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        self.transactions.retain(|_, transaction| {
            !matches!(
                now.duration_since(transaction.completed_at),
                Ok(age) if age >= TRANSACTION_IDEMPOTENCY_WINDOW
            )
        });
        // An expired item's deletion is an item-level modification like any other, so a
        // stream-enabled table owes its stream one record per item swept. The records
        // are queued once the walk over the tables is done, because the queue is the
        // STORE's.
        let mut expired: Vec<PendingStreamRecord> = Vec::new();
        for tables in self.regions.values_mut() {
            tables.retain(|_, table| !table.removal_due(now));
            for table in tables.values_mut() {
                let stream_arn = table.enabled_stream_arn();
                for item in table.apply_due_expirations(now) {
                    if let Some(stream_arn) = stream_arn.clone() {
                        expired.push(PendingStreamRecord {
                            stream_arn,
                            keys: table.stream_keys(&item),
                            old_image: Some(item),
                            new_image: None,
                        });
                    }
                }
                table.refresh_due_counters(now);
                table.tags.apply_due_propagation(now);
            }
        }
        for change in expired {
            self.queue_stream_record(Some(change));
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

    /// The global tables whose replication group holds a replica in `region`, in
    /// ascending name order, each with the regions of its group — the walk
    /// `ListGlobalTables` pages over.
    ///
    /// `region` is the `RegionName` the request named ("Lists the global tables in a
    /// specific Region" — `ListGlobalTables$RegionName`); a request that names none asks
    /// for every global table this store holds, whichever regions their groups span.
    pub fn global_tables_with_replica_in(
        &self,
        region: Option<&str>,
    ) -> Vec<(String, Vec<String>)> {
        self.global_table_groups
            .iter()
            .filter(|(_, group)| region.is_none_or(|region| group.contains(region)))
            .map(|(name, group)| (name.clone(), group.iter().cloned().collect()))
            .collect()
    }

    /// Whether that region already holds a table by that name.
    pub fn contains_table(&self, region: &str, name: &str) -> bool {
        self.tables_in(region)
            .is_some_and(|tables| tables.contains_key(name))
    }

    /// What one region has already PROVISIONED, in capacity units — the two figures the
    /// documented throughput quotas are measured against.
    ///
    /// Region-scoped because a throughput quota is: it bounds what an account provisions
    /// "across all of your DynamoDB tables in a given Region"
    /// (`DescribeLimits$AccountMaxReadCapacityUnits`). Every table the region holds counts
    /// from the moment it is created, whatever status it is in, because its capacity is
    /// provisioned for it from then on; a table whose deletion the clock has already made
    /// due is gone from the store before this walk sees it
    /// ([`Self::apply_due_transitions`]).
    pub fn provisioned_capacity_in(&self, region: &str) -> ProvisionedCapacity {
        self.tables_in(region)
            .map(|tables| {
                tables
                    .values()
                    .fold(ProvisionedCapacity::default(), |totals, table| {
                        let (read, write) = table.provisioned_capacity();
                        ProvisionedCapacity {
                            aggregate_read: totals.aggregate_read.saturating_add(read),
                            aggregate_write: totals.aggregate_write.saturating_add(write),
                            largest_table_read: totals.largest_table_read.max(read),
                            largest_table_write: totals.largest_table_write.max(write),
                        }
                    })
            })
            .unwrap_or_default()
    }

    /// The four provisioned-capacity quotas this account holds in one region, as
    /// `DescribeLimits` reports them.
    ///
    /// Each is the documented default ([`DEFAULT_ACCOUNT_MAX_READ_CAPACITY_UNITS`] and
    /// its three siblings) RAISED to what the region has already provisioned: capacity an
    /// account holds was accepted under a quota at least that large, so a quota below it
    /// could never have been the one in force. The per-account pair is therefore at least
    /// the region's aggregate and the per-table pair at least its largest single table's
    /// own plus its indexes' capacity, which keeps the report consistent with the
    /// descriptions of the very tables the `DescribeLimits` doc has a client sum up.
    ///
    /// The per-table quota never exceeds the per-account one, because one table's
    /// capacity is part of the account's aggregate.
    pub fn capacity_quotas_in(&self, region: &str) -> CapacityQuotas {
        let provisioned = self.provisioned_capacity_in(region);
        let account_max_read =
            DEFAULT_ACCOUNT_MAX_READ_CAPACITY_UNITS.max(provisioned.aggregate_read);
        let account_max_write =
            DEFAULT_ACCOUNT_MAX_WRITE_CAPACITY_UNITS.max(provisioned.aggregate_write);
        CapacityQuotas {
            account_max_read_capacity_units: account_max_read,
            account_max_write_capacity_units: account_max_write,
            table_max_read_capacity_units: DEFAULT_TABLE_MAX_READ_CAPACITY_UNITS
                .max(provisioned.largest_table_read)
                .min(account_max_read),
            table_max_write_capacity_units: DEFAULT_TABLE_MAX_WRITE_CAPACITY_UNITS
                .max(provisioned.largest_table_write)
                .min(account_max_write),
        }
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

    /// Set the provisioned read or write capacity of a table — or of one of its global
    /// secondary indexes — to `units`, answering whether there was a provisioned resource
    /// there to move.
    ///
    /// The bookkeeping a capacity move owes, in ONE place, the same `UpdateTable` does: the
    /// move is recorded in that resource's throughput history (which is what
    /// `ProvisionedThroughputDescription` reports as the last increase or decrease, and what
    /// counts the day's decreases), and the table leaves `ACTIVE` for the span a capacity
    /// update takes ([`STATUS_TRANSITION`]).
    ///
    /// This is what a service that SCALES a table's throughput moves: Application Auto
    /// Scaling holds no capacity of its own — it "adjusts the capacity of the scalable
    /// target", which for `dynamodb:table:ReadCapacityUnits` and its three siblings is this
    /// table's own provisioned throughput — so it calls this instead of keeping a second
    /// copy a client would never see here.
    ///
    /// Nothing moves for a table that is not there or is being deleted, for an index the
    /// table does not have, for a resource that provisions no capacity at all (a
    /// `PAY_PER_REQUEST` table), for a capacity below the one unit a provisioned resource
    /// can be given, and for one that is already the capacity the resource holds.
    pub fn scale_provisioned_capacity(
        &mut self,
        region: &str,
        table_name: &str,
        index_name: Option<&str>,
        reads: bool,
        units: i64,
        now: SystemTime,
    ) -> bool {
        if units < 1 {
            return false;
        }
        let Ok(table) = self.table_mut(region, table_name) else {
            return false;
        };
        if table.removed_at.is_some() {
            return false;
        }
        let moved = {
            let (throughput, history) = match index_name {
                None => (
                    table.provisioned_throughput.as_mut(),
                    &mut table.throughput_history,
                ),
                Some(index_name) => {
                    match table
                        .global_secondary_indexes
                        .iter_mut()
                        .find(|index| index.name == index_name)
                    {
                        Some(index) => (
                            index.provisioned_throughput.as_mut(),
                            &mut index.throughput_history,
                        ),
                        None => return false,
                    }
                }
            };
            let Some(throughput) = throughput else {
                return false;
            };
            let before = throughput.clone();
            if reads {
                throughput.read_capacity_units = units;
            } else {
                throughput.write_capacity_units = units;
            }
            let after = throughput.clone();
            if after.read_capacity_units == before.read_capacity_units
                && after.write_capacity_units == before.write_capacity_units
            {
                false
            } else {
                history.record(now, &before, &after);
                true
            }
        };
        if moved {
            table.updated_at = Some(now + STATUS_TRANSITION);
        }
        moved
    }

    /// Remember what a `ClientRequestToken` stands for, so a retry of that transaction
    /// is answered rather than re-applied.
    ///
    /// The map is BUDGETED, like every other growth site in this store: a record past
    /// [`MAX_RECORDED_TRANSACTIONS`] evicts the OLDEST one, so what the store retains is
    /// bounded whatever number of distinct tokens a caller presents inside the window.
    /// Evicting the oldest is what a client can least observe — the records nearest
    /// their own expiry are the ones least likely to be retried — and a token whose
    /// record is gone is treated as a new request, exactly as one past the window is.
    pub fn record_transaction(
        &mut self,
        region: &str,
        token: &str,
        transaction: CompletedTransaction,
    ) {
        self.transactions
            .insert((region.to_string(), token.to_string()), transaction);
        while self.transactions.len() > MAX_RECORDED_TRANSACTIONS {
            let Some(oldest) = self
                .transactions
                .iter()
                .min_by_key(|(_, recorded)| recorded.completed_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.transactions.remove(&oldest);
        }
    }

    /// What a `ClientRequestToken` already stands for in `region`, while it is still
    /// valid.
    pub fn recorded_transaction(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&CompletedTransaction> {
        self.transactions
            .get(&(region.to_string(), token.to_string()))
    }

    /// The table an ARN-ADDRESSED operation names.
    ///
    /// The same lookup as [`Self::table`] with a different REFUSAL: the caller
    /// addressed this resource by its `ResourceArn`, so a miss is reported against
    /// that member and echoes the value sent ([`resource_arn_not_found`]) rather than
    /// a table name derived from it, which no member of the request carries.
    pub fn table_by_arn(&self, resource: &TaggedResource) -> Result<&Table, RequestError> {
        self.tables_in(&resource.region)
            .and_then(|tables| tables.get(&resource.table))
            .ok_or_else(|| resource_arn_not_found(&resource.arn))
    }

    /// The same, mutably, for the ARN-addressed operations that change a table.
    pub fn table_by_arn_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Result<&mut Table, RequestError> {
        self.regions
            .get_mut(&resource.region)
            .and_then(|tables| tables.get_mut(&resource.table))
            .ok_or_else(|| resource_arn_not_found(&resource.arn))
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

    /// Catalogue a new backup under the ARN minted for it.
    pub fn insert_backup(&mut self, backup: Backup) {
        self.backups.insert(backup.arn.clone(), backup);
    }

    /// The backup that `BackupArn` names, mutably, for the operation that takes it for a
    /// restore. The same lookup and the same miss as [`Self::backup`].
    pub fn backup_mut(&mut self, region: &str, backup_arn: &str) -> Result<&mut Backup, RequestError> {
        self.backups
            .get_mut(backup_arn)
            .filter(|backup| backup.region == region)
            .ok_or_else(|| backup_not_found(backup_arn))
    }

    /// The bytes of item data the whole catalogue holds — the figure
    /// [`MAX_BACKUP_CATALOGUE_BYTES`] is measured against. O(backups), because each
    /// record carries the size of the data it holds.
    pub fn backup_bytes(&self) -> i64 {
        self.backups
            .values()
            .map(|backup| backup.size_bytes)
            .fold(0, i64::saturating_add)
    }

    /// How many restores are still in flight in `region` at `now` — the figure
    /// [`MAX_CONCURRENT_RESTORES`] is measured against. A restore is in flight while the
    /// table it is creating is still `CREATING`.
    pub fn restores_in_progress(&self, region: &str, now: SystemTime) -> usize {
        self.tables_in(region).map_or(0, |tables| {
            tables
                .values()
                .filter(|table| {
                    table.restore.is_some() && table.status(now) == TableStatus::Creating
                })
                .count()
        })
    }

    /// How many backups that region already holds — what [`MAX_BACKUPS_PER_REGION`] is
    /// measured against.
    pub fn backup_count_in(&self, region: &str) -> usize {
        self.backups_in(region).count()
    }

    /// Every backup of one region, in ascending `BackupArn` order, which is the scope a
    /// list reports and the order it pages through.
    pub fn backups_in<'s>(&'s self, region: &'s str) -> impl Iterator<Item = &'s Backup> + 's {
        self.backups
            .values()
            .filter(move |backup| backup.region == region)
    }

    /// Every import of one region, in ascending `ImportArn` order, which is the scope a
    /// listing reports and the order it pages through.
    pub fn imports_in<'s>(&'s self, region: &'s str) -> impl Iterator<Item = &'s Import> + 's {
        self.imports
            .values()
            .filter(move |import| import.region == region)
    }

    /// Record one import under the ARN minted for it.
    pub fn insert_import(&mut self, import: Import) {
        self.imports.insert(import.arn.clone(), import);
    }

    /// Every export of one region, in DESCENDING `ExportArn` order — "in reverse
    /// alphanumeric order of ExportArn" (the `ListExports` doc), which is the scope a
    /// listing reports and the order it pages through.
    pub fn exports_in<'s>(&'s self, region: &'s str) -> impl Iterator<Item = &'s Export> + 's {
        self.exports
            .values()
            .rev()
            .filter(move |export| export.region == region)
    }

    /// Record one export under the ARN minted for it.
    pub fn insert_export(&mut self, export: Export) {
        self.exports.insert(export.arn.clone(), export);
    }

    /// The backup that `BackupArn` names, inside the region the request reached.
    ///
    /// A backup of ANOTHER region is not addressable from this endpoint, so it is the
    /// same miss as one that was never taken ([`backup_not_found`]).
    pub fn backup(&self, region: &str, backup_arn: &str) -> Result<&Backup, RequestError> {
        self.backups
            .get(backup_arn)
            .filter(|backup| backup.region == region)
            .ok_or_else(|| backup_not_found(backup_arn))
    }

    /// Remove that backup from the catalogue, returning the record removed.
    pub fn remove_backup(&mut self, region: &str, backup_arn: &str) -> Result<Backup, RequestError> {
        self.backup(region, backup_arn)?;
        self.backups
            .remove(backup_arn)
            .ok_or_else(|| backup_not_found(backup_arn))
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
    /// This is the store's only growth site, so it is where this emulator's own
    /// HOST-PROTECTION ceilings are enforced ([`MAX_ITEMS_PER_TABLE`],
    /// [`MAX_TABLE_SIZE_BYTES`], [`MAX_STORE_SIZE_BYTES`]): the clamp precedes the insert,
    /// so no write path can grow the process past them. They are not AWS quotas — the
    /// service bounds ONE item ("the maximum item size in DynamoDB is 400 KB",
    /// [`MAX_ITEM_SIZE_BYTES`], checked where the item is built) and not what a table
    /// holds — and they sit far above the data any workload stores, so a legal item is
    /// written however much the table already carries. Replacing an existing item frees
    /// that item's bytes, so a steady overwrite loop is never refused.
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
        // Whether this write INSERTS is whether the key is held, not whether the item it
        // holds sizes to zero — an item can legitimately size to nothing.
        let replaced = table.items.get(&key);
        let replaced_size = replaced.map_or(0, item_size);
        let table_bytes = table
            .stored_bytes
            .saturating_sub(replaced_size)
            .saturating_add(size);
        if (replaced.is_none() && table.items.len() >= MAX_ITEMS_PER_TABLE)
            || table_bytes > MAX_TABLE_SIZE_BYTES
            || stored.saturating_sub(replaced_size).saturating_add(size) > MAX_STORE_SIZE_BYTES
        {
            return Err(store_exhausted(name));
        }
        table.stored_bytes = table_bytes;
        // "DynamoDB Streams captures a time-ordered sequence of item-level
        // modifications in any DynamoDB table": a write that LANDS on a stream-enabled
        // table owes its stream one record, whether it stores a new item or replaces
        // one. The images the record carries are the only reason to keep a copy of the
        // item, so a table with no enabled stream clones nothing.
        let publishes_to = table.enabled_stream_arn();
        let keys = publishes_to
            .as_ref()
            .map(|_| table.stream_keys(&item))
            .unwrap_or_default();
        let new_image = publishes_to.as_ref().map(|_| item.clone());
        let replaced = table.items.insert(key, item);
        let change = publishes_to.map(|stream_arn| PendingStreamRecord {
            stream_arn,
            keys,
            old_image: replaced.clone(),
            new_image,
        });
        self.queue_stream_record(change);
        Ok(replaced)
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
        // A delete that removed NOTHING changed no item, so the stream carries no
        // record for it: the stream is a sequence of modifications, and a delete of
        // an absent key is none.
        let change = removed.as_ref().zip(table.enabled_stream_arn()).map(
            |(before, stream_arn)| PendingStreamRecord {
                stream_arn,
                keys: table.stream_keys(before),
                old_image: Some(before.clone()),
                new_image: None,
            },
        );
        self.queue_stream_record(change);
        Ok(removed)
    }

    /// How many changes are queued for the streams service — the mark a
    /// transactional write takes before it starts applying, so
    /// [`Self::rewind_stream_records`] can put the queue back where the items go back.
    pub fn queued_stream_record_count(&self) -> usize {
        self.pending_stream_records.len()
    }

    /// Drop every change queued since `mark`, for a transactional write that is
    /// rolling its items back: a cancelled transaction made no modification, so its
    /// tables' streams must carry no record of one.
    pub fn rewind_stream_records(&mut self, mark: usize) {
        self.pending_stream_records.truncate(mark);
    }

    /// Take every queued change, leaving the queue empty — what
    /// [`publish_item_changes`] hands to the streams service.
    pub fn take_stream_records(&mut self) -> Vec<PendingStreamRecord> {
        ::std::mem::take(&mut self.pending_stream_records)
    }

    /// Queue one change for the streams service, within [`MAX_PENDING_STREAM_RECORDS`].
    ///
    /// Past the cap the change is DROPPED rather than making room, so the entries
    /// already queued keep the positions a transactional write's mark names. Reaching
    /// the cap at all takes a write path that queued changes without handing them over,
    /// which is a defect in that path rather than something a caller can ask for, so
    /// the drop is logged.
    fn queue_stream_record(&mut self, change: Option<PendingStreamRecord>) {
        let Some(change) = change else {
            return;
        };
        if self.pending_stream_records.len() >= MAX_PENDING_STREAM_RECORDS {
            ::tracing::error!(
                queued = MAX_PENDING_STREAM_RECORDS,
                "an item change was dropped before it reached the DynamoDB Streams store"
            );
            return;
        }
        self.pending_stream_records.push(change);
    }
}

/// The `ResourceNotFoundException` message the CONTROL plane answers with: the
/// table-level operations (`DescribeTable`, `UpdateTable`, the tagging surface)
/// name the resource they could not find, because the caller addressed that
/// table by name and the describing surface reports it back.
fn not_found(name: &str) -> RequestError {
    RequestError::NotFound(format!("Requested resource not found: Table: {name} not found"))
}

/// The `ResourceNotFoundException` message for an INDEX the addressed table does not
/// carry.
///
/// "The operation tried to access a nonexistent table or index" (the
/// `ResourceNotFoundException` doc): the caller addressed this one by a table and an
/// index name, so the refusal names both, exactly as the name-addressed [`not_found`]
/// above names the table it could not find.
pub fn index_not_found(table: &str, index: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: Index not found for table {table}: {index}"
    ))
}

/// The `ResourceNotFoundException` message a VECTOR index the addressed table does not
/// carry is refused with.
///
/// Same rule as [`index_not_found`] — "the operation tried to access a nonexistent table
/// or index" (the `ResourceNotFoundException` doc), named by both the index and the table
/// it was looked for in — and a different SENTENCE, because the surface that changes a
/// table's vector indexes words it with the index name first and the table after it. One
/// sentence per surface rather than one shared one: a message is part of each surface's
/// contract, so the surfaces that answer the other wording keep it.
pub fn vector_index_not_found(table: &str, index: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: Index {index} for table {table}"
    ))
}

/// The `ResourceNotFoundException` message an ARN-ADDRESSED operation answers with.
///
/// The tagging surface and the resource-policy surface carry no `TableName`: the
/// caller's whole selector is the `ResourceArn` member, so the refusal names THAT
/// member and echoes the value the request carried — the same construction the
/// name-addressed [`not_found`] above makes for `Table`, over the identifier that was
/// actually sent. Nothing derived from the ARN (the table name inside it) appears,
/// because no member of the request holds it.
pub fn resource_arn_not_found(resource_arn: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: ResourceArn: {resource_arn} not found"
    ))
}

/// The `ValidationException` message an ARN-ADDRESSED operation answers a
/// `ResourceArn` it cannot address with: the fixed prefix DynamoDB's own ARN
/// validator states, followed by the value the request carried.
///
/// This is an INPUT refusal, not a lookup miss: a value that is not an ARN this
/// service can address names no resource whose existence could be checked.
pub fn invalid_resource_arn(resource_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "Invalid TableArn: Invalid ResourceArn provided as input {resource_arn}"
    ))
}

/// The `ValidationException` message for a request whose tag list repeats a key.
///
/// Built from the request: the fixed prefix, then the key found twice. A tag key
/// identifies at most one tag on a resource ("Each DynamoDB table can only have up to
/// one tag with the same key" — the `Tag$Key` doc), so a list that names one key twice
/// asks for two tags that cannot both exist and is refused rather than silently
/// resolved to one of its values.
pub fn duplicate_tag_key(key: &str) -> RequestError {
    RequestError::Validation(format!(
        "Duplicate Tag Keys provided as input: Duplicate Tag Key found {key}"
    ))
}

/// The `ResourceNotFoundException` message the DATA plane answers with.
///
/// An item operation (`GetItem`, `PutItem`, `DeleteItem`, `Query`, …) against a
/// table that is absent — or present but not yet serving items — is refused with
/// AWS's FIXED text, with no resource detail appended: the one message is all a
/// client sees for "the table you addressed is not there", so nothing about this
/// store's contents (which name was looked up, whether it exists in another
/// status) leaks through the data-plane refusal.
///
/// A data-plane read of an INDEX that the table declares but that is not serving yet
/// answers the same sentence, for the same reason: the operation's own
/// `ResourceNotFoundException` covers both ("The operation tried to access a nonexistent
/// table or index … or its status might not be ACTIVE").
pub fn item_resource_not_found() -> RequestError {
    RequestError::NotFound("Requested resource not found".to_string())
}

/// The `ValidationException` message for a request that names an index the addressed
/// table does not declare as the SOURCE IT READS.
///
/// An index named in a statement's source — or in an `IndexName` member — is part of the
/// request's own shape rather than a resource the caller addressed: the table was found,
/// and what is wrong is that it has no such index, which AWS states with this fixed
/// sentence. A statement's source is refused with the sentence alone; a member that
/// carries the name is answered the name as well ([`unknown_index_named`]). (A caller that
/// addresses an index AS THE RESOURCE, by table and index name, is answered by
/// [`index_not_found`] instead: there the index is what it asked for.)
pub const UNKNOWN_INDEX_MESSAGE: &str = "The table does not have the specified index";

/// That message as the refusal it is.
pub fn unknown_index() -> RequestError {
    RequestError::Validation(UNKNOWN_INDEX_MESSAGE.to_string())
}

/// The same refusal for a request that names the index in an `IndexName` MEMBER, which
/// AWS answers with that sentence and the name the request sent appended after a colon.
///
/// The name echoed is the caller's own input, so the refusal still reports nothing of
/// what the table does carry — a caller that misspelled a member learns which value was
/// not resolved, which is why AWS echoes it here and not for a statement's source.
pub fn unknown_index_named(index_name: &str) -> RequestError {
    RequestError::Validation(format!("{UNKNOWN_INDEX_MESSAGE}: {index_name}"))
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

/// The region a request that names no region at all is served: the region a generated
/// emulator defaults to, which is also the region `::engine::emulator_arn` mints an ARN
/// for when nothing says otherwise.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most bytes a region name this module reads out of a request may carry — in a
/// credential scope or in a host label. A region code is far shorter ("us-gov-east-1" is
/// 13); this keeps the scan over a caller-supplied `Host` bounded by a constant, and
/// bounds the length of a region that becomes a store key and an ARN segment.
const MAX_REGION_LABEL_BYTES: usize = 64;

/// The region this request addresses: the region its tables are stored in and read
/// from, and the region the ARNs it mints carry.
///
/// A request names its endpoint, and a REGION and a service name are two fields of that
/// one endpoint, so the region is resolved in the SAME tiered order the framework
/// resolves the service from (`::emulator_server::resolve_route_key`, which is what
/// routed this request here): the SigV4 credential scope in the `Authorization` header,
/// then the credential scope a PRESIGNED request carries in its `X-Amz-Credential`
/// query parameter, then the region label of the `Host`/`:authority` endpoint it was
/// sent to. Whichever of those a request carries, it is addressing one regional
/// endpoint, and the store it reaches has to be that region's.
///
/// Reading only the first tier is what made a table invisible to a later reader: a
/// request that named its region in the presigned scope or in the endpoint host was
/// served [`DEFAULT_REGION`]'s store instead of the one it addressed, so a `CreateTable`
/// that arrived through one tier and a `DescribeTable` that arrived through another
/// disagreed about which region's tables they were talking about.
///
/// A request that names no region anywhere, or whose credential scope names something
/// that is not a region NAME ([`is_region_shaped`]), is served [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    signed_region(&parts.headers)
        .or_else(|| presigned_region(&parts.uri))
        .or_else(|| endpoint_region(parts))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region of the SigV4 credential scope in the `Authorization` header — the signal
/// every signed AWS SDK request carries, and the first tier
/// [`::emulator_server::resolve_route_key`] reads.
fn signed_region(headers: &::http::HeaderMap) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
}

/// The region of the SigV4 credential scope a PRESIGNED request carries in its
/// `X-Amz-Credential` query parameter, where no `Authorization` header holds one.
///
/// The value is the same `<access key id>/<date>/<region>/<service>/aws4_request` scope,
/// percent-encoded (the `/` separators arrive as `%2F`), read here exactly as
/// `::emulator_server::service_from_sigv4_query` reads the SERVICE name out of it — the
/// 3rd field rather than the 4th.
fn presigned_region(uri: &::http::Uri) -> Option<String> {
    let credential = uri.query()?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == "X-Amz-Credential").then(|| value.to_string())
    })?;
    let decoded = credential.replace("%2F", "/").replace("%2f", "/");
    let mut fields = decoded.split('/');
    let (Some(_access_key_id), Some(_date), Some(region), Some(_service), Some("aws4_request")) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    is_region_shaped(region).then(|| region.to_string())
}

/// Whether a region a CREDENTIAL SCOPE states may be used as it stands.
///
/// That field IS the region whatever region exists by that name, so it is not held to
/// the three-group shape a host label is ([`is_region_code`]) — a credential scope may
/// legitimately name a region this emulator has never heard of. It IS held to a region
/// name's CHARACTER SET and a bounded length, because the value becomes a store key and
/// a segment of every ARN this service mints: without that, a caller's own credential
/// scope could put an ARN delimiter (`:` or `/`) into an ARN, or an unbounded string into
/// a map key. A value outside it names no region, so the caller falls back to
/// [`DEFAULT_REGION`].
fn is_region_shaped(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_LABEL_BYTES
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region of the endpoint host the request was sent to:
/// `dynamodb.<region>.amazonaws.com`, `dynamodb-fips.<region>.amazonaws.com` and
/// `dynamodb.<region>.api.aws` all name their region in a LABEL of the host, which is
/// the last tier [`::emulator_server::resolve_route_key`] reads a request's endpoint
/// from (there as the FIRST label, which names the service).
///
/// The service label is therefore skipped and the first following label that is shaped
/// like a region code ([`is_region_code`]) is the region. A host that names none — a
/// loopback address, or the legacy global `dynamodb.amazonaws.com` — yields `None`, and
/// the caller falls back to [`DEFAULT_REGION`], which is the region that global endpoint
/// resolves to anyway.
fn endpoint_region(parts: &::http::request::Parts) -> Option<String> {
    // The `Host` label first and the URI authority second, which is the order the
    // framework reads a host-style endpoint in (`service_from_host` then
    // `service_from_authority`), so a region-less `Host` beside a regional `:authority`
    // still names its region.
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        parts.uri.host(),
    ]
    .into_iter()
    .flatten()
    .find_map(region_of_host)
}

/// The region label of one endpoint host, with any `user@` and `:port` removed: the
/// first label after the service's own that is shaped like a region code.
fn region_of_host(host: &str) -> Option<String> {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    let hostname = without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo);
    hostname
        .split('.')
        .skip(1)
        .find(|label| is_region_code(label))
        .map(ToString::to_string)
}

/// Whether `label` is shaped like an AWS region code: lowercase hyphen-joined words
/// ending in a number, at least three groups long — `us-east-1`, `ap-southeast-4`,
/// `us-gov-west-1`.
///
/// This is the test that tells a region label apart from the other labels of an endpoint
/// host (`amazonaws`, `com`, `api`, `aws`); the region a credential scope states is held
/// to the looser [`is_region_shaped`] instead, because that field IS the region and may
/// name a region this emulator has never heard of.
fn is_region_code(label: &str) -> bool {
    if label.len() > MAX_REGION_LABEL_BYTES {
        return false;
    }
    // The last hyphen group is the region's number and every group before it is a word,
    // walked without collecting the caller-supplied label into a Vec.
    let Some((words, number)) = label.rsplit_once('-') else {
        return false;
    };
    let word_count = words.split('-').count();
    word_count >= 2
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && words
            .split('-')
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// `arn:aws:dynamodb:<region>:<account>:table/<name>`, built through the core's ARN
/// helper so the account segment is the canonical emulator account.
pub fn table_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("dynamodb", region, &format!("table/{name}"))
}

/// The modelled `ProvisionedThroughputExceededException` one refused write earns.
///
/// It carries the sentence for the ceiling that bound and, in `ThrottlingReasons`, that
/// ceiling's own reason code with "the Amazon Resource Name (ARN) of the DynamoDB table
/// or index that experienced the throttling event" (`ThrottlingReason$resource`) — which
/// is how a caller tells a table that needs more capacity from a key design that needs
/// fixing.
pub fn throughput_exceeded(
    table_arn: &str,
    throttle: WriteThrottle,
) -> ProvisionedThroughputExceededException {
    exceeded(table_arn, throttle.reason(), throttle.message())
}

/// The modelled `ProvisionedThroughputExceededException` one refused READ earns, built
/// the same way from the read ceiling that bound it ([`ReadThrottle`]).
pub fn read_throughput_exceeded(
    table_arn: &str,
    throttle: ReadThrottle,
) -> ProvisionedThroughputExceededException {
    exceeded(table_arn, throttle.reason(), throttle.message())
}

/// The one `ProvisionedThroughputExceededException` builder both kinds of throttle are
/// reported through, so a refused read and a refused write are worded alike.
fn exceeded(
    table_arn: &str,
    reason: &str,
    message: &str,
) -> ProvisionedThroughputExceededException {
    ProvisionedThroughputExceededException {
        message: Some(message.to_string()),
        throttling_reasons: Some(vec![ThrottlingReason {
            reason: Some(reason.to_string()),
            resource: Some(table_arn.to_string()),
        }]),
    }
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

// =============================================================================
// The DynamoDB Streams seam lives in `state/streams_seam.rs`: enabling a stream
// on a table registers it in the STREAMS service's own registry, and this store
// keeps only the table's latest stream identity.
// =============================================================================
pub mod streams_seam;

pub use streams_seam::{
    disable_table_stream, publish_item_changes, register_table_stream, stream_view_type,
    StreamFault, STREAM_VIEW_TYPE_REQUIRED, STREAM_VIEW_TYPE_REQUIRED_ON_ENABLE,
};

/// The requirement phrase for a value that is spelled as an ARN and is not a valid one.
///
/// A member that takes either a name or an ARN (`TableArn`, which the data-plane
/// `TableName` members target) states the ARN FORM rather than the name's pattern when
/// the value it refused was an attempt at an ARN: the pattern a bare table name must
/// satisfy says nothing about which ARN field was wrong.
pub const ARN_FORM_REQUIREMENT: &str = "Valid ARN format is \
     'arn:<awsPartition>:<vendor>:<region>:<awsAccountId>:<resourceType>/<resourceId>'";

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
    // A value spelled as an ARN is refused in one of TWO ways, and which one depends on
    // whether it is an ARN at all:
    //
    //   * a string that is NOT a well-formed ARN — a required field missing or empty —
    //     is refused by the ARN FORM, because that is what is wrong with it;
    //   * a well-formed ARN that simply does not name a table of this service is no way
    //     of naming a table, so it is refused as the table NAME it would have to be,
    //     by the pattern a table name must satisfy.
    let malformed_arn = || {
        invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(table_name),
            ARN_FORM_REQUIREMENT,
        )
    };
    let not_a_table_arn = || {
        invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(table_name),
            &pattern_requirement(TABLE_NAME_PATTERN),
        )
    };
    let name = match table_name.strip_prefix("arn:") {
        Some(rest) => {
            let fields: Vec<&str> = rest.splitn(5, ':').collect();
            let [partition, service, region, account, resource] = fields.as_slice() else {
                return Err(malformed_arn());
            };
            if partition.is_empty()
                || service.is_empty()
                || region.is_empty()
                || account.is_empty()
                || resource.is_empty()
            {
                return Err(malformed_arn());
            }
            if *service != "dynamodb" {
                return Err(not_a_table_arn());
            }
            let path = resource.strip_prefix("table/").ok_or_else(not_a_table_arn)?;
            // A sub-resource ARN (`.../index/<i>`, `.../stream/<label>`) names
            // something other than the table, so it is not a way of naming it.
            if path.contains('/') {
                return Err(not_a_table_arn());
            }
            path
        }
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

/// The DynamoDB resource an ARN-addressed operation's `ResourceArn` names: the region
/// it lives in, the table it is, and the ARN as the request spelled it.
///
/// Such a call carries no `TableName`, so the ARN is the caller's whole selector — and
/// it is also the only identifier the request holds, which is why it is kept here: a
/// refusal about this resource echoes `arn`, never the `table` derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    pub region: String,
    pub table: String,
    pub arn: String,
}

/// Resolve an ARN-addressed operation's `ResourceArn` to the table it names, in the
/// scope the request itself is made in.
///
/// The one place an ARN is classified for the tagging and resource-policy surfaces, so
/// `TagResource`, `UntagResource`, `ListTagsOfResource` and the three `*ResourcePolicy`
/// operations share a single rule, in two parts:
///
/// * SHAPE — the value must be an ARN of THIS service
///   (`arn:<partition>:dynamodb:<region>:<account>:table/<name>`) naming a table and
///   nothing deeper: a sub-resource ARN (`.../index/<i>`, `.../stream/<label>`) is not
///   a resource this emulator keeps a tag set or a policy for, so it is refused rather
///   than silently taken for its table;
/// * SCOPE — the ARN must address a resource of the endpoint the request reached: the
///   same partition, account and REGION this request is signed for, which is the one
///   this service answers for. [`table_arn`] mints exactly that form for a table of
///   this region, so the comparison against it is the whole scope check.
///
/// Either way the value names no resource whose existence could be looked up, so it is
/// an INPUT failure ([`invalid_resource_arn`], 400 `ValidationException`) and not a
/// missing resource — an out-of-scope ARN is not "absent here", it is unaddressable
/// from here. Only a well-formed, in-scope ARN reaches the lookup, where a table that
/// is not there is [`resource_arn_not_found`].
pub fn resolve_resource_arn(
    resource_arn: &str,
    request_region: &str,
) -> Result<TaggedResource, RequestError> {
    let invalid = || invalid_resource_arn(resource_arn);
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
    let table = resolve_table_name(path).map_err(|_| invalid())?;
    // The scope half of the rule: anything but this endpoint's own ARN form for that
    // table — another partition, another account, another region — is unaddressable
    // here, whatever this store happens to hold.
    if resource_arn != table_arn(request_region, &table) {
        return Err(invalid());
    }
    Ok(TaggedResource {
        region: request_region.to_string(),
        table,
        arn: resource_arn.to_string(),
    })
}

/// Fold a request's `TagList` into the stored [`TagSet`], refusing a list AWS does not
/// accept.
///
/// The one place a request's tag list becomes a set, so every operation that takes one
/// (`CreateTable$Tags`, `TagResource$Tags`) enforces the same two rules:
///
/// * a tag KEY may appear at most once in one request — "Each DynamoDB table can only
///   have up to one tag with the same key" (`Tag$Key`), so a list repeating a key asks
///   for two tags that cannot both exist, and is refused ([`duplicate_tag_key`])
///   instead of collapsing to whichever value happens to win;
/// * a key in the reserved `aws:` namespace is refused — the user "cannot assign" it —
///   so no stored key is ever one AWS alone mints.
///
/// Keys are compared as sent: tag keys are case sensitive, so `Env` and `env` are two
/// keys rather than one repeated.
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
        if set.insert(tag.key.clone(), tag.value.clone()).is_some() {
            return Err(duplicate_tag_key(&tag.key));
        }
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
/// Every clause is DynamoDB's own for the documented rule it states — the unparsable
/// text, the precision ceiling and the two magnitude bounds — and each is stated
/// inside the validation envelope ([`validation_message`]), which is where this
/// service states a clause about a value a request carried. Only the numeric-text
/// limit is this emulator's, and its message says so.
pub fn number_rejection(text: &str, fault: NumberFault) -> RequestError {
    match fault {
        NumberFault::NotNumeric => refused(format!(
            "The parameter cannot be converted to a numeric value: {}",
            clipped(text)
        )),
        NumberFault::Precision => refused(format!(
            "Attempting to store more than {MAX_SIGNIFICANT_DIGITS} significant digits in a Number"
        )),
        NumberFault::Overflow => refused(
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        NumberFault::Underflow => refused(
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

/// WHICH KIND of capacity an operation consumed, which decides the arm every figure it
/// reports is carried in beside the aggregate.
///
/// `ConsumedCapacity` and `Capacity` each carry an aggregate (`CapacityUnits`, "the total
/// number of capacity units consumed") and one figure per kind: `ReadCapacityUnits` is
/// "the total number of read capacity units consumed" and `WriteCapacityUnits` the write
/// equivalent. A data-plane operation spends capacity of ONE kind — a read reads and a
/// write writes — so the figure of that kind is the aggregate, and the other kind's is
/// absent because the operation consumed none of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityKind {
    /// Read capacity: what `GetItem`, `Query`, `Scan` and the transactional and batch
    /// reads consume.
    Read,
    /// Write capacity: what `PutItem`, `UpdateItem`, `DeleteItem` and the transactional
    /// and batch writes consume.
    Write,
}

impl CapacityKind {
    /// One `Capacity` line — a table's or an index's — of `units` of this kind: the
    /// aggregate and this kind's own figure, which are the same number.
    fn line(self, units: f64) -> Capacity {
        Capacity {
            capacity_units: Some(units),
            read_capacity_units: (self == Self::Read).then_some(units),
            write_capacity_units: (self == Self::Write).then_some(units),
        }
    }

    /// A `ConsumedCapacity` reporting `units` of this kind for `table_name`, with no
    /// per-table or per-index breakdown yet: the operation's aggregate and this kind's
    /// own figure, filled from [`Self::line`] so one rule decides every arm.
    fn consumed(self, table_name: &str, units: f64) -> ConsumedCapacity {
        let aggregate = self.line(units);
        ConsumedCapacity {
            table_name: Some(table_name.to_string()),
            capacity_units: aggregate.capacity_units,
            read_capacity_units: aggregate.read_capacity_units,
            write_capacity_units: aggregate.write_capacity_units,
            ..ConsumedCapacity::default()
        }
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
///
/// Every figure is reported in `kind`'s own arm as well as in the aggregate
/// ([`CapacityKind`]). The breakdown is a WRITE's — the table's units and those of each
/// index the write fanned out to — while the arm is the kind the operation BILLED it as,
/// which is `Write` for a write that was applied and `Read` where the service bills the
/// same figures as a read (a `ClientRequestToken` replay, which reads the transaction's
/// items rather than writing them).
pub fn consumed_write_capacity(
    requested: Option<&str>,
    table_name: &str,
    capacity: &WriteCapacity,
    kind: CapacityKind,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    Some(ConsumedCapacity {
        table: detail.then(|| kind.line(capacity.table)),
        local_secondary_indexes: detail
            .then(|| capacity_breakdown(kind, &capacity.local_secondary_indexes))
            .flatten(),
        global_secondary_indexes: detail
            .then(|| capacity_breakdown(kind, &capacity.global_secondary_indexes))
            .flatten(),
        ..kind.consumed(table_name, capacity.total())
    })
}

/// One per-index `ConsumedCapacity` map, or `None` when no index of that kind was
/// reached at all — an index an operation did not touch consumed nothing on it, so it is
/// left out rather than reported as zero.
fn capacity_breakdown(
    kind: CapacityKind,
    units: &BTreeMap<String, f64>,
) -> Option<HashMap<String, Capacity>> {
    (!units.is_empty()).then(|| {
        units
            .iter()
            .map(|(name, units)| (name.clone(), kind.line(*units)))
            .collect()
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

/// The `ConsumedCapacity` a read SERVED FROM A SECONDARY INDEX reports, or `None` when
/// the request asked for `NONE` (or did not ask at all).
///
/// `INDEXES` asks for "the aggregate ConsumedCapacity for the operation, together with
/// ConsumedCapacity for each table and secondary index that was accessed" (the
/// `ReturnConsumedCapacity` doc): a read of an index spends its units ON THAT INDEX, so
/// the index carries the aggregate and the table's own line is zero — the read did not go
/// to the table (`Scan$Select`: a read served from the index "reads only the index and not
/// the table"). Which of the two per-index maps the index appears under is its kind.
pub fn consumed_index_capacity(
    requested: Option<&str>,
    table_name: &str,
    index: &SecondaryIndex<'_>,
    units: f64,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    let kind = CapacityKind::Read;
    let mut per_index: BTreeMap<String, f64> = BTreeMap::new();
    per_index.insert(index.name.to_string(), units);
    let breakdown = detail.then(|| capacity_breakdown(kind, &per_index)).flatten();
    Some(ConsumedCapacity {
        // The read did not go to the table, so its own line is zero rather than absent:
        // `INDEXES` asks for "each table and secondary index that was accessed", and the
        // table that holds the index is named by every line of the breakdown.
        table: detail.then(|| kind.line(0.0)),
        local_secondary_indexes: (!index.global).then(|| breakdown.clone()).flatten(),
        global_secondary_indexes: index.global.then_some(breakdown).flatten(),
        ..kind.consumed(table_name, units)
    })
}

/// The `ConsumedCapacity` a READ that asked for it gets back, or `None` when it
/// asked for `NONE` (or did not ask at all).
///
/// `INDEXES` asks for the per-table breakdown as well as the aggregate; on a base
/// table there is no index to break out, so the table's own capacity is the whole of
/// it (the `ReturnConsumedCapacity` doc: "specifying INDEXES will only return
/// ConsumedCapacity information for table(s)").
///
/// Every figure is reported in the read arm as well as the aggregate
/// ([`CapacityKind`]).
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
    let kind = CapacityKind::Read;
    Some(ConsumedCapacity {
        table: detail.then(|| kind.line(units)),
        ..kind.consumed(table_name, units)
    })
}

// ===========================================================================
// What the multi-item paths share: `BatchWriteItem`, `BatchGetItem`,
// `TransactWriteItems` and `TransactGetItems`
// ===========================================================================

/// The ceilings, refusals, per-table ledgers and canonical encoding the four
/// multi-item operations share, in their own file: they are about a REQUEST that spans
/// several items, while this module is the table and item store itself. Re-exported
/// below, so every caller still names them `crate::state::…`.
pub mod multi_item;

pub use multi_item::*;

// ===========================================================================
// The PartiQL dialect `ExecuteStatement` runs
// ===========================================================================

/// The statement language the PartiQL surface serves, in its own file: it is about
/// the TEXT one request carries, which it parses into this module's primitives — the
/// item an `INSERT` writes, the key a singleton write addresses, and the condition
/// and update expressions the item API already evaluates. Re-exported below, so every
/// caller still names it `crate::state::…`.
pub mod partiql;

pub use partiql::*;

// ===========================================================================
// The expression language `PutItem`, `UpdateItem` and `DeleteItem` share
// ===========================================================================

/// The most any one expression member may carry: "Maximum length of any expression
/// string is 4 KB" ("Service, Account, and Table Quotas"), plus the ceilings on the
/// tokens and the nesting that 4 KB of text can express. All three bound the work one
/// request can ask the parsers below for, so neither the token vector nor the parser's
/// stack use is a function of what a caller sends.
pub const MAX_EXPRESSION_BYTES: usize = 4096;
pub const MAX_EXPRESSION_TOKENS: usize = 1024;
pub const MAX_EXPRESSION_DEPTH: usize = 16;

/// The most actions one update may state, whichever member states them.
///
/// An `UpdateExpression` is already bounded by the token ceiling above — the shortest
/// action an expression can spell takes three tokens (a path, an operator and a value)
/// — and this is that same bound, named, so the legacy `AttributeUpdates` map is held
/// to it too: that member carries no modelled length, so without this the number of
/// actions one request can ask the service to index, apply and write would be the
/// number of entries a request body can carry. Reaching it is the ordinary
/// `ValidationException`.
pub const MAX_UPDATE_ACTIONS: usize = MAX_EXPRESSION_TOKENS / 3;

/// The most entries one LEGACY CONDITION MAP — `ScanFilter`, `QueryFilter`, `Expected`
/// — may state.
///
/// The expression member each of those was replaced by is already bounded by the
/// ceilings above, and a legacy map carries no modelled length; without this the number
/// of comparisons one request can ask the service to evaluate FOR EVERY ITEM IT READS
/// would be the number of entries a request body can carry. The bound is that same
/// token ceiling read as comparisons — the shortest comparison an expression can spell
/// takes three tokens, a name, an operator and a value — so one legacy map is held to
/// what one expression of the documented size could state. Reaching it is the ordinary
/// `ValidationException`.
pub const MAX_LEGACY_CONDITIONS: usize = MAX_EXPRESSION_TOKENS / 3;

/// The most values one `IN` comparison may list: "The list can contain up to 100
/// values, separated by commas" (the Developer Guide's comparison-operator reference
/// for `IN`).
///
/// One ceiling for the legacy spelling of that comparison, whose `AttributeValueList`
/// has no modelled length, so what one `IN` entry costs per item read is a constant of
/// the service rather than of the request body.
pub const MAX_IN_OPERANDS: usize = 100;

/// The most members one COLLECTION attribute — a set, a list or a map — may carry,
/// whether a request sent it or an update computed it. This is the EMULATOR's own
/// limit, not an AWS quota.
///
/// A collection's cardinality is what every operation over it costs (comparison, set
/// union and difference, a `list_append` concatenation), and it is the request's to
/// choose. The documented 400 KB item limit does not bound it on its own, because a
/// member's accounted size can be zero (an empty string member). This ceiling is the
/// item limit's own byte count, so it refuses only collections far larger than one a
/// 400 KB item could hold with a byte per member, and it keeps the work one request can
/// ask for a function of this service's constants.
pub const MAX_COLLECTION_MEMBERS: usize = 400 * 1024;

/// The message a conditional write's refusal carries.
///
/// The CONTRACT is the error code `ConditionalCheckFailedException` and its 400; this
/// is the text the service is observed to send with it, the same for every operation
/// and every unmet condition, so it reveals nothing about the stored item.
pub const CONDITION_FAILED_MESSAGE: &str = "The conditional request failed";

/// One comparison an expression makes between two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl CompareOp {
    /// The operator as an expression writes it, which is what a refusal that quotes
    /// the offending token has to state.
    fn as_str(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "<>",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
        }
    }
}

/// One lexical token of a DynamoDB expression.
///
/// The lexer is shared by the condition and update grammars below: both spell a
/// document path, an `#alias`, a `:placeholder` and a parenthesised argument list the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprToken {
    /// A bare word: an attribute name, a section or operator keyword (`SET`, `AND`,
    /// ...) or a function name. Which it is, is the parser's business.
    Word(String),
    /// A `#alias` naming an `ExpressionAttributeNames` entry, with its `#`, which is
    /// how that map's keys are spelled.
    Alias(String),
    /// A `:placeholder` naming an `ExpressionAttributeValues` entry, with its `:`.
    Placeholder(String),
    /// A list index, as a document path writes it: `[3]`.
    Index(usize),
    Compare(CompareOp),
    Open,
    Close,
    Comma,
    Dot,
    Plus,
    Minus,
}

/// Which characters make up a bare word: an attribute name written out in an
/// expression, or a keyword. A name outside this set is reachable only through
/// `ExpressionAttributeNames`, which is what that member is for ("To prevent special
/// characters in an attribute name from being misinterpreted in an expression").
fn is_expression_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// The refusal for an expression member past [`MAX_EXPRESSION_BYTES`]: the member it
/// was sent as and the rule it broke, inside the same validation envelope every other
/// request refusal of this service renders. One wording for one rule, whichever
/// expression member a request oversized.
pub fn expression_size_exceeded(member: &str) -> RequestError {
    refused(format!(
        "Invalid {member}: Expression size has exceeded the maximum allowed size;"
    ))
}

/// One clause of the validation envelope for a refusal the EXPRESSION language raises:
/// AWS states the detail behind the expression member it came from, so a caller reading
/// the message knows which member of the request carried the mistake.
///
/// Whether the clause is wrapped in the `<N> validation error(s) detected: ` envelope
/// follows the MEMBER: the members a WRITE states its intent through are validated
/// inside it, and the members a READ narrows its result with are not — the service
/// states those on their own.
fn invalid_expression_member(member: &str, detail: &str) -> RequestError {
    expression_refusal(
        envelopes_expression_refusals(member),
        format!("Invalid {member}: {detail}"),
    )
}

/// The expression members whose refusals carry the validation envelope: the two a
/// write states its intent through.
const ENVELOPED_EXPRESSION_MEMBERS: [&str; 2] = ["UpdateExpression", "ConditionExpression"];

/// Whether a refusal about the expressions of a request carrying `member` is stated
/// inside the `<N> validation error(s) detected: ` envelope.
///
/// The ONE place that question is answered, for every refusal this service raises about
/// a request's expressions — an unreadable expression, a token no table defines, a table
/// entry no expression references — so the shape of the sentence follows the member the
/// request states its intent through and not the check that happened to notice.
fn envelopes_expression_refusals(member: &str) -> bool {
    ENVELOPED_EXPRESSION_MEMBERS.contains(&member)
}

/// State one expression refusal, inside the envelope or on its own.
fn expression_refusal(enveloped: bool, clause: String) -> RequestError {
    if enveloped {
        refused(clause)
    } else {
        RequestError::Validation(clause)
    }
}

/// The clause for a `#alias` an expression dereferences and `ExpressionAttributeNames`
/// does not define. ONE wording for that fact, whichever expression member carried the
/// token and whichever reader resolved it.
fn undefined_name_clause(alias: &str) -> String {
    format!(
        "An expression attribute name used in the document path is not defined; \
         attribute name: {alias}"
    )
}

/// The clause for a `:placeholder` an expression dereferences and
/// `ExpressionAttributeValues` does not define.
fn undefined_value_clause(placeholder: &str) -> String {
    format!(
        "An expression attribute value used in expression is not defined; \
         attribute value: {placeholder}"
    )
}

/// The refusal for an undefined `#alias`, stated behind the expression member that
/// carried it — what every reader of an expression answers for a token the request never
/// defined.
pub fn undefined_name(member: &str, alias: &str) -> RequestError {
    invalid_expression_member(member, &undefined_name_clause(alias))
}

/// The refusal for an undefined `:placeholder`, stated behind the expression member that
/// carried it.
pub fn undefined_value(member: &str, placeholder: &str) -> RequestError {
    invalid_expression_member(member, &undefined_value_clause(placeholder))
}

/// How a refusal names the position past the last token of an expression.
pub const END_OF_EXPRESSION_TOKEN: &str = "<EOF>";

/// The refusal for an expression this service cannot parse: the token the parse
/// stopped at and the text around it, both taken from the expression the request sent.
pub fn expression_syntax_error_at(member: &str, token: &str, near: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!("Syntax error; token: \"{token}\", near: \"{near}\""),
    )
}

/// The same refusal for a CHARACTER the lexer cannot read: the character is the token,
/// and the whitespace-delimited run it sits in is the text around it.
pub fn lexical_syntax_error(member: &str, characters: &[char], at: usize) -> RequestError {
    let token = characters
        .get(at)
        .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), |c| c.to_string());
    let start = characters[..at.min(characters.len())]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |index| index + 1);
    let end = characters
        .iter()
        .enumerate()
        .skip(at.min(characters.len()))
        .find(|(_, c)| c.is_whitespace())
        .map_or(characters.len(), |(index, _)| index);
    let near: String = characters[start..end.max(start)].iter().collect();
    expression_syntax_error_at(member, &token, &near)
}

/// The refusal for a DynamoDB RESERVED WORD written out as an attribute name.
///
/// "To access an attribute whose name conflicts with a DynamoDB reserved word" is what
/// `ExpressionAttributeNames` is for (`UpdateItem$ExpressionAttributeNames`), so a bare
/// reserved word in an expression is read as the keyword it is and the expression is
/// refused — the caller has to supply the name through that map instead.
fn reserved_attribute_name(member: &str, word: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!("Attribute name is a reserved keyword; reserved keyword: {word}"),
    )
}

/// The refusal for a function name the expression languages do not have.
///
/// The functions are a CLOSED set — "These function names are case-sensitive" and the
/// `ConditionExpression`/`UpdateExpression` docs list every one of them — so an
/// identifier called like a function that is not one of them is refused naming the
/// offending identifier, as the request spelled it, behind the expression member it
/// travelled in.
fn unknown_function(member: &str, name: &str) -> RequestError {
    invalid_expression_member(member, &format!("Invalid function name; function: {name}"))
}

/// Whether `word`, written out in an expression, is one of DynamoDB's reserved words.
///
/// Matched without regard to case, the way the keywords of the expression languages are
/// ("The operator and section keywords are case-insensitive").
fn is_reserved_word(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    RESERVED_WORDS.binary_search(&upper.as_str()).is_ok()
}

/// The lookup above is a binary search, which silently answers "not reserved" for every
/// word if the list below ever goes out of order — a missed reserved word is a request
/// wrongly accepted — so the order is a compile-time requirement rather than a
/// convention.
const _: () = {
    let mut at = 1;
    while at < RESERVED_WORDS.len() {
        assert!(
            str_before(RESERVED_WORDS[at - 1], RESERVED_WORDS[at]),
            "the reserved-word list must be sorted: the binary search below depends on it"
        );
        at += 1;
    }
};

/// Whether `left` sorts strictly before `right`, byte by byte — `str::cmp` is not
/// callable in a const context, so the comparison the assertion above needs is spelled
/// out here.
const fn str_before(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let mut at = 0;
    while at < left.len() && at < right.len() {
        if left[at] != right[at] {
            return left[at] < right[at];
        }
        at += 1;
    }
    left.len() < right.len()
}

/// DynamoDB's reserved words, from the "Reserved words in DynamoDB" list in the Amazon
/// DynamoDB Developer Guide, sorted so a lookup is a binary search rather than a scan
/// of the whole list per name an expression writes.
const RESERVED_WORDS: [&str; 573] = [
    "ABORT", "ABSOLUTE", "ACTION", "ADD", "AFTER", "AGENT", "AGGREGATE", "ALL", "ALLOCATE",
    "ALTER", "ANALYZE", "AND", "ANY", "ARCHIVE", "ARE", "ARRAY", "AS", "ASC", "ASCII",
    "ASENSITIVE", "ASSERTION", "ASYMMETRIC", "AT", "ATOMIC", "ATTACH", "ATTRIBUTE", "AUTH",
    "AUTHORIZATION", "AUTHORIZE", "AUTO", "AVG", "BACK", "BACKUP", "BASE", "BATCH", "BEFORE",
    "BEGIN", "BETWEEN", "BIGINT", "BINARY", "BIT", "BLOB", "BLOCK", "BOOLEAN", "BOTH",
    "BREADTH", "BUCKET", "BULK", "BY", "BYTE", "CALL", "CALLED", "CALLING", "CAPACITY",
    "CASCADE", "CASCADED", "CASE", "CAST", "CATALOG", "CHAR", "CHARACTER", "CHECK", "CLASS",
    "CLOB", "CLOSE", "CLUSTER", "CLUSTERED", "CLUSTERING", "CLUSTERS", "COALESCE", "COLLATE",
    "COLLATION", "COLLECTION", "COLUMN", "COLUMNS", "COMBINE", "COMMENT", "COMMIT", "COMPACT",
    "COMPILE", "COMPRESS", "CONDITION", "CONFLICT", "CONNECT", "CONNECTION", "CONSISTENCY",
    "CONSISTENT", "CONSTRAINT", "CONSTRAINTS", "CONSTRUCTOR", "CONSUMED", "CONTINUE", "CONVERT",
    "COPY", "CORRESPONDING", "COUNT", "COUNTER", "CREATE", "CROSS", "CUBE", "CURRENT", "CURSOR",
    "CYCLE", "DATA", "DATABASE", "DATE", "DATETIME", "DAY", "DEALLOCATE", "DEC", "DECIMAL",
    "DECLARE", "DEFAULT", "DEFERRABLE", "DEFERRED", "DEFINE", "DEFINED", "DEFINITION", "DELETE",
    "DELIMITED", "DEPTH", "DEREF", "DESC", "DESCRIBE", "DESCRIPTOR", "DETACH", "DETERMINISTIC",
    "DIAGNOSTICS", "DIRECTORIES", "DISABLE", "DISCONNECT", "DISTINCT", "DISTRIBUTE", "DO",
    "DOMAIN", "DOUBLE", "DROP", "DUMP", "DURATION", "DYNAMIC", "EACH", "ELEMENT", "ELSE",
    "ELSEIF", "EMPTY", "ENABLE", "END", "EQUAL", "EQUALS", "ERROR", "ESCAPE", "ESCAPED", "EVAL",
    "EVALUATE", "EXCEEDED", "EXCEPT", "EXCEPTION", "EXCEPTIONS", "EXCLUSIVE", "EXEC", "EXECUTE",
    "EXISTS", "EXIT", "EXPLAIN", "EXPLODE", "EXPORT", "EXPRESSION", "EXTENDED", "EXTERNAL",
    "EXTRACT", "FAIL", "FALSE", "FAMILY", "FETCH", "FIELDS", "FILE", "FILTER", "FILTERING",
    "FINAL", "FINISH", "FIRST", "FIXED", "FLATTERN", "FLOAT", "FOR", "FORCE", "FOREIGN",
    "FORMAT", "FORWARD", "FOUND", "FREE", "FROM", "FULL", "FUNCTION", "FUNCTIONS", "GENERAL",
    "GENERATE", "GET", "GLOB", "GLOBAL", "GO", "GOTO", "GRANT", "GREATER", "GROUP", "GROUPING",
    "HANDLER", "HASH", "HAVE", "HAVING", "HEAP", "HIDDEN", "HOLD", "HOUR", "IDENTIFIED",
    "IDENTITY", "IF", "IGNORE", "IMMEDIATE", "IMPORT", "IN", "INCLUDING", "INCLUSIVE",
    "INCREMENT", "INCREMENTAL", "INDEX", "INDEXED", "INDEXES", "INDICATOR", "INFINITE",
    "INITIALLY", "INLINE", "INNER", "INNTER", "INOUT", "INPUT", "INSENSITIVE", "INSERT",
    "INSTEAD", "INT", "INTEGER", "INTERSECT", "INTERVAL", "INTO", "INVALIDATE", "IS",
    "ISOLATION", "ITEM", "ITEMS", "ITERATE", "JOIN", "KEY", "KEYS", "LAG", "LANGUAGE", "LARGE",
    "LAST", "LATERAL", "LEAD", "LEADING", "LEAVE", "LEFT", "LENGTH", "LESS", "LEVEL", "LIKE",
    "LIMIT", "LIMITED", "LINES", "LIST", "LOAD", "LOCAL", "LOCALTIME", "LOCALTIMESTAMP",
    "LOCATION", "LOCATOR", "LOCK", "LOCKS", "LOG", "LOGED", "LONG", "LOOP", "LOWER", "MAP",
    "MATCH", "MATERIALIZED", "MAX", "MAXLEN", "MEMBER", "MERGE", "METHOD", "METRICS", "MIN",
    "MINUS", "MINUTE", "MISSING", "MOD", "MODE", "MODIFIES", "MODIFY", "MODULE", "MONTH",
    "MULTI", "MULTISET", "NAME", "NAMES", "NATIONAL", "NATURAL", "NCHAR", "NCLOB", "NEW",
    "NEXT", "NO", "NONE", "NOT", "NULL", "NULLIF", "NUMBER", "NUMERIC", "OBJECT", "OF",
    "OFFLINE", "OFFSET", "OLD", "ON", "ONLINE", "ONLY", "OPAQUE", "OPEN", "OPERATOR", "OPTION",
    "OR", "ORDER", "ORDINALITY", "OTHER", "OTHERS", "OUT", "OUTER", "OUTPUT", "OVER",
    "OVERLAPS", "OVERRIDE", "OWNER", "PAD", "PARALLEL", "PARAMETER", "PARAMETERS", "PARTIAL",
    "PARTITION", "PARTITIONED", "PARTITIONS", "PATH", "PERCENT", "PERCENTILE", "PERMISSION",
    "PERMISSIONS", "PIPE", "PIPELINED", "PLAN", "POOL", "POSITION", "PRECISION", "PREPARE",
    "PRESERVE", "PRIMARY", "PRIOR", "PRIVATE", "PRIVILEGES", "PROCEDURE", "PROCESSED",
    "PROJECT", "PROJECTION", "PROPERTY", "PROVISIONING", "PUBLIC", "PUT", "QUERY", "QUIT",
    "QUORUM", "RAISE", "RANDOM", "RANGE", "RANK", "RAW", "READ", "READS", "REAL", "REBUILD",
    "RECORD", "RECURSIVE", "REDUCE", "REF", "REFERENCE", "REFERENCES", "REFERENCING", "REGEXP",
    "REGION", "REINDEX", "RELATIVE", "RELEASE", "REMAINDER", "RENAME", "REPEAT", "REPLACE",
    "REQUEST", "RESET", "RESIGNAL", "RESOURCE", "RESPONSE", "RESTORE", "RESTRICT", "RESULT",
    "RETURN", "RETURNING", "RETURNS", "REVERSE", "REVOKE", "RIGHT", "ROLE", "ROLES",
    "ROLLBACK", "ROLLUP", "ROUTINE", "ROW", "ROWS", "RULE", "RULES", "SAMPLE", "SATISFIES",
    "SAVE", "SAVEPOINT", "SCAN", "SCHEMA", "SCOPE", "SCROLL", "SEARCH", "SECOND", "SECTION",
    "SEGMENT", "SEGMENTS", "SELECT", "SELF", "SEMI", "SENSITIVE", "SEPARATE", "SEQUENCE",
    "SERIALIZABLE", "SESSION", "SET", "SETS", "SHARD", "SHARE", "SHARED", "SHORT", "SHOW",
    "SIGNAL", "SIMILAR", "SIZE", "SKEWED", "SMALLINT", "SNAPSHOT", "SOME", "SOURCE", "SPACE",
    "SPACES", "SPARSE", "SPECIFIC", "SPECIFICTYPE", "SPLIT", "SQL", "SQLCODE", "SQLERROR",
    "SQLEXCEPTION", "SQLSTATE", "SQLWARNING", "START", "STATE", "STATIC", "STATUS", "STORAGE",
    "STORE", "STORED", "STREAM", "STRING", "STRUCT", "STYLE", "SUB", "SUBMULTISET",
    "SUBPARTITION", "SUBSTRING", "SUBTYPE", "SUM", "SUPER", "SYMMETRIC", "SYNONYM", "SYSTEM",
    "TABLE", "TABLESAMPLE", "TEMP", "TEMPORARY", "TERMINATED", "TEXT", "THAN", "THEN",
    "THROUGHPUT", "TIME", "TIMESTAMP", "TIMEZONE", "TINYINT", "TO", "TOKEN", "TOTAL", "TOUCH",
    "TRAILING", "TRANSACTION", "TRANSFORM", "TRANSLATE", "TRANSLATION", "TREAT", "TRIGGER",
    "TRIM", "TRUE", "TRUNCATE", "TTL", "TUPLE", "TYPE", "UNDER", "UNDO", "UNION", "UNIQUE",
    "UNIT", "UNKNOWN", "UNLOGGED", "UNNEST", "UNPROCESSED", "UNSIGNED", "UNTIL", "UPDATE",
    "UPPER", "URL", "USAGE", "USE", "USER", "USERS", "USING", "UUID", "VACUUM", "VALUE",
    "VALUED", "VALUES", "VARCHAR", "VARIABLE", "VARIANCE", "VARINT", "VARYING", "VIEW",
    "VIEWS", "VIRTUAL", "VOID", "WAIT", "WHEN", "WHENEVER", "WHERE", "WHILE", "WINDOW", "WITH",
    "WITHIN", "WITHOUT", "WORK", "WRAPPED", "WRITE", "YEAR", "ZONE",
];

/// The expression text around the token at `at`: the token before it, the token itself
/// (there is none past the end) and the token after, as the expression wrote them.
fn token_window(tokens: &[ExprToken], at: usize) -> String {
    let before = at.checked_sub(1).and_then(|index| tokens.get(index));
    let here = tokens.get(at);
    let after = tokens.get(at + 1);
    [before, here, after]
        .into_iter()
        .flatten()
        .map(token_text)
        .collect::<Vec<String>>()
        .join(" ")
}

/// One token as the expression wrote it, which is what the two refusals above quote.
fn token_text(token: &ExprToken) -> String {
    match token {
        ExprToken::Word(word) => word.clone(),
        ExprToken::Alias(alias) => alias.clone(),
        ExprToken::Placeholder(placeholder) => placeholder.clone(),
        ExprToken::Index(index) => format!("[{index}]"),
        ExprToken::Compare(operator) => operator.as_str().to_string(),
        ExprToken::Open => "(".to_string(),
        ExprToken::Close => ")".to_string(),
        ExprToken::Comma => ",".to_string(),
        ExprToken::Dot => ".".to_string(),
        ExprToken::Plus => "+".to_string(),
        ExprToken::Minus => "-".to_string(),
    }
}

/// Split one expression into tokens, refusing a text or a token count past the bounds
/// above before any parsing starts.
pub fn tokenize_expression(
    member: &str,
    expression: &str,
) -> Result<Vec<ExprToken>, RequestError> {
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(expression_size_exceeded(member));
    }
    let characters: Vec<char> = expression.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        if tokens.len() >= MAX_EXPRESSION_TOKENS {
            return Err(RequestError::Validation(format!(
                "Invalid {member}: an expression may hold at most {MAX_EXPRESSION_TOKENS} tokens"
            )));
        }
        let character = characters[at];
        match character {
            c if c.is_whitespace() => at += 1,
            '(' => {
                tokens.push(ExprToken::Open);
                at += 1;
            }
            ')' => {
                tokens.push(ExprToken::Close);
                at += 1;
            }
            ',' => {
                tokens.push(ExprToken::Comma);
                at += 1;
            }
            '.' => {
                tokens.push(ExprToken::Dot);
                at += 1;
            }
            '+' => {
                tokens.push(ExprToken::Plus);
                at += 1;
            }
            '-' => {
                tokens.push(ExprToken::Minus);
                at += 1;
            }
            '=' => {
                tokens.push(ExprToken::Compare(CompareOp::Equal));
                at += 1;
            }
            '<' | '>' => {
                let next = characters.get(at + 1).copied();
                let (operator, width) = match (character, next) {
                    ('<', Some('=')) => (CompareOp::LessOrEqual, 2),
                    ('<', Some('>')) => (CompareOp::NotEqual, 2),
                    ('<', _) => (CompareOp::Less, 1),
                    (_, Some('=')) => (CompareOp::GreaterOrEqual, 2),
                    _ => (CompareOp::Greater, 1),
                };
                tokens.push(ExprToken::Compare(operator));
                at += width;
            }
            '[' => {
                let start = at + 1;
                let mut end = start;
                while end < characters.len() && characters[end].is_ascii_digit() {
                    end += 1;
                }
                if end == start || characters.get(end) != Some(&']') {
                    return Err(lexical_syntax_error(member, &characters, at));
                }
                let digits: String = characters[start..end].iter().collect();
                let index = digits
                    .parse::<usize>()
                    .map_err(|_| lexical_syntax_error(member, &characters, at))?;
                tokens.push(ExprToken::Index(index));
                at = end + 1;
            }
            '#' | ':' => {
                let start = at;
                at += 1;
                while at < characters.len() && is_expression_word_char(characters[at]) {
                    at += 1;
                }
                if at - start < 2 {
                    return Err(lexical_syntax_error(member, &characters, start));
                }
                let text: String = characters[start..at].iter().collect();
                tokens.push(if character == '#' {
                    ExprToken::Alias(text)
                } else {
                    ExprToken::Placeholder(text)
                });
            }
            c if is_expression_word_char(c) => {
                let start = at;
                while at < characters.len() && is_expression_word_char(characters[at]) {
                    at += 1;
                }
                tokens.push(ExprToken::Word(characters[start..at].iter().collect()));
            }
            _ => return Err(lexical_syntax_error(member, &characters, at)),
        }
    }
    if tokens.is_empty() {
        return Err(lexical_syntax_error(member, &characters, characters.len()));
    }
    Ok(tokens)
}

/// The TOP-LEVEL attribute names one `ProjectionExpression` names: "A string that
/// identifies one or more attributes to retrieve … Separate attribute names with commas"
/// (`ProjectionExpression`), each written out or reached through an
/// `ExpressionAttributeNames` alias.
///
/// `Ok(None)` is the one well-formed case this does not answer a name list for: a path
/// that reaches INSIDE an attribute (`a.b`, `a[1]`). Whether that is served is the
/// caller's to decide — a reader that does not serve nested paths answers the scaffold's
/// 501 for it rather than refusing a request the real service accepts.
///
/// Lives here beside [`tokenize_expression`] and [`Substitutions`] so every operation
/// that serves the member reads it the same way.
pub fn projection_expression_attributes(
    member: &str,
    expression: &str,
    substitutions: Substitutions<'_>,
) -> Result<Option<BTreeSet<String>>, RequestError> {
    let tokens = tokenize_expression(member, expression)?;
    let mut attributes: BTreeSet<String> = BTreeSet::new();
    // The list reads as name (`,` name)*, so a name is expected at the start and after
    // every comma, and nowhere else.
    let mut expecting_name = true;
    for (at, token) in tokens.iter().enumerate() {
        match token {
            ExprToken::Word(name) if expecting_name => {
                // A bare reserved word is read as the keyword it is, wherever an
                // expression writes an attribute name.
                if is_reserved_word(name) {
                    return Err(reserved_attribute_name(member, name));
                }
                attributes.insert(name.clone());
                expecting_name = false;
            }
            ExprToken::Alias(alias) if expecting_name => {
                attributes.insert(
                    substitutions
                        .name(alias)
                        .map_err(|error| invalid_expression_member(member, error.message()))?,
                );
                expecting_name = false;
            }
            ExprToken::Comma if !expecting_name => expecting_name = true,
            ExprToken::Dot | ExprToken::Index(_) => return Ok(None),
            _ => {
                return Err(expression_syntax_error_at(
                    member,
                    &token_text(token),
                    &token_window(&tokens, at),
                ))
            }
        }
    }
    if expecting_name {
        // Nothing, or a trailing comma: the list names an attribute it never gives.
        return Err(expression_syntax_error_at(
            member,
            END_OF_EXPRESSION_TOKEN,
            &token_window(&tokens, tokens.len()),
        ));
    }
    Ok(Some(attributes))
}

/// The substitution tables an expression resolves its names and values through:
/// `ExpressionAttributeNames` and `ExpressionAttributeValues`, exactly as the request
/// sent them.
#[derive(Debug, Clone, Copy, Default)]
pub struct Substitutions<'a> {
    names: Option<&'a HashMap<String, String>>,
    values: Option<&'a HashMap<String, AttributeValue>>,
}

impl<'a> Substitutions<'a> {
    pub fn new(
        names: Option<&'a HashMap<String, String>>,
        values: Option<&'a HashMap<String, AttributeValue>>,
    ) -> Self {
        Self { names, values }
    }

    /// The attribute name an `#alias` stands for. An alias with no entry is the
    /// caller's mistake, not an empty name.
    pub fn name(&self, alias: &str) -> Result<String, RequestError> {
        self.names
            .and_then(|names| names.get(alias))
            .cloned()
            .ok_or_else(|| RequestError::Validation(undefined_name_clause(alias)))
    }

    /// The value a `:placeholder` stands for.
    pub fn value(&self, placeholder: &str) -> Result<AttributeValue, RequestError> {
        self.values
            .and_then(|values| values.get(placeholder))
            .cloned()
            .ok_or_else(|| RequestError::Validation(undefined_value_clause(placeholder)))
    }

    /// Hold the request's expressions and both substitution tables to each other: every
    /// token an expression DEREFERENCES must be one the tables define, and every token
    /// the tables define must be one an expression REFERENCES. The whole request is
    /// refused when either side has a token the other does not.
    ///
    /// Both members exist to be substituted INTO — they hold "substitution tokens for
    /// attribute names in an expression" and "values that can be substituted in an
    /// expression" (those members' docs) — so a token no expression names states
    /// nothing, and DynamoDB refuses the request rather than ignoring it. One entry
    /// point for every operation that takes expression members, so the rule does not
    /// drift between them.
    ///
    /// PRECEDENCE: a token the expression dereferences and no table defines is what the
    /// caller is told about FIRST, before any surplus entry, because it is the mistake
    /// that makes the expression unreadable — dropping a surplus entry would leave the
    /// request refused for the undefined token anyway. It is stated through the member
    /// that carried the expression, in the same words the parser that resolves the token
    /// for real states it in, so one request earns one sentence whichever check reaches
    /// it first.
    ///
    /// `expressions` pairs each expression member of the request with its value as it
    /// was sent; an expression this does not tokenize is left to the parser that reads
    /// it for real, which refuses it with the syntax error the member earns.
    pub fn check_all_referenced(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut values: BTreeSet<String> = BTreeSet::new();
        for (member, expression) in expressions {
            let Some(expression) = expression else {
                continue;
            };
            let Ok(tokens) = tokenize_expression(member, expression) else {
                // An expression this cannot read tells nothing about which tokens it
                // references, and the parser that reads it for real refuses it with the
                // syntax error the member earns — which is the refusal the caller gets.
                return Ok(());
            };
            for token in tokens {
                match token {
                    ExprToken::Alias(alias) => {
                        if self.name(&alias).is_err() {
                            return Err(undefined_name(member, &alias));
                        }
                        names.insert(alias);
                    }
                    ExprToken::Placeholder(placeholder) => {
                        if self.value(&placeholder).is_err() {
                            return Err(undefined_value(member, &placeholder));
                        }
                        values.insert(placeholder);
                    }
                    _ => {}
                }
            }
        }
        // The sentence follows the expression member this request states its intent
        // through, like every other refusal raised about its expressions.
        let enveloped = expressions.iter().any(|(member, expression)| {
            expression.is_some() && envelopes_expression_refusals(member)
        });
        check_referenced("ExpressionAttributeNames", self.names, &names, enveloped)?;
        check_referenced("ExpressionAttributeValues", self.values, &values, enveloped)
    }
}

/// Refuse one substitution table that defines a key no expression referenced.
///
/// The keys are reported in one stable order — the order the map sorts in — so the
/// same request earns the same message every time, and the refusal names the member
/// that carried them.
///
/// This rule is the SERVICE's own, checked once the request validator has already
/// passed a well-formed input, so the sentence is stated on its own and carries the
/// `<N> validation error(s) detected: ` envelope only where every other refusal about
/// this request's expressions does — `enveloped`, decided by
/// [`envelopes_expression_refusals`] over the members the request carried.
fn check_referenced<V>(
    member: &str,
    supplied: Option<&HashMap<String, V>>,
    referenced: &BTreeSet<String>,
    enveloped: bool,
) -> Result<(), RequestError> {
    let unused: BTreeSet<&str> = supplied
        .into_iter()
        .flatten()
        .map(|(key, _)| key.as_str())
        .filter(|key| !referenced.contains(*key))
        .collect();
    if unused.is_empty() {
        return Ok(());
    }
    Err(expression_refusal(
        enveloped,
        format!(
            "Value provided in {member} unused in expressions: keys: {{{}}}",
            unused.into_iter().collect::<Vec<&str>>().join(", ")
        ),
    ))
}

/// One step of a document path: an attribute name, or an index into a list.
///
/// Ordered so that two paths of one update can be ordered step by step — an index by
/// its number and an attribute by its name — which is what lets the removals of one
/// update be applied greatest-first (see [`ParsedUpdate::apply`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathStep {
    Attribute(String),
    Index(usize),
}

/// A document path, as an expression writes it — `a`, `a.b`, `a[2].b` — with every
/// `#alias` already resolved. The first step is always an attribute name, so every
/// path starts at one of the item's own attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPath(Vec<PathStep>);

impl DocumentPath {
    /// The path naming one top-level attribute, which is all the legacy
    /// `AttributeUpdates`/`Expected` maps can address.
    pub fn attribute_named(name: &str) -> Self {
        Self(vec![PathStep::Attribute(name.to_string())])
    }

    /// The top-level attribute this path reaches into, which is the attribute a write
    /// through it changes.
    pub fn attribute(&self) -> &str {
        match self.0.first() {
            Some(PathStep::Attribute(name)) => name.as_str(),
            // Unreachable by construction: every path starts at an attribute.
            _ => "",
        }
    }

    /// The whole path as ONE top-level attribute, when that is all it is: `a` yes, `a.b`
    /// and `a[0]` no. What tells a path that READS an attribute from one that reaches
    /// inside it.
    pub fn single_attribute(&self) -> Option<&str> {
        match self.0.as_slice() {
            [PathStep::Attribute(name)] => Some(name.as_str()),
            _ => None,
        }
    }

    /// How the path reads, for a refusal message and for the prefix index two actions
    /// are checked for overlap through.
    fn render(&self) -> String {
        self.render_prefix(self.0.len())
    }

    /// The same for this path's first `steps` steps only: a path overlaps another
    /// exactly when one of them is one of the other's step-prefixes.
    fn render_prefix(&self, steps: usize) -> String {
        let mut text = String::new();
        for step in self.0.iter().take(steps) {
            match step {
                PathStep::Attribute(name) => {
                    if !text.is_empty() {
                        text.push('.');
                    }
                    text.push_str(name);
                }
                PathStep::Index(index) => {
                    text.push('[');
                    text.push_str(&index.to_string());
                    text.push(']');
                }
            }
        }
        text
    }

    /// The same path as AWS's refusals print it: the steps as a comma-separated token
    /// list, an index keeping its brackets — `l[1]` is `[l, [1]]` and `m.a.b` is
    /// `[m, a, b]`.
    fn render_tokens(&self) -> String {
        self.render_tokens_prefix(self.0.len())
    }

    /// The same for this path's first `steps` steps only, which is what the overlap
    /// refusal states for the path that contains the other.
    fn render_tokens_prefix(&self, steps: usize) -> String {
        let tokens: Vec<String> = self
            .0
            .iter()
            .take(steps)
            .map(|step| match step {
                PathStep::Attribute(name) => name.clone(),
                PathStep::Index(index) => format!("[{index}]"),
            })
            .collect();
        format!("[{}]", tokens.join(", "))
    }

    /// The refusal for a path that does not address a location this item can hold.
    ///
    /// AWS's text for this condition is FIXED: it states that the path is invalid and
    /// nothing of which path it was, so a client matching the sentence exactly — the
    /// normal pattern for a constant message — recognises the condition.
    fn invalid(&self) -> RequestError {
        RequestError::Validation(INVALID_DOCUMENT_PATH_MESSAGE.to_string())
    }

    /// The value at this path, or `None` when the item holds nothing there.
    pub fn get<'i>(&self, item: &'i Item) -> Option<&'i AttributeValue> {
        let mut steps = self.0.iter();
        let PathStep::Attribute(first) = steps.next()? else {
            return None;
        };
        let mut current = item.get(first)?;
        for step in steps {
            current = match (step, current) {
                (PathStep::Attribute(name), AttributeValue::M(entries)) => entries.get(name)?,
                (PathStep::Index(index), AttributeValue::L(values)) => values.get(*index)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Write `value` at this path, creating a missing map entry and appending past the
    /// end of a list, but never inventing a missing parent container.
    pub fn set(&self, item: &mut Item, value: AttributeValue) -> Result<(), RequestError> {
        let Some((PathStep::Attribute(first), rest)) = self.0.split_first() else {
            return Err(self.invalid());
        };
        if rest.is_empty() {
            item.insert(first.clone(), value);
            return Ok(());
        }
        let parent = item.get_mut(first).ok_or_else(|| self.invalid())?;
        set_within(parent, rest, value).map_err(|()| self.invalid())
    }

    /// Remove whatever this path addresses, reporting what was there.
    pub fn remove(&self, item: &mut Item) -> Option<AttributeValue> {
        let (head, rest) = self.0.split_first()?;
        let PathStep::Attribute(first) = head else {
            return None;
        };
        if rest.is_empty() {
            return item.remove(first);
        }
        remove_within(item.get_mut(first)?, rest)
    }
}

/// Write `value` at `steps` inside `target`.
///
/// Recursive, and bounded: a path carries at most [`MAX_ATTRIBUTE_DEPTH`] steps (the
/// parser refuses a longer one), so the number of frames is a constant of this service
/// rather than a function of the request.
fn set_within(
    target: &mut AttributeValue,
    steps: &[PathStep],
    value: AttributeValue,
) -> Result<(), ()> {
    match (steps.first(), target) {
        (None, slot) => {
            *slot = value;
            Ok(())
        }
        (Some(PathStep::Attribute(name)), AttributeValue::M(entries)) => {
            if steps.len() == 1 {
                entries.insert(name.clone(), value);
                Ok(())
            } else {
                set_within(entries.get_mut(name).ok_or(())?, &steps[1..], value)
            }
        }
        (Some(PathStep::Index(index)), AttributeValue::L(values)) => {
            if steps.len() == 1 {
                // "the new element is appended to the end of the list" when the index
                // is past its end, which is what DynamoDB does with an out-of-range
                // index in a SET.
                if *index < values.len() {
                    values[*index] = value;
                } else {
                    values.push(value);
                }
                Ok(())
            } else {
                set_within(values.get_mut(*index).ok_or(())?, &steps[1..], value)
            }
        }
        _ => Err(()),
    }
}

/// Remove what `steps` addresses inside `target`, bounded the same way
/// [`set_within`] is. Removing a list element shifts the ones after it down.
fn remove_within(target: &mut AttributeValue, steps: &[PathStep]) -> Option<AttributeValue> {
    match (steps.first()?, target) {
        (PathStep::Attribute(name), AttributeValue::M(entries)) => {
            if steps.len() == 1 {
                entries.remove(name)
            } else {
                remove_within(entries.get_mut(name)?, &steps[1..])
            }
        }
        (PathStep::Index(index), AttributeValue::L(values)) => {
            if steps.len() == 1 {
                (*index < values.len()).then(|| values.remove(*index))
            } else {
                remove_within(values.get_mut(*index)?, &steps[1..])
            }
        }
        _ => None,
    }
}

/// Whether two attribute values are the same value.
///
/// Comparison is by TYPE first, as the `ComparisonOperator` doc's `EQ` entry states:
/// "If an item contains an AttributeValue element of a different type than the one
/// provided in the request, the value does not match. For example, {"S":"6"} does not
/// equal {"N":"6"}". Within a type, two `N` values compare NUMERICALLY (`6` equals
/// `6.0`), a set compares as a set — its members in any order — and a list or map
/// compares element by element.
pub fn attribute_values_equal(left: &AttributeValue, right: &AttributeValue) -> bool {
    values_equal_within(left, right, MAX_ATTRIBUTE_DEPTH)
}

/// The same, with an explicit depth budget: a stored value nests at most
/// [`MAX_ATTRIBUTE_DEPTH`] levels (every write path refuses a deeper one), so the walk
/// cannot recurse further than that however the two values are shaped.
fn values_equal_within(left: &AttributeValue, right: &AttributeValue, budget: usize) -> bool {
    if budget == 0 {
        return false;
    }
    match (left, right) {
        (AttributeValue::S(mine), AttributeValue::S(theirs)) => mine == theirs,
        (AttributeValue::B(mine), AttributeValue::B(theirs)) => mine == theirs,
        (AttributeValue::N(mine), AttributeValue::N(theirs)) => numbers_equal(mine, theirs),
        (AttributeValue::Bool(mine), AttributeValue::Bool(theirs)) => mine == theirs,
        (AttributeValue::Null(mine), AttributeValue::Null(theirs)) => mine == theirs,
        (AttributeValue::Ss(mine), AttributeValue::Ss(theirs)) => {
            same_members(mine, theirs, |text| text.as_str())
        }
        (AttributeValue::Ns(mine), AttributeValue::Ns(theirs)) => {
            same_members(mine, theirs, |text| canonical_number(text))
        }
        (AttributeValue::Bs(mine), AttributeValue::Bs(theirs)) => {
            same_members(mine, theirs, |bytes| bytes.as_slice())
        }
        (AttributeValue::L(mine), AttributeValue::L(theirs)) => {
            mine.len() == theirs.len()
                && mine
                    .iter()
                    .zip(theirs.iter())
                    .all(|(a, b)| values_equal_within(a, b, budget - 1))
        }
        (AttributeValue::M(mine), AttributeValue::M(theirs)) => {
            mine.len() == theirs.len()
                && mine.iter().all(|(name, value)| {
                    theirs
                        .get(name)
                        .is_some_and(|other| values_equal_within(value, other, budget - 1))
                })
        }
        _ => false,
    }
}

/// Whether two `N` texts denote one value, which is a numeric question rather than a
/// textual one. A text neither side can parse falls back to the text itself.
fn numbers_equal(left: &str, right: &str) -> bool {
    match (NumberKey::parse(left), NumberKey::parse(right)) {
        (Some(mine), Some(theirs)) => mine == theirs,
        _ => left == right,
    }
}

/// The refusal for an update that states more actions than this service will index,
/// apply and write for one request.
fn too_many_actions(count: usize) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: an update may state at most \
         {MAX_UPDATE_ACTIONS} actions, got {count}"
    ))
}

/// AWS's fixed text for an update whose document path addresses no location the item
/// can hold — a step through a parent that is not there.
pub const INVALID_DOCUMENT_PATH_MESSAGE: &str =
    "The document path provided in the update expression is invalid for update";

/// AWS's fixed text for an update action given an operand of a data type that action
/// cannot be applied to — an `ADD` of one set type onto another, an `ADD` of anything
/// but a number onto a number, an `ADD` or `DELETE` against an attribute that is
/// neither a number nor a set.
///
/// It names NEITHER operand: the sentence AWS states for this class carries no
/// attribute path and no `AttributeValue` type tag, so the types the request and the
/// stored item happened to carry are not interpolated into it.
pub const INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE: &str =
    "An operand in the update expression has an incorrect data type";

/// The refusal [`INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE`] states, which every update
/// action raises for an operand whose type it cannot be applied to.
fn incorrect_update_operand_type() -> RequestError {
    RequestError::Validation(INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE.to_string())
}

/// AWS's fixed text for an update operand that reads an attribute the item does not
/// carry: the sentence names the class of mistake and not the path that was read, which
/// is the one place a caller would otherwise read a path out of this message.
pub const MISSING_OPERAND_ATTRIBUTE_MESSAGE: &str =
    "The provided expression refers to an attribute that does not exist in the item";

/// The refusal for two actions of one update that write the same document location, or
/// one inside the other: DynamoDB cannot apply both.
///
/// The two paths are stated in the ORDER THE UPDATE STATES THEM — `path one` is the
/// path of the earlier action and `path two` the later one, whichever of them is the
/// deeper — because the message is a report of what the caller wrote. Two actions on
/// the SAME location state that one location twice, which reads alike either way round.
fn overlapping_paths(member: &str, one: &str, two: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "Two document paths overlap with each other; must remove or rewrite one of these \
             paths; path one: {one}, path two: {two}"
        ),
    )
}

/// The refusal for an operand whose type the operator or function it was given to does
/// not take: AWS names the operator or function and the type of the offending operand,
/// both read off the request.
fn incorrect_operand_type(
    member: &str,
    operator_or_function: &str,
    operand_type: &str,
) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "Incorrect operand type for operator or function; operator or function: \
             {operator_or_function}, operand type: {operand_type}"
        ),
    )
}

/// The limits every attribute value this service stores — or compares a stored one
/// against — is held to: the documented nesting depth, the documented 400 KB item size
/// (no ONE value can be larger than the item that must hold it), and
/// [`MAX_COLLECTION_MEMBERS`] over the sets, lists and maps the value carries.
///
/// Every term comes from the request, and every one bounds work done per request later
/// (the walk over a value, the clone of it into the item, the set and list operations
/// over its members), so every value is held to them at admission — on every write
/// path, for every substituted expression value, AND for every value an update computes
/// before it is written.
pub fn check_attribute_value(name: &str, value: &AttributeValue) -> Result<(), RequestError> {
    if attribute_depth_exceeds(value, MAX_ATTRIBUTE_DEPTH) {
        return Err(nesting_levels_exceeded());
    }
    // No ONE value can be larger than the item that would have to hold it, so this is
    // also the bound on the bytes a single substitution can carry into the store: the
    // check PRECEDES every clone of it.
    if attribute_value_size(value) > MAX_ITEM_SIZE_BYTES {
        return Err(RequestError::Validation(
            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
        ));
    }
    if value_members(value) > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members(name, value_members(value)));
    }
    Ok(())
}

/// The TOTAL members every collection inside `value` carries, summed — the stricter of
/// the two bounds a value could be held to, so neither one wide collection nor many
/// narrow ones escapes [`MAX_COLLECTION_MEMBERS`].
///
/// The walk is ITERATIVE, over an explicit stack, for the reason
/// [`attribute_value_size`] gives.
fn value_members(value: &AttributeValue) -> usize {
    let mut total = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        total = total.saturating_add(match value {
            AttributeValue::L(values) => {
                pending.extend(values.iter());
                values.len()
            }
            AttributeValue::M(entries) => {
                pending.extend(entries.values());
                entries.len()
            }
            other => set_len(other).unwrap_or(0),
        });
    }
    total
}

/// The members an update has written so far, plus the ones `value` carries — refused
/// past [`MAX_COLLECTION_MEMBERS`] in TOTAL across the update, so a per-value ceiling
/// cannot be multiplied by the action count. Counted before the value is written.
fn check_members_written(
    written: usize,
    name: &str,
    value: &AttributeValue,
) -> Result<usize, RequestError> {
    let total = written.saturating_add(value_members(value));
    if total > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members(name, total));
    }
    Ok(total)
}

/// Whether `item` has room for `value` at `path` — the clamp that PRECEDES the write,
/// so the item an update builds cannot grow past the documented 400 KB even while it is
/// being built, however many actions the update states.
fn check_room_for(
    item: &Item,
    path: &DocumentPath,
    value: &AttributeValue,
) -> Result<(), RequestError> {
    let replaced = path.get(item).map_or(0, attribute_value_size);
    let projected = item_size(item)
        .saturating_sub(replaced)
        .saturating_add(attribute_value_size(value));
    if projected > MAX_ITEM_SIZE_BYTES {
        return Err(RequestError::Validation(
            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// The refusal for a collection past [`MAX_COLLECTION_MEMBERS`], whether a request sent
/// it or an update's own `ADD`/`list_append` built it.
fn too_many_members(name: &str, members: usize) -> RequestError {
    RequestError::Validation(format!(
        "Attribute {name} holds {members} members, more than this emulator's maximum of \
         {MAX_COLLECTION_MEMBERS}"
    ))
}

/// A set member's canonical sort key, so that members compare and deduplicate by the
/// VALUE they denote: two `N` spellings of one number share one key.
fn canonical_number(text: &str) -> String {
    NumberKey::parse(text).map_or_else(|| text.to_string(), |number| number.canonical_text())
}

/// Whether two sets hold the same members. A DynamoDB set is unordered, so this is
/// member-wise rather than position-wise.
///
/// Both sides are SORTED by `key` rather than walked pairwise: the member count comes
/// from the request, so a pairwise walk would cost the product of two caller-chosen
/// counts, where sorting costs `n log n`.
fn same_members<'v, T: 'v, K: Ord>(
    left: &'v [T],
    right: &'v [T],
    key: impl Fn(&'v T) -> K,
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut mine: Vec<K> = left.iter().map(&key).collect();
    let mut theirs: Vec<K> = right.iter().map(key).collect();
    mine.sort_unstable();
    theirs.sort_unstable();
    mine == theirs
}

/// How two attribute values order, or `None` when DynamoDB documents no order between
/// them: "For type Number, value comparisons are numeric. String value comparisons for
/// greater than, equals, or less than are based on ASCII character code values. For
/// Binary, DynamoDB treats each byte of the binary data as unsigned when it compares
/// binary values" (the `AttributeValueList` doc) — and nothing for the other types, or
/// across types.
fn compare_values(left: &AttributeValue, right: &AttributeValue) -> Option<Ordering> {
    match (left, right) {
        (AttributeValue::S(mine), AttributeValue::S(theirs)) => Some(mine.cmp(theirs)),
        (AttributeValue::B(mine), AttributeValue::B(theirs)) => Some(mine.cmp(theirs)),
        (AttributeValue::N(mine), AttributeValue::N(theirs)) => {
            Some(NumberKey::parse(mine)?.cmp(&NumberKey::parse(theirs)?))
        }
        _ => None,
    }
}

/// `begins_with (path, operand)` — "true if the attribute specified by path begins
/// with a particular substring", which only a string or binary value can.
fn begins_with_value(value: &AttributeValue, prefix: &AttributeValue) -> bool {
    match (value, prefix) {
        (AttributeValue::S(text), AttributeValue::S(prefix)) => text.starts_with(prefix.as_str()),
        (AttributeValue::B(bytes), AttributeValue::B(prefix)) => bytes.starts_with(prefix),
        _ => false,
    }
}

/// `contains (path, operand)` — "true if the attribute specified by path is: A String
/// that contains a particular substring, [or] A Set that contains a particular
/// element". A list is searched for the element too, the way `CONTAINS` reads it.
fn contains_value(haystack: &AttributeValue, needle: &AttributeValue) -> bool {
    match (haystack, needle) {
        (AttributeValue::S(text), AttributeValue::S(part)) => text.contains(part.as_str()),
        (AttributeValue::Ss(members), AttributeValue::S(member)) => members.contains(member),
        (AttributeValue::Ns(members), AttributeValue::N(member)) => {
            members.iter().any(|stored| numbers_equal(stored, member))
        }
        (AttributeValue::Bs(members), AttributeValue::B(member)) => members.contains(member),
        (AttributeValue::L(members), needle) => members
            .iter()
            .any(|stored| attribute_values_equal(stored, needle)),
        _ => false,
    }
}

/// `size (path)` — the length of the value at a path: bytes for a string or binary
/// value, members for a set, elements for a list, entries for a map. Nothing else has
/// a documented size.
fn size_of_value(value: &AttributeValue) -> Option<i64> {
    Some(match value {
        AttributeValue::S(text) => byte_len(text.len()),
        AttributeValue::B(bytes) => byte_len(bytes.len()),
        AttributeValue::Ss(members) => byte_len(members.len()),
        AttributeValue::Ns(members) => byte_len(members.len()),
        AttributeValue::Bs(members) => byte_len(members.len()),
        AttributeValue::L(values) => byte_len(values.len()),
        AttributeValue::M(entries) => byte_len(entries.len()),
        AttributeValue::N(_) | AttributeValue::Bool(_) | AttributeValue::Null(_) => return None,
    })
}

/// Add or subtract two `N` values, reporting the canonical text of the result.
///
/// The arithmetic is exact decimal arithmetic (the `bigdecimal` crate), rounded to the
/// documented 38 significant digits, so `SET n = n + :v` and `ADD n :v` answer what
/// DynamoDB answers rather than what binary floating point would. The result is
/// rendered through [`canonical_number_text`], the same canonicalization a written
/// number goes through, so a computed number is refused by the rule it breaks and
/// stored in the one form every read reports.
pub fn number_arithmetic(
    left: &str,
    right: &str,
    subtract: bool,
) -> Result<String, RequestError> {
    let left_value = parse_decimal(left)?;
    let right_value = parse_decimal(right)?;
    let result = if subtract {
        left_value - right_value
    } else {
        left_value + right_value
    };
    let precision = u64::try_from(MAX_SIGNIFICANT_DIGITS).unwrap_or(38);
    let (digits, scale) = result.with_prec(precision).as_bigint_and_exponent();
    // `digits * 10^-scale` is the value, which is exactly what an `N` text spells.
    canonical_number_text(&format!("{digits}e{}", -scale))
}

/// One operand of that arithmetic. The canonical text is what reaches the decimal
/// parser, so only one spelling of a number ever does, and a text that denotes no
/// number this store can hold is refused by the rule it breaks.
fn parse_decimal(text: &str) -> Result<BigDecimal, RequestError> {
    let canonical = canonical_number_text(text)?;
    BigDecimal::from_str(&canonical)
        .map_err(|_| number_rejection(text, NumberFault::NotNumeric))
}

/// The members of a set value, as the member-wise set operations below walk them.
enum SetMembers<'a> {
    Strings(&'a [String]),
    Numbers(&'a [String]),
    Binaries(&'a [Vec<u8>]),
}

/// A value's set members, or `None` when it is not a set at all.
fn set_members(value: &AttributeValue) -> Option<SetMembers<'_>> {
    match value {
        AttributeValue::Ss(members) => Some(SetMembers::Strings(members)),
        AttributeValue::Ns(members) => Some(SetMembers::Numbers(members)),
        AttributeValue::Bs(members) => Some(SetMembers::Binaries(members)),
        _ => None,
    }
}

/// How many members a set holds, for the empty-set refusal.
fn set_len(value: &AttributeValue) -> Option<usize> {
    Some(match set_members(value)? {
        SetMembers::Strings(members) | SetMembers::Numbers(members) => members.len(),
        SetMembers::Binaries(members) => members.len(),
    })
}

/// The members a union would hold at most, refused past [`MAX_COLLECTION_MEMBERS`]
/// BEFORE the union is built: a repeated `ADD` must not grow one stored set past the
/// ceiling every value is admitted under.
fn check_union_members(stored: usize, incoming: usize) -> Result<(), RequestError> {
    let total = stored.saturating_add(incoming);
    if total > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members("the set being added to", total));
    }
    Ok(())
}

/// `ADD`'s set form: "If the existing data type is a set and if Value is also a set,
/// then Value is added to the existing set". Both sets must hold the same primitive
/// type, and a member the set already holds is not added twice.
///
/// Membership is tested against a SORTED index of the stored members rather than by
/// scanning them per incoming member: both counts come from the request, so a scan per
/// member would cost their product.
fn set_union(
    existing: &AttributeValue,
    addition: &AttributeValue,
) -> Result<AttributeValue, RequestError> {
    match (existing, addition) {
        (AttributeValue::Ss(stored), AttributeValue::Ss(incoming)) => {
            check_union_members(stored.len(), incoming.len())?;
            let mut held: BTreeSet<&str> = stored.iter().map(String::as_str).collect();
            let mut members = stored.clone();
            for member in incoming {
                if held.insert(member.as_str()) {
                    members.push(member.clone());
                }
            }
            Ok(AttributeValue::Ss(members))
        }
        (AttributeValue::Ns(stored), AttributeValue::Ns(incoming)) => {
            check_union_members(stored.len(), incoming.len())?;
            let mut held: BTreeSet<String> = stored.iter().map(|text| canonical_number(text)).collect();
            let mut members = stored.clone();
            for member in incoming {
                if held.insert(canonical_number(member)) {
                    members.push(member.clone());
                }
            }
            Ok(AttributeValue::Ns(members))
        }
        (AttributeValue::Bs(stored), AttributeValue::Bs(incoming)) => {
            check_union_members(stored.len(), incoming.len())?;
            let mut held: BTreeSet<&[u8]> = stored.iter().map(Vec::as_slice).collect();
            let mut members = stored.clone();
            for member in incoming {
                if held.insert(member.as_slice()) {
                    members.push(member.clone());
                }
            }
            Ok(AttributeValue::Bs(members))
        }
        (_held, _incoming) => Err(incorrect_update_operand_type()),
    }
}

/// `DELETE`'s set form: "those values are subtracted from the old set". An empty
/// result is no set at all, so the attribute goes away with its last member. The
/// members to remove are indexed first, for the reason [`set_union`] gives.
fn set_difference(
    existing: &AttributeValue,
    removal: &AttributeValue,
) -> Result<Option<AttributeValue>, RequestError> {
    let remaining = match (existing, removal) {
        (AttributeValue::Ss(stored), AttributeValue::Ss(removed)) => {
            let gone: BTreeSet<&str> = removed.iter().map(String::as_str).collect();
            let members: Vec<String> = stored
                .iter()
                .filter(|member| !gone.contains(member.as_str()))
                .cloned()
                .collect();
            (!members.is_empty()).then_some(AttributeValue::Ss(members))
        }
        (AttributeValue::Ns(stored), AttributeValue::Ns(removed)) => {
            let gone: BTreeSet<String> = removed.iter().map(|text| canonical_number(text)).collect();
            let members: Vec<String> = stored
                .iter()
                .filter(|member| !gone.contains(&canonical_number(member)))
                .cloned()
                .collect();
            (!members.is_empty()).then_some(AttributeValue::Ns(members))
        }
        (AttributeValue::Bs(stored), AttributeValue::Bs(removed)) => {
            let gone: BTreeSet<&[u8]> = removed.iter().map(Vec::as_slice).collect();
            let members: Vec<Vec<u8>> = stored
                .iter()
                .filter(|member| !gone.contains(member.as_slice()))
                .cloned()
                .collect();
            (!members.is_empty()).then_some(AttributeValue::Bs(members))
        }
        (_held, _removed) => return Err(incorrect_update_operand_type()),
    };
    Ok(remaining)
}

/// One operand of a condition: a document path, a literal value, or `size (path)`.
#[derive(Debug, Clone)]
enum Operand {
    Path(DocumentPath),
    Value(AttributeValue),
    Size(DocumentPath),
}

/// One node of a parsed `ConditionExpression`.
#[derive(Debug, Clone)]
enum ConditionNode {
    /// Every one of these must hold (`AND`).
    All(Vec<ConditionNode>),
    /// At least one of these must hold (`OR`).
    Any(Vec<ConditionNode>),
    Not(Box<ConditionNode>),
    Compare(Operand, CompareOp, Operand),
    Between(Operand, Operand, Operand),
    In(Operand, Vec<Operand>),
    /// `attribute_exists` when true, `attribute_not_exists` when false.
    Exists(DocumentPath, bool),
    AttributeType(DocumentPath, Operand),
    BeginsWith(Operand, Operand),
    Contains(Operand, Operand),
}

/// A `ConditionExpression`, parsed once and then evaluated against the stored item.
#[derive(Debug, Clone)]
pub struct ParsedCondition {
    node: ConditionNode,
}

impl ParsedCondition {
    /// Parse one `ConditionExpression`. The grammar is DynamoDB's own: comparisons,
    /// `BETWEEN`, `IN`, the documented functions, combined with `AND`, `OR` and `NOT`
    /// and grouped with parentheses.
    pub fn parse(
        member: &'static str,
        expression: &str,
        substitutions: Substitutions<'_>,
    ) -> Result<Self, RequestError> {
        let tokens = tokenize_expression(member, expression)?;
        let mut parser = ExprParser::new(member, &tokens, substitutions);
        let node = parser.disjunction()?;
        if !parser.at_end() {
            return Err(parser.syntax());
        }
        Ok(Self { node })
    }

    /// Whether the condition holds for `item` — the stored item, or `None` when the
    /// key holds no item, which is what `attribute_not_exists` is for.
    pub fn evaluate(&self, item: Option<&Item>) -> Result<bool, RequestError> {
        let absent = Item::new();
        evaluate_condition(&self.node, item.unwrap_or(&absent))
    }

    /// How this condition READS each top-level attribute it mentions: every attribute it
    /// reads, each with the kinds of read it makes of it.
    ///
    /// What it is for: a condition that may only speak about a named set of attributes, and
    /// only in certain ways — the search schema of a vector index, whose partitioning
    /// attribute may only be pinned by equality — is held to that from the parsed tree,
    /// rather than guessed from the expression's text.
    pub fn attribute_reads(&self) -> BTreeMap<String, BTreeSet<AttributeRead>> {
        let mut reads = BTreeMap::new();
        collect_condition_attributes(&self.node, &mut reads);
        reads
    }
}

/// How a condition reads one top-level attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttributeRead {
    /// Pinned to one value by the equality operator: `attr = <value>`, or the same
    /// comparison written the other way round.
    Equality,
    /// Read in any other way: another comparison operator, a range, a function, or a
    /// document path that reaches INSIDE the attribute.
    Other,
}

/// Collect how one condition node reads each top-level attribute into `reads`.
///
/// Recursion follows the parsed tree, whose depth the parser holds at
/// [`MAX_EXPRESSION_DEPTH`], so the number of frames is a constant of this service
/// rather than a function of the request.
fn collect_condition_attributes(
    node: &ConditionNode,
    reads: &mut BTreeMap<String, BTreeSet<AttributeRead>>,
) {
    // An equality between an attribute and a VALUE pins that attribute; every other
    // comparison, and an equality against another attribute, reads it some other way.
    if let ConditionNode::Compare(left, CompareOp::Equal, right) = node {
        if let Some((Operand::Path(path), Operand::Value(_))) = match (left, right) {
            (Operand::Path(_), Operand::Value(_)) => Some((left, right)),
            (Operand::Value(_), Operand::Path(_)) => Some((right, left)),
            _ => None,
        } {
            if let Some(attribute) = path.single_attribute() {
                reads
                    .entry(attribute.to_string())
                    .or_default()
                    .insert(AttributeRead::Equality);
                return;
            }
        }
    }
    match node {
        ConditionNode::All(nodes) | ConditionNode::Any(nodes) => {
            for node in nodes {
                collect_condition_attributes(node, reads);
            }
        }
        ConditionNode::Not(inner) => collect_condition_attributes(inner, reads),
        ConditionNode::Compare(left, _, right)
        | ConditionNode::BeginsWith(left, right)
        | ConditionNode::Contains(left, right) => {
            collect_operand_attributes(left, reads);
            collect_operand_attributes(right, reads);
        }
        ConditionNode::Between(value, low, high) => {
            collect_operand_attributes(value, reads);
            collect_operand_attributes(low, reads);
            collect_operand_attributes(high, reads);
        }
        ConditionNode::In(value, candidates) => {
            collect_operand_attributes(value, reads);
            for candidate in candidates {
                collect_operand_attributes(candidate, reads);
            }
        }
        ConditionNode::Exists(path, _) => {
            read_otherwise(path, reads);
        }
        ConditionNode::AttributeType(path, operand) => {
            read_otherwise(path, reads);
            collect_operand_attributes(operand, reads);
        }
    }
}

/// How one operand reads a top-level attribute, if it reads one at all: a literal value
/// reads none, and anything an operand does other than the equality above is `Other`.
fn collect_operand_attributes(
    operand: &Operand,
    reads: &mut BTreeMap<String, BTreeSet<AttributeRead>>,
) {
    match operand {
        Operand::Path(path) | Operand::Size(path) => read_otherwise(path, reads),
        Operand::Value(_) => {}
    }
}

/// Record that `path`'s top-level attribute is read some way other than being pinned.
fn read_otherwise(path: &DocumentPath, reads: &mut BTreeMap<String, BTreeSet<AttributeRead>>) {
    reads
        .entry(path.attribute().to_string())
        .or_default()
        .insert(AttributeRead::Other);
}

/// Evaluate one condition node.
///
/// Recursion follows the parsed tree, whose depth the parser holds at
/// [`MAX_EXPRESSION_DEPTH`]: `AND` and `OR` chains are flattened into one node each,
/// so a long expression widens the tree rather than deepening it.
fn evaluate_condition(node: &ConditionNode, item: &Item) -> Result<bool, RequestError> {
    match node {
        ConditionNode::All(nodes) => {
            for node in nodes {
                if !evaluate_condition(node, item)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ConditionNode::Any(nodes) => {
            for node in nodes {
                if evaluate_condition(node, item)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ConditionNode::Not(inner) => Ok(!evaluate_condition(inner, item)?),
        ConditionNode::Compare(left, operator, right) => {
            let left = resolve_operand(left, item);
            let right = resolve_operand(right, item);
            let (Some(left), Some(right)) = (left.as_ref(), right.as_ref()) else {
                // One side addresses nothing: the item carries no value there, and
                // nothing is NOT EQUAL to a value it does not hold, so the one
                // comparator an unheld operand satisfies is the not-equal one — the
                // attribute need not be present for it, while every other comparator
                // (equality included) needs both values to compare. With NEITHER side
                // held there is no value to be unequal to, so nothing holds.
                return Ok(matches!(operator, CompareOp::NotEqual)
                    && (left.is_some() || right.is_some()));
            };
            Ok(match operator {
                CompareOp::Equal => attribute_values_equal(left, right),
                CompareOp::NotEqual => !attribute_values_equal(left, right),
                CompareOp::Less => compare_values(left, right) == Some(Ordering::Less),
                CompareOp::LessOrEqual => {
                    matches!(
                        compare_values(left, right),
                        Some(Ordering::Less | Ordering::Equal)
                    )
                }
                CompareOp::Greater => compare_values(left, right) == Some(Ordering::Greater),
                CompareOp::GreaterOrEqual => matches!(
                    compare_values(left, right),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
            })
        }
        ConditionNode::Between(value, low, high) => {
            let (Some(value), Some(low), Some(high)) = (
                resolve_operand(value, item),
                resolve_operand(low, item),
                resolve_operand(high, item),
            ) else {
                return Ok(false);
            };
            Ok(matches!(
                compare_values(&value, &low),
                Some(Ordering::Greater | Ordering::Equal)
            ) && matches!(
                compare_values(&value, &high),
                Some(Ordering::Less | Ordering::Equal)
            ))
        }
        ConditionNode::In(value, candidates) => {
            let Some(value) = resolve_operand(value, item) else {
                return Ok(false);
            };
            Ok(candidates.iter().any(|candidate| {
                resolve_operand(candidate, item)
                    .is_some_and(|candidate| attribute_values_equal(&value, &candidate))
            }))
        }
        ConditionNode::Exists(path, expected) => Ok(path.get(item).is_some() == *expected),
        ConditionNode::AttributeType(path, operand) => {
            let Some(AttributeValue::S(wanted)) = resolve_operand(operand, item) else {
                return Err(RequestError::Validation(
                    "Invalid ConditionExpression: the second operand of attribute_type must be a \
                     string naming an attribute type"
                        .to_string(),
                ));
            };
            Ok(path
                .get(item)
                .is_some_and(|value| attribute_value_type(value) == wanted))
        }
        ConditionNode::BeginsWith(value, prefix) => {
            let (Some(value), Some(prefix)) =
                (resolve_operand(value, item), resolve_operand(prefix, item))
            else {
                return Ok(false);
            };
            Ok(begins_with_value(&value, &prefix))
        }
        ConditionNode::Contains(haystack, needle) => {
            let (Some(haystack), Some(needle)) = (
                resolve_operand(haystack, item),
                resolve_operand(needle, item),
            ) else {
                return Ok(false);
            };
            Ok(contains_value(&haystack, &needle))
        }
    }
}

/// The value one condition operand stands for, or `None` when it addresses nothing.
fn resolve_operand(operand: &Operand, item: &Item) -> Option<AttributeValue> {
    match operand {
        Operand::Value(value) => Some(value.clone()),
        Operand::Path(path) => path.get(item).cloned(),
        Operand::Size(path) => path
            .get(item)
            .and_then(size_of_value)
            .map(|size| AttributeValue::N(size.to_string())),
    }
}

/// One action of a parsed `UpdateExpression` (or of the legacy `AttributeUpdates`
/// map, which states the same four actions a request at a time).
#[derive(Debug, Clone)]
enum UpdateAction {
    Set(DocumentPath, SetValue),
    Remove(DocumentPath),
    Add(DocumentPath, AttributeValue),
    Delete(DocumentPath, AttributeValue),
}

impl UpdateAction {
    /// The path this action writes to.
    fn path(&self) -> &DocumentPath {
        match self {
            Self::Set(path, _) | Self::Remove(path) | Self::Add(path, _) | Self::Delete(path, _) => {
                path
            }
        }
    }
}

/// The right-hand side of one `SET`: one operand, or two joined by `+` or `-`.
#[derive(Debug, Clone)]
enum SetValue {
    Single(SetOperand),
    Sum(SetOperand, SetOperand),
    Difference(SetOperand, SetOperand),
}

/// One operand of a `SET`: a path, a literal value, or one of the two documented
/// functions.
#[derive(Debug, Clone)]
enum SetOperand {
    Path(DocumentPath),
    Value(AttributeValue),
    /// `if_not_exists (path, operand)` — "if the item does not contain an attribute at
    /// the specified path, then if_not_exists evaluates to operand; otherwise, it
    /// evaluates to path".
    IfNotExists(DocumentPath, Box<SetOperand>),
    /// `list_append (operand, operand)` — "evaluates to a list with a new element
    /// added to it".
    ListAppend(Box<SetOperand>, Box<SetOperand>),
}

/// The update one request states, from `UpdateExpression` or from the legacy
/// `AttributeUpdates` map, parsed once and then applied to the stored item.
#[derive(Debug, Clone)]
pub struct ParsedUpdate {
    /// The input member the update travelled in, which is what every refusal this
    /// update raises names.
    member: &'static str,
    actions: Vec<UpdateAction>,
}

impl ParsedUpdate {
    /// Parse one `UpdateExpression`: the `SET`, `REMOVE`, `ADD` and `DELETE` sections,
    /// each at most once, each a comma-separated list of actions.
    pub fn parse(
        member: &'static str,
        expression: &str,
        substitutions: Substitutions<'_>,
    ) -> Result<Self, RequestError> {
        let tokens = tokenize_expression(member, expression)?;
        let mut parser = ExprParser::new(member, &tokens, substitutions);
        let actions = parser.update_actions()?;
        Self::from_actions(member, actions)
    }

    /// The same update, as the legacy `AttributeUpdates` map states it: one action per
    /// top-level attribute, `PUT` by default (the `AttributeValueUpdate$Action` doc).
    ///
    /// The map's entry count is the request's to choose and this legacy member carries
    /// no modelled bound, so it is clamped BEFORE one action per entry is built.
    pub fn from_attribute_updates(
        updates: &HashMap<String, AttributeValueUpdate>,
    ) -> Result<Self, RequestError> {
        if updates.len() > MAX_UPDATE_ACTIONS {
            return Err(too_many_actions(updates.len()));
        }
        let mut actions = Vec::new();
        for (name, update) in updates {
            let path = DocumentPath::attribute_named(name);
            let action = update.action.as_deref().unwrap_or("PUT");
            match action {
                "PUT" => actions.push(UpdateAction::Set(
                    path,
                    SetValue::Single(SetOperand::Value(required_update_value(name, update)?)),
                )),
                // "DELETE - If no value is specified, the attribute and its value are
                // removed from the item", and with a set value those members are
                // subtracted from the stored set.
                "DELETE" => actions.push(match &update.value {
                    None => UpdateAction::Remove(path),
                    Some(value) => UpdateAction::Delete(path, value.clone()),
                }),
                "ADD" => {
                    actions.push(UpdateAction::Add(path, required_update_value(name, update)?));
                }
                other => {
                    return Err(RequestError::Validation(format!(
                        "One or more parameter values were invalid: Unknown AttributeUpdates \
                         action: {other} for attribute {name}"
                    )))
                }
            }
        }
        Self::from_actions("AttributeUpdates", actions)
    }

    /// Hold the actions, refusing two that write overlapping document locations:
    /// DynamoDB cannot apply both, so the request is wrong rather than one of them
    /// winning silently.
    ///
    /// The action count comes from the request, so it is CLAMPED here — before the
    /// overlap check, before [`Self::apply`] walks it and before any caller takes the
    /// store's lock — and the overlap check itself indexes the paths rather than
    /// comparing every pair: a path overlaps another exactly when one is a step-prefix
    /// of the other, so each path only has to look its own prefixes up. The index
    /// keeps each path's POSITION in the update, because that is what orders the two
    /// paths in the refusal ([`overlapping_paths`]).
    fn from_actions(
        member: &'static str,
        actions: Vec<UpdateAction>,
    ) -> Result<Self, RequestError> {
        if actions.len() > MAX_UPDATE_ACTIONS {
            return Err(too_many_actions(actions.len()));
        }
        let mut written: BTreeMap<String, usize> = BTreeMap::new();
        for (position, action) in actions.iter().enumerate() {
            let path = action.path();
            if written.insert(path.render(), position).is_some() {
                return Err(overlapping_paths(
                    member,
                    &path.render_tokens(),
                    &path.render_tokens(),
                ));
            }
        }
        for (position, action) in actions.iter().enumerate() {
            let path = action.path();
            for steps in 1..path.0.len() {
                let prefix = path.render_prefix(steps);
                if let Some(wrote_prefix) = written.get(&prefix) {
                    let whole = path.render_tokens();
                    let prefix = path.render_tokens_prefix(steps);
                    // The path stated EARLIER in the update is `path one`.
                    let (one, two) = if *wrote_prefix < position {
                        (prefix, whole)
                    } else {
                        (whole, prefix)
                    };
                    return Err(overlapping_paths(member, &one, &two));
                }
            }
        }
        Ok(Self { member, actions })
    }

    /// The update a request that states none makes: nothing at all. `UpdateItem`
    /// still "adds a new item to the table if it does not already exist", so such a
    /// request creates the item from its key alone.
    pub fn nothing() -> Self {
        Self {
            member: "UpdateExpression",
            actions: Vec::new(),
        }
    }

    /// Apply every action to `item`, reporting the top-level attributes that changed.
    ///
    /// Every operand reads the item as it stood BEFORE the update, and so does every
    /// document path: all the clauses of one update resolve against ONE pre-image, so
    /// a clause that shortens a list cannot move the location a later clause named.
    /// That is what orders the two phases below — the writes first (writing an
    /// existing location, and appending past the end of a list, leave every earlier
    /// index where the pre-image had it), then the removals greatest path first (so
    /// removing one location never shifts a smaller one that is still to be removed).
    ///
    /// A value an action COMPUTES goes through [`check_attribute_value`] before it is
    /// written, exactly as one a request sent does, and the item is clamped AS IT
    /// GROWS — the room check and the running member count both precede each write —
    /// so a many-action update cannot multiply a per-value bound into an unbounded item
    /// while it is being built, however many actions the expression states.
    pub fn apply(&self, item: &mut Item) -> Result<UpdateOutcome, RequestError> {
        let before = item.clone();
        let mut changed: Vec<DocumentPath> = Vec::new();
        let mut members_written = 0usize;
        let mut removals: Vec<&DocumentPath> = Vec::new();
        for action in &self.actions {
            let path = action.path();
            match action {
                UpdateAction::Set(path, value) => {
                    let value = evaluate_set_value(self.member, value, &before)?;
                    check_attribute_value(path.attribute(), &value)?;
                    members_written =
                        check_members_written(members_written, path.attribute(), &value)?;
                    check_room_for(item, path, &value)?;
                    path.set(item, value)?;
                    changed.push(path.clone());
                }
                UpdateAction::Remove(path) => removals.push(path),
                UpdateAction::Add(_, value) => {
                    let added = apply_add(path, value, &before)?;
                    check_attribute_value(path.attribute(), &added)?;
                    members_written =
                        check_members_written(members_written, path.attribute(), &added)?;
                    check_room_for(item, path, &added)?;
                    path.set(item, added)?;
                    changed.push(path.clone());
                }
                UpdateAction::Delete(_, value) => {
                    if apply_delete(path, value, &before, item)? {
                        changed.push(path.clone());
                    }
                }
            }
        }
        removals.sort_by(|one, two| two.0.cmp(&one.0));
        for path in removals {
            if path.remove(item).is_some() {
                changed.push(path.clone());
            }
        }
        Ok(UpdateOutcome { changed })
    }
}

/// What one update changed: every document path it wrote or removed.
///
/// Kept as PATHS rather than as the attribute names they reach into, because the two
/// things a caller asks about them are different questions: which of the item's
/// attributes the update touched (a key attribute among them refuses the whole update),
/// and which of the locations the expression NAMED hold a value afterwards (what the
/// `UPDATED_*` forms report).
#[derive(Debug, Clone, Default)]
pub struct UpdateOutcome {
    changed: Vec<DocumentPath>,
}

impl UpdateOutcome {
    /// The top-level attributes the update changed.
    pub fn attributes(&self) -> BTreeSet<String> {
        self.changed
            .iter()
            .map(|path| path.attribute().to_string())
            .collect()
    }

    /// The `Attributes` an `UPDATED_*` projection of `item` reports.
    ///
    /// "Returns only the updated attributes" is read PER DOCUMENT PATH, not per
    /// attribute: what is reported for each path the update changed is the value AT
    /// that path, wrapped in the containers the path steps through — `REMOVE l[1]`
    /// reports `l` as a one-element list holding whatever index 1 addresses now, not
    /// the whole stored list. A path that addresses nothing in `item` contributes
    /// nothing, which is what a `SET` past the end of a list leaves behind: the element
    /// is appended, so the index the expression named holds nothing afterwards.
    fn projected_paths(&self, item: &Item) -> Item {
        let mut reported = Item::new();
        // In DOCUMENT-PATH order, so the report does not depend on the order the
        // clauses happened to be written in: two changed indexes of one list are
        // reported in index order.
        let mut paths: Vec<&DocumentPath> = self.changed.iter().collect();
        paths.sort_by(|one, two| one.0.cmp(&two.0));
        for path in paths {
            let Some(value) = path.get(item) else { continue };
            let Some((PathStep::Attribute(name), rest)) = path.0.split_first() else {
                continue;
            };
            let built = path_skeleton(rest, value.clone());
            match reported.get_mut(name) {
                Some(slot) => merge_reported(slot, built),
                None => {
                    reported.insert(name.clone(), built);
                }
            }
        }
        reported
    }
}

/// `value`, wrapped in one container per remaining path step: a map entry for an
/// attribute step and a one-element list for an index step. An empty step list is the
/// value itself, which is what a path naming one top-level attribute reports.
fn path_skeleton(steps: &[PathStep], value: AttributeValue) -> AttributeValue {
    match steps.split_first() {
        None => value,
        Some((PathStep::Attribute(name), rest)) => {
            let mut entries = HashMap::new();
            entries.insert(name.clone(), path_skeleton(rest, value));
            AttributeValue::M(entries)
        }
        Some((PathStep::Index(_), rest)) => AttributeValue::L(vec![path_skeleton(rest, value)]),
    }
}

/// Fold one path's reported skeleton into another's under the same attribute: two
/// changed paths of one update may step through the same containers.
fn merge_reported(into: &mut AttributeValue, other: AttributeValue) {
    match (into, other) {
        (AttributeValue::M(mine), AttributeValue::M(theirs)) => {
            for (name, value) in theirs {
                match mine.get_mut(&name) {
                    Some(slot) => merge_reported(slot, value),
                    None => {
                        mine.insert(name, value);
                    }
                }
            }
        }
        (AttributeValue::L(mine), AttributeValue::L(theirs)) => mine.extend(theirs),
        // Two paths that write the same location are refused before this point
        // (`ParsedUpdate::from_actions`), so the only way one value replaces another
        // here is a container that is not one: the later value is what the item holds.
        (slot, other) => *slot = other,
    }
}

/// The value of an `AttributeValueUpdate` that needs one.
fn required_update_value(
    name: &str,
    update: &AttributeValueUpdate,
) -> Result<AttributeValue, RequestError> {
    update.value.clone().ok_or_else(|| {
        RequestError::Validation(format!(
            "One or more parameter values were invalid: Only DELETE action is allowed when no \
             attribute value is specified: {name}"
        ))
    })
}

/// What one `ADD` leaves at its path: "If the existing attribute is a number, and if
/// Value is also a number, then Value is mathematically added to the existing
/// attribute"; a set is unioned; and an attribute that is not there yet starts from
/// `0` for a number, or from the empty set, so the value itself is the result.
fn apply_add(
    path: &DocumentPath,
    value: &AttributeValue,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    if set_len(value) == Some(0) {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: An ADD action may not specify an empty \
             set: {}",
            path.render()
        )));
    }
    match path.get(before) {
        None => match value {
            AttributeValue::N(_) | AttributeValue::Ns(_) | AttributeValue::Ss(_)
            | AttributeValue::Bs(_) => Ok(value.clone()),
            _other => Err(incorrect_update_operand_type()),
        },
        Some(AttributeValue::N(held)) => match value {
            AttributeValue::N(increment) => Ok(AttributeValue::N(number_arithmetic(
                held, increment, false,
            )?)),
            _other => Err(incorrect_update_operand_type()),
        },
        Some(held) if set_members(held).is_some() => set_union(held, value),
        Some(_held) => Err(incorrect_update_operand_type()),
    }
}

/// Apply one `DELETE`, reporting whether it changed the item. Deleting from an
/// attribute that is not there is nothing to do, the way the legacy action's doc says:
/// "DELETE - Nothing happens; there is no attribute to delete".
fn apply_delete(
    path: &DocumentPath,
    value: &AttributeValue,
    before: &Item,
    item: &mut Item,
) -> Result<bool, RequestError> {
    if set_len(value) == Some(0) {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: A DELETE action may not specify an empty \
             set: {}",
            path.render()
        )));
    }
    let Some(held) = path.get(before) else {
        return Ok(false);
    };
    match set_difference(held, value)? {
        Some(remaining) => path.set(item, remaining)?,
        None => {
            path.remove(item);
        }
    }
    Ok(true)
}

/// The value one `SET` writes, evaluated against the item as it stood before the
/// update.
fn evaluate_set_value(
    member: &str,
    value: &SetValue,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    match value {
        SetValue::Single(operand) => evaluate_set_operand(member, operand, before),
        SetValue::Sum(left, right) | SetValue::Difference(left, right) => {
            let subtract = matches!(value, SetValue::Difference(_, _));
            let left = evaluate_set_operand(member, left, before)?;
            let right = evaluate_set_operand(member, right, before)?;
            match (&left, &right) {
                (AttributeValue::N(left), AttributeValue::N(right)) => Ok(AttributeValue::N(
                    number_arithmetic(left, right, subtract)?,
                )),
                // Arithmetic takes two numbers, so the operand that is not one is what
                // the refusal names — the left one when neither is.
                _ => {
                    let offending = if matches!(left, AttributeValue::N(_)) {
                        &right
                    } else {
                        &left
                    };
                    Err(incorrect_operand_type(
                        member,
                        if subtract { "-" } else { "+" },
                        attribute_value_type(offending),
                    ))
                }
            }
        }
    }
}

/// One `SET` operand's value. A path that addresses nothing is the caller's mistake —
/// there is no value to write — rather than a silent no-op.
fn evaluate_set_operand(
    member: &str,
    operand: &SetOperand,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    match operand {
        SetOperand::Value(value) => Ok(value.clone()),
        SetOperand::Path(path) => path
            .get(before)
            .cloned()
            .ok_or_else(|| RequestError::Validation(MISSING_OPERAND_ATTRIBUTE_MESSAGE.to_string())),
        SetOperand::IfNotExists(path, fallback) => match path.get(before) {
            Some(value) => Ok(value.clone()),
            None => evaluate_set_operand(member, fallback, before),
        },
        SetOperand::ListAppend(first, second) => {
            let first = evaluate_set_operand(member, first, before)?;
            let second = evaluate_set_operand(member, second, before)?;
            match (first, second) {
                (AttributeValue::L(mut head), AttributeValue::L(tail)) => {
                    // The concatenation is bounded BEFORE it is built, and the result
                    // is held to the documented item size: a `list_append` nested
                    // inside another would otherwise double its operand per level, and
                    // only the outermost result ever reaches the item-size check.
                    let total = head.len().saturating_add(tail.len());
                    if total > MAX_COLLECTION_MEMBERS {
                        return Err(too_many_members("the list being appended to", total));
                    }
                    head.extend(tail);
                    let appended = AttributeValue::L(head);
                    if attribute_value_size(&appended) > MAX_ITEM_SIZE_BYTES {
                        return Err(RequestError::Validation(
                            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
                        ));
                    }
                    Ok(appended)
                }
                // `list_append` concatenates two lists, so the operand that is not a
                // list is what the refusal names, whichever position it arrived in.
                (first, second) => {
                    let offending = if matches!(first, AttributeValue::L(_)) {
                        &second
                    } else {
                        &first
                    };
                    Err(incorrect_operand_type(
                        member,
                        "list_append",
                        attribute_value_type(offending),
                    ))
                }
            }
        }
    }
}

/// Recursive-descent parser over the tokens of one expression.
///
/// `depth` caps how far the rules that descend may: the nesting is caller-chosen, so
/// without the cap the number of stack frames would be too (and a stack overflow
/// aborts the process rather than failing one request).
struct ExprParser<'a> {
    member: &'static str,
    tokens: &'a [ExprToken],
    at: usize,
    depth: usize,
    substitutions: Substitutions<'a>,
}

impl<'a> ExprParser<'a> {
    fn new(member: &'static str, tokens: &'a [ExprToken], substitutions: Substitutions<'a>) -> Self {
        Self {
            member,
            tokens,
            at: 0,
            depth: 0,
            substitutions,
        }
    }

    fn syntax(&self) -> RequestError {
        expression_syntax_error_at(self.member, &self.stopped_token(), &self.near_text())
    }

    /// The token the parse stopped at, as the expression wrote it, or the end-of-input
    /// marker when it ran out of tokens.
    fn stopped_token(&self) -> String {
        self.tokens
            .get(self.at)
            .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), token_text)
    }

    /// The expression text around that token: the token before it, the token itself
    /// (there is none past the end) and the token after, as the expression wrote them.
    fn near_text(&self) -> String {
        token_window(self.tokens, self.at)
    }

    /// The attribute name an `#alias` stands for, with an undefined alias refused by
    /// the expression member it travelled in.
    fn alias_name(&self, alias: &str) -> Result<String, RequestError> {
        self.substitutions
            .name(alias)
            .map_err(|error| invalid_expression_member(self.member, error.message()))
    }

    /// The value a `:placeholder` stands for, refused the same way.
    fn placeholder_value(&self, placeholder: &str) -> Result<AttributeValue, RequestError> {
        self.substitutions
            .value(placeholder)
            .map_err(|error| invalid_expression_member(self.member, error.message()))
    }

    fn at_end(&self) -> bool {
        self.at >= self.tokens.len()
    }

    fn peek(&self) -> Option<&ExprToken> {
        self.tokens.get(self.at)
    }

    fn next_token(&mut self) -> Result<ExprToken, RequestError> {
        let token = self.peek().cloned().ok_or_else(|| self.syntax())?;
        self.at += 1;
        Ok(token)
    }

    /// Consume `token` if that is what comes next.
    fn eat(&mut self, token: &ExprToken) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &ExprToken) -> Result<(), RequestError> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.syntax())
        }
    }

    /// Consume a keyword if that is what comes next. The operator and section keywords
    /// are case-insensitive; the FUNCTION names are not ("These function names are
    /// case-sensitive"), so they are matched exactly, elsewhere.
    fn eat_keyword(&mut self, keyword: &str) -> bool {
        match self.peek() {
            Some(ExprToken::Word(word)) if word.eq_ignore_ascii_case(keyword) => {
                self.at += 1;
                true
            }
            _ => false,
        }
    }

    /// The name of the function call that starts here, if one does.
    fn peek_call(&self) -> Option<String> {
        match (self.peek(), self.tokens.get(self.at + 1)) {
            (Some(ExprToken::Word(word)), Some(ExprToken::Open)) => Some(word.clone()),
            _ => None,
        }
    }

    fn descend(&mut self) -> Result<(), RequestError> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(RequestError::Validation(format!(
                "Invalid {}: an expression may nest at most {MAX_EXPRESSION_DEPTH} levels deep",
                self.member
            )));
        }
        Ok(())
    }

    /// `path := (name | #alias) ('.' (name | #alias) | '[' index ']')*`
    fn path(&mut self) -> Result<DocumentPath, RequestError> {
        let mut steps = vec![PathStep::Attribute(self.path_name()?)];
        loop {
            if steps.len() >= MAX_ATTRIBUTE_DEPTH {
                return Err(RequestError::Validation(format!(
                    "Invalid {}: a document path may name at most {MAX_ATTRIBUTE_DEPTH} levels",
                    self.member
                )));
            }
            match self.peek().cloned() {
                Some(ExprToken::Dot) => {
                    self.at += 1;
                    steps.push(PathStep::Attribute(self.path_name()?));
                }
                Some(ExprToken::Index(index)) => {
                    self.at += 1;
                    steps.push(PathStep::Index(index));
                }
                _ => break,
            }
        }
        Ok(DocumentPath(steps))
    }

    /// One attribute name of a path, written out or behind an `#alias`.
    fn path_name(&mut self) -> Result<String, RequestError> {
        match self.next_token()? {
            ExprToken::Word(word) => {
                if is_reserved_word(&word) {
                    return Err(reserved_attribute_name(self.member, &word));
                }
                Ok(word)
            }
            ExprToken::Alias(alias) => self.alias_name(&alias),
            _ => Err(self.syntax()),
        }
    }

    /// `disjunction := conjunction (OR conjunction)*`
    fn disjunction(&mut self) -> Result<ConditionNode, RequestError> {
        let mut nodes = vec![self.conjunction()?];
        while self.eat_keyword("OR") {
            nodes.push(self.conjunction()?);
        }
        Ok(if nodes.len() == 1 {
            nodes.remove(0)
        } else {
            ConditionNode::Any(nodes)
        })
    }

    /// `conjunction := negation (AND negation)*`
    fn conjunction(&mut self) -> Result<ConditionNode, RequestError> {
        let mut nodes = vec![self.negation()?];
        while self.eat_keyword("AND") {
            nodes.push(self.negation()?);
        }
        Ok(if nodes.len() == 1 {
            nodes.remove(0)
        } else {
            ConditionNode::All(nodes)
        })
    }

    /// `negation := NOT negation | primary`
    fn negation(&mut self) -> Result<ConditionNode, RequestError> {
        if self.eat_keyword("NOT") {
            self.descend()?;
            let inner = self.negation()?;
            self.depth -= 1;
            return Ok(ConditionNode::Not(Box::new(inner)));
        }
        self.condition_primary()
    }

    /// `primary := '(' disjunction ')' | function | comparison`
    fn condition_primary(&mut self) -> Result<ConditionNode, RequestError> {
        if self.eat(&ExprToken::Open) {
            self.descend()?;
            let node = self.disjunction()?;
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(node);
        }
        // `size (path)` is an OPERAND rather than a condition of its own, so it falls
        // through to the comparison below.
        if let Some(name) = self.peek_call().filter(|name| name != "size") {
            return self.condition_function(&name);
        }
        let left = self.operand()?;
        if self.eat_keyword("BETWEEN") {
            let low = self.operand()?;
            if !self.eat_keyword("AND") {
                return Err(self.syntax());
            }
            let high = self.operand()?;
            return Ok(ConditionNode::Between(left, low, high));
        }
        if self.eat_keyword("IN") {
            self.expect(&ExprToken::Open)?;
            let mut candidates = vec![self.operand()?];
            while self.eat(&ExprToken::Comma) {
                candidates.push(self.operand()?);
            }
            self.expect(&ExprToken::Close)?;
            return Ok(ConditionNode::In(left, candidates));
        }
        match self.next_token()? {
            ExprToken::Compare(operator) => {
                let right = self.operand()?;
                Ok(ConditionNode::Compare(left, operator, right))
            }
            _ => Err(self.syntax()),
        }
    }

    /// One of the documented condition functions: `attribute_exists`,
    /// `attribute_not_exists`, `attribute_type`, `begins_with` and `contains`.
    fn condition_function(&mut self, name: &str) -> Result<ConditionNode, RequestError> {
        self.at += 2;
        let node = match name {
            "attribute_exists" => ConditionNode::Exists(self.path()?, true),
            "attribute_not_exists" => ConditionNode::Exists(self.path()?, false),
            "attribute_type" => {
                let path = self.path()?;
                self.expect(&ExprToken::Comma)?;
                ConditionNode::AttributeType(path, self.operand()?)
            }
            "begins_with" => {
                let value = self.operand()?;
                self.expect(&ExprToken::Comma)?;
                ConditionNode::BeginsWith(value, self.operand()?)
            }
            "contains" => {
                let haystack = self.operand()?;
                self.expect(&ExprToken::Comma)?;
                ConditionNode::Contains(haystack, self.operand()?)
            }
            other => return Err(unknown_function(self.member, other)),
        };
        self.expect(&ExprToken::Close)?;
        Ok(node)
    }

    /// `operand := :value | size '(' path ')' | path`
    fn operand(&mut self) -> Result<Operand, RequestError> {
        if let Some(ExprToken::Placeholder(placeholder)) = self.peek().cloned() {
            self.at += 1;
            return Ok(Operand::Value(self.placeholder_value(&placeholder)?));
        }
        if self.peek_call().as_deref() == Some("size") {
            self.at += 2;
            let path = self.path()?;
            self.expect(&ExprToken::Close)?;
            return Ok(Operand::Size(path));
        }
        match self.peek() {
            Some(ExprToken::Word(_) | ExprToken::Alias(_)) => Ok(Operand::Path(self.path()?)),
            _ => Err(self.syntax()),
        }
    }

    /// `update := (SET sets | REMOVE removes | ADD adds | DELETE deletes)+`, each
    /// section at most once.
    fn update_actions(&mut self) -> Result<Vec<UpdateAction>, RequestError> {
        const SECTIONS: [&str; 4] = ["SET", "REMOVE", "ADD", "DELETE"];
        let mut actions = Vec::new();
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        while !self.at_end() {
            let section = SECTIONS
                .iter()
                .find(|section| self.eat_keyword(section))
                .copied()
                .ok_or_else(|| self.syntax())?;
            if !seen.insert(section) {
                return Err(RequestError::Validation(format!(
                    "Invalid {}: the {section} section can only be used once in an expression",
                    self.member
                )));
            }
            loop {
                let path = self.path()?;
                actions.push(match section {
                    "SET" => {
                        self.expect(&ExprToken::Compare(CompareOp::Equal))?;
                        UpdateAction::Set(path, self.set_value()?)
                    }
                    "REMOVE" => UpdateAction::Remove(path),
                    "ADD" => UpdateAction::Add(path, self.action_value()?),
                    _ => UpdateAction::Delete(path, self.action_value()?),
                });
                if !self.eat(&ExprToken::Comma) {
                    break;
                }
            }
        }
        if actions.is_empty() {
            return Err(self.syntax());
        }
        Ok(actions)
    }

    /// The value operand `ADD` and `DELETE` take, which is a `:placeholder` — both
    /// actions take a value, not another of the item's attributes.
    fn action_value(&mut self) -> Result<AttributeValue, RequestError> {
        match self.next_token()? {
            ExprToken::Placeholder(placeholder) => self.placeholder_value(&placeholder),
            _ => Err(self.syntax()),
        }
    }

    /// `set_value := set_operand (('+' | '-') set_operand)?`
    fn set_value(&mut self) -> Result<SetValue, RequestError> {
        let left = self.set_operand()?;
        if self.eat(&ExprToken::Plus) {
            return Ok(SetValue::Sum(left, self.set_operand()?));
        }
        if self.eat(&ExprToken::Minus) {
            return Ok(SetValue::Difference(left, self.set_operand()?));
        }
        Ok(SetValue::Single(left))
    }

    /// `set_operand := :value | if_not_exists '(' path ',' set_operand ')'
    ///              | list_append '(' set_operand ',' set_operand ')' | path`
    fn set_operand(&mut self) -> Result<SetOperand, RequestError> {
        if let Some(name) = self.peek_call() {
            self.at += 2;
            self.descend()?;
            let operand = match name.as_str() {
                "if_not_exists" => {
                    let path = self.path()?;
                    self.expect(&ExprToken::Comma)?;
                    SetOperand::IfNotExists(path, Box::new(self.set_operand()?))
                }
                "list_append" => {
                    let first = self.set_operand()?;
                    self.expect(&ExprToken::Comma)?;
                    SetOperand::ListAppend(Box::new(first), Box::new(self.set_operand()?))
                }
                other => return Err(unknown_function(self.member, other)),
            };
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(operand);
        }
        if let Some(ExprToken::Placeholder(placeholder)) = self.peek().cloned() {
            self.at += 1;
            return Ok(SetOperand::Value(self.placeholder_value(&placeholder)?));
        }
        match self.peek() {
            Some(ExprToken::Word(_) | ExprToken::Alias(_)) => Ok(SetOperand::Path(self.path()?)),
            _ => Err(self.syntax()),
        }
    }
}

/// The refusal for a request that states one thing twice: through an expression member
/// and through the legacy member that expression member replaces ("This is a legacy
/// parameter. Use ConditionExpression instead"). The message names both members, the
/// way the service's own refusal does, so the caller knows which pair to drop.
///
/// Stated on its own rather than inside the `<N> validation error(s) detected: `
/// envelope: it is not a constraint on one member's value but a contradiction between
/// two of them, and AWS states it as its own sentence.
pub fn expression_conflict(legacy: &str, expression: &str) -> RequestError {
    RequestError::Validation(format!(
        "Can not use both expression and non-expression parameters in the same request: \
         Non-expression parameters: {{{legacy}}} Expression parameters: {{{expression}}}"
    ))
}

/// Whether the condition a conditional write states is satisfied by the stored item.
///
/// One entry point for all three single-item writes: each states its condition the
/// same two ways — the `ConditionExpression` language, or the legacy `Expected` map
/// combined by `ConditionalOperator` — and they are incompatible with each other
/// ("Use ConditionExpression instead"; a request may not use both).
pub fn write_condition_satisfied(
    condition_expression: Option<&str>,
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
    conditional_operator: Option<&str>,
    substitutions: Substitutions<'_>,
    item: Option<&Item>,
) -> Result<bool, RequestError> {
    let expected = expected.filter(|map| !map.is_empty());
    match (condition_expression, expected) {
        (Some(_), Some(_)) => Err(expression_conflict("Expected", "ConditionExpression")),
        (Some(expression), None) => {
            if conditional_operator.is_some() {
                return Err(expression_conflict(
                    "ConditionalOperator",
                    "ConditionExpression",
                ));
            }
            ParsedCondition::parse("ConditionExpression", expression, substitutions)?.evaluate(item)
        }
        (None, Some(expected)) => expected_satisfied(expected, conditional_operator, item),
        (None, None) => {
            if conditional_operator.is_some() {
                return Err(RequestError::Validation(
                    "ConditionalOperator can only be used together with Expected".to_string(),
                ));
            }
            Ok(true)
        }
    }
}

/// How the entries of a legacy condition map are combined: "AND - If all of the
/// conditions evaluate to true ... OR - If at least one of the conditions evaluate to
/// true" (`ConditionalOperator`), with `AND` the default — answering whether ANY entry
/// holding is enough.
///
/// One reading for every legacy surface that carries the member (`Expected`,
/// `ScanFilter`), so a value outside the model's enum is refused in one wording.
pub fn conditional_operator_any(conditional_operator: Option<&str>) -> Result<bool, RequestError> {
    match conditional_operator {
        None | Some("AND") => Ok(false),
        Some("OR") => Ok(true),
        Some(other) => Err(RequestError::Validation(format!(
            "ConditionalOperator must be AND or OR, got {other}"
        ))),
    }
}

/// Whether the legacy `Expected` map holds for the stored item.
fn expected_satisfied(
    expected: &HashMap<String, ExpectedAttributeValue>,
    conditional_operator: Option<&str>,
    item: Option<&Item>,
) -> Result<bool, RequestError> {
    let any = conditional_operator_any(conditional_operator)?;
    let absent = Item::new();
    let item = item.unwrap_or(&absent);
    let mut satisfied = !any;
    for (name, condition) in expected {
        let held = item.get(name);
        let holds = expected_entry_satisfied(name, condition, held)?;
        if any {
            satisfied |= holds;
        } else {
            satisfied &= holds;
        }
    }
    Ok(satisfied)
}

/// One entry of the legacy `Expected` map.
///
/// "Value and Exists are incompatible with AttributeValueList and ComparisonOperator.
/// Note that if you use both sets of parameters at once, DynamoDB will return a
/// ValidationException"; with neither set, there is nothing to check.
fn expected_entry_satisfied(
    name: &str,
    condition: &ExpectedAttributeValue,
    held: Option<&AttributeValue>,
) -> Result<bool, RequestError> {
    let comparison = condition.comparison_operator.as_deref();
    let operands = condition
        .attribute_value_list
        .as_deref()
        .filter(|values| !values.is_empty());
    if (comparison.is_some() || operands.is_some())
        && (condition.value.is_some() || condition.exists.is_some())
    {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: Value and Exists can not be used with \
             AttributeValueList and ComparisonOperator for attribute {name}"
        )));
    }
    if let Some(operator) = comparison {
        return legacy_comparison_matches(operator, held, operands.unwrap_or(&[]));
    }
    match (condition.value.as_ref(), condition.exists) {
        // "If you supply a Value all by itself, DynamoDB assumes the attribute exists"
        // — the attribute must be there AND match.
        (Some(value), None | Some(true)) => {
            Ok(held.is_some_and(|held| attribute_values_equal(held, value)))
        }
        // "Exists is false but you also provide a Value" is refused, as is an `Exists`
        // of true with nothing to check.
        (Some(_), Some(false)) => Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: Value cannot be used when Exists is false \
             for attribute {name}"
        ))),
        (None, Some(true)) => Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: Value must also be used when Exists is \
             true for attribute {name}"
        ))),
        (None, Some(false)) => Ok(held.is_none()),
        (None, None) => Ok(true),
    }
}

/// Whether one legacy `ComparisonOperator` holds between the stored attribute and the
/// request's `AttributeValueList`, as the `ComparisonOperator` doc defines each one.
pub fn legacy_comparison_matches(
    operator: &str,
    held: Option<&AttributeValue>,
    operands: &[AttributeValue],
) -> Result<bool, RequestError> {
    check_legacy_comparison(operator, operands)?;
    Ok(match operator {
        // "NOT_NULL : the attribute exists"; "NULL : the attribute does not exist".
        "NOT_NULL" => held.is_some(),
        "NULL" => held.is_none(),
        _ => {
            let Some(held) = held else {
                // Every other operator compares a value, and the item holds none here.
                // The legacy operators lower onto the expression language one for one
                // ("Use FilterExpression instead": `NE` is `<>` and `NOT_CONTAINS` is
                // `NOT contains(...)`), so the NEGATED ones hold for an attribute that
                // is not there — nothing is not equal to, and does not contain, a value
                // — while a positive comparison has nothing to compare and fails.
                return Ok(matches!(operator, "NE" | "NOT_CONTAINS"));
            };
            match operator {
                "EQ" => attribute_values_equal(held, &operands[0]),
                "NE" => !attribute_values_equal(held, &operands[0]),
                "LT" => compare_values(held, &operands[0]) == Some(Ordering::Less),
                "LE" => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Less | Ordering::Equal)
                ),
                "GT" => compare_values(held, &operands[0]) == Some(Ordering::Greater),
                "GE" => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
                "CONTAINS" => contains_value(held, &operands[0]),
                "NOT_CONTAINS" => !contains_value(held, &operands[0]),
                "BEGINS_WITH" => begins_with_value(held, &operands[0]),
                "IN" => operands
                    .iter()
                    .any(|candidate| attribute_values_equal(held, candidate)),
                _ => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Greater | Ordering::Equal)
                ) && matches!(
                    compare_values(held, &operands[1]),
                    Some(Ordering::Less | Ordering::Equal)
                ),
            }
        }
    })
}

/// Whether one legacy comparison is one the service serves at all, and whether it was
/// given the operands its operator takes — the facts of the REQUEST, so a reader that
/// evaluates a legacy condition map against many items settles them once, before it
/// reads anything.
///
/// The operand count is part of each operator's definition in the `ComparisonOperator`
/// doc, so a request that gives the wrong number of them is refused rather than guessed
/// at.
pub fn check_legacy_comparison(
    operator: &str,
    operands: &[AttributeValue],
) -> Result<(), RequestError> {
    let required = match operator {
        "NULL" | "NOT_NULL" => 0,
        "BETWEEN" => 2,
        "IN" => usize::MAX,
        "EQ" | "NE" | "LE" | "LT" | "GE" | "GT" | "CONTAINS" | "NOT_CONTAINS" | "BEGINS_WITH" => 1,
        other => {
            return Err(RequestError::Validation(format!(
                "ComparisonOperator must be one of EQ, NE, IN, LE, LT, GE, GT, BETWEEN, NOT_NULL, \
                 NULL, CONTAINS, NOT_CONTAINS or BEGINS_WITH, got {other}"
            )))
        }
    };
    let count_ok = if required == usize::MAX {
        !operands.is_empty()
    } else {
        operands.len() == required
    };
    if !count_ok {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: {operator} takes {} value(s) in \
             AttributeValueList, got {}",
            if required == usize::MAX {
                "one or more".to_string()
            } else {
                required.to_string()
            },
            operands.len()
        )));
    }
    // The one operator whose operand count the request chooses, held to the documented
    // ceiling on that list ([`MAX_IN_OPERANDS`]).
    if required == usize::MAX && operands.len() > MAX_IN_OPERANDS {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: {operator} takes at most \
             {MAX_IN_OPERANDS} value(s) in AttributeValueList, got {}",
            operands.len()
        )));
    }
    Ok(())
}

/// Hold a legacy condition MAP to what one request may ask the service to evaluate for
/// every item it reads, and settle every fact of the request its entries carry: how many
/// entries there are ([`MAX_LEGACY_CONDITIONS`]), whether each names a comparison this
/// service serves with the operands that comparison takes
/// ([`check_legacy_comparison`]), and whether the `ConditionalOperator` combining them is
/// one the model declares.
///
/// Settled ONCE, before anything is read, so the work the evaluator below does per item
/// is a function of this service's constants rather than of the request body.
pub fn check_legacy_conditions(
    conditions: &HashMap<String, Condition>,
    conditional_operator: Option<&str>,
) -> Result<(), RequestError> {
    if conditions.len() > MAX_LEGACY_CONDITIONS {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: a filter may state at most \
             {MAX_LEGACY_CONDITIONS} conditions, got {}",
            conditions.len()
        )));
    }
    for condition in conditions.values() {
        check_legacy_comparison(
            &condition.comparison_operator,
            legacy_operands(condition),
        )?;
    }
    conditional_operator_any(conditional_operator)?;
    Ok(())
}

/// The operands one entry of a legacy condition map compares against.
pub fn legacy_operands(condition: &Condition) -> &[AttributeValue] {
    condition
        .attribute_value_list
        .as_deref()
        .unwrap_or_default()
}

/// Whether a legacy CONDITION MAP — the pre-expression filter surface (`ScanFilter`,
/// `QueryFilter`) — selects `item`, its entries combined by `ConditionalOperator`.
///
/// Each entry is read by the same comparison the legacy conditional writes are read by
/// ([`legacy_comparison_matches`]), which is what makes a `NOT_NULL` or `NULL` entry
/// select exactly the items `attribute_exists` and `attribute_not_exists` select: both
/// forms ask one question of one attribute, through one evaluator.
///
/// The walk stops as soon as the answer is settled — the first entry that fails an `AND`
/// or holds for an `OR` — so what one item costs is bounded by the first decisive entry
/// rather than by the whole map. Whether an entry is one this service can evaluate at all
/// is settled before any item is read ([`check_legacy_conditions`]), so stopping early
/// cannot hide a refusal the request had coming.
pub fn legacy_conditions_match(
    conditions: &HashMap<String, Condition>,
    conditional_operator: Option<&str>,
    item: &Item,
) -> Result<bool, RequestError> {
    let any = conditional_operator_any(conditional_operator)?;
    for (name, condition) in conditions {
        let holds = legacy_comparison_matches(
            &condition.comparison_operator,
            item.get(name),
            legacy_operands(condition),
        )?;
        if holds == any {
            return Ok(any);
        }
    }
    Ok(!any)
}

/// The `Attributes` a write reports, by the `ReturnValues` the request asked for.
///
/// `before` is the item as it stood immediately before the write and `after` as it
/// stands immediately after it (`None` when the write removed it); `changed` is what the
/// write changed. An empty projection is reported as no `Attributes` member at all, the
/// way a write that replaced nothing reports none — which is also the answer for an
/// `UPDATED_*` projection whose every changed path addresses nothing in the item being
/// projected.
pub fn projected_return_values(
    return_values: Option<&str>,
    before: Option<&Item>,
    after: Option<&Item>,
    changed: &UpdateOutcome,
) -> Option<Item> {
    let projected = match return_values {
        Some("ALL_OLD") => before.cloned(),
        Some("ALL_NEW") => after.cloned(),
        Some("UPDATED_OLD") => before.map(|item| changed.projected_paths(item)),
        Some("UPDATED_NEW") => after.map(|item| changed.projected_paths(item)),
        _ => None,
    };
    projected.filter(|item| !item.is_empty())
}

/// Just the named attributes of an item.
///
/// The same rule narrows any read to the attributes a request named — a projection over
/// an index entry, say — so a name the item does not carry reports nothing for it rather
/// than a null.
pub fn project_attributes(item: &Item, names: &BTreeSet<String>) -> Item {
    names
        .iter()
        .filter_map(|name| {
            item.get_key_value(name)
                .map(|(name, value)| (name.clone(), value.clone()))
        })
        .collect()
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

    /// Whether this policy EXPLICITLY DENIES `action` on `resource_arn` to a caller any
    /// of `principals` names, and whether the statement that denied it carried a
    /// `Condition` the emulator did not evaluate.
    ///
    /// Only a Deny decides anything here: "an explicit deny in any policy overrides any
    /// allow" (the IAM policy evaluation logic), so a Deny that matches the request is the
    /// one outcome this half of the decision reaches. `principals` is every spelling the
    /// caller may be named by — its own ARN, the ARN of the role an assumed-role session
    /// is a session of, and the spellings that name its whole account, since a statement
    /// denying the account denies each of its principals. What GRANTS is
    /// [`Self::grants_any`], which is decided against the caller's own spellings alone.
    pub fn denies_any(
        &self,
        principals: &[String],
        action: &str,
        resource_arn: &str,
    ) -> Option<bool> {
        self.statements
            .iter()
            .find(|statement| statement.denies(principals, action, resource_arn))
            .map(PolicyStatement::conditioned)
    }

    /// Whether this policy GRANTS `action` on `resource_arn` to a caller any of
    /// `principals` names.
    ///
    /// A resource-based policy's `Allow` is a permission of its own where it names the
    /// CALLER: "the resource-based policy and the identity-based policy are evaluated
    /// together, and access is granted if either allows it". Only the caller's OWN
    /// spellings may be passed — a statement naming the account delegates to that
    /// account's identity-based policies rather than granting (see [`authorization`]).
    pub fn grants_any(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.statements
            .iter()
            .any(|statement| statement.grants(principals, action, resource_arn))
    }
}

/// The fixed opening of every `PolicyNotFoundException` message, whichever
/// resource-policy operation raises it and whichever of the two situations the
/// modelled error covers ("tried to access a nonexistent resource-based policy" and
/// "a policy is present for the resource but its revision ID didn't match the
/// expected value"). What follows it is the detail built from the request and from
/// this store: see [`policy_not_found_message`] and
/// [`policy_revision_mismatch_message`].
pub const POLICY_NOT_FOUND_PREFIX: &str =
    "Resource-based policy not found for the provided ResourceArn: ";

/// The refusal of an UNCONDITIONED request for a policy that is not there: the fixed
/// prefix and the `ResourceArn` the request sent, spelled as the request spelled it.
pub fn policy_not_found_message(resource_arn: &str) -> String {
    format!("{POLICY_NOT_FOUND_PREFIX}{resource_arn}")
}

/// The refusal of a request whose `ExpectedRevisionId` the target does not satisfy:
/// the fixed prefix, the revision id the request asked for, and what the target
/// actually carries — the revision id of the policy attached to it, or that it
/// carries no policy at all. The two tails are what lets a client doing optimistic
/// concurrency tell a stale revision apart from a policy that is gone.
pub fn policy_revision_mismatch_message(
    expected_revision_id: &str,
    table: &str,
    attached_revision_id: Option<&str>,
) -> String {
    match attached_revision_id {
        Some(current) => format!(
            "{POLICY_NOT_FOUND_PREFIX}Requested update for policy with revision id \
             {expected_revision_id}, but the policy associated to target table {table} has \
             revision id {current}."
        ),
        None => format!(
            "{POLICY_NOT_FOUND_PREFIX}Requested update for policy with revision id \
             {expected_revision_id}, but table {table} has no associated policy."
        ),
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
    /// from applying. `principals` is every spelling the caller may be named by; a
    /// statement naming any of them is about this request.
    fn denies(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.deny && self.about(principals, action, resource_arn)
    }

    /// Whether this statement GRANTS the request.
    ///
    /// A conditioned `Allow` grants nothing: the condition is not evaluated (see the
    /// field's doc), so the same fail-closed reading that applies a conditioned Deny
    /// withholds a conditioned grant — the emulator never serves on a condition it did
    /// not check.
    fn grants(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        !self.deny && !self.conditioned && self.about(principals, action, resource_arn)
    }

    /// Whether this statement is ABOUT the request at all: its principal, action and
    /// resource elements each select it.
    fn about(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.principal
            .matches(|value| principal_names(value, principals))
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

/// The identity every request to this emulator carries when nothing more is known about
/// it: the root of the single account it serves.
///
/// What the POLICY-MANAGEMENT lock-out is decided against ([`locks_the_caller_out`]),
/// which asks whether a document would take the ACCOUNT's own ability to change that
/// policy away. Who a DATA-PLANE request speaks for is resolved from the credential it
/// signed with instead ([`authorization::request_caller`]).
pub fn caller_principal() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// Whether a policy `Principal` value names the identity this emulator serves.
///
/// `principals` is every spelling the caller may be named by — its own ARN, the role
/// behind an assumed-role session, and (where a DENY is being decided) the ARN and bare
/// id of its account, because a statement about the account is about each of its
/// principals. A value matches when it names any of them, with IAM's wildcards.
///
/// A principal of a kind that names no IAM identity — a `Service`, a `Federated` or a
/// `CanonicalUser` — is kept WITH its kind by [`principal_values`], so it matches no
/// spelling here rather than being read as one.
fn principal_names(value: &str, principals: &[String]) -> bool {
    principals
        .iter()
        .any(|named| wildcard_match(value, named))
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

/// The actions that let a caller CHANGE a resource's policy again.
///
/// A policy that denies the caller any of them removes "your permissions to change the
/// policy of this resource in the future"
/// (`PutResourcePolicy$ConfirmRemoveSelfResourceAccess`), which is what that member has
/// to be set to `true` to accept.
const POLICY_MANAGEMENT_ACTIONS: [&str; 3] = [
    "dynamodb:PutResourcePolicy",
    "dynamodb:DeleteResourcePolicy",
    "dynamodb:GetResourcePolicy",
];

/// Whether `policy`, attached to the table at `resource_arn`, would deny THIS CALLER the
/// ability to change that policy again.
///
/// Checked against the policy the request is attaching rather than the one the table
/// carries: the lock-out is what the NEW document would do, and it is decided before
/// anything is stored so a refused request leaves the table exactly as it was.
///
/// Every spelling a statement may name this account by is compared — the root user's ARN
/// and the bare account id that stands for it ([`::service_common::iam_authorization::account_principals`]) —
/// because a statement that denies the ACCOUNT denies each of its principals, so a
/// document that names the account by its id takes the same access away as one that names
/// its root.
pub fn locks_the_caller_out(policy: &AttachedPolicy, resource_arn: &str) -> bool {
    let caller = caller_principal();
    let mut named = vec![caller.clone()];
    named.extend(::service_common::iam_authorization::account_principals(
        &caller,
    ));
    POLICY_MANAGEMENT_ACTIONS
        .iter()
        .any(|action| policy.denies_any(&named, action, resource_arn).is_some())
}

/// The `AccessDeniedException` message a data-plane request earns when the policies in
/// force do not permit it, or `None` when they do.
///
/// `caller` is who the request speaks for ([`authorization::request_caller`], resolved
/// before this store was locked) and `action` is the IAM action the operation performs
/// (`dynamodb:GetItem`). The decision itself is [`authorization::denial`]'s, which is the
/// one place this service makes it.
pub fn policy_denial(
    caller: &authorization::Caller,
    table: &Table,
    action: &str,
    now: SystemTime,
) -> Option<String> {
    authorization::denial(caller, table, action, now)
}

/// Who a data-plane request speaks for, and whether the policies in force let it act on
/// the table it addresses, in its own file: it is about the CALLER and the documents of
/// other services, while this module is the table and item store itself. Re-exported
/// below, so every caller still names it `crate::state::…`.
pub mod authorization;

pub use authorization::request_caller;

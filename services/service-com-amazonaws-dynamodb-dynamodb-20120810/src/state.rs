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
/// An exponent is read with `parse::<i32>`, which accepts any run of leading zeros, so a
/// bound on the exponent's VALUE (the documented range, enforced below as an over- or
/// underflow) bounds no amount of exponent TEXT. An exponent spelled wider than an `i32`
/// denotes no number this service can convert — the same fault as an exponent that is
/// not digits at all — and bounding the text keeps what one value retains bounded, which
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
                // An exponent spelled wider than the integer it is read into denotes a
                // magnitude far outside the documented range whatever the mantissa
                // shifts it by — so the TEXT is no number this service can convert,
                // which is the same fault `parse` reports for an exponent that is not
                // digits at all. Bounded before it is parsed, because `parse::<i32>`
                // reads through any number of leading zeros.
                if text.len() > MAX_EXPONENT_TEXT_CHARS {
                    return Err(NumberFault::NotNumeric);
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

// =============================================================================
// ON-DEMAND BACKUPS
//
// A backup is a resource of its own: `CreateBackup` catalogues one from a table,
// `ListBackups` reports the ones a region holds, and `DescribeBackup`/`DeleteBackup`
// address one by the `BackupArn` minted for it. Everything the four of them report is
// read from the records below.
// =============================================================================

// =============================================================================
// Imports of table data: the records the import surface reports
// =============================================================================

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
    StreamFault, STREAM_VIEW_TYPE_FORBIDDEN_ON_CREATE, STREAM_VIEW_TYPE_FORBIDDEN_ON_UPDATE,
    STREAM_VIEW_TYPE_REQUIRED, STREAM_VIEW_TYPE_REQUIRED_ON_ENABLE,
};

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
// Who a request speaks for, and what every stored record is made of
// ===========================================================================

/// Who a data-plane request speaks for, and whether the policies in force let it act on
/// the table it addresses, in its own file: it is about the CALLER and the documents of
/// other services, while this module is the table and item store itself. Re-exported
/// below, so every caller still names it `crate::state::…`.
pub mod authorization;
mod capacity;
mod values;
mod addressing;
mod refusals;
mod regions;
pub use regions::*;
pub use refusals::*;
pub use addressing::*;
pub use values::*;
pub use capacity::*;

/// The backup, import and export catalogues: the records a table's COPIES live in,
/// their ARNs and their status vocabularies. Re-exported below, so every caller still
/// names them `crate::state::…`.
mod backups;

/// The expression language every item operation shares: the tokenizer, the
/// substitution tables, document paths, the condition and update trees their parser
/// builds, the projection a read reports, and the legacy condition maps those
/// expressions replaced. Re-exported below, so every caller still names them
/// `crate::state::…`.
mod expressions;

/// What a read of a table — or of one of its secondary indexes — asks for and reports:
/// what being pointed at an index means for the read, which index an `IndexName`
/// resolves to, and what each reported item is narrowed to. Re-exported below, so every
/// caller still names them `crate::state::…`.
mod indexed_read;

/// The resource-based policy a table carries and the authorization decision it
/// drives. Re-exported below, so every caller still names them `crate::state::…`.
mod policies;

/// The `ProjectionExpression` member: the document paths a read asks for, and what
/// those paths report of one item. Re-exported below, so every caller still names them
/// `crate::state::…`.
mod projection;

/// What a single-item read reports: where the projection reader and the projection
/// itself meet for the one item a key addresses. Re-exported below, so every caller
/// still names them `crate::state::…`.
mod reads;

/// The table record itself — its key schema, its secondary indexes, its items and
/// everything one table answers about itself. Re-exported below, so every caller still
/// names them `crate::state::…`.
mod tables;

/// The per-table settings that each carry a status of their own: continuous backups
/// and point-in-time recovery, contributor insights, time to live, and the Kinesis
/// destinations a table streams to. Re-exported below, so every caller still names
/// them `crate::state::…`.
mod table_settings;
mod kinesis;
pub use kinesis::*;

pub use authorization::request_caller;
pub use backups::*;
pub use expressions::*;
pub use indexed_read::*;
pub use policies::*;
pub use projection::*;
pub use reads::*;
pub use table_settings::*;
pub use tables::*;

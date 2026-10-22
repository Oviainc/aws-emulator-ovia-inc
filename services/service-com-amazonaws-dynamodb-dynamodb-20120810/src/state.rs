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
    GlobalSecondaryIndexDescription, KeySchemaElement, Projection, ProvisionedThroughput,
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

/// How deeply an attribute value may nest: "DynamoDB supports nested attributes up to
/// 32 levels deep" ("Service, Account, and Table Quotas"). A value deeper than this
/// is refused, so no walk over a request's attributes is driven by caller-chosen
/// depth.
pub const MAX_ATTRIBUTE_DEPTH: usize = 32;

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
pub const MAX_ATTRIBUTE_DEFINITIONS: usize =
    2 + MAX_GLOBAL_SECONDARY_INDEXES * MAX_INDEX_KEY_ELEMENTS;

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

impl NumberKey {
    /// Parse the textual form of an `N` attribute value, or `None` when it is not a
    /// number at all.
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, unsigned) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(at) => (&unsigned[..at], unsigned[at + 1..].parse::<i32>().ok()?),
            None => (unsigned, 0),
        };
        let (integral, fractional) = match mantissa.find('.') {
            Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
            None => (mantissa, ""),
        };
        if integral.is_empty() && fractional.is_empty() {
            return None;
        }
        if !integral.bytes().chain(fractional.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        // The TEXT is bounded before anything is normalised: the digits a longer one
        // carries cannot change which documented value it denotes, and the parsed
        // value's accounted size is computed from its significant digits, not its
        // text. Both terms come from the request.
        if integral.len().saturating_add(fractional.len()) > MAX_NUMBER_TEXT_DIGITS {
            return None;
        }
        let mut digits: Vec<u8> = integral
            .bytes()
            .chain(fractional.bytes())
            .map(|b| b - b'0')
            .collect();
        // The value is 0.<digits> * 10^point before normalisation: the decimal point
        // sits after the integral digits, shifted by the exponent. Both terms come
        // from the request, so every step is checked: an out-of-range exponent
        // answers `None`, which the callers turn into a ValidationException.
        let mut point = i32::try_from(integral.len()).ok()?.checked_add(exponent)?;
        let leading_zeros = digits.iter().take_while(|digit| **digit == 0).count();
        digits.drain(..leading_zeros);
        point = point.checked_sub(i32::try_from(leading_zeros).ok()?)?;
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            // Every spelling of zero normalises to the same value, sign included.
            return Some(Self {
                negative: false,
                point: 0,
                digits,
            });
        }
        if digits.len() > MAX_SIGNIFICANT_DIGITS || !(MIN_POINT..=MAX_POINT).contains(&point) {
            return None;
        }
        Some(Self {
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

    /// Whether `item` is projected into this index: an item reaches a global secondary
    /// index only when it carries every attribute of the index's key schema ("Not all
    /// items will qualify: For example, a partition key cannot have any duplicate
    /// values" — the `Backfilling` doc).
    fn covers(&self, item: &Item) -> bool {
        self.key_schema
            .iter()
            .all(|element| item.contains_key(&element.attribute_name))
    }

    /// The bytes this index stores for one covered item: the attributes the index
    /// projects, which are the index and table key attributes plus whatever
    /// `Projection` adds.
    fn projected_size(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> i64 {
        let projection_type = self.projection.projection_type.as_deref().unwrap_or("ALL");
        if projection_type == "ALL" {
            return item_size(item);
        }
        let non_key = self
            .projection
            .non_key_attributes
            .as_deref()
            .unwrap_or_default();
        let projected: Item = item
            .iter()
            .filter(|(name, _)| {
                self.key_schema
                    .iter()
                    .chain(table_key_schema)
                    .any(|element| &element.attribute_name == *name)
                    || (projection_type == "INCLUDE" && non_key.iter().any(|kept| kept == *name))
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        item_size(&projected)
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
    /// declared.
    pub global_secondary_indexes: Vec<GlobalSecondaryIndex>,
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
    /// `ACTIVE` transition. `None` once no update is in flight.
    pub updated_at: Option<SystemTime>,
    /// When the clock owes this table its removal, set by `DeleteTable`.
    pub removed_at: Option<SystemTime>,
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
    /// table declared for it.
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
        match (declared, value) {
            ("S", AttributeValue::S(text)) => Ok(AttributeValueKey::S(text.clone())),
            ("B", AttributeValue::B(bytes)) => Ok(AttributeValueKey::B(bytes.clone())),
            ("N", AttributeValue::N(text)) => NumberKey::parse(text)
                .map(AttributeValueKey::N)
                .ok_or_else(|| {
                    // A text longer than this emulator's own numeric-text limit is
                    // refused here too, so the message names that limit as well as the
                    // documented form.
                    RequestError::Validation(format!(
                        "The parameter cannot be converted to a numeric value: {text} \
                         (a number must carry at most {MAX_SIGNIFICANT_DIGITS} significant \
                         digits, an exponent within 10^{MIN_POINT}..10^{MAX_POINT}, and at \
                         most {MAX_NUMBER_TEXT_DIGITS} digits of text, this emulator's \
                         input-size limit)"
                    ))
                }),
            (_, actual) => Err(RequestError::Validation(format!(
                "Type mismatch for key {attribute} expected: {declared} actual: {}",
                attribute_value_type(actual)
            ))),
        }
    }

    /// The primary key of an item being written: every key attribute the schema
    /// declares must be present, with its declared type. Non-key attributes are the
    /// item's own business.
    pub fn key_of_item(&self, item: &Item) -> Result<ItemKey, RequestError> {
        let mut partition = None;
        let mut sort = None;
        for element in &self.key_schema {
            let name = element.attribute_name.as_str();
            let value = item.get(name).ok_or_else(|| {
                RequestError::Validation(format!(
                    "One of the required keys was not given a value: {name}"
                ))
            })?;
            let encoded = self.key_value(name, value)?;
            if element.key_type == "HASH" {
                partition = Some(encoded);
            } else {
                sort = Some(encoded);
            }
        }
        let partition = partition.ok_or_else(|| {
            RequestError::Validation("The provided key element does not match the schema".into())
        })?;
        Ok(ItemKey { partition, sort })
    }

    /// The primary key named by a `Key` member: exactly the key attributes, no more.
    pub fn key_of_key_map(&self, key: &Item) -> Result<ItemKey, RequestError> {
        for name in key.keys() {
            if name != self.partition_key_name() && Some(name.as_str()) != self.sort_key_name() {
                return Err(RequestError::Validation(format!(
                    "The provided key element does not match the schema: {name} is not a key attribute"
                )));
            }
        }
        self.key_of_item(key)
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
                last_update_to_pay_per_request_date_time: None,
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
    pub fn active_table(
        &self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<&Table, RequestError> {
        let table = self.table(region, name)?;
        if table.status(now).serves_items() {
            Ok(table)
        } else {
            Err(not_found(name))
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
            _ => Err(not_found(name)),
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
        item: Item,
    ) -> Result<Option<Item>, RequestError> {
        let size = item_size(&item);
        let stored = self.stored_bytes();
        let table = self.active_table_mut(region, name, now)?;
        let replaced_size = table.items.get(&key).map_or(0, item_size);
        if replaced_size == 0 && table.items.len() >= MAX_ITEMS_PER_TABLE {
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

/// The `ResourceNotFoundException` message for a table that is not readable.
fn not_found(name: &str) -> RequestError {
    RequestError::NotFound(format!("Requested resource not found: Table: {name} not found"))
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

/// The `TableName` member accepts a bare table name or the table's ARN ("You can
/// also provide the Amazon Resource Name (ARN) of the table in this parameter" —
/// the `CreateTable$TableName` doc). Resolve either to the bare name, refusing a
/// name outside the documented 3..=255 `[a-zA-Z0-9_.-]+` form.
pub fn resolve_table_name(table_name: &str) -> Result<String, RequestError> {
    let name = match table_name.strip_prefix("arn:") {
        Some(rest) => rest
            .splitn(5, ':')
            .nth(4)
            .and_then(|resource| resource.strip_prefix("table/"))
            .and_then(|path| path.split('/').next())
            .ok_or_else(|| {
                RequestError::Validation(format!(
                    "Invalid TableName: {table_name} is not a DynamoDB table ARN"
                ))
            })?,
        None => table_name,
    };
    if !(3..=255).contains(&name.chars().count()) {
        return Err(RequestError::Validation(format!(
            "Invalid TableName: {name} must have length between 3 and 255"
        )));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    {
        return Err(RequestError::Validation(format!(
            "Invalid TableName: {name} must satisfy the pattern ^[a-zA-Z0-9_.-]+$"
        )));
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

/// The `ConsumedCapacity` a request that asked for it gets back, or `None` when it
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

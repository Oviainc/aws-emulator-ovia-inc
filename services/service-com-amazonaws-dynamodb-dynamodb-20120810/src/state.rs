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
    KeySchemaElement, ProvisionedThroughput, ProvisionedThroughputDescription, Tag,
    TableClassSummary, TableDescription,
};

/// A DynamoDB item: the attribute-name -> attribute-value map a client put.
pub type Item = HashMap<String, AttributeValue>;

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

/// The most tables this emulator will hold: "There is a soft account quota of 2,500
/// tables" (the `LimitExceededException` doc). Reaching it answers that modelled
/// error instead of growing without end.
pub const MAX_TABLES: usize = 2500;

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
    Active,
    Deleting,
}

impl TableStatus {
    /// The wire value, one of the model's `TableStatus` enum values.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
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

/// One stored table: its configuration, its lifecycle instants, and its items.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub arn: String,
    pub key_schema: Vec<KeySchemaElement>,
    pub attribute_definitions: Vec<AttributeDefinition>,
    /// `PROVISIONED` or `PAY_PER_REQUEST`, as resolved at CreateTable.
    pub billing_mode: String,
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    pub table_class: Option<String>,
    pub deletion_protection_enabled: bool,
    pub tags: Vec<Tag>,
    pub creation_date_time: SystemTime,
    /// When the clock owes this table its `CREATING` -> `ACTIVE` transition.
    pub active_at: SystemTime,
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
        } else if now >= self.active_at {
            TableStatus::Active
        } else {
            TableStatus::Creating
        }
    }

    /// Whether the clock has already made this table's removal due.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removed_at.is_some_and(|due| now >= due)
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
                    RequestError::Validation(format!(
                        "The parameter cannot be converted to a numeric value: {text}"
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

    /// Project this table into the `TableDescription` every table operation returns.
    /// `ItemCount` and `TableSizeBytes` are derived from `items` here, never stored.
    pub fn describe(&self, now: SystemTime) -> TableDescription {
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
                number_of_decreases_today: Some(0),
                last_increase_date_time: None,
                last_decrease_date_time: None,
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
#[derive(Debug, Default)]
pub struct ComAmazonawsDynamodbDynamodb20120810State {
    /// Every table this emulator holds, keyed by table name.
    pub tables: HashMap<String, Table>,
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Materialise the status changes the clock has already made due: a table whose
    /// `DeleteTable` span has elapsed is gone.
    ///
    /// This is the only write a reporting operation makes — it finishes a transition
    /// that was owed at this instant whether or not anybody called.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        self.tables.retain(|_, table| !table.removal_due(now));
    }

    /// The table by that name, whatever its status — what `DescribeTable` reports.
    pub fn table(&self, name: &str) -> Result<&Table, RequestError> {
        self.tables.get(name).ok_or_else(|| not_found(name))
    }

    /// The table by that name, which must be `ACTIVE`.
    ///
    /// An item operation on a table that is still `CREATING`, or already `DELETING`,
    /// is answered as if the table were not there: the `CreateTable` doc says "You
    /// can perform read and write operations only on an ACTIVE table".
    pub fn active_table(&self, name: &str, now: SystemTime) -> Result<&Table, RequestError> {
        let table = self.table(name)?;
        if table.status(now) == TableStatus::Active {
            Ok(table)
        } else {
            Err(not_found(name))
        }
    }

    /// The same, mutably, for the operations that write items.
    pub fn active_table_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Result<&mut Table, RequestError> {
        match self.tables.get_mut(name) {
            Some(table) if table.status(now) == TableStatus::Active => Ok(table),
            _ => Err(not_found(name)),
        }
    }

    /// The bytes every table together holds — the store-wide figure admission
    /// control measures against. O(tables), because each table keeps its own total.
    pub fn stored_bytes(&self) -> i64 {
        self.tables
            .values()
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
        name: &str,
        now: SystemTime,
        key: ItemKey,
        item: Item,
    ) -> Result<Option<Item>, RequestError> {
        let size = item_size(&item);
        let stored = self.stored_bytes();
        let table = self.active_table_mut(name, now)?;
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
        name: &str,
        now: SystemTime,
        key: &ItemKey,
    ) -> Result<Option<Item>, RequestError> {
        let table = self.active_table_mut(name, now)?;
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

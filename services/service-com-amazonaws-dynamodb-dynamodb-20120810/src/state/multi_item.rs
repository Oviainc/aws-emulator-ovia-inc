// SPDX-License-Identifier: CC0-1.0

//! What the multi-item paths share: `BatchWriteItem`, `BatchGetItem`,
//! `TransactWriteItems` and `TransactGetItems`.
//!
//! The documented ceilings and refusal wordings those four operations hold a request
//! to, the per-table capacity and item-collection ledgers each of them reports from,
//! what a `ClientRequestToken` stands for, and the canonical encoding a repeated token
//! is compared through. They live beside [`super`] rather than inside it because they
//! are about a REQUEST that spans several items, while the parent module is the table
//! and item store itself; `super` re-exports them, so every caller still names them
//! `crate::state::…`.

use super::*;

/// How many put-or-delete operations one `BatchWriteItem` may carry: "up to 25 item
/// put or delete operations" (the `BatchWriteItem` doc), which is also the length the
/// model gives `BatchWriteItemRequestMap`. The ceiling is on the OPERATIONS, counted
/// across every table the request groups them under, not on the tables.
pub const MAX_BATCH_WRITE_REQUESTS: usize = 25;

/// How many keys one `BatchGetItem` may address: "A single operation can retrieve up
/// to 16 MB of data, which can contain as many as 100 items" (the `BatchGetItem`
/// doc), again counted across every table in the request.
pub const MAX_BATCH_GET_KEYS: usize = 100;

/// How a batch that addresses one key twice is refused: AWS's FIXED sentence for that
/// case, naming neither the key nor the table — "you cannot put and delete the same
/// item in the same BatchWriteItem request" (the `BatchWriteItem` doc), and
/// `BatchGetItem` refuses a repeated key the same way.
pub const DUPLICATE_BATCH_KEYS_MESSAGE: &str = "Provided list of item keys contains duplicates";

/// How a batch that addresses one table twice is refused: the rule the model states,
/// "Each table name or ARN can be used only once per BatchGetItem request"
/// (`BatchGetItem$RequestItems`) — two map entries that resolve to the same table are
/// one table addressed twice, whichever spelling each used.
pub const DUPLICATE_BATCH_TABLE_MESSAGE: &str =
    "Each table name or ARN can be used only once per request";

/// How a batch that asks for more operations than it may carry is refused: AWS's
/// FIXED sentence for that quota, naming the call and neither the ceiling nor the
/// number asked for ([`MAX_BATCH_WRITE_REQUESTS`]/[`MAX_BATCH_GET_KEYS`] are internal
/// to the check).
pub fn too_many_batch_items(call: &str) -> RequestError {
    RequestError::Validation(format!("Too many items requested for the {call} call"))
}

/// How a member that must carry exactly one of a set of operations — a `WriteRequest`
/// (`PutRequest` or `DeleteRequest`) or a `TransactWriteItem` (`ConditionCheck`,
/// `Put`, `Delete` or `Update`) — is refused when it carries none of them, or more
/// than one: "You can only request one of these operations, not both, in a single
/// WriteRequest" (the `WriteRequest` doc).
pub fn exactly_one_operation(member: &str, choices: &str) -> RequestError {
    refused(format!(
        "Value at '{member}' failed to satisfy constraint: Member must contain exactly one of \
         {choices}"
    ))
}

/// How a transaction that addresses one item twice is refused: "Transaction requests
/// cannot include multiple operations on one item" (the `TransactWriteItems` doc).
pub const DUPLICATE_TRANSACT_ITEM_MESSAGE: &str =
    "Transaction request cannot include multiple operations on one item";

impl Substitutions<'_> {
    /// Hold ONE ELEMENT's expressions and its substitution tables to each other — every
    /// token its expressions dereference defined, every entry it defines referenced — and
    /// state what is wrong with them as a refusal about an element is stated.
    ///
    /// The CHECK is the request-level one ([`Substitutions::check_all_referenced`]), because
    /// the rule is the same wherever the members sit and stating it twice would let the two
    /// drift. What differs is the ENVELOPE: `<N> validation error(s) detected: ` reports
    /// violations of the REQUEST's own members, which is what the request validator is handed,
    /// and the expression members of one `TransactItems` action or one `RequestItems` entry
    /// are members of that element — so the sentence an element earns is stated on its own,
    /// like every other refusal this service raises about an element
    /// ([`DUPLICATE_TRANSACT_ITEM_MESSAGE`], [`too_many_batch_items`]).
    pub fn check_all_referenced_in_element(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        self.check_all_referenced(expressions)
            .map_err(stated_outside_the_envelope)
    }
}

/// One refusal, stated outside the `<N> validation error(s) detected: ` envelope.
///
/// The envelope over an EMPTY clause is exactly the text it puts in front of a clause, so
/// the prefix comes from the one function that writes it ([`validation_message`]) rather than
/// being spelled a second time here. A refusal that carries no envelope is already stated on
/// its own and is left exactly as it came.
fn stated_outside_the_envelope(error: RequestError) -> RequestError {
    match error {
        RequestError::Validation(message) => {
            let envelope = validation_message([String::new()]).unwrap_or_default();
            let clause = message.strip_prefix(&envelope).map(str::to_string);
            RequestError::Validation(clause.unwrap_or(message))
        }
        other => other,
    }
}

/// How a transaction that no item of could be applied is reported: the
/// `TransactionCanceledException` message, which lists the per-action reason codes the
/// `CancellationReasons` member carries in the same order ("Cancellation reason codes
/// and possible error messages" in the `TransactWriteItems` doc).
pub fn transaction_cancelled_message(codes: &[&str]) -> String {
    format!(
        "Transaction cancelled, please refer cancellation reasons for specific reasons [{}]",
        codes.join(", ")
    )
}

/// The `CancellationReason$Code` an action that nothing was wrong with carries: "None
/// - No conditions ... were violated" (the `CancellationReason` doc).
pub const CANCELLATION_CODE_NONE: &str = "None";

/// The `CancellationReason$Code` an action whose condition was not met carries.
pub const CANCELLATION_CODE_CONDITION_FAILED: &str = "ConditionalCheckFailed";

/// The `CancellationReason$Code` an action the transaction refused on its own merits
/// carries: "ValidationError - Code: ValidationError, Message: One or more parameter
/// values were invalid" ("Cancellation reason codes and possible error messages", the
/// `TransactWriteItems` doc). A failure the transaction decides PER ACTION is reported
/// this way rather than as a bare `ValidationException`, which is how a malformed
/// REQUEST — one addressing an item twice, or an entry naming no operation — is
/// refused.
pub const CANCELLATION_CODE_VALIDATION_ERROR: &str = "ValidationError";

/// The clause every refusal of a parameter value in this service opens with, which the
/// documented `ValidationError` cancellation reason also states ("Message: One or more
/// parameter values were invalid").
pub const INVALID_PARAMETER_CLAUSE: &str = "One or more parameter values were invalid";

/// What a transaction reports when IT refused ONE of its elements while preparing that
/// element against its table: the `TransactionCanceledException` message and the
/// `CancellationReasons`, which name the element at `at` with
/// [`CANCELLATION_CODE_VALIDATION_ERROR`] and `detail`, and every other element of
/// `elements` with [`CANCELLATION_CODE_NONE`] and no message, since nothing was wrong
/// with those ("Cancellation reason codes and possible error messages").
///
/// `detail` is stated behind [`INVALID_PARAMETER_CLAUSE`], the clause the documented
/// reason opens with, the way every other validation refusal of a parameter value in
/// this service words it. Every transactional operation that prepares elements against a
/// table reaches this, so the documented reason shape has one implementation.
pub fn cancelled_by_element_reasons(
    at: usize,
    elements: usize,
    detail: &str,
) -> (String, Vec<CancellationReason>) {
    let detail = if detail.starts_with(INVALID_PARAMETER_CLAUSE) {
        detail.to_string()
    } else {
        format!("{INVALID_PARAMETER_CLAUSE}: {detail}")
    };
    let codes: Vec<&str> = (0..elements)
        .map(|element| {
            if element == at {
                CANCELLATION_CODE_VALIDATION_ERROR
            } else {
                CANCELLATION_CODE_NONE
            }
        })
        .collect();
    let reasons: Vec<CancellationReason> = codes
        .iter()
        .enumerate()
        .map(|(element, code)| CancellationReason {
            code: Some((*code).to_string()),
            message: (element == at).then(|| detail.clone()),
            item: None,
        })
        .collect();
    (transaction_cancelled_message(&codes), reasons)
}

/// What a transactional read or write costs relative to the same single-item one:
/// "Transactional write requests require two write capacity units" and "Transactional
/// read requests require two read capacity units to perform one read per second for
/// items up to 4 KB" ("Read/write capacity mode", Amazon DynamoDB Developer Guide).
pub const TRANSACTIONAL_CAPACITY_FACTOR: f64 = 2.0;

/// How long a `ClientRequestToken` keeps a completed transaction idempotent: "A client
/// request token is valid for 10 minutes after the first request that uses it is
/// completed. After 10 minutes, any request with the same client token is treated as a
/// new request" (`TransactWriteItems$ClientRequestToken`).
pub const TRANSACTION_IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(10 * 60);

/// How a repeat of a `ClientRequestToken` carrying different actions is refused.
///
/// The modelled error says what happened — "DynamoDB rejected the request because you
/// retried a request with a different payload but with an idempotent token that was
/// already used" (`IdempotentParameterMismatchException`) — and the service states it in
/// the one fixed sentence below, which names the token's window and nothing of the
/// request that was sent. Shared by every operation a token makes idempotent, because it
/// is one refusal.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "Specified idempotent token was used with different request parameters within the idempotency \
     window";

/// How many `ClientRequestToken`s this emulator remembers at once.
///
/// The token is caller-chosen, so the number of DISTINCT tokens one client can present
/// inside [`TRANSACTION_IDEMPOTENCY_WINDOW`] is caller-chosen too; like every other
/// growth site in this store ([`MAX_STORE_SIZE_BYTES`], [`MAX_REGIONS`]) it is budgeted,
/// so what the store retains is not a function of what a caller sends. This emulator
/// CHOOSES 1024: far more than a client retrying a
/// handful of transactions needs, and bounded. Each record is a constant size — a
/// digest of the actions, not the actions ([`action_fingerprint`]) — plus the capacity
/// figures of at most one transaction, and the OLDEST record is dropped once the
/// ceiling is reached ([`ComAmazonawsDynamodbDynamodb20120810State::record_transaction`]).
pub const MAX_RECORDED_TRANSACTIONS: usize = 1024;

/// The fingerprint a `ClientRequestToken`'s actions are remembered by: the SHA-256 of
/// their canonical encoding, so what the store retains per token is a constant 32 bytes
/// however large the transaction was, and two calls' actions still compare equal
/// exactly when their canonical encodings do.
pub fn action_fingerprint(canonical: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(canonical.as_bytes()))
}

/// A transaction that has already been applied under a `ClientRequestToken`.
///
/// "Providing a ClientRequestToken makes the call to TransactWriteItems idempotent,
/// meaning that multiple identical calls have the same effect as one single call"
/// (`TransactWriteItems$ClientRequestToken`): a repeat inside
/// [`TRANSACTION_IDEMPOTENCY_WINDOW`] applies NOTHING and answers what the first call
/// answered, and a repeat of the token with different actions is the documented
/// `IdempotentParameterMismatchException` — which is what [`Self::actions`], the
/// fingerprint of the actions the first call carried, is compared against.
#[derive(Debug, Clone)]
pub struct CompletedTransaction {
    /// The fingerprint of the actions the first call under this token carried
    /// ([`action_fingerprint`] over [`canonical_item_text`]), which is a constant size
    /// whatever the transaction carried.
    pub actions: String,
    /// When that call completed: the idempotency window is measured from here.
    pub completed_at: SystemTime,
    /// The capacity and item-collection figures that call reported, so a repeat is
    /// answered from what the transaction DID rather than re-applied.
    pub ledger: WriteLedger,
}

/// The capacity and item-collection figures a multi-item WRITE reports, totalled per
/// table.
///
/// `BatchWriteItem` and `TransactWriteItems` both report "the capacity units consumed
/// by the entire operation" as one `ConsumedCapacity` per table and item-collection
/// metrics as a list per table, so both accumulate into this as each item lands and
/// project it once at the end.
#[derive(Debug, Clone, Default)]
pub struct WriteLedger {
    capacity: BTreeMap<String, WriteCapacity>,
    metrics: BTreeMap<String, Vec<ItemCollectionMetrics>>,
}

impl WriteLedger {
    /// Add what one item write consumed on `table`.
    pub fn record(&mut self, table: &str, capacity: &WriteCapacity) {
        let total = self.capacity.entry(table.to_string()).or_default();
        total.table += capacity.table;
        for (index, units) in &capacity.local_secondary_indexes {
            *total
                .local_secondary_indexes
                .entry(index.clone())
                .or_default() += units;
        }
        for (index, units) in &capacity.global_secondary_indexes {
            *total
                .global_secondary_indexes
                .entry(index.clone())
                .or_default() += units;
        }
    }

    /// Add the item collection one item write landed in, when the table has one.
    pub fn record_metrics(&mut self, table: &str, metrics: Option<ItemCollectionMetrics>) {
        if let Some(metrics) = metrics {
            self.metrics
                .entry(table.to_string())
                .or_default()
                .push(metrics);
        }
    }

    /// Scale every figure by `factor` — what a TRANSACTIONAL write consumes relative to
    /// the same single-item write ([`TRANSACTIONAL_CAPACITY_FACTOR`]).
    pub fn scale(&mut self, factor: f64) {
        for capacity in self.capacity.values_mut() {
            capacity.table *= factor;
            for units in capacity
                .local_secondary_indexes
                .values_mut()
                .chain(capacity.global_secondary_indexes.values_mut())
            {
                *units *= factor;
            }
        }
    }

    /// The `ConsumedCapacity` list the operation reports, one entry per table it wrote
    /// to, or `None` when the request asked for no capacity detail.
    ///
    /// `kind` is the capacity the operation BILLS these figures as
    /// ([`crate::state::CapacityKind`]): the write it performed, or the read a
    /// `ClientRequestToken` replay bills instead of re-applying the transaction.
    pub fn consumed_capacity(
        &self,
        requested: Option<&str>,
        kind: CapacityKind,
    ) -> Option<Vec<ConsumedCapacity>> {
        let reported: Vec<ConsumedCapacity> = self
            .capacity
            .iter()
            .filter_map(|(table, capacity)| {
                consumed_write_capacity(requested, table, capacity, kind)
            })
            .collect();
        (!reported.is_empty()).then_some(reported)
    }

    /// The `ItemCollectionMetrics` map the operation reports, or `None` when no write
    /// landed in a table that has an item collection to report.
    pub fn item_collection_metrics(&self) -> Option<HashMap<String, Vec<ItemCollectionMetrics>>> {
        (!self.metrics.is_empty()).then(|| {
            self.metrics
                .iter()
                .map(|(table, metrics)| (table.clone(), metrics.clone()))
                .collect()
        })
    }
}

/// The capacity a multi-item READ reports, totalled per table, for the same reason
/// [`WriteLedger`] exists: `BatchGetItem` and `TransactGetItems` both report one
/// `ConsumedCapacity` per table.
#[derive(Debug, Clone, Default)]
pub struct ReadLedger {
    units: BTreeMap<String, f64>,
}

impl ReadLedger {
    /// Add what one read consumed on `table`.
    pub fn record(&mut self, table: &str, units: f64) {
        *self.units.entry(table.to_string()).or_default() += units;
    }

    /// The `ConsumedCapacity` list the operation reports, one entry per table it read.
    pub fn consumed_capacity(&self, requested: Option<&str>) -> Option<Vec<ConsumedCapacity>> {
        let reported: Vec<ConsumedCapacity> = self
            .units
            .iter()
            .filter_map(|(table, units)| consumed_capacity(requested, table, *units))
            .collect();
        (!reported.is_empty()).then_some(reported)
    }
}

/// `item`, encoded so that two requests carrying the same item encode identically
/// whatever order their JSON maps — or their set members — arrived in: attribute names
/// in order, every value tagged by its type, sets sorted by their encoded members.
///
/// This is what makes a repeated `ClientRequestToken` comparable
/// ([`CompletedTransaction::actions`]); it is never reported to a caller.
///
/// The walk is ITERATIVE over an explicit stack, like [`attribute_value_size`] and
/// [`canonicalize_attribute_value`] and for the same reason: nesting in a request is
/// chosen by the caller, so recursing on it would make stack use a function of the
/// request.
pub fn canonical_item_text(item: &Item) -> String {
    let mut text = String::new();
    let mut work: Vec<CanonicalStep<'_>> = Vec::new();
    push_attributes(item, &mut work);
    while let Some(step) = work.pop() {
        match step {
            // The closing delimiter of a collection whose members are already on the
            // stack above it, so it is emitted after the last of them.
            CanonicalStep::Literal(literal) => text.push_str(literal),
            CanonicalStep::Named(name, value) => {
                text.push_str(&format!("{}={};", name.len(), name));
                work.push(CanonicalStep::Value(value));
            }
            CanonicalStep::Value(value) => match value {
                AttributeValue::S(string) => {
                    text.push_str(&format!("S{}:{string};", string.len()));
                }
                AttributeValue::N(number) => text.push_str(&format!("N:{number};")),
                AttributeValue::B(bytes) => {
                    text.push_str(&format!("B:{};", hex::encode(bytes)));
                }
                AttributeValue::Ss(members) => {
                    append_canonical_set('S', members.clone(), &mut text);
                }
                AttributeValue::Ns(members) => {
                    append_canonical_set('N', members.clone(), &mut text);
                }
                AttributeValue::Bs(members) => {
                    append_canonical_set('B', members.iter().map(hex::encode).collect(), &mut text);
                }
                AttributeValue::Null(is_null) => text.push_str(&format!("NULL:{is_null};")),
                AttributeValue::Bool(flag) => text.push_str(&format!("BOOL:{flag};")),
                AttributeValue::L(members) => {
                    text.push_str("L[");
                    work.push(CanonicalStep::Literal("];"));
                    for member in members.iter().rev() {
                        work.push(CanonicalStep::Value(member));
                    }
                }
                AttributeValue::M(map) => {
                    text.push_str("M[");
                    work.push(CanonicalStep::Literal("];"));
                    push_attributes(map, &mut work);
                }
            },
        }
    }
    text
}

/// One step of the canonical walk above: a value to encode, one attribute's name and
/// value, or a literal already decided (a collection's closing delimiter).
enum CanonicalStep<'v> {
    Literal(&'static str),
    Named(&'v str, &'v AttributeValue),
    Value(&'v AttributeValue),
}

/// Push every attribute of `item` so that they POP in name order: the canonical form
/// is the same whatever order the request's JSON map arrived in.
fn push_attributes<'v>(item: &'v Item, work: &mut Vec<CanonicalStep<'v>>) {
    for (name, value) in item.iter().collect::<BTreeMap<_, _>>().into_iter().rev() {
        work.push(CanonicalStep::Named(name, value));
    }
}

/// One set, appended with its members in order: a set is unordered, so two requests
/// that sent the same members in different orders encode identically.
fn append_canonical_set(tag: char, mut members: Vec<String>, text: &mut String) {
    members.sort();
    text.push_str(&format!("{tag}[{}];", members.join(",")));
}

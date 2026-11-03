// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request: the ONE place a refusal is shaped and worded.
//!
//! Every refusal this service answers is its own modelled `ValidationException`
//! (HTTP 400), which the model declares as `{message (required), reason (REQUIRED,
//! ValidationExceptionReason), fieldList}` — so `reason` is set on every one of them and
//! `fieldList` names what was wrong. Two rules, by where the rule being broken comes
//! from:
//!
//! * [`constraint_exception`] — a value that failed a constraint the MODEL declares
//!   (`@pattern`, `@length`, `@range`, an enum's value set). `message` is the AWS
//!   validation report built from the request member and the modelled constraint
//!   (`N validation error(s) detected: Value '<v>' at '<member path>' failed to satisfy
//!   constraint: <requirement>`), rendered by [`crate::service::constraint_envelope`] —
//!   the same renderer `override_refusal` uses for a refusal the framework still makes —
//!   so a clause cannot drift between the two paths. A `@pattern` is quoted there as the
//!   expression itself: Smithy's `@pattern` is an unanchored FIND, so the `^` / `$` a
//!   model spells it with are that model's way of demanding a whole-string match and are
//!   not part of the expression a caller has to satisfy.
//! * [`crate::state::validation_exception`] — a rule the model CANNOT express: a
//!   combination of members, an ordering between two of them, which chain an address
//!   member has to belong to ([`ChainAddresses`]), or a continuation token
//!   this service never issued. The top-level `message` is the generic
//!   [`crate::state::INVALID_PARAMETER_VALUES_MESSAGE`] sentinel and the explanation
//!   moves into `fieldList`, which is the machine-readable half of the error: a client
//!   reads it to tell one violated rule from another instead of matching on prose.
//!
//! ## Why the HANDLER validates, and not the framework boundary
//!
//! The framework's centralized walk would refuse a modelled constraint BEFORE any
//! handler runs, and the synthetic shape it answers with carries only `message` — no
//! `reason`, no `fieldList`, and it quotes a `@pattern` verbatim from the model, anchors
//! and all. So every constrained input member of this service carries `skip_validation`
//! on its struct in `src/ops/types/`, which keeps the constraint traits off the generated
//! member schema and hands the check to the handler; the handler runs it through this
//! module and answers with the service's own modelled shape.
//!
//! What runs the check is still the FRAMEWORK, not this module: each constrained shape is
//! declared here once as a member schema carrying the model's own constraint trait
//! (`PatternTrait`, `LengthTrait`, `RangeTrait`, `EnumTrait`), and a value is judged by
//! writing it into [`::aws_smithy_schema_server::constraints::ValidatingSerializer`] —
//! the same walk the boundary runs — so the violation, its kind and its rendering of the
//! rejected value are the framework's. Only the member PATH is this module's, because the
//! walk is handed one value at a time rather than a twin of the whole input. An
//! enumerated shape's value set is read from this crate's generated, model-derived
//! `*_VALUES`; a `@pattern` and a numeric or length bound are restated beside the shape
//! that declares them, as the sibling crates that validate inside a handler restate
//! theirs, because the schema those traits came from is the one `skip_validation` strips.
//! A constrained member added to the model later has to be added here as well — one call
//! per member below, each naming the shape the constraint lives on.
//!
//! The one refusal no HANDLER can make is an ABSENT `@required` member: it is found while
//! READING the request, so no input value ever exists for a handler to check — the model
//! makes such a member non-`Option` on the generated input struct, so a request missing it
//! never becomes one. It is still answered in this service's own modelled shape, through
//! [`boundary_exception`]: `crate::service`'s per-operation wrapper collects the
//! framework's own violation for it before the handler would have run, and words it with
//! the renderer below, so the two refusal paths cannot disagree about the exception's
//! shape.

use std::sync::LazyLock;

use ::aws_smithy_schema::serde::ShapeSerializer as _;
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    EnumTrait, LengthTrait, PatternTrait, RangeTrait, ValidatingSerializer, Violation, ViolationKind,
};

use crate::ops::types::{
    CONFIRMATION_STATUS_VALUES, LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY_VALUES,
    LIST_TRANSACTIONS_SORT_BY_VALUES, QUERY_NETWORK_VALUES, QUERY_TOKEN_STANDARD_VALUES,
    SORT_ORDER_VALUES,
};

use crate::ops::types::{
    AddressIdentifierFilter, BatchGetTokenBalanceInputItem, ConfirmationStatusFilter,
    ContractFilter, ContractIdentifier, GetAssetContractInput, GetTokenBalanceInput,
    GetTransactionInput, ListAssetContractsInput, ListFilteredTransactionEventsInput,
    ListFilteredTransactionEventsSort, ListTokenBalancesInput, ListTransactionEventsInput,
    ListTransactionsInput, ListTransactionsSort, OwnerFilter, OwnerIdentifier, TokenFilter,
    TokenIdentifier, ValidationException, ValidationExceptionField,
};

// ---------------------------------------------------------------------------
// The modelled constraints, each named by the shape that declares it
// ---------------------------------------------------------------------------

/// `ChainAddress`'s `@pattern`, as the model spells it
/// (`smithy_query(query="shape",shape="ChainAddress")`). Every address member of this
/// service targets that shape.
pub const CHAIN_ADDRESS_PATTERN: &str = "^[-A-Za-z0-9]{13,74}$";

/// `QueryTokenId`'s `@pattern`
/// (`smithy_query(query="shape",shape="QueryTokenId")`).
pub const QUERY_TOKEN_ID_PATTERN: &str = "^[a-zA-Z0-9]{1,66}$";

/// The `@pattern` of `QueryTransactionHash` AND `QueryTransactionId`, which declare the
/// same expression (`smithy_query(query="shape",shape="QueryTransactionHash")`).
pub const QUERY_TRANSACTION_HASH_PATTERN: &str = "^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$";

/// `NextToken`'s `@length`, which every paginated operation's `nextToken` targets
/// (`smithy_query(query="shape",shape="NextToken")`).
pub const NEXT_TOKEN_LENGTH: (i64, i64) = (0, 131_070);

/// The `@range` every paginated operation's `maxResults` carries; its maximum is the
/// page size [`crate::state::MAX_PAGE_SIZE`], which the output lists' `@length` repeats.
pub const MAX_RESULTS_RANGE: (i32, i32) = (1, crate::state::MAX_PAGE_SIZE);

/// `ConfirmationStatusFilter$include`'s `@length`: at least one status, no upper bound
/// (`smithy_query(query="shape",shape="ConfirmationStatusFilter")`).
pub const CONFIRMATION_STATUS_INCLUDE_MIN_LENGTH: i64 = 1;

/// `AddressIdentifierFilter$transactionEventToAddress`'s `@length(min: 1, max: 1)`
/// (`smithy_query(query="shape",shape="AddressIdentifierFilter")`).
pub const TRANSACTION_EVENT_TO_ADDRESS_LENGTH: (i64, i64) = (1, 1);

/// `GetTokenBalanceInputList`'s `@length`, which bounds one `BatchGetTokenBalance`
/// request's `getTokenBalanceInputs` (`smithy_query(query="shape",
/// shape="GetTokenBalanceInputList")`).
pub const GET_TOKEN_BALANCE_INPUTS_LENGTH: (i64, i64) = (1, 10);

/// Each constrained shape as a MEMBER SCHEMA carrying the model's own constraint
/// traits, so the check itself is the framework's
/// ([`::aws_smithy_schema_server::constraints::ValidatingSerializer`]) rather than this
/// module's: the value is written into that walk and what comes back is the framework's
/// own [`Violation`] — its kind, its rendering of the rejected value and its ceiling on
/// how much of it is echoed. Only the modelled constraint itself is stated here, the way
/// the sibling crates that validate inside a handler state it
/// (`grep -rln 'constraints::validate\|ValidatingSerializer' services/*/src`), because a
/// member carrying `skip_validation` has no schema left at the boundary to read it from.
///
/// An enumerated shape's value set is NOT restated: it is this crate's generated,
/// model-derived `*_VALUES`, so a value added to the model reaches the refusal with the
/// model.
macro_rules! constrained_shape {
    ($traits:ident, $schema:ident, $shape:literal, $kind:ident, $trait_:expr) => {
        static $traits: LazyLock<TraitMap> = LazyLock::new(|| {
            let mut traits = TraitMap::new();
            traits.insert(::std::boxed::Box::new($trait_));
            traits
        });
        static $schema: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_member(
                ShapeId::from_static(
                    concat!("com.amazonaws.managedblockchainquery#", $shape),
                    "com.amazonaws.managedblockchainquery",
                    $shape,
                ),
                ShapeType::$kind,
                $shape,
                0,
            )
            .with_traits(&$traits)
        });
    };
}

constrained_shape!(
    CHAIN_ADDRESS_TRAITS,
    CHAIN_ADDRESS,
    "ChainAddress",
    String,
    PatternTrait::new(CHAIN_ADDRESS_PATTERN)
);
constrained_shape!(
    QUERY_TOKEN_ID_TRAITS,
    QUERY_TOKEN_ID,
    "QueryTokenId",
    String,
    PatternTrait::new(QUERY_TOKEN_ID_PATTERN)
);
constrained_shape!(
    QUERY_TRANSACTION_HASH_TRAITS,
    QUERY_TRANSACTION_HASH,
    "QueryTransactionHash",
    String,
    PatternTrait::new(QUERY_TRANSACTION_HASH_PATTERN)
);
constrained_shape!(
    NEXT_TOKEN_TRAITS,
    NEXT_TOKEN,
    "NextToken",
    String,
    LengthTrait::new(Some(NEXT_TOKEN_LENGTH.0), Some(NEXT_TOKEN_LENGTH.1))
);
constrained_shape!(
    MAX_RESULTS_TRAITS,
    MAX_RESULTS,
    "MaxResults",
    Integer,
    RangeTrait::new(
        Some(f64::from(MAX_RESULTS_RANGE.0)),
        Some(f64::from(MAX_RESULTS_RANGE.1))
    )
);
constrained_shape!(
    QUERY_NETWORK_TRAITS,
    QUERY_NETWORK,
    "QueryNetwork",
    String,
    EnumTrait::new(QUERY_NETWORK_VALUES)
);
constrained_shape!(
    QUERY_TOKEN_STANDARD_TRAITS,
    QUERY_TOKEN_STANDARD,
    "QueryTokenStandard",
    String,
    EnumTrait::new(QUERY_TOKEN_STANDARD_VALUES)
);
constrained_shape!(
    CONFIRMATION_STATUS_TRAITS,
    CONFIRMATION_STATUS,
    "ConfirmationStatus",
    String,
    EnumTrait::new(CONFIRMATION_STATUS_VALUES)
);
constrained_shape!(
    SORT_ORDER_TRAITS,
    SORT_ORDER,
    "SortOrder",
    String,
    EnumTrait::new(SORT_ORDER_VALUES)
);
constrained_shape!(
    LIST_TRANSACTIONS_SORT_BY_TRAITS,
    LIST_TRANSACTIONS_SORT_BY,
    "ListTransactionsSortBy",
    String,
    EnumTrait::new(LIST_TRANSACTIONS_SORT_BY_VALUES)
);
constrained_shape!(
    LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY_TRAITS,
    LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY,
    "ListFilteredTransactionEventsSortBy",
    String,
    EnumTrait::new(LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY_VALUES)
);

// ---------------------------------------------------------------------------
// Accumulating the violations of one request
// ---------------------------------------------------------------------------

/// Ceiling on the violations one request may accumulate, and the framework's own
/// ([`::aws_smithy_schema_server::constraints`] caps its walk at the same number for the
/// same reason): every violation is rendered into the response TWICE — as a clause of
/// the `message` and as a `fieldList` entry — so an uncapped count turns a few bytes of
/// request into an arbitrarily long answer. Past the ceiling nothing is recorded, and the
/// report stays literally true of what was detected.
///
/// A caller cannot act on a hundred clauses anyway, and real services truncate too.
const MAX_VIOLATIONS: usize = 100;

/// The constraint violations found in one request, in the order the members were
/// checked — which is the order the report lists them in, at most [`MAX_VIOLATIONS`] of
/// them.
///
/// Accumulating rather than failing at the first one is what lets a caller fix a request
/// in one round trip, and is what the framework's own walk does.
#[derive(Debug, Default)]
pub struct Violations {
    found: Vec<Violation>,
}

impl Violations {
    /// An empty report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether every member checked so far satisfied its constraints.
    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }

    /// Whether the report is full, so nothing further would be recorded. A checker that
    /// walks a collection stops here rather than iterating a list a caller sized.
    pub fn is_full(&self) -> bool {
        self.found.len() >= MAX_VIOLATIONS
    }

    /// The violations found, for a caller that reports them somewhere other than a
    /// top-level error (`BatchGetTokenBalance` reports a per-item failure inside its
    /// 200 response).
    pub fn found(&self) -> &[Violation] {
        &self.found
    }

    /// Adopt what the framework's walk found about ONE value, reported at `path`.
    ///
    /// The violation is the framework's: its [`ViolationKind`], and its rendering of the
    /// rejected value under the framework's own echo ceiling. Only the PATH is this
    /// module's, because the walk is handed one value at a time (a constrained shape, not
    /// a twin of the whole input) and so cannot know the member path the request reached
    /// it by; the dotted spelling is the one
    /// [`::aws_smithy_schema_server::constraints::path`] defines.
    ///
    /// A full report records nothing more: the count ceiling is enforced HERE, in the one
    /// place a violation can enter, so no checker can bypass it.
    fn adopt(&mut self, path: &str, found: Vec<Violation>) {
        for mut violation in found {
            if self.is_full() {
                return;
            }
            violation.path = path.to_string();
            self.found.push(violation);
        }
    }

    /// Check one string value against the constrained shape `schema` declares.
    ///
    /// The check is the framework's: the trait map on `schema` is the model's constraint
    /// and [`ValidatingSerializer`] is the same walk the request boundary runs, so a
    /// `@pattern`, a value set and a string `@length` are judged here exactly as they are
    /// judged there.
    fn string(&mut self, schema: &Schema, path: &str, value: &str) {
        if self.is_full() {
            return;
        }
        let mut validator = ValidatingSerializer::new();
        // Infallible: the validator accumulates violations instead of failing.
        let _ = validator.write_string(schema, value);
        self.adopt(path, validator.into_violations());
    }

    /// Check one integer value against the constrained shape `schema` declares; see
    /// [`Violations::string`].
    fn integer(&mut self, schema: &Schema, path: &str, value: i32) {
        if self.is_full() {
            return;
        }
        let mut validator = ValidatingSerializer::new();
        let _ = validator.write_integer(schema, value);
        self.adopt(path, validator.into_violations());
    }

    /// A list member's `@length` — the element COUNT, echoed the way the framework's
    /// walk echoes a collection (`"<n> items"`), with the framework's own
    /// [`ViolationKind`] so the clause reads identically either way.
    ///
    /// The one check this module still performs itself: the framework judges a
    /// collection's `@length` from inside a walk over the collection
    /// (`check_collection_length`, reached through `write_list`), and both that method and
    /// the list-schema builders codegen uses are private to the frozen
    /// `vendor/smithy-schema-server`, so a handler cannot hand it a list the way
    /// [`Violations::string`] hands it a string. What is judged is still only the modelled
    /// bound, and what is reported is still the framework's violation kind.
    ///
    /// Answers whether the list is WITHIN its bound, which is what bounds the work a
    /// request can cause: a caller whose list is already too long gets that one
    /// violation, and its elements are not walked (see [`confirmation_status_filter`]).
    fn list_length(
        &mut self,
        path: &str,
        count: usize,
        min: Option<i64>,
        max: Option<i64>,
    ) -> bool {
        if self.is_full() {
            return true;
        }
        let shown = format!("{count} items");
        let count = count as i64;
        let kind = match (min.filter(|min| count < *min), max.filter(|max| count > *max)) {
            (Some(min), _) => ViolationKind::MinLength(min),
            (None, Some(max)) => ViolationKind::MaxLength(max),
            (None, None) => return true,
        };
        self.found.push(Violation {
            path: path.to_string(),
            kind,
            value: Some(shown),
            sensitive: false,
        });
        false
    }
}

// ---------------------------------------------------------------------------
// One checker per constrained shape
// ---------------------------------------------------------------------------

/// A `ChainAddress`-targeting member.
pub fn chain_address(violations: &mut Violations, path: &str, value: &str) {
    violations.string(&CHAIN_ADDRESS, path, value);
}

/// A `QueryTokenId`-targeting member.
pub fn query_token_id(violations: &mut Violations, path: &str, value: &str) {
    violations.string(&QUERY_TOKEN_ID, path, value);
}

/// A `QueryTransactionHash`- or `QueryTransactionId`-targeting member.
pub fn query_transaction_hash(violations: &mut Violations, path: &str, value: &str) {
    violations.string(&QUERY_TRANSACTION_HASH, path, value);
}

/// A `QueryNetwork`-targeting member.
pub fn query_network(violations: &mut Violations, path: &str, value: &str) {
    violations.string(&QUERY_NETWORK, path, value);
}

/// A paginated operation's `nextToken`.
pub fn next_token(violations: &mut Violations, token: Option<&str>) {
    if let Some(token) = token {
        violations.string(&NEXT_TOKEN, "nextToken", token);
    }
}

/// A paginated operation's `maxResults`.
pub fn max_results(violations: &mut Violations, max_results: Option<i32>) {
    if let Some(max_results) = max_results {
        violations.integer(&MAX_RESULTS, "maxResults", max_results);
    }
}

/// `TokenIdentifier`: a `QueryNetwork` and the two optional members that name the token.
pub fn token_identifier(violations: &mut Violations, path: &str, token: &TokenIdentifier) {
    query_network(violations, &format!("{path}.network"), &token.network);
    if let Some(contract) = &token.contract_address {
        chain_address(violations, &format!("{path}.contractAddress"), contract);
    }
    if let Some(token_id) = &token.token_id {
        query_token_id(violations, &format!("{path}.tokenId"), token_id);
    }
}

/// `OwnerIdentifier`.
pub fn owner_identifier(violations: &mut Violations, path: &str, owner: &OwnerIdentifier) {
    chain_address(violations, &format!("{path}.address"), &owner.address);
}

/// `OwnerFilter`, which declares the same single address member as `OwnerIdentifier`.
pub fn owner_filter(violations: &mut Violations, path: &str, owner: &OwnerFilter) {
    chain_address(violations, &format!("{path}.address"), &owner.address);
}

/// `TokenFilter`, which declares the same three members as `TokenIdentifier`.
pub fn token_filter(violations: &mut Violations, path: &str, filter: &TokenFilter) {
    query_network(violations, &format!("{path}.network"), &filter.network);
    if let Some(contract) = &filter.contract_address {
        chain_address(violations, &format!("{path}.contractAddress"), contract);
    }
    if let Some(token_id) = &filter.token_id {
        query_token_id(violations, &format!("{path}.tokenId"), token_id);
    }
}

/// `ContractIdentifier`.
pub fn contract_identifier(
    violations: &mut Violations,
    path: &str,
    contract: &ContractIdentifier,
) {
    query_network(violations, &format!("{path}.network"), &contract.network);
    chain_address(
        violations,
        &format!("{path}.contractAddress"),
        &contract.contract_address,
    );
}

/// `ContractFilter`.
pub fn contract_filter(violations: &mut Violations, path: &str, filter: &ContractFilter) {
    query_network(violations, &format!("{path}.network"), &filter.network);
    violations.string(
        &QUERY_TOKEN_STANDARD,
        &format!("{path}.tokenStandard"),
        &filter.token_standard,
    );
    chain_address(
        violations,
        &format!("{path}.deployerAddress"),
        &filter.deployer_address,
    );
}

/// `ConfirmationStatusFilter`: a non-empty list of `ConfirmationStatus` values.
///
/// `include` is the one list of this service whose `@length` declares no MAXIMUM. Every
/// element is still checked, as the framework's own walk checks every element, so the
/// value set is enforced at EVERY position; what is bounded is the report, which stops
/// recording once it is full.
pub fn confirmation_status_filter(
    violations: &mut Violations,
    path: &str,
    filter: &ConfirmationStatusFilter,
) {
    let include = format!("{path}.include");
    let within = violations.list_length(
        &include,
        filter.include.len(),
        Some(CONFIRMATION_STATUS_INCLUDE_MIN_LENGTH),
        None,
    );
    if !within {
        return;
    }
    for (position, status) in filter.include.iter().enumerate() {
        if violations.is_full() {
            return;
        }
        violations.string(&CONFIRMATION_STATUS, &element(&include, position), status);
    }
}

/// `ListTransactionsSort`.
pub fn list_transactions_sort(violations: &mut Violations, path: &str, sort: &ListTransactionsSort) {
    if let Some(sort_by) = &sort.sort_by {
        violations.string(
            &LIST_TRANSACTIONS_SORT_BY,
            &format!("{path}.sortBy"),
            sort_by,
        );
    }
    if let Some(order) = &sort.sort_order {
        violations.string(&SORT_ORDER, &format!("{path}.sortOrder"), order);
    }
}

/// `ListFilteredTransactionEventsSort`.
pub fn list_filtered_transaction_events_sort(
    violations: &mut Violations,
    path: &str,
    sort: &ListFilteredTransactionEventsSort,
) {
    if let Some(sort_by) = &sort.sort_by {
        violations.string(
            &LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY,
            &format!("{path}.sortBy"),
            sort_by,
        );
    }
    if let Some(order) = &sort.sort_order {
        violations.string(&SORT_ORDER, &format!("{path}.sortOrder"), order);
    }
}

/// `AddressIdentifierFilter`: exactly one `ChainAddress`.
///
/// A list outside its own `@length` is reported by that failure alone, so the declared
/// bound — not the request — is what bounds the element checks below.
pub fn address_identifier_filter(
    violations: &mut Violations,
    path: &str,
    filter: &AddressIdentifierFilter,
) {
    let addresses = format!("{path}.transactionEventToAddress");
    let (min, max) = TRANSACTION_EVENT_TO_ADDRESS_LENGTH;
    let within = violations.list_length(
        &addresses,
        filter.transaction_event_to_address.len(),
        Some(min),
        Some(max),
    );
    if !within {
        return;
    }
    for (position, address) in filter.transaction_event_to_address.iter().enumerate() {
        chain_address(violations, &element(&addresses, position), address);
    }
}

/// The path of a list element, spelled the way a validation report spells it.
///
/// The spelling comes from the framework's own
/// [`::aws_smithy_schema_server::constraints::path`], whose module doc asks every path
/// builder to go through it "so the two cannot drift into different spellings of the
/// same position": the 1-based position, then the step into the collection.
fn element(path: &str, position: usize) -> String {
    ::aws_smithy_schema_server::constraints::path::join(&[
        path.to_string(),
        ::aws_smithy_schema_server::constraints::path::element(position + 1),
    ])
}

// ---------------------------------------------------------------------------
// The one rule about an address the model cannot state
// ---------------------------------------------------------------------------

/// Every `ChainAddress`-targeting member of a request, checked against the chain the
/// request names it on.
///
/// `ChainAddress` is ONE type for every chain this service indexes, so its `@pattern`
/// cannot say which strings are addresses of the chain a request asked about: a base58
/// Bitcoin address, an EVM address and a 13-character label satisfy it equally. Which
/// ones are addresses of a chain is fixed by that chain's own encodings
/// ([`crate::state::address`]), and which chain a member has to carry an address of is
/// fixed by the `QueryNetwork` the SAME request carries — so the check is this service's,
/// and it is made here, in the one place every operation's input is checked, rather than
/// in the handlers.
///
/// Each entry names the member a caller would repair and says which chain's address it
/// had to be, which is all the request got wrong; the entries ACCUMULATE, so a request
/// whose owner and whose contract address are both of the wrong chain learns about both
/// in one answer.
#[derive(Debug, Default)]
struct ChainAddresses {
    refused: Vec<ValidationExceptionField>,
}

impl ChainAddresses {
    /// One `ChainAddress`-targeting member, against the chain `network` runs.
    fn member(&mut self, path: &str, network: &str, address: &str) {
        if !crate::state::address::is_address_of_network(network, address) {
            self.refused.push(crate::state::validation_field(
                path,
                crate::state::address::invalid_address_message(network),
            ));
        }
    }

    /// `TokenIdentifier`'s and `TokenFilter`'s shared address member, against the network
    /// the identifier itself names.
    fn contract_address(&mut self, path: &str, network: &str, contract: Option<&String>) {
        if let Some(contract) = contract {
            self.member(&format!("{path}.contractAddress"), network, contract);
        }
    }

    /// `Ok(())` when every address member carried an address of its own chain, else this
    /// service's modelled `ValidationException` naming each one that did not.
    fn report(self) -> Result<(), ValidationException> {
        if self.refused.is_empty() {
            return Ok(());
        }
        Err(crate::state::validation_exception(self.refused))
    }
}

// ---------------------------------------------------------------------------
// One checker per operation input
// ---------------------------------------------------------------------------

/// Every modelled constraint on `BatchGetTokenBalanceInput` that is a property of the
/// REQUEST rather than of one item: the `@length` of the item list itself.
///
/// A per-item violation is NOT a request-level failure — it is reported inside the 200
/// response's `errors` list — so it is collected separately by
/// [`batch_get_token_balance_item`].
pub fn batch_get_token_balance_input(
    items: Option<&[BatchGetTokenBalanceInputItem]>,
) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    // `getTokenBalanceInputs` is modelled OPTIONAL, so a request that omits it breaks
    // no bound; the list's `@length` applies to a list that is there.
    if let Some(items) = items {
        let (min, max) = GET_TOKEN_BALANCE_INPUTS_LENGTH;
        // The answer is this one violation; the items themselves are checked by the
        // handler only for a list the bound admits, so a caller cannot make the batch's
        // work grow past the modelled ten.
        let _within = violations.list_length(
            "getTokenBalanceInputs",
            items.len(),
            Some(min),
            Some(max),
        );
    }
    report(violations)
}

/// Every rule one `BatchGetTokenBalanceInputItem` has to satisfy — the modelled
/// constraints and the chain each address member has to belong to — reported against the
/// item's OWN members: the item's failure travels in the response beside the identifiers
/// it echoes, so the caller already knows which item it was.
///
/// `None` when the item breaks no rule. An item is checked exactly as
/// [`get_token_balance_input`] checks the single read's input, so the batch and the single
/// read cannot disagree about one item.
pub fn batch_get_token_balance_item(
    item: &BatchGetTokenBalanceInputItem,
) -> Option<ValidationException> {
    let mut violations = Violations::new();
    token_identifier(&mut violations, "tokenIdentifier", &item.token_identifier);
    owner_identifier(&mut violations, "ownerIdentifier", &item.owner_identifier);
    if !violations.is_empty() {
        return Some(constraint_exception(&violations));
    }
    token_balance_addresses(&item.token_identifier, &item.owner_identifier).err()
}

/// The chain-address rule over the pair of identifiers a token-balance read names, which
/// `GetTokenBalance` and one `BatchGetTokenBalance` item declare identically: the owner,
/// and the contract the token lives at, both on the network the `TokenIdentifier` names.
fn token_balance_addresses(
    token: &TokenIdentifier,
    owner: &OwnerIdentifier,
) -> Result<(), ValidationException> {
    let mut addresses = ChainAddresses::default();
    addresses.contract_address("tokenIdentifier", &token.network, token.contract_address.as_ref());
    addresses.member("ownerIdentifier.address", &token.network, &owner.address);
    addresses.report()
}

/// Every modelled constraint on `GetAssetContractInput`, and the chain its contract
/// address has to belong to.
///
/// The address check is made only on the networks this operation SERVES: "The Bitcoin
/// blockchain networks do not support this operation", so on a Bitcoin network what is
/// wrong with the request is the network itself
/// ([`crate::state::contracts_unsupported_on_network`], raised by the handler) and not the
/// address it carries.
pub fn get_asset_contract_input(input: &GetAssetContractInput) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    contract_identifier(
        &mut violations,
        "contractIdentifier",
        &input.contract_identifier,
    );
    report(violations)?;
    let requested = &input.contract_identifier;
    let mut addresses = ChainAddresses::default();
    if !crate::state::is_bitcoin(&requested.network) {
        addresses.member(
            "contractIdentifier.contractAddress",
            &requested.network,
            &requested.contract_address,
        );
    }
    addresses.report()
}

/// Every modelled constraint on `GetTokenBalanceInput`, and the chain each of its address
/// members has to belong to.
pub fn get_token_balance_input(input: &GetTokenBalanceInput) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    token_identifier(&mut violations, "tokenIdentifier", &input.token_identifier);
    owner_identifier(&mut violations, "ownerIdentifier", &input.owner_identifier);
    report(violations)?;
    token_balance_addresses(&input.token_identifier, &input.owner_identifier)
}

/// Every modelled constraint on `GetTransactionInput`.
pub fn get_transaction_input(input: &GetTransactionInput) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    query_network(&mut violations, "network", &input.network);
    if let Some(hash) = &input.transaction_hash {
        query_transaction_hash(&mut violations, "transactionHash", hash);
    }
    if let Some(id) = &input.transaction_id {
        query_transaction_hash(&mut violations, "transactionId", id);
    }
    report(violations)
}

/// Every modelled constraint on `ListAssetContractsInput`, and the chain its deployer
/// address has to belong to — on the networks this operation serves, for the reason
/// [`get_asset_contract_input`] gives.
pub fn list_asset_contracts_input(
    input: &ListAssetContractsInput,
) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    contract_filter(&mut violations, "contractFilter", &input.contract_filter);
    next_token(&mut violations, input.next_token.as_deref());
    max_results(&mut violations, input.max_results);
    report(violations)?;
    let filter = &input.contract_filter;
    let mut addresses = ChainAddresses::default();
    if !crate::state::is_bitcoin(&filter.network) {
        addresses.member(
            "contractFilter.deployerAddress",
            &filter.network,
            &filter.deployer_address,
        );
    }
    addresses.report()
}

/// Every modelled constraint on `ListFilteredTransactionEventsInput`.
///
/// `network` is the one network member of this service that targets a plain `String`
/// rather than `QueryNetwork` (`smithy_query(query="operation",
/// operation="ListFilteredTransactionEvents")`), so it carries no value set to check.
pub fn list_filtered_transaction_events_input(
    input: &ListFilteredTransactionEventsInput,
) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    address_identifier_filter(
        &mut violations,
        "addressIdentifierFilter",
        &input.address_identifier_filter,
    );
    if let Some(filter) = &input.confirmation_status_filter {
        confirmation_status_filter(&mut violations, "confirmationStatusFilter", filter);
    }
    if let Some(sort) = &input.sort {
        list_filtered_transaction_events_sort(&mut violations, "sort", sort);
    }
    next_token(&mut violations, input.next_token.as_deref());
    max_results(&mut violations, input.max_results);
    report(violations)
}

/// Every modelled constraint on `ListTokenBalancesInput`, and the chain each of its
/// address members has to belong to.
///
/// `OwnerFilter` carries no network of its own, so the chain both members are checked
/// against is the one `tokenFilter` names — the network the listing reads.
pub fn list_token_balances_input(
    input: &ListTokenBalancesInput,
) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    if let Some(owner) = &input.owner_filter {
        owner_filter(&mut violations, "ownerFilter", owner);
    }
    token_filter(&mut violations, "tokenFilter", &input.token_filter);
    next_token(&mut violations, input.next_token.as_deref());
    max_results(&mut violations, input.max_results);
    report(violations)?;
    let network = input.token_filter.network.as_str();
    let mut addresses = ChainAddresses::default();
    addresses.contract_address(
        "tokenFilter",
        network,
        input.token_filter.contract_address.as_ref(),
    );
    if let Some(owner) = &input.owner_filter {
        addresses.member("ownerFilter.address", network, &owner.address);
    }
    addresses.report()
}

/// Every modelled constraint on `ListTransactionEventsInput`.
pub fn list_transaction_events_input(
    input: &ListTransactionEventsInput,
) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    query_network(&mut violations, "network", &input.network);
    if let Some(hash) = &input.transaction_hash {
        query_transaction_hash(&mut violations, "transactionHash", hash);
    }
    if let Some(id) = &input.transaction_id {
        query_transaction_hash(&mut violations, "transactionId", id);
    }
    next_token(&mut violations, input.next_token.as_deref());
    max_results(&mut violations, input.max_results);
    report(violations)
}

/// Every modelled constraint on `ListTransactionsInput`, and the chain its address has to
/// belong to.
pub fn list_transactions_input(input: &ListTransactionsInput) -> Result<(), ValidationException> {
    let mut violations = Violations::new();
    chain_address(&mut violations, "address", &input.address);
    query_network(&mut violations, "network", &input.network);
    if let Some(sort) = &input.sort {
        list_transactions_sort(&mut violations, "sort", sort);
    }
    if let Some(filter) = &input.confirmation_status_filter {
        confirmation_status_filter(&mut violations, "confirmationStatusFilter", filter);
    }
    next_token(&mut violations, input.next_token.as_deref());
    max_results(&mut violations, input.max_results);
    report(violations)?;
    let mut addresses = ChainAddresses::default();
    addresses.member("address", &input.network, &input.address);
    addresses.report()
}

/// `Ok(())` when nothing was violated, else the modelled `ValidationException`.
fn report(violations: Violations) -> Result<(), ValidationException> {
    if violations.is_empty() {
        return Ok(());
    }
    Err(constraint_exception(&violations))
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

/// The modelled `ValidationException` for values that failed a MODELLED constraint.
///
/// `message` is the AWS validation report, and `fieldList` carries one entry per failing
/// member: its `name` is that member's path in the request and its `message` is that
/// member's own clause of the report. Both go through
/// [`crate::service::constraint_envelope`] / [`crate::service::constraint_clause`] — the
/// SAME renderer the service's refusal hook uses for the refusals the framework still
/// makes (an absent `@required` member), so a clause cannot read one way here and
/// another way there. `reason` is the model's `fieldValidationFailed`, which the hook
/// cannot carry and this shape can.
pub fn constraint_exception(violations: &Violations) -> ValidationException {
    let violations = violations.found();
    violations_exception(
        violations,
        crate::service::constraint_envelope(violations)
            .unwrap_or_else(|| crate::state::INVALID_PARAMETER_VALUES_MESSAGE.to_string()),
    )
}

/// The modelled `ValidationException` for a refusal the framework made BEFORE any handler
/// ran, worded from the violations its own boundary walk produced.
///
/// This is the one refusal path no handler can serve — an ABSENT `@required` member, found
/// while the request is being READ (see this module's header) — and it is answered with
/// the SAME renderer [`constraint_exception`] uses, so a request refused for an absent
/// member and one refused for a bad value carry one envelope: the AWS validation report as
/// `message`, the model-required `reason`, and one `fieldList` entry per violation naming
/// the member. Nothing here is keyed to an operation or a member: whatever violations the
/// framework's walk produces are stated, so it holds for every `@required` member of every
/// operation, top-level or nested in a filter, an identifier or a batch item.
///
/// `None` when nothing was refused, and when the set holds a violation class
/// [`crate::service::constraint_clause`] has no wording for — which leaves that request to
/// the framework's own envelope rather than inventing a sentence for it.
pub fn boundary_exception(found: &[Violation]) -> Option<ValidationException> {
    // The same ceiling every refusal of this service is bounded by: the body renders each
    // violation twice, so the count a request can cause is capped here too.
    let capped = &found[..found.len().min(MAX_VIOLATIONS)];
    // The report is rendered ONCE: the same string decides whether this path states the
    // refusal at all (an unworded violation class declines the whole refusal, leaving it to
    // the framework's own envelope) and is the `message` it states.
    let message = crate::service::constraint_envelope(capped)?;
    Some(violations_exception(capped, message))
}

/// The one rendering of a violation set into this service's modelled exception, shared by
/// both refusal paths: [`constraint_exception`] for the constraints a handler checks and
/// [`boundary_exception`] for the absence the framework finds. `message` is the report its
/// caller rendered from the same violations.
fn violations_exception(violations: &[Violation], message: String) -> ValidationException {
    ValidationException {
        message,
        reason: crate::state::VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(
            violations
                .iter()
                .map(|violation| {
                    crate::state::validation_field(
                        &violation.path,
                        crate::service::constraint_clause(violation).unwrap_or_else(|| {
                            crate::state::INVALID_PARAMETER_VALUES_MESSAGE.to_string()
                        }),
                    )
                })
                .collect(),
        ),
    }
}

/// What a refusal says about the members it names, as ONE sentence.
///
/// For a report that has nowhere to put a `fieldList`: a
/// `BatchGetTokenBalanceErrorItem` models a single `errorMessage`, so an item refused on
/// a rule whose top-level message is the generic sentinel would otherwise say nothing
/// about what was wrong. The entries are what carry that, so they are joined; a refusal
/// whose own message is already the detail (a constraint report) keeps it.
pub fn explanation(error: &ValidationException) -> String {
    let fields = error.field_list.as_deref().unwrap_or_default();
    if fields.is_empty() || error.message != crate::state::INVALID_PARAMETER_VALUES_MESSAGE {
        return error.message.clone();
    }
    fields
        .iter()
        .map(|field| field.message.as_str())
        .collect::<Vec<&str>>()
        .join(" ")
}

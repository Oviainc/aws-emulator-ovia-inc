// SPDX-License-Identifier: CC0-1.0

//! What a refused request looks like on the wire, and what the two operations that
//! report a failure INSIDE a success answer.
//!
//! Four rules are under test, each as a TABLE whose rows differ along what the rule
//! generalises over (the constraint class, the member, the shape, the operation, the
//! item within a batch) rather than as one recorded request:
//!
//! 1. a value that failed a constraint the MODEL declares is refused with HTTP 400
//!    `ValidationException` carrying `reason` = `fieldValidationFailed`, the AWS
//!    validation report as `message`, and one `fieldList` entry per failing member —
//!    with a `@pattern` quoted WITHOUT the `^` / `$` the model spells it with;
//! 2. a rule the model cannot express (a member combination, an ordering between two
//!    members, a continuation token this service never issued) is refused with the
//!    generic top-level message and the explanation in `fieldList`;
//! 3. `BatchGetTokenBalance` answers every item it can and reports the others in the
//!    response's `errors` list, inside HTTP 200;
//! 4. every `ResourceNotFoundException` states the same request-independent message and
//!    carries the identity in `resourceId` / `resourceType`.
//!
//! Every bound, pattern and value set written down here names the model shape it comes
//! from; none is read out of the implementation.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::{
    CONFIRMATION_STATUS_VALUES, LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY_VALUES,
    LIST_TRANSACTIONS_SORT_BY_VALUES, QUERY_NETWORK_VALUES, QUERY_TOKEN_STANDARD_VALUES,
    SORT_ORDER_VALUES,
};

use crate::chain_query_slice::{error_code, Emulator, NetworkFixture};

// ---------------------------------------------------------------------------
// The modelled constraints, from the model rather than from the implementation
// ---------------------------------------------------------------------------

/// `ChainAddress`'s `@pattern` with the model's anchors
/// (`smithy_query(query="shape",shape="ChainAddress")`), and the expression a refusal
/// has to quote: Smithy's `@pattern` is an unanchored find, so `^` / `$` are the model's
/// way of demanding a whole-string match and are not part of the expression.
const CHAIN_ADDRESS_ANCHORED: &str = "^[-A-Za-z0-9]{13,74}$";
/// See [`CHAIN_ADDRESS_ANCHORED`].
const CHAIN_ADDRESS: &str = "[-A-Za-z0-9]{13,74}";

/// `QueryTokenId`'s `@pattern` (`smithy_query(query="shape",shape="QueryTokenId")`).
const QUERY_TOKEN_ID_ANCHORED: &str = "^[a-zA-Z0-9]{1,66}$";
/// See [`QUERY_TOKEN_ID_ANCHORED`].
const QUERY_TOKEN_ID: &str = "[a-zA-Z0-9]{1,66}";

/// The `@pattern` of `QueryTransactionHash` and `QueryTransactionId`, which declare the
/// same expression (`smithy_query(query="shape",shape="QueryTransactionHash")`).
const TRANSACTION_HASH_ANCHORED: &str = "^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$";
/// See [`TRANSACTION_HASH_ANCHORED`].
const TRANSACTION_HASH: &str = "(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})";

/// Each enumerated shape's value set, as the crate's GENERATED, model-derived constants
/// carry it (`smithy_query(query="shape",shape="QueryNetwork,QueryTokenStandard,
/// ConfirmationStatus,SortOrder,ListTransactionsSortBy,
/// ListFilteredTransactionEventsSortBy")` for what the model declares).
///
/// Read from the generated constants rather than written out again: the refusal quotes
/// the value set the schema carries, so a value added to or dropped from the model moves
/// the expectation with the behaviour instead of leaving a second copy to drift. The
/// ORDER is therefore the generated order, which is the order a reported set is in.
const QUERY_NETWORK: &[&str] = QUERY_NETWORK_VALUES;
/// See [`QUERY_NETWORK`].
const QUERY_TOKEN_STANDARD: &[&str] = QUERY_TOKEN_STANDARD_VALUES;
/// See [`QUERY_NETWORK`].
const CONFIRMATION_STATUS: &[&str] = CONFIRMATION_STATUS_VALUES;
/// See [`QUERY_NETWORK`].
const SORT_ORDER: &[&str] = SORT_ORDER_VALUES;
/// See [`QUERY_NETWORK`].
const LIST_TRANSACTIONS_SORT_BY: &[&str] = LIST_TRANSACTIONS_SORT_BY_VALUES;
/// See [`QUERY_NETWORK`].
const LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY: &[&str] =
    LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY_VALUES;

/// `NextToken`'s `@length` maximum, which every paginated operation's `nextToken`
/// targets (`smithy_query(query="shape",shape="NextToken")`).
const NEXT_TOKEN_MAX_LENGTH: i64 = 131_070;

/// The `@range` every paginated operation's `maxResults` carries (e.g.
/// `smithy_query(query="operation",operation="ListTransactions")`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 250);

/// `GetTokenBalanceInputList`'s `@length`, which bounds one `BatchGetTokenBalance`
/// request's `getTokenBalanceInputs`.
const BATCH_LENGTH: (i64, i64) = (1, 10);

/// The `ValidationExceptionReason` a refused member value carries: one of
/// `unknownOperation | cannotParse | fieldValidationFailed | other`
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The generic top-level message of a refusal this service words itself.
const PARAMETER_VALUES_INVALID: &str = "One or more parameter values were invalid.";

/// The `message` every `ResourceNotFoundException` of this service states.
const RESOURCE_NOT_FOUND: &str = "The requested resource was not found.";

/// The only value the model's `ResourceType` enum declares
/// (`smithy_query(query="shape",shape="ResourceType")`).
const RESOURCE_TYPE: &str = "collection";

// ---------------------------------------------------------------------------
// The report a refusal states, written from the AWS form
// ---------------------------------------------------------------------------

/// The AWS validation report for ONE failing member.
fn one_validation_error(value: Option<&str>, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: {}",
        clause(value, member, requirement)
    )
}

/// One clause of the report: where the offending value was, and what it had to satisfy.
///
/// `None` for a COLLECTION, whose size failed its `@length`: the collection itself is
/// not a value any caller submitted, so the clause names the member alone.
fn clause(value: Option<&str>, member: &str, requirement: &str) -> String {
    match value {
        Some(value) => {
            format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
        }
        None => format!("Value at '{member}' failed to satisfy constraint: {requirement}"),
    }
}

/// The requirement a `@pattern` states, quoting the expression unanchored.
fn pattern_requirement(unanchored: &str) -> String {
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// The requirement an enumerated shape states.
fn enum_requirement(values: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// The requirement a `@length` minimum states.
fn min_length_requirement(min: i64) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a `@length` maximum states.
fn max_length_requirement(max: i64) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a `@range` minimum states.
fn min_value_requirement(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement a `@range` maximum states.
fn max_value_requirement(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

// ---------------------------------------------------------------------------
// Rule 1: a modelled constraint
// ---------------------------------------------------------------------------

/// One row of the constraint table: a request with exactly ONE member outside its
/// modelled constraint, and the clause that member's refusal has to state.
struct Refused {
    case: String,
    route: &'static str,
    body: Value,
    /// The member path the report names, as AWS spells it (lowerCamel, dotted, with a
    /// list element's 1-based position and the `member` step).
    member: &'static str,
    /// The offending value as the clause echoes it, or `None` for a collection whose
    /// SIZE failed its `@length` — see [`clause`].
    value: Option<String>,
    requirement: String,
}

/// The constraint table: every constraint class this service's input shapes declare
/// (`@pattern`, an enum value set, `@length` on a string and on a list, `@range`),
/// reached through a different member and a different operation on each row.
fn constraint_rows(ethereum: &NetworkFixture, bitcoin: &NetworkFixture) -> Vec<Refused> {
    let network = ethereum.network.as_str();
    let address = ethereum.busiest_address();
    let hash = ethereum.transactions[0].transaction_hash.clone();
    let valid_item = json!({
        "tokenIdentifier": {"network": network, "tokenId": "eth"},
        "ownerIdentifier": {"address": address},
    });
    // One character past `NextToken`'s `@length` maximum. A value that long is echoed
    // into the clause elided, as the framework elides one.
    let long_token = "t".repeat(NEXT_TOKEN_MAX_LENGTH as usize + 1);
    let elided = format!("{}... [{} bytes]", &long_token[..256], long_token.len());

    let mut rows = vec![
        // --- @pattern, on every shape that declares one and through several members ---
        Refused {
            case: "GetAssetContract: a contract address shorter than ChainAddress admits"
                .to_string(),
            route: "/get-asset-contract",
            body: json!({"contractIdentifier": {"network": network, "contractAddress": "0x123"}}),
            member: "contractIdentifier.contractAddress",
            value: Some("0x123".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        Refused {
            case: "GetTokenBalance: an owner address shorter than ChainAddress admits"
                .to_string(),
            route: "/get-token-balance",
            body: json!({
                "tokenIdentifier": {"network": network, "tokenId": "eth"},
                "ownerIdentifier": {"address": "short"},
            }),
            member: "ownerIdentifier.address",
            value: Some("short".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        Refused {
            case: "GetTokenBalance: a contract address outside ChainAddress".to_string(),
            route: "/get-token-balance",
            body: json!({
                "tokenIdentifier": {"network": network, "contractAddress": "under13"},
                "ownerIdentifier": {"address": address},
            }),
            member: "tokenIdentifier.contractAddress",
            value: Some("under13".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        Refused {
            case: "GetTokenBalance: a tokenId outside QueryTokenId".to_string(),
            route: "/get-token-balance",
            body: json!({
                "tokenIdentifier": {"network": network, "tokenId": "not a token id"},
                "ownerIdentifier": {"address": address},
            }),
            member: "tokenIdentifier.tokenId",
            value: Some("not a token id".to_string()),
            requirement: pattern_requirement(QUERY_TOKEN_ID),
        },
        Refused {
            case: "GetTransaction: a hash that is not 64 hex digits".to_string(),
            route: "/get-transaction",
            body: json!({"network": network, "transactionHash": "not-a-hash"}),
            member: "transactionHash",
            value: Some("not-a-hash".to_string()),
            requirement: pattern_requirement(TRANSACTION_HASH),
        },
        Refused {
            case: "GetTransaction: a transactionId that is not 64 hex digits, on the \
                   network that accepts one"
                .to_string(),
            route: "/get-transaction",
            body: json!({"network": bitcoin.network, "transactionId": "0xnot-hex"}),
            member: "transactionId",
            value: Some("0xnot-hex".to_string()),
            requirement: pattern_requirement(TRANSACTION_HASH),
        },
        Refused {
            case: "ListTransactionEvents: 64 characters that are not hex".to_string(),
            route: "/list-transaction-events",
            body: json!({"network": network, "transactionHash": "g".repeat(64)}),
            member: "transactionHash",
            value: Some("g".repeat(64)),
            requirement: pattern_requirement(TRANSACTION_HASH),
        },
        Refused {
            case: "ListTransactions: an address outside ChainAddress".to_string(),
            route: "/list-transactions",
            body: json!({"network": network, "address": "tooshort"}),
            member: "address",
            value: Some("tooshort".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        Refused {
            case: "ListAssetContracts: a deployer address outside ChainAddress".to_string(),
            route: "/list-asset-contracts",
            body: json!({"contractFilter": {
                "network": network,
                "tokenStandard": "ERC20",
                "deployerAddress": "0x0",
            }}),
            member: "contractFilter.deployerAddress",
            value: Some("0x0".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        Refused {
            case: "ListTokenBalances: an owner filter address outside ChainAddress".to_string(),
            route: "/list-token-balances",
            body: json!({
                "tokenFilter": {"network": network, "tokenId": "eth"},
                "ownerFilter": {"address": "x"},
            }),
            member: "ownerFilter.address",
            value: Some("x".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        Refused {
            case: "ListFilteredTransactionEvents: a list ELEMENT outside ChainAddress, \
                   reported at the element's own position"
                .to_string(),
            route: "/list-filtered-transaction-events",
            body: json!({
                "network": network,
                "addressIdentifierFilter": {"transactionEventToAddress": ["short"]},
            }),
            member: "addressIdentifierFilter.transactionEventToAddress.1.member",
            value: Some("short".to_string()),
            requirement: pattern_requirement(CHAIN_ADDRESS),
        },
        // --- an enum value set, on every enumerated input shape ---
        Refused {
            case: "GetTransaction: a network outside QueryNetwork".to_string(),
            route: "/get-transaction",
            body: json!({"network": "NOT_A_NETWORK", "transactionHash": hash}),
            member: "network",
            value: Some("NOT_A_NETWORK".to_string()),
            requirement: enum_requirement(QUERY_NETWORK),
        },
        Refused {
            case: "GetTokenBalance: a network outside QueryNetwork, nested in \
                   TokenIdentifier"
                .to_string(),
            route: "/get-token-balance",
            body: json!({
                "tokenIdentifier": {"network": "NOT_A_NETWORK", "tokenId": "eth"},
                "ownerIdentifier": {"address": address},
            }),
            member: "tokenIdentifier.network",
            value: Some("NOT_A_NETWORK".to_string()),
            requirement: enum_requirement(QUERY_NETWORK),
        },
        Refused {
            case: "ListTokenBalances: a network whose spelling differs only in case"
                .to_string(),
            route: "/list-token-balances",
            body: json!({"tokenFilter": {"network": "ethereum_mainnet", "tokenId": "eth"}}),
            member: "tokenFilter.network",
            value: Some("ethereum_mainnet".to_string()),
            requirement: enum_requirement(QUERY_NETWORK),
        },
        Refused {
            case: "ListAssetContracts: a token standard outside QueryTokenStandard"
                .to_string(),
            route: "/list-asset-contracts",
            body: json!({"contractFilter": {
                "network": network,
                "tokenStandard": "NOT_A_STANDARD",
                "deployerAddress": address,
            }}),
            member: "contractFilter.tokenStandard",
            value: Some("NOT_A_STANDARD".to_string()),
            requirement: enum_requirement(QUERY_TOKEN_STANDARD),
        },
        Refused {
            case: "ListTransactions: a sort order outside SortOrder".to_string(),
            route: "/list-transactions",
            body: json!({
                "network": network,
                "address": address,
                "sort": {"sortOrder": "SIDEWAYS"},
            }),
            member: "sort.sortOrder",
            value: Some("SIDEWAYS".to_string()),
            requirement: enum_requirement(SORT_ORDER),
        },
        Refused {
            case: "ListTransactions: a sortBy outside ListTransactionsSortBy".to_string(),
            route: "/list-transactions",
            body: json!({
                "network": network,
                "address": address,
                "sort": {"sortBy": "TRANSACTION_COST"},
            }),
            member: "sort.sortBy",
            value: Some("TRANSACTION_COST".to_string()),
            requirement: enum_requirement(LIST_TRANSACTIONS_SORT_BY),
        },
        Refused {
            case: "ListFilteredTransactionEvents: a sortBy outside its own sort shape"
                .to_string(),
            route: "/list-filtered-transaction-events",
            body: json!({
                "network": network,
                "addressIdentifierFilter": {"transactionEventToAddress": [address]},
                "sort": {"sortBy": "transactionTimestamp"},
            }),
            member: "sort.sortBy",
            value: Some("transactionTimestamp".to_string()),
            requirement: enum_requirement(LIST_FILTERED_TRANSACTION_EVENTS_SORT_BY),
        },
        Refused {
            case: "ListTransactions: a confirmation status ELEMENT outside \
                   ConfirmationStatus"
                .to_string(),
            route: "/list-transactions",
            body: json!({
                "network": network,
                "address": address,
                "confirmationStatusFilter": {"include": ["MAYBE"]},
            }),
            member: "confirmationStatusFilter.include.1.member",
            value: Some("MAYBE".to_string()),
            requirement: enum_requirement(CONFIRMATION_STATUS),
        },
        // --- @length, on a string and on a list, at both bounds ---
        Refused {
            case: "ListTransactions: a nextToken one character past NextToken's maximum"
                .to_string(),
            route: "/list-transactions",
            body: json!({"network": network, "address": address, "nextToken": long_token}),
            member: "nextToken",
            value: Some(elided),
            requirement: max_length_requirement(NEXT_TOKEN_MAX_LENGTH),
        },
        Refused {
            case: "ListTransactions: an empty confirmation-status list".to_string(),
            route: "/list-transactions",
            body: json!({
                "network": network,
                "address": address,
                "confirmationStatusFilter": {"include": []},
            }),
            member: "confirmationStatusFilter.include",
            // A collection's SIZE failed, and the collection is not a value the caller
            // submitted, so the clause names the member alone.
            value: None,
            requirement: min_length_requirement(1),
        },
        Refused {
            case: "ListFilteredTransactionEvents: two addresses where the list admits one"
                .to_string(),
            route: "/list-filtered-transaction-events",
            body: json!({
                "network": network,
                "addressIdentifierFilter": {"transactionEventToAddress": [address, address]},
            }),
            member: "addressIdentifierFilter.transactionEventToAddress",
            value: None,
            requirement: max_length_requirement(1),
        },
        Refused {
            case: "BatchGetTokenBalance: an empty item list".to_string(),
            route: "/batch-get-token-balance",
            body: json!({"getTokenBalanceInputs": []}),
            member: "getTokenBalanceInputs",
            value: None,
            requirement: min_length_requirement(BATCH_LENGTH.0),
        },
        Refused {
            case: "BatchGetTokenBalance: one item past the list's maximum".to_string(),
            route: "/batch-get-token-balance",
            body: json!({
                "getTokenBalanceInputs":
                    vec![valid_item.clone(); BATCH_LENGTH.1 as usize + 1],
            }),
            member: "getTokenBalanceInputs",
            value: None,
            requirement: max_length_requirement(BATCH_LENGTH.1),
        },
        // --- @range, one past each published bound ---
        Refused {
            case: "ListTransactions: maxResults one below the range".to_string(),
            route: "/list-transactions",
            body: json!({"network": network, "address": address, "maxResults": 0}),
            member: "maxResults",
            value: Some("0".to_string()),
            requirement: min_value_requirement(MAX_RESULTS_RANGE.0),
        },
        Refused {
            case: "ListTransactions: maxResults one above the range".to_string(),
            route: "/list-transactions",
            body: json!({"network": network, "address": address, "maxResults": 251}),
            member: "maxResults",
            value: Some("251".to_string()),
            requirement: max_value_requirement(MAX_RESULTS_RANGE.1),
        },
        Refused {
            case: "ListTokenBalances: a negative maxResults".to_string(),
            route: "/list-token-balances",
            body: json!({
                "tokenFilter": {"network": network, "tokenId": "eth"},
                "maxResults": -1,
            }),
            member: "maxResults",
            value: Some("-1".to_string()),
            requirement: min_value_requirement(MAX_RESULTS_RANGE.0),
        },
    ];

    // The same `@pattern` through the Bitcoin networks' own address spelling: a value
    // the model admits anywhere is refused nowhere, and one it refuses is refused on
    // every network.
    rows.push(Refused {
        case: "ListTransactions on a Bitcoin network: an address outside ChainAddress"
            .to_string(),
        route: "/list-transactions",
        body: json!({"network": bitcoin.network, "address": "1Short"}),
        member: "address",
        value: Some("1Short".to_string()),
        requirement: pattern_requirement(CHAIN_ADDRESS),
    });
    rows
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_with_reason_and_a_field_list() {
    let emulator = Emulator::new();
    let (ethereum, bitcoin) = networks(&emulator);

    for row in constraint_rows(&ethereum, &bitcoin) {
        let case = &row.case;
        let body = emulator
            .fails(row.route, &row.body, 400, "ValidationException")
            .await;

        let expected = one_validation_error(row.value.as_deref(), row.member, &row.requirement);
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the AWS validation report, got {body:?}"
        );
        // `reason` is `@required` on the modelled `ValidationException`, and it is the
        // discriminator a client switches on.
        assert_eq!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {body:?}"
        );
        // `fieldList` is the machine-readable half: one entry naming the member that
        // was wrong, carrying that member's own clause.
        assert_eq!(
            body["fieldList"],
            json!([{
                "name": row.member,
                "message": clause(row.value.as_deref(), row.member, &row.requirement),
            }]),
            "case {case:?}: expected one fieldList entry for {}, got {body:?}",
            row.member
        );
        // A `@pattern` is never quoted with the model's anchors.
        for anchored in [
            CHAIN_ADDRESS_ANCHORED,
            QUERY_TOKEN_ID_ANCHORED,
            TRANSACTION_HASH_ANCHORED,
        ] {
            assert!(
                !body.to_string().contains(anchored),
                "case {case:?}: a pattern is quoted unanchored, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_reported_value_set_is_the_models_own() {
    // The value sets the rows above build their expected messages from are written in
    // the model's DECLARATION order, which the generated `*_VALUES` constants do not
    // keep (the generator sorts them). Their MEMBERSHIP is the model's, though, so each
    // set is checked against the generated constant here: an enum the model grows, or a
    // value it drops, fails this test instead of silently diverging in both the service
    // and the table.
    for (shape, ordered, generated) in [
        ("QueryNetwork", QUERY_NETWORK, QUERY_NETWORK_VALUES),
        (
            "QueryTokenStandard",
            QUERY_TOKEN_STANDARD,
            QUERY_TOKEN_STANDARD_VALUES,
        ),
        (
            "ConfirmationStatus",
            CONFIRMATION_STATUS,
            CONFIRMATION_STATUS_VALUES,
        ),
        ("SortOrder", SORT_ORDER, SORT_ORDER_VALUES),
    ] {
        let mut mine: Vec<&str> = ordered.to_vec();
        mine.sort_unstable();
        let mut theirs: Vec<&str> = generated.to_vec();
        theirs.sort_unstable();
        assert_eq!(
            mine, theirs,
            "case {shape}: the table's value set must be the model's, got {mine:?} \
             against the generated {theirs:?}"
        );
    }
}

#[tokio::test]
async fn a_request_with_many_bad_values_is_answered_with_a_bounded_report() {
    let emulator = Emulator::new();
    let (ethereum, _) = networks(&emulator);
    let address = ethereum.busiest_address();

    // `ConfirmationStatusFilter$include` is the one list whose `@length` declares no
    // maximum, so it is the member a caller could use to make the report grow with the
    // request. A thousand bad elements must still be answered with a report bounded by
    // the service's own ceiling, not by the request.
    let include: Vec<&str> = std::iter::repeat_n("NOT_A_STATUS", 1_000).collect();
    let body = emulator
        .fails(
            "/list-transactions",
            &json!({
                "network": ethereum.network,
                "address": address,
                "confirmationStatusFilter": {"include": include},
            }),
            400,
            "ValidationException",
        )
        .await;
    let fields = body["fieldList"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert!(
        fields > 0 && fields <= 100,
        "case 1000 refused statuses: expected a bounded fieldList, got {fields} entries"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("100 validation errors detected: "),
        "case 1000 refused statuses: expected the report to stop at the ceiling, \
         got {message:?}"
    );
}

#[tokio::test]
async fn a_value_the_published_bounds_admit_is_not_refused() {
    let emulator = Emulator::new();
    let (ethereum, _) = networks(&emulator);
    let address = ethereum.busiest_address();

    // Both published `@range` bounds of `maxResults`, and the shortest and longest
    // value `ChainAddress` admits — the rows one past which are refused above.
    for max_results in [MAX_RESULTS_RANGE.0, MAX_RESULTS_RANGE.1] {
        emulator
            .ok(
                "/list-transactions",
                &json!({
                    "network": ethereum.network,
                    "address": address,
                    "maxResults": max_results,
                }),
            )
            .await;
    }
    for length in [13usize, 74] {
        let admitted = "a".repeat(length);
        let (status, body) = emulator
            .post(
                "/list-transactions",
                &json!({"network": ethereum.network, "address": admitted}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {length}-character address: inside ChainAddress, got {status} with {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a rule the model cannot express
// ---------------------------------------------------------------------------

/// One row of the service-rule table: a request that satisfies every modelled
/// constraint and breaks a rule only this service knows, and the `fieldList` name the
/// refusal has to carry.
struct Unsupported {
    case: String,
    route: &'static str,
    body: Value,
    field: &'static str,
}

/// Every rule of its own this service enforces, on every operation that enforces one.
fn service_rule_rows(ethereum: &NetworkFixture, bitcoin: &NetworkFixture) -> Vec<Unsupported> {
    let network = ethereum.network.as_str();
    let address = ethereum.busiest_address();
    let hash = ethereum.transactions[0].transaction_hash.clone();
    let some_id = "a".repeat(64);
    let opened_at = ethereum.transactions[0].transaction_timestamp;

    let mut rows = Vec::new();
    for route in ["/get-transaction", "/list-transaction-events"] {
        rows.push(Unsupported {
            case: format!("{route}: neither identifier"),
            route,
            body: json!({"network": network}),
            field: "transactionHash",
        });
        rows.push(Unsupported {
            case: format!("{route}: both identifiers"),
            route,
            body: json!({
                "network": network,
                "transactionHash": hash,
                "transactionId": some_id,
            }),
            field: "transactionHash",
        });
        rows.push(Unsupported {
            case: format!("{route}: a transactionId off the Bitcoin networks"),
            route,
            body: json!({"network": network, "transactionId": some_id}),
            field: "transactionId",
        });
    }
    // Only the listing is `@paginated`, so only it takes a `nextToken` to refuse.
    rows.push(Unsupported {
        case: "ListTransactionEvents: a nextToken this service never issued".to_string(),
        route: "/list-transaction-events",
        body: json!({
            "network": network,
            "transactionHash": hash,
            "nextToken": "not-a-token-this-service-issued",
        }),
        field: "nextToken",
    });
    rows.push(Unsupported {
        case: "GetTokenBalance: a tokenIdentifier that names no token".to_string(),
        route: "/get-token-balance",
        body: json!({
            "tokenIdentifier": {"network": network},
            "ownerIdentifier": {"address": address},
        }),
        field: "tokenIdentifier",
    });
    rows.push(Unsupported {
        case: "ListTokenBalances: a filter that narrows the network by nothing".to_string(),
        route: "/list-token-balances",
        body: json!({"tokenFilter": {"network": network}}),
        field: "tokenFilter",
    });
    rows.push(Unsupported {
        case: "ListTransactions: a window that ends before it starts".to_string(),
        route: "/list-transactions",
        body: json!({
            "network": network,
            "address": address,
            "fromBlockchainInstant": {"time": opened_at + 600},
            "toBlockchainInstant": {"time": opened_at},
        }),
        // Both members target `BlockchainInstant`, and the rule is about the pair.
        field: "BlockchainInstant",
    });
    rows.push(Unsupported {
        case: "GetAssetContract: a Bitcoin network, which has no asset contracts"
            .to_string(),
        route: "/get-asset-contract",
        body: json!({"contractIdentifier": {
            "network": bitcoin.network,
            "contractAddress": bitcoin.busiest_address(),
        }}),
        field: "contractIdentifier.network",
    });
    rows.push(Unsupported {
        case: "ListAssetContracts: a Bitcoin network, which has no asset contracts"
            .to_string(),
        route: "/list-asset-contracts",
        body: json!({"contractFilter": {
            "network": bitcoin.network,
            "tokenStandard": "ERC721",
            "deployerAddress": bitcoin.busiest_address(),
        }}),
        field: "contractFilter.network",
    });
    rows.push(Unsupported {
        case: "ListFilteredTransactionEvents: a network outside the member's published \
               Bitcoin-only value set"
            .to_string(),
        route: "/list-filtered-transaction-events",
        body: json!({
            "network": network,
            "addressIdentifierFilter": {"transactionEventToAddress": [address]},
        }),
        field: "network",
    });
    rows
}

#[tokio::test]
async fn a_rule_the_model_cannot_express_is_reported_in_the_field_list() {
    let emulator = Emulator::new();
    let (ethereum, bitcoin) = networks(&emulator);

    for row in service_rule_rows(&ethereum, &bitcoin) {
        let case = &row.case;
        let body = emulator
            .fails(row.route, &row.body, 400, "ValidationException")
            .await;

        // The top-level message is the generic sentinel: the detail belongs in
        // `fieldList`, not in prose a client would have to match on.
        assert_eq!(
            body["message"].as_str(),
            Some(PARAMETER_VALUES_INVALID),
            "case {case:?}: expected the generic message, got {body:?}"
        );
        assert_eq!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {body:?}"
        );
        let fields = body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {body:?}"));
        // One entry per violated rule — a request can break more than one at once (both
        // identifiers on a network that admits no `transactionId`), and the rule this
        // row is about has to be among them.
        let reported = fields
            .iter()
            .find(|field| field["name"].as_str() == Some(row.field))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a fieldList entry naming {}, got {body:?}",
                    row.field
                )
            });
        let explanation = reported["message"].as_str().unwrap_or_default();
        assert!(
            !explanation.is_empty() && explanation != PARAMETER_VALUES_INVALID,
            "case {case:?}: the entry explains the rule, got {body:?}"
        );
        // The explanation is a property of the RULE, so it never echoes the value the
        // request sent.
        assert!(
            !explanation.contains("not-a-token-this-service-issued"),
            "case {case:?}: an explanation does not echo the request, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a batch reports a per-item failure inside its 200
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_get_token_balance_answers_the_good_items_and_reports_the_bad_ones() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // Rows: the holdings the ledger really has, each answered; and four kinds of
        // item that cannot be answered — a value outside an enum, a value outside a
        // `@pattern`, an identifier that names no token, and an owner the ledger holds
        // nothing for.
        let good: Vec<Value> = network
            .holdings
            .iter()
            .take(3)
            .map(|(key, _)| {
                let mut token = json!({"network": network.network});
                if let Some(contract) = &key.contract_address {
                    token["contractAddress"] = json!(contract);
                }
                if let Some(token_id) = &key.token_id {
                    token["tokenId"] = json!(token_id);
                }
                json!({
                    "tokenIdentifier": token,
                    "ownerIdentifier": {"address": key.owner_address},
                })
            })
            .collect();
        assert!(
            !good.is_empty(),
            "case {}: the seeded ledger holds no balance",
            network.network
        );
        let stranger = "0x0000000000000000000000000000000000000000";
        let bad = [
            (
                json!({
                    "tokenIdentifier": {"network": "NOT_A_NETWORK", "tokenId": "eth"},
                    "ownerIdentifier": {"address": network.busiest_address()},
                }),
                "VALIDATION_EXCEPTION",
                "400",
            ),
            (
                json!({
                    "tokenIdentifier": {"network": network.network, "tokenId": "eth"},
                    "ownerIdentifier": {"address": "short"},
                }),
                "VALIDATION_EXCEPTION",
                "400",
            ),
            (
                json!({
                    "tokenIdentifier": {"network": network.network},
                    "ownerIdentifier": {"address": network.busiest_address()},
                }),
                "VALIDATION_EXCEPTION",
                "400",
            ),
            (
                json!({
                    "tokenIdentifier": {"network": network.network, "tokenId": "eth"},
                    "ownerIdentifier": {"address": stranger},
                }),
                "RESOURCE_NOT_FOUND_EXCEPTION",
                "404",
            ),
        ];

        let mut items = good.clone();
        items.extend(bad.iter().map(|(item, _, _)| item.clone()));
        let reply = emulator
            .ok(
                "/batch-get-token-balance",
                &json!({"getTokenBalanceInputs": items}),
            )
            .await;
        let case = format!("{} mixed batch", network.network);

        // Every good item is answered, and with exactly what the single read answers:
        // the batch and `GetTokenBalance` share one read of the ledger.
        let balances = reply["tokenBalances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: tokenBalances is required, got {reply:?}"));
        assert_eq!(
            balances.len(),
            good.len(),
            "case {case:?}: expected every good item answered, got {reply:?}"
        );
        for (position, item) in good.iter().enumerate() {
            let single = emulator.ok("/get-token-balance", item).await;
            assert_eq!(
                balances[position], single,
                "case {case:?} item {position}: expected the single read's answer {single:?}, \
                 got {:?}",
                balances[position]
            );
        }

        // Every bad item is reported in `errors`, echoing the identifiers it was asked
        // about, so a caller can tell which of its items failed.
        let errors = reply["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: errors is required, got {reply:?}"));
        assert_eq!(
            errors.len(),
            bad.len(),
            "case {case:?}: expected one entry per refused item, got {reply:?}"
        );
        for (position, (item, error_type, error_code)) in bad.iter().enumerate() {
            let reported = &errors[position];
            assert_eq!(
                reported["errorType"].as_str(),
                Some(*error_type),
                "case {case:?} error {position}: expected errorType {error_type}, got {reported:?}"
            );
            assert_eq!(
                reported["errorCode"].as_str(),
                Some(*error_code),
                "case {case:?} error {position}: expected errorCode {error_code}, got {reported:?}"
            );
            assert_eq!(
                reported["tokenIdentifier"], item["tokenIdentifier"],
                "case {case:?} error {position}: expected the item's tokenIdentifier echoed, \
                 got {reported:?}"
            );
            assert_eq!(
                reported["ownerIdentifier"], item["ownerIdentifier"],
                "case {case:?} error {position}: expected the item's ownerIdentifier echoed, \
                 got {reported:?}"
            );
            assert!(
                reported["errorMessage"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?} error {position}: errorMessage is required, got {reported:?}"
            );
        }

        // The item refused for its address carries the clause that member's own
        // violation states, with the path relative to the ITEM: the entry already names
        // which item it was, by echoing its identifiers.
        let refused_address = &errors[1];
        assert_eq!(
            refused_address["errorMessage"].as_str(),
            Some(
                one_validation_error(
                    Some("short"),
                    "ownerIdentifier.address",
                    &pattern_requirement(CHAIN_ADDRESS),
                )
                .as_str()
            ),
            "case {case:?}: expected the item's own validation report, got {refused_address:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_that_names_no_item_is_answered_with_two_empty_lists() {
    let emulator = Emulator::new();
    // `getTokenBalanceInputs` is modelled OPTIONAL, so an absent list breaks no bound
    // and asks for no balance; `tokenBalances` and `errors` are both `@required`, so
    // both are still there.
    let reply = emulator.ok("/batch-get-token-balance", &json!({})).await;
    assert_eq!(
        reply,
        json!({"tokenBalances": [], "errors": []}),
        "case absent item list: expected two empty lists, got {reply:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: one not-found message, with the identity in its own members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_not_found_states_the_same_message_and_carries_the_identity() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // Rows: a well-formed hash no ledger holds, a well-formed owner that holds
        // nothing, and (on the networks that have contracts) a well-formed contract
        // address the ledger does not know.
        let unknown_hash = format!("0x{}", "0".repeat(64));
        let stranger = "0x0000000000000000000000000000000000000000";
        let mut rows = vec![
            (
                "GetTransaction".to_string(),
                "/get-transaction",
                json!({"network": network.network, "transactionHash": unknown_hash}),
            ),
            (
                "GetTokenBalance".to_string(),
                "/get-token-balance",
                json!({
                    "tokenIdentifier": {"network": network.network, "tokenId": "eth"},
                    "ownerIdentifier": {"address": stranger},
                }),
            ),
        ];
        if !network.contracts.is_empty() {
            rows.push((
                "GetAssetContract".to_string(),
                "/get-asset-contract",
                json!({"contractIdentifier": {
                    "network": network.network,
                    "contractAddress": stranger,
                }}),
            ));
        }

        for (operation, route, body) in rows {
            let case = format!("{}/{operation}", network.network);
            let reply = emulator
                .fails(route, &body, 404, "ResourceNotFoundException")
                .await;
            assert_eq!(
                reply["message"].as_str(),
                Some(RESOURCE_NOT_FOUND),
                "case {case:?}: expected the service's constant not-found message, got {reply:?}"
            );
            // The identity is in the error's own members, which is why the message does
            // not need to carry it — and must not, since it is the same for every
            // caller.
            assert!(
                reply["resourceId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: resourceId is required, got {reply:?}"
            );
            assert_eq!(
                reply["resourceType"].as_str(),
                Some(RESOURCE_TYPE),
                "case {case:?}: expected the only modelled resourceType, got {reply:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The asset-contract reads, proved against the ledger they answer from
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_listed_asset_contract_is_the_one_get_asset_contract_returns() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for address in &network.contracts {
            let case = format!("{}/{address}", network.network);
            let identifier = json!({
                "network": network.network,
                "contractAddress": address,
            });

            // What the contract IS, as the read reports it: the listing has to agree
            // with this record, which is the one the ledger holds.
            let read = emulator
                .ok(
                    "/get-asset-contract",
                    &json!({"contractIdentifier": identifier}),
                )
                .await;
            assert_eq!(
                read["contractIdentifier"], identifier,
                "case {case:?}: expected the contract asked for, got {read:?}"
            );
            let standard = read["tokenStandard"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: tokenStandard is required, got {read:?}"))
                .to_string();
            let deployer = read["deployerAddress"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: deployerAddress is required, got {read:?}")
                })
                .to_string();
            assert!(
                QUERY_TOKEN_STANDARD.contains(&standard.as_str()),
                "case {case:?}: tokenStandard is a QueryTokenStandard value, got {standard:?}"
            );
            let summary = json!({
                "contractIdentifier": identifier,
                "tokenStandard": standard,
                "deployerAddress": deployer,
            });

            // The listing narrows by every member of `ContractFilter`, not just the
            // address: with the deployer fixed, the contract under test is listed under
            // its OWN standard and under no other, and whatever each standard lists
            // matches that filter.
            for candidate in QUERY_TOKEN_STANDARD {
                let listed = emulator
                    .ok(
                        "/list-asset-contracts",
                        &json!({"contractFilter": {
                            "network": network.network,
                            "tokenStandard": candidate,
                            "deployerAddress": deployer,
                        }}),
                    )
                    .await;
                let contracts = listed["contracts"].as_array().unwrap_or_else(|| {
                    panic!("case {case:?}/{candidate}: contracts is required, got {listed:?}")
                });
                assert_eq!(
                    contracts.contains(&summary),
                    *candidate == standard,
                    "case {case:?}/{candidate}: the contract is listed under its own \
                     standard and no other, got {listed:?}"
                );
                for item in contracts {
                    assert_eq!(
                        item["tokenStandard"].as_str(),
                        Some(*candidate),
                        "case {case:?}/{candidate}: every listed contract matches the \
                         filter's standard, got {item:?}"
                    );
                    assert_eq!(
                        item["deployerAddress"].as_str(),
                        Some(deployer.as_str()),
                        "case {case:?}/{candidate}: every listed contract matches the \
                         filter's deployer, got {item:?}"
                    );
                    assert_eq!(
                        item["contractIdentifier"]["network"].as_str(),
                        Some(network.network.as_str()),
                        "case {case:?}/{candidate}: every listed contract is on the \
                         filter's network, got {item:?}"
                    );
                }
                // The fixture ledger is far smaller than the modelled page size, so a
                // listing of it is exhausted in one page and carries no token.
                assert!(
                    listed.get("nextToken").is_none(),
                    "case {case:?}/{candidate}: an exhausted listing carries no token, \
                     got {listed:?}"
                );
            }

            // RULE: an address NAMES an account, and for the hex-encoded addresses of an
            // EVM network the letter case of a `ChainAddress` is an EIP-55 checksum over
            // the same 20 bytes, not part of the name — so a filter selects the same
            // contracts however the caller spelled the deployer, exactly as
            // `GetAssetContract` resolves either spelling of a contract address. Rows:
            // the spelling the read reports, plus the two case extremes of it (both are
            // still values the model's `ChainAddress` pattern admits, `[-A-Za-z0-9]{13,74}`
            // being case-blind).
            let mut selections = Vec::new();
            for spelling in [
                deployer.clone(),
                deployer.to_ascii_uppercase(),
                deployer.to_ascii_lowercase(),
            ] {
                let listed = emulator
                    .ok(
                        "/list-asset-contracts",
                        &json!({"contractFilter": {
                            "network": network.network,
                            "tokenStandard": standard,
                            "deployerAddress": spelling,
                        }}),
                    )
                    .await;
                selections.push((spelling, listed));
            }
            let (baseline_spelling, baseline) = &selections[0];
            for (spelling, selected) in &selections[1..] {
                assert_eq!(
                    selected, baseline,
                    "case {case:?}: the deployer filter {spelling:?} must select what \
                     {baseline_spelling:?} selects, got {selected:?}"
                );
            }
        }
    }
}

/// The two networks a table row needs: one Ethereum network (which has contracts) and
/// one Bitcoin network (which has none, and is the only one that accepts a
/// `transactionId`). Both are read out of the ledger the emulator serves.
fn networks(emulator: &Emulator) -> (NetworkFixture, NetworkFixture) {
    let mut ethereum = None;
    let mut bitcoin = None;
    for network in emulator.fixture() {
        if network.network.starts_with("BITCOIN_") {
            bitcoin = bitcoin.or(Some(network));
        } else {
            ethereum = ethereum.or(Some(network));
        }
    }
    (
        ethereum.expect("the fixture holds an Ethereum network"),
        bitcoin.expect("the fixture holds a Bitcoin network"),
    )
}

/// Every refusal in this suite goes through [`Emulator::fails`], which already asserts
/// the wire error code; this keeps the helper named so a reader can find it.
#[tokio::test]
async fn a_refusal_names_the_modelled_error_shape() {
    let emulator = Emulator::new();
    let (ethereum, _) = networks(&emulator);
    let body = emulator
        .post(
            "/get-transaction",
            &json!({"network": ethereum.network, "transactionHash": "not-a-hash"}),
        )
        .await;
    assert_eq!(
        error_code(&body.1),
        "ValidationException",
        "case a pattern refusal: expected the modelled error code, got {body:?}"
    );
}

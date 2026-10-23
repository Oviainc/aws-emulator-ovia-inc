// SPDX-License-Identifier: CC0-1.0

//! The error contract of the ManagedBlockchain Query emulator: HOW a refused or
//! unresolvable request is rendered, driven over HTTP through the same
//! `MultiServiceRouter` the production front door serves (the [`Emulator`] harness the
//! sibling suite stands up).
//!
//! Four rules are under test, and each one is asserted over a TABLE spanning the class
//! it describes rather than over the one request that exposed it:
//!
//! 1. A value that fails a constraint the MODEL declares is refused with HTTP 400
//!    `ValidationException` whose message is AWS's validation-framework envelope — one
//!    clause per violation naming the submitted value, the member's dotted path and the
//!    constraint — and a `@pattern` clause quotes the expression itself, never the
//!    model's `^`/`$`-anchored spelling.
//! 2. A request this SERVICE refuses on a rule the model cannot express carries the
//!    generic top-level message `One or more parameter values were invalid.`, the
//!    modelled `reason`, and one `fieldList` entry per violated rule.
//! 3. A resource the ledger does not hold is a 404 `ResourceNotFoundException` with a
//!    fixed, request-independent message; the identity travels in
//!    `resourceId`/`resourceType`.
//! 4. `ListFilteredTransactionEvents` serves only the two Bitcoin networks its `network`
//!    member documents, and refuses every other value before anything else happens.
//!
//! Every pattern, bound and value set a row states is the model's own, named in the
//! comment beside it; nothing is copied out of the implementation.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::QUERY_NETWORK_VALUES;

use crate::chain_query_slice::{error_code, unseeded_bitcoin_address, Emulator};

/// `ChainAddress`'s `@pattern`, as a message must report it: the model spells it
/// `^[-A-Za-z0-9]{13,74}$` and the anchors are not part of the expression a value has to
/// satisfy.
const CHAIN_ADDRESS_PATTERN: &str = "[-A-Za-z0-9]{13,74}";

/// `QueryTokenId`'s `@pattern` (model: `^[a-zA-Z0-9]{1,66}$`), unanchored.
const TOKEN_ID_PATTERN: &str = "[a-zA-Z0-9]{1,66}";

/// `QueryTransactionHash`'s and `QueryTransactionId`'s `@pattern` (model:
/// `^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$`), unanchored.
const TRANSACTION_PATTERN: &str = "(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})";

/// An address that satisfies `ChainAddress` — 42 characters of `[-A-Za-z0-9]`, inside
/// the modelled 13..74 — so a row can violate exactly one constraint at a time.
const VALID_ADDRESS: &str = "0x28c6c06298d514db089934071355e5743bf21d60";

/// A `QueryTransactionHash`/`QueryTransactionId`: 64 hex digits, no `0x` prefix. Well
/// formed and absent from the seeded ledger, which is what makes it a 404 rather than a
/// 400.
const UNMINED_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The top-level `message` this service's `ValidationException` carries for a request it
/// refuses on its own rules — generic, with the detail in `fieldList`.
const GENERIC_VALIDATION_MESSAGE: &str = "One or more parameter values were invalid.";

/// The `message` every `ResourceNotFoundException` carries, whatever was looked up.
const GENERIC_NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// `ValidationExceptionReason`'s value for a member whose value was refused (the model's
/// own value list: `unknownOperation | cannotParse | fieldValidationFailed | other`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The only value `ResourceType` declares.
const RESOURCE_TYPE: &str = "collection";

/// The two values `ListFilteredTransactionEventsInput$network` documents ("Valid Values:
/// BITCOIN_MAINNET | BITCOIN_TESTNET"), restated here from the model rather than read out
/// of the service.
const BITCOIN_NETWORKS: [&str; 2] = ["BITCOIN_MAINNET", "BITCOIN_TESTNET"];

// ---------------------------------------------------------------------------
// Rule 1: constraint refusals are rendered in AWS's validation-framework form
// ---------------------------------------------------------------------------

/// One constraint-refusal row: a request that violates exactly ONE modelled constraint,
/// and the whole clause the message must state for it.
struct Clause {
    route: &'static str,
    body: Value,
    /// The text after `1 validation error detected: `.
    clause: String,
}

/// A `@pattern` row: `value` at `path` must be reported against `pattern`.
fn pattern_case(route: &'static str, body: Value, path: &str, value: &str, pattern: &str) -> Clause {
    Clause {
        route,
        body,
        clause: format!(
            "Value '{value}' at '{path}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {pattern}"
        ),
    }
}

/// A `BatchGetTokenBalanceInputItem` every modelled constraint admits.
fn batch_item(network: &str, address: &str) -> Value {
    json!({
        "tokenIdentifier": {"network": network, "tokenId": "eth"},
        "ownerIdentifier": {"address": address},
    })
}

/// Every member of this service that carries a `@pattern`, one row per member, reached
/// through the operation that takes it: a scalar member, a nested one, and a list
/// element, across six operations.
fn pattern_cases() -> Vec<Clause> {
    vec![
        // ContractIdentifier$contractAddress -> ChainAddress.
        pattern_case(
            "/get-asset-contract",
            json!({"contractIdentifier": {
                "network": "ETHEREUM_MAINNET",
                "contractAddress": "!!!not-an-address!!!",
            }}),
            "contractIdentifier.contractAddress",
            "!!!not-an-address!!!",
            CHAIN_ADDRESS_PATTERN,
        ),
        // TokenFilter$contractAddress -> ChainAddress.
        pattern_case(
            "/list-token-balances",
            json!({"tokenFilter": {
                "network": "ETHEREUM_MAINNET",
                "contractAddress": "not-an-address!",
            }}),
            "tokenFilter.contractAddress",
            "not-an-address!",
            CHAIN_ADDRESS_PATTERN,
        ),
        // TokenFilter$tokenId -> QueryTokenId: a different pattern on the same operation.
        pattern_case(
            "/list-token-balances",
            json!({"tokenFilter": {
                "network": "ETHEREUM_MAINNET",
                "tokenId": "not a token id",
            }}),
            "tokenFilter.tokenId",
            "not a token id",
            TOKEN_ID_PATTERN,
        ),
        // ListTransactionsInput$address -> ChainAddress: a top-level member.
        pattern_case(
            "/list-transactions",
            json!({"network": "ETHEREUM_MAINNET", "address": "too-short"}),
            "address",
            "too-short",
            CHAIN_ADDRESS_PATTERN,
        ),
        // OwnerIdentifier$address -> ChainAddress.
        pattern_case(
            "/get-token-balance",
            json!({
                "tokenIdentifier": {"network": "ETHEREUM_MAINNET", "tokenId": "eth"},
                "ownerIdentifier": {"address": "nope!"},
            }),
            "ownerIdentifier.address",
            "nope!",
            CHAIN_ADDRESS_PATTERN,
        ),
        // GetTransactionInput$transactionHash -> QueryTransactionHash.
        pattern_case(
            "/get-transaction",
            json!({"network": "ETHEREUM_MAINNET", "transactionHash": "0xnothex"}),
            "transactionHash",
            "0xnothex",
            TRANSACTION_PATTERN,
        ),
        // GetTransactionInput$transactionId -> QueryTransactionId.
        pattern_case(
            "/get-transaction",
            json!({"network": "BITCOIN_MAINNET", "transactionId": "zz"}),
            "transactionId",
            "zz",
            TRANSACTION_PATTERN,
        ),
        // ListTransactionEventsInput$transactionHash -> QueryTransactionHash.
        pattern_case(
            "/list-transaction-events",
            json!({"network": "ETHEREUM_MAINNET", "transactionHash": "not-a-hash"}),
            "transactionHash",
            "not-a-hash",
            TRANSACTION_PATTERN,
        ),
        // ChainAddresses' element, inside a required list: the path names the element's
        // 1-based position and the collection step.
        pattern_case(
            "/list-filtered-transaction-events",
            json!({
                "network": "BITCOIN_MAINNET",
                "addressIdentifierFilter": {"transactionEventToAddress": ["bad!"]},
            }),
            "addressIdentifierFilter.transactionEventToAddress.1.member",
            "bad!",
            CHAIN_ADDRESS_PATTERN,
        ),
        // NOTE: the deepest path this service has — a member of a structure inside a
        // list — belongs to `BatchGetTokenBalance`, and it is deliberately NOT a row
        // here: that operation's output models `errors` as a `@required` list beside
        // `tokenBalances`, so one item's refused value is reported as an entry of that
        // list inside the HTTP 200 and not as a top-level refusal (the recorded wire
        // behaviour, and cause 1 of the "error responses are not AWS-shaped" issue).
        // `tests/all/error_response_shape.rs` asserts that per-item clause there.
    ]
}

/// Every other constraint class the model declares on this service's inputs: `@length` on
/// a list (at and past both published bounds), `@range` on an integer, and `@required`.
fn bound_cases() -> Vec<Clause> {
    let mut cases = vec![
        // GetTokenBalanceInputList carries @length(min: 1, max: 10); eleven items is one
        // past the published maximum. A collection is reported by member only — the
        // rejected value is the list, which the clause does not render.
        Clause {
            route: "/batch-get-token-balance",
            body: json!({"getTokenBalanceInputs":
                (0..11).map(|_| batch_item("ETHEREUM_MAINNET", VALID_ADDRESS)).collect::<Vec<_>>()}),
            clause: "Value at 'getTokenBalanceInputs' failed to satisfy constraint: \
                     Member must have length less than or equal to 10"
                .to_string(),
        },
        // ... and one short of the published minimum.
        Clause {
            route: "/batch-get-token-balance",
            body: json!({"getTokenBalanceInputs": []}),
            clause: "Value at 'getTokenBalanceInputs' failed to satisfy constraint: \
                     Member must have length greater than or equal to 1"
                .to_string(),
        },
        // AddressIdentifierFilter$transactionEventToAddress carries @length(min: 1, max: 1).
        Clause {
            route: "/list-filtered-transaction-events",
            body: json!({
                "network": "BITCOIN_MAINNET",
                "addressIdentifierFilter": {
                    "transactionEventToAddress": [VALID_ADDRESS, VALID_ADDRESS],
                },
            }),
            clause: "Value at 'addressIdentifierFilter.transactionEventToAddress' failed to \
                     satisfy constraint: Member must have length less than or equal to 1"
                .to_string(),
        },
        // ListTransactionsInput$maxResults carries @range(min: 1, max: 250). A scalar's
        // value IS what the caller sent, so the clause quotes it.
        Clause {
            route: "/list-transactions",
            body: json!({
                "network": "ETHEREUM_MAINNET",
                "address": VALID_ADDRESS,
                "maxResults": 0,
            }),
            clause: "Value '0' at 'maxResults' failed to satisfy constraint: \
                     Member must have value greater than or equal to 1"
                .to_string(),
        },
        Clause {
            route: "/list-transactions",
            body: json!({
                "network": "ETHEREUM_MAINNET",
                "address": VALID_ADDRESS,
                "maxResults": 251,
            }),
            clause: "Value '251' at 'maxResults' failed to satisfy constraint: \
                     Member must have value less than or equal to 250"
                .to_string(),
        },
    ];
    // GetTransactionInput$network carries @required; an absent member has no submitted
    // value, so it is reported as the null it was.
    cases.push(Clause {
        route: "/get-transaction",
        body: json!({"transactionHash": UNMINED_HASH}),
        clause: "Value null at 'network' failed to satisfy constraint: Member must not be null"
            .to_string(),
    });
    cases
}

#[tokio::test]
async fn modelled_constraint_refusals_render_in_the_aws_validation_envelope() {
    let emulator = Emulator::new();
    let cases: Vec<Clause> = pattern_cases().into_iter().chain(bound_cases()).collect();

    for case in &cases {
        let (status, body) = emulator.post(case.route, &case.body).await;
        let expected = format!("1 validation error detected: {}", case.clause);
        assert_eq!(
            status, 400,
            "case {} {}: expected 400, got {status} with {body:?}",
            case.route, case.body
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {} {}: expected ValidationException, got {body:?}",
            case.route,
            case.body
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {} {}: expected message {expected:?}, got {message:?}",
            case.route, case.body
        );
    }
}

#[tokio::test]
async fn a_reported_pattern_never_carries_the_models_anchors() {
    let emulator = Emulator::new();

    for case in &pattern_cases() {
        let (_, body) = emulator.post(case.route, &case.body).await;
        let message = body["message"].as_str().unwrap_or_default();
        // The anchors are the only `^`/`$` any of these patterns contains, so their
        // absence from the whole message is exactly "the pattern was unanchored".
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {} {}: the reported pattern must carry no anchors, got {message:?}",
            case.route,
            case.body
        );
    }
}

#[tokio::test]
async fn a_request_at_a_published_bound_is_not_refused() {
    let emulator = Emulator::new();
    // (route, body, why it is at the bound). Each satisfies every modelled constraint, so
    // the only thing it may NOT answer is a 400 ValidationException.
    let cases = vec![
        // GetTokenBalanceInputList @length(min: 1, max: 10), at both bounds.
        (
            "/batch-get-token-balance",
            json!({"getTokenBalanceInputs":
                (0..10).map(|_| batch_item("ETHEREUM_MAINNET", VALID_ADDRESS)).collect::<Vec<_>>()}),
        ),
        (
            "/batch-get-token-balance",
            json!({"getTokenBalanceInputs": [batch_item("ETHEREUM_MAINNET", VALID_ADDRESS)]}),
        ),
        // ListTransactionsInput$maxResults @range(min: 1, max: 250), at both bounds.
        (
            "/list-transactions",
            json!({"network": "ETHEREUM_MAINNET", "address": VALID_ADDRESS, "maxResults": 1}),
        ),
        (
            "/list-transactions",
            json!({"network": "ETHEREUM_MAINNET", "address": VALID_ADDRESS, "maxResults": 250}),
        ),
        // ChainAddress @pattern, at both ends of its 13..74 length window.
        (
            "/list-transactions",
            json!({"network": "ETHEREUM_MAINNET", "address": "a".repeat(13)}),
        ),
        (
            "/list-transactions",
            json!({"network": "ETHEREUM_MAINNET", "address": "a".repeat(74)}),
        ),
    ];

    for (route, body) in &cases {
        let (status, answer) = emulator.post(route, body).await;
        assert_ne!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {route} {body}: a request at a published bound must not be refused, \
             got {status} with {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a service-level refusal is generic on top, detailed in fieldList
// ---------------------------------------------------------------------------

/// The `fieldList` entry for the member that must name the transaction.
fn exclusivity_field() -> Value {
    json!({
        "name": "transactionHash",
        "message": "The request must include either a transactionId or transactionHash but not both.",
    })
}

/// The `fieldList` entry for a `transactionId` used off the Bitcoin networks
/// (`GetTransactionInput$transactionId`: "transactionId is only supported on the Bitcoin
/// networks").
fn bitcoin_only_field() -> Value {
    json!({
        "name": "transactionId",
        "message": "transactionId can only be specified for Bitcoin networks.",
    })
}

/// The `fieldList` entry for a request that names no transaction at all.
fn missing_identifier_field() -> Value {
    json!({
        "name": "transactionHash",
        "message": "The request must include a transactionHash.",
    })
}

#[tokio::test]
async fn transaction_identifier_rules_are_reported_in_the_field_list() {
    let emulator = Emulator::new();
    let hash = UNMINED_HASH.to_string();

    // Both operations model the same optional `transactionHash`/`transactionId` pair, so
    // both accept the same combinations and refuse the rest identically.
    for route in ["/get-transaction", "/list-transaction-events"] {
        let mut cases: Vec<(Value, Vec<Value>)> = Vec::new();
        for network in QUERY_NETWORK_VALUES {
            let bitcoin = BITCOIN_NETWORKS.contains(network);
            // Both members: never allowed, and on an EVM network `transactionId` breaks
            // its own rule at the same time, so TWO entries come back.
            let both = if bitcoin {
                vec![exclusivity_field()]
            } else {
                vec![exclusivity_field(), bitcoin_only_field()]
            };
            cases.push((
                json!({"network": network, "transactionHash": hash, "transactionId": hash}),
                both,
            ));
            // Neither member: nothing names a transaction.
            cases.push((json!({"network": network}), vec![missing_identifier_field()]));
            // `transactionId` alone is the Bitcoin-only member, so it is refused on every
            // other network (and accepted on the Bitcoin ones — see the round-trip test).
            if !bitcoin {
                cases.push((
                    json!({"network": network, "transactionId": hash}),
                    vec![bitcoin_only_field()],
                ));
            }
        }

        for (body, fields) in &cases {
            let (status, answer) = emulator.post(route, body).await;
            assert_eq!(
                status, 400,
                "case {route} {body}: expected 400, got {status} with {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {route} {body}: expected ValidationException, got {answer:?}"
            );
            assert_eq!(
                answer["message"].as_str().unwrap_or_default(),
                GENERIC_VALIDATION_MESSAGE,
                "case {route} {body}: the top-level message must be generic, got {answer:?}"
            );
            assert_eq!(
                answer["reason"].as_str().unwrap_or_default(),
                FIELD_VALIDATION_FAILED,
                "case {route} {body}: expected reason {FIELD_VALIDATION_FAILED}, got {answer:?}"
            );
            assert_eq!(
                &answer["fieldList"],
                &json!(fields),
                "case {route} {body}: expected fieldList {fields:?}, got {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_identifier_still_resolves_its_transaction() {
    let emulator = Emulator::new();
    let fixture = emulator.fixture();
    assert!(
        !fixture.is_empty(),
        "every QueryNetwork value must have a seeded ledger, got none"
    );

    // The stricter refusal above must not have cost the accepted combinations: a
    // `transactionHash` on any network, and a `transactionId` on the Bitcoin ones, still
    // read the transaction back out of the ledger.
    for network in &fixture {
        for stored in &network.transactions {
            let by_hash = json!({
                "network": network.network,
                "transactionHash": stored.transaction_hash,
            });
            let (status, answer) = emulator.post("/get-transaction", &by_hash).await;
            assert_eq!(
                status, 200,
                "case {by_hash}: expected 200, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["transaction"]["transactionHash"].as_str().unwrap_or_default(),
                stored.transaction_hash,
                "case {by_hash}: expected the stored transaction, got {answer:?}"
            );

            if let Some(id) = &stored.transaction_id {
                let by_id = json!({"network": network.network, "transactionId": id});
                let (status, answer) = emulator.post("/get-transaction", &by_id).await;
                assert_eq!(
                    status, 200,
                    "case {by_id}: expected 200, got {status} with {answer:?}"
                );
                assert_eq!(
                    answer["transaction"]["transactionId"].as_str().unwrap_or_default(),
                    id,
                    "case {by_id}: expected the transaction that id names, got {answer:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: not-found carries a fixed message and the identity in its own members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn not_found_is_generic_and_names_the_resource_in_its_own_members() {
    let emulator = Emulator::new();
    // (route, body, the resourceId the answer must report). Three different lookups —
    // by hash, by Bitcoin id, and by (token, owner) — on every network the service holds
    // a ledger for.
    let mut cases: Vec<(&str, Value, String)> = Vec::new();
    for network in QUERY_NETWORK_VALUES {
        cases.push((
            "/get-transaction",
            json!({"network": network, "transactionHash": UNMINED_HASH}),
            UNMINED_HASH.to_string(),
        ));
        if BITCOIN_NETWORKS.contains(network) {
            cases.push((
                "/get-transaction",
                json!({"network": network, "transactionId": UNMINED_HASH}),
                UNMINED_HASH.to_string(),
            ));
        }
        cases.push((
            "/get-token-balance",
            json!({
                "tokenIdentifier": {"network": network, "tokenId": "eth"},
                "ownerIdentifier": {"address": VALID_ADDRESS},
            }),
            // HoldingKey's resourceId joins the token half and the owner, with `-` for an
            // absent member: no ChainAddress or QueryTokenId can contain `/` or `-`.
            format!("-/eth/{VALID_ADDRESS}"),
        ));
    }

    for (route, body, resource_id) in &cases {
        let (status, answer) = emulator.post(route, body).await;
        assert_eq!(
            status, 404,
            "case {route} {body}: expected 404, got {status} with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {route} {body}: expected ResourceNotFoundException, got {answer:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, GENERIC_NOT_FOUND_MESSAGE,
            "case {route} {body}: expected message {GENERIC_NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
        assert_eq!(
            answer["resourceId"].as_str().unwrap_or_default(),
            resource_id,
            "case {route} {body}: expected resourceId {resource_id:?}, got {answer:?}"
        );
        assert_eq!(
            answer["resourceType"].as_str().unwrap_or_default(),
            RESOURCE_TYPE,
            "case {route} {body}: expected resourceType {RESOURCE_TYPE}, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: ListFilteredTransactionEvents is Bitcoin-only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_filtered_transaction_events_serves_only_the_bitcoin_networks() {
    let emulator = Emulator::new();
    // `network` targets a plain `String`, so the value set is the operation's own rule
    // rather than a modelled enum: every QueryNetwork value, plus values no enum holds —
    // a casing variant, a prefix, a padded value and the empty string.
    let values: Vec<String> = QUERY_NETWORK_VALUES
        .iter()
        .map(|network| network.to_string())
        .chain(
            ["bitcoin_mainnet", "BITCOIN", "BITCOIN_MAINNET ", "", "NOT_A_NETWORK"]
                .iter()
                .map(|value| value.to_string()),
        )
        .collect();

    for network in &values {
        // The row varies the NETWORK alone, so the filter carries an address this
        // operation accepts on the networks it serves: a well-formed Bitcoin address.
        let body = json!({
            "network": network,
            "addressIdentifierFilter": {
                "transactionEventToAddress": [unseeded_bitcoin_address()],
            },
        });
        let (status, answer) = emulator.post("/list-filtered-transaction-events", &body).await;
        if BITCOIN_NETWORKS.contains(&network.as_str()) {
            // A network the operation serves gets past the check; the listing behind it
            // is still scaffolded, which is its own issue — what matters here is that the
            // refusal is NOT what a Bitcoin network answers.
            assert_ne!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {body}: a Bitcoin network must not be refused, got {status} with {answer:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {body}: expected 400, got {status} with {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {body}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer["message"].as_str().unwrap_or_default(),
            GENERIC_VALIDATION_MESSAGE,
            "case {body}: the top-level message must be generic, got {answer:?}"
        );
        assert_eq!(
            answer["reason"].as_str().unwrap_or_default(),
            FIELD_VALIDATION_FAILED,
            "case {body}: expected reason {FIELD_VALIDATION_FAILED}, got {answer:?}"
        );
        assert_eq!(
            answer["fieldList"][0]["name"].as_str().unwrap_or_default(),
            "network",
            "case {body}: the fieldList must name the member that was refused, got {answer:?}"
        );
    }
}

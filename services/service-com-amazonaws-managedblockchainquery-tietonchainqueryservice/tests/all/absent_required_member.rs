// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request that OMITS a `@required` input member.
//!
//! One rule is under test: a request refused before any handler runs because a member the
//! model requires is absent is answered with HTTP 400 `ValidationException` carrying the
//! AWS validation report as `message`, the model-required `reason` =
//! `fieldValidationFailed`, and one `fieldList` entry naming the absent member by the
//! dotted path the report states it at — the same envelope every other 400 of this service
//! carries.
//!
//! That refusal is found while the request is being READ, not while a value is being
//! checked, so it is the one refusal no handler of this service can make; it is therefore
//! the path a table has to span rather than one recorded request. The rows differ along
//! what the rule generalises over:
//!
//! * the OPERATION — every one of the nine the model declares;
//! * the member's POSITION — a top-level member, a member nested in an identifier or a
//!   filter one level down, and a member nested in a LIST element, which the report names
//!   by the element's 1-based position;
//! * the member's TARGET — a structure, a string, an enumerated string and a list;
//! * which member of a shape that requires SEVERAL (`ContractIdentifier` requires both of
//!   its members, `ContractFilter` all three).
//!
//! The second test is the other half of the rule: a request that names every required
//! member is never refused for an absent one, whatever else the ledger answers it — so the
//! refusal is keyed to absence and not to a route or a body shape.
//!
//! Every member path and every required member written down here comes from the model
//! (`smithy_query(query="operation",operation="…")` and
//! `smithy_query(query="shape",shape="…")`), named beside the row it belongs to; none is
//! read out of the implementation.

use serde_json::{json, Value};

use crate::chain_query_slice::{error_code, Emulator, NetworkFixture};
use crate::error_response_shape::networks;

/// The `ValidationExceptionReason` a refused member carries: one of
/// `unknownOperation | cannotParse | fieldValidationFailed | other`
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`). `reason` is
/// `@required` on `ValidationException`, so every refusal states one.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The requirement AWS's validation framework states for a member that was not sent: the
/// value it reports is the `null` the member was, and the constraint it failed is that the
/// member must be there at all.
fn absent_requirement() -> String {
    "Member must not be null".to_string()
}

/// One clause of the report: the member that was absent, and what it failed.
fn absent_clause(member: &str) -> String {
    format!(
        "Value null at '{member}' failed to satisfy constraint: {}",
        absent_requirement()
    )
}

/// The whole report for ONE absent member. Absence is found while the request is being
/// read, which stops at the first member missing, so the report states one error.
fn absent_report(member: &str) -> String {
    format!("1 validation error detected: {}", absent_clause(member))
}

/// One row: a request missing exactly ONE member the model requires, and the dotted path
/// the refusal has to name it by.
struct Absent {
    case: String,
    route: &'static str,
    body: Value,
    /// The member path the report names, as AWS spells it: lowerCamel, dotted, with a list
    /// element's 1-based position followed by the `member` step.
    member: &'static str,
}

/// The table: every operation of this service, reached through a member the model requires
/// at a different position and of a different target type.
fn absent_rows(ethereum: &NetworkFixture, bitcoin: &NetworkFixture) -> Vec<Absent> {
    let network = ethereum.network.as_str();
    let address = ethereum.busiest_address();
    let hash = ethereum.transactions[0].transaction_hash.clone();
    let owner = json!({"address": address});
    let token = json!({"network": network, "tokenId": "eth"});
    vec![
        // --- a top-level member whose target is a STRUCTURE ---
        Absent {
            case: "GetTokenBalance without its tokenIdentifier".to_string(),
            route: "/get-token-balance",
            body: json!({"ownerIdentifier": owner}),
            member: "tokenIdentifier",
        },
        Absent {
            case: "GetTokenBalance without its ownerIdentifier".to_string(),
            route: "/get-token-balance",
            body: json!({"tokenIdentifier": token}),
            member: "ownerIdentifier",
        },
        Absent {
            case: "GetAssetContract without its contractIdentifier".to_string(),
            route: "/get-asset-contract",
            body: json!({}),
            member: "contractIdentifier",
        },
        Absent {
            case: "ListAssetContracts without its contractFilter".to_string(),
            route: "/list-asset-contracts",
            body: json!({"maxResults": 5}),
            member: "contractFilter",
        },
        Absent {
            case: "ListTokenBalances without its tokenFilter".to_string(),
            route: "/list-token-balances",
            body: json!({"ownerFilter": {"address": address}}),
            member: "tokenFilter",
        },
        Absent {
            case: "ListFilteredTransactionEvents without its addressIdentifierFilter"
                .to_string(),
            route: "/list-filtered-transaction-events",
            body: json!({"network": bitcoin.network}),
            member: "addressIdentifierFilter",
        },
        // --- a top-level member whose target is an ENUMERATED string ---
        Absent {
            case: "GetTransaction without its network".to_string(),
            route: "/get-transaction",
            body: json!({"transactionHash": hash}),
            member: "network",
        },
        Absent {
            case: "ListTransactionEvents without its network".to_string(),
            route: "/list-transaction-events",
            body: json!({"transactionHash": hash}),
            member: "network",
        },
        Absent {
            case: "ListTransactions without its network".to_string(),
            route: "/list-transactions",
            body: json!({"address": address}),
            member: "network",
        },
        // --- a top-level member whose target is a PLAIN string: the one network member of
        // this service that targets `String` rather than `QueryNetwork` ---
        Absent {
            case: "ListFilteredTransactionEvents without its network".to_string(),
            route: "/list-filtered-transaction-events",
            body: json!({
                "addressIdentifierFilter": {"transactionEventToAddress": [address]},
            }),
            member: "network",
        },
        // --- a top-level member whose target is a CONSTRAINED string ---
        Absent {
            case: "ListTransactions without its address".to_string(),
            route: "/list-transactions",
            body: json!({"network": network}),
            member: "address",
        },
        // --- a member nested ONE level down, in a structure the request did send ---
        Absent {
            case: "GetTokenBalance: a tokenIdentifier without its network".to_string(),
            route: "/get-token-balance",
            body: json!({"tokenIdentifier": {"tokenId": "eth"}, "ownerIdentifier": owner}),
            member: "tokenIdentifier.network",
        },
        Absent {
            case: "GetTokenBalance: an ownerIdentifier without its address".to_string(),
            route: "/get-token-balance",
            body: json!({"tokenIdentifier": token, "ownerIdentifier": {}}),
            member: "ownerIdentifier.address",
        },
        Absent {
            case: "ListTokenBalances: a tokenFilter without its network".to_string(),
            route: "/list-token-balances",
            body: json!({"tokenFilter": {"tokenId": "eth"}}),
            member: "tokenFilter.network",
        },
        Absent {
            case: "ListTokenBalances: an ownerFilter without its address, in an OPTIONAL \
                   member the request chose to send"
                .to_string(),
            route: "/list-token-balances",
            body: json!({"tokenFilter": token, "ownerFilter": {}}),
            member: "ownerFilter.address",
        },
        // `ContractIdentifier` requires BOTH of its members, so each is a row of its own.
        Absent {
            case: "GetAssetContract: a contractIdentifier without its contractAddress"
                .to_string(),
            route: "/get-asset-contract",
            body: json!({"contractIdentifier": {"network": network}}),
            member: "contractIdentifier.contractAddress",
        },
        Absent {
            case: "GetAssetContract: a contractIdentifier without its network".to_string(),
            route: "/get-asset-contract",
            body: json!({"contractIdentifier": {"contractAddress": address}}),
            member: "contractIdentifier.network",
        },
        // `ContractFilter` requires all THREE of its members.
        Absent {
            case: "ListAssetContracts: a contractFilter without its deployerAddress"
                .to_string(),
            route: "/list-asset-contracts",
            body: json!({"contractFilter": {"network": network, "tokenStandard": "ERC20"}}),
            member: "contractFilter.deployerAddress",
        },
        Absent {
            case: "ListAssetContracts: a contractFilter without its tokenStandard".to_string(),
            route: "/list-asset-contracts",
            body: json!({"contractFilter": {
                "network": network,
                "deployerAddress": address,
            }}),
            member: "contractFilter.tokenStandard",
        },
        // --- a nested member whose target is a LIST ---
        Absent {
            case: "ListFilteredTransactionEvents: an addressIdentifierFilter without its \
                   transactionEventToAddress"
                .to_string(),
            route: "/list-filtered-transaction-events",
            body: json!({"network": bitcoin.network, "addressIdentifierFilter": {}}),
            member: "addressIdentifierFilter.transactionEventToAddress",
        },
        Absent {
            case: "ListTransactions: a confirmationStatusFilter without its include, in an \
                   OPTIONAL member the request chose to send"
                .to_string(),
            route: "/list-transactions",
            body: json!({
                "network": network,
                "address": address,
                "confirmationStatusFilter": {},
            }),
            member: "confirmationStatusFilter.include",
        },
        // --- a member nested in a LIST ELEMENT, named by the element's 1-based position ---
        Absent {
            case: "BatchGetTokenBalance: the FIRST item without its tokenIdentifier"
                .to_string(),
            route: "/batch-get-token-balance",
            body: json!({"getTokenBalanceInputs": [{"ownerIdentifier": owner}]}),
            member: "getTokenBalanceInputs.1.member.tokenIdentifier",
        },
        Absent {
            case: "BatchGetTokenBalance: the SECOND item without its ownerIdentifier, after \
                   one item the model accepts"
                .to_string(),
            route: "/batch-get-token-balance",
            body: json!({"getTokenBalanceInputs": [
                {"tokenIdentifier": token, "ownerIdentifier": owner},
                {"tokenIdentifier": token},
            ]}),
            member: "getTokenBalanceInputs.2.member.ownerIdentifier",
        },
        Absent {
            case: "BatchGetTokenBalance: an item's ownerIdentifier without its address, two \
                   levels under the list"
                .to_string(),
            route: "/batch-get-token-balance",
            body: json!({"getTokenBalanceInputs": [
                {"tokenIdentifier": token, "ownerIdentifier": {}},
            ]}),
            member: "getTokenBalanceInputs.1.member.ownerIdentifier.address",
        },
    ]
}

#[tokio::test]
async fn an_absent_required_member_is_refused_with_reason_and_a_field_list() {
    let emulator = Emulator::new();
    let (ethereum, bitcoin) = networks(&emulator);

    for row in absent_rows(&ethereum, &bitcoin) {
        let case = &row.case;
        let body = emulator
            .fails(row.route, &row.body, 400, "ValidationException")
            .await;

        let expected = absent_report(row.member);
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the AWS validation report, got {body:?}"
        );
        // `reason` is `@required` on the modelled `ValidationException`, and it is the
        // discriminator a client switches on: a body without it is a null in a member the
        // model says is always there.
        assert_eq!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {body:?}"
        );
        // `fieldList` is the machine-readable half: one entry naming the absent member,
        // carrying that member's own clause of the report.
        assert_eq!(
            body["fieldList"],
            json!([{"name": row.member, "message": absent_clause(row.member)}]),
            "case {case:?}: expected one fieldList entry for {}, got {body:?}",
            row.member
        );
    }
}

/// A request naming every required member, for each operation of this service — built from
/// the seeded ledger the emulator serves, so the values are ones the model admits.
///
/// What each one ANSWERS is the business of the suites that own those operations; all this
/// one asks is that none of them is refused for an absent member.
fn complete_bodies(
    ethereum: &NetworkFixture,
    bitcoin: &NetworkFixture,
) -> Vec<(&'static str, Value)> {
    let network = ethereum.network.as_str();
    let address = ethereum.busiest_address();
    let hash = ethereum.transactions[0].transaction_hash.clone();
    let contract = ethereum
        .contracts
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the seeded Ethereum ledger holds no asset contract"));
    let bitcoin_address = bitcoin.busiest_address();
    let token = json!({"network": network, "tokenId": "eth"});
    let owner = json!({"address": address});
    vec![
        (
            "/get-token-balance",
            json!({"tokenIdentifier": token, "ownerIdentifier": owner}),
        ),
        (
            "/batch-get-token-balance",
            json!({"getTokenBalanceInputs": [
                {"tokenIdentifier": token, "ownerIdentifier": owner},
            ]}),
        ),
        (
            "/get-asset-contract",
            json!({"contractIdentifier": {"network": network, "contractAddress": contract}}),
        ),
        (
            "/list-asset-contracts",
            json!({"contractFilter": {
                "network": network,
                "tokenStandard": "ERC20",
                "deployerAddress": address,
            }}),
        ),
        (
            "/get-transaction",
            json!({"network": network, "transactionHash": hash}),
        ),
        (
            "/list-transaction-events",
            json!({"network": network, "transactionHash": hash}),
        ),
        (
            "/list-transactions",
            json!({"network": network, "address": address}),
        ),
        ("/list-token-balances", json!({"tokenFilter": token})),
        (
            // `ListFilteredTransactionEvents` is documented as supported on the Bitcoin
            // networks only, so its complete request names one.
            "/list-filtered-transaction-events",
            json!({
                "network": bitcoin.network,
                "addressIdentifierFilter": {"transactionEventToAddress": [bitcoin_address]},
            }),
        ),
    ]
}

#[tokio::test]
async fn a_request_that_names_every_required_member_is_not_refused_for_an_absent_one() {
    let emulator = Emulator::new();
    let (ethereum, bitcoin) = networks(&emulator);

    for (route, body) in complete_bodies(&ethereum, &bitcoin) {
        let (status, answered) = emulator.post(route, &body).await;
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(&absent_requirement()),
            "case {route}: a request naming every required member must not be refused for \
             an absent one, got {status} with {answered:?}"
        );
        // And not refused at all: a complete request is answered by the operation (200, or
        // the modelled error the ledger's contents make due), never with the 400 this
        // suite's table is about.
        assert_ne!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {route}: a request naming every required member must not be refused, got \
             {status} with {answered:?}"
        );
        // A 5xx would mean the request was not served at all.
        assert!(
            status < 500,
            "case {route}: expected the request to be served, got {status} with {answered:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The PUBLISHED mainnet accounts the ledger indexes, end to end over HTTP.
//!
//! The rule under test: this service is an index over PUBLIC chains, so what a mainnet
//! ledger has to hold are the identifiers a client of that chain actually reads — its
//! well-known token contracts AND its published accounts — and a read keyed by one of
//! them is answered out of stored records: a balance with its required members, and a
//! non-empty history. An identifier the ledger holds nothing under stays the modelled
//! `ResourceNotFoundException` or the modelled empty page, because this emulator indexes
//! no chain and reports only what it holds.
//!
//! What the sibling suite `derived_chain_history` asserts about the fixture's wallets —
//! the span, the pagination, the windows, the balances over time — holds for these
//! accounts because on a mainnet they ARE those wallets. What is asserted HERE is what
//! that cannot see: that the catalogued accounts are the ones served, that a mainnet
//! account is indexed on its mainnet only, and that one account has one balance and one
//! history however its hex is spelled (EIP-55 spends an address's letter case on a
//! checksum over the same 20 bytes).
//!
//! Every test is a TABLE whose rows come from the catalog the service itself serves
//! (`state::derived::catalogued_accounts`) crossed with the token kinds a
//! `TokenIdentifier` can name and the letter cases an address can be spelled in — never
//! from an identifier written into this file — and every assertion is an invariant, not a
//! number this crate wrote down.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::QUERY_NETWORK_VALUES;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::{
    derived, HoldingKey,
};

use crate::chain_query_slice::{balance_or_none, drain, hashes, item_identity, Emulator};

/// Every network the model declares, from the model's own `QueryNetwork` enum.
fn networks() -> Vec<String> {
    QUERY_NETWORK_VALUES
        .iter()
        .map(|network| (*network).to_string())
        .collect()
}

/// Whether `network` is one of the model's two Bitcoin networks.
fn is_bitcoin(network: &str) -> bool {
    network.starts_with("BITCOIN_")
}

/// The `tokenId` of `network`'s native token: `TokenIdentifier$tokenId` is "the 3
/// character abbreviation that best matches your token. For example, `btc` for Bitcoin,
/// `eth` for Ether".
fn native_token(network: &str) -> &'static str {
    if is_bitcoin(network) {
        "btc"
    } else {
        "eth"
    }
}

/// The published accounts the service holds a ledger for on `network`, read out of the
/// service's own catalog so a row is never a constant copied into this file.
fn catalogued(network: &str) -> Vec<String> {
    derived::catalogued_accounts(network)
        .iter()
        .map(|address| (*address).to_string())
        .collect()
}

/// Every `TokenIdentifier` a holding of `owner` can be asked for: the network's native
/// token, and every token the ledger reports the owner holding — so the rows are the
/// ledger's own, not a second copy of the contract catalog.
async fn tokens(emulator: &Emulator, network: &str, owner: &str) -> Vec<Value> {
    let mut rows = vec![json!({"network": network, "tokenId": native_token(network)})];
    let listed = drain(
        emulator,
        "/list-token-balances",
        "tokenBalances",
        &json!({
            "tokenFilter": {"network": network},
            "ownerFilter": {"address": owner},
        }),
        None,
    )
    .await;
    for item in listed {
        let token = item["tokenIdentifier"].clone();
        if !rows.contains(&token) {
            rows.push(token);
        }
    }
    rows
}

/// The seconds of a `BlockchainInstant` in a response.
fn instant(value: &Value, case: &str) -> i64 {
    value["time"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a BlockchainInstant, got {value:?}"))
        as i64
}

/// A response member that has to be a base-10 integer, as `TokenBalance$balance` is.
fn integer(value: &Value, case: &str) -> i128 {
    value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string, got {value:?}"))
        .parse()
        .unwrap_or_else(|error| panic!("case {case}: expected a base-10 integer: {error}"))
}

/// Every transaction summary the service lists for `address`.
async fn listed(emulator: &Emulator, network: &str, address: &str) -> Vec<Value> {
    drain(
        emulator,
        "/list-transactions",
        "transactions",
        &json!({"network": network, "address": address}),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The catalogued accounts are the ones served
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_published_account_of_a_mainnet_has_balances_and_a_history() {
    let emulator = Emulator::new();
    let mut served = 0usize;

    for network in networks() {
        for address in catalogued(&network) {
            served += 1;
            let tokens = tokens(&emulator, &network, &address).await;
            // The network's own currency always, and a token of every contract the
            // ledger holds for it — the Bitcoin networks have no contracts at all
            // (`GetAssetContract`: "the Bitcoin blockchain networks do not support this
            // operation"), so there it is the native row alone.
            let least = if is_bitcoin(&network) { 1 } else { 2 };
            assert!(
                tokens.len() >= least,
                "case {network}/{address}: a published account holds at least {least} \
                 token(s), got {tokens:?}"
            );
            for token in tokens {
                let body = json!({
                    "tokenIdentifier": token,
                    "ownerIdentifier": {"address": address},
                });
                let case = format!("{network}/{address}/{token}");
                let reply = emulator.ok("/get-token-balance", &body).await;

                // `balance` and `atBlockchainInstant` are the required output members,
                // and a balance is a whole number of the token's smallest unit, so it is
                // never negative.
                let balance = integer(&reply["balance"], &case);
                assert!(
                    balance >= 0,
                    "case {case}: a balance is not negative, got {balance}"
                );
                let asked_at = instant(&reply["atBlockchainInstant"], &case);
                // `TokenBalance$lastUpdatedTime` is "the Timestamp of the last
                // transaction at which the balance ... was updated", so it cannot be
                // later than the instant the balance was priced at.
                let updated = instant(&reply["lastUpdatedTime"], &case);
                assert!(
                    updated <= asked_at,
                    "case {case}: lastUpdatedTime {updated} is after atBlockchainInstant {asked_at}"
                );
                // And the answer names the holding that was asked about.
                assert_eq!(
                    reply["ownerIdentifier"]["address"],
                    json!(address),
                    "case {case}: the owner is reported, got {:?}",
                    reply["ownerIdentifier"]
                );
                assert_eq!(
                    reply["tokenIdentifier"], token,
                    "case {case}: the token is reported, got {:?}",
                    reply["tokenIdentifier"]
                );
            }

            // And the account has a history: the discovery call of this API.
            let transactions = listed(&emulator, &network, &address).await;
            assert!(
                !transactions.is_empty(),
                "case {network}/{address}: a published account has transactions"
            );
            let distinct: BTreeSet<String> = hashes(&json!(transactions)).into_iter().collect();
            assert_eq!(
                distinct.len(),
                transactions.len(),
                "case {network}/{address}: every listed transaction is a different one"
            );
        }
    }

    assert!(
        served > 0,
        "the service must index the published accounts of at least one network"
    );
}

#[tokio::test]
async fn a_published_mainnet_account_is_indexed_on_its_mainnet_only() {
    let emulator = Emulator::new();

    for network in networks() {
        // The rows: every catalogued account of every OTHER network. A mainnet account
        // is not deployed on a testnet, nor on the other family's chain, so this
        // network's index holds nothing under it — the same rule the well-known contract
        // catalog follows.
        for other in networks() {
            if other == network {
                continue;
            }
            for address in catalogued(&other) {
                if catalogued(&network).contains(&address) {
                    continue;
                }
                let case = format!("{network}/{address}");
                emulator
                    .fails(
                        "/get-token-balance",
                        &json!({
                            "tokenIdentifier": {
                                "network": network,
                                "tokenId": native_token(&network),
                            },
                            "ownerIdentifier": {"address": address},
                        }),
                        404,
                        "ResourceNotFoundException",
                    )
                    .await;
                let transactions = listed(&emulator, &network, &address).await;
                assert!(
                    transactions.is_empty(),
                    "case {case}: an account this network does not index has no history, \
                     got {transactions:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// One account, either spelling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_account_has_one_balance_and_one_history_in_either_letter_case() {
    let emulator = Emulator::new();

    for network in networks().into_iter().filter(|network| !is_bitcoin(network)) {
        for address in catalogued(&network) {
            let case = format!("{network}/{address}");
            // EIP-55 spends the letter case of a hex address on a checksum over the same
            // 20 bytes, so these two spellings name ONE account.
            let lower = address.to_ascii_lowercase();
            let upper = format!("0x{}", address[2..].to_ascii_uppercase());

            assert_eq!(
                hashes(&json!(listed(&emulator, &network, &lower).await)),
                hashes(&json!(listed(&emulator, &network, &upper).await)),
                "case {case}: both spellings list one history"
            );

            for token in tokens(&emulator, &network, &lower).await {
                let key_of = |owner: &str, contract: Option<&str>| HoldingKey {
                    contract_address: contract.map(str::to_string),
                    token_id: token["tokenId"].as_str().map(str::to_string),
                    owner_address: owner.to_string(),
                };
                let contract = token["contractAddress"].as_str().map(str::to_string);
                let lowered = balance_or_none(
                    &emulator,
                    &network,
                    &key_of(&lower, contract.as_deref()),
                    None,
                )
                .await;
                let uppered = balance_or_none(
                    &emulator,
                    &network,
                    &key_of(&upper, contract.as_deref()),
                    None,
                )
                .await;
                assert_eq!(
                    lowered, uppered,
                    "case {case}/{token}: both spellings of the owner read one balance"
                );

                // And a token contract is one contract in either spelling too.
                if let Some(contract) = contract {
                    let shouted = format!("0x{}", contract[2..].to_ascii_uppercase());
                    assert_eq!(
                        lowered,
                        balance_or_none(
                            &emulator,
                            &network,
                            &key_of(&lower, Some(&shouted)),
                            None
                        )
                        .await,
                        "case {case}/{token}: both spellings of the contract read one balance"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn reading_a_published_account_never_changes_what_the_next_read_sees() {
    let emulator = Emulator::new();

    for network in networks() {
        for address in catalogued(&network) {
            let case = format!("{network}/{address}");
            let first = listed(&emulator, &network, &address).await;
            // Read everything that hangs off the listing in between: if any read wrote
            // anything, the second listing or the second balance would differ.
            for item in &first {
                let hash = item["transactionHash"].as_str().expect("a hash");
                emulator
                    .ok(
                        "/get-transaction",
                        &json!({"network": network, "transactionHash": hash}),
                    )
                    .await;
                drain(
                    &emulator,
                    "/list-transaction-events",
                    "events",
                    &json!({"network": network, "transactionHash": hash}),
                    None,
                )
                .await;
            }
            assert_eq!(
                first,
                listed(&emulator, &network, &address).await,
                "case {case}: the listing is the same for the next caller"
            );

            for token in tokens(&emulator, &network, &address).await {
                let body = json!({
                    "tokenIdentifier": token,
                    "ownerIdentifier": {"address": address},
                });
                let one = emulator.ok("/get-token-balance", &body).await;
                let two = emulator.ok("/get-token-balance", &body).await;
                assert_eq!(
                    item_identity(&one),
                    item_identity(&two),
                    "case {case}/{token}: the balance is the same for the next caller"
                );
            }
        }
    }
}

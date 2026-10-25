// SPDX-License-Identifier: CC0-1.0

//! The unit an amount of a token a CONTRACT issues is quoted in, driven end-to-end
//! against what the service itself publishes about that contract.
//!
//! The rule: an amount of such a token is quoted at the `decimals` its contract publishes
//! — `smithy_query(query="shape",shape="ContractMetadata")` on `decimals`: "The decimals
//! used by the token contract", the member `GetAssetContract` reports in
//! `metadata.decimals` — so a balance or a transferred value is that many places down
//! from the smallest unit a ledger holds it in, and a token whose contract publishes no
//! metadata is the only one quoted as held. The native half of the same rule (a currency's
//! own precision: 10^8 satoshi to the bitcoin, 10^18 wei to the ether) is
//! `native_token_denomination.rs`.
//!
//! Every expectation here reads the scaling factor OFF THE WIRE, from
//! `GetAssetContract`'s own `metadata.decimals` for the contract the holding names, so the
//! two answers of one service cannot agree with each other while disagreeing about the
//! unit. Each test is a TABLE: the rows are every contract-issued holding and every
//! contract-issued event the served ledger holds, which span the published subdivisions
//! this index catalogs (6, 8 and 18 places) and a contract that publishes none, with the
//! counters at the end of each test refusing a run whose rows do not span them.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has a
//! single front door for every suite.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::HoldingKey;

use crate::chain_query_slice::{token_json, Emulator, TRANSACTION_EVENTS};
use crate::native_token_denomination::{amount_units, contract_key, is_canonical};

/// The route `GetAssetContract` is bound to (`@http(method: POST, uri:
/// "/get-asset-contract")`).
const ASSET_CONTRACT: &str = "/get-asset-contract";

/// The widest subdivision an amount can be quoted at: an amount is a 128-bit integer
/// count of smallest units, so a scale past 10^38 could not divide one at all.
const MAX_DECIMALS: i64 = 38;

/// What `GetAssetContract` publishes as the subdivision of the token issued by the
/// contract at `contract_address` on `network`: its `metadata.decimals`, or `None` for a
/// contract the index holds no metadata — and so no subdivision — for.
async fn published_decimals(
    emulator: &Emulator,
    network: &str,
    contract_address: &str,
) -> Option<i64> {
    let body = json!({
        "contractIdentifier": {"network": network, "contractAddress": contract_address},
    });
    let (status, reply) = emulator.post(ASSET_CONTRACT, &body).await;
    assert_eq!(
        status, 200,
        "case {network}/{contract_address}: a contract its own holdings name is one this \
         index holds, got {status} with {reply:?}"
    );
    reply["metadata"]["decimals"].as_i64()
}

/// [`published_decimals`], asked once per contract.
async fn decimals_of(
    emulator: &Emulator,
    cache: &mut BTreeMap<String, Option<i64>>,
    network: &str,
    contract_address: &str,
) -> Option<i64> {
    let key = contract_key(contract_address);
    if let Some(known) = cache.get(&key) {
        return *known;
    }
    let published = published_decimals(emulator, network, contract_address).await;
    cache.insert(key, published);
    published
}

/// The `balance` `GetTokenBalance` reports for one holding.
async fn reported_balance(emulator: &Emulator, network: &str, key: &HoldingKey) -> String {
    let reply = emulator
        .ok(
            "/get-token-balance",
            &json!({
                "tokenIdentifier": token_json(network, key),
                "ownerIdentifier": {"address": key.owner_address},
            }),
        )
        .await;
    reply["balance"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {network}/{}: balance is required, got {reply:?}",
                key.resource_id()
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_contract_issued_balance_is_quoted_at_the_decimals_the_contract_publishes() {
    let emulator = Emulator::new();

    // Which published subdivisions the rows covered, and whether they produced both a
    // whole and a fractional amount of a token — the counters the end of the test
    // requires, so a run whose ledger stopped spanning the class fails instead of
    // passing on one row.
    let mut subdivisions: BTreeSet<i64> = BTreeSet::new();
    let mut whole = 0usize;
    let mut fractional = 0usize;
    let mut unpublished = 0usize;

    for network in emulator.fixture() {
        let mut cache: BTreeMap<String, Option<i64>> = BTreeMap::new();
        for (key, holding) in &network.holdings {
            let Some(contract) = key.contract_address.clone() else {
                continue;
            };
            let case = format!("{}/{}", network.network, key.resource_id());
            let published = decimals_of(&emulator, &mut cache, &network.network, &contract).await;
            match published {
                Some(places) => {
                    assert!(
                        (0..=MAX_DECIMALS).contains(&places),
                        "case {case}: a published subdivision divides a 128-bit amount, \
                         got {places}"
                    );
                    subdivisions.insert(places);
                }
                None => unpublished += 1,
            }
            let decimals = published.unwrap_or(0).clamp(0, MAX_DECIMALS) as u32;

            let stored = holding
                .history
                .last()
                .unwrap_or_else(|| panic!("case {case}: the holding has no balance"))
                .balance;
            let text = reported_balance(&emulator, &network.network, key).await;

            assert!(
                is_canonical(&text, decimals),
                "case {case}: a balance is a decimal of at most {decimals} places with no \
                 trailing zero, got {text:?}"
            );
            assert_eq!(
                amount_units(&text, decimals),
                Some(stored),
                "case {case}: the holding's {stored} smallest units read as a \
                 {decimals}-place amount, got {text:?}"
            );
            // The same statement about the MAGNITUDE rather than the round trip: a
            // published subdivision moves the point, so the whole part of the reported
            // amount is the smallest-unit count divided by that power of ten.
            let scale = 10i128.pow(decimals);
            let reported_whole = text
                .strip_prefix('-')
                .unwrap_or(&text)
                .split('.')
                .next()
                .unwrap_or_default();
            assert_eq!(
                reported_whole,
                (stored.abs() / scale).to_string(),
                "case {case}: {stored} smallest units are {} whole tokens at {decimals} \
                 places, got {text:?}",
                stored.abs() / scale
            );
            if text.contains('.') {
                fractional += 1;
            } else {
                whole += 1;
            }

            // The same balance through the listing and the batch read: one projection
            // serves all three, so none of them may quote it in another unit.
            let listed = emulator
                .ok(
                    "/list-token-balances",
                    &json!({
                        "tokenFilter": token_json(&network.network, key),
                        "ownerFilter": {"address": key.owner_address},
                    }),
                )
                .await;
            let rows = listed["tokenBalances"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                rows.len(),
                1,
                "case {case}: the listing of one holding has one row, got {listed:?}"
            );
            assert_eq!(
                rows[0]["balance"].as_str(),
                Some(text.as_str()),
                "case {case}: the listing reports the single read's balance {text}, got \
                 {listed:?}"
            );

            let item = json!({
                "tokenIdentifier": token_json(&network.network, key),
                "ownerIdentifier": {"address": key.owner_address},
            });
            let batched = emulator
                .ok(
                    "/batch-get-token-balance",
                    &json!({"getTokenBalanceInputs": [item]}),
                )
                .await;
            assert_eq!(
                batched["tokenBalances"][0]["balance"].as_str(),
                Some(text.as_str()),
                "case {case}: the batch read reports the single read's balance {text}, got \
                 {batched:?}"
            );
        }
    }

    // The index catalogs token contracts of three different precisions (6, 8 and 18
    // places), so a run that saw fewer is not testing the rule across the factor it
    // generalises over.
    assert!(
        subdivisions.len() >= 3,
        "the rows must span several published subdivisions, got {subdivisions:?}"
    );
    assert!(
        whole > 0 && fractional > 0,
        "the rows must hold both a whole and a fractional amount of a contract-issued \
         token, got {whole} whole and {fractional} fractional"
    );
    assert!(
        unpublished > 0,
        "a token whose contract publishes no decimals is part of the rule, got no such \
         holding"
    );
}

#[tokio::test]
async fn a_contract_issued_event_value_is_quoted_the_same_way_as_the_balance_it_moves() {
    let emulator = Emulator::new();

    let mut subdivisions: BTreeSet<i64> = BTreeSet::new();
    let mut values = 0usize;

    for network in emulator.fixture() {
        let mut cache: BTreeMap<String, Option<i64>> = BTreeMap::new();
        for stored in &network.transactions {
            // Only the transactions that moved a contract-issued token have a row here;
            // the native ones are `native_token_denomination.rs`'s.
            if !stored
                .events
                .iter()
                .any(|event| event.contract_address.is_some() && event.value.is_some())
            {
                continue;
            }
            let reported = emulator
                .ok(
                    TRANSACTION_EVENTS,
                    &json!({
                        "network": network.network,
                        "transactionHash": stored.transaction_hash,
                    }),
                )
                .await["events"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}/{}: events is required",
                        network.network, stored.transaction_hash
                    )
                });
            assert_eq!(
                reported.len(),
                stored.events.len(),
                "case {}/{}: expected {} events, got {reported:?}",
                network.network,
                stored.transaction_hash,
                stored.events.len()
            );

            for (position, (event, expected)) in reported.iter().zip(&stored.events).enumerate() {
                let case = format!(
                    "{}/{} event {position}",
                    network.network, stored.transaction_hash
                );
                let (Some(contract), Some(amount)) =
                    (expected.contract_address.clone(), expected.value)
                else {
                    continue;
                };
                let published =
                    decimals_of(&emulator, &mut cache, &network.network, &contract).await;
                if let Some(places) = published {
                    subdivisions.insert(places);
                }
                let decimals = published.unwrap_or(0).clamp(0, MAX_DECIMALS) as u32;
                let text = event["value"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a value, got {event:?}"));
                assert!(
                    is_canonical(text, decimals),
                    "case {case}: a value is a decimal of at most {decimals} places with no \
                     trailing zero, got {text:?}"
                );
                assert_eq!(
                    amount_units(text, decimals),
                    Some(amount),
                    "case {case}: {amount} of the token's smallest unit is {decimals} \
                     places down, got {text:?}"
                );
                values += 1;
            }
        }
    }

    assert!(
        values > 0,
        "the ledger must hold contract-issued event values for this rule to have a case, \
         got none"
    );
    assert!(
        subdivisions.len() >= 2,
        "the rows must span several published subdivisions, got {subdivisions:?}"
    );
}

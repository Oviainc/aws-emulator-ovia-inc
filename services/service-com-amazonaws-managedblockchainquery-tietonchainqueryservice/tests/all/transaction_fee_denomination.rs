// SPDX-License-Identifier: CC0-1.0

//! The unit `Transaction$transactionFee` is reported in, and the unit the gas members
//! beside it are reported in.
//!
//! One rule, a property of the QUANTITY rather than of the member: an amount of a chain's
//! own CURRENCY that this service reports is quoted in that currency, which subdivides
//! into a published number of smallest units (10^8 satoshi to the bitcoin, 10^18 wei to
//! the ether), while a count of something the currency does not denominate is reported as
//! the integer it is. A transaction's fee is paid in the currency of the chain that mined
//! it — `smithy_query(query="shape",shape="Transaction")` on `transactionFee`: "The
//! transaction fee" — so it is quoted the way every other native amount this service
//! reports is (`TransactionEvent$value`, `TokenBalance$balance`; the rule itself is owned
//! by `native_token_denomination.rs`, whose restatement of the subdivisions this suite
//! reuses). `gasUsed` and `cumulativeGasUsed` count gas and `effectiveGasPrice` is a
//! price per unit of gas, so those three stay whole numbers — which is also what lets a
//! client reconcile them: gas x price is the fee, in the currency's smallest unit.
//!
//! Two tables. The first is every network and every transaction the served ledger holds,
//! read out of the engine the router was built on, so its rows span both chains, both of
//! their currencies' precisions, and transactions with and without gas. The second is a
//! hand-written catalog of fees the fixture does not contain — a whole currency unit, a
//! fraction of one, the smallest unit a currency has, and none at all — on every value of
//! the model's `QueryNetwork` enum.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has
//! a single front door for every suite.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::QUERY_NETWORK_VALUES;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::{
    amount_string, network_native_decimals,
};

use crate::chain_query_slice::Emulator;
use crate::native_token_denomination::{
    amount_decimals, amount_units, is_canonical, native_token,
};

/// The route `GetTransaction` is bound to (`@http(uri:)`).
const GET_TRANSACTION: &str = "/get-transaction";

/// The decimal places an amount of `network`'s own currency is quoted with, restated from
/// the chains' published subdivisions rather than borrowed from the implementation: see
/// `native_token_denomination::amount_decimals`, reached through the `tokenId` of the
/// currency the network runs.
fn native_decimals(network: &str) -> u32 {
    amount_decimals(None, Some(native_token(network)))
}

/// A member a response reports as a plain count: present as a string of digits with no
/// decimal point, and equal to the integer the ledger holds.
fn assert_counted(case: &str, transaction: &Value, member: &str, stored: Option<i128>) {
    match stored {
        Some(expected) => {
            let text = transaction[member].as_str().unwrap_or_else(|| {
                panic!("case {case}: the ledger holds {member} {expected}, got {transaction:?}")
            });
            assert_eq!(
                text,
                expected.to_string(),
                "case {case}: {member} counts units no currency subdivides, so it reads as \
                 {expected}, got {text:?}"
            );
        }
        None => assert!(
            transaction[member].is_null(),
            "case {case}: the ledger holds no {member}, got {transaction:?}"
        ),
    }
}

#[tokio::test]
async fn a_transaction_fee_is_quoted_in_the_chains_own_currency() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let decimals = native_decimals(&network.network);
        let mut fees = 0usize;
        let mut fractional = 0usize;
        for stored in &network.transactions {
            let case = format!("{}/{}", network.network, stored.transaction_hash);
            let reply = emulator
                .ok(
                    GET_TRANSACTION,
                    &json!({
                        "network": network.network,
                        "transactionHash": stored.transaction_hash,
                    }),
                )
                .await;
            let transaction = &reply["transaction"];

            match stored.transaction_fee {
                Some(units) => {
                    fees += 1;
                    let text = transaction["transactionFee"].as_str().unwrap_or_else(|| {
                        panic!("case {case}: the ledger holds a fee, got {transaction:?}")
                    });
                    assert!(
                        is_canonical(text, decimals),
                        "case {case}: a fee is a decimal of at most {decimals} places with no \
                         trailing zero, got {text:?}"
                    );
                    assert_eq!(
                        amount_units(text, decimals),
                        Some(units),
                        "case {case}: a fee of {units} of the currency's smallest unit is {decimals} \
                         places down, got {text:?}"
                    );
                    if text.contains('.') {
                        fractional += 1;
                    }
                }
                None => assert!(
                    transaction["transactionFee"].is_null(),
                    "case {case}: the ledger holds no fee, got {transaction:?}"
                ),
            }

            // The gas trio is not an amount of the currency, so it is reported raw — and
            // that is what lets a client cross-check the fee against it.
            assert_counted(&case, transaction, "gasUsed", stored.gas_used);
            assert_counted(
                &case,
                transaction,
                "cumulativeGasUsed",
                stored.cumulative_gas_used,
            );
            assert_counted(
                &case,
                transaction,
                "effectiveGasPrice",
                stored.effective_gas_price,
            );
            if let (Some(gas), Some(price), Some(fee)) = (
                stored.gas_used,
                stored.effective_gas_price,
                transaction["transactionFee"].as_str(),
            ) {
                assert_eq!(
                    amount_units(fee, decimals),
                    Some(gas * price),
                    "case {case}: the fee is the gas burned at the price paid, {gas} x {price} of \
                     the currency's smallest unit, got {fee:?}"
                );
            }
        }
        assert!(
            fees > 0,
            "case {}: the ledger must hold transactions with a fee for this rule to have a \
             case, got none",
            network.network
        );
        // A fee of this ledger is a fraction of a unit of the chain's currency, so quoting
        // it in that currency has to show a fraction somewhere. Without this row the rule
        // could be satisfied by reporting the smallest-unit count unchanged.
        assert!(
            fractional > 0,
            "case {}: a fee of this ledger is a fraction of a unit of its currency, got none",
            network.network
        );
    }
}

#[tokio::test]
async fn a_fee_in_the_smallest_unit_reads_as_the_currency_amount() {
    /// One row of the catalog below: a fee in the smallest unit of the currency the
    /// network runs, and how quoting it in that currency reads.
    struct Row {
        case: &'static str,
        network: &'static str,
        units: i128,
        text: &'static str,
    }

    /// One bitcoin in satoshi, the chain's published subdivision.
    const BTC: i128 = 100_000_000;
    /// One ether in wei, the chain's published subdivision.
    const ETH: i128 = 1_000_000_000_000_000_000;

    // Every value of the model's `QueryNetwork` enum appears, each with fees the fixture
    // ledger does not hold: a whole currency unit, several of them, fractions at the
    // scales a fee actually lands on (a gas burn at a gwei-scale price, a Bitcoin fee of a
    // few thousand satoshi), the smallest unit each currency has, and no fee at all.
    let rows = [
        Row { case: "a 20,000-satoshi fee", network: "BITCOIN_MAINNET", units: 20_000, text: "0.0002" },
        Row { case: "a dust-level fee", network: "BITCOIN_TESTNET", units: 546, text: "0.00000546" },
        Row { case: "one satoshi", network: "BITCOIN_TESTNET", units: 1, text: "0.00000001" },
        Row { case: "a whole bitcoin of fee", network: "BITCOIN_MAINNET", units: BTC, text: "1" },
        Row { case: "every satoshi place used", network: "BITCOIN_MAINNET", units: 199_999_999, text: "1.99999999" },
        Row { case: "no bitcoin fee at all", network: "BITCOIN_TESTNET", units: 0, text: "0" },
        Row { case: "21,000 gas at 20 gwei", network: "ETHEREUM_MAINNET", units: 21_000 * 20_000_000_000, text: "0.00042" },
        Row { case: "100,000 gas at 1 gwei", network: "ETHEREUM_SEPOLIA_TESTNET", units: 100_000 * 1_000_000_000, text: "0.0001" },
        Row { case: "one wei", network: "ETHEREUM_SEPOLIA_TESTNET", units: 1, text: "0.000000000000000001" },
        Row { case: "a whole ether of fee", network: "ETHEREUM_MAINNET", units: ETH, text: "1" },
        Row { case: "two and a half ether of fee", network: "ETHEREUM_MAINNET", units: 5 * ETH / 2, text: "2.5" },
        Row { case: "no ether fee at all", network: "ETHEREUM_MAINNET", units: 0, text: "0" },
    ];

    for network in QUERY_NETWORK_VALUES.iter().copied() {
        let expected = native_decimals(network);
        let got = network_native_decimals(network);
        assert_eq!(
            got, expected,
            "case {network}: an amount of this network's currency is quoted {expected} places \
             down, got {got}"
        );
        assert!(
            rows.iter().any(|row| row.network == network),
            "case {network}: the catalog must carry a fee for every network, got none"
        );
    }

    for Row {
        case,
        network,
        units,
        text: expected,
    } in rows
    {
        let decimals = native_decimals(network);
        let got = amount_string(units, decimals);
        assert_eq!(
            got, expected,
            "case {case}: {units} of {network}'s smallest unit reads as {expected}, got {got}"
        );
        assert_eq!(
            amount_units(&got, decimals),
            Some(units),
            "case {case}: {got} must convert back to {units}"
        );
    }
}

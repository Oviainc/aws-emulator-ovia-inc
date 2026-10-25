// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the rule `ListAssetContracts`' required `deployerAddress`
//! filter rests on: the deployer this index reports for a contract it catalogs is the
//! account that chain records as having deployed it, so the address a client brings from
//! the chain is the address the listing is keyed by.
//!
//! `ContractFilter` makes all three of its members `@required`, so a client cannot list a
//! network's contracts without naming a deployer: if the stored deployer were anything
//! other than the chain's own, the only value that selects a catalogued contract would be
//! one no client could obtain.
//!
//! The one hand-written table is [`PUBLISHED_MAINNET_ERC20`]; the comment on it names
//! where each row comes from. Every assertion is driven over the same
//! `MultiServiceRouter` the production front door serves (the harness in
//! [`crate::chain_query_slice`]).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::QUERY_TOKEN_STANDARD_VALUES;

use crate::chain_query_slice::{drain, unseeded_addresses, Emulator};

/// The route `ListAssetContracts` is bound to (`@http(method: POST, uri:
/// "/list-asset-contracts")`).
const LIST: &str = "/list-asset-contracts";

/// The route `GetAssetContract` is bound to (`@http(method: POST, uri:
/// "/get-asset-contract")`).
const GET: &str = "/get-asset-contract";

/// The response member `ListAssetContracts` carries its items in
/// (`@paginated(items: "contracts")`).
const ITEMS: &str = "contracts";

/// The value of the model's `QueryNetwork` enum naming the chain these deployments are on.
const MAINNET: &str = "ETHEREUM_MAINNET";

/// The `QueryTokenStandard` value a fungible token contract carries.
const ERC20: &str = "ERC20";

/// Published deployments of the Ethereum mainnet, as `(contractAddress, deployerAddress,
/// symbol)`.
///
/// Written out here rather than read from the service, so the test asserts the published
/// facts and not the implementation. Each row is public chain data: the address the token
/// contract is deployed at, the account that deployed it — the sender of its creation
/// transaction, which is what `AssetContract$deployerAddress` ("the address of the
/// contract deployer") reports — and the `symbol` its ERC-20 interface reports, which is
/// what identifies the row.
///
/// USD Coin's row is the one the issue cited (its deployer
/// `0x95ba4cf87d6723ad9c0db21737d862be80e93911`); the other five are not, and they differ
/// from it in both columns.
const PUBLISHED_MAINNET_ERC20: &[(&str, &str, &str)] = &[
    (
        "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "0x95ba4cf87d6723ad9c0db21737d862be80e93911",
        "USDC",
    ),
    (
        "0xdac17f958d2ee523a2206206994597c13d831ec7",
        "0x36928500bc1dcd7af6a2b4008875cc336b927d57",
        "USDT",
    ),
    (
        "0x6b175474e89094c44da98b954eedeac495271d0f",
        "0xddb108893104de4e1c6d0e47c42237db4e617acc",
        "DAI",
    ),
    (
        "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
        "0x4f26ffbe5f04ed43630fdc30a87638d53d0b0876",
        "WETH",
    ),
    (
        "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
        "0xca06411bd7a7296d7dbdd0050dfc846e95febeb7",
        "WBTC",
    ),
    (
        "0x514910771af9ca656af840dff83e8264ecf986ca",
        "0x583431d10d9e2f2a1f71ae4b5f3a3f1d6d1e14e2",
        "LINK",
    ),
];

/// How a test respells an address: a label and the transform.
type Spelling = (&'static str, fn(&str) -> String);

/// A `ListAssetContracts` request whose filter names one network, token standard and
/// deployer — the three members `ContractFilter` requires.
fn list_request(network: &str, token_standard: &str, deployer: &str) -> Value {
    json!({"contractFilter": {
        "network": network,
        "tokenStandard": token_standard,
        "deployerAddress": deployer,
    }})
}

/// A `GetAssetContract` request naming one contract.
fn get_request(network: &str, contract_address: &str) -> Value {
    json!({"contractIdentifier": {"network": network, "contractAddress": contract_address}})
}

/// The `contractAddress` of every item of a listing, as a set: which contracts a filter
/// selected, independent of the order the service pages them in.
fn listed_addresses(items: &[Value]) -> BTreeSet<String> {
    items
        .iter()
        .map(|item| {
            item["contractIdentifier"]["contractAddress"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed contract carries no address: {item:?}"))
                .to_string()
        })
        .collect()
}

/// Every contract `deployer` deployed on `network`, walked to the end of the listing.
async fn contracts_deployed_by(
    emulator: &Emulator,
    network: &str,
    token_standard: &str,
    deployer: &str,
) -> Vec<Value> {
    let body = list_request(network, token_standard, deployer);
    drain(emulator, LIST, ITEMS, &body, None).await
}

#[tokio::test]
async fn a_catalogued_contract_is_listed_under_the_deployer_the_chain_records() {
    let emulator = Emulator::new();

    for (address, deployer, symbol) in PUBLISHED_MAINNET_ERC20 {
        let case = format!("{MAINNET}/{symbol}");
        let listed = contracts_deployed_by(&emulator, MAINNET, ERC20, deployer).await;
        let addresses = listed_addresses(&listed);
        assert!(
            addresses.contains(*address),
            "case {case}: the contract must be listed under its deployer {deployer}, \
             got {addresses:?}"
        );

        for item in &listed {
            assert_eq!(
                item["deployerAddress"].as_str(),
                Some(*deployer),
                "case {case}: every listed contract was deployed by the filter's \
                 deployer, got {item:?}"
            );
            assert_eq!(
                item["tokenStandard"].as_str(),
                Some(ERC20),
                "case {case}: every listed contract carries the filter's token standard, \
                 got {item:?}"
            );
            assert_eq!(
                item["contractIdentifier"]["network"].as_str(),
                Some(MAINNET),
                "case {case}: every listed contract is on the filter's network, \
                 got {item:?}"
            );
        }
    }
}

#[tokio::test]
async fn get_asset_contract_reports_the_deployer_the_listing_is_keyed_by() {
    let emulator = Emulator::new();

    for (address, deployer, symbol) in PUBLISHED_MAINNET_ERC20 {
        let case = format!("{MAINNET}/{symbol}");
        let read = emulator.ok(GET, &get_request(MAINNET, address)).await;
        assert_eq!(
            read["deployerAddress"].as_str(),
            Some(*deployer),
            "case {case}: expected the deployer the chain records, got {read:?}"
        );
        assert_eq!(
            read["metadata"]["symbol"].as_str(),
            Some(*symbol),
            "case {case}: the address must be the contract this row names, got {read:?}"
        );

        // A read must not change what a later caller sees.
        let again = emulator.ok(GET, &get_request(MAINNET, address)).await;
        assert_eq!(
            again, read,
            "case {case}: reading the contract twice must answer the same, got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_deployer_filter_resolves_every_spelling_of_one_account() {
    let emulator = Emulator::new();

    // EIP-55 spends the letter case of an EVM address on a checksum over the SAME 20
    // bytes, so every letter case of one account names that account and a client may send
    // any of them. (The `0x` prefix is part of how this service spells an ACCOUNT, which
    // `crate::chain_query_slice` covers for every address-keyed read alike.)
    let spellings: [Spelling; 4] = [
        ("as published", |address| address.to_string()),
        ("uppercase", |address| {
            format!("0x{}", address.trim_start_matches("0x").to_ascii_uppercase())
        }),
        ("lowercase", |address| address.to_ascii_lowercase()),
        ("mixed case", |address| {
            let body: String = address
                .trim_start_matches("0x")
                .chars()
                .enumerate()
                .map(|(index, character)| {
                    if index % 2 == 0 {
                        character.to_ascii_uppercase()
                    } else {
                        character
                    }
                })
                .collect();
            format!("0x{body}")
        }),
    ];

    for (_, deployer, symbol) in PUBLISHED_MAINNET_ERC20 {
        let published = contracts_deployed_by(&emulator, MAINNET, ERC20, deployer).await;
        let expected = listed_addresses(&published);
        for (label, spell) in spellings {
            let spelled = spell(deployer);
            let case = format!("{MAINNET}/{symbol} ({label}: {spelled})");
            let listed = contracts_deployed_by(&emulator, MAINNET, ERC20, &spelled).await;
            let addresses = listed_addresses(&listed);
            assert_eq!(
                addresses, expected,
                "case {case}: the spelling of the deployer must not change which \
                 contracts are listed, got {addresses:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_deployer_the_index_holds_no_deployment_for_lists_nothing() {
    let emulator = Emulator::new();

    // Accounts OF this chain the fixture index holds no deployment under, plus the
    // CONTRACT addresses themselves: a token contract did not deploy itself, so naming
    // one as the deployer selects nothing even though the index holds it.
    let mut absent = unseeded_addresses(MAINNET);
    absent.extend(
        PUBLISHED_MAINNET_ERC20
            .iter()
            .map(|(address, _, _)| (*address).to_string()),
    );

    for deployer in &absent {
        let case = format!("{MAINNET}/{deployer}");
        let listed = contracts_deployed_by(&emulator, MAINNET, ERC20, deployer).await;
        assert!(
            listed.is_empty(),
            "case {case}: the index holds no contract deployed by it, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_token_standard_narrows_the_deployer_listing() {
    let emulator = Emulator::new();

    // `ContractFilter` requires a `tokenStandard` as well as a deployer, so a deployer's
    // fungible token contract is listed under ERC20 and under no other standard the model
    // declares.
    for (address, deployer, symbol) in PUBLISHED_MAINNET_ERC20 {
        for standard in QUERY_TOKEN_STANDARD_VALUES {
            let case = format!("{MAINNET}/{symbol}/{standard}");
            let listed = contracts_deployed_by(&emulator, MAINNET, standard, deployer).await;
            let addresses = listed_addresses(&listed);
            let expected = *standard == ERC20;
            assert_eq!(
                addresses.contains(*address),
                expected,
                "case {case}: a fungible token contract is listed under {ERC20} only, \
                 got {addresses:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetAssetContract`, driven over HTTP through the same
//! `MultiServiceRouter` the production front door serves (the harness in
//! [`crate::chain_query_slice`]).
//!
//! The service is read-only, so there is no create to round-trip: the thing under test
//! is that the answer is COMPUTED from the seeded ledger. Every test is therefore a
//! table whose rows come from the ledger the router was actually built on — every
//! network, every stored contract, every spelling of an address — rather than from one
//! record copied into the test.
//!
//! The one hand-written table is [`WELL_KNOWN_MAINNET_ERC20`]; the comment on it names
//! where each row comes from.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::QUERY_TOKEN_STANDARD_VALUES;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::{
    ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState as Ledgers, StoredContract,
};

use crate::chain_query_slice::{error_code, Emulator};

/// The route `GetAssetContract` is bound to (`@http(method: POST, uri:
/// "/get-asset-contract")`).
const ROUTE: &str = "/get-asset-contract";

/// The only value the model's `ResourceType` enum declares, so it is the `resourceType`
/// every `ResourceNotFoundException` of this service carries.
const RESOURCE_TYPE: &str = "collection";

/// Well-known ERC-20 token contracts of the Ethereum mainnet, as `(contractAddress,
/// name, symbol, decimals)`.
///
/// Written out here rather than read from the service so the test asserts the published
/// facts and not the implementation: each row is the address the token is deployed at on
/// the Ethereum mainnet together with the `name`, `symbol` and `decimals` its ERC-20
/// interface reports. `GetAssetContract`'s documentation ("Metadata is currently only
/// available for some ERC-20 contracts") is why these are the contracts that carry
/// metadata.
///
/// USDC is the row the issue cited; the others (USDT, WBTC, LINK — note WBTC's 8
/// decimals and USDT's 6, so the rows differ in every member) are not.
const WELL_KNOWN_MAINNET_ERC20: &[(&str, &str, &str, i64)] = &[
    (
        "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "USD Coin",
        "USDC",
        6,
    ),
    (
        "0xdac17f958d2ee523a2206206994597c13d831ec7",
        "Tether USD",
        "USDT",
        6,
    ),
    (
        "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
        "Wrapped BTC",
        "WBTC",
        8,
    ),
    (
        "0x514910771af9ca656af840dff83e8264ecf986ca",
        "ChainLink Token",
        "LINK",
        18,
    ),
];

/// A `GetAssetContract` request naming one contract.
fn request(network: &str, contract_address: &str) -> Value {
    json!({"contractIdentifier": {"network": network, "contractAddress": contract_address}})
}

/// Every contract the seeded ledger holds, read out of the engine the router serves
/// from — the same store the handler reads.
fn seeded_contracts(emulator: &Emulator) -> Vec<StoredContract> {
    let state = emulator.engine.state::<Ledgers>();
    let state = state.lock();
    state
        .networks
        .values()
        .flat_map(|ledger| ledger.contracts.values().cloned())
        .collect()
}

/// Every network the seeded ledger holds, in its own order.
fn networks(emulator: &Emulator) -> Vec<String> {
    let state = emulator.engine.state::<Ledgers>();
    let state = state.lock();
    state.networks.keys().cloned().collect()
}

/// Whether `network` is one of the model's two Bitcoin networks
/// (`QueryNetwork`: BITCOIN_MAINNET | BITCOIN_TESTNET | ETHEREUM_MAINNET |
/// ETHEREUM_SEPOLIA_TESTNET).
fn is_bitcoin(network: &str) -> bool {
    network.starts_with("BITCOIN_")
}

/// Whether `value` satisfies `ChainAddress`'s `@pattern("^[-A-Za-z0-9]{13,74}$")`.
fn is_chain_address(value: &str) -> bool {
    (13..=74).contains(&value.chars().count())
        && value
            .chars()
            .all(|character| character == '-' || character.is_ascii_alphanumeric())
}

/// The stored metadata of `contract` as the wire renders it: the members it holds, and
/// JSON `null` for a contract the index holds no metadata for.
fn metadata_json(contract: &StoredContract) -> Value {
    match &contract.metadata {
        None => Value::Null,
        Some(metadata) => {
            let mut value = json!({});
            if let Some(name) = &metadata.name {
                value["name"] = json!(name);
            }
            if let Some(symbol) = &metadata.symbol {
                value["symbol"] = json!(symbol);
            }
            if let Some(decimals) = metadata.decimals {
                value["decimals"] = json!(decimals);
            }
            value
        }
    }
}

/// The `metadata` of a response, as JSON `null` when the response carries none.
fn reply_metadata(reply: &Value) -> Value {
    reply.get("metadata").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn get_asset_contract_reports_every_seeded_contract() {
    let emulator = Emulator::new();
    let contracts = seeded_contracts(&emulator);
    assert!(
        !contracts.is_empty(),
        "the seeded ledger holds no asset contract, so this operation has nothing to report"
    );

    for contract in &contracts {
        let case = format!("{}/{}", contract.network, contract.contract_address);
        let body = request(&contract.network, &contract.contract_address);
        let reply = emulator.ok(ROUTE, &body).await;

        assert_eq!(
            reply["contractIdentifier"], body["contractIdentifier"],
            "case {case}: the response identifies the contract the request named, got {reply:?}"
        );
        assert_eq!(
            reply["tokenStandard"].as_str(),
            Some(contract.token_standard.as_str()),
            "case {case}: expected the stored tokenStandard {}, got {reply:?}",
            contract.token_standard
        );
        let standard = reply["tokenStandard"].as_str().unwrap_or_default();
        assert!(
            QUERY_TOKEN_STANDARD_VALUES.contains(&standard),
            "case {case}: tokenStandard must be a QueryTokenStandard value, got {standard:?}"
        );
        assert_eq!(
            reply["deployerAddress"].as_str(),
            Some(contract.deployer_address.as_str()),
            "case {case}: expected the stored deployerAddress {}, got {reply:?}",
            contract.deployer_address
        );
        let deployer = reply["deployerAddress"].as_str().unwrap_or_default();
        assert!(
            is_chain_address(deployer),
            "case {case}: deployerAddress must satisfy ChainAddress, got {deployer:?}"
        );
        assert_eq!(
            reply_metadata(&reply),
            metadata_json(contract),
            "case {case}: expected the stored metadata, got {reply:?}"
        );

        // A read must not change what a later caller sees.
        let again = emulator.ok(ROUTE, &body).await;
        assert_eq!(
            again, reply,
            "case {case}: reading the contract twice must answer the same, got {again:?}"
        );
    }
}

/// How a test respells a contract address: a label and the transform.
type Spelling = (&'static str, fn(&str) -> String);

#[tokio::test]
async fn get_asset_contract_resolves_every_spelling_of_one_address() {
    let emulator = Emulator::new();

    // An EVM address is 20 bytes: the `0x` prefix and the mixed case of an EIP-55
    // checksummed spelling are presentation, and `ChainAddress`'s
    // `^[-A-Za-z0-9]{13,74}$` admits all of these, so each names the same contract.
    let spellings: [Spelling; 4] = [
        ("as stored", |address| address.to_string()),
        ("uppercase", |address| address.to_ascii_uppercase()),
        ("no 0x prefix", |address| {
            address.trim_start_matches("0x").to_string()
        }),
        ("uppercase, no prefix", |address| {
            address.trim_start_matches("0x").to_ascii_uppercase()
        }),
    ];

    for contract in seeded_contracts(&emulator) {
        let stored = emulator
            .ok(ROUTE, &request(&contract.network, &contract.contract_address))
            .await;
        for (label, spell) in spellings {
            let spelled = spell(&contract.contract_address);
            let case = format!("{}/{spelled} ({label})", contract.network);
            assert!(
                is_chain_address(&spelled),
                "case {case}: the spelling itself must satisfy ChainAddress"
            );
            let body = request(&contract.network, &spelled);
            let reply = emulator.ok(ROUTE, &body).await;

            for member in ["tokenStandard", "deployerAddress"] {
                assert_eq!(
                    reply[member], stored[member],
                    "case {case}: {member} must not depend on how the address is spelled, \
                     got {reply:?}"
                );
            }
            assert_eq!(
                reply_metadata(&reply),
                reply_metadata(&stored),
                "case {case}: metadata must not depend on how the address is spelled, \
                 got {reply:?}"
            );
            assert_eq!(
                reply["contractIdentifier"], stored["contractIdentifier"],
                "case {case}: the response identifies the contract in the one spelling \
                 the service reports an address in, whichever spelling reached it, \
                 got {reply:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_well_known_mainnet_erc20_contracts_are_served_on_the_mainnet_only() {
    let emulator = Emulator::new();
    let networks = networks(&emulator);
    let mainnet = "ETHEREUM_MAINNET";
    assert!(
        networks.iter().any(|network| network == mainnet),
        "the ledger must hold {mainnet}, got {networks:?}"
    );

    for (address, name, symbol, decimals) in WELL_KNOWN_MAINNET_ERC20 {
        let case = format!("{mainnet}/{symbol}");
        let reply = emulator.ok(ROUTE, &request(mainnet, address)).await;
        assert_eq!(
            reply["tokenStandard"].as_str(),
            Some("ERC20"),
            "case {case}: a token contract is ERC20, got {reply:?}"
        );
        assert_eq!(
            reply_metadata(&reply),
            json!({"name": name, "symbol": symbol, "decimals": decimals}),
            "case {case}: expected the contract's published metadata, got {reply:?}"
        );

        // These are mainnet deployments, so no other network holds them.
        for network in networks.iter().filter(|network| *network != mainnet) {
            let (status, parsed) = emulator.post(ROUTE, &request(network, address)).await;
            let expected = if is_bitcoin(network) { 400 } else { 404 };
            assert_eq!(
                status, expected,
                "case {network}/{symbol}: a mainnet contract is not deployed there, \
                 expected {expected}, got {status} with {parsed:?}"
            );
        }
    }
}

#[tokio::test]
async fn get_asset_contract_is_refused_on_the_bitcoin_networks() {
    let emulator = Emulator::new();
    let contracts = seeded_contracts(&emulator);

    // Every address the ledger knows a contract at, plus a Bitcoin-shaped address: on a
    // Bitcoin network the answer cannot depend on the address, because the operation is
    // not supported there at all.
    let mut addresses: Vec<String> = contracts
        .iter()
        .map(|contract| contract.contract_address.clone())
        .collect();
    addresses.push("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2".to_string());

    for network in networks(&emulator).into_iter().filter(|n| is_bitcoin(n)) {
        for address in &addresses {
            let case = format!("{network}/{address}");
            let body = request(&network, address);
            let parsed = emulator.fails(ROUTE, &body, 400, "ValidationException").await;
            assert!(
                parsed.get("tokenStandard").is_none(),
                "case {case}: a refusal reports no contract, got {parsed:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_address_no_contract_is_deployed_at_is_not_found() {
    let emulator = Emulator::new();
    let contracts = seeded_contracts(&emulator);

    // Well-formed `ChainAddress` values no contract is deployed at: both published
    // length bounds, an EVM-shaped address, and an address the ledger DOES know as a
    // wallet but not as a contract.
    let mut absent: Vec<String> = vec![
        "a".repeat(13),
        "a".repeat(74),
        "0x0000000000000000000000000000000000000000".to_string(),
        "-0deadbeefcafe".to_string(),
    ];
    for contract in &contracts {
        absent.push(contract.deployer_address.clone());
    }

    for network in networks(&emulator).into_iter().filter(|n| !is_bitcoin(n)) {
        for address in &absent {
            let case = format!("{network}/{address}");
            assert!(
                is_chain_address(address),
                "case {case}: the probe must satisfy ChainAddress"
            );
            let body = request(&network, address);
            let parsed = emulator
                .fails(ROUTE, &body, 404, "ResourceNotFoundException")
                .await;
            assert_eq!(
                parsed["resourceId"].as_str(),
                Some(address.as_str()),
                "case {case}: resourceId names the resource that was asked for, got {parsed:?}"
            );
            assert_eq!(
                parsed["resourceType"].as_str(),
                Some(RESOURCE_TYPE),
                "case {case}: resourceType is the modelled ResourceType value, got {parsed:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_address_outside_the_modelled_length_is_a_validation_failure() {
    let emulator = Emulator::new();

    // One past each published bound of `ChainAddress`'s `@pattern` length: refused by
    // the modelled constraint, so it never reaches a lookup and is a 400 rather than the
    // 404 of the in-bounds probes above.
    for length in [12usize, 75] {
        let address = "a".repeat(length);
        let body = request("ETHEREUM_MAINNET", &address);
        let (status, parsed) = emulator.post(ROUTE, &body).await;
        assert_eq!(
            status, 400,
            "case length {length}: outside ChainAddress's 13..=74, expected 400, \
             got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case length {length}: expected ValidationException, got {parsed:?}"
        );
    }
}

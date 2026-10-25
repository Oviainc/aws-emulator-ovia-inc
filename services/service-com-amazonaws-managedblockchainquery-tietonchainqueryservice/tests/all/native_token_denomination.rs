// SPDX-License-Identifier: CC0-1.0

//! Which token an event moves, and the unit an amount of that token is quoted in.
//!
//! Two rules, both properties of the TOKEN rather than of any one operation, so this
//! suite drives every operation that reports either:
//!
//! * `TransactionEvent$tokenId` is "the unique identifier for the token involved in the
//!   transaction", and for a native transfer that identifier is the 3-character
//!   abbreviation of the network's own currency —
//!   `smithy_query(query="shape",shape="TokenIdentifier")` on `tokenId`: "For native
//!   tokens, use the 3 character abbreviation that best matches your token. For example,
//!   btc for Bitcoin, eth for Ether". So a native event names it, and names the same
//!   token the balance it moves is reported under.
//! * an amount of a native token is QUOTED IN THAT CURRENCY, which subdivides into a
//!   published number of smallest units (10^8 satoshi to the bitcoin, 10^18 wei to the
//!   ether), and an amount of a token a CONTRACT issues is quoted at the `decimals` that
//!   contract publishes — the same number `GetAssetContract` reports for it — so a token
//!   whose contract publishes no metadata is the only one quoted as the ledger holds it.
//!   The contract-issued half of that rule is driven end-to-end against what the service
//!   itself publishes in `contract_token_denomination.rs`.
//!
//! Every expectation here is restated from those two sources rather than borrowed from
//! the implementation, and each test is a TABLE: the rows are every network, every
//! transaction, every event and every holding the served ledger holds, crossed with the
//! two event listings — plus, for the rendering rule, a hand-written catalog of amounts
//! spanning a whole currency unit, a sub-unit fraction, the smallest unit a currency has
//! and zero, which the fixture does not all contain.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has
//! a single front door for every suite.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::{
    amount_string, ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState as Ledgers,
    HoldingKey,
};

use crate::chain_query_slice::{token_json, Emulator, FILTERED_EVENTS, TRANSACTION_EVENTS};

/// The 3-character abbreviation of `network`'s own currency: the `tokenId` a native token
/// is named by, from `TokenIdentifier$tokenId`'s "btc for Bitcoin, eth for Ether".
///
/// Read off the network name, which is this model's `QueryNetwork` enum
/// (`BITCOIN_MAINNET | BITCOIN_TESTNET | ETHEREUM_MAINNET | ETHEREUM_SEPOLIA_TESTNET`):
/// a network of a chain is a network of that chain's currency.
pub(crate) fn native_token(network: &str) -> &'static str {
    if network.starts_with("BITCOIN_") {
        "btc"
    } else {
        "eth"
    }
}

/// A contract address in the one spelling that identifies it: an EVM address is 20 bytes,
/// so the `0x` prefix and the letter case of a checksummed spelling are presentation.
pub(crate) fn contract_key(address: &str) -> String {
    let lowercase = address.to_ascii_lowercase();
    lowercase
        .strip_prefix("0x")
        .unwrap_or(&lowercase)
        .to_string()
}

/// The `decimals` this service publishes for the contract deployed at `address` —
/// `ContractMetadata$decimals`, "the decimals used by the token contract", the member
/// `GetAssetContract` reports.
///
/// `None` for a contract the index holds no metadata for and for an address no contract
/// is deployed at: neither publishes a subdivision. Read out of a ledger of this service
/// because WHICH contracts the index holds is the fixture's own fact; that the number
/// here is the one the wire reports is asserted over HTTP in
/// `contract_token_denomination.rs`.
pub(crate) fn published_decimals(address: &str) -> Option<i32> {
    static CATALOG: OnceLock<BTreeMap<String, i32>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let ledgers = Ledgers::default();
            ledgers
                .networks
                .values()
                .flat_map(|ledger| ledger.contracts.values())
                .filter_map(|contract| {
                    let decimals = contract.metadata.as_ref()?.decimals?;
                    Some((contract_key(&contract.contract_address), decimals))
                })
                .collect()
        })
        .get(&contract_key(address))
        .copied()
}

/// How many decimal places an amount of the token `(contract_address, token_id)` names is
/// quoted with.
///
/// Restated from the chains' and the contracts' own published subdivisions rather than
/// borrowed from the implementation: a native token — no contract, the currency's
/// 3-character abbreviation as its `tokenId` — is quoted in the currency, 10^8 satoshi to
/// the bitcoin and 10^18 wei to the ether; a token a contract issues is quoted at the
/// `decimals` that contract publishes ([`published_decimals`]), and one whose contract
/// publishes none is quoted as the ledger holds it.
pub(crate) fn amount_decimals(contract_address: Option<&str>, token_id: Option<&str>) -> u32 {
    match contract_address {
        None => match token_id {
            Some("btc") => 8,
            Some("eth") => 18,
            _ => 0,
        },
        // A scale past 10^38 could not divide an amount held in 128 bits at all.
        Some(address) => published_decimals(address).unwrap_or(0).clamp(0, 38) as u32,
    }
}

/// The decimal places an amount of the token `key` names is quoted with; see
/// [`amount_decimals`].
pub(crate) fn holding_decimals(key: &HoldingKey) -> u32 {
    amount_decimals(key.contract_address.as_deref(), key.token_id.as_deref())
}

/// An amount this service reported, converted back into the token's smallest unit — the
/// inverse of the fixed-point rendering, so a test can compare a reported amount with the
/// integer the ledger holds.
///
/// `None` when `reported` is not a fixed-point decimal of at most `decimals` places,
/// which is itself part of the rule.
pub(crate) fn amount_units(reported: &str, decimals: u32) -> Option<i128> {
    let (sign, digits) = match reported.strip_prefix('-') {
        Some(rest) => (-1i128, rest),
        None => (1i128, reported),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (digits, ""),
    };
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > decimals as usize
    {
        return None;
    }
    let padded = format!("{whole}{fraction:0<width$}", width = decimals as usize);
    padded.parse::<i128>().ok().map(|units| sign * units)
}

/// Whether `reported` is in the CANONICAL form the rendering fixes: a fixed-point decimal
/// of at most `decimals` places, with no trailing zero left in the fraction and no point
/// at all where the amount is whole.
pub(crate) fn is_canonical(reported: &str, decimals: u32) -> bool {
    if amount_units(reported, decimals).is_none() {
        return false;
    }
    match reported.split_once('.') {
        Some((_, fraction)) => !fraction.is_empty() && !fraction.ends_with('0'),
        None => true,
    }
}

/// The currency an event of `event_type` moves, as the 3-character abbreviation that
/// names it — `None` for an event that moves a token a CONTRACT issues, which names the
/// token the event itself recorded (one of its contract's, or none at all for a fungible
/// balance).
///
/// Restated from the token standards rather than borrowed from the implementation: an
/// Ether transfer and a Bitcoin input or output move the network's own currency, an
/// ERC-20 event moves one fungible balance per contract, and an ERC-721/ERC-1155 event
/// moves one token of a contract.
pub(crate) fn native_event_token(event_type: &str) -> Option<&'static str> {
    match event_type {
        "ETH_TRANSFER" | "INTERNAL_ETH_TRANSFER" => Some("eth"),
        "BITCOIN_VIN" | "BITCOIN_VOUT" => Some("btc"),
        _ => None,
    }
}

/// Every event `ListTransactionEvents` reports for one transaction.
async fn events_of(emulator: &Emulator, network: &str, hash: &str) -> Vec<Value> {
    emulator
        .ok(
            TRANSACTION_EVENTS,
            &json!({"network": network, "transactionHash": hash}),
        )
        .await["events"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {network}/{hash}: events is required"))
}

#[tokio::test]
async fn a_native_event_names_the_networks_currency_and_quotes_its_value_in_it() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let mut native_events = 0usize;
        for stored in &network.transactions {
            let got = events_of(&emulator, &network.network, &stored.transaction_hash).await;
            let case = format!("{}/{}", network.network, stored.transaction_hash);
            assert_eq!(
                got.len(),
                stored.events.len(),
                "case {case}: expected {} events, got {got:?}",
                stored.events.len()
            );
            for (position, (event, expected)) in got.iter().zip(&stored.events).enumerate() {
                let case = format!("{case} event {position}");
                let reported = event["tokenId"].as_str();
                if let Some(wanted) = native_event_token(&expected.event_type) {
                    native_events += 1;
                    assert_eq!(
                        reported,
                        Some(wanted),
                        "case {case}: a {} event names the network's own currency, got {event:?}",
                        expected.event_type
                    );
                    assert_eq!(
                        wanted,
                        native_token(&network.network),
                        "case {case}: the currency of this network is {}, got {wanted}",
                        native_token(&network.network)
                    );
                } else {
                    assert_eq!(
                        reported,
                        expected.token_id.as_deref(),
                        "case {case}: expected the token the event records, got {event:?}"
                    );
                }

                // The value, in the unit the token it moves is quoted in.
                let decimals = amount_decimals(expected.contract_address.as_deref(), reported);
                match expected.value {
                    Some(amount) => {
                        let text = event["value"]
                            .as_str()
                            .unwrap_or_else(|| panic!("case {case}: expected a value: {event:?}"));
                        assert!(
                            is_canonical(text, decimals),
                            "case {case}: a value is a decimal of at most {decimals} places \
                             with no trailing zero, got {text:?}"
                        );
                        assert_eq!(
                            amount_units(text, decimals),
                            Some(amount),
                            "case {case}: {amount} of the token's smallest unit is {} places \
                             down, got {text:?}",
                            decimals
                        );
                    }
                    None => assert!(
                        event["value"].is_null(),
                        "case {case}: the event moved no value, got {event:?}"
                    ),
                }
            }
        }
        assert!(
            native_events > 0,
            "case {}: the ledger must hold native-token events for this rule to have a \
             case, got none",
            network.network
        );
    }
}

#[tokio::test]
async fn an_amount_is_rendered_as_the_fixed_point_decimal_of_its_smallest_unit() {
    /// One row of the catalog below: an amount in the token's smallest unit, the token it
    /// is an amount of, and how quoting it in that token's own unit reads.
    struct Row {
        case: &'static str,
        contract: Option<&'static str>,
        token_id: Option<&'static str>,
        units: i128,
        text: &'static str,
    }

    // The subdivisions are the chains' published ones (10^8 satoshi to the bitcoin, 10^18
    // wei to the ether) and the contract rows the `decimals` those token contracts publish
    // (USD Coin 6, Wrapped BTC 8, Dai 18 — the addresses they are deployed at on the
    // Ethereum mainnet). The rows span a whole currency unit, a thousand of them, a
    // sub-unit fraction, a fraction whose last places are zero, every place a currency
    // has, its smallest unit, and zero — amounts the fixture ledger does not all hold.
    const BTC: i128 = 100_000_000;
    const ETH: i128 = 1_000_000_000_000_000_000;
    /// USD Coin, 6 decimals.
    const USDC: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    /// Wrapped BTC, 8 decimals.
    const WBTC: &str = "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599";
    /// Dai Stablecoin, 18 decimals.
    const DAI: &str = "0x6b175474e89094c44da98b954eedeac495271d0f";
    /// An address this index holds no contract — and so no `decimals` — for.
    const UNPUBLISHED: &str = "0x9999999999999999999999999999999999999999";
    let rows = [
        Row { case: "one bitcoin", contract: None, token_id: Some("btc"), units: BTC, text: "1" },
        Row { case: "a thousand bitcoin", contract: None, token_id: Some("btc"), units: 1_000 * BTC, text: "1000" },
        Row { case: "a fiftieth of a bitcoin", contract: None, token_id: Some("btc"), units: BTC / 50, text: "0.02" },
        Row { case: "a dust output", contract: None, token_id: Some("btc"), units: 546, text: "0.00000546" },
        Row { case: "one satoshi", contract: None, token_id: Some("btc"), units: 1, text: "0.00000001" },
        Row { case: "a bitcoin fee", contract: None, token_id: Some("btc"), units: 5_000, text: "0.00005" },
        Row { case: "every satoshi place used", contract: None, token_id: Some("btc"), units: 199_999_999, text: "1.99999999" },
        Row { case: "no bitcoin at all", contract: None, token_id: Some("btc"), units: 0, text: "0" },
        Row { case: "one ether", contract: None, token_id: Some("eth"), units: ETH, text: "1" },
        Row { case: "a thousand ether", contract: None, token_id: Some("eth"), units: 1_000 * ETH, text: "1000" },
        Row { case: "a hundredth of an ether", contract: None, token_id: Some("eth"), units: ETH / 100, text: "0.01" },
        Row { case: "a two-hundredth of an ether", contract: None, token_id: Some("eth"), units: ETH / 200, text: "0.005" },
        Row { case: "one wei", contract: None, token_id: Some("eth"), units: 1, text: "0.000000000000000001" },
        Row { case: "no ether at all", contract: None, token_id: Some("eth"), units: 0, text: "0" },
        Row { case: "a million whole six-decimal tokens", contract: Some(USDC), token_id: None, units: 1_000_000_000_000, text: "1000000" },
        Row { case: "a fraction of a six-decimal token", contract: Some(USDC), token_id: None, units: 119_094_756_568, text: "119094.756568" },
        Row { case: "the smallest unit of a six-decimal token", contract: Some(USDC), token_id: None, units: 1, text: "0.000001" },
        Row { case: "one whole eight-decimal token", contract: Some(WBTC), token_id: None, units: 100_000_000, text: "1" },
        Row { case: "a fraction of an eight-decimal token", contract: Some(WBTC), token_id: None, units: 12_345_678, text: "0.12345678" },
        Row { case: "half an eighteen-decimal token", contract: Some(DAI), token_id: None, units: ETH / 2, text: "0.5" },
        Row { case: "a contract that publishes no decimals", contract: Some(UNPUBLISHED), token_id: None, units: 119_094_756_568, text: "119094756568" },
        Row { case: "one unit of a non-fungible token", contract: Some(UNPUBLISHED), token_id: Some("7"), units: 1, text: "1" },
    ];

    for Row {
        case,
        contract,
        token_id,
        units: amount,
        text: expected,
    } in rows
    {
        let decimals = amount_decimals(contract, token_id);
        let got = amount_string(amount, decimals);
        assert_eq!(
            got, expected,
            "case {case}: {amount} of the smallest unit reads as {expected}, got {got}"
        );
        assert!(
            is_canonical(&got, decimals),
            "case {case}: {got} is not the canonical form of a {decimals}-place amount"
        );
        assert_eq!(
            amount_units(&got, decimals),
            Some(amount),
            "case {case}: {got} must convert back to {amount}"
        );
    }
}

#[tokio::test]
async fn a_balance_is_quoted_at_the_subdivision_its_token_publishes() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let mut fractional = 0usize;
        let mut contract_rows = 0usize;
        for (key, holding) in &network.holdings {
            let case = format!("{}/{}", network.network, key.resource_id());
            let stored = holding
                .history
                .last()
                .unwrap_or_else(|| panic!("case {case}: the holding has no balance"))
                .balance;
            let reply = emulator
                .ok(
                    "/get-token-balance",
                    &json!({
                        "tokenIdentifier": token_json(&network.network, key),
                        "ownerIdentifier": {"address": key.owner_address},
                    }),
                )
                .await;
            let text = reply["balance"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: balance is required, got {reply:?}"));
            let decimals = holding_decimals(key);
            assert!(
                is_canonical(text, decimals),
                "case {case}: a balance is a decimal of at most {decimals} places with no \
                 trailing zero, got {text:?}"
            );
            assert_eq!(
                amount_units(text, decimals),
                Some(stored),
                "case {case}: the holding's {stored} smallest units read as a {decimals}-place \
                 amount, got {text:?}"
            );
            if key.contract_address.is_some() {
                contract_rows += 1;
            }
            if text.contains('.') {
                fractional += 1;
            }
        }
        // The balances of this ledger are not whole units of their token — the events
        // move fractions of one — so quoting them at the subdivision the token publishes
        // has to show a fraction somewhere. Without this row the rule could be satisfied
        // by reporting the smallest-unit count unchanged.
        assert!(
            fractional > 0,
            "case {}: a balance of this ledger is a fraction of a unit of its token, \
             got none",
            network.network
        );
        if !network.network.starts_with("BITCOIN_") {
            assert!(
                contract_rows > 0,
                "case {}: this ledger holds contract-issued balances too, got none",
                network.network
            );
        }
    }
}

#[tokio::test]
async fn both_event_listings_report_one_event_the_same_way() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // `ListFilteredTransactionEvents` "is only supported on the Bitcoin networks".
        if !network.network.starts_with("BITCOIN_") {
            continue;
        }
        let address = network.busiest_address();
        let filtered = emulator
            .ok(
                FILTERED_EVENTS,
                &json!({
                    "network": network.network,
                    "addressIdentifierFilter": {"transactionEventToAddress": [address]},
                    "confirmationStatusFilter": {"include": ["FINAL", "NONFINAL"]},
                    "maxResults": 250,
                }),
            )
            .await["events"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !filtered.is_empty(),
            "case {}/{address}: the busiest address receives events, got none",
            network.network
        );
        for event in &filtered {
            let hash = event["transactionHash"]
                .as_str()
                .unwrap_or_else(|| panic!("case {event:?}: transactionHash is required"));
            let case = format!("{}/{hash}", network.network);
            let unfiltered = events_of(&emulator, &network.network, hash).await;
            assert!(
                unfiltered.contains(event),
                "case {case}: the two listings report one event identically, got \
                 {event:?} against {unfiltered:?}"
            );
            assert_eq!(
                event["tokenId"].as_str(),
                Some(native_token(&network.network)),
                "case {case}: a native event names the network's currency, got {event:?}"
            );
        }
    }
}

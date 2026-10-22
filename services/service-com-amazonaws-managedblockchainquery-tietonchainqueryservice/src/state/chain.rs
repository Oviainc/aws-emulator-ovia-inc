// SPDX-License-Identifier: CC0-1.0

//! The fixture ledger's chain history: what the index holds, and how far back.
//!
//! ManagedBlockchain Query answers out of an index of a PUBLIC chain. This emulator
//! cannot hold one — a public chain's transactions are facts it has no way to compute —
//! so it holds a FIXTURE chain instead, and an identifier the fixture does not hold is
//! answered with the modelled empty page or `ResourceNotFoundException`, never with a
//! history made up from what the request asked for. Nothing in this module reads a
//! request: the whole ledger is built ONCE, from the fixed labels below and the chains'
//! own published facts, when the service's state is constructed.
//!
//! What this fixture gets right that a handful of transactions at a single instant could
//! not:
//!
//! * it SPANS the network's life — from a chain start at or after that network's genesis
//!   block, up to the instant the state was built — so a balance pinned to an instant
//!   years back, and a listing filtered to a month-long window back then, both resolve
//!   instead of falling off the end of a half-hour ledger;
//! * its BUSIEST wallet takes part in every transaction, so any month-long window of the
//!   ledger's life holds several of that wallet's transactions;
//! * it holds [`FIXTURE_TX_COUNT`] transactions per network, [`TX_PER_BLOCK`] to a
//!   block, which is more than the `maxResults` a client pages with and fewer than the
//!   modelled maximum page, so every listing pages without a client having to walk
//!   hundreds of pages;
//! * each block's height follows from its instant at the network's own target block
//!   time, rather than being counted from an arbitrary constant.
//!
//! The ledger is bounded by these constants and the chain's own span, and no request can
//! grow it: [`seed_ledger`] is called once per network from
//! [`crate::state::ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState`]'s
//! [`Default`].

use crate::ops::types::ContractMetadata;

use super::{
    bitcoin_address, build_ledger, digest_hex, erc20_holding, ethereum_address, is_bitcoin,
    native_btc, native_eth, well_known_mainnet_erc20, HoldingKey, NetworkLedger, SeedTx,
    StoredContract, StoredEvent, BTC, CONFIRMATION_FINAL, CONFIRMATION_NONFINAL, ETH,
    ETHEREUM_MAINNET, EXECUTION_FAILED, EXECUTION_SUCCEEDED,
};

// ---------------------------------------------------------------------------
// What the chains themselves fix
// ---------------------------------------------------------------------------

/// The instant `network`'s chain begins: the timestamp of its genesis block, in seconds
/// since the Unix epoch. Nothing on that network can predate it, so no read resolves
/// before it.
///
/// * `BITCOIN_MAINNET` — 2009-01-03T18:15:05Z, the Bitcoin genesis block.
/// * `BITCOIN_TESTNET` — 2011-02-02T23:16:42Z, the testnet3 genesis block.
/// * `ETHEREUM_MAINNET` — 2015-07-30T15:26:13Z, the Ethereum genesis block.
/// * `ETHEREUM_SEPOLIA_TESTNET` — 2021-10-03T15:24:41Z, the Sepolia genesis block.
pub fn network_genesis(network: &str) -> i64 {
    match network {
        "BITCOIN_MAINNET" => 1_231_006_505,
        "BITCOIN_TESTNET" => 1_296_688_602,
        "ETHEREUM_SEPOLIA_TESTNET" => 1_633_267_481,
        // Every other `QueryNetwork` value is an Ethereum mainnet-era network.
        _ => 1_438_269_973,
    }
}

/// The seconds between blocks `network` targets: Bitcoin targets one block every 10
/// minutes, Ethereum one block per 12-second slot.
fn seconds_per_block(network: &str) -> i64 {
    if is_bitcoin(network) {
        600
    } else {
        12
    }
}

/// The height of the block holding `time`: the blocks `network` produces, at its own
/// target rate, between its genesis block and that instant.
pub(super) fn block_height(network: &str, time: i64) -> i64 {
    (time - network_genesis(network)).max(0) / seconds_per_block(network)
}

// ---------------------------------------------------------------------------
// What the fixture chooses
// ---------------------------------------------------------------------------

/// How many transactions the fixture holds per network.
///
/// Nothing AWS publishes fixes this. It is chosen between two bounds the model sets:
/// above the `maxResults` a client pages with (`@range(min: 1, max: 250)`), so every
/// listing really pages, and below the 250 a single page may carry, so a client that
/// asks for no `maxResults` still sees a whole address's history at once.
const FIXTURE_TX_COUNT: i64 = 160;

/// How many transactions share a fixture block: more than one, so
/// `Transaction$transactionIndex` and `Transaction$numberOfTransactions` are counted
/// over a block that holds several.
const TX_PER_BLOCK: i64 = 2;

/// The interval between the fixture's blocks (28 days).
///
/// Chosen, like [`FIXTURE_TX_COUNT`], rather than published: short enough that every
/// month-long window of the ledger's life holds a transaction of the busiest wallet, and
/// long enough that [`FIXTURE_TX_COUNT`] transactions reach back more than six years —
/// so a read pinned years into the past resolves.
pub(super) const FIXTURE_BLOCK_INTERVAL: i64 = 28 * 24 * 60 * 60;

/// The wallets the fixture holds, as the labels their addresses are derived from.
///
/// The FIRST is the busiest: it takes part in every transaction, the way an exchange's
/// hot wallet does, so a window- or page-scoped read of it is never empty. The rest are
/// its counterparties.
const WALLETS: [&str; 5] = [
    "wallet-a",
    "wallet-b",
    "wallet-c",
    "wallet-d",
    "wallet-e",
];

/// The gas a fixture transaction burns: the 21,000 the protocol charges for a plain
/// transfer, and the larger amounts a token contract's `transfer`, a mint, a revert and a
/// two-event call cost.
const GAS_NATIVE: i128 = 21_000;
/// See [`GAS_NATIVE`].
const GAS_TOKEN: i128 = 52_000;
/// See [`GAS_NATIVE`].
const GAS_MINT: i128 = 84_000;
/// See [`GAS_NATIVE`].
const GAS_REVERTED: i128 = 23_000;
/// See [`GAS_NATIVE`].
const GAS_MULTI_EVENT: i128 = 96_000;

/// What a fixture transfer moves of the network's own currency, in its smallest unit
/// (wei, satoshi). On the Ethereum networks the busiest wallet receives more than it
/// sends, so its balance at every instant of the ledger's life is a positive integer; on
/// the Bitcoin networks every wallet opens with far more than the ledger can move, since
/// each transfer there also pays a fee.
const NATIVE_IN_WEI: i128 = ETH / 50;
/// See [`NATIVE_IN_WEI`].
const NATIVE_OUT_WEI: i128 = ETH / 100;
/// See [`NATIVE_IN_WEI`].
const NATIVE_INTERNAL_WEI: i128 = ETH / 200;
/// What the first transfer of each Bitcoin pair is funded with, less its fee; see
/// [`NATIVE_IN_WEI`].
const NATIVE_OUT_SATOSHI: i128 = BTC / 50;

/// What a Bitcoin fixture transaction pays in fees, in satoshi: the difference between
/// what its input spent and what its output created.
const BITCOIN_FEE: i128 = 5_000;

/// What a fixture fungible-token transfer moves, in the contract's own smallest unit, and
/// the supply its deployer opened with.
const TOKEN_OUT: i128 = 250_000;
/// See [`TOKEN_OUT`].
const TOKEN_IN: i128 = 100_000;
/// See [`TOKEN_OUT`].
const TOKEN_INTERNAL: i128 = 50_000;
/// See [`TOKEN_OUT`].
const TOKEN_SUPPLY: i128 = 1_000_000_000_000_000;

/// How many whole tokens of a well-known contract a fixture wallet opens with, and how
/// many of them it sends on: whole tokens, scaled by that contract's own `decimals`.
const TOKEN_UNITS_HELD: i128 = 1_000_000;
/// See [`TOKEN_UNITS_HELD`].
const TOKEN_UNITS_SENT: i128 = 1_250;

/// How many transfers of a well-known contract the fixture holds: more than one, so the
/// contract has several holders and a holder listing is not one row.
const TRANSFERS_PER_CONTRACT: i64 = 3;

/// How many contracts of the Ethereum fixture are its own synthetic ones (one fungible,
/// one not); the rest of its contract list is the well-known mainnet contracts.
const SYNTHETIC_CONTRACTS: usize = 2;

/// How many transactions one turn of the Ethereum fixture's pattern holds — the shapes
/// a client has to be able to read: a native transfer out and one in, a fungible-token
/// transfer out and one in, a non-fungible mint and the onward transfer of that same
/// token, a transaction that reverted, and one that emitted two events.
const PATTERN_LENGTH: usize = 8;

/// What each fixture wallet opens with, in the network's smallest unit: more than the
/// whole ledger can move, so no balance the fixture reports is negative.
const WALLET_OPENING_WEI: i128 = 1_000 * ETH;
/// See [`WALLET_OPENING_WEI`].
const WALLET_OPENING_SATOSHI: i128 = 1_000 * BTC;

// ---------------------------------------------------------------------------
// Building one network's fixture
// ---------------------------------------------------------------------------

/// When the fixture's history starts on `network`, given the instant `tip` its state was
/// built at: [`FIXTURE_TX_COUNT`] transactions back, or the network's genesis block when
/// the chain itself is younger than that.
fn chain_start(network: &str, tip: i64) -> i64 {
    let span = (FIXTURE_TX_COUNT / TX_PER_BLOCK) * FIXTURE_BLOCK_INTERVAL;
    (tip - span).max(network_genesis(network))
}

/// How many blocks the fixture holds on `network`: one every
/// [`FIXTURE_BLOCK_INTERVAL`] from [`chain_start`] up to the tip, and never more than
/// [`FIXTURE_TX_COUNT`] transactions' worth.
fn fixture_blocks(network: &str, tip: i64) -> i64 {
    let blocks = (tip - chain_start(network, tip)) / FIXTURE_BLOCK_INTERVAL;
    blocks.clamp(1, FIXTURE_TX_COUNT / TX_PER_BLOCK)
}

/// The fixture ledger of one network, as of the instant `tip` the state was built at.
pub(super) fn seed_ledger(network: &str, tip: i64) -> NetworkLedger {
    if is_bitcoin(network) {
        bitcoin_fixture(network, tip)
    } else {
        ethereum_fixture(network, tip)
    }
}

/// The address of the fixture wallet in slot `slot`, in the shape `network` spells
/// addresses in.
///
/// On a network whose published accounts this emulator catalogs
/// ([`super::derived::mainnet_account`]) the slots ARE those accounts, so a read of an
/// address a client of that chain actually looks up is answered out of this fixture
/// rather than with the modelled not-found; every other network derives a synthetic
/// address from the slot's label.
fn wallet(network: &str, slot: usize) -> String {
    if let Some(published) = super::derived::mainnet_account(network, slot % WALLETS.len()) {
        return published.to_string();
    }
    let label = WALLETS[slot % WALLETS.len()];
    if is_bitcoin(network) {
        bitcoin_address(network, label)
    } else {
        ethereum_address(network, label)
    }
}

/// Which counterparty transaction `index` is with: the busiest wallet transacts with each
/// of the others in turn, one for a whole CYCLE of the pattern below.
///
/// Per cycle and not per transaction, because the transactions of one cycle depend on
/// each other: the tokens one of them sends are the tokens an earlier one received, and
/// the non-fungible token one of them moves is the one minted to the same address a
/// transaction earlier. A counterparty that changed inside a cycle would be replaying a
/// transfer from an address that never received what it sends, which settles to a
/// negative balance and to one non-fungible token held by two addresses.
fn counterparty(network: &str, index: i64) -> String {
    let cycle = index as usize / PATTERN_LENGTH;
    let slot = 1 + (cycle % (WALLETS.len() - 1));
    wallet(network, slot)
}

/// A fixture transfer event of an Ethereum network's own currency.
fn native_transfer(kind: &str, from: &str, to: &str, value: i128) -> StoredEvent {
    StoredEvent {
        event_type: kind.to_string(),
        from: Some(from.to_string()),
        to: Some(to.to_string()),
        value: Some(value),
        ..StoredEvent::default()
    }
}

/// A fixture fungible-token transfer event: one fungible balance per contract, so no
/// token id.
fn token_transfer(contract: &str, from: &str, to: &str, value: i128) -> StoredEvent {
    StoredEvent {
        event_type: "ERC20_TRANSFER".to_string(),
        from: Some(from.to_string()),
        to: Some(to.to_string()),
        value: Some(value),
        contract_address: Some(contract.to_string()),
        ..StoredEvent::default()
    }
}

/// The id of the non-fungible token minted by transaction `index`.
///
/// `TokenIdentifier$tokenId`: "For all other token types you must specify the `tokenId`
/// in the 64 character hexadecimal `tokenid` format", so a fixture token id is a
/// 64-character hex digest.
fn token_id(network: &str, index: i64) -> String {
    digest_hex(&format!("{network}/token-id/{index}"))
}

/// The Ethereum fixture: five wallets, two synthetic contracts — plus, on the mainnet,
/// the well-known token contracts of [`super::ETHEREUM_MAINNET_ERC20`], which this
/// ledger's own wallets hold and move balances of — and transactions that move native
/// Ether, a fungible balance and non-fungible tokens, and that once every cycle revert
/// or emit two events, so a client sees those shapes too.
fn ethereum_fixture(network: &str, tip: i64) -> NetworkLedger {
    let busy = wallet(network, 0);
    let erc20 = ethereum_address(network, "contract-erc20");
    let erc721 = ethereum_address(network, "contract-erc721");
    let hash = |index: i64| format!("0x{}", digest_hex(&format!("{network}/tx/{index}")));

    let mut contracts = vec![
        StoredContract {
            network: network.to_string(),
            contract_address: erc20.clone(),
            token_standard: "ERC20".to_string(),
            deployer_address: busy.clone(),
            metadata: Some(ContractMetadata {
                name: Some("Emulated Fungible Token".to_string()),
                symbol: Some("EFT".to_string()),
                decimals: Some(18),
            }),
        },
        StoredContract {
            network: network.to_string(),
            contract_address: erc721.clone(),
            token_standard: "ERC721".to_string(),
            deployer_address: busy.clone(),
            // `GetAssetContract`'s documentation: "Metadata is currently only available
            // for some ERC-20 contracts", so this index holds metadata for ERC-20
            // contracts only and a non-fungible contract carries none.
            metadata: None,
        },
    ];
    // The well-known token contracts of the Ethereum mainnet are deployments of THAT
    // chain, so they belong to its ledger and to no other network's.
    if network == ETHEREUM_MAINNET {
        contracts.extend(well_known_mainnet_erc20(network));
    }

    // The balances the ledger opens with, at the instant the fixture's chain starts.
    // Every later balance is these plus the events below, never a number written down on
    // its own.
    let mut opening: Vec<(HoldingKey, i128)> = (0..WALLETS.len())
        .map(|slot| (native_eth(&wallet(network, slot)), WALLET_OPENING_WEI))
        .collect();
    // The fungible contract's whole supply opens in the hands of the wallet that
    // deployed it.
    opening.push((erc20_holding(&erc20, &busy), TOKEN_SUPPLY));

    let blocks = fixture_blocks(network, tip);
    let last_block = blocks - 1;
    let mut seeds = Vec::new();
    for index in 0..blocks * TX_PER_BLOCK {
        let block = index / TX_PER_BLOCK;
        let other = counterparty(network, index);
        // The transaction at the chain tip has no later block confirming it.
        let confirmation_status = if block == last_block {
            CONFIRMATION_NONFINAL
        } else {
            CONFIRMATION_FINAL
        };
        let mut seed = SeedTx {
            hash: hash(index),
            transaction_id: None,
            block,
            from: Some(busy.clone()),
            to: other.clone(),
            contract_address: None,
            gas_used: Some(GAS_NATIVE),
            confirmation_status,
            execution_status: Some(EXECUTION_SUCCEEDED),
            events: Vec::new(),
        };
        match index as usize % PATTERN_LENGTH {
            // The busiest wallet sends native Ether: the transaction's own `to` is the
            // recipient.
            0 => {
                seed.events = vec![native_transfer(
                    "ETH_TRANSFER",
                    &busy,
                    &other,
                    NATIVE_OUT_WEI,
                )];
            }
            // And receives it.
            1 => {
                seed.from = Some(other.clone());
                seed.to = busy.clone();
                seed.events = vec![native_transfer(
                    "ETH_TRANSFER",
                    &other,
                    &busy,
                    NATIVE_IN_WEI,
                )];
            }
            // A fungible-token transfer out: the transaction's `to` is the CONTRACT, and
            // the wallet that received the tokens appears only on the event.
            2 => {
                seed.to = erc20.clone();
                seed.contract_address = Some(erc20.clone());
                seed.gas_used = Some(GAS_TOKEN);
                seed.events = vec![token_transfer(&erc20, &busy, &other, TOKEN_OUT)];
            }
            // And one back in, sent by the counterparty.
            3 => {
                seed.from = Some(other.clone());
                seed.to = erc20.clone();
                seed.contract_address = Some(erc20.clone());
                seed.gas_used = Some(GAS_TOKEN);
                seed.events = vec![token_transfer(&erc20, &other, &busy, TOKEN_IN)];
            }
            // A non-fungible token is minted to the counterparty: one token of a
            // contract moves, so the event carries both the contract and the token id,
            // and a mint has no sender — the token did not exist before it.
            4 => {
                seed.to = erc721.clone();
                seed.contract_address = Some(erc721.clone());
                seed.gas_used = Some(GAS_MINT);
                seed.events = vec![StoredEvent {
                    event_type: "ERC721_TRANSFER".to_string(),
                    from: None,
                    to: Some(other.clone()),
                    value: Some(1),
                    contract_address: Some(erc721.clone()),
                    token_id: Some(token_id(network, index)),
                    ..StoredEvent::default()
                }];
            }
            // And moves on to the busiest wallet: the token the PREVIOUS transaction
            // minted, and minted to this same counterparty (one counterparty per cycle),
            // so no balance is ever moved by an owner who does not hold it.
            5 => {
                let minted_to = counterparty(network, index - 1);
                seed.from = Some(minted_to.clone());
                seed.to = erc721.clone();
                seed.contract_address = Some(erc721.clone());
                seed.gas_used = Some(GAS_MINT);
                seed.events = vec![StoredEvent {
                    event_type: "ERC721_TRANSFER".to_string(),
                    from: Some(minted_to),
                    to: Some(busy.clone()),
                    value: Some(1),
                    contract_address: Some(erc721.clone()),
                    token_id: Some(token_id(network, index - 1)),
                    ..StoredEvent::default()
                }];
            }
            // A transaction that reverted: it is on the chain and `GetTransaction`
            // returns it, but it emitted no events and moved no value.
            6 => {
                seed.to = erc20.clone();
                seed.contract_address = Some(erc20.clone());
                seed.gas_used = Some(GAS_REVERTED);
                seed.execution_status = Some(EXECUTION_FAILED);
            }
            // One transaction, two events: `ListTransactionEvents` returns them in the
            // order they were emitted.
            _ => {
                seed.to = erc20.clone();
                seed.contract_address = Some(erc20.clone());
                seed.gas_used = Some(GAS_MULTI_EVENT);
                seed.events = vec![
                    token_transfer(&erc20, &busy, &other, TOKEN_INTERNAL),
                    native_transfer(
                        "INTERNAL_ETH_TRANSFER",
                        &busy,
                        &other,
                        NATIVE_INTERNAL_WEI,
                    ),
                ];
            }
        }
        seeds.push(seed);
    }

    // The ledger's own wallets hold the well-known token contracts too, and move them:
    // a contract this index holds without a single holder would report an empty holder
    // listing, which is indistinguishable from "this contract has no holders". The
    // holders are this fixture's wallets — the index claims no real holder of a real
    // contract — and each transfer is sized in the contract's own units, from the
    // `decimals` its metadata reports.
    let well_known: Vec<(String, i32)> = contracts
        .iter()
        .skip(SYNTHETIC_CONTRACTS)
        .map(|contract| {
            (
                contract.contract_address.clone(),
                contract
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.decimals)
                    .unwrap_or(0),
            )
        })
        .collect();
    for (slot, (contract, decimals)) in well_known.iter().enumerate() {
        let unit = 10i128.pow((*decimals).clamp(0, 18) as u32);
        opening.push((erc20_holding(contract, &busy), TOKEN_UNITS_HELD * unit));
        for round in 0..TRANSFERS_PER_CONTRACT {
            let index = blocks * TX_PER_BLOCK + (slot as i64 * TRANSFERS_PER_CONTRACT) + round;
            let other = counterparty(network, index);
            let block = ((slot as i64 * TRANSFERS_PER_CONTRACT + round + 1) * blocks)
                / (well_known.len() as i64 * TRANSFERS_PER_CONTRACT + 1);
            seeds.push(SeedTx {
                hash: hash(index),
                transaction_id: None,
                block,
                from: Some(busy.clone()),
                to: contract.clone(),
                contract_address: Some(contract.clone()),
                gas_used: Some(GAS_TOKEN),
                confirmation_status: CONFIRMATION_FINAL,
                execution_status: Some(EXECUTION_SUCCEEDED),
                events: vec![token_transfer(
                    contract,
                    &busy,
                    &other,
                    TOKEN_UNITS_SENT * unit,
                )],
            });
        }
    }

    build_ledger(network, seeds, opening, contracts, chain_start(network, tip))
}

/// The Bitcoin fixture: five wallets whose transfers each spend one input and create one
/// output, so every balance moves through a `BITCOIN_VIN` / `BITCOIN_VOUT` pair and the
/// fee is what the input spent beyond the output.
///
/// The transfers go in pairs, and the SECOND of each pair spends the output the first
/// created, so half this ledger's outputs are spent and half unspent — the distinction
/// `TransactionEvent$voutSpent` reports and `ListFilteredTransactionEvents`'s
/// `voutFilter` selects on. Which ones are spent is DERIVED by [`build_ledger`] from the
/// inputs that name them, never written down beside the output.
fn bitcoin_fixture(network: &str, tip: i64) -> NetworkLedger {
    let busy = wallet(network, 0);
    // A Bitcoin transaction hash is conventionally written bare, without a `0x` prefix;
    // the modelled pattern admits both spellings and `normalize_hash` makes them equal.
    let hash = |index: i64| digest_hex(&format!("{network}/tx/{index}"));
    let txid = |index: i64| digest_hex(&format!("{network}/txid/{index}"));
    // The outpoint a transfer that is NOT funded by this ledger spends: output 0 of a
    // transaction this ledger does not hold.
    let funding_hash = |index: i64| digest_hex(&format!("{network}/funding/{index}"));
    let funding_id = |index: i64| digest_hex(&format!("{network}/funding-id/{index}"));

    let opening: Vec<(HoldingKey, i128)> = (0..WALLETS.len())
        .map(|slot| (native_btc(&wallet(network, slot)), WALLET_OPENING_SATOSHI))
        .collect();

    let blocks = fixture_blocks(network, tip);
    let last_block = blocks - 1;
    let mut seeds = Vec::new();
    // What the previous transaction's output is worth and who holds it, so the
    // transaction that spends it spends exactly that.
    let mut previous_output: Option<(i128, String)> = None;
    for index in 0..blocks * TX_PER_BLOCK {
        let block = index / TX_PER_BLOCK;
        let other = counterparty(network, index);
        // The first transfer of each pair is funded from outside this ledger and pays a
        // counterparty; the second spends that very output — which is why its sender is
        // the address that received it — and pays the busiest wallet, less the fee.
        let (spent_value, spent_hash, spent_id, from, to) = match previous_output.take() {
            Some((value, owner)) => (value, hash(index - 1), txid(index - 1), owner, busy.clone()),
            None => (
                NATIVE_OUT_SATOSHI + BITCOIN_FEE,
                funding_hash(index),
                funding_id(index),
                busy.clone(),
                other,
            ),
        };
        // A Bitcoin output is worth what the input it spends was worth, less the fee, so
        // what this transfer moves is computed rather than declared.
        let value = spent_value - BITCOIN_FEE;
        // Only an output this ledger's own next transaction spends is carried forward;
        // the second of each pair leaves its output unspent.
        if index % TX_PER_BLOCK == 0 {
            previous_output = Some((value, to.clone()));
        }
        seeds.push(SeedTx {
            hash: hash(index),
            transaction_id: Some(txid(index)),
            block,
            from: Some(from.clone()),
            to: to.clone(),
            contract_address: None,
            // Bitcoin has no gas: the fee is the difference between what the inputs
            // spent and what the outputs created, which `build_ledger` computes.
            gas_used: None,
            confirmation_status: if block == last_block {
                CONFIRMATION_NONFINAL
            } else {
                CONFIRMATION_FINAL
            },
            // There is no contract execution to succeed or fail on Bitcoin.
            execution_status: None,
            events: vec![
                StoredEvent {
                    event_type: "BITCOIN_VIN".to_string(),
                    from: Some(from),
                    // The input spends the whole output it names, fee included.
                    value: Some(spent_value),
                    spent_vout_transaction_hash: Some(spent_hash),
                    spent_vout_transaction_id: Some(spent_id),
                    spent_vout_index: Some(0),
                    ..StoredEvent::default()
                },
                StoredEvent {
                    event_type: "BITCOIN_VOUT".to_string(),
                    to: Some(to),
                    value: Some(value),
                    vout_index: Some(0),
                    // `voutSpent` is derived by `build_ledger` from the inputs that
                    // spend this output.
                    ..StoredEvent::default()
                },
            ],
        });
    }

    // The Bitcoin networks have no asset contracts: `GetAssetContract`'s documentation
    // is explicit that "the Bitcoin blockchain networks do not support this operation".
    build_ledger(
        network,
        seeds,
        opening,
        Vec::new(),
        chain_start(network, tip),
    )
}

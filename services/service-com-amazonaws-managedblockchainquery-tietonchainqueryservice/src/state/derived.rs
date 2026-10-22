// SPDX-License-Identifier: CC0-1.0

//! The PUBLISHED identifiers the mainnet ledgers are seeded with, and the spelling an
//! address is looked up by.
//!
//! # Why a mainnet ledger holds published accounts
//!
//! ManagedBlockchain Query answers out of an index of a PUBLIC chain, and the
//! identifiers a client reads it with are that chain's own: the token contracts everyone
//! trades and the accounts everyone looks up. [`super::ETHEREUM_MAINNET_ERC20`] already
//! seeds the mainnet ledger with the first half of that — the well-known ERC-20
//! contracts, so `GetAssetContract` answers for a contract a client brings rather than
//! only for a synthetic one. Without the second half, every balance and transaction read
//! of a real mainnet ACCOUNT answered `ResourceNotFoundException` or an empty page,
//! which a client cannot tell apart from "this wallet is empty".
//!
//! So on the mainnets the wallets of [`super::chain`]'s fixture ARE these published
//! accounts: [`mainnet_account`] hands the chain builder one per wallet slot, and
//! everything that fixture gives a wallet — a history spanning the network's life, a
//! balance of the network's own currency and of every well-known contract, transfers a
//! window and a page can select — a published account gets too. The testnets keep the
//! synthetic labels: a mainnet account is not deployed on a testnet, the same rule
//! [`super::well_known_mainnet_erc20`] follows for the contracts.
//!
//! # What is and is not claimed
//!
//! The catalogs claim only the IDENTIFIERS: the address each account is published at,
//! which is public chain data. Every balance, transfer and timestamp attached to them is
//! [`super::chain`]'s fixture, built once when the state is constructed — nothing is read
//! off the real chain, which this emulator does not index, and nothing is computed from a
//! request. An identifier no catalog and no fixture names is answered with the modelled
//! `ResourceNotFoundException` or the modelled empty page.

/// The spelling an address is looked up by.
///
/// The letter case of a hex address carries no information about the account it names:
/// EIP-55 spends it on a checksum over the SAME 20 bytes, so `0xD8dA...` and `0xd8da...`
/// are one account and a client may send either. Every other form — a base58 Bitcoin
/// address, a label — is case-SENSITIVE (base58 drops `0`, `O`, `I` and `l` precisely so
/// characters cannot be confused), so it is left exactly as it was spelled.
///
/// The ledger stores its addresses in this form and every address-keyed lookup goes
/// through it ([`super::NetworkLedger::for_address`],
/// [`super::HoldingKey::normalized`]), which is what makes the two spellings of one
/// account read one balance and one history.
pub fn normalize_address(address: &str) -> String {
    let is_hex = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .is_some_and(|body| !body.is_empty() && body.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if is_hex {
        address.to_ascii_lowercase()
    } else {
        address.to_string()
    }
}

/// Published accounts of the Ethereum mainnet.
///
/// Public chain data: each row is an address anyone can read the holdings of off that
/// chain, and every one of them is an Ethereum account — `0x` and the 40 hex digits of 20
/// bytes. They are here because they are the accounts a client of this API asks about:
/// the balance of a known wallet and the transactions it took part in are the two most
/// common calls this service serves.
const ETHEREUM_MAINNET_ACCOUNTS: &[&str] = &[
    // The Ethereum co-founder's account, published as the ENS name `vitalik.eth`.
    "0xd8da6bf26964af9d7eed9e03e53415d37aa96045",
    // The Ethereum Foundation's account.
    "0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae",
    // The beacon chain deposit contract, which holds the mainnet's staked Ether.
    "0x00000000219ab540356cbb839cbe05303d7705fa",
    // Two widely-published exchange wallets.
    "0x3f5ce5fbfe3e9af3971dd833d26ba9b5c936f0be",
    "0x742d35cc6634c0532925a3b844bc454e4438f44e",
];

/// Published accounts of the Bitcoin mainnet; see [`ETHEREUM_MAINNET_ACCOUNTS`]. Each is
/// a base58 address of that chain.
const BITCOIN_MAINNET_ACCOUNTS: &[&str] = &[
    // An early published address, cited as the recipient of the first Bitcoin payment
    // between two parties (block 170).
    "1Q2TWHE3GMdB6BZKafqwxXtWAWgFt5Jvm3",
    // A widely-published large holder.
    "1FeexV6bAHb8ybZjqQMjJrcCrHGW9sb6uF",
];

/// The published account the fixture's wallet slot `slot` is, on a network that has a
/// catalog — `None` on every other network, where the fixture derives a synthetic
/// address for the slot instead.
///
/// The slot order is the catalog order, so the FIRST catalogued account is the fixture's
/// busiest wallet: the account a client is most likely to read is the one whose history
/// no window or page comes back empty for.
pub(super) fn mainnet_account(network: &str, slot: usize) -> Option<&'static str> {
    catalogued_accounts(network).get(slot).copied()
}

/// Every account the catalogs name on `network`, in slot order; empty for a network with
/// no catalog.
pub fn catalogued_accounts(network: &str) -> &'static [&'static str] {
    if network == super::ETHEREUM_MAINNET {
        ETHEREUM_MAINNET_ACCOUNTS
    } else if network == BITCOIN_MAINNET {
        BITCOIN_MAINNET_ACCOUNTS
    } else {
        &[]
    }
}

/// The value of the model's `QueryNetwork` enum that names the Bitcoin mainnet, from
/// [`super::BITCOIN_NETWORKS`].
const BITCOIN_MAINNET: &str = super::BITCOIN_NETWORKS[0];

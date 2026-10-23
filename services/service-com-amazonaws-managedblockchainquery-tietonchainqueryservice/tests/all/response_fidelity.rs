// SPDX-License-Identifier: CC0-1.0

//! The CONTENT of this service's query answers, as distinct from their shape: which
//! spelling an address comes back in, how a refusal reports every field it refused, what
//! a batch error item says about the item that failed, and that the chain facts a read
//! reports are the ones the index holds.
//!
//! Five rules are under test, each asserted over a TABLE spanning the class it describes
//! rather than over the one request that exposed it:
//!
//! 1. Every `ChainAddress`-typed member of a response is reported in the ONE spelling
//!    this API uses — a hex address lowercased, since EIP-55 spends its letter case on a
//!    checksum over the same 20 bytes — whichever spelling the request reached it with.
//! 2. A request refused on this service's own rules carries one `fieldList` entry per
//!    VIOLATED FIELD: every field that failed, in one response.
//! 3. The address filter of `ListFilteredTransactionEvents`, which "is only supported on
//!    the Bitcoin networks", admits the address encodings Bitcoin defines and refuses
//!    what is not one of them.
//! 4. A `BatchGetTokenBalanceErrorItem` reports the HTTP STATUS of the per-item failure
//!    in `errorCode` (the exception's identity being `errorType`'s job) and identifies
//!    the item that failed, instant included.
//! 5. A balance or contract fact is READ from the chain data the service holds, so the
//!    single read, the batch read and the listing agree, and a published account of a
//!    mainnet is answered out of stored records rather than with an empty page.
//!
//! Every row is generated from the ledger the service actually serves or from a published
//! encoding rule named in the comment beside it; nothing is copied out of the
//! implementation.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::derived;

use crate::chain_query_slice::{
    drain, error_code, refusal_explanation, unseeded_bitcoin_address, Emulator,
    GENERIC_VALIDATION_MESSAGE,
};

/// Every response member of this service that carries a `ChainAddress`:
/// `OwnerIdentifier$address`, `ContractIdentifier$contractAddress` /
/// `TokenIdentifier$contractAddress` / `Transaction$contractAddress` /
/// `TransactionEvent$contractAddress`, `AssetContract$deployerAddress` and the
/// `from`/`to` of a `Transaction` and a `TransactionEvent`.
const CHAIN_ADDRESS_MEMBERS: [&str; 5] = [
    "address",
    "contractAddress",
    "deployerAddress",
    "from",
    "to",
];

/// The modelled `ValidationExceptionReason` beside the generic top-level message
/// (`crate::chain_query_slice::GENERIC_VALIDATION_MESSAGE`): `fieldValidationFailed` is a
/// `ValidationExceptionReason` value.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// Whether `network` is one of the model's two Bitcoin networks (`QueryNetwork`:
/// BITCOIN_MAINNET | BITCOIN_TESTNET | ETHEREUM_MAINNET | ETHEREUM_SEPOLIA_TESTNET).
fn is_bitcoin(network: &str) -> bool {
    network.starts_with("BITCOIN_")
}

/// Whether `address` is in the one spelling this API reports an address in.
///
/// The letter case of a HEX address carries no information about the account it names —
/// EIP-55 spends it on a checksum over the same 20 bytes — so one account has one
/// rendering, the lowercase one. Every other form (a base58 Bitcoin address, a label) is
/// case-SENSITIVE and is reported exactly as the ledger holds it.
fn is_canonical(address: &str) -> bool {
    let body = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"));
    match body {
        Some(body) if !body.is_empty() && body.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            address == address.to_ascii_lowercase()
        }
        _ => true,
    }
}

/// Assert [`is_canonical`] of every `ChainAddress`-typed member anywhere in `value`.
fn assert_canonical_addresses(value: &Value, case: &str) {
    match value {
        Value::Object(members) => {
            for (name, child) in members {
                if CHAIN_ADDRESS_MEMBERS.contains(&name.as_str()) {
                    if let Some(address) = child.as_str() {
                        assert!(
                            is_canonical(address),
                            "case {case}: {name} must be reported in this API's own \
                             spelling, got {address:?}"
                        );
                    }
                }
                assert_canonical_addresses(child, case);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_canonical_addresses(item, case);
            }
        }
        _ => {}
    }
}

/// The spellings one EVM address can be written in, as `(label, spelling)`.
///
/// `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$` admits every one of them, and they name the
/// same 20 bytes: the prefix's own case and the body's case are presentation.
fn respellings(address: &str) -> Vec<(&'static str, String)> {
    let body = address.strip_prefix("0x").unwrap_or(address);
    let alternating: String = body
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character.to_ascii_lowercase()
            }
        })
        .collect();
    vec![
        ("as stored", address.to_string()),
        ("uppercase body", format!("0x{}", body.to_ascii_uppercase())),
        ("alternating case", format!("0x{alternating}")),
        ("uppercase prefix", format!("0X{body}")),
    ]
}

/// The `fieldList` entry names of a refusal, as a set — the half of the error a client
/// branches on. Each entry is checked to carry the `message` the model requires of it.
fn refused_fields(body: &Value, case: &str) -> BTreeSet<String> {
    let entries = body["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a fieldList, got {body:?}"))
        .clone();
    let mut names = BTreeSet::new();
    for entry in &entries {
        let name = entry["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a fieldList entry names a member: {entry:?}"))
            .to_string();
        assert!(
            entry["message"].as_str().is_some_and(|text| !text.is_empty()),
            "case {case}: a fieldList entry carries a message, got {entry:?}"
        );
        assert!(
            names.insert(name),
            "case {case}: one entry per violated field, got {entries:?}"
        );
    }
    names
}

/// Assert that `body` is this service's refusal reporting exactly `expected` as the
/// fields it refused.
fn assert_refused(status: u16, body: &Value, expected: &[&str], case: &str) {
    assert_eq!(
        status, 400,
        "case {case}: ValidationException carries httpError 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(GENERIC_VALIDATION_MESSAGE),
        "case {case}: the top-level message is generic, got {body:?}"
    );
    assert_eq!(
        body["reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case}: expected reason {FIELD_VALIDATION_FAILED}, got {body:?}"
    );
    let wanted: BTreeSet<String> = expected.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        refused_fields(body, case),
        wanted,
        "case {case}: expected one entry per violated field {wanted:?}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: one address, one spelling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_chain_address_in_a_response_comes_back_in_this_apis_own_spelling() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // The EVM networks: only a hex address has a letter case to lose. A base58
        // Bitcoin address is case-sensitive, so there is no respelling of one.
        if is_bitcoin(&network.network) {
            continue;
        }
        let name = &network.network;
        let (holding, _) = network
            .holdings
            .first()
            .unwrap_or_else(|| panic!("case {name}: the seeded ledger holds no balance"));

        // Rows: every spelling of the owner of a stored holding, crossed with the
        // operations that echo an address back.
        for (label, owner) in respellings(&holding.owner_address) {
            let case = format!("{name}/{owner} ({label})");
            let token = {
                let mut token = json!({"network": name});
                if let Some(contract) = &holding.contract_address {
                    token["contractAddress"] = json!(contract);
                }
                if let Some(token_id) = &holding.token_id {
                    token["tokenId"] = json!(token_id);
                }
                token
            };
            let item = json!({"tokenIdentifier": token, "ownerIdentifier": {"address": owner}});

            let single = emulator.ok("/get-token-balance", &item).await;
            assert_canonical_addresses(&single, &case);
            assert_eq!(
                single["ownerIdentifier"]["address"].as_str(),
                Some(holding.owner_address.as_str()),
                "case {case}: the owner is reported as the ledger spells it, got {single:?}"
            );

            // An item the ledger holds nothing for, so the SAME owner appears in an
            // error entry too. `QueryTokenId` is `^[a-zA-Z0-9]{1,66}$`, so the
            // unresolvable token id is spelled within it.
            let unresolvable = json!({
                "tokenIdentifier": {"network": name, "tokenId": "nosuchtoken"},
                "ownerIdentifier": {"address": owner},
            });
            let batch = emulator
                .ok(
                    "/batch-get-token-balance",
                    &json!({"getTokenBalanceInputs": [item.clone(), unresolvable]}),
                )
                .await;
            assert_canonical_addresses(&batch, &case);
            for member in ["tokenBalances", "errors"] {
                let entries = batch[member]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: {member} is required: {batch:?}"));
                assert!(
                    !entries.is_empty(),
                    "case {case}: expected a {member} entry, got {batch:?}"
                );
                for entry in entries {
                    assert_eq!(
                        entry["ownerIdentifier"]["address"].as_str(),
                        Some(holding.owner_address.as_str()),
                        "case {case}: a {member} entry names the owner as the ledger \
                         spells it, got {entry:?}"
                    );
                }
            }

            // And so do the listings the owner appears in.
            for (route, items_key, body) in [
                (
                    "/list-token-balances",
                    "tokenBalances",
                    json!({
                        "tokenFilter": {"network": name},
                        "ownerFilter": {"address": owner},
                    }),
                ),
                (
                    "/list-transactions",
                    "transactions",
                    json!({"network": name, "address": owner}),
                ),
            ] {
                let listed = drain(&emulator, route, items_key, &body, None).await;
                assert_canonical_addresses(&json!(listed), &format!("{case} {route}"));
            }
        }

        // Rows: every spelling of every contract the ledger holds, through the operation
        // that echoes a contract identifier back.
        for contract in &network.contracts {
            for (label, spelled) in respellings(contract) {
                let case = format!("{name}/{spelled} ({label})");
                let reply = emulator
                    .ok(
                        "/get-asset-contract",
                        &json!({"contractIdentifier": {
                            "network": name,
                            "contractAddress": spelled,
                        }}),
                    )
                    .await;
                assert_canonical_addresses(&reply, &case);
                assert_eq!(
                    reply["contractIdentifier"]["contractAddress"].as_str(),
                    Some(derived::normalize_address(contract).as_str()),
                    "case {case}: the contract is identified as the ledger holds it, \
                     got {reply:?}"
                );
            }
        }

        // And the transaction reads, whose addresses are never echoed from a request.
        for transaction in network.transactions.iter().take(4) {
            let hash = &transaction.transaction_hash;
            let case = format!("{name}/{hash}");
            let body = json!({"network": name, "transactionHash": hash});
            assert_canonical_addresses(&emulator.ok("/get-transaction", &body).await, &case);
            let events = drain(&emulator, "/list-transaction-events", "events", &body, None).await;
            assert_canonical_addresses(&json!(events), &case);
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: one fieldList entry per violated field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refusal_reports_every_field_it_refused_not_only_the_first() {
    let emulator = Emulator::new();
    // The fields this operation can refuse, one request member each, so a row can
    // violate any subset of them: `network` (Bitcoin-only), the filter's address
    // (a Bitcoin address) and `timeFilter` (a window that ends after it starts).
    let bitcoin = "BITCOIN_MAINNET";
    let good_network = bitcoin;
    let bad_network = "ETHEREUM_MAINNET";
    let good_address = unseeded_bitcoin_address();
    // An EVM address: well formed under `ChainAddress`, and not an address of the chain
    // this operation serves.
    let bad_address = "0x28c6c06298d514db089934071355e5743bf21d60";
    let good_window = json!({"from": {"time": 1}, "to": {"time": 2_000_000_000}});
    let bad_window = json!({"from": {"time": 2_000_000_000}, "to": {"time": 1}});

    // Rows: the whole lattice of violated-field subsets, from none to all three.
    let rows: [(&str, &str, bool, bool, Vec<&str>); 8] = [
        ("none", good_network, false, false, vec![]),
        ("network", bad_network, false, false, vec!["network"]),
        (
            "address",
            good_network,
            true,
            false,
            vec!["addressIdentifierFilter"],
        ),
        ("window", good_network, false, true, vec!["timeFilter.from"]),
        (
            "network+address",
            bad_network,
            true,
            false,
            vec!["network", "addressIdentifierFilter"],
        ),
        (
            "network+window",
            bad_network,
            false,
            true,
            vec!["network", "timeFilter.from"],
        ),
        (
            "address+window",
            good_network,
            true,
            true,
            vec!["addressIdentifierFilter", "timeFilter.from"],
        ),
        (
            "network+address+window",
            bad_network,
            true,
            true,
            vec!["network", "addressIdentifierFilter", "timeFilter.from"],
        ),
    ];

    for (label, network, wrong_address, wrong_window, expected) in rows {
        let address = if wrong_address {
            bad_address.to_string()
        } else {
            good_address.clone()
        };
        let body = json!({
            "network": network,
            "addressIdentifierFilter": {"transactionEventToAddress": [address]},
            "timeFilter": if wrong_window { bad_window.clone() } else { good_window.clone() },
        });
        let (status, answer) = emulator
            .post("/list-filtered-transaction-events", &body)
            .await;
        if expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {label}: a request that breaks no rule is answered, got {status} \
                 with {answer:?}"
            );
            continue;
        }
        assert_refused(status, &answer, &expected, label);
    }

    // The rule is the service's, not this operation's: `GetTransaction` models
    // `transactionHash` and `transactionId` as a pair of optional members, and a request
    // naming BOTH on a network that admits no `transactionId` ("transactionId is only
    // supported on the Bitcoin networks") breaks two rules at once.
    let hash = "0".repeat(64);
    let (status, answer) = emulator
        .post(
            "/get-transaction",
            &json!({
                "network": "ETHEREUM_MAINNET",
                "transactionHash": hash,
                "transactionId": hash,
            }),
        )
        .await;
    assert_refused(
        status,
        &answer,
        &["transactionHash", "transactionId"],
        "get-transaction/both-members-on-an-evm-network",
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the address filter admits the encodings Bitcoin defines
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_bitcoin_address_filter_admits_the_encodings_bitcoin_defines() {
    let emulator = Emulator::new();

    for network in ["BITCOIN_MAINNET", "BITCOIN_TESTNET"] {
        // Rows: one string of every form the chain's two encodings define, and one of
        // each way a string can fail to be one of them. The base58 rows are published
        // BIP-13 forms — the leading character encodes the version byte: `1` mainnet
        // P2PKH, `3` mainnet P2SH, `m`/`n` testnet P2PKH, `2` testnet P2SH — and the
        // bech32 rows are the published BIP-173 example address on each network's
        // human-readable part. Neither network is excluded from either: which network an
        // address belongs to is carried by the address itself, and the other network's
        // index simply holds nothing under it.
        let admitted = [
            ("base58 mainnet p2pkh", format!("1{}", "A".repeat(33))),
            ("base58 mainnet p2sh", format!("3{}", "B".repeat(33))),
            ("base58 testnet p2pkh (m)", format!("m{}", "C".repeat(33))),
            ("base58 testnet p2pkh (n)", format!("n{}", "D".repeat(33))),
            ("base58 testnet p2sh", format!("2{}", "E".repeat(33))),
            ("base58 at the shortest published length", format!("1{}", "F".repeat(25))),
            (
                "bech32 mainnet p2wpkh (BIP-173)",
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            ),
            (
                "bech32 testnet p2wpkh (BIP-173)",
                "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string(),
            ),
            (
                "bech32 uppercase, which the encoding reads as the same address",
                "BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4".to_string(),
            ),
        ];
        let refused = [
            // An EVM address: `0` and `x` are outside both alphabets.
            (
                "an EVM address",
                "0x28c6c06298d514db089934071355e5743bf21d60".to_string(),
            ),
            // One character outside the base58 alphabet, which drops `0`, `O`, `I`, `l`.
            ("a base58 body holding 0", format!("1{}0{}", "A".repeat(16), "A".repeat(16))),
            ("a base58 body holding O", format!("1{}O{}", "A".repeat(16), "A".repeat(16))),
            ("a base58 body holding l", format!("1{}l{}", "A".repeat(16), "A".repeat(16))),
            // A leading character no version byte of this chain encodes to.
            ("an unknown base58 version", format!("4{}", "A".repeat(33))),
            ("an unknown base58 version", format!("z{}", "A".repeat(33))),
            // One character past each published base58 length bound.
            ("one shorter than the shortest", format!("1{}", "A".repeat(24))),
            ("one longer than the longest", format!("1{}", "A".repeat(35))),
            // A bech32 human-readable part of another chain.
            (
                "an unknown human-readable part",
                "ex1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            ),
            // `b` is outside bech32's data alphabet, which drops `1`, `b`, `i`, `o`.
            (
                "a bech32 body holding b",
                "bc1bw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            ),
            // bech32 is case-insensitive but a MIXED-case string is invalid.
            (
                "a mixed-case bech32 address",
                "bc1QW508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            ),
            // And a label that is neither encoding.
            ("a label", "not-a-chain-address-at-all".to_string()),
        ];

        for (label, address) in admitted {
            let case = format!("{network}/{label}/{address}");
            let reply = emulator
                .ok(
                    "/list-filtered-transaction-events",
                    &json!({
                        "network": network,
                        "addressIdentifierFilter": {"transactionEventToAddress": [&address]},
                    }),
                )
                .await;
            assert!(
                reply["events"].is_array(),
                "case {case}: events is required, got {reply:?}"
            );
        }

        for (label, address) in refused {
            let case = format!("{network}/{label}/{address}");
            let (status, answer) = emulator
                .post(
                    "/list-filtered-transaction-events",
                    &json!({
                        "network": network,
                        "addressIdentifierFilter": {"transactionEventToAddress": [&address]},
                    }),
                )
                .await;
            assert_refused(status, &answer, &["addressIdentifierFilter"], &case);
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a batch error item reports the status and the item that failed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_error_item_reports_the_status_and_the_item_that_failed() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let name = &network.network;
        let (holding, _) = network
            .holdings
            .first()
            .unwrap_or_else(|| panic!("case {name}: the seeded ledger holds no balance"));
        let mut token = json!({"network": name});
        if let Some(contract) = &holding.contract_address {
            token["contractAddress"] = json!(contract);
        }
        if let Some(token_id) = &holding.token_id {
            token["tokenId"] = json!(token_id);
        }
        let owner = holding.owner_address.clone();

        // Rows: each way one item of a batch can fail, with and without an instant of
        // its own — a holding the index does not hold (the modelled
        // `ResourceNotFoundException`, httpError 404) and a `tokenIdentifier` that names
        // no token (the modelled `ValidationException`, httpError 400).
        let mut rows: Vec<(String, Value)> = Vec::new();
        for at in [None, Some(1_700_000_000i64)] {
            for (label, failing) in [
                (
                    "an owner the index does not hold",
                    json!({
                        "tokenIdentifier": token.clone(),
                        "ownerIdentifier": {"address": "0x0000000000000000000000000000000000000000"},
                    }),
                ),
                (
                    "a tokenIdentifier naming no token",
                    json!({
                        "tokenIdentifier": {"network": name},
                        "ownerIdentifier": {"address": owner},
                    }),
                ),
            ] {
                let mut item = failing;
                if let Some(at) = at {
                    item["atBlockchainInstant"] = json!({"time": at});
                }
                rows.push((format!("{name}/{label}/at={at:?}"), item));
            }
        }

        for (case, failing) in rows {
            // What the single-item read answers for the same item is the status the
            // batch item has to report.
            let (status, single) = emulator.post("/get-token-balance", &failing).await;
            assert!(
                (400..500).contains(&status),
                "case {case}: the single read must refuse this item, got {status} with \
                 {single:?}"
            );

            // The failing item is batched WITH a resolvable one, so the batch answers
            // both: the failure costs the caller nothing else.
            let good = json!({"tokenIdentifier": token.clone(), "ownerIdentifier": {"address": owner}});
            let reply = emulator
                .ok(
                    "/batch-get-token-balance",
                    &json!({"getTokenBalanceInputs": [good, failing.clone()]}),
                )
                .await;
            let balances = reply["tokenBalances"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: tokenBalances is required: {reply:?}"));
            assert_eq!(
                balances.len(),
                1,
                "case {case}: the resolvable item is still answered, got {reply:?}"
            );
            let errors = reply["errors"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: errors is required: {reply:?}"));
            assert_eq!(
                errors.len(),
                1,
                "case {case}: the failing item is reported once, got {reply:?}"
            );
            let entry = &errors[0];

            // `errorCode` is the status class of the failure — the `@httpError` of the
            // exception the single read raised — and `errorType` carries the exception's
            // identity, so the two members say different things.
            assert_eq!(
                entry["errorCode"].as_str(),
                Some(status.to_string().as_str()),
                "case {case}: errorCode is the failure's HTTP status {status}, got {entry:?}"
            );
            assert_eq!(
                entry["errorType"].as_str(),
                Some(match status {
                    404 => "RESOURCE_NOT_FOUND_EXCEPTION",
                    _ => "VALIDATION_EXCEPTION",
                }),
                "case {case}: errorType is the modelled ErrorType of the exception, \
                 got {entry:?}"
            );
            // `errorMessage` is what the single read SAID about this item. A
            // `BatchGetTokenBalanceErrorItem` models ONE message and no `fieldList`, so
            // for a refusal whose top-level `message` is this service's generic sentinel
            // — the shape a rule the model cannot express is reported in, with the
            // detail in `fieldList` — the item states that detail; otherwise it states
            // the message itself.
            assert_eq!(
                entry["errorMessage"].as_str().map(str::to_string),
                Some(refusal_explanation(&single)),
                "case {case}: errorMessage is what the single read said, got {entry:?}"
            );

            // And the entry identifies WHICH item failed: its token, its owner and the
            // instant it was read as of.
            assert_eq!(
                entry["tokenIdentifier"], failing["tokenIdentifier"],
                "case {case}: the entry names the token it failed on, got {entry:?}"
            );
            assert_eq!(
                entry["ownerIdentifier"], failing["ownerIdentifier"],
                "case {case}: the entry names the owner it failed on, got {entry:?}"
            );
            let reported = entry["atBlockchainInstant"]["time"].as_f64().map(|t| t as i64);
            match failing["atBlockchainInstant"]["time"].as_f64() {
                Some(asked) => assert_eq!(
                    reported,
                    Some(asked as i64),
                    "case {case}: the entry echoes the instant the item named, got {entry:?}"
                ),
                // An item that named no instant was read as of the current time, the
                // default `TokenBalance$atBlockchainInstant` documents, so the entry
                // reports that instant rather than leaving the item unidentified.
                None => assert!(
                    reported.is_some(),
                    "case {case}: the entry reports the instant it was read as of, \
                     got {entry:?}"
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a fact is read from the chain data the service holds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_balance_fact_is_reported_by_every_operation_that_reports_balances() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let name = &network.network;
        // Rows: every holding the ledger holds, read three ways.
        for (key, _) in &network.holdings {
            let mut token = json!({"network": name});
            if let Some(contract) = &key.contract_address {
                token["contractAddress"] = json!(contract);
            }
            if let Some(token_id) = &key.token_id {
                token["tokenId"] = json!(token_id);
            }
            let item = json!({
                "tokenIdentifier": token.clone(),
                "ownerIdentifier": {"address": key.owner_address},
            });
            let case = format!("{name}/{}", key.resource_id());

            let single = emulator.ok("/get-token-balance", &item).await;
            let balance = single["balance"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: balance is required: {single:?}"))
                .to_string();
            assert!(
                balance.parse::<i128>().is_ok(),
                "case {case}: a balance is a base-10 integer, got {balance:?}"
            );

            let batched = emulator
                .ok(
                    "/batch-get-token-balance",
                    &json!({"getTokenBalanceInputs": [item.clone()]}),
                )
                .await;
            assert_eq!(
                batched["tokenBalances"][0]["balance"].as_str(),
                Some(balance.as_str()),
                "case {case}: the batch read reports the single read's balance, \
                 got {batched:?}"
            );

            let listed = drain(
                &emulator,
                "/list-token-balances",
                "tokenBalances",
                &json!({
                    "tokenFilter": token.clone(),
                    "ownerFilter": {"address": key.owner_address},
                }),
                None,
            )
            .await;
            let row = listed
                .iter()
                .find(|row| row["tokenIdentifier"] == token)
                .unwrap_or_else(|| panic!("case {case}: the listing holds the holding: {listed:?}"));
            assert_eq!(
                row["balance"].as_str(),
                Some(balance.as_str()),
                "case {case}: the listing reports the single read's balance, got {row:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_published_account_of_a_mainnet_is_answered_out_of_stored_records() {
    let emulator = Emulator::new();
    let mut covered = 0usize;

    for network in emulator.fixture() {
        let name = &network.network;
        // The rows are the service's OWN catalog of the chain's published accounts: the
        // addresses a client of a chain index reads it with.
        for address in derived::catalogued_accounts(name) {
            covered += 1;
            let case = format!("{name}/{address}");

            // The discovery call of this API answers out of the index rather than with
            // an empty page.
            let listed = drain(
                &emulator,
                "/list-transactions",
                "transactions",
                &json!({"network": name, "address": address}),
                None,
            )
            .await;
            assert!(
                !listed.is_empty(),
                "case {case}: a published account has a history"
            );

            if !is_bitcoin(name) {
                continue;
            }
            // On the Bitcoin networks the only view of an address's received outputs is
            // the filtered event listing, so it answers for a published account too —
            // with the events the index holds for that recipient, newest first when the
            // request asks for it.
            let body = json!({
                "network": name,
                "addressIdentifierFilter": {"transactionEventToAddress": [address]},
                "sort": {"sortBy": "blockchainInstant", "sortOrder": "DESCENDING"},
            });
            let events = drain(
                &emulator,
                "/list-filtered-transaction-events",
                "events",
                &body,
                Some(2),
            )
            .await;
            assert!(
                !events.is_empty(),
                "case {case}: a published Bitcoin account has received events"
            );
            let mut previous: Option<i64> = None;
            for event in &events {
                assert_eq!(
                    event["to"].as_str(),
                    Some(*address),
                    "case {case}: every event was sent to the filtered address, got {event:?}"
                );
                assert_eq!(
                    event["network"].as_str(),
                    Some(name.as_str()),
                    "case {case}: every event is on the queried network, got {event:?}"
                );
                // `QueryTransactionEventType` names the Bitcoin output event
                // `BITCOIN_VOUT`; an output is what an address RECEIVES on that chain.
                assert_eq!(
                    event["eventType"].as_str(),
                    Some("BITCOIN_VOUT"),
                    "case {case}: a received Bitcoin event is an output, got {event:?}"
                );
                assert!(
                    event["voutIndex"].is_number() && event["voutSpent"].is_boolean(),
                    "case {case}: an output reports its index and spend state, got {event:?}"
                );
                let instant = event["blockchainInstant"]["time"]
                    .as_f64()
                    .unwrap_or_else(|| panic!("case {case}: no blockchainInstant: {event:?}"))
                    as i64;
                if let Some(previous) = previous {
                    assert!(
                        instant <= previous,
                        "case {case}: DESCENDING orders newest first, got {instant} after \
                         {previous}"
                    );
                }
                previous = Some(instant);
            }
        }
    }

    assert!(
        covered > 0,
        "the service must index the published accounts of at least one network"
    );
}

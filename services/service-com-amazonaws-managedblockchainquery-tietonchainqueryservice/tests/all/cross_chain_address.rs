// SPDX-License-Identifier: CC0-1.0

//! Every `ChainAddress`-typed input member is read as an address OF THE CHAIN the
//! request's network names, driven end to end over HTTP through the same
//! `MultiServiceRouter` the production front door serves.
//!
//! The rule under test: a string that is not an address of the chain the request named is
//! refused with the modelled `ValidationException` (HTTP 400, `reason` =
//! `fieldValidationFailed`) carrying a `fieldList` entry that NAMES the member and says
//! which chain's address it had to be — never answered with an empty page or with
//! `ResourceNotFoundException`, which say that the index holds nothing under an address it
//! understood. A well-formed address of that chain keeps exactly those answers.
//!
//! `ChainAddress` is one Smithy type for every chain this service indexes — its
//! `@pattern("^[-A-Za-z0-9]{13,74}$")` (`smithy_query(query="shape",shape="ChainAddress")`)
//! admits a base58 Bitcoin address, a bech32 one, an EVM address and a bare label alike —
//! so the model cannot state this rule and every row below satisfies the pattern: what
//! refuses the row is the service, not the constraint walk, and each test asserts that by
//! checking the pattern itself holds of the value it sends.
//!
//! The tables span the rule rather than one request: every network of the model's
//! `QueryNetwork` enum, every operation that takes an address and every address-typed
//! member it declares, crossed with address forms that differ in chain, encoding,
//! alphabet, length and case.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::QUERY_NETWORK_VALUES;

use crate::chain_query_slice::{error_code, unseeded_addresses, Emulator};

/// What a `fieldList` entry says about a member that had to carry a Bitcoin address.
///
/// The recorded wording AWS answers an address of another chain with on the operation
/// whose chain is fixed by the operation itself (`ListFilteredTransactionEvents`, whose
/// entry is `{name: "addressIdentifierFilter", message: "Please provide a valid bitcoin
/// address."}`): one sentence naming the chain the member had to carry an address of, with
/// no request value in it.
const INVALID_BITCOIN_ADDRESS: &str = "Please provide a valid bitcoin address.";

/// The same sentence for the chain the model's other two networks run; see
/// [`INVALID_BITCOIN_ADDRESS`].
const INVALID_ETHEREUM_ADDRESS: &str = "Please provide a valid ethereum address.";

/// The top-level `message` of every refusal this service makes on a rule the model cannot
/// express, with the per-member detail in `fieldList`.
const GENERIC_VALIDATION_MESSAGE: &str = "One or more parameter values were invalid.";

/// `ValidationExceptionReason`'s value for a member whose value was refused (the model's
/// own value list: `unknownOperation | cannotParse | fieldValidationFailed | other`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// Whether `network` is one of the two Bitcoin networks the model's `QueryNetwork` enum
/// declares (`BITCOIN_MAINNET | BITCOIN_TESTNET | ETHEREUM_MAINNET |
/// ETHEREUM_SEPOLIA_TESTNET`) — restated from the model rather than read out of the
/// service under test.
fn is_bitcoin(network: &str) -> bool {
    network.starts_with("BITCOIN_")
}

/// Whether `value` satisfies `ChainAddress`'s `@pattern("^[-A-Za-z0-9]{13,74}$")`, so a
/// row can prove it is not the modelled constraint that refused it.
fn is_chain_address(value: &str) -> bool {
    (13..=74).contains(&value.chars().count())
        && value
            .chars()
            .all(|character| character == '-' || character.is_ascii_alphanumeric())
}

/// What a refusal of an address member on `network` has to say about it.
fn expected_message(network: &str) -> &'static str {
    if is_bitcoin(network) {
        INVALID_BITCOIN_ADDRESS
    } else {
        INVALID_ETHEREUM_ADDRESS
    }
}

/// Strings that are NOT addresses of `network`'s chain, as `(label, value)`.
///
/// The rows differ along what the rule generalises over — the chain the string belongs
/// to, the encoding, the alphabet, the length and the letter case — and every one of them
/// satisfies `ChainAddress`'s pattern:
///
/// * an address of the OTHER family's chain, in each of the encodings that chain spells
///   one in (base58 BIP-13 and bech32 BIP-173 for Bitcoin; `0x` and 40 hex digits for an
///   EVM chain);
/// * a string in the right encoding but outside the chain's published lengths;
/// * a string over an alphabet the chain's encoding excludes (base58 drops `0`, `O`, `I`
///   and `l`; bech32 drops `1`, `b`, `i` and `o`; hex is 16 digits wide);
/// * a label of no chain at all, at the shortest and the longest the pattern admits.
fn refused_forms(network: &str) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = if is_bitcoin(network) {
        vec![
            // An EVM address: `0` and `x` are outside the base58 alphabet, and `x` is
            // outside bech32's.
            ("an EVM address", format!("0x{}", "d".repeat(40))),
            ("an EVM address without its prefix", "d".repeat(40)),
            (
                "an EVM address in upper case",
                format!("0x{}", "D".repeat(40)),
            ),
            // base58 in shape but not in length: the encoding admits 26..=35.
            ("one shorter than the shortest base58", format!("1{}", "A".repeat(24))),
            ("one longer than the longest base58", format!("1{}", "A".repeat(35))),
            // base58 characters the alphabet excludes, and a version byte no Bitcoin
            // address form uses.
            ("a base58 body holding 0", format!("1{}0{}", "A".repeat(16), "A".repeat(16))),
            ("an unknown base58 version", format!("4{}", "A".repeat(33))),
            // bech32 with a human-readable part no Bitcoin network uses.
            (
                "bech32 for another chain",
                format!("ex1q{}", "z".repeat(38)),
            ),
        ]
    } else {
        vec![
            // A base58 Bitcoin address, in each version form the chain publishes.
            ("a base58 Bitcoin address", format!("1{}", "z".repeat(32))),
            ("a base58 Bitcoin P2SH address", format!("3{}", "y".repeat(33))),
            (
                "a bech32 Bitcoin address",
                format!("bc1q{}", "z".repeat(38)),
            ),
            // 20 bytes is 40 hex digits: one digit short, one too many, and a digit hex
            // does not have.
            ("one hex digit short", format!("0x{}", "a".repeat(39))),
            ("one hex digit too many", format!("0x{}", "a".repeat(41))),
            ("a non-hex digit in the body", format!("0x{}g", "a".repeat(39))),
        ]
    };
    // Of no chain at all, at both ends of the modelled pattern's length window.
    rows.push(("a label at the pattern's shortest", "a".repeat(13)));
    rows.push(("a label at the pattern's longest", "Z".repeat(74)));
    rows.push(("a hyphenated label", "not-an-address-at-all".to_string()));
    rows
}

/// Assert that `body` is this service's refusal of exactly the members `fields` names, on
/// `network`.
fn assert_refused(case: &str, status: u16, body: &Value, network: &str, fields: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case}: an address of another chain is refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str().unwrap_or_default(),
        GENERIC_VALIDATION_MESSAGE,
        "case {case}: the top-level message stays generic, got {body:?}"
    );
    assert_eq!(
        body["reason"].as_str().unwrap_or_default(),
        FIELD_VALIDATION_FAILED,
        "case {case}: expected reason {FIELD_VALIDATION_FAILED}, got {body:?}"
    );
    let expected: Vec<Value> = fields
        .iter()
        .map(|field| json!({"name": field, "message": expected_message(network)}))
        .collect();
    assert_eq!(
        &body["fieldList"],
        &json!(expected),
        "case {case}: expected fieldList {expected:?}, got {body:?}"
    );
}

/// One row of the member table: the operation, and the request that carries `address` in
/// the member named by `field`.
struct Member {
    route: &'static str,
    field: &'static str,
    /// A request for `(network, address)`.
    body: fn(&str, &str) -> Value,
}

/// Every operation of this service that takes a `ChainAddress` paired with a
/// `QueryNetwork`, one row per address-typed member
/// (`smithy_query(query="operation",operation="ListTransactions,GetTokenBalance,
/// ListTokenBalances,GetAssetContract,ListAssetContracts")`).
///
/// `ListFilteredTransactionEvents` is not here: its chain is fixed by the operation
/// ("This operation is only supported on the Bitcoin networks") rather than by the
/// network the request names, and `response_fidelity.rs` owns that rule.
fn members() -> Vec<Member> {
    vec![
        Member {
            route: "/list-transactions",
            field: "address",
            body: |network, address| json!({"network": network, "address": address}),
        },
        Member {
            route: "/get-token-balance",
            field: "ownerIdentifier.address",
            body: |network, address| {
                json!({
                    "tokenIdentifier": {"network": network, "tokenId": "eth"},
                    "ownerIdentifier": {"address": address},
                })
            },
        },
        Member {
            route: "/get-token-balance",
            field: "tokenIdentifier.contractAddress",
            body: |network, address| {
                json!({
                    "tokenIdentifier": {"network": network, "contractAddress": address},
                    "ownerIdentifier": {"address": owner_of(network)},
                })
            },
        },
        Member {
            route: "/list-token-balances",
            field: "ownerFilter.address",
            body: |network, address| {
                json!({
                    "tokenFilter": {"network": network, "tokenId": "eth"},
                    "ownerFilter": {"address": address},
                })
            },
        },
        Member {
            route: "/list-token-balances",
            field: "tokenFilter.contractAddress",
            body: |network, address| {
                json!({"tokenFilter": {"network": network, "contractAddress": address}})
            },
        },
        // The asset-contract operations serve the Ethereum networks only ("The Bitcoin
        // blockchain networks do not support this operation"), so on a Bitcoin network
        // what is wrong with the request is the network and not the address it carries;
        // `asset_contracts.rs` owns that refusal, and these rows are driven on the
        // networks the operations serve.
        Member {
            route: "/get-asset-contract",
            field: "contractIdentifier.contractAddress",
            body: |network, address| {
                json!({"contractIdentifier": {"network": network, "contractAddress": address}})
            },
        },
        Member {
            route: "/list-asset-contracts",
            field: "contractFilter.deployerAddress",
            body: |network, address| {
                json!({"contractFilter": {
                    "network": network,
                    "tokenStandard": "ERC20",
                    "deployerAddress": address,
                }})
            },
        },
    ]
}

/// Whether `route` serves `network` at all — the asset-contract operations do not serve
/// the Bitcoin networks, so a row of theirs on one of those would be testing that refusal
/// instead of this one.
fn serves(route: &str, network: &str) -> bool {
    !is_bitcoin(network) || !route.contains("asset-contract")
}

/// An owner address a row can carry while the member under test is the one that is wrong:
/// a well-formed address of `network`'s own chain.
fn owner_of(network: &str) -> String {
    unseeded_addresses(network)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {network}: no address of its chain is known"))
}

#[tokio::test]
async fn an_address_of_another_chain_is_refused_on_the_member_that_carried_it() {
    let emulator = Emulator::new();

    for network in QUERY_NETWORK_VALUES {
        for (label, address) in refused_forms(network) {
            assert!(
                is_chain_address(&address),
                "case {network}/{label}: the row must satisfy ChainAddress, so that what \
                 refuses it is this service's rule and not the modelled constraint"
            );
            for member in members() {
                if !serves(member.route, network) {
                    continue;
                }
                let case = format!("{network}/{label}/{}/{}", member.route, member.field);
                let body = (member.body)(network, &address);
                let (status, answer) = emulator.post(member.route, &body).await;
                assert_refused(&case, status, &answer, network, &[member.field]);
            }
        }
    }
}

#[tokio::test]
async fn a_well_formed_address_of_the_queried_chain_is_never_refused() {
    let emulator = Emulator::new();

    for network in QUERY_NETWORK_VALUES {
        // The same members, carrying an address of the chain the request names: the
        // answer is then about the index (an empty page, or a resource it does not hold)
        // and never this refusal.
        for address in unseeded_addresses(network) {
            for member in members() {
                if !serves(member.route, network) {
                    continue;
                }
                let case = format!("{network}/{address}/{}/{}", member.route, member.field);
                let body = (member.body)(network, &address);
                let (status, answer) = emulator.post(member.route, &body).await;
                let refused = status == 400
                    && answer["fieldList"]
                        .as_array()
                        .is_some_and(|fields| {
                            fields.iter().any(|field| {
                                field["message"].as_str() == Some(expected_message(network))
                            })
                        });
                assert!(
                    !refused,
                    "case {case}: an address of this chain must not be refused as one of \
                     another chain, got {status} with {answer:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_wrong_address_of_one_request_is_reported_in_the_same_answer() {
    let emulator = Emulator::new();

    for network in QUERY_NETWORK_VALUES {
        let (label, wrong) = refused_forms(network)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {network}: no refused form is known"));
        // `GetTokenBalance` is the one operation of this service that declares TWO
        // address members, so it is where a request can get both wrong at once;
        // `ValidationException$fieldList` is the machine-readable half a client repairs
        // a request from, so it names both rather than one per round trip.
        let case = format!("{network}/{label}/both-members");
        let body = json!({
            "tokenIdentifier": {"network": network, "contractAddress": wrong},
            "ownerIdentifier": {"address": wrong},
        });
        let (status, answer) = emulator.post("/get-token-balance", &body).await;
        assert_refused(
            &case,
            status,
            &answer,
            network,
            &["tokenIdentifier.contractAddress", "ownerIdentifier.address"],
        );

        // And a batch reports it per ITEM, inside the 200 the operation answers, without
        // confusing it with an item whose address this chain simply holds nothing under:
        // that one is still the modelled not-found.
        let unheld = json!({
            "tokenIdentifier": {"network": network, "tokenId": "eth"},
            "ownerIdentifier": {"address": owner_of(network)},
        });
        let reply = emulator
            .ok(
                "/batch-get-token-balance",
                &json!({"getTokenBalanceInputs": [body, unheld]}),
            )
            .await;
        let errors = reply["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            2,
            "case {case}: each item that cannot be answered is one entry, got {reply:?}"
        );
        assert_eq!(
            errors[0]["errorCode"].as_str(),
            Some("400"),
            "case {case}: the refused item carries the refusal's status, got {reply:?}"
        );
        assert_eq!(
            errors[0]["errorType"].as_str(),
            Some("VALIDATION_EXCEPTION"),
            "case {case}: the refused item carries the modelled ErrorType, got {reply:?}"
        );
        let said = errors[0]["errorMessage"].as_str().unwrap_or_default();
        assert!(
            said.contains(expected_message(network)),
            "case {case}: the item says which chain's address the member had to be, \
             got {said:?}"
        );
        assert_eq!(
            errors[1]["errorType"].as_str(),
            Some("RESOURCE_NOT_FOUND_EXCEPTION"),
            "case {case}: an address of this chain the ledger holds nothing under stays a \
             missing resource, got {reply:?}"
        );
    }
}

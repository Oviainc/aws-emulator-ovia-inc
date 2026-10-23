// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ManagedBlockchain Query emulator's transaction and
//! balance operations — `ListTransactions`, `GetTransaction`, `ListTransactionEvents`,
//! `ListFilteredTransactionEvents`, `GetTokenBalance` and `ListTokenBalances` — driven
//! over HTTP through the same `MultiServiceRouter` the production front door serves.
//!
//! The service is READ-ONLY, so there is no create to round-trip through: the thing
//! under test is that every operation's answer comes out of the SEEDED ledger and that
//! the answers agree with each other. Each test is therefore a TABLE whose rows
//! are generated from the ledger the service actually serves (every network, every
//! transaction, every event, every holding) rather than from one record picked by hand,
//! and the ledger is read out of the engine the router was built on — never from a
//! constant copied into the test.
//!
//! Where a bound or a spelling comes from the Smithy model, the comment beside it names
//! the shape it belongs to.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::{
    ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState as Ledgers, HoldingKey,
    StoredHolding, StoredTransaction,
};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service: that scope
/// is how the router picks the service which claims a request. The signature itself is
/// never verified here — [`Emulator`] turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/managedblockchain-query/aws4_request, SignedHeaders=host, Signature=not-verified";

/// The paginated operations of this slice: route and the response member carrying
/// the items (`@paginated(items=...)` on each operation).
const PAGINATED: [(&str, &str); 4] = [
    ("/list-transactions", "transactions"),
    ("/list-transaction-events", "events"),
    ("/list-filtered-transaction-events", "events"),
    ("/list-token-balances", "tokenBalances"),
];

/// The route `ListFilteredTransactionEvents` is bound to (`@http(uri:)`).
const FILTERED_EVENTS: &str = "/list-filtered-transaction-events";

/// Whether `network` is one of the model's two Bitcoin networks —
/// `ListFilteredTransactionEvents` "is only supported on the Bitcoin networks".
///
/// Restated here from the model's `QueryNetwork` value names rather than borrowed from
/// `crate::state::is_bitcoin`, deliberately: the expectation a test checks must not come
/// from the implementation it is checking.
fn is_bitcoin(network: &str) -> bool {
    network.starts_with("BITCOIN_")
}

/// A `ListFilteredTransactionEvents` request for the events sent to one address.
fn filtered_events_body(network: &str, address: &str) -> Value {
    json!({
        "network": network,
        "addressIdentifierFilter": {"transactionEventToAddress": [address]},
    })
}

/// A well-formed Bitcoin address that the fixture ledger holds nothing under.
///
/// Built here rather than written down: `z` is in the base58 alphabet and `1` is the
/// leading character of a mainnet pay-to-public-key-hash address, so this has the shape
/// the chain's base58 encoding fixes while being a body no fixture digest produces. A
/// listing keyed by it is therefore empty for the only reason that matters: the index
/// holds nothing under it. It takes no network, because what makes a string an address of
/// this chain is the encoding, which both of the chain's networks share.
///
/// `pub(crate)` because the sibling suites need the same row.
pub(crate) fn unseeded_bitcoin_address() -> String {
    format!("1{}", "z".repeat(32))
}

/// The operations this slice deliberately leaves scaffolded, with a request that
/// satisfies every modelled constraint so the answer can only be the scaffold's own.
///
/// `BatchGetTokenBalance` is no longer among them: it is implemented and covered by
/// `tests/all/batch_token_balance.rs`. Neither is `GetAssetContract`, covered by
/// `tests/all/asset_contracts.rs`.
fn scaffolded_operations(network: &str, address: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "/list-asset-contracts",
            json!({"contractFilter": {
                "network": network,
                // `QueryTokenStandard`: ERC1155 | ERC20 | ERC721.
                "tokenStandard": "ERC20",
                "deployerAddress": address,
            }}),
        ),
    ]
}

/// A router with only this service mounted — one emulator "session" — plus the engine
/// it was built on, so a test can read the ledger it serves.
///
/// `pub(crate)` because the sibling suites in this target drive the emulator through the
/// same seam rather than each standing one up their own way.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST `path` with a JSON body; returns the status and the parsed body.
    pub(crate) async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://localhost{path}"))
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// POST and require HTTP 200.
    pub(crate) async fn ok(&self, path: &str, body: &Value) -> Value {
        let (status, parsed) = self.post(path, body).await;
        assert_eq!(
            status, 200,
            "case {path} {body}: expected 200, got {status} with {parsed:?}"
        );
        parsed
    }

    /// POST and require `status` with the wire error code `code`.
    pub(crate) async fn fails(&self, path: &str, body: &Value, status: u16, code: &str) -> Value {
        let (got, parsed) = self.post(path, body).await;
        assert_eq!(
            got, status,
            "case {path} {body}: expected {status}, got {got} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            code,
            "case {path} {body}: expected {code}, got {parsed:?}"
        );
        parsed
    }

    /// The seeded ledger of every network, read out of the engine the router serves
    /// from — the same store the handlers read.
    pub(crate) fn fixture(&self) -> Vec<NetworkFixture> {
        let state = self.engine.state::<Ledgers>();
        let state = state.lock();
        state
            .networks
            .iter()
            .map(|(network, ledger)| NetworkFixture {
                network: network.clone(),
                transactions: ledger.transactions.values().cloned().collect(),
                holdings: ledger
                    .holdings
                    .iter()
                    .map(|(key, holding)| (key.clone(), holding.clone()))
                    .collect(),
                contracts: ledger
                    .contracts
                    .values()
                    .map(|contract| contract.contract_address.clone())
                    .collect(),
            })
            .collect()
    }
}

/// One network's seeded ledger, as a test reads it.
pub(crate) struct NetworkFixture {
    pub(crate) network: String,
    /// In ledger (chain) order.
    pub(crate) transactions: Vec<StoredTransaction>,
    /// In the ledger's own key order.
    pub(crate) holdings: Vec<(HoldingKey, StoredHolding)>,
    /// The address of every asset contract this ledger holds.
    pub(crate) contracts: Vec<String>,
}

impl NetworkFixture {
    /// Every address that appears anywhere in this ledger.
    pub(crate) fn addresses(&self) -> BTreeSet<String> {
        self.transactions
            .iter()
            .flat_map(|transaction| {
                transaction
                    .addresses()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            })
            .collect()
    }

    /// The hashes of the stored transactions `address` took part in, in chain order.
    pub(crate) fn hashes_for(&self, address: &str) -> Vec<String> {
        self.transactions
            .iter()
            .filter(|transaction| transaction.addresses().contains(&address))
            .map(|transaction| transaction.transaction_hash.clone())
            .collect()
    }

    /// The address with the most transactions — the busiest row, so a filter test has
    /// something to narrow.
    pub(crate) fn busiest_address(&self) -> String {
        let mut addresses: Vec<String> = self.addresses().into_iter().collect();
        addresses.sort_by_key(|address| (usize::MAX - self.hashes_for(address).len(), address.clone()));
        addresses
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {}: the seeded ledger holds no address", self.network))
    }

    /// The instant the ledger's balances open at: the earliest balance record it holds.
    pub(crate) fn opened_at(&self) -> i64 {
        self.holdings
            .iter()
            .filter_map(|(_, holding)| holding.history.first().map(|record| record.time))
            .min()
            .unwrap_or_else(|| panic!("case {}: the seeded ledger holds no balance", self.network))
    }
}

/// The wire error code of a failure response: the last segment of its `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `transactionHash` of every item in a list response.
pub(crate) fn hashes(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of items, got {items:?}"))
        .iter()
        .map(|item| {
            item["transactionHash"]
                .as_str()
                .unwrap_or_else(|| panic!("item carries no transactionHash: {item:?}"))
                .to_string()
        })
        .collect()
}

/// A `TokenIdentifier`/`TokenFilter` naming the token half of `key`. The two shapes
/// declare the same three members (`network` required, `contractAddress`, `tokenId`).
pub(crate) fn token_json(network: &str, key: &HoldingKey) -> Value {
    let mut value = json!({"network": network});
    if let Some(contract) = &key.contract_address {
        value["contractAddress"] = json!(contract);
    }
    if let Some(token_id) = &key.token_id {
        value["tokenId"] = json!(token_id);
    }
    value
}

/// Which `(contractAddress, tokenId)` an event moves value in, from its `eventType`.
///
/// Restated here from the token standards themselves rather than borrowed from the
/// implementation: a native transfer moves the network's native token (no contract, the
/// 3-character abbreviation `TokenIdentifier$tokenId` documents), an ERC-20 event moves
/// one fungible balance per contract (a contract and no `tokenId`), and an
/// ERC-721/ERC-1155 event moves one token of a contract (both).
pub(crate) fn event_holding(event: &Value) -> Option<(Option<String>, Option<String>)> {
    let kind = event["eventType"].as_str()?;
    let contract = event["contractAddress"].as_str().map(str::to_string);
    let token_id = event["tokenId"].as_str().map(str::to_string);
    match kind {
        "ETH_TRANSFER" | "INTERNAL_ETH_TRANSFER" => Some((None, Some("eth".to_string()))),
        "BITCOIN_VIN" | "BITCOIN_VOUT" => Some((None, Some("btc".to_string()))),
        other if other.starts_with("ERC20_") => contract.map(|contract| (Some(contract), None)),
        "ERC721_TRANSFER" | "ERC1155_TRANSFER" => match (contract, token_id) {
            (Some(contract), Some(token_id)) => Some((Some(contract), Some(token_id))),
            _ => None,
        },
        _ => None,
    }
}

/// `GetTokenBalance` for one holding, as of `at` (or now when absent), as an integer —
/// or `None` when the ledger knows no balance for it at that instant (a 404).
pub(crate) async fn balance_or_none(
    emulator: &Emulator,
    network: &str,
    key: &HoldingKey,
    at: Option<i64>,
) -> Option<i128> {
    let mut body = json!({
        "tokenIdentifier": token_json(network, key),
        "ownerIdentifier": {"address": key.owner_address},
    });
    if let Some(at) = at {
        body["atBlockchainInstant"] = json!({"time": at});
    }
    let (status, reply) = emulator.post("/get-token-balance", &body).await;
    match status {
        200 => Some(
            reply["balance"]
                .as_str()
                .unwrap_or_else(|| panic!("case {body}: balance is required, got {reply:?}"))
                .parse()
                .unwrap_or_else(|error| panic!("case {body}: balance is not an integer: {error}")),
        ),
        404 => None,
        other => panic!("case {body}: expected 200 or 404, got {other} with {reply:?}"),
    }
}

/// `GetTokenBalance` for one holding, requiring it to resolve.
async fn balance_of(
    emulator: &Emulator,
    network: &str,
    key: &HoldingKey,
    at: Option<i64>,
) -> i128 {
    balance_or_none(emulator, network, key, at)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {network}/{}@{at:?}: the holding must have a balance",
                key.resource_id()
            )
        })
}

/// An item's identity: the item with the members the service reads off the CLOCK per
/// call removed.
///
/// `TokenBalance$atBlockchainInstant` is "the time for when the TokenBalance is
/// requested or the current time if a time is not provided in the request", so it is a
/// per-call clock reading: two walks of the same listing legitimately differ in it, and
/// a comparison of the two listings has to look past it. Everything else, including the
/// stored `lastUpdatedTime`, is kept.
pub(crate) fn item_identity(item: &Value) -> Value {
    let mut item = item.clone();
    if let Some(object) = item.as_object_mut() {
        object.remove("atBlockchainInstant");
    }
    item
}

/// Walk a listing to its end, following `nextToken`, and return every item in the order
/// the service returned them.
///
/// Checks the pagination contract as it goes: no page exceeds the `maxResults` asked
/// for, no item comes back twice, a `nextToken` never accompanies an empty page, and
/// the walk terminates.
pub(crate) async fn drain(
    emulator: &Emulator,
    path: &str,
    items_key: &str,
    body: &Value,
    max_results: Option<i32>,
) -> Vec<Value> {
    /// More pages than the fixture could ever need: a token that fails to advance would
    /// otherwise loop forever.
    const PAGE_LIMIT: usize = 500;

    let mut items: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut request = body.clone();
        if let Some(max) = max_results {
            request["maxResults"] = json!(max);
        }
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok(path, &request).await;
        let page_items = page[items_key]
            .as_array()
            .unwrap_or_else(|| panic!("case {path} {request}: no {items_key} list in {page:?}"))
            .clone();

        pages += 1;
        assert!(
            pages <= PAGE_LIMIT,
            "case {path} {request}: pagination did not terminate within {PAGE_LIMIT} pages"
        );
        if let Some(max) = max_results {
            assert!(
                page_items.len() <= max as usize,
                "case {path} {request}: page holds {} items, more than maxResults={max}",
                page_items.len()
            );
        }
        for item in &page_items {
            assert!(
                seen.insert(item.to_string()),
                "case {path} {request}: {item} came back on more than one page"
            );
        }
        let empty = page_items.is_empty();
        items.extend(page_items);

        match page["nextToken"].as_str() {
            Some(next) => {
                assert!(
                    !empty,
                    "case {path} {request}: a nextToken came back with an empty page"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }

    items
}

// ---------------------------------------------------------------------------
// ListTransactions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_transactions_returns_exactly_the_stored_transactions_of_each_address() {
    let emulator = Emulator::new();
    let fixture = emulator.fixture();
    assert!(
        !fixture.is_empty(),
        "every QueryNetwork value must have a seeded ledger, got none"
    );

    for network in &fixture {
        for address in network.addresses() {
            let case = format!("{}/{address}", network.network);
            let expected = network.hashes_for(&address);
            let reply = emulator
                .ok(
                    "/list-transactions",
                    &json!({"network": network.network, "address": address}),
                )
                .await;
            let got = hashes(&reply["transactions"]);
            assert_eq!(
                got, expected,
                "case {case}: expected the stored transactions {expected:?}, got {got:?}"
            );

            // Each item is the stored record's summary, not a re-statement of the
            // request: `TransactionOutputItem` requires transactionHash, network and
            // transactionTimestamp.
            for item in reply["transactions"].as_array().unwrap_or(&Vec::new()) {
                let hash = item["transactionHash"].as_str().unwrap_or_default();
                let stored = network
                    .transactions
                    .iter()
                    .find(|transaction| transaction.transaction_hash == hash)
                    .unwrap_or_else(|| panic!("case {case}: {hash} is not in the ledger"));
                assert_eq!(
                    item["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {case}: expected network {}, got {item:?}",
                    network.network
                );
                assert_eq!(
                    item["transactionTimestamp"].as_f64().map(|secs| secs as i64),
                    Some(stored.transaction_timestamp),
                    "case {case}: expected the stored timestamp {}, got {item:?}",
                    stored.transaction_timestamp
                );
                assert_eq!(
                    item["confirmationStatus"].as_str(),
                    Some(stored.confirmation_status.as_str()),
                    "case {case}: expected the stored confirmationStatus {}, got {item:?}",
                    stored.confirmation_status
                );
            }
        }
    }
}

#[tokio::test]
async fn list_transactions_returns_nothing_for_an_address_the_ledger_does_not_know() {
    let emulator = Emulator::new();
    // Each row satisfies `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$` and differs in form:
    // an Ethereum-shaped address, a base58-shaped one, a hyphenated label, and the
    // longest and shortest the pattern admits. The emulator holds no index of the
    // public chains, so an identifier its fixture ledger does not hold answers with an
    // empty page rather than with a history made up for it.
    let strangers = [
        "0x0000000000000000000000000000000000000000".to_string(),
        "1NotASeededBitcoinAddressAtAll".to_string(),
        "not-a-seeded-address".to_string(),
        "a".repeat(13),
        "Z".repeat(74),
    ];

    for network in emulator.fixture() {
        for stranger in &strangers {
            let reply = emulator
                .ok(
                    "/list-transactions",
                    &json!({"network": network.network, "address": stranger}),
                )
                .await;
            let got = hashes(&reply["transactions"]);
            assert!(
                got.is_empty(),
                "case {}/{stranger}: an unknown address has no transactions, got {got:?}",
                network.network
            );
            assert!(
                reply["nextToken"].is_null(),
                "case {}/{stranger}: an empty page carries no nextToken, got {:?}",
                network.network,
                reply["nextToken"]
            );
        }
    }
}

#[tokio::test]
async fn list_transactions_honours_sort_order_the_confirmation_filter_and_the_window() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let address = network.busiest_address();
        let ascending = network.hashes_for(&address);
        let base = json!({"network": network.network, "address": address});
        let case = format!("{}/{address}", network.network);

        // `ListTransactionsSort$sortOrder`: absent defaults to ASCENDING.
        for order in ["ASCENDING", "DESCENDING"] {
            let mut body = base.clone();
            body["sort"] = json!({"sortBy": "TRANSACTION_TIMESTAMP", "sortOrder": order});
            let got = hashes(&emulator.ok("/list-transactions", &body).await["transactions"]);
            let mut expected = ascending.clone();
            if order == "DESCENDING" {
                expected.reverse();
            }
            assert_eq!(
                got, expected,
                "case {case} sortOrder={order}: expected {expected:?}, got {got:?}"
            );
        }

        // `ConfirmationStatus`: FINAL | NONFINAL, and both together.
        for include in [vec!["FINAL"], vec!["NONFINAL"], vec!["FINAL", "NONFINAL"]] {
            let mut body = base.clone();
            body["confirmationStatusFilter"] = json!({"include": include});
            // `ConfirmationStatusFilter$include`: "Transactions that have reached
            // finality are always part of the response", so the filter only decides
            // whether the ones that have not are included too.
            let expected: Vec<String> = network
                .transactions
                .iter()
                .filter(|transaction| transaction.addresses().contains(&address.as_str()))
                .filter(|transaction| {
                    transaction.confirmation_status == "FINAL"
                        || include.contains(&transaction.confirmation_status.as_str())
                })
                .map(|transaction| transaction.transaction_hash.clone())
                .collect();
            let got = hashes(&emulator.ok("/list-transactions", &body).await["transactions"]);
            assert_eq!(
                got, expected,
                "case {case} include={include:?}: expected {expected:?}, got {got:?}"
            );
        }

        // The instant window: every pair of stored timestamps, plus each bound alone.
        let instants: Vec<i64> = network
            .transactions
            .iter()
            .map(|transaction| transaction.transaction_timestamp)
            .collect::<BTreeSet<i64>>()
            .into_iter()
            .collect();
        for from in instants.iter().copied().map(Some).chain([None]) {
            for to in instants.iter().copied().map(Some).chain([None]) {
                if let (Some(from), Some(to)) = (from, to) {
                    if from > to {
                        continue;
                    }
                }
                let mut body = base.clone();
                if let Some(from) = from {
                    body["fromBlockchainInstant"] = json!({"time": from});
                }
                if let Some(to) = to {
                    body["toBlockchainInstant"] = json!({"time": to});
                }
                let expected: Vec<String> = network
                    .transactions
                    .iter()
                    .filter(|transaction| transaction.addresses().contains(&address.as_str()))
                    .filter(|transaction| {
                        from.is_none_or(|earliest| transaction.transaction_timestamp >= earliest)
                    })
                    .filter(|transaction| {
                        to.is_none_or(|latest| transaction.transaction_timestamp <= latest)
                    })
                    .map(|transaction| transaction.transaction_hash.clone())
                    .collect();
                let got = hashes(&emulator.ok("/list-transactions", &body).await["transactions"]);
                assert_eq!(
                    got, expected,
                    "case {case} window={from:?}..{to:?}: expected {expected:?}, got {got:?}"
                );
            }
        }

        // A window that ends before it starts selects nothing a caller could have
        // meant, so it is refused rather than answered with an empty page.
        if let (Some(first), Some(last)) = (instants.first(), instants.last()) {
            if first != last {
                let mut body = base.clone();
                body["fromBlockchainInstant"] = json!({"time": last});
                body["toBlockchainInstant"] = json!({"time": first});
                emulator
                    .fails("/list-transactions", &body, 400, "ValidationException")
                    .await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GetTransaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_transaction_returns_the_stored_record_for_every_spelling_of_its_hash() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for stored in &network.transactions {
            let bare = stored
                .transaction_hash
                .strip_prefix("0x")
                .unwrap_or(&stored.transaction_hash)
                .to_string();
            // `QueryTransactionHash`'s pattern `^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$`
            // admits the same 64 digits with or without the prefix and in either letter
            // case, so every row below names the SAME transaction.
            let spellings = [
                bare.clone(),
                format!("0x{bare}"),
                bare.to_uppercase(),
                format!("0x{}", bare.to_uppercase()),
            ];
            for spelling in &spellings {
                let case = format!("{}/{spelling}", network.network);
                let reply = emulator
                    .ok(
                        "/get-transaction",
                        &json!({"network": network.network, "transactionHash": spelling}),
                    )
                    .await;
                let got = &reply["transaction"];
                // Every `@required` member of `Transaction`, then the optional ones the
                // fixture stores.
                assert_eq!(
                    got["transactionHash"].as_str(),
                    Some(stored.transaction_hash.as_str()),
                    "case {case}: expected the stored hash, got {got:?}"
                );
                assert_eq!(
                    got["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {case}: expected network {}, got {got:?}",
                    network.network
                );
                assert_eq!(
                    got["transactionTimestamp"].as_f64().map(|secs| secs as i64),
                    Some(stored.transaction_timestamp),
                    "case {case}: expected timestamp {}, got {got:?}",
                    stored.transaction_timestamp
                );
                assert_eq!(
                    got["transactionIndex"].as_i64(),
                    Some(stored.transaction_index),
                    "case {case}: expected index {}, got {got:?}",
                    stored.transaction_index
                );
                assert_eq!(
                    got["numberOfTransactions"].as_i64(),
                    Some(stored.number_of_transactions),
                    "case {case}: expected {} transactions in the block, got {got:?}",
                    stored.number_of_transactions
                );
                assert_eq!(
                    got["to"].as_str(),
                    Some(stored.to.as_str()),
                    "case {case}: expected to {}, got {got:?}",
                    stored.to
                );
                assert_eq!(
                    got["from"].as_str(),
                    stored.from.as_deref(),
                    "case {case}: expected from {:?}, got {got:?}",
                    stored.from
                );
                assert_eq!(
                    got["blockHash"].as_str(),
                    stored.block_hash.as_deref(),
                    "case {case}: expected blockHash {:?}, got {got:?}",
                    stored.block_hash
                );
                assert_eq!(
                    got["blockNumber"].as_str(),
                    stored.block_number.as_deref(),
                    "case {case}: expected blockNumber {:?}, got {got:?}",
                    stored.block_number
                );
                assert_eq!(
                    got["gasUsed"].as_str().map(str::to_string),
                    stored.gas_used.map(|gas| gas.to_string()),
                    "case {case}: expected gasUsed {:?}, got {got:?}",
                    stored.gas_used
                );
                assert_eq!(
                    got["confirmationStatus"].as_str(),
                    Some(stored.confirmation_status.as_str()),
                    "case {case}: expected confirmationStatus {}, got {got:?}",
                    stored.confirmation_status
                );
                assert_eq!(
                    got["executionStatus"].as_str(),
                    stored.execution_status.as_deref(),
                    "case {case}: expected executionStatus {:?}, got {got:?}",
                    stored.execution_status
                );
            }
        }
    }
}

#[tokio::test]
async fn get_transaction_is_a_404_for_a_hash_the_ledger_does_not_hold() {
    let emulator = Emulator::new();
    // Rows differ in form: the all-zero and all-f hashes, prefixed and bare, upper and
    // lower case. Every one satisfies the modelled pattern and none is a SHA-256 digest
    // the fixture could have produced.
    let unknown = [
        "0".repeat(64),
        format!("0x{}", "0".repeat(64)),
        "f".repeat(64),
        format!("0x{}", "F".repeat(64)),
        format!("0x{}{}", "dead".repeat(15), "beef"),
    ];

    for network in emulator.fixture() {
        for hash in &unknown {
            emulator
                .fails(
                    "/get-transaction",
                    &json!({"network": network.network, "transactionHash": hash}),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
        }
    }
}

#[tokio::test]
async fn a_transaction_scoped_read_needs_exactly_one_identifier() {
    let emulator = Emulator::new();
    // `transactionId` is a Bitcoin identifier: `GetTransactionInput$transactionId` says
    // "transactionId is only supported on the Bitcoin networks".
    let some_id = "a".repeat(64);

    for network in emulator.fixture() {
        let bitcoin = network.network.starts_with("BITCOIN_");
        let stored = network
            .transactions
            .first()
            .unwrap_or_else(|| panic!("case {}: no seeded transaction", network.network));

        for path in ["/get-transaction", "/list-transaction-events"] {
            // Neither identifier: nothing names a transaction.
            emulator
                .fails(
                    path,
                    &json!({"network": network.network}),
                    400,
                    "ValidationException",
                )
                .await;
            // Both identifiers: ambiguous.
            emulator
                .fails(
                    path,
                    &json!({
                        "network": network.network,
                        "transactionHash": stored.transaction_hash,
                        "transactionId": some_id,
                    }),
                    400,
                    "ValidationException",
                )
                .await;

            let by_id = json!({"network": network.network, "transactionId": some_id});
            if bitcoin {
                // Accepted on Bitcoin — and this particular id is not in the ledger, so
                // `GetTransaction` answers 404 while the listing answers an empty page.
                let (status, body) = emulator.post(path, &by_id).await;
                let expected = if path == "/get-transaction" { 404 } else { 200 };
                assert_eq!(
                    status, expected,
                    "case {}/{path}: transactionId is supported on Bitcoin, got {status} with {body:?}",
                    network.network
                );
            } else {
                emulator
                    .fails(path, &by_id, 400, "ValidationException")
                    .await;
            }
        }

        // A Bitcoin transaction's stored `transactionId` resolves the same record its
        // hash does.
        if bitcoin {
            let id = stored
                .transaction_id
                .as_deref()
                .unwrap_or_else(|| panic!("case {}: no stored transactionId", network.network));
            let reply = emulator
                .ok(
                    "/get-transaction",
                    &json!({"network": network.network, "transactionId": id}),
                )
                .await;
            assert_eq!(
                reply["transaction"]["transactionHash"].as_str(),
                Some(stored.transaction_hash.as_str()),
                "case {}/{id}: expected the transaction the id names, got {reply:?}",
                network.network
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListTransactionEvents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_transaction_events_returns_the_stored_events_in_emission_order() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for stored in &network.transactions {
            let case = format!("{}/{}", network.network, stored.transaction_hash);
            let reply = emulator
                .ok(
                    "/list-transaction-events",
                    &json!({
                        "network": network.network,
                        "transactionHash": stored.transaction_hash,
                    }),
                )
                .await;
            let got = reply["events"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: events is required, got {reply:?}"));
            assert_eq!(
                got.len(),
                stored.events.len(),
                "case {case}: expected {} events, got {got:?}",
                stored.events.len()
            );
            for (position, (event, expected)) in got.iter().zip(&stored.events).enumerate() {
                // `TransactionEvent` requires network, transactionHash and eventType.
                assert_eq!(
                    event["eventType"].as_str(),
                    Some(expected.event_type.as_str()),
                    "case {case} event {position}: expected {}, got {event:?}",
                    expected.event_type
                );
                assert_eq!(
                    event["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {case} event {position}: expected network {}, got {event:?}",
                    network.network
                );
                assert_eq!(
                    event["transactionHash"].as_str(),
                    Some(stored.transaction_hash.as_str()),
                    "case {case} event {position}: expected the transaction's hash, got {event:?}"
                );
                assert_eq!(
                    event["value"].as_str().map(str::to_string),
                    expected.value.map(|value| value.to_string()),
                    "case {case} event {position}: expected value {:?}, got {event:?}",
                    expected.value
                );
                assert_eq!(
                    event["contractAddress"].as_str(),
                    expected.contract_address.as_deref(),
                    "case {case} event {position}: expected contractAddress {:?}, got {event:?}",
                    expected.contract_address
                );
                assert_eq!(
                    event["tokenId"].as_str(),
                    expected.token_id.as_deref(),
                    "case {case} event {position}: expected tokenId {:?}, got {event:?}",
                    expected.token_id
                );
                assert_eq!(
                    event["from"].as_str(),
                    expected.from.as_deref(),
                    "case {case} event {position}: expected from {:?}, got {event:?}",
                    expected.from
                );
                assert_eq!(
                    event["to"].as_str(),
                    expected.to.as_deref(),
                    "case {case} event {position}: expected to {:?}, got {event:?}",
                    expected.to
                );
            }
        }
    }
}

#[tokio::test]
async fn list_transaction_events_is_empty_for_a_transaction_the_ledger_does_not_hold() {
    let emulator = Emulator::new();
    // This operation declares no `ResourceNotFoundException` (see `OPERATIONS.md`), so
    // the only answer the model leaves for an unknown transaction is an empty list.
    let unknown = ["0".repeat(64), format!("0x{}", "9".repeat(64))];

    for network in emulator.fixture() {
        for hash in &unknown {
            let reply = emulator
                .ok(
                    "/list-transaction-events",
                    &json!({"network": network.network, "transactionHash": hash}),
                )
                .await;
            let got = reply["events"].as_array().cloned().unwrap_or_default();
            assert!(
                got.is_empty(),
                "case {}/{hash}: expected no events, got {got:?}",
                network.network
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Balances
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_balance_moved_exactly_as_much_as_the_events_that_touched_it() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let opened_at = network.opened_at();

        // What the events the service reports say each (token, owner) moved. Built from
        // the HTTP responses alone, so it is an independent account of the same ledger.
        let mut net: BTreeMap<(Option<String>, Option<String>, String), i128> = BTreeMap::new();
        for stored in &network.transactions {
            let reply = emulator
                .ok(
                    "/list-transaction-events",
                    &json!({
                        "network": network.network,
                        "transactionHash": stored.transaction_hash,
                    }),
                )
                .await;
            for event in reply["events"].as_array().cloned().unwrap_or_default() {
                let Some((contract, token_id)) = event_holding(&event) else {
                    continue;
                };
                let Some(value) = event["value"]
                    .as_str()
                    .and_then(|value| value.parse::<i128>().ok())
                else {
                    continue;
                };
                if let Some(from) = event["from"].as_str() {
                    *net.entry((contract.clone(), token_id.clone(), from.to_string()))
                        .or_default() -= value;
                }
                if let Some(to) = event["to"].as_str() {
                    *net.entry((contract.clone(), token_id.clone(), to.to_string()))
                        .or_default() += value;
                }
            }
        }

        for (key, _) in &network.holdings {
            let case = format!("{}/{}", network.network, key.resource_id());
            // The balance the ledger opened with — zero where it recorded none for this
            // holding yet, which is what an owner who had not received the token looks
            // like.
            let opening = balance_or_none(&emulator, &network.network, key, Some(opened_at))
                .await
                .unwrap_or_default();
            let current = balance_of(&emulator, &network.network, key, None).await;
            let moved = net
                .get(&(
                    key.contract_address.clone(),
                    key.token_id.clone(),
                    key.owner_address.clone(),
                ))
                .copied()
                .unwrap_or_default();
            assert_eq!(
                current,
                opening + moved,
                "case {case}: the events move {moved} from an opening balance of \
                 {opening}, so the balance must be {}, got {current}",
                opening + moved
            );
        }
    }
}

#[tokio::test]
async fn get_token_balance_and_list_token_balances_agree_on_every_holding() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for (key, holding) in &network.holdings {
            let case = format!("{}/{}", network.network, key.resource_id());
            let stored = holding
                .history
                .last()
                .unwrap_or_else(|| panic!("case {case}: the holding has no balance"))
                .balance;

            let got = balance_of(&emulator, &network.network, key, None).await;
            assert_eq!(
                got, stored,
                "case {case}: expected the stored balance {stored}, got {got}"
            );

            let listed = emulator
                .ok(
                    "/list-token-balances",
                    &json!({
                        "tokenFilter": token_json(&network.network, key),
                        "ownerFilter": {"address": key.owner_address},
                    }),
                )
                .await;
            let balances = listed["tokenBalances"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: tokenBalances is required: {listed:?}"));
            assert_eq!(
                balances.len(),
                1,
                "case {case}: one holding matches that filter, got {balances:?}"
            );
            assert_eq!(
                balances[0]["balance"].as_str(),
                Some(stored.to_string().as_str()),
                "case {case}: ListTokenBalances must agree with GetTokenBalance ({stored})"
            );
            assert!(
                balances[0]["atBlockchainInstant"]["time"].is_number(),
                "case {case}: atBlockchainInstant is required, got {:?}",
                balances[0]
            );
            assert_eq!(
                balances[0]["ownerIdentifier"]["address"].as_str(),
                Some(key.owner_address.as_str()),
                "case {case}: expected the holding's owner, got {:?}",
                balances[0]
            );
        }
    }
}

#[tokio::test]
async fn get_token_balance_reports_the_instant_it_was_asked_for_and_the_last_update() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for (key, holding) in &network.holdings {
            // Every instant the holding changed at, plus one second after each: on both
            // the balance in force is the record at or before that instant.
            let instants: Vec<i64> = holding
                .history
                .iter()
                .flat_map(|record| [record.time, record.time + 1])
                .collect();
            for at in instants {
                let case = format!("{}/{}@{at}", network.network, key.resource_id());
                let expected = holding
                    .history
                    .iter()
                    .rev()
                    .find(|record| record.time <= at)
                    .unwrap_or_else(|| panic!("case {case}: no record at or before {at}"));
                let reply = emulator
                    .ok(
                        "/get-token-balance",
                        &json!({
                            "tokenIdentifier": token_json(&network.network, key),
                            "ownerIdentifier": {"address": key.owner_address},
                            "atBlockchainInstant": {"time": at},
                        }),
                    )
                    .await;
                assert_eq!(
                    reply["balance"].as_str(),
                    Some(expected.balance.to_string().as_str()),
                    "case {case}: expected the balance in force ({}), got {reply:?}",
                    expected.balance
                );
                // "The time for when the TokenBalance is requested" — so the instant the
                // request named, echoed to the second.
                assert_eq!(
                    reply["atBlockchainInstant"]["time"]
                        .as_f64()
                        .map(|secs| secs as i64),
                    Some(at),
                    "case {case}: expected atBlockchainInstant {at}, got {reply:?}"
                );
                // "The Timestamp of the last transaction at which the balance ... was
                // updated" — so the instant of the record the balance came from.
                assert_eq!(
                    reply["lastUpdatedTime"]["time"]
                        .as_f64()
                        .map(|secs| secs as i64),
                    Some(expected.time),
                    "case {case}: expected lastUpdatedTime {}, got {reply:?}",
                    expected.time
                );
            }

            // Before the ledger opened, no balance is known for the holding.
            let earliest = holding.history.first().map(|record| record.time).unwrap_or(0);
            emulator
                .fails(
                    "/get-token-balance",
                    &json!({
                        "tokenIdentifier": token_json(&network.network, key),
                        "ownerIdentifier": {"address": key.owner_address},
                        "atBlockchainInstant": {"time": earliest - 1},
                    }),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
        }
    }
}

#[tokio::test]
async fn get_token_balance_refuses_a_token_it_cannot_identify_and_404s_an_unknown_owner() {
    let emulator = Emulator::new();
    // Addresses that satisfy `ChainAddress`'s pattern and hold nothing in the fixture.
    let strangers = [
        "0x0000000000000000000000000000000000000000".to_string(),
        "1NotASeededBitcoinAddressAtAll".to_string(),
        "not-a-seeded-owner".to_string(),
    ];

    for network in emulator.fixture() {
        // A `TokenIdentifier` naming only a network identifies no token.
        emulator
            .fails(
                "/get-token-balance",
                &json!({
                    "tokenIdentifier": {"network": network.network},
                    "ownerIdentifier": {"address": strangers[0]},
                }),
                400,
                "ValidationException",
            )
            .await;

        // A `tokenFilter` that narrows the network by nothing asks for a listing
        // `ListTokenBalances` does not offer.
        emulator
            .fails(
                "/list-token-balances",
                &json!({"tokenFilter": {"network": network.network}}),
                400,
                "ValidationException",
            )
            .await;

        let tokens: BTreeMap<String, Value> = network
            .holdings
            .iter()
            .map(|(key, _)| {
                let token = token_json(&network.network, key);
                (token.to_string(), token)
            })
            .collect();
        for token in tokens.values() {
            for stranger in &strangers {
                emulator
                    .fails(
                        "/get-token-balance",
                        &json!({
                            "tokenIdentifier": token,
                            "ownerIdentifier": {"address": stranger},
                        }),
                        404,
                        "ResourceNotFoundException",
                    )
                    .await;
            }
        }
    }
}

#[tokio::test]
async fn a_token_filter_selects_every_holding_it_names_and_no_other() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // Rows: no narrowing beyond the owner, by contract, by contract and token, and
        // by token alone — the forms `TokenFilter` admits.
        let contracts: BTreeSet<Option<String>> = network
            .holdings
            .iter()
            .map(|(key, _)| key.contract_address.clone())
            .collect();
        let token_ids: BTreeSet<Option<String>> = network
            .holdings
            .iter()
            .map(|(key, _)| key.token_id.clone())
            .collect();

        for contract in &contracts {
            for token_id in &token_ids {
                let mut filter = json!({"network": network.network});
                if let Some(contract) = contract {
                    filter["contractAddress"] = json!(contract);
                }
                if let Some(token_id) = token_id {
                    filter["tokenId"] = json!(token_id);
                }
                if contract.is_none() && token_id.is_none() {
                    // Refused on its own; covered by the owner-scoped rows below.
                    continue;
                }
                let expected: BTreeSet<String> = network
                    .holdings
                    .iter()
                    .filter(|(key, _)| {
                        key.matches_token(contract.as_deref(), token_id.as_deref())
                    })
                    .map(|(key, _)| key.resource_id())
                    .collect();
                let items = drain(
                    &emulator,
                    "/list-token-balances",
                    "tokenBalances",
                    &json!({"tokenFilter": filter}),
                    None,
                )
                .await;
                let got: BTreeSet<String> = items
                    .iter()
                    .map(|item| {
                        format!(
                            "{}/{}/{}",
                            item["tokenIdentifier"]["contractAddress"]
                                .as_str()
                                .unwrap_or("-"),
                            item["tokenIdentifier"]["tokenId"].as_str().unwrap_or("-"),
                            item["ownerIdentifier"]["address"].as_str().unwrap_or("-")
                        )
                    })
                    .collect();
                assert_eq!(
                    got, expected,
                    "case {} filter={filter}: expected {expected:?}, got {got:?}",
                    network.network
                );
            }
        }

        // The owner-scoped form: every holding of one address, and nothing else.
        let owners: BTreeSet<String> = network
            .holdings
            .iter()
            .map(|(key, _)| key.owner_address.clone())
            .collect();
        for owner in &owners {
            let expected: BTreeSet<String> = network
                .holdings
                .iter()
                .filter(|(key, _)| key.owner_address == *owner)
                .map(|(key, _)| key.resource_id())
                .collect();
            let items = drain(
                &emulator,
                "/list-token-balances",
                "tokenBalances",
                &json!({
                    "tokenFilter": {"network": network.network},
                    "ownerFilter": {"address": owner},
                }),
                None,
            )
            .await;
            let got: BTreeSet<String> = items
                .iter()
                .map(|item| {
                    format!(
                        "{}/{}/{}",
                        item["tokenIdentifier"]["contractAddress"]
                            .as_str()
                            .unwrap_or("-"),
                        item["tokenIdentifier"]["tokenId"].as_str().unwrap_or("-"),
                        item["ownerIdentifier"]["address"].as_str().unwrap_or("-")
                    )
                })
                .collect();
            assert_eq!(
                got, expected,
                "case {}/{owner}: expected {expected:?}, got {got:?}",
                network.network
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_page_size_walks_a_listing_exactly_once() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let address = network.busiest_address();
        let busiest_transaction = network
            .transactions
            .iter()
            .max_by_key(|transaction| transaction.events.len())
            .unwrap_or_else(|| panic!("case {}: no seeded transaction", network.network));
        let bodies: [(&str, &str, Value); 3] = [
            (
                "/list-transactions",
                "transactions",
                json!({"network": network.network, "address": address}),
            ),
            (
                "/list-transaction-events",
                "events",
                json!({
                    "network": network.network,
                    "transactionHash": busiest_transaction.transaction_hash,
                }),
            ),
            (
                "/list-token-balances",
                "tokenBalances",
                json!({"tokenFilter": {"network": network.network, "tokenId": "eth"},
                       "ownerFilter": {"address": address}}),
            ),
        ];

        for (path, items_key, body) in &bodies {
            // `maxResults` carries `@range(min: 1, max: 250)`: the rows below are the
            // published minimum, a few sizes in between, the published maximum, and no
            // `maxResults` at all.
            let whole: Vec<Value> = drain(&emulator, path, items_key, body, None)
                .await
                .iter()
                .map(item_identity)
                .collect();
            for max in [Some(1), Some(2), Some(3), Some(249), Some(250), None] {
                let got: Vec<Value> = drain(&emulator, path, items_key, body, max)
                    .await
                    .iter()
                    .map(item_identity)
                    .collect();
                assert_eq!(
                    got, whole,
                    "case {path} {body} maxResults={max:?}: paging must not change the \
                     listing, expected {whole:?}, got {got:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    let network = emulator
        .fixture()
        .into_iter()
        .next()
        .expect("a seeded network");
    let address = network.busiest_address();
    let hash = network.transactions[0].transaction_hash.clone();

    // Rows: not a token at all, valid hex that decodes to something this service never
    // wrote, and a token for a position that is not in the listing being paged.
    let tokens = [
        "not-a-token".to_string(),
        String::new(),
        hex::encode("managedblockchain-query:v0:0"),
        hex::encode("some-other-service:v1:0"),
        hex::encode(format!("managedblockchain-query:v1:{}", "f".repeat(64))),
    ];

    for (path, _items_key) in PAGINATED {
        for token in &tokens {
            let base = match path {
                "/list-transactions" => {
                    json!({"network": network.network, "address": address, "nextToken": token})
                }
                "/list-transaction-events" => {
                    json!({"network": network.network, "transactionHash": hash, "nextToken": token})
                }
                FILTERED_EVENTS => {
                    let mut body = filtered_events_body(&network.network, &address);
                    body["nextToken"] = json!(token);
                    body
                }
                _ => json!({
                    "tokenFilter": {"network": network.network, "tokenId": "eth"},
                    "nextToken": token,
                }),
            };
            emulator
                .fails(path, &base, 400, "ValidationException")
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// ListFilteredTransactionEvents
// ---------------------------------------------------------------------------

/// Every event the ledger holds whose RECIPIENT is `address`, in chain order, as
/// `ListTransactionEvents` reports it.
///
/// Built by asking the per-transaction listing for each stored transaction in chain
/// order and keeping the events sent to `address`, so the expectation comes from the
/// OTHER operation's view of the same stored events rather than from a copy of the
/// implementation.
async fn events_sent_to(emulator: &Emulator, network: &NetworkFixture, address: &str) -> Vec<Value> {
    let mut expected: Vec<Value> = Vec::new();
    for transaction in &network.transactions {
        let events = drain(
            emulator,
            "/list-transaction-events",
            "events",
            &json!({
                "network": network.network,
                "transactionHash": transaction.transaction_hash,
            }),
            None,
        )
        .await;
        expected.extend(
            events
                .into_iter()
                .filter(|event| event["to"].as_str() == Some(address)),
        );
    }
    expected
}

/// The `blockchainInstant.time` one event reports.
fn event_instant(event: &Value) -> i64 {
    event["blockchainInstant"]["time"]
        .as_i64()
        .unwrap_or_else(|| panic!("event carries no blockchainInstant.time: {event:?}"))
}

/// The `blockchainInstant.time` of each event, in the order the listing reported them.
fn instants(events: &[Value]) -> Vec<i64> {
    events.iter().map(event_instant).collect()
}

#[tokio::test]
async fn list_filtered_transaction_events_returns_exactly_the_events_sent_to_the_address() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        if !is_bitcoin(&network.network) {
            continue;
        }
        // Rows: every address the ledger knows — which spans addresses that received
        // several events, one, and none — plus an address it does not know at all. The
        // unknown row is a well-formed Bitcoin address (which this operation requires of
        // its filter) that no fixture produced.
        let mut addresses: Vec<String> = network.addresses().into_iter().collect();
        addresses.push(unseeded_bitcoin_address());

        for address in addresses {
            let ascending = events_sent_to(&emulator, &network, &address).await;
            let base = filtered_events_body(&network.network, &address);

            // `ListFilteredTransactionEventsSortBy` declares exactly one value,
            // `blockchainInstant`; `SortOrder` is ASCENDING | DESCENDING and defaults to
            // ASCENDING when the request names none.
            for order in [None, Some("ASCENDING"), Some("DESCENDING")] {
                let mut body = base.clone();
                if let Some(order) = order {
                    body["sort"] = json!({"sortBy": "blockchainInstant", "sortOrder": order});
                }
                let mut want = ascending.clone();
                if order == Some("DESCENDING") {
                    want.reverse();
                }

                // `maxResults` carries `@range(min: 1, max: 250)`: the published
                // minimum, sizes in between, the published maximum, and none at all.
                for max in [Some(1), Some(2), Some(3), Some(250), None] {
                    let got = drain(&emulator, FILTERED_EVENTS, "events", &body, max).await;
                    assert_eq!(
                        got, want,
                        "case {}/{address} sortOrder={order:?} maxResults={max:?}: expected \
                         {want:?}, got {got:?}",
                        network.network
                    );
                }

                let reported = instants(&want);
                let mut sorted = reported.clone();
                if order == Some("DESCENDING") {
                    sorted.sort_unstable_by(|left, right| right.cmp(left));
                } else {
                    sorted.sort_unstable();
                }
                assert_eq!(
                    reported, sorted,
                    "case {}/{address} sortOrder={order:?}: the listing must be ordered by \
                     blockchainInstant, got {reported:?}",
                    network.network
                );
            }

            for event in &ascending {
                assert_eq!(
                    event["to"].as_str(),
                    Some(address.as_str()),
                    "case {}/{address}: every event must name the filtered address as its \
                     recipient, got {event:?}",
                    network.network
                );
                assert_eq!(
                    event["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {}/{address}: every event must belong to the requested network, got \
                     {event:?}",
                    network.network
                );
            }
        }
    }
}

#[tokio::test]
async fn the_vout_filter_selects_the_outputs_in_the_spend_state_it_asks_for() {
    let emulator = Emulator::new();
    // `VoutFilter$voutSpent` "specifies if the transaction output is spent or unspent",
    // so the two rows can only be told apart on a ledger that holds both.
    let mut spent_seen = false;
    let mut unspent_seen = false;

    for network in emulator.fixture() {
        if !is_bitcoin(&network.network) {
            continue;
        }
        for address in network.addresses() {
            let whole = events_sent_to(&emulator, &network, &address).await;

            let mut partitioned: Vec<Value> = Vec::new();
            for wanted in [true, false] {
                let want: Vec<Value> = whole
                    .iter()
                    .filter(|event| event["voutSpent"].as_bool() == Some(wanted))
                    .cloned()
                    .collect();
                spent_seen |= wanted && !want.is_empty();
                unspent_seen |= !wanted && !want.is_empty();

                let mut body = filtered_events_body(&network.network, &address);
                body["voutFilter"] = json!({"voutSpent": wanted});
                let got = drain(&emulator, FILTERED_EVENTS, "events", &body, None).await;
                assert_eq!(
                    got, want,
                    "case {}/{address} voutSpent={wanted}: expected {want:?}, got {got:?}",
                    network.network
                );
                partitioned.extend(want);
            }

            // The two rows partition the events that report a spend state at all: an
            // event the filter cannot judge is on neither.
            let judged: Vec<Value> = whole
                .iter()
                .filter(|event| event["voutSpent"].as_bool().is_some())
                .cloned()
                .collect();
            partitioned.sort_by_key(Value::to_string);
            let mut judged_sorted = judged.clone();
            judged_sorted.sort_by_key(Value::to_string);
            assert_eq!(
                partitioned, judged_sorted,
                "case {}/{address}: the two voutSpent rows must partition the outputs, \
                 expected {judged_sorted:?}, got {partitioned:?}",
                network.network
            );
        }
    }

    assert!(
        spent_seen,
        "case voutSpent=true: the ledger must hold a spent output, or the filter is untestable"
    );
    assert!(
        unspent_seen,
        "case voutSpent=false: the ledger must hold an unspent output, or the filter is untestable"
    );
}

#[tokio::test]
async fn the_confirmation_filter_narrows_the_listing_to_the_statuses_it_names() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        if !is_bitcoin(&network.network) {
            continue;
        }
        let address = network.busiest_address();
        let whole = events_sent_to(&emulator, &network, &address).await;

        // Rows: each value of the model's `ConfirmationStatus` enum on its own, and both
        // together.
        // `ConfirmationStatusFilter$include`: "Transactions that have reached finality
        // are always part of the response", so the filter only decides whether the
        // events of the transactions that have NOT reached it come too.
        let filter_status = |event: &Value, include: &[&str]| {
            let status = event["confirmationStatus"].as_str().unwrap_or_default();
            status == "FINAL" || include.contains(&status)
        };
        for include in [vec!["FINAL"], vec!["NONFINAL"], vec!["FINAL", "NONFINAL"]] {
            let want: Vec<Value> = whole
                .iter()
                .filter(|event| filter_status(event, &include))
                .cloned()
                .collect();
            let mut body = filtered_events_body(&network.network, &address);
            body["confirmationStatusFilter"] = json!({"include": include});
            let got = drain(&emulator, FILTERED_EVENTS, "events", &body, None).await;
            assert_eq!(
                got, want,
                "case {}/{address} include={include:?}: expected {want:?}, got {got:?}",
                network.network
            );
        }
    }
}

#[tokio::test]
async fn the_time_filter_keeps_the_events_inside_the_window_it_names() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        if !is_bitcoin(&network.network) {
            continue;
        }
        let address = network.busiest_address();
        let whole = events_sent_to(&emulator, &network, &address).await;
        let times: BTreeSet<i64> = instants(&whole).into_iter().collect();
        let earliest = *times
            .iter()
            .next()
            .unwrap_or_else(|| panic!("case {}: no event to bound", network.network));
        let latest = *times
            .iter()
            .next_back()
            .unwrap_or_else(|| panic!("case {}: no event to bound", network.network));

        // Rows: no window, the whole window, each of its published bounds alone, a
        // single instant, and a window one second past each bound (which the ledger has
        // nothing in).
        let windows: [(Option<i64>, Option<i64>); 8] = [
            (None, None),
            (Some(earliest), Some(latest)),
            (Some(earliest), None),
            (None, Some(latest)),
            (Some(earliest), Some(earliest)),
            (Some(latest), Some(latest)),
            (Some(latest + 1), None),
            (None, Some(earliest - 1)),
        ];

        for (from, to) in windows {
            let want: Vec<Value> = whole
                .iter()
                .filter(|event| {
                    let time = event_instant(event);
                    from.is_none_or(|earliest| time >= earliest)
                        && to.is_none_or(|latest| time <= latest)
                })
                .cloned()
                .collect();

            let mut body = filtered_events_body(&network.network, &address);
            let mut filter = json!({});
            if let Some(from) = from {
                filter["from"] = json!({"time": from});
            }
            if let Some(to) = to {
                filter["to"] = json!({"time": to});
            }
            body["timeFilter"] = filter;

            let got = drain(&emulator, FILTERED_EVENTS, "events", &body, None).await;
            assert_eq!(
                got, want,
                "case {}/{address} timeFilter=({from:?},{to:?}): expected {want:?}, got {got:?}",
                network.network
            );
        }
    }
}

#[tokio::test]
async fn list_filtered_transaction_events_refuses_a_request_it_cannot_answer() {
    let emulator = Emulator::new();
    let networks = emulator.fixture();
    let bitcoin = networks
        .iter()
        .find(|network| is_bitcoin(&network.network))
        .expect("a seeded Bitcoin network");
    let address = bitcoin.busiest_address();
    let other_address = bitcoin
        .addresses()
        .into_iter()
        .find(|candidate| *candidate != address)
        .expect("a second seeded address");

    // Rows: a network outside the model's `QueryNetwork` enum, every network the
    // operation does not support (its documentation: "This operation is only supported
    // on the Bitcoin networks"), a filter naming no address and one naming two
    // (`transactionEventToAddress` is `@length(min: 1, max: 1)`), and a window that ends
    // before it starts.
    let mut refused: Vec<Value> = vec![
        filtered_events_body("BITCOIN_REGTEST", &address),
        filtered_events_body("bitcoin_mainnet", &address),
        filtered_events_body("", &address),
        json!({
            "network": bitcoin.network,
            "addressIdentifierFilter": {"transactionEventToAddress": []},
        }),
        json!({
            "network": bitcoin.network,
            "addressIdentifierFilter": {
                "transactionEventToAddress": [address, other_address],
            },
        }),
    ];
    for network in &networks {
        if !is_bitcoin(&network.network) {
            refused.push(filtered_events_body(&network.network, &address));
        }
    }
    let mut inverted = filtered_events_body(&bitcoin.network, &address);
    inverted["timeFilter"] = json!({"from": {"time": 2_000_000_000}, "to": {"time": 1}});
    refused.push(inverted);

    for body in &refused {
        emulator
            .fails(FILTERED_EVENTS, body, 400, "ValidationException")
            .await;
    }

    // And the request the rows above vary from is answered, so each refusal is about the
    // member it changed.
    emulator
        .ok(FILTERED_EVENTS, &filtered_events_body(&bitcoin.network, &address))
        .await;
}

// ---------------------------------------------------------------------------
// What this slice deliberately leaves alone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_501() {
    let emulator = Emulator::new();
    let network = emulator
        .fixture()
        .into_iter()
        .next()
        .expect("a seeded network");
    let address = network.busiest_address();

    for (path, body) in scaffolded_operations(&network.network, &address) {
        let (status, parsed) = emulator.post(path, &body).await;
        assert_eq!(
            status, 501,
            "case {path}: an unimplemented operation answers 501, not an empty success; \
             got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "NotImplemented",
            "case {path}: expected NotImplemented, got {parsed:?}"
        );
    }
}

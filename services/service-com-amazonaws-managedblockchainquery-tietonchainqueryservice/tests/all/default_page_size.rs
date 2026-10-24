// SPDX-License-Identifier: CC0-1.0

//! The page a paginated listing serves when the request does not size it.
//!
//! RULE: every paginated operation of this service documents its `maxResults` as
//! "Default: 100", so a request that names no `maxResults` is served at most that many
//! items and carries a continuation token exactly when further items remain; an
//! explicit `maxResults` bounds the page at the value asked for, and the modelled
//! maximum of 250 bounds only what a caller may ASK for.
//!
//! Each test is a TABLE over (operation, requested page size, listing length): every
//! paginated operation of the service, the published bounds of `maxResults` together
//! with the sizes either side of the documented default, against listings both shorter
//! and longer than that default. The listings are generated from the ledger the router
//! was actually built on, never from a record copied into the test, and the harness is
//! the one in [`crate::chain_query_slice`].

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::{
    ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState as Ledgers, HoldingKey,
    StoredContract,
};

use crate::chain_query_slice::{drain, item_identity, token_json, Emulator};

/// The page size an absent `maxResults` means.
///
/// From the documentation of the member itself — `ListTransactionsInput$maxResults`,
/// `ListTokenBalancesInput$maxResults`, `ListAssetContractsInput$maxResults`,
/// `ListTransactionEventsInput$maxResults` and
/// `ListFilteredTransactionEventsInput$maxResults` each read "Default: `100`"
/// (`smithy_query(query="doc", shape="ListTransactionsInput", member="maxResults")`).
/// Written down here rather than read from the crate under test, so the expectation is
/// the published fact and not the implementation.
const DOCUMENTED_DEFAULT: usize = 100;

/// The largest page a caller may ask for: `@range(min: 1, max: 250)` on every
/// `maxResults`, the number each output list's `@length(min: 0, max: 250)` repeats.
const MODELLED_MAX: i32 = 250;

/// One paginated listing to page: the route, the response member carrying the items and
/// a request that selects the listing.
struct Listing {
    /// Which operation, on which network — named in every assertion.
    case: String,
    path: &'static str,
    items_key: &'static str,
    body: Value,
}

/// Every contract the seeded ledger holds, read out of the engine the router serves
/// from — the same store the handlers read.
fn seeded_contracts(emulator: &Emulator) -> Vec<StoredContract> {
    let state = emulator.engine.state::<Ledgers>();
    let state = state.lock();
    state
        .networks
        .values()
        .flat_map(|ledger| ledger.contracts.values().cloned())
        .collect()
}

/// Every paginated operation of this service, each against a listing the seeded ledger
/// holds: the busiest wallet's transactions, the busiest transaction's events, a whole
/// network's token balances, the events sent to that wallet and the contracts a stored
/// contract's deployer deployed. The lengths differ by two orders of magnitude, which is
/// what makes the default page size observable on some rows and not on others.
fn listings(emulator: &Emulator) -> Vec<Listing> {
    let contracts = seeded_contracts(emulator);
    let mut rows: Vec<Listing> = Vec::new();

    for network in emulator.fixture() {
        let address = network.busiest_address();
        let busiest_transaction = network
            .transactions
            .iter()
            .max_by_key(|transaction| transaction.events.len())
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the seeded ledger must hold a transaction",
                    network.network
                )
            })
            .transaction_hash
            .clone();

        rows.push(Listing {
            case: format!("ListTransactions/{}", network.network),
            path: "/list-transactions",
            items_key: "transactions",
            body: json!({"network": network.network, "address": address}),
        });
        rows.push(Listing {
            case: format!("ListTransactionEvents/{}", network.network),
            path: "/list-transaction-events",
            items_key: "events",
            body: json!({"network": network.network, "transactionHash": busiest_transaction}),
        });
        // The token the network's ledger holds the most balances of, so the listing is
        // the longest `ListTokenBalances` can serve for it. A `tokenFilter` naming the
        // token alone leaves the owner unfiltered, which is every holder of it.
        let token_filter = {
            let mut by_token: BTreeMap<(Option<&String>, Option<&String>), Vec<&HoldingKey>> =
                BTreeMap::new();
            for (key, _) in &network.holdings {
                by_token
                    .entry((key.contract_address.as_ref(), key.token_id.as_ref()))
                    .or_default()
                    .push(key);
            }
            let key = by_token
                .values()
                .max_by_key(|keys| keys.len())
                .and_then(|keys| keys.first().copied())
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: the seeded ledger must hold a balance",
                        network.network
                    )
                });
            token_json(&network.network, key)
        };
        rows.push(Listing {
            case: format!("ListTokenBalances/{}", network.network),
            path: "/list-token-balances",
            items_key: "tokenBalances",
            body: json!({"tokenFilter": token_filter}),
        });
        // `ListFilteredTransactionEvents` "is only supported on the Bitcoin networks"
        // (`ListFilteredTransactionEventsInput$network`), whose `QueryNetwork` values
        // are the BITCOIN_ ones.
        if network.network.starts_with("BITCOIN_") {
            rows.push(Listing {
                case: format!("ListFilteredTransactionEvents/{}", network.network),
                path: "/list-filtered-transaction-events",
                items_key: "events",
                body: json!({
                    "network": network.network,
                    "addressIdentifierFilter": {"transactionEventToAddress": [address]},
                }),
            });
        }
        if let Some(contract) = contracts
            .iter()
            .find(|contract| contract.network == network.network)
        {
            rows.push(Listing {
                case: format!("ListAssetContracts/{}", network.network),
                path: "/list-asset-contracts",
                items_key: "contracts",
                // Every member of `ContractFilter` is `@required`.
                body: json!({"contractFilter": {
                    "network": network.network,
                    "tokenStandard": contract.token_standard,
                    "deployerAddress": contract.deployer_address,
                }}),
            });
        }
    }

    rows
}

/// One page of a listing: the items and the continuation token the service answered
/// with.
async fn page(
    emulator: &Emulator,
    listing: &Listing,
    max_results: Option<i32>,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = listing.body.clone();
    if let Some(max) = max_results {
        request["maxResults"] = json!(max);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    let reply = emulator.ok(listing.path, &request).await;
    let items = reply[listing.items_key]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {}: {} is required, got {reply:?}",
                listing.case, listing.items_key
            )
        })
        .clone();
    (items, reply["nextToken"].as_str().map(str::to_string))
}

/// How long the whole listing is, walked at the biggest page the model admits.
async fn listing_length(emulator: &Emulator, listing: &Listing) -> usize {
    drain(
        emulator,
        listing.path,
        listing.items_key,
        &listing.body,
        Some(MODELLED_MAX),
    )
    .await
    .len()
}

#[tokio::test]
async fn a_first_page_holds_the_requested_size_or_the_documented_default() {
    let emulator = Emulator::new();
    let listings = listings(&emulator);
    assert!(
        !listings.is_empty(),
        "the seeded ledger must hold a paginated listing to page, got none"
    );
    let mut longer_than_default = 0usize;

    for listing in &listings {
        let total = listing_length(&emulator, listing).await;
        if total > DOCUMENTED_DEFAULT {
            longer_than_default += 1;
        }

        // Rows: the published minimum of `maxResults`, a size in between, the sizes
        // either side of the documented default, the published maximum, and no
        // `maxResults` at all.
        let sizes = [
            Some(1),
            Some(2),
            Some(DOCUMENTED_DEFAULT as i32 - 1),
            Some(DOCUMENTED_DEFAULT as i32),
            Some(DOCUMENTED_DEFAULT as i32 + 1),
            Some(MODELLED_MAX),
            None,
        ];
        for requested in sizes {
            let (items, next) = page(&emulator, listing, requested, None).await;
            let expected = requested
                .map_or(DOCUMENTED_DEFAULT, |max| max as usize)
                .min(total);
            assert_eq!(
                items.len(),
                expected,
                "case {} maxResults={requested:?}: expected {expected} of the {total} \
                 items on the first page, got {}",
                listing.case,
                items.len()
            );
            assert_eq!(
                next.is_some(),
                total > expected,
                "case {} maxResults={requested:?}: a nextToken comes back exactly when \
                 items remain ({total} in all, {expected} served), got {next:?}",
                listing.case
            );
        }
    }

    // Without a listing longer than the default, the bound under test could not bite on
    // any row and the table would prove nothing.
    assert!(
        longer_than_default > 0,
        "at least one seeded listing must hold more than {DOCUMENTED_DEFAULT} items, got none"
    );
}

#[tokio::test]
async fn an_unsized_walk_pages_the_whole_listing_at_the_documented_default() {
    let emulator = Emulator::new();

    for listing in listings(&emulator) {
        let whole = drain(
            &emulator,
            listing.path,
            listing.items_key,
            &listing.body,
            Some(MODELLED_MAX),
        )
        .await;
        // An empty listing is still one page, which is why this never rounds to zero.
        let expected_pages = whole.len().div_ceil(DOCUMENTED_DEFAULT).max(1);

        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (items, next) = page(&emulator, &listing, None, token.as_deref()).await;
            pages += 1;
            assert!(
                pages <= expected_pages,
                "case {}: {} items must page at {DOCUMENTED_DEFAULT} in {expected_pages} \
                 pages, got at least {pages}",
                listing.case,
                whole.len()
            );
            assert!(
                items.len() <= DOCUMENTED_DEFAULT,
                "case {}: page {pages} holds {} items, more than the default of \
                 {DOCUMENTED_DEFAULT}",
                listing.case,
                items.len()
            );
            collected.extend(items);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        assert_eq!(
            pages,
            expected_pages,
            "case {}: {} items page at {DOCUMENTED_DEFAULT} in {expected_pages} pages, got \
             {pages}",
            listing.case,
            whole.len()
        );
        // Paging must not change WHAT is listed, only how much of it comes back at a
        // time; `item_identity` drops the one member this service reads off the clock
        // per call.
        let got: Vec<Value> = collected.iter().map(item_identity).collect();
        let expected: Vec<Value> = whole.iter().map(item_identity).collect();
        assert_eq!(
            got, expected,
            "case {}: an unsized walk lists what a 250-item walk lists, expected \
             {expected:?}, got {got:?}",
            listing.case
        );
    }
}

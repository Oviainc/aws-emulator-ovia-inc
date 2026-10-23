// SPDX-License-Identifier: CC0-1.0

//! The chain history the fixture ledger holds, and what it answers for an identifier it
//! does not hold.
//!
//! ManagedBlockchain Query answers out of an index of a public chain. This emulator
//! holds no such index — a public chain's transactions are facts it cannot compute — so
//! it serves a FIXTURE chain, and the rule this suite pins is what that fixture has to
//! be for the API to be usable end to end:
//!
//! * it SPANS each network's life, from a chain start at or after that network's genesis
//!   block up to the instant the state was built, so a balance pinned years back and a
//!   listing filtered to a month-long window back then both resolve;
//! * every address it holds has enough transactions to PAGE, and every month-long window
//!   of the busiest address's life is non-empty;
//! * the five implemented reads AGREE: a summary a listing returns is fetchable by
//!   `GetTransaction` (by `transactionId` too, on the Bitcoin networks), its events are
//!   what `ListTransactionEvents` returns, and the balances `GetTokenBalance` and
//!   `ListTokenBalances` report come from those events;
//! * `confirmationStatusFilter` is additive, and `sortOrder` reverses the listing;
//! * and an identifier the fixture does NOT hold is answered with the modelled empty
//!   page or `ResourceNotFoundException` — never with a history made up from the
//!   request.
//!
//! Each test is a TABLE. Its rows are read out of the ledger the service actually serves
//! (every network, every address, every contract) rather than copied here, crossed with
//! windows and page sizes taken from the ledger's own span, and — for the last test —
//! identifiers that differ in form, length, case and alphabet, the dimensions
//! `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$` generalises over.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has
//! a single front door for every suite.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::{
    CONFIRMATION_STATUS_VALUES, QUERY_TRANSACTION_EVENT_TYPE_VALUES,
};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::chain::network_genesis;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::is_bitcoin;

use crate::chain_query_slice::{hashes, Emulator, NetworkFixture};

/// Identifiers the fixture ledger does not hold. They satisfy `ChainAddress`'s
/// `^[-A-Za-z0-9]{13,74}$` and differ in form, length, case and alphabet:
///
/// * the mixed-case hex form an Ethereum mainnet wallet is written in (the wallet the
///   recorded trace queried — one row, not the rule);
/// * the base58 alphabet a Bitcoin address is written in, over a body no fixture
///   produced (a PUBLISHED Bitcoin address would not do: those the index holds, which
///   is what `state::derived`'s catalog is for);
/// * a lowercase bech32-shaped address;
/// * a hyphenated label;
/// * the shortest and the longest the pattern admits.
const STRANGERS: [&str; 6] = [
    "0x28C6C06298d514Db089934071355E5743bf21d60",
    "1NotASeededBitcoinAddressAtAll",
    "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq",
    "a-stranger-with-hyphens",
    "aaaaaaaaaaaaa",
    "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
];

/// 2023-01-01T00:00:00Z and 2023-02-01T00:00:00Z: the month-long historical window the
/// recorded traces query. Used as a row wherever the ledger's own span covers it.
const RECORDED_WINDOW: (i64, i64) = (1_672_531_200, 1_675_209_600);

/// A month, in seconds, for the windows this suite builds out of the ledger's span.
const MONTH: i64 = 31 * 24 * 60 * 60;

/// How far back a fixture ledger has to reach for a client's historical reads to
/// resolve: more than three years of chain before its tip, so an instant pinned a few
/// years ago is inside it.
const REQUIRED_SPAN: i64 = 3 * 365 * 24 * 60 * 60;

/// The `tokenId` of `network`'s own currency.
///
/// `TokenIdentifier$tokenId`: "For native tokens, use the 3 character abbreviation that
/// best matches your token. For example, btc for Bitcoin, eth for Ether".
fn native_token(network: &str) -> &'static str {
    if is_bitcoin(network) {
        "btc"
    } else {
        "eth"
    }
}

/// The items of a list response, under the member `@paginated(items=...)` names.
fn items(reply: &Value, member: &str) -> Vec<Value> {
    reply[member].as_array().cloned().unwrap_or_default()
}

/// The `transactionTimestamp` of every item of a transaction listing.
fn timestamps(reply: &Value) -> Vec<i64> {
    items(reply, "transactions")
        .iter()
        .filter_map(|item| item["transactionTimestamp"].as_f64())
        .map(|secs| secs as i64)
        .collect()
}

/// A `BlockchainInstant`'s whole seconds, from a response member.
fn instant(value: &Value) -> Option<i64> {
    value["time"].as_f64().map(|secs| secs as i64)
}

/// The `balance` of a balance response, which the model types as a String and the
/// service reports as a base-10 integer.
fn balance(case: &str, reply: &Value) -> i128 {
    let raw = reply["balance"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: balance is required, got {reply:?}"));
    raw.parse::<i128>()
        .unwrap_or_else(|_| panic!("case {case}: expected a base-10 integer balance, got {raw}"))
}

/// The instants the ledger's own transactions run between.
fn span(fixture: &NetworkFixture) -> (i64, i64) {
    let instants: Vec<i64> = fixture
        .transactions
        .iter()
        .map(|transaction| transaction.transaction_timestamp)
        .collect();
    let earliest = instants
        .iter()
        .copied()
        .min()
        .unwrap_or_else(|| panic!("case {}: the ledger holds no transaction", fixture.network));
    let latest = instants.iter().copied().max().unwrap_or(earliest);
    (earliest, latest)
}

/// The month-long windows a listing is read for: the first month of the ledger's life, a
/// month in the middle of it, the month that ends at its tip, and — where the span
/// covers it — the month the recorded traces query.
fn windows(fixture: &NetworkFixture) -> Vec<(i64, i64)> {
    let (earliest, latest) = span(fixture);
    let mut rows = vec![
        (earliest, earliest + MONTH),
        (
            earliest + (latest - earliest) / 2,
            earliest + (latest - earliest) / 2 + MONTH,
        ),
        (latest - MONTH, latest),
    ];
    let (from, to) = RECORDED_WINDOW;
    if from >= earliest && to <= latest {
        rows.push(RECORDED_WINDOW);
    }
    rows
}

/// Walk a listing, following `nextToken` for at most `pages` pages.
async fn walk(
    emulator: &Emulator,
    path: &str,
    member: &str,
    body: &Value,
    pages: usize,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next: Option<String> = None;
    for _ in 0..pages {
        let mut request = body.clone();
        if let Some(token) = &next {
            request["nextToken"] = json!(token);
        }
        let reply = emulator.ok(path, &request).await;
        collected.extend(items(&reply, member));
        next = reply["nextToken"].as_str().map(str::to_string);
        if next.is_none() {
            break;
        }
    }
    collected
}

#[tokio::test]
async fn the_ledger_spans_the_networks_life_and_every_address_pages() {
    let emulator = Emulator::new();
    let fixture = emulator.fixture();
    assert!(
        !fixture.is_empty(),
        "every QueryNetwork value must have a seeded ledger, got none"
    );

    for network in &fixture {
        let (earliest, latest) = span(network);
        let genesis = network_genesis(&network.network);
        assert!(
            earliest >= genesis,
            "case {}: nothing on a chain predates its genesis block ({genesis}), got {earliest}",
            network.network
        );
        assert!(
            latest - earliest >= REQUIRED_SPAN,
            "case {}: the ledger must hold more than {REQUIRED_SPAN} seconds of chain so \
             a historical read resolves, got {}",
            network.network,
            latest - earliest
        );

        for address in network.addresses() {
            let case = format!("{}/{address}", network.network);
            let stored = network.hashes_for(&address);
            // `maxResults` carries `@range(min: 1, max: 250)`; five is the page the
            // recorded trace asks for.
            let body = json!({"network": network.network, "address": address, "maxResults": 5});
            let reply = emulator.ok("/list-transactions", &body).await;
            let page = items(&reply, "transactions");
            assert_eq!(
                page.len(),
                stored.len().min(5),
                "case {case}: a page of five holds the first five of this address's {} \
                 transactions, got {page:?}",
                stored.len()
            );
            // A token comes back exactly when the listing has more rows than the page.
            let token = reply["nextToken"].as_str().map(str::to_string);
            assert_eq!(
                token.is_some(),
                stored.len() > 5,
                "case {case}: an address with {} transactions {} a nextToken on a page of \
                 five, got {reply:?}",
                stored.len(),
                if stored.len() > 5 { "carries" } else { "does not carry" }
            );

            // `ListTransactionsSort$sortOrder`: absent defaults to ASCENDING.
            let instants = timestamps(&reply);
            assert!(
                instants.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case}: the default order is ascending by transactionTimestamp, \
                 got {instants:?}"
            );

            for item in &page {
                assert_eq!(
                    item["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {case}: expected network {}, got {item:?}",
                    network.network
                );
                let hash = item["transactionHash"].as_str().unwrap_or_default();
                let digits = hash.strip_prefix("0x").unwrap_or(hash);
                assert!(
                    digits.len() == 64 && digits.chars().all(|digit| digit.is_ascii_hexdigit()),
                    "case {case}: `QueryTransactionHash` is \
                     ^(0x[A-Fa-f0-9]{{64}}|[A-Fa-f0-9]{{64}})$, got {hash}"
                );
                let status = item["confirmationStatus"].as_str().unwrap_or_default();
                assert!(
                    CONFIRMATION_STATUS_VALUES.contains(&status),
                    "case {case}: expected a ConfirmationStatus value, got {status}"
                );
                // `GetTransactionInput$transactionId`: "transactionId is only supported
                // on the Bitcoin networks".
                assert_eq!(
                    item["transactionId"].is_string(),
                    is_bitcoin(&network.network),
                    "case {case}: a Bitcoin summary carries a transactionId and no other \
                     network's does, got {item:?}"
                );
            }

            // The second page resumes where the first stopped: no item twice.
            if let Some(token) = token {
                let mut resumed = body.clone();
                resumed["nextToken"] = json!(token);
                let second = emulator.ok("/list-transactions", &resumed).await;
                let followed = items(&second, "transactions");
                assert!(
                    !followed.is_empty() && followed.len() <= 5,
                    "case {case}: expected 1..=5 transactions on the second page, got \
                     {followed:?}"
                );
                let first_hashes = hashes(&reply["transactions"]);
                for hash in hashes(&second["transactions"]) {
                    assert!(
                        !first_hashes.contains(&hash),
                        "case {case}: {hash} is on both pages, so paging does not resume \
                         after the first ({first_hashes:?})"
                    );
                }
            }
        }

        // The busiest address — the one a client with a heavy history reads — has many
        // pages of five, which is what makes the paging path reachable at all.
        let busiest = network.busiest_address();
        assert!(
            network.hashes_for(&busiest).len() > 5,
            "case {}/{busiest}: the busiest address must hold more than one page of \
             five transactions, got {}",
            network.network,
            network.hashes_for(&busiest).len()
        );
    }
}

#[tokio::test]
async fn a_window_anywhere_in_the_ledgers_life_holds_the_busiest_addresss_transactions() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let address = network.busiest_address();
        for (from, to) in windows(&network) {
            let case = format!("{}/{address}@{from}..{to}", network.network);
            let body = json!({
                "network": network.network,
                "address": address,
                "fromBlockchainInstant": {"time": from},
                "toBlockchainInstant": {"time": to},
            });
            let reply = emulator.ok("/list-transactions", &body).await;
            let instants = timestamps(&reply);
            assert!(
                !instants.is_empty(),
                "case {case}: a month of the busiest address's life is not empty, got {reply:?}"
            );
            assert!(
                instants
                    .iter()
                    .all(|instant| *instant >= from && *instant <= to),
                "case {case}: every transaction is inside the window, got {instants:?}"
            );

            // `ListTransactionsSort$sortOrder` DESCENDING is the same listing, read from
            // the other end.
            let mut descending = body.clone();
            descending["sort"] =
                json!({"sortBy": "TRANSACTION_TIMESTAMP", "sortOrder": "DESCENDING"});
            let reversed = emulator.ok("/list-transactions", &descending).await;
            let mut expected = hashes(&reply["transactions"]);
            expected.reverse();
            let got = hashes(&reversed["transactions"]);
            assert_eq!(
                got, expected,
                "case {case}: DESCENDING must reverse the listing, expected {expected:?}, \
                 got {got:?}"
            );

            // `ConfirmationStatusFilter$include`: "Transactions that have reached
            // finality are always part of the response", so neither filter can drop a
            // transaction that has, and a filter naming only the non-final ones is the
            // whole window again whenever the window holds no non-final transaction of
            // its own.
            let whole = hashes(&reply["transactions"]);
            for include in [vec!["FINAL"], vec!["NONFINAL"], vec!["FINAL", "NONFINAL"]] {
                let mut filtered = body.clone();
                filtered["confirmationStatusFilter"] = json!({"include": include});
                let got = hashes(&emulator.ok("/list-transactions", &filtered).await["transactions"]);
                let final_only: Vec<String> = network
                    .transactions
                    .iter()
                    .filter(|transaction| transaction.addresses().contains(&address.as_str()))
                    .filter(|transaction| {
                        transaction.transaction_timestamp >= from
                            && transaction.transaction_timestamp <= to
                    })
                    .filter(|transaction| transaction.confirmation_status == "FINAL")
                    .map(|transaction| transaction.transaction_hash.clone())
                    .collect();
                let expected = if include.contains(&"NONFINAL") {
                    whole.clone()
                } else {
                    final_only
                };
                assert_eq!(
                    got, expected,
                    "case {case} include={include:?}: expected {expected:?}, got {got:?}"
                );
            }
        }

        // A window that closes before the ledger's chain starts holds nothing, and an
        // empty page carries no token.
        let (earliest, _) = span(&network);
        let before = emulator
            .ok(
                "/list-transactions",
                &json!({
                    "network": network.network,
                    "address": address,
                    "toBlockchainInstant": {"time": earliest - 1},
                }),
            )
            .await;
        assert!(
            items(&before, "transactions").is_empty() && before["nextToken"].is_null(),
            "case {}/{address}: nothing precedes the ledger's first transaction, got \
             {before:?}",
            network.network
        );
    }
}

#[tokio::test]
async fn a_listed_summary_resolves_through_get_transaction_and_its_events() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for address in network.addresses() {
            let case = format!("{}/{address}", network.network);
            let listed = emulator
                .ok(
                    "/list-transactions",
                    &json!({"network": network.network, "address": address, "maxResults": 1}),
                )
                .await;
            let summary = items(&listed, "transactions")
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("case {case}: expected a summary, got {listed:?}"));
            let hash = summary["transactionHash"]
                .as_str()
                .expect("a summary carries its transactionHash")
                .to_string();

            let fetched = emulator
                .ok(
                    "/get-transaction",
                    &json!({"network": network.network, "transactionHash": hash}),
                )
                .await;
            let transaction = &fetched["transaction"];
            for member in ["transactionHash", "network", "transactionTimestamp"] {
                assert_eq!(
                    transaction[member], summary[member],
                    "case {case}: GetTransaction must agree with the summary on {member}, \
                     got {transaction:?} for {summary:?}"
                );
            }
            for member in ["blockHash", "blockNumber", "numberOfTransactions"] {
                assert!(
                    !transaction[member].is_null(),
                    "case {case}: a transaction the ledger holds knows its {member}, got \
                     {transaction:?}"
                );
            }
            if is_bitcoin(&network.network) {
                // `GetTransactionInput$transactionId`: "transactionId is only supported
                // on the Bitcoin networks" — and it resolves the same transaction.
                let id = transaction["transactionId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a Bitcoin transaction carries a transactionId")
                    })
                    .to_string();
                let by_id = emulator
                    .ok(
                        "/get-transaction",
                        &json!({"network": network.network, "transactionId": id}),
                    )
                    .await;
                assert_eq!(
                    by_id["transaction"]["transactionHash"].as_str(),
                    Some(hash.as_str()),
                    "case {case}: transactionId {id} must resolve the same transaction, got \
                     {by_id:?}"
                );
            } else {
                for member in ["gasUsed", "effectiveGasPrice", "transactionFee"] {
                    assert!(
                        transaction[member].is_string(),
                        "case {case}: an Ethereum transaction reports {member}, got \
                         {transaction:?}"
                    );
                }
            }

            let events = emulator
                .ok(
                    "/list-transaction-events",
                    &json!({"network": network.network, "transactionHash": hash}),
                )
                .await;
            let stored = network
                .transactions
                .iter()
                .find(|candidate| candidate.transaction_hash == hash)
                .unwrap_or_else(|| panic!("case {case}: {hash} is not in the ledger"));
            let reported = items(&events, "events");
            assert_eq!(
                reported.len(),
                stored.events.len(),
                "case {case}: expected the transaction's {} stored events, got {reported:?}",
                stored.events.len()
            );
            for event in &reported {
                assert_eq!(
                    event["transactionHash"].as_str(),
                    Some(hash.as_str()),
                    "case {case}: every event belongs to {hash}, got {event:?}"
                );
                assert_eq!(
                    event["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {case}: every event is on {}, got {event:?}",
                    network.network
                );
                let kind = event["eventType"].as_str().unwrap_or_default();
                assert!(
                    QUERY_TRANSACTION_EVENT_TYPE_VALUES.contains(&kind),
                    "case {case}: expected a QueryTransactionEventType value, got {kind}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_native_balance_resolves_now_and_at_instants_across_the_ledgers_life() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let token = json!({
            "network": network.network,
            "tokenId": native_token(&network.network),
        });
        let (earliest, latest) = span(&network);
        let address = network.busiest_address();
        let case = format!("{}/{address}", network.network);
        let body = json!({
            "tokenIdentifier": token,
            "ownerIdentifier": {"address": address},
        });

        let opened = network.opened_at();
        let clock_before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the test clock is after the Unix epoch")
            .as_secs() as i64;
        let reply = emulator.ok("/get-token-balance", &body).await;
        let clock_after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the test clock is after the Unix epoch")
            .as_secs() as i64;
        let held = balance(&case, &reply);
        assert!(
            held > 0,
            "case {case}: the busiest wallet received more than it sent, so its balance is \
             positive, got {held}"
        );
        assert_eq!(
            reply["ownerIdentifier"]["address"].as_str(),
            Some(address.as_str()),
            "case {case}: the owner is echoed back, got {reply:?}"
        );
        assert_eq!(
            reply["tokenIdentifier"], token,
            "case {case}: the token is echoed back, got {reply:?}"
        );
        // "The time for when the TokenBalance is requested or the CURRENT TIME if a time
        // is not provided in the request": the instant the call itself saw, which is
        // inside its own wall-clock window — not a fixed epoch.
        let reported = instant(&reply["atBlockchainInstant"])
            .unwrap_or_else(|| panic!("case {case}: atBlockchainInstant is required: {reply:?}"));
        assert!(
            (clock_before..=clock_after).contains(&reported),
            "case {case}: an unpinned read resolves to now ({clock_before}..={clock_after}), \
             got {reported}"
        );

        // Rows: the instant the ledger opened, one in the middle of its life, its tip,
        // and — where the span covers it — the instant the recorded traces pin.
        let mut pinned = vec![opened, earliest + (latest - earliest) / 2, latest];
        if RECORDED_WINDOW.0 >= earliest && RECORDED_WINDOW.0 <= latest {
            pinned.push(RECORDED_WINDOW.0);
        }
        for at in pinned {
            let case = format!("{case}@{at}");
            let mut historical = body.clone();
            historical["atBlockchainInstant"] = json!({"time": at});
            let past = emulator.ok("/get-token-balance", &historical).await;
            let then = balance(&case, &past);
            assert!(
                then > 0,
                "case {case}: the wallet held the network's currency at that instant, got \
                 {then}"
            );
            assert_eq!(
                instant(&past["atBlockchainInstant"]),
                Some(at),
                "case {case}: a pinned read reports the instant it was asked for, got {past:?}"
            );
            let updated = instant(&past["lastUpdatedTime"])
                .unwrap_or_else(|| panic!("case {case}: expected a lastUpdatedTime: {past:?}"));
            assert!(
                updated <= at,
                "case {case}: the balance in force was last updated at or before that \
                 instant, got {updated}"
            );
            // The same read twice is the same answer: a read of this service writes
            // nothing.
            let again = emulator.ok("/get-token-balance", &historical).await;
            assert_eq!(
                again["balance"], past["balance"],
                "case {case}: a second read reports the same balance, got {again:?} after \
                 {past:?}"
            );
        }

        // Before the ledger opened, no balance is known for the holding.
        let mut impossible = body.clone();
        impossible["atBlockchainInstant"] = json!({"time": opened - 1});
        emulator
            .fails(
                "/get-token-balance",
                &impossible,
                404,
                "ResourceNotFoundException",
            )
            .await;

        // And `ListTokenBalances` agrees with `GetTokenBalance` for the same key.
        let listed = emulator
            .ok(
                "/list-token-balances",
                &json!({
                    "tokenFilter": token,
                    "ownerFilter": {"address": address},
                }),
            )
            .await;
        let rows = items(&listed, "tokenBalances");
        assert_eq!(
            rows.len(),
            1,
            "case {case}: one holding matches that filter, got {rows:?}"
        );
        assert_eq!(
            rows[0]["balance"].as_str(),
            Some(held.to_string().as_str()),
            "case {case}: ListTokenBalances must agree with GetTokenBalance ({held}), got \
             {rows:?}"
        );
    }
}

#[tokio::test]
async fn an_owner_scoped_listing_returns_the_native_row_and_every_token_row() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for address in network.addresses() {
            let case = format!("{}/{address}", network.network);
            // A `tokenFilter` naming only the network widens the listing to every token
            // standard the owner holds.
            let rows = walk(
                &emulator,
                "/list-token-balances",
                "tokenBalances",
                &json!({
                    "tokenFilter": {"network": network.network},
                    "ownerFilter": {"address": address},
                    "maxResults": 50,
                }),
                4,
            )
            .await;
            let expected: Vec<String> = network
                .holdings
                .iter()
                .filter(|(key, _)| key.owner_address == address)
                .map(|(key, _)| key.resource_id())
                .collect();
            if expected.is_empty() {
                // A contract address takes part in transactions without holding a
                // balance of its own; it has no rows to list.
                assert!(
                    rows.is_empty(),
                    "case {case}: the ledger records no holding for this address, got {rows:?}"
                );
                continue;
            }
            for row in &rows {
                assert_eq!(
                    row["ownerIdentifier"]["address"].as_str(),
                    Some(address.as_str()),
                    "case {case}: every row belongs to the filtered owner, got {row:?}"
                );
                assert_eq!(
                    row["tokenIdentifier"]["network"].as_str(),
                    Some(network.network.as_str()),
                    "case {case}: every row is on the filtered network, got {row:?}"
                );
                let raw = row["balance"].as_str().unwrap_or_default();
                assert!(
                    raw.parse::<i128>().is_ok_and(|balance| balance >= 0),
                    "case {case}: a balance is a base-10 integer and no chain holds a \
                     negative one, got {raw}"
                );
            }
            let got: Vec<String> = rows
                .iter()
                .map(|row| {
                    format!(
                        "{}/{}/{}",
                        row["tokenIdentifier"]["contractAddress"]
                            .as_str()
                            .unwrap_or("-"),
                        row["tokenIdentifier"]["tokenId"].as_str().unwrap_or("-"),
                        row["ownerIdentifier"]["address"].as_str().unwrap_or("-")
                    )
                })
                .collect();
            assert_eq!(
                got, expected,
                "case {case}: expected every holding of this owner, got {got:?}"
            );

            // The native row: a `tokenIdentifier` with a `tokenId` and no
            // `contractAddress`.
            let native = rows.iter().any(|row| {
                row["tokenIdentifier"]["tokenId"].as_str() == Some(native_token(&network.network))
                    && row["tokenIdentifier"]["contractAddress"].is_null()
            });
            assert!(
                native,
                "case {case}: an address that holds anything holds the native currency, got \
                 {rows:?}"
            );

            // A contract row beside it — on the networks that have asset contracts.
            // `GetAssetContract`: "the Bitcoin blockchain networks do not support this
            // operation", and they have no token contracts to hold balances of.
            let contract_rows = rows
                .iter()
                .filter(|row| row["tokenIdentifier"]["contractAddress"].is_string())
                .count();
            if is_bitcoin(&network.network) {
                assert_eq!(
                    contract_rows, 0,
                    "case {case}: a Bitcoin network has no token contracts, got {rows:?}"
                );
            } else {
                assert!(
                    contract_rows > 0,
                    "case {case}: a wallet of this ledger holds at least one token-contract \
                     balance row, got {rows:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_contract_scoped_listing_returns_the_holders_of_that_contract() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for contract in &network.contracts {
            let case = format!("{}/{contract}", network.network);
            let rows = walk(
                &emulator,
                "/list-token-balances",
                "tokenBalances",
                &json!({
                    "tokenFilter": {"network": network.network, "contractAddress": contract},
                    "maxResults": 10,
                }),
                10,
            )
            .await;
            assert!(
                !rows.is_empty(),
                "case {case}: a token contract the ledger holds has holders, got an empty \
                 tokenBalances list"
            );
            let mut owners = Vec::new();
            for row in &rows {
                assert_eq!(
                    row["tokenIdentifier"]["contractAddress"].as_str(),
                    Some(contract.as_str()),
                    "case {case}: every row is a balance of the filtered contract, got {row:?}"
                );
                let raw = row["balance"].as_str().unwrap_or_default();
                assert!(
                    raw.parse::<i128>().is_ok_and(|balance| balance >= 0),
                    "case {case}: a balance is a base-10 integer and no chain holds a \
                     negative one, got {raw}"
                );
                let owner = row["ownerIdentifier"]["address"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    (13..=74).contains(&owner.chars().count())
                        && owner
                            .chars()
                            .all(|letter| letter == '-' || letter.is_ascii_alphanumeric()),
                    "case {case}: a holder's address satisfies ChainAddress's \
                     ^[-A-Za-z0-9]{{13,74}}$, got {owner}"
                );
                owners.push((owner, row["tokenIdentifier"]["tokenId"].clone()));
            }
            let mut distinct = owners.clone();
            distinct.sort_by_key(|(owner, token)| (owner.clone(), token.to_string()));
            distinct.dedup();
            assert_eq!(
                distinct.len(),
                owners.len(),
                "case {case}: each (holder, token) appears once, got {owners:?}"
            );
            // A holder row is a real balance: at least one of them is positive, or the
            // contract has no tokens at all.
            assert!(
                rows.iter().any(|row| row["balance"]
                    .as_str()
                    .and_then(|raw| raw.parse::<i128>().ok())
                    .is_some_and(|balance| balance > 0)),
                "case {case}: a contract with holders has a positive balance among them, got \
                 {rows:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_non_fungible_token_has_exactly_one_holder_of_one_unit() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // The non-fungible holdings of this ledger: a token identified by a contract AND
        // a token id, which by the standard exists once and so is held by exactly one
        // address at a time.
        let mut tokens: Vec<(String, String)> = network
            .holdings
            .iter()
            .filter_map(|(key, _)| {
                Some((key.contract_address.clone()?, key.token_id.clone()?))
            })
            .collect();
        tokens.sort();
        tokens.dedup();
        if is_bitcoin(&network.network) {
            assert!(
                tokens.is_empty(),
                "case {}: a Bitcoin network has no token contracts, got {tokens:?}",
                network.network
            );
            continue;
        }
        assert!(
            !tokens.is_empty(),
            "case {}: the ledger must hold non-fungible tokens for this rule to have a \
             case, got none",
            network.network
        );

        for (contract, token) in tokens {
            let case = format!("{}/{contract}/{token}", network.network);
            let rows = walk(
                &emulator,
                "/list-token-balances",
                "tokenBalances",
                &json!({
                    "tokenFilter": {
                        "network": network.network,
                        "contractAddress": contract,
                        "tokenId": token,
                    },
                    "maxResults": 50,
                }),
                3,
            )
            .await;
            let balances: Vec<i128> = rows
                .iter()
                .map(|row| {
                    let raw = row["balance"].as_str().unwrap_or_default();
                    raw.parse::<i128>().unwrap_or_else(|_| {
                        panic!("case {case}: expected a base-10 integer balance, got {raw}")
                    })
                })
                .collect();
            assert!(
                balances.iter().all(|balance| *balance >= 0),
                "case {case}: no chain holds a negative balance, got {balances:?}"
            );
            assert_eq!(
                balances.iter().filter(|balance| **balance > 0).count(),
                1,
                "case {case}: one address holds a non-fungible token, got {rows:?}"
            );
            assert!(
                balances.iter().all(|balance| *balance <= 1),
                "case {case}: a non-fungible token is one unit, got {balances:?}"
            );
            // And the holder `GetTokenBalance` reports is the one the listing names.
            let holder = rows
                .iter()
                .find(|row| row["balance"].as_str() == Some("1"))
                .and_then(|row| row["ownerIdentifier"]["address"].as_str())
                .unwrap_or_else(|| panic!("case {case}: expected a holder row: {rows:?}"))
                .to_string();
            let reply = emulator
                .ok(
                    "/get-token-balance",
                    &json!({
                        "tokenIdentifier": {
                            "network": network.network,
                            "contractAddress": contract,
                            "tokenId": token,
                        },
                        "ownerIdentifier": {"address": holder},
                    }),
                )
                .await;
            assert_eq!(
                reply["balance"].as_str(),
                Some("1"),
                "case {case}: the holder holds exactly one, got {reply:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_the_ledger_does_not_hold_is_an_empty_page_or_not_found() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        for stranger in STRANGERS {
            let case = format!("{}/{stranger}", network.network);

            // No history: the emulator holds no index of the public chain, so it reports
            // nothing rather than a history made up from the request.
            let listed = emulator
                .ok(
                    "/list-transactions",
                    &json!({"network": network.network, "address": stranger}),
                )
                .await;
            assert!(
                items(&listed, "transactions").is_empty() && listed["nextToken"].is_null(),
                "case {case}: an identifier the ledger does not hold has no transactions and \
                 no nextToken, got {listed:?}"
            );

            // No holdings, as an owner or as a contract.
            for filter in [
                json!({
                    "tokenFilter": {"network": network.network},
                    "ownerFilter": {"address": stranger},
                }),
                json!({
                    "tokenFilter": {"network": network.network, "contractAddress": stranger},
                }),
            ] {
                let balances = emulator.ok("/list-token-balances", &filter).await;
                assert!(
                    items(&balances, "tokenBalances").is_empty()
                        && balances["nextToken"].is_null(),
                    "case {case} filter={filter}: expected an empty tokenBalances list, got \
                     {balances:?}"
                );
            }

            // And a balance read of it is the modelled `ResourceNotFoundException`.
            emulator
                .fails(
                    "/get-token-balance",
                    &json!({
                        "tokenIdentifier": {
                            "network": network.network,
                            "tokenId": native_token(&network.network),
                        },
                        "ownerIdentifier": {"address": stranger},
                    }),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
        }
    }
}

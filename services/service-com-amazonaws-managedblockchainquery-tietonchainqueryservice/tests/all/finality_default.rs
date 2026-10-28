// SPDX-License-Identifier: CC0-1.0

//! What a finality filter does to a listing, on the two operations that model one:
//! `ListFilteredTransactionEvents` and `ListTransactions`.
//!
//! `ConfirmationStatusFilter$include` is documented "The container to determine whether
//! to list results that have only reached finality. Transactions that have reached
//! finality are always part of the response", and the member that carries the filter
//! states the same rule from the other side: "This filter is used to include
//! transactions in the response that haven't reached finality."
//!
//! So the filter only ever ADDS results. The rule this suite pins over the whole class
//! of filter forms the model admits:
//!
//! * a result that has reached finality is listed whatever the request carries;
//! * a result that has NOT reached finality is listed only where the request's filter
//!   names its `confirmationStatus`;
//! * and therefore a request carrying NO filter lists exactly the results that have
//!   reached finality — it is not the whole listing.
//!
//! Each test is a TABLE. Its rows are the filter forms, which differ in which statuses
//! they name, how many, in what order and with what repetition (`include` is
//! `@length(min: 1)` with no maximum and no `@uniqueItems`, over the two values of
//! `ConfirmationStatus`), crossed with every network the operation serves, every
//! recipient its ledger holds, and — in the second test of each pair — the operation's
//! other filters, its sort order and its page sizes. The expectation for each row is
//! computed from what the ledger HOLDS: for the event listings, from the events the
//! sibling per-transaction listing reports (`ListTransactionEvents` models no finality
//! filter at all), and for `ListTransactions`, from the stored transactions of the
//! seeded ledger — never a copy of the handler under test.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has
//! a single front door for every suite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::CONFIRMATION_STATUS_VALUES;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::is_bitcoin;

use crate::chain_query_slice::{
    drain, hashes, unseeded_address, unseeded_bitcoin_address, Emulator, NetworkFixture,
    FILTERED_EVENTS, TRANSACTIONS, TRANSACTION_EVENTS,
};

/// The `ConfirmationStatus` value that means a result has reached finality, read off the
/// model's own value list rather than written down here: the enum declares exactly two
/// values, and the other one is what has NOT reached finality.
fn final_status() -> &'static str {
    CONFIRMATION_STATUS_VALUES
        .iter()
        .copied()
        .find(|value| *value == "FINAL")
        .expect("ConfirmationStatus declares FINAL")
}

/// The other one; see [`final_status`].
fn nonfinal_status() -> &'static str {
    CONFIRMATION_STATUS_VALUES
        .iter()
        .copied()
        .find(|value| *value != final_status())
        .expect("ConfirmationStatus declares a second value")
}

/// Every filter form this suite spans, as `(label, include)` with `None` for the request
/// that carries no `confirmationStatusFilter` at all.
///
/// The rows differ along what the rule generalises over: whether a filter is present,
/// which statuses it names, how many, in what order, and whether it repeats one.
fn filter_forms() -> Vec<(String, Option<Vec<&'static str>>)> {
    let reached = final_status();
    let pending = nonfinal_status();
    vec![
        ("absent".to_string(), None),
        (format!("[{reached}]"), Some(vec![reached])),
        (format!("[{pending}]"), Some(vec![pending])),
        (
            format!("[{reached},{pending}]"),
            Some(vec![reached, pending]),
        ),
        (
            format!("[{pending},{reached}]"),
            Some(vec![pending, reached]),
        ),
        (
            format!("[{pending},{pending}]"),
            Some(vec![pending, pending]),
        ),
    ]
}

/// A `ListFilteredTransactionEvents` request for the events sent to `address`, carrying
/// `include` as its `ConfirmationStatusFilter$include` when the row names one.
fn request(network: &str, address: &str, include: Option<&[&str]>) -> Value {
    let mut body = json!({
        "network": network,
        "addressIdentifierFilter": {"transactionEventToAddress": [address]},
    });
    if let Some(include) = include {
        body["confirmationStatusFilter"] = json!({"include": include});
    }
    body
}

/// The `confirmationStatus` one event reports.
fn status(event: &Value) -> &str {
    event["confirmationStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("event carries no confirmationStatus: {event:?}"))
}

/// Whether a record whose `confirmationStatus` is `status` is listed by a request whose
/// filter named `include` (`None` for a request carrying no filter) — the rule this suite
/// pins, stated once and applied to every row.
fn listed_status(status: &str, include: Option<&[&str]>) -> bool {
    status == final_status() || include.is_some_and(|include| include.contains(&status))
}

/// [`listed_status`] for one item of a listing.
fn listed(event: &Value, include: Option<&[&str]>) -> bool {
    listed_status(status(event), include)
}

/// Every event the ledger of `network` holds, grouped by the address it was sent TO and
/// in chain order, as `ListTransactionEvents` reports them.
///
/// `ListTransactionEvents` models no finality filter, so this is the ledger's whole view
/// of one recipient's events and the expectation every row below is computed from. One
/// pass over the ledger rather than one per address.
async fn events_by_recipient(
    emulator: &Emulator,
    network: &NetworkFixture,
) -> BTreeMap<String, Vec<Value>> {
    let mut by_recipient: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for transaction in &network.transactions {
        let events = drain(
            emulator,
            TRANSACTION_EVENTS,
            "events",
            &json!({
                "network": network.network,
                "transactionHash": transaction.transaction_hash,
            }),
            None,
        )
        .await;
        for event in events {
            if let Some(recipient) = event["to"].as_str() {
                by_recipient
                    .entry(recipient.to_string())
                    .or_default()
                    .push(event);
            }
        }
    }
    by_recipient
}

#[tokio::test]
async fn the_confirmation_status_filter_only_ever_adds_events_that_have_not_reached_finality() {
    let emulator = Emulator::new();
    let forms = filter_forms();
    // The rule is only observable on a ledger that holds both kinds of result, so the
    // sweep records whether it met one of each and says so rather than passing vacuously.
    let mut final_seen = false;
    let mut nonfinal_seen = false;
    let mut narrowed_seen = false;

    for network in emulator.fixture() {
        // "This operation is only supported on the Bitcoin networks."
        if !is_bitcoin(&network.network) {
            continue;
        }
        let held = events_by_recipient(&emulator, &network).await;
        // Rows: every address the ledger knows — recipients of many events, of one, and
        // of none — plus a well-formed address it holds nothing under.
        let mut addresses: Vec<String> = network.addresses().into_iter().collect();
        addresses.push(unseeded_bitcoin_address());

        for address in addresses {
            let empty = Vec::new();
            let whole = held.get(&address).unwrap_or(&empty);
            final_seen |= whole.iter().any(|event| status(event) == final_status());
            nonfinal_seen |= whole.iter().any(|event| status(event) != final_status());

            for (label, include) in &forms {
                let case = format!("{}/{address}/include={label}", network.network);
                let include = include.as_deref();
                let want: Vec<Value> = whole
                    .iter()
                    .filter(|event| listed(event, include))
                    .cloned()
                    .collect();
                let got = drain(
                    &emulator,
                    FILTERED_EVENTS,
                    "events",
                    &request(&network.network, &address, include),
                    None,
                )
                .await;
                assert_eq!(
                    got, want,
                    "case {case}: expected {want:?}, got {got:?}"
                );
                // Whatever the filter says, nothing that has reached finality is ever
                // withheld, and nothing that has not is listed unless it was named.
                for event in &got {
                    assert!(
                        listed(event, include),
                        "case {case}: {} was not opted in to, got {event:?}",
                        status(event)
                    );
                }
                narrowed_seen |= got.len() < whole.len();
            }
        }
    }

    assert!(
        final_seen,
        "case ledger: the fixture must hold an event that reached finality, or the rule \
         is untestable"
    );
    assert!(
        nonfinal_seen,
        "case ledger: the fixture must hold an event that has not reached finality, or \
         the rule is untestable"
    );
    assert!(
        narrowed_seen,
        "case ledger: some row must be narrowed by the rule, or the assertions pass \
         vacuously"
    );
}

#[tokio::test]
async fn an_absent_confirmation_status_filter_is_the_finality_only_listing_under_every_other_filter()
{
    let emulator = Emulator::new();
    let reached = final_status();
    let pending = nonfinal_status();
    // The rule is the operation's, not one request shape's: it holds whatever else the
    // request carries. Rows cross the other filters this operation models
    // (`VoutFilter$voutSpent`, `ListFilteredTransactionEventsSort$sortOrder`) with the
    // page sizes `maxResults` admits (`@range(min: 1, max: 250)`: the published minimum,
    // a size in between, the published maximum, and none at all).
    let vouts: [Option<bool>; 3] = [None, Some(true), Some(false)];
    let orders: [Option<&str>; 3] = [None, Some("ASCENDING"), Some("DESCENDING")];
    let pages: [Option<i32>; 4] = [Some(1), Some(3), Some(250), None];
    let mut compared = 0usize;
    // The assertions below hold trivially over empty pages, so the sweep records that it
    // met a listing with something in it and a listing the rule narrowed.
    let mut served = false;
    let mut narrowed = false;

    for network in emulator.fixture() {
        if !is_bitcoin(&network.network) {
            continue;
        }
        let address = network.busiest_address();

        for vout in vouts {
            for order in orders {
                for page in pages {
                    let decorate = |include: Option<&[&str]>| {
                        let mut body = request(&network.network, &address, include);
                        if let Some(vout) = vout {
                            body["voutFilter"] = json!({"voutSpent": vout});
                        }
                        if let Some(order) = order {
                            body["sort"] =
                                json!({"sortBy": "blockchainInstant", "sortOrder": order});
                        }
                        body
                    };
                    let case = format!(
                        "{}/{address}/voutSpent={vout:?}/sortOrder={order:?}/maxResults={page:?}",
                        network.network
                    );

                    let default =
                        drain(&emulator, FILTERED_EVENTS, "events", &decorate(None), page).await;
                    let final_only = drain(
                        &emulator,
                        FILTERED_EVENTS,
                        "events",
                        &decorate(Some(&[reached])),
                        page,
                    )
                    .await;
                    let whole = drain(
                        &emulator,
                        FILTERED_EVENTS,
                        "events",
                        &decorate(Some(&[reached, pending])),
                        page,
                    )
                    .await;

                    assert_eq!(
                        default, final_only,
                        "case {case}: an absent filter is the finality-only listing, \
                         expected {final_only:?}, got {default:?}"
                    );
                    for event in &default {
                        assert_eq!(
                            status(event),
                            reached,
                            "case {case}: an unfiltered listing carries only events that \
                             reached finality, got {event:?}"
                        );
                    }
                    // Opting in ADDS: the whole listing keeps every event the default
                    // one had, in the same relative order, and may hold more.
                    let kept: Vec<Value> = whole
                        .iter()
                        .filter(|event| status(event) == reached)
                        .cloned()
                        .collect();
                    assert_eq!(
                        kept, default,
                        "case {case}: opting in must not drop or reorder what finality \
                         already listed, expected {default:?}, got {kept:?}"
                    );
                    assert!(
                        whole.len() >= default.len(),
                        "case {case}: opting in cannot shrink the listing, {} < {}",
                        whole.len(),
                        default.len()
                    );
                    served |= !whole.is_empty();
                    narrowed |= default.len() < whole.len();
                    compared += 1;
                }
            }
        }
    }

    assert!(
        compared > 0,
        "case ledger: the operation's networks must have a seeded ledger, compared none"
    );
    assert!(
        served,
        "case ledger: some row must return events, or the assertions pass vacuously"
    );
    assert!(
        narrowed,
        "case ledger: some row must be narrowed by the rule, or the assertions pass \
         vacuously"
    );
}

// ---------------------------------------------------------------------------
// ListTransactions — the same rule, on the other listing that models the filter
// ---------------------------------------------------------------------------

/// A `ListTransactions` request for the transactions `address` took part in, carrying
/// `include` as its `ConfirmationStatusFilter$include` when the row names one.
fn transactions_request(network: &str, address: &str, include: Option<&[&str]>) -> Value {
    let mut body = json!({"network": network, "address": address});
    if let Some(include) = include {
        body["confirmationStatusFilter"] = json!({"include": include});
    }
    body
}

/// The hashes of the stored transactions `address` took part in whose
/// `confirmationStatus` a request naming `include` lists, in chain order.
///
/// Read off the seeded ledger, so the expectation is the state the service holds rather
/// than a second copy of the handler's filtering.
fn held_hashes(network: &NetworkFixture, address: &str, include: Option<&[&str]>) -> Vec<String> {
    network
        .transactions
        .iter()
        .filter(|transaction| transaction.addresses().contains(&address))
        .filter(|transaction| listed_status(&transaction.confirmation_status, include))
        .map(|transaction| transaction.transaction_hash.clone())
        .collect()
}

#[tokio::test]
async fn the_confirmation_status_filter_only_ever_adds_transactions_that_have_not_reached_finality()
{
    let emulator = Emulator::new();
    let forms = filter_forms();
    // The rule is only observable on a ledger holding both kinds of transaction, so the
    // sweep records whether it met one of each and says so rather than passing vacuously.
    let mut final_seen = false;
    let mut nonfinal_seen = false;
    let mut narrowed_seen = false;

    for network in emulator.fixture() {
        // Rows: every address the ledger knows — the busiest, the ones with a single
        // transaction — plus a well-formed address of this chain it holds nothing under.
        let mut addresses: Vec<String> = network.addresses().into_iter().collect();
        addresses.push(unseeded_address(&network.network));

        for address in addresses {
            let whole = held_hashes(&network, &address, Some(&[final_status(), nonfinal_status()]));
            final_seen |= network
                .transactions
                .iter()
                .any(|transaction| transaction.confirmation_status == final_status());
            nonfinal_seen |= network
                .transactions
                .iter()
                .any(|transaction| transaction.confirmation_status != final_status());

            for (label, include) in &forms {
                let case = format!("{}/{address}/include={label}", network.network);
                let include = include.as_deref();
                let want = held_hashes(&network, &address, include);
                let items = drain(
                    &emulator,
                    TRANSACTIONS,
                    "transactions",
                    &transactions_request(&network.network, &address, include),
                    None,
                )
                .await;
                let got = hashes(&Value::Array(items.clone()));
                assert_eq!(got, want, "case {case}: expected {want:?}, got {got:?}");
                // Whatever the filter says, nothing that has reached finality is ever
                // withheld, and nothing that has not is listed unless it was named.
                for item in &items {
                    assert!(
                        listed(item, include),
                        "case {case}: {} was not opted in to, got {item:?}",
                        status(item)
                    );
                }
                narrowed_seen |= got.len() < whole.len();
            }
        }
    }

    assert!(
        final_seen,
        "case ledger: the fixture must hold a transaction that reached finality, or the \
         rule is untestable"
    );
    assert!(
        nonfinal_seen,
        "case ledger: the fixture must hold a transaction that has not reached finality, \
         or the rule is untestable"
    );
    assert!(
        narrowed_seen,
        "case ledger: some row must be narrowed by the rule, or the assertions pass \
         vacuously"
    );
}

#[tokio::test]
async fn an_absent_confirmation_status_filter_is_the_finality_only_transaction_listing() {
    let emulator = Emulator::new();
    let reached = final_status();
    let pending = nonfinal_status();
    // The rule is the operation's, not one request shape's: it holds whatever else the
    // request carries. Rows cross the operation's sort order
    // (`ListTransactionsSort$sortOrder`) and the instant window its two
    // `BlockchainInstant` members bound with the page sizes `maxResults` admits
    // (`@range(min: 1, max: 250)`: the published minimum, a size in between, the
    // published maximum, and none at all).
    let orders: [Option<&str>; 3] = [None, Some("ASCENDING"), Some("DESCENDING")];
    let pages: [Option<i32>; 4] = [Some(1), Some(3), Some(250), None];
    let mut compared = 0usize;
    // The assertions below hold trivially over empty pages, so the sweep records that it
    // met a listing with something in it and a listing the rule narrowed.
    let mut served = false;
    let mut narrowed = false;

    for network in emulator.fixture() {
        let address = network.busiest_address();
        // The windows the rows are read for, out of the address's own history: its whole
        // life, everything from its first transaction on, everything up to its last, and
        // the half of it that ends at its last.
        let instants: Vec<i64> = network
            .transactions
            .iter()
            .filter(|transaction| transaction.addresses().contains(&address.as_str()))
            .map(|transaction| transaction.transaction_timestamp)
            .collect();
        let earliest = *instants
            .iter()
            .min()
            .unwrap_or_else(|| panic!("case {}: the busiest address has no history", network.network));
        let latest = *instants.iter().max().unwrap_or(&earliest);
        let windows: [(Option<i64>, Option<i64>); 4] = [
            (None, None),
            (Some(earliest), None),
            (None, Some(latest)),
            (Some(earliest + (latest - earliest) / 2), Some(latest)),
        ];

        for (from, to) in windows {
            for order in orders {
                for page in pages {
                    let decorate = |include: Option<&[&str]>| {
                        let mut body = transactions_request(&network.network, &address, include);
                        if let Some(from) = from {
                            body["fromBlockchainInstant"] = json!({"time": from});
                        }
                        if let Some(to) = to {
                            body["toBlockchainInstant"] = json!({"time": to});
                        }
                        if let Some(order) = order {
                            body["sort"] =
                                json!({"sortBy": "TRANSACTION_TIMESTAMP", "sortOrder": order});
                        }
                        body
                    };
                    let case = format!(
                        "{}/{address}/window={from:?}..{to:?}/sortOrder={order:?}/\
                         maxResults={page:?}",
                        network.network
                    );

                    let default =
                        drain(&emulator, TRANSACTIONS, "transactions", &decorate(None), page).await;
                    let final_only = drain(
                        &emulator,
                        TRANSACTIONS,
                        "transactions",
                        &decorate(Some(&[reached])),
                        page,
                    )
                    .await;
                    let whole = drain(
                        &emulator,
                        TRANSACTIONS,
                        "transactions",
                        &decorate(Some(&[reached, pending])),
                        page,
                    )
                    .await;

                    assert_eq!(
                        default, final_only,
                        "case {case}: an absent filter is the finality-only listing, \
                         expected {final_only:?}, got {default:?}"
                    );
                    for item in &default {
                        assert_eq!(
                            status(item),
                            reached,
                            "case {case}: an unfiltered listing carries only transactions \
                             that reached finality, got {item:?}"
                        );
                    }
                    // Opting in ADDS: the whole listing keeps every transaction the
                    // default one had, in the same relative order, and may hold more.
                    let kept: Vec<Value> = whole
                        .iter()
                        .filter(|item| status(item) == reached)
                        .cloned()
                        .collect();
                    assert_eq!(
                        kept, default,
                        "case {case}: opting in must not drop or reorder what finality \
                         already listed, expected {default:?}, got {kept:?}"
                    );
                    assert!(
                        whole.len() >= default.len(),
                        "case {case}: opting in cannot shrink the listing, {} < {}",
                        whole.len(),
                        default.len()
                    );
                    served |= !whole.is_empty();
                    narrowed |= default.len() < whole.len();
                    compared += 1;
                }
            }
        }
    }

    assert!(
        compared > 0,
        "case ledger: every network must have a seeded ledger, compared none"
    );
    assert!(
        served,
        "case ledger: some row must return transactions, or the assertions pass vacuously"
    );
    assert!(
        narrowed,
        "case ledger: some row must be narrowed by the rule, or the assertions pass \
         vacuously"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What a finality filter does to a listing, on `ListFilteredTransactionEvents`.
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
//! recipient its ledger holds, and — in the second test — the operation's other filters,
//! its sort order and its page sizes. The expectation for each row is computed from the
//! events the SIBLING per-transaction listing reports, which models no finality filter at
//! all, so it is never a copy of the handler under test.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has
//! a single front door for every suite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::ops::types::CONFIRMATION_STATUS_VALUES;
use service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::state::is_bitcoin;

use crate::chain_query_slice::{
    drain, unseeded_bitcoin_address, Emulator, NetworkFixture, FILTERED_EVENTS, TRANSACTION_EVENTS,
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

/// Whether an event is listed by a request whose filter named `include` — the rule this
/// suite pins, stated once and applied to every row.
fn listed(event: &Value, include: Option<&[&str]>) -> bool {
    status(event) == final_status()
        || include.is_some_and(|include| include.contains(&status(event)))
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

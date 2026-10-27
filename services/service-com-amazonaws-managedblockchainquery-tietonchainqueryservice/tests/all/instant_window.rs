// SPDX-License-Identifier: CC0-1.0

//! The instant window a listing is narrowed by, driven through EVERY operation that
//! takes one.
//!
//! Two of this service's listings carry a window: `ListTransactions` as the
//! `fromBlockchainInstant`/`toBlockchainInstant` pair, and
//! `ListFilteredTransactionEvents` as `timeFilter.from`/`timeFilter.to`. One rule, in two
//! halves, decides what each of them answers:
//!
//! * the window is FORWARDS-ONLY — both bounds present with the start after the end names
//!   no window a caller could have meant, so it is this service's own
//!   `ValidationException` (HTTP 400) whose `fieldList` says which pair of members has to
//!   be ordered, under the generic top-level message every such refusal carries;
//! * and each bound a request carried is INCLUSIVE, while a bound it carried none for
//!   bounds nothing, so a record exactly on a bound is listed and a record one second
//!   outside it is not.
//!
//! Both halves are stated once for the whole service, so this suite drives them through
//! one operation from each side of that seam and asserts the SAME rule of both: the
//! listings may differ in which members carry the window and in which member of an item
//! the window is measured on, and in nothing else.
//!
//! Each test is a TABLE whose rows are windows built out of the instants the served
//! ledger actually holds — each published bound of the record set, one second inside each,
//! one second past each, a single-instant window, each bound alone, and no window at all —
//! crossed with every network each listing serves. The expectation for a row is computed
//! from the unwindowed listing the same operation returns, never from a constant copied
//! here.
//!
//! The harness (`Emulator`) is the one `chain_query_slice.rs` builds, so this target has
//! a single front door for every suite.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::chain_query_slice::{drain, Emulator, GENERIC_VALIDATION_MESSAGE};

/// Epoch seconds out of a response member, whichever JSON number form it took.
fn secs(case: &str, value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|secs| secs as i64))
        .unwrap_or_else(|| panic!("case {case}: expected epoch seconds, got {value:?}"))
}

/// The instant a `ListTransactions` item is placed in the window by: `transactionTimestamp`
/// is "the time when the transaction occurred".
fn transaction_instant(case: &str, item: &Value) -> i64 {
    secs(case, &item["transactionTimestamp"])
}

/// The instant a `ListFilteredTransactionEvents` item is placed in the window by: the
/// event's own `blockchainInstant`, whose `time` is the instant it names.
fn event_instant(case: &str, item: &Value) -> i64 {
    secs(case, &item["blockchainInstant"]["time"])
}

/// A `ListTransactions` request for every transaction of one address.
///
/// It names both `ConfirmationStatus` values because `ConfirmationStatusFilter$include` is
/// the opt-in to results that have not reached finality, so the window under test is the
/// only thing narrowing the listing.
fn list_transactions_body(network: &str, address: &str) -> Value {
    json!({
        "network": network,
        "address": address,
        "confirmationStatusFilter": {"include": ["FINAL", "NONFINAL"]},
    })
}

/// A `ListFilteredTransactionEvents` request for every event sent to one address; see
/// [`list_transactions_body`] for the finality filter.
fn filtered_events_body(network: &str, address: &str) -> Value {
    json!({
        "network": network,
        "addressIdentifierFilter": {"transactionEventToAddress": [address]},
        "confirmationStatusFilter": {"include": ["FINAL", "NONFINAL"]},
    })
}

/// One listing a request can narrow by an instant window.
struct Windowed {
    /// The operation, for a case label.
    name: &'static str,
    /// Its route (`@http(uri:)`).
    route: &'static str,
    /// The response member carrying the items (`@paginated(items:)`).
    items: &'static str,
    /// The `ValidationExceptionField$name` its window refusal is reported under: the
    /// member the rule is stated about, or the shape both members target when the rule is
    /// about the pair itself.
    field: &'static str,
    /// The dotted path of the member carrying the start of the window.
    from_member: &'static str,
    /// The dotted path of the member carrying the end of the window.
    to_member: &'static str,
    /// Whether the operation serves only the Bitcoin networks — `ListFilteredTransactionEvents`
    /// is documented "This operation is only supported on the Bitcoin networks".
    bitcoin_only: bool,
    /// A request for the whole listing of one (network, address).
    base: fn(&str, &str) -> Value,
    /// The instant one of its items is placed in the window by.
    instant: fn(&str, &Value) -> i64,
}

/// Every listing a request narrows by an instant window. Both are driven through the same
/// rows below, which is what pins the rule rather than one operation's spelling of it.
const WINDOWED: [Windowed; 2] = [
    Windowed {
        name: "ListTransactions",
        route: "/list-transactions",
        items: "transactions",
        field: "BlockchainInstant",
        from_member: "fromBlockchainInstant",
        to_member: "toBlockchainInstant",
        bitcoin_only: false,
        base: list_transactions_body,
        instant: transaction_instant,
    },
    Windowed {
        name: "ListFilteredTransactionEvents",
        route: "/list-filtered-transaction-events",
        items: "events",
        field: "timeFilter.from",
        from_member: "timeFilter.from",
        to_member: "timeFilter.to",
        bitcoin_only: true,
        base: filtered_events_body,
        instant: event_instant,
    },
];

/// Put `{"time": secs}` at the dotted member path `path` of a request body, creating the
/// containers on the way — so a window bound is written at exactly the path the refusal
/// below names.
fn set_instant(body: &mut Value, path: &str, instant: i64) {
    let mut cursor = body;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cursor[part] = json!({"time": instant});
            return;
        }
        if !cursor[part].is_object() {
            cursor[part] = json!({});
        }
        cursor = &mut cursor[part];
    }
}

impl Windowed {
    /// `base` narrowed to the window `from`..`to`, each bound written only where the row
    /// carries it.
    fn request(&self, base: &Value, from: Option<i64>, to: Option<i64>) -> Value {
        let mut body = base.clone();
        if let Some(from) = from {
            set_instant(&mut body, self.from_member, from);
        }
        if let Some(to) = to {
            set_instant(&mut body, self.to_member, to);
        }
        body
    }

    /// What a `fieldList` entry says about a window that ends before it starts.
    ///
    /// ONE sentence for every listing that takes a window, differing only in the pair of
    /// member paths it names. Written out here rather than borrowed from the
    /// implementation under test.
    fn refusal(&self) -> String {
        format!(
            "The 'time' field in {} must be less than or equal to the 'time' field in {}.",
            self.from_member, self.to_member
        )
    }

    /// Whether this listing serves `network`.
    ///
    /// Restated from the model's `QueryNetwork` value names rather than borrowed from the
    /// implementation: the two Bitcoin networks are the ones whose names start with
    /// `BITCOIN_`.
    fn serves(&self, network: &str) -> bool {
        !self.bitcoin_only || network.starts_with("BITCOIN_")
    }
}

/// The whole listing of one (network, address) — every page of it — and the instant of
/// each item, in the order the service reported them.
async fn whole_listing(
    emulator: &Emulator,
    listing: &Windowed,
    case: &str,
    base: &Value,
) -> (Vec<Value>, Vec<i64>) {
    let items = drain(emulator, listing.route, listing.items, base, None).await;
    let instants = items
        .iter()
        .map(|item| (listing.instant)(case, item))
        .collect();
    (items, instants)
}

#[tokio::test]
async fn every_windowed_listing_keeps_exactly_the_records_inside_its_inclusive_window() {
    let emulator = Emulator::new();

    for listing in &WINDOWED {
        for network in emulator.fixture() {
            if !listing.serves(&network.network) {
                continue;
            }
            // The busiest address of the ledger, so a window has records to narrow at
            // every bound.
            let address = network.busiest_address();
            let case = format!("{}/{}/{address}", listing.name, network.network);
            let base = (listing.base)(&network.network, &address);
            let (whole, instants) = whole_listing(&emulator, listing, &case, &base).await;
            assert!(
                !whole.is_empty(),
                "case {case}: the busiest address must hold records to window, got none"
            );

            let distinct: BTreeSet<i64> = instants.iter().copied().collect();
            let earliest = *distinct
                .iter()
                .next()
                .unwrap_or_else(|| panic!("case {case}: no instant to bound"));
            let latest = *distinct
                .iter()
                .next_back()
                .unwrap_or_else(|| panic!("case {case}: no instant to bound"));
            let middle = *distinct
                .iter()
                .nth(distinct.len() / 2)
                .unwrap_or_else(|| panic!("case {case}: no instant to bound"));

            // Rows: no window, the whole span, each of its bounds alone, a single-instant
            // window, a span wider than the ledger's, and one second past each bound.
            let mut rows: Vec<(String, Option<i64>, Option<i64>)> = vec![
                ("no window".to_string(), None, None),
                ("the whole span".to_string(), Some(earliest), Some(latest)),
                ("the start bound alone".to_string(), Some(earliest), None),
                ("the end bound alone".to_string(), None, Some(latest)),
                (
                    format!("the single instant {middle}"),
                    Some(middle),
                    Some(middle),
                ),
                (
                    "wider than the ledger".to_string(),
                    Some(earliest - 1),
                    Some(latest + 1),
                ),
                (
                    "one second past the end".to_string(),
                    Some(latest + 1),
                    None,
                ),
                (
                    "one second before the start".to_string(),
                    None,
                    Some(earliest - 1),
                ),
                (
                    "the start bound to the middle".to_string(),
                    Some(earliest),
                    Some(middle),
                ),
            ];
            // One second inside each bound: a row only where the span is wide enough for
            // it to still be a forwards window.
            if latest - earliest >= 2 {
                rows.push((
                    "one second inside each bound".to_string(),
                    Some(earliest + 1),
                    Some(latest - 1),
                ));
            }

            for (label, from, to) in rows {
                let want: Vec<Value> = whole
                    .iter()
                    .filter(|item| {
                        let instant = (listing.instant)(&case, item);
                        from.is_none_or(|earliest| instant >= earliest)
                            && to.is_none_or(|latest| instant <= latest)
                    })
                    .cloned()
                    .collect();
                let body = listing.request(&base, from, to);
                let got = drain(&emulator, listing.route, listing.items, &body, None).await;
                assert_eq!(
                    got,
                    want,
                    "case {case} {label} ({from:?}..{to:?}): expected {} items {want:?}, got {} \
                     items {got:?}",
                    want.len(),
                    got.len()
                );
            }

            // The bounds are INCLUSIVE: a record exactly ON either bound is listed, and
            // moving that bound one second over it drops the record.
            for bound in [earliest, middle, latest] {
                let on = drain(
                    &emulator,
                    listing.route,
                    listing.items,
                    &listing.request(&base, Some(bound), Some(bound)),
                    None,
                )
                .await;
                assert!(
                    !on.is_empty(),
                    "case {case} bound {bound}: a window of exactly that instant must list \
                     the records at it, got none"
                );
                for item in &on {
                    let instant = (listing.instant)(&case, item);
                    assert_eq!(
                        instant, bound,
                        "case {case} bound {bound}: expected only records at that instant, got \
                         {instant} in {item:?}"
                    );
                }

                for (label, from, to) in [
                    ("one second after the start bound", Some(bound + 1), None),
                    ("one second before the end bound", None, Some(bound - 1)),
                ] {
                    let past = drain(
                        &emulator,
                        listing.route,
                        listing.items,
                        &listing.request(&base, from, to),
                        None,
                    )
                    .await;
                    for item in &past {
                        let instant = (listing.instant)(&case, item);
                        assert_ne!(
                            instant, bound,
                            "case {case} bound {bound} {label}: a record at the excluded \
                             instant was listed, got {item:?}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn every_windowed_listing_refuses_a_window_that_ends_before_it_starts() {
    let emulator = Emulator::new();

    for listing in &WINDOWED {
        for network in emulator.fixture() {
            if !listing.serves(&network.network) {
                continue;
            }
            let address = network.busiest_address();
            let case = format!("{}/{}/{address}", listing.name, network.network);
            let base = (listing.base)(&network.network, &address);
            let (_, instants) = whole_listing(&emulator, listing, &case, &base).await;
            let distinct: BTreeSet<i64> = instants.iter().copied().collect();
            let earliest = *distinct
                .iter()
                .next()
                .unwrap_or_else(|| panic!("case {case}: no instant to bound"));
            let latest = *distinct
                .iter()
                .next_back()
                .unwrap_or_else(|| panic!("case {case}: no instant to bound"));

            // Rows: inverted by one second, the ledger's own span reversed, a pair far
            // apart, and one whose bounds are both outside the ledger — the refusal is
            // about the ORDER of the bounds, not about what they would have selected.
            let rows: Vec<(String, i64, i64)> = vec![
                ("inverted by one second".to_string(), earliest + 1, earliest),
                ("the ledger's span reversed".to_string(), latest, earliest),
                ("far apart".to_string(), 2_000_000_000, 1),
                (
                    "both bounds past the ledger".to_string(),
                    latest + 2,
                    latest + 1,
                ),
            ];

            for (label, from, to) in rows {
                if from <= to {
                    continue;
                }
                let body = listing.request(&base, Some(from), Some(to));
                let reply = emulator
                    .fails(listing.route, &body, 400, "ValidationException")
                    .await;
                assert_eq!(
                    reply["message"].as_str(),
                    Some(GENERIC_VALIDATION_MESSAGE),
                    "case {case} {label} ({from}..{to}): expected the generic refusal message, \
                     got {reply:?}"
                );
                let fields = reply["fieldList"].as_array().cloned().unwrap_or_default();
                let entry = json!({"name": listing.field, "message": listing.refusal()});
                assert!(
                    fields.contains(&entry),
                    "case {case} {label} ({from}..{to}): expected the fieldList to carry \
                     {entry}, got {fields:?}"
                );
            }

            // A FORWARDS window that selects nothing is answered, not refused: the two
            // halves of the rule are separate.
            let selects_nothing = listing.request(&base, Some(latest + 1), Some(latest + 2));
            let got = drain(
                &emulator,
                listing.route,
                listing.items,
                &selects_nothing,
                None,
            )
            .await;
            assert!(
                got.is_empty(),
                "case {case}: a window past the ledger's last record lists nothing, got {got:?}"
            );
        }
    }
}

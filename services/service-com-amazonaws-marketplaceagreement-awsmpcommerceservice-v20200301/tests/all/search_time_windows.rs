// SPDX-License-Identifier: CC0-1.0

//! A `SearchAgreements` filter list that bounds ONE stored instant from both
//! sides, end to end over HTTP.
//!
//! The rule under test is the one `time_windows.rs` pins over the listings whose
//! INPUT MEMBERS carry such a pair, here where the pair is stated as two FILTER
//! NAMES instead: an exclusive lower bound and an exclusive upper bound on one
//! stored instant span no instant at all when the lower one sits at or after the
//! upper one, so the pair states a period rather than selecting on one and is
//! refused with HTTP 400 `ValidationException` carrying the reason
//! `ValidationExceptionReason` publishes for a period, naming both bounds. A
//! window that spans any instant stays a selector: it is served, and an empty
//! page is a legitimate answer.
//!
//! Every pair of filter names `SearchAgreementsInput$filters` documents for one
//! stored instant is a row — "`BeforeEndTime` – A date used to filter agreements
//! with a date before the `endTime` of an agreement", and the same for
//! `startTime` and `lastUpdateTime` (`smithy_query(query="doc",
//! shape="SearchAgreementsInput", member="filters")`), which also expects both
//! bounds at once ("To filter by `EndTime`, you can use `BeforeEndTime`,
//! `AfterEndTime`, or both"). The rows differ in what the rule generalises over:
//! which stored instant the pair bounds, how far the wrong way round the bounds
//! are (a year, a second, a millisecond, and the degenerate pair at the same
//! instant), where the window sits relative to the stored agreement (wholly
//! before, straddling, wholly after), where in the filter list the two bounds
//! sit, and what the store holds — a refusal checked only while scanning would
//! answer an empty page instead.
//!
//! The harness (`emulator`, `search`, `agreement_over`, `upfront_term`,
//! `agreement_type_filter`) is the one `agreement_lifecycle` owns, the instant
//! formatter is the one `agreement_end_dates` owns, and the reader of a
//! refusal's named members is the one `time_windows` owns — the suite that pins
//! this same rule where the pair arrives as input members.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_end_dates::filter_instant;
use crate::agreement_lifecycle::{
    agreement_over, agreement_type_filter, emulator, search, upfront_term,
};
use crate::time_windows::named_members;

/// The reason a period no instant can fall in is reported under — the one value
/// `ValidationExceptionReason` publishes about a period
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`), spelled
/// out here rather than imported so a handler reporting another one fails.
const PERIOD_REASON: &str = "INVALID_INVOICE_ADJUSTMENT_PERIOD";

/// The sentence that reason states, as this service words every refusal of a
/// member it names.
const PERIOD_MESSAGE: &str = "Provided invoice adjustment period is invalid.";

/// Seconds in a day and in a year, for the windows the rows span.
const DAY: f64 = 86_400.0;
const YEAR: f64 = 365.0 * DAY;

/// One stored instant a `SearchAgreements` filter list can bound from both
/// sides: the two filter names that bound it, and the member of
/// `AgreementViewSummary` that reports it.
struct SearchWindow {
    /// The lower-bound filter name.
    after: &'static str,
    /// The upper-bound filter name.
    before: &'static str,
    /// The summary member carrying the instant the pair bounds, so a row bounds
    /// an instant the store really holds.
    reports: &'static str,
}

/// SOURCE: the `Before…`/`After…` filter names `SearchAgreementsInput$filters`
/// documents, paired by the stored instant each pair bounds.
const WINDOWS: &[SearchWindow] = &[
    SearchWindow {
        after: "AfterEndTime",
        before: "BeforeEndTime",
        reports: "endTime",
    },
    SearchWindow {
        after: "AfterStartTime",
        before: "BeforeStartTime",
        reports: "startTime",
    },
    SearchWindow {
        after: "AfterLastUpdateTime",
        before: "BeforeLastUpdateTime",
        reports: "lastUpdateTime",
    },
];

/// One filter naming `name` and the single value `FilterValueList` admits.
fn filter(name: &str, value: &str) -> Value {
    json!({"name": name, "values": [value]})
}

/// The dotted request path of the value the `position`-th filter (1-based)
/// carried, which is how this service names a member of a list element.
fn value_path(position: usize) -> String {
    format!("filters.{position}.member.values.1.member")
}

/// Where the two bounds and the required filter sit in one request's filter
/// list. The refusal names each bound by the position of the filter that carried
/// it, so a row says WHERE the pair sat — and the pair is the same window
/// wherever it sits.
const ORDERS: &[(&str, [&str; 3])] = &[
    ("the pair first, lower bound before upper", ["after", "before", "type"]),
    ("the pair first, upper bound before lower", ["before", "after", "type"]),
    ("the required filter first, then the pair", ["type", "after", "before"]),
];

/// The filter list one order states, and the 1-based positions the lower and the
/// upper bound ended up at.
fn laid_out(
    window: &SearchWindow,
    order: &[&str; 3],
    after: &str,
    before: &str,
) -> (Value, usize, usize) {
    let mut filters = Vec::with_capacity(order.len());
    let mut after_at = 0;
    let mut before_at = 0;
    for role in order {
        match *role {
            "after" => {
                filters.push(filter(window.after, after));
                after_at = filters.len();
            }
            "before" => {
                filters.push(filter(window.before, before));
                before_at = filters.len();
            }
            _ => filters.push(agreement_type_filter()),
        }
    }
    (Value::Array(filters), after_at, before_at)
}

/// A store holding one agreement whose terms commit a duration, so every instant
/// the three pairs bound has a stored value.
struct Held {
    router: MultiServiceRouter,
    agreement: String,
    summary: Value,
}

async fn held() -> Held {
    let router = emulator();
    let agreement = agreement_over(
        &router,
        "ap-searchwindow",
        vec![upfront_term("cfg", "P12M", &[("Users", 1)])],
    )
    .await;
    let answer = search(
        &router,
        json!({"filters": [agreement_type_filter()]}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the fixture's own search must be served, got {} with {:?}",
        answer.status, answer.body
    );
    let summary = answer.body["agreementViewSummaries"]
        .as_array()
        .and_then(|summaries| summaries.first())
        .cloned()
        .unwrap_or_else(|| panic!("the search must report the fixture, got {:?}", answer.body));
    Held {
        router,
        agreement,
        summary,
    }
}

impl Held {
    /// The stored instant one pair bounds, read off the agreement's own summary.
    fn pivot(&self, window: &SearchWindow) -> f64 {
        self.summary[window.reports].as_f64().unwrap_or_else(|| {
            panic!(
                "the fixture must report {} to bound, got {:?}",
                window.reports, self.summary
            )
        })
    }
}

/// A pair of bounds the wrong way round, as offsets from the stored instant the
/// pair bounds: how far apart they are, and where the window sits relative to
/// that instant.
const INVERTED: &[(&str, f64, f64)] = &[
    ("the bounds a year the wrong way round", YEAR, -YEAR),
    ("a second the wrong way round", 1.0, -1.0),
    ("a millisecond the wrong way round", 0.001, -0.001),
    ("both bounds at the same instant", 0.0, 0.0),
    ("a window wholly before the agreement, inverted", -DAY, -2.0 * DAY),
    ("a window wholly after the agreement, inverted", 2.0 * DAY, DAY),
];

/// A pair of filter names the wrong way round is refused as a malformed period,
/// whatever the store holds, wherever the window sits and wherever in the filter
/// list the two bounds sit.
#[tokio::test]
async fn an_inverted_search_window_is_refused_as_a_malformed_period() {
    let held = held().await;
    let empty = emulator();
    // The second store holds no agreement at all, so a period checked only while
    // scanning would answer an empty page here.
    let stores: Vec<(&str, &MultiServiceRouter)> = vec![
        ("a store holding an agreement", &held.router),
        ("an empty store", &empty),
    ];

    for window in WINDOWS {
        let pivot = held.pivot(window);
        for (store, router) in &stores {
            for (margin, after, before) in INVERTED {
                for (order, roles) in ORDERS {
                    let case =
                        format!("{} {margin}, {order} ({store})", window.after);
                    let (filters, after_at, before_at) = laid_out(
                        window,
                        roles,
                        &filter_instant(pivot + after),
                        &filter_instant(pivot + before),
                    );
                    let answer = search(router, json!({"filters": filters})).await;
                    assert_eq!(
                        answer.status, 400,
                        "case {case:?}: expected HTTP 400, got {} with {:?}",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.error_code(),
                        "ValidationException",
                        "case {case:?}: expected ValidationException, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.reason(),
                        PERIOD_REASON,
                        "case {case:?}: expected reason {PERIOD_REASON}, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.str_at("/message"),
                        PERIOD_MESSAGE,
                        "case {case:?}: expected {PERIOD_MESSAGE:?}, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        named_members(&answer),
                        vec![value_path(after_at), value_path(before_at)],
                        "case {case:?}: the refusal names both bounds that stated the period, \
                         got {:?}",
                        answer.body
                    );
                    assert!(
                        answer.body.get("agreementViewSummaries").is_none(),
                        "case {case:?}: a refused search reports nothing, got {:?}",
                        answer.body
                    );
                }
            }
        }
    }
}

/// A window that spans any instant at all stays a selector: the search is
/// served, and it reports exactly the agreements whose stored instant falls
/// inside the window.
///
/// The rows differ along how wide the window is, which of the two bounds the
/// search names, and whether the stored instant falls inside it — so the refusal
/// above is shown to be of the pair that spans nothing, not of the filter names.
#[tokio::test]
async fn a_search_window_that_spans_an_instant_is_served() {
    let held = held().await;
    // (case, lower bound, upper bound, whether the agreement is reported)
    let cases: &[(&str, Option<f64>, Option<f64>, bool)] = &[
        ("neither bound", None, None, true),
        ("only the lower bound, below the instant", Some(-1.0), None, true),
        ("only the upper bound, above it", None, Some(1.0), true),
        ("a window of two seconds around it", Some(-1.0), Some(1.0), true),
        ("a window of two days around it", Some(-DAY), Some(DAY), true),
        ("a window of two years around it", Some(-YEAR), Some(YEAR), true),
        (
            "a window wholly before it selects nothing",
            Some(-2.0 * DAY),
            Some(-DAY),
            false,
        ),
        (
            "a window wholly after it selects nothing",
            Some(DAY),
            Some(2.0 * DAY),
            false,
        ),
    ];

    for window in WINDOWS {
        let pivot = held.pivot(window);
        for (case, after, before, selected) in cases {
            let case = format!("{} {case}", window.after);
            let mut filters = vec![agreement_type_filter()];
            if let Some(after) = after {
                filters.push(filter(window.after, &filter_instant(pivot + after)));
            }
            if let Some(before) = before {
                filters.push(filter(window.before, &filter_instant(pivot + before)));
            }
            let answer = search(&held.router, json!({"filters": filters})).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a window that spans an instant must be served, got {} with {:?}",
                answer.status, answer.body
            );
            let reported: Vec<String> = answer.body["agreementViewSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a served search answers a page, got {:?}",
                        answer.body
                    )
                })
                .iter()
                .map(|summary| {
                    summary["agreementId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                reported.contains(&held.agreement),
                *selected,
                "case {case:?}: expected the agreement reported={selected}, got {reported:?}"
            );
        }
    }
}

/// One malformed period is refused even beside well-formed windows on the other
/// instants: the rule is about the pair that spans nothing, and the refusal names
/// that pair's own bounds.
#[tokio::test]
async fn one_inverted_pair_is_refused_beside_well_formed_windows() {
    let held = held().await;

    for inverted in WINDOWS {
        let case = format!("only the {} window inverted", inverted.after);
        let mut filters = vec![agreement_type_filter()];
        let mut after_at = 0;
        let mut before_at = 0;
        for window in WINDOWS {
            let pivot = held.pivot(window);
            // The inverted pair's bounds are a year the wrong way round; every
            // other pair spans two years around the instant it bounds.
            let is_inverted = window.after == inverted.after;
            let (after, before) = if is_inverted {
                (pivot + YEAR, pivot - YEAR)
            } else {
                (pivot - YEAR, pivot + YEAR)
            };
            filters.push(filter(window.after, &filter_instant(after)));
            let at_after = filters.len();
            filters.push(filter(window.before, &filter_instant(before)));
            let at_before = filters.len();
            if is_inverted {
                after_at = at_after;
                before_at = at_before;
            }
        }

        let answer = search(&held.router, json!({"filters": filters})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.reason(),
            PERIOD_REASON,
            "case {case:?}: expected reason {PERIOD_REASON}, got {:?}",
            answer.body
        );
        assert_eq!(
            named_members(&answer),
            vec![value_path(after_at), value_path(before_at)],
            "case {case:?}: the refusal names the bounds of the pair that spans nothing, got {:?}",
            answer.body
        );
    }

    // Every pair well formed at once is served: the refusals above are of the
    // inverted pair, not of carrying three windows.
    let mut filters = vec![agreement_type_filter()];
    for window in WINDOWS {
        let pivot = held.pivot(window);
        filters.push(filter(window.after, &filter_instant(pivot - YEAR)));
        filters.push(filter(window.before, &filter_instant(pivot + YEAR)));
    }
    let answer = search(&held.router, json!({"filters": filters})).await;
    assert_eq!(
        answer.status, 200,
        "three well-formed windows at once must be served, got {} with {:?}",
        answer.status, answer.body
    );
    let reported: Vec<String> = answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        reported.contains(&held.agreement),
        "every instant of the agreement falls inside its own window, got {reported:?}"
    );
}

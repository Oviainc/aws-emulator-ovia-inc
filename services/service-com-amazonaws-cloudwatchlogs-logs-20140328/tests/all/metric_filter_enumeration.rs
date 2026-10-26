// SPDX-License-Identifier: CC0-1.0

//! What `DescribeMetricFilters` ENUMERATES, and what `DeleteMetricFilter` takes out of
//! that enumeration.
//!
//! The rules under test:
//!
//! 1. `DescribeMetricFilters` reports the metric filters the store holds, "ASCII-sorted by
//!    filter name", with the log group breaking a tie between two groups' filters of the
//!    same name — so the order is TOTAL and a page that resumes in it neither repeats nor
//!    skips a filter at any page size. A request that names a log group reports that
//!    group's filters; one that names none reports the account's, across every group.
//! 2. `filterNamePrefix` is used "only if you also include the logGroupName parameter in
//!    your request", so the same prefix that selects within one group selects nothing away
//!    from an account-wide enumeration.
//! 3. `DeleteMetricFilter` "deletes the specified metric filter" — the one the log group it
//!    names holds under that name — so the enumeration and the group's derived
//!    `metricFilterCount` stop counting it, while a filter of the SAME NAME on another log
//!    group is untouched: a metric filter is identified by the group it is associated with
//!    together with its own name.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span what the rules generalise over: the page size, the prefix, which
//! selectors a request names, and which of a set of filters sharing names across groups is
//! removed. The sibling suite `metric_filters.rs` is the per-filter table (what one put
//! stores and reports back); this one is about the SET.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::Logs;

/// The published bounds of `DescribeMetricFilters$limit`. Source: the `DescribeLimit`
/// shape, `range 1..50`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 50;

/// The namespace every filter stored here emits its metric in. One namespace keeps the
/// cases about the ENUMERATION rather than about metric identity, which
/// `metric_filters.rs` tables.
const NAMESPACE: &str = "Custom/MetricFilterEnumeration";

/// One stored metric filter: the log group it is associated with and its own name, which
/// together identify it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stored {
    group: &'static str,
    filter: &'static str,
}

/// The filters every case here enumerates over, in the order they are PUT — deliberately
/// neither the reported order nor grouped by log group, so an answer that echoes the
/// insertion order cannot pass. Three log groups share the filter name `shared`, which is
/// what makes the tie-break observable, and the groups' own ASCII order
/// (`mfe-one` < `mfe-three` < `mfe-two`) differs from the order they are created in.
const STORED: [Stored; 7] = [
    Stored {
        group: "mfe-two",
        filter: "shared",
    },
    Stored {
        group: "mfe-one",
        filter: "zeta",
    },
    Stored {
        group: "mfe-three",
        filter: "shared",
    },
    Stored {
        group: "mfe-one",
        filter: "alpha",
    },
    Stored {
        group: "mfe-two",
        filter: "beta",
    },
    Stored {
        group: "mfe-one",
        filter: "shared",
    },
    Stored {
        group: "mfe-three",
        filter: "mid",
    },
];

impl Stored {
    /// The `PutMetricFilter` request that stores this filter.
    fn put_request(self) -> Value {
        json!({
            "logGroupName": self.group,
            "filterName": self.filter,
            "filterPattern": format!("\"{}\"", self.filter),
            "metricTransformations": [{
                "metricName": self.filter,
                "metricNamespace": NAMESPACE,
                "metricValue": "1",
            }],
        })
    }

    /// This filter as the enumeration reports it: its name and the log group it is
    /// associated with.
    fn reported(self) -> (String, String) {
        (self.filter.to_string(), self.group.to_string())
    }
}

/// The log groups the stored filters are associated with.
fn groups() -> BTreeSet<&'static str> {
    STORED.iter().map(|stored| stored.group).collect()
}

/// Create every log group and store every filter of [`STORED`].
async fn store_the_filters(logs: &Logs) {
    for group in groups() {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    for stored in STORED {
        logs.ok("PutMetricFilter", &stored.put_request()).await;
    }
}

/// The order the service must report a set of filters in: ASCII by filter name, with the
/// log group breaking a tie between two groups' filters of the same name.
fn expected_order(stored: &[Stored]) -> Vec<(String, String)> {
    let mut order: Vec<(String, String)> = stored.iter().map(|one| one.reported()).collect();
    order.sort();
    order
}

/// Every filter one `DescribeMetricFilters` page reports, as `(filterName,
/// logGroupName)`, in the order the page reports them.
fn reported(page: &Value) -> Vec<(String, String)> {
    page.get("metricFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeMetricFilters must return metricFilters, got {page}"))
        .iter()
        .map(|filter| {
            let member = |key: &str| {
                filter
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("a reported metric filter carries {key}, got {filter}")
                    })
                    .to_string()
            };
            (member("filterName"), member("logGroupName"))
        })
        .collect()
}

/// Walk every page of one `DescribeMetricFilters` request at `limit`, asserting each page
/// holds at most that many filters, and answer everything reported in order.
async fn walk(logs: &Logs, case: &str, request: &Value, limit: i64) -> Vec<(String, String)> {
    let page_size = usize::try_from(limit).expect("the page size is positive");
    let mut found: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..64 {
        let mut body = request.clone();
        body["limit"] = json!(limit);
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let page = logs.ok("DescribeMetricFilters", &body).await;
        let on_this_page = reported(&page);
        assert!(
            on_this_page.len() <= page_size,
            "case {case:?}: page {page_index} must hold at most {page_size} filters, got \
             {on_this_page:?}"
        );
        found.extend(on_this_page);
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return found,
        }
    }
    panic!("case {case:?}: pagination did not terminate, collected {found:?}");
}

/// One page of a request, with no `limit` of its own: what a caller that pages at the
/// service's own default sees in one call.
async fn one_page(logs: &Logs, request: &Value) -> Vec<(String, String)> {
    reported(&logs.ok("DescribeMetricFilters", request).await)
}

/// The enumeration is ASCII-sorted by filter name with the log group breaking a tie, both
/// account-wide and within one log group, and every page size reports that one order
/// exactly once.
#[tokio::test]
async fn the_order_is_total_and_every_page_size_reports_it_once() {
    let logs = Logs::new();
    store_the_filters(&logs).await;

    let everything = expected_order(&STORED);
    assert_eq!(
        one_page(&logs, &json!({})).await,
        everything,
        "an account-wide enumeration reports every group's filters in one total order"
    );

    // Page sizes spanning the published range: one filter at a time, several at a time, a
    // size that divides the set evenly, and both published bounds of `limit`.
    for limit in [LIMIT_MIN, 2, 3, 6, 7, LIMIT_MAX] {
        let case = format!("account-wide at limit {limit}");
        assert_eq!(
            walk(&logs, &case, &json!({}), limit).await,
            everything,
            "case {case:?}: paging reports the one order exactly once, expected {everything:?}"
        );

        for group in groups() {
            let case = format!("{group} at limit {limit}");
            let of_group: Vec<Stored> = STORED
                .iter()
                .copied()
                .filter(|stored| stored.group == group)
                .collect();
            let expected = expected_order(&of_group);
            assert_eq!(
                walk(&logs, &case, &json!({"logGroupName": group}), limit).await,
                expected,
                "case {case:?}: a request naming a log group reports that group's filters, \
                 expected {expected:?}"
            );
        }
    }
}

/// `filterNamePrefix` selects within the log group a request names, and is used only when
/// the request names one: the same prefix leaves an account-wide enumeration whole.
#[tokio::test]
async fn a_prefix_selects_only_where_a_log_group_is_named() {
    let logs = Logs::new();
    store_the_filters(&logs).await;

    let everything = expected_order(&STORED);
    // Prefixes spanning the class: one that selects a single filter, one that selects
    // several (the shared name), one that is a strict prefix of several names, one that is
    // a whole filter name, and one nothing starts with.
    for prefix in ["alpha", "s", "shared", "z", "no-such-prefix"] {
        for group in groups() {
            let case = format!("prefix {prefix:?} on {group}");
            let selected: Vec<Stored> = STORED
                .iter()
                .copied()
                .filter(|stored| stored.group == group && stored.filter.starts_with(prefix))
                .collect();
            let expected = expected_order(&selected);
            assert_eq!(
                one_page(
                    &logs,
                    &json!({"logGroupName": group, "filterNamePrefix": prefix}),
                )
                .await,
                expected,
                "case {case:?}: a prefix keeps the group's filters that start with it, \
                 expected {expected:?}"
            );
        }

        let case = format!("prefix {prefix:?} with no log group");
        assert_eq!(
            one_page(&logs, &json!({"filterNamePrefix": prefix})).await,
            everything,
            "case {case:?}: a prefix is used only where the request also names a log group"
        );
        // The same holds page by page: the prefix changes neither the order nor the set a
        // continuation walks through.
        assert_eq!(
            walk(
                &logs,
                &case,
                &json!({"filterNamePrefix": prefix}),
                LIMIT_MIN,
            )
            .await,
            everything,
            "case {case:?}: paging an account-wide enumeration ignores the prefix too"
        );
    }
}

/// A deleted metric filter leaves the enumeration and its log group's
/// `metricFilterCount`, one of the same name on another group stays, and a delete that
/// names a filter or a log group the account does not hold is refused.
#[tokio::test]
async fn a_deleted_filter_leaves_the_enumeration() {
    let logs = Logs::new();
    store_the_filters(&logs).await;

    // Deleting in an order that is neither the put order nor the reported one, taking the
    // shared name off one group at a time so the surviving twins are observable.
    let removal_order = [
        Stored {
            group: "mfe-three",
            filter: "shared",
        },
        Stored {
            group: "mfe-one",
            filter: "alpha",
        },
        Stored {
            group: "mfe-two",
            filter: "shared",
        },
        Stored {
            group: "mfe-one",
            filter: "shared",
        },
        Stored {
            group: "mfe-one",
            filter: "zeta",
        },
        Stored {
            group: "mfe-three",
            filter: "mid",
        },
        Stored {
            group: "mfe-two",
            filter: "beta",
        },
    ];
    let mut remaining: Vec<Stored> = STORED.to_vec();

    for removed in removal_order {
        let case = format!("{} on {}", removed.filter, removed.group);
        let request = json!({"logGroupName": removed.group, "filterName": removed.filter});
        let body = logs.ok("DeleteMetricFilter", &request).await;
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {case:?}: a delete answers the modelled empty body, got {body}"
        );

        remaining.retain(|stored| *stored != removed);
        let expected = expected_order(&remaining);
        assert_eq!(
            one_page(&logs, &json!({})).await,
            expected,
            "case {case:?}: the account-wide enumeration stops reporting a deleted filter, \
             expected {expected:?}"
        );
        for group in groups() {
            let held: Vec<Stored> = remaining
                .iter()
                .copied()
                .filter(|stored| stored.group == group)
                .collect();
            let expected = expected_order(&held);
            assert_eq!(
                one_page(&logs, &json!({"logGroupName": group})).await,
                expected,
                "case {case:?}: {group} reports the filters it still holds, expected \
                 {expected:?}"
            );
            let described = logs
                .describe_group(group)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the log group {group} must exist"));
            assert_eq!(
                described.get("metricFilterCount").and_then(Value::as_i64),
                i64::try_from(held.len()).ok(),
                "case {case:?}: {group} counts the metric filters it holds, got {described}"
            );
        }

        // The same delete a second time names a filter the group no longer holds.
        let (code, message) = logs.refusal("DeleteMetricFilter", &request).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "ResourceNotFoundException",
                "The specified metric filter does not exist."
            ),
            "case {case:?}: deleting a filter the log group does not hold is refused"
        );
    }

    // A name one group holds is not a filter of another: the pair identifies the filter.
    logs.ok(
        "PutMetricFilter",
        &Stored {
            group: "mfe-one",
            filter: "alpha",
        }
        .put_request(),
    )
    .await;
    let (code, message) = logs
        .refusal(
            "DeleteMetricFilter",
            &json!({"logGroupName": "mfe-two", "filterName": "alpha"}),
        )
        .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "ResourceNotFoundException",
            "The specified metric filter does not exist."
        ),
        "a filter name another log group holds names no filter of this one"
    );
    assert_eq!(
        one_page(&logs, &json!({"logGroupName": "mfe-one"})).await,
        vec![("alpha".to_string(), "mfe-one".to_string())],
        "the filter of the group that does hold it survives a delete aimed at another group"
    );

    // A log group the account does not hold holds no filter to delete, and a group that
    // was deleted took its filters with it.
    logs.ok("DeleteLogGroup", &json!({"logGroupName": "mfe-one"}))
        .await;
    for group in ["mfe-one", "mfe-never-created"] {
        let (code, message) = logs
            .refusal(
                "DeleteMetricFilter",
                &json!({"logGroupName": group, "filterName": "alpha"}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "ResourceNotFoundException",
                "The specified log group does not exist."
            ),
            "case {group}: a log group that does not exist holds no metric filter"
        );
    }
}

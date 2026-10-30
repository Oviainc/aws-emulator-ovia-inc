// SPDX-License-Identifier: CC0-1.0

//! `ListMonitors` when the request names NO position in the listing's order.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) and the create body are the ones
//! `monitor_and_probe`/`list_monitors_pagination` already own. The rule under
//! test:
//!
//! * a pagination cursor is an opaque name for a POSITION in the listing's key
//!   order, and the empty string carries no key — so a `nextToken` the modelled
//!   length bound admits as empty names no position and is the same request as
//!   one carrying no cursor at all: the FIRST page, whatever page size, filter
//!   or stored set the listing is made with;
//! * only a NON-EMPTY token claims to be a cursor, so a non-empty value this
//!   service never vended is still refused with 400 `ValidationException`.

use serde_json::Value;

use super::list_monitors_pagination::create_body;
use super::monitor_and_probe::{create_monitor, error_code, list_monitors, page_names, router};

/// How one request can name no position in the `monitorName` order.
#[derive(Debug, Clone, Copy)]
enum NoPosition {
    /// `nextToken` absent from the query altogether.
    Absent,
    /// `nextToken=` — the empty value, which `PaginationToken`'s modelled
    /// `@length(min: 0)` (smithy_query shape=ListMonitorsInput) admits, so the
    /// boundary validator hands it to the operation.
    Empty,
    /// `nextToken` with no `=` at all, which a query string parses as that same
    /// empty value.
    Bare,
}

impl NoPosition {
    /// The query parameter this form contributes, empty for none at all.
    fn parameter(self) -> &'static str {
        match self {
            NoPosition::Absent => "",
            NoPosition::Empty => "nextToken=",
            NoPosition::Bare => "nextToken",
        }
    }
}

/// Every form a request has for naming no position.
const NO_POSITION: &[NoPosition] = &[NoPosition::Absent, NoPosition::Empty, NoPosition::Bare];

/// One listing: the stored monitors, the page it is asked for, and the first
/// page every "no position" form must be served.
#[derive(Debug)]
struct Case {
    case: &'static str,
    /// The monitors created before the listing, in creation order.
    stored: &'static [&'static str],
    /// Whether each monitor is created with a probe, which is what puts it in
    /// `PENDING` rather than `INACTIVE`.
    with_probes: bool,
    /// The `state` filter the listing is made with, if any.
    filter: Option<&'static str>,
    /// The `maxResults` the listing is made with, if any.
    max_results: Option<u32>,
    /// The monitors the first page must report, in order.
    first_page: &'static [&'static str],
    /// Whether that page leaves monitors owed to the caller, so it must vend a
    /// cursor.
    vends_cursor: bool,
}

/// The `ListMonitors` query for `case`, with `token` as its only cursor
/// parameter (empty for none).
fn page_query(case: &Case, token: &str) -> String {
    let mut params: Vec<String> = Vec::new();
    if let Some(max) = case.max_results {
        params.push(format!("maxResults={max}"));
    }
    if let Some(state) = case.filter {
        params.push(format!("state={state}"));
    }
    if !token.is_empty() {
        params.push(token.to_string());
    }
    if params.is_empty() {
        return String::new();
    }
    format!("?{}", params.join("&"))
}

/// The cursor a page vended, if any.
fn vended_cursor(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A request that names no position is served the first page — the empty
/// `nextToken` exactly as an absent one.
///
/// The table spans what the listing is made over: nothing stored, one monitor,
/// several (created out of name order), a page smaller than the set, the
/// model's own `maxResults` bounds, a filter that matches everything, one that
/// matches nothing, and a filtered listing of `PENDING` monitors. The reported
/// case (two monitors, `?nextToken=`) is one row.
#[tokio::test]
async fn a_request_that_names_no_position_is_served_the_first_page() {
    const CASES: &[Case] = &[
        Case {
            case: "nothing is stored",
            stored: &[],
            with_probes: false,
            filter: None,
            max_results: None,
            first_page: &[],
            vends_cursor: false,
        },
        Case {
            case: "one monitor is stored",
            stored: &["mon-a"],
            with_probes: false,
            filter: None,
            max_results: None,
            first_page: &["mon-a"],
            vends_cursor: false,
        },
        Case {
            case: "the whole set fits one page, created out of name order",
            stored: &["mon-c", "mon-a", "mon-b"],
            with_probes: false,
            filter: None,
            max_results: None,
            first_page: &["mon-a", "mon-b", "mon-c"],
            vends_cursor: false,
        },
        Case {
            case: "a one-at-a-time page smaller than the set",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            max_results: Some(1),
            first_page: &["mon-a"],
            vends_cursor: true,
        },
        Case {
            case: "a two-at-a-time page smaller than the set",
            stored: &["mon-a", "mon-b", "mon-c", "mon-d"],
            with_probes: false,
            filter: None,
            max_results: Some(2),
            first_page: &["mon-a", "mon-b"],
            vends_cursor: true,
        },
        // 25 is `MaxResults`' modelled maximum (smithy_query
        // shape=ListMonitorsInput: `maxResults` range 1..25).
        Case {
            case: "a page size at the modelled maximum",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            max_results: Some(25),
            first_page: &["mon-a", "mon-b", "mon-c"],
            vends_cursor: false,
        },
        Case {
            case: "a filter every stored monitor matches",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: Some("INACTIVE"),
            max_results: None,
            first_page: &["mon-a", "mon-b", "mon-c"],
            vends_cursor: false,
        },
        Case {
            case: "a filter no stored monitor matches",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: Some("ACTIVE"),
            max_results: None,
            first_page: &[],
            vends_cursor: false,
        },
        Case {
            case: "a filtered listing of monitors holding probes",
            stored: &["pend-a", "pend-b", "pend-c"],
            with_probes: true,
            filter: Some("PENDING"),
            max_results: Some(1),
            first_page: &["pend-a"],
            vends_cursor: true,
        },
    ];

    for case in CASES {
        let label = case.case;
        let router = router();
        for name in case.stored {
            let (status, created) =
                create_monitor(&router, &create_body(name, case.with_probes)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: setup CreateMonitor {name} must succeed, got {status} {created:?}"
            );
        }

        let mut served: Option<(Vec<String>, Option<String>)> = None;
        for form in NO_POSITION {
            let query = page_query(case, form.parameter());
            let (status, body) = list_monitors(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {label:?} / {form:?} ({query:?}): a request naming no position must be \
                 served the first page with 200, got {status} {body:?}"
            );
            let names = page_names(&body);
            assert_eq!(
                names, case.first_page,
                "case {label:?} / {form:?} ({query:?}): expected the first page {:?}, got {names:?}",
                case.first_page
            );
            let cursor = vended_cursor(&body);
            assert_eq!(
                cursor.is_some(),
                case.vends_cursor,
                "case {label:?} / {form:?} ({query:?}): expected a vended cursor \
                 ({}), got {cursor:?}",
                case.vends_cursor
            );
            match &served {
                None => served = Some((names, cursor)),
                Some(first) => assert_eq!(
                    &(names, cursor),
                    first,
                    "case {label:?} / {form:?} ({query:?}): must answer exactly what an \
                     absent nextToken answers, which was {first:?}"
                ),
            }
        }
    }
}

/// An empty `nextToken` names no position, so it does not ADVANCE a paginator:
/// it serves the first page again, while the cursor the page vended serves the
/// next one.
#[tokio::test]
async fn an_empty_next_token_does_not_advance_the_listing() {
    let router = router();
    for name in ["mon-a", "mon-b", "mon-c"] {
        let (status, created) = create_monitor(&router, &create_body(name, false)).await;
        assert_eq!(
            status, 200,
            "setup: CreateMonitor {name} must succeed, got {status} {created:?}"
        );
    }

    let (status, first) = list_monitors(&router, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "expected the first page with 200, got {status} {first:?}"
    );
    assert_eq!(
        page_names(&first),
        vec!["mon-a".to_string()],
        "expected the first page to report the first monitor, got {first:?}"
    );
    let cursor = vended_cursor(&first).expect("the first page of three monitors vends a cursor");

    let (status, again) = list_monitors(&router, "?maxResults=1&nextToken=").await;
    assert_eq!(
        status, 200,
        "an empty nextToken must be served the first page, got {status} {again:?}"
    );
    assert_eq!(
        page_names(&again),
        page_names(&first),
        "an empty nextToken names no position, so it must not advance the listing: \
         expected {:?}, got {again:?}",
        page_names(&first)
    );
    assert_eq!(
        vended_cursor(&again),
        Some(cursor.clone()),
        "an empty nextToken must vend the same cursor the first page did, got {again:?}"
    );

    let (status, second) = list_monitors(&router, &format!("?maxResults=1&nextToken={cursor}")).await;
    assert_eq!(
        status, 200,
        "a vended cursor must resume with 200, got {status} {second:?}"
    );
    assert_eq!(
        page_names(&second),
        vec!["mon-b".to_string()],
        "a vended cursor advances where an empty token does not: expected the second \
         monitor, got {second:?}"
    );
}

/// A NON-EMPTY `nextToken` still has to be a cursor this service vended: the
/// empty value is the only one that names no position.
///
/// The rows sit next to the empty string on purpose — a token that is only
/// whitespace, one that is not even the cursor encoding, and a cursor written
/// in a format this service does not mint — so admitting the empty value cannot
/// quietly admit them too.
#[tokio::test]
async fn a_non_empty_next_token_this_service_never_vended_is_still_refused() {
    let router = router();
    for name in ["mon-a", "mon-b"] {
        let (status, created) = create_monitor(&router, &create_body(name, false)).await;
        assert_eq!(
            status, 200,
            "setup: CreateMonitor {name} must succeed, got {status} {created:?}"
        );
    }

    // The last row is the encoding of `monitor-page/9:mon-a`: a cursor shaped
    // like one, marked with a format version this service does not vend.
    let other_version = hex::encode("monitor-page/9:mon-a");
    for (case, token) in [
        ("a single space", "%20".to_string()),
        ("whitespace only", "%20%20%20".to_string()),
        ("not the cursor encoding at all", "zz".to_string()),
        ("an odd number of encoding digits", "abc".to_string()),
        ("a cursor marked with another format", other_version),
    ] {
        let (status, refused) = list_monitors(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({token:?}): expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?} ({token:?}): expected ValidationException, got {refused:?}"
        );
    }

    let (status, listed) = list_monitors(&router, "?nextToken=").await;
    assert_eq!(
        status, 200,
        "a refused cursor changes nothing: expected 200, got {status} {listed:?}"
    );
    assert_eq!(
        page_names(&listed),
        vec!["mon-a".to_string(), "mon-b".to_string()],
        "a refused cursor changes nothing: expected both monitors, got {listed:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! How a `ListMonitors` cursor behaves when the result set SHIFTS between the
//! page that vended it and the resume that uses it.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` already owns. The rules
//! under test:
//!
//! * a cursor this service vended names a POSITION in the `monitorName` order,
//!   not a particular monitor: the resume starts at the first monitor whose
//!   name is at or after the cursor, so it answers 200 with the monitors still
//!   owed to the caller however the set moved — the cursor's own monitor
//!   deleted, re-created, or settled out of the `state` filter, monitors added
//!   or removed on either side of it — and never turns a vended cursor into a
//!   bad request;
//! * monitors sorting BEFORE the cursor are not listed again, monitors sorting
//!   after it are listed once, and a resume whose whole tail is gone is an
//!   empty final page with no `nextToken`;
//! * a `nextToken` this service never vended still names no page of the result
//!   set and is refused with 400 `ValidationException`, changing nothing.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, error_code, list_monitors, page_names, router, source_arn,
    PAST_ACTIVATION,
};

/// What happens to the stored set between the page that vends a cursor and the
/// resume that uses it.
#[derive(Debug, Clone, Copy)]
enum Shift {
    /// `DeleteMonitor` on one monitor.
    Delete(&'static str),
    /// `CreateMonitor` for another (probe-less, so `INACTIVE`) monitor.
    Create(&'static str),
    /// Wait until the clock has made every stored monitor's activation due, so
    /// a monitor listed as `PENDING` is `ACTIVE` by the resume.
    Settle,
}

/// One resume: the stored monitors, the first page's query, the shift, and the
/// monitors the resume still owes the caller.
#[derive(Debug)]
struct Case {
    case: &'static str,
    /// The monitors created before the first page, in creation order.
    stored: &'static [&'static str],
    /// Whether each stored monitor is created with a probe, which is what puts
    /// it in `PENDING` rather than `INACTIVE`.
    with_probes: bool,
    /// The `state` filter the listing is made with, if any.
    filter: Option<&'static str>,
    page_size: usize,
    /// What the first page must report, which is what fixes where the cursor
    /// was cut.
    first_page: &'static [&'static str],
    shifts: &'static [Shift],
    /// Every monitor the pages after the cursor must report, in order.
    remaining: &'static [&'static str],
}

/// The body that creates a monitor with one probe (so the monitor is stored
/// `PENDING`), or none (so it is stored `INACTIVE`).
fn create_body(name: &str, with_probes: bool) -> String {
    if with_probes {
        format!(
            r#"{{"monitorName":"{name}","probes":[{{"sourceArn":"{}","destination":"10.0.0.1",
                 "protocol":"ICMP","packetSize":56}}]}}"#,
            source_arn()
        )
    } else {
        format!(r#"{{"monitorName":"{name}"}}"#)
    }
}

/// The `ListMonitors` query for one page of `case`, resuming from `token` when
/// there is one.
fn page_query(case: &Case, token: Option<&str>) -> String {
    let mut query = format!("?maxResults={}", case.page_size);
    if let Some(state) = case.filter {
        query.push_str(&format!("&state={state}"));
    }
    if let Some(token) = token {
        query.push_str(&format!("&nextToken={token}"));
    }
    query
}

async fn apply(router: &MultiServiceRouter, case: &str, shift: Shift) {
    match shift {
        Shift::Delete(name) => {
            let (status, body) = call(router, "DELETE", &format!("/monitors/{name}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteMonitor {name} must succeed, got {status} {body:?}"
            );
        }
        Shift::Create(name) => {
            let (status, body) = create_monitor(router, &create_body(name, false)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateMonitor {name} must succeed, got {status} {body:?}"
            );
        }
        Shift::Settle => std::thread::sleep(PAST_ACTIVATION),
    }
}

/// A vended cursor resumes the listing with 200 across a mutation of the result
/// set.
///
/// The table spans the ways the set can move under a cursor — the boundary
/// monitor deleted, re-created or settled out of the filter, a monitor removed
/// or added before or after the cursor, the whole tail gone, nothing at all —
/// over both an unfiltered and a filtered listing and two page sizes. The
/// reported case (page 1, delete the monitor the cursor names, page 2) is one
/// row.
#[tokio::test]
async fn a_vended_next_token_resumes_across_a_mutation_of_the_result_set() {
    const CASES: &[Case] = &[
        Case {
            case: "the set is unchanged",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[],
            remaining: &["mon-b", "mon-c"],
        },
        Case {
            case: "the monitor the cursor names is deleted",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Delete("mon-b")],
            remaining: &["mon-c"],
        },
        Case {
            case: "the monitor the cursor names is deleted and re-created",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Delete("mon-b"), Shift::Create("mon-b")],
            remaining: &["mon-b", "mon-c"],
        },
        Case {
            case: "a monitor before the cursor is deleted",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Delete("mon-a")],
            remaining: &["mon-b", "mon-c"],
        },
        Case {
            case: "a monitor after the cursor is deleted",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Delete("mon-c")],
            remaining: &["mon-b"],
        },
        Case {
            case: "every monitor after the cursor is deleted",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Delete("mon-b"), Shift::Delete("mon-c")],
            remaining: &[],
        },
        Case {
            case: "a monitor is added before the cursor",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Create("mon-a0")],
            remaining: &["mon-b", "mon-c"],
        },
        Case {
            case: "a monitor is added after the cursor",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: None,
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Create("mon-bb")],
            remaining: &["mon-b", "mon-bb", "mon-c"],
        },
        Case {
            case: "a two-at-a-time listing whose whole tail is deleted",
            stored: &["mon-a", "mon-b", "mon-c", "mon-d"],
            with_probes: false,
            filter: None,
            page_size: 2,
            first_page: &["mon-a", "mon-b"],
            shifts: &[Shift::Delete("mon-c"), Shift::Delete("mon-d")],
            remaining: &[],
        },
        Case {
            case: "a two-at-a-time listing whose boundary monitor is deleted",
            stored: &["mon-a", "mon-b", "mon-c", "mon-d"],
            with_probes: false,
            filter: None,
            page_size: 2,
            first_page: &["mon-a", "mon-b"],
            shifts: &[Shift::Delete("mon-c")],
            remaining: &["mon-d"],
        },
        Case {
            case: "a filtered listing whose set is unchanged",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: Some("INACTIVE"),
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[],
            remaining: &["mon-b", "mon-c"],
        },
        Case {
            case: "a filtered listing whose boundary monitor is deleted",
            stored: &["mon-a", "mon-b", "mon-c"],
            with_probes: false,
            filter: Some("INACTIVE"),
            page_size: 1,
            first_page: &["mon-a"],
            shifts: &[Shift::Delete("mon-b")],
            remaining: &["mon-c"],
        },
        Case {
            case: "a filtered listing whose boundary monitor settles out of the filter",
            stored: &["pend-a", "pend-b"],
            with_probes: true,
            filter: Some("PENDING"),
            page_size: 1,
            first_page: &["pend-a"],
            shifts: &[Shift::Settle],
            remaining: &[],
        },
    ];

    for case in CASES {
        let label = case.case;
        let router = router();
        for name in case.stored {
            let (status, created) = create_monitor(&router, &create_body(name, case.with_probes))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: setup CreateMonitor {name} must succeed, got {status} {created:?}"
            );
        }

        let (status, first) = list_monitors(&router, &page_query(case, None)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListMonitors 200, got {status} {first:?}"
        );
        assert_eq!(
            page_names(&first),
            case.first_page,
            "case {label:?}: expected first page {:?}, got {first:?}",
            case.first_page
        );
        let mut token = first
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert!(
            token.is_some(),
            "case {label:?}: the first page must vend a cursor, got {first:?}"
        );

        for shift in case.shifts {
            apply(&router, label, *shift).await;
        }

        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0;
        while let Some(cursor) = token {
            let (status, body) = list_monitors(&router, &page_query(case, Some(&cursor))).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a cursor this service vended must resume with 200, \
                 got {status} {body:?}"
            );
            seen.extend(page_names(&body));
            pages += 1;
            assert!(
                pages <= case.stored.len() + 2,
                "case {label:?}: paging must terminate, saw {seen:?}"
            );
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        assert_eq!(
            seen, case.remaining,
            "case {label:?}: expected the resume to report {:?}, got {seen:?}",
            case.remaining
        );
    }
}

/// A `nextToken` this service never vended still names no page of the result
/// set: 400 `ValidationException`, and the listing itself is untouched.
///
/// The rows span the ways a string can fail to be a cursor this service minted:
/// text that is not even the token's encoding, text that is but carries nothing
/// this service wrote, and the bare encoding of a stored `monitorName` — which
/// is what keeps a caller from hand-rolling a cursor out of a name it read.
#[tokio::test]
async fn a_next_token_this_service_never_vended_is_refused() {
    let router = router();
    for name in ["mon-a", "mon-b"] {
        let (status, created) = create_monitor(&router, &create_body(name, false)).await;
        assert_eq!(
            status, 200,
            "setup: CreateMonitor {name} must succeed, got {status} {created:?}"
        );
    }

    for (case, token) in [
        ("text that is not the token encoding", "not-a-token"),
        ("the encoding of text nothing vended", "6e6f742d612d637572736f72"),
        ("the bare encoding of a stored monitorName", "6d6f6e2d62"),
    ] {
        let (status, refused) = list_monitors(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }

    let (status, listed) = list_monitors(&router, "").await;
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

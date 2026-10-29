// SPDX-License-Identifier: CC0-1.0

//! The page bound every read whose `MaxResults` declares its whole range but NO usable
//! `@default` holds to, over HTTP.
//!
//! The rule under test: such a member reaches its handler as an `Option`, so the read serves
//! the bound a request states — the centralized walk having already refused anything outside
//! the modelled range — and a request that states NO bound serves that operation's documented
//! default page, never a page of nothing. It is ONE rule, so it is asserted on all three reads
//! of this service that own such a member:
//!
//! * `ListAuditFindings` — `MaxResults` a BODY member, `range 1..10`, documented default 10
//!   ("If you omit this parameter, the default of 10 is used");
//! * `ListServiceLevelObjectiveExclusionWindows` — `MaxResults` query-bound, `range 1..10`;
//! * `ListServiceLevelObjectives` — `MaxResults` query-bound, `range 1..50`,
//!
//! the ranges from `smithy_query(query="operation",
//! operation="ListAuditFindings,ListServiceLevelObjectiveExclusionWindows,
//! ListServiceLevelObjectives")`. The rows therefore differ along where the model binds the
//! member, the modelled maximum, the default page and the kind of projection paged (findings
//! derived per request, one SLO's stored window set, the stored objectives).
//!
//! `defaulted_page_bound` pins the SIBLING rule, for a member whose own model carries the
//! default; `page_bounds` the one for a member that states a maximum only, whose bound is
//! refused rather than defaulted. The harness is the one `slo_lifecycle` drives this service's
//! `restJson1` surface with, so every suite here speaks to one mounted router.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::exclusion_windows::{batch, window, MAX_WINDOWS_PER_SLO};
use crate::slo_lifecycle::{call, create, name_path_segment, router, sli_config};

/// The service entity every seeded objective monitors, and that the audited target names.
const SERVICE: &str = "checkout";

/// The SLO whose exclusion windows are paged.
const WINDOWED_SLO: &str = "windowed-objective";

/// One read whose `MaxResults` declares its whole range and no usable default.
#[derive(Clone, Copy, Debug)]
enum Read {
    /// `ListAuditFindings`, paging the findings its targets resolve to.
    AuditFindings,
    /// `ListServiceLevelObjectiveExclusionWindows`, paging one SLO's window set.
    ExclusionWindows,
    /// `ListServiceLevelObjectives`, paging the stored objectives.
    Objectives,
}

impl Read {
    /// Every read the rule covers.
    const ALL: [Read; 3] = [Read::AuditFindings, Read::ExclusionWindows, Read::Objectives];

    fn operation(self) -> &'static str {
        match self {
            Read::AuditFindings => "ListAuditFindings",
            Read::ExclusionWindows => "ListServiceLevelObjectiveExclusionWindows",
            Read::Objectives => "ListServiceLevelObjectives",
        }
    }

    /// The maximum this read's own `MaxResults` range admits (see this module's header).
    fn modelled_maximum(self) -> usize {
        match self {
            Read::AuditFindings | Read::ExclusionWindows => 10,
            Read::Objectives => 50,
        }
    }

    /// The page a caller who states no `MaxResults` gets from this read.
    ///
    /// `ListAuditFindings` documents 10; the other two members document no page their own
    /// range admits, so each falls back to the largest page it can serve — the modelled
    /// maximum of its own member.
    fn documented_default(self) -> usize {
        match self {
            Read::AuditFindings => 10,
            Read::ExclusionWindows | Read::Objectives => self.modelled_maximum(),
        }
    }

    /// The most items this read's projection can ever hold, where the API caps it: one SLO
    /// holds at most ten exclusion windows, while findings and objectives are uncapped here.
    fn capacity(self) -> Option<usize> {
        match self {
            Read::ExclusionWindows => Some(MAX_WINDOWS_PER_SLO),
            Read::AuditFindings | Read::Objectives => None,
        }
    }
}

/// A router holding `stored` items for one read, with everything a page request needs.
struct Seeded {
    read: Read,
    router: MultiServiceRouter,
    stored: usize,
    /// Epoch seconds the seeded records were written around, for the window the time-ranged
    /// read bounds its findings by.
    now: i64,
}

impl Seeded {
    /// One page of this read: the identity of each item it reported, in order, and the cursor
    /// that follows it. Asserts only that the read answered 200, so a caller can assert the
    /// bound.
    async fn page(
        &self,
        max_results: Option<usize>,
        token: Option<&str>,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        let (method, uri, body) = match self.read {
            Read::AuditFindings => {
                let mut body = json!({
                    "AuditTargets": [{
                        "Type": "service",
                        "Data": { "Service": key_attributes(SERVICE) },
                    }],
                });
                if let Some(max_results) = max_results {
                    body["MaxResults"] = json!(max_results);
                }
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                (
                    "POST",
                    format!(
                        "/auditFindings?StartTime={}&EndTime={}",
                        self.now - 3_600,
                        self.now + 3_600
                    ),
                    Some(body),
                )
            }
            Read::ExclusionWindows => (
                "GET",
                format!(
                    "/slo/{}/exclusion-windows{}",
                    name_path_segment(WINDOWED_SLO),
                    query(max_results, token)
                ),
                None,
            ),
            Read::Objectives => (
                "POST",
                format!("/slos{}", query(max_results, token)),
                Some(json!({})),
            ),
        };
        let (status, answer) = call(&self.router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} with {answer}"
        );
        let items = answer[self.items_member()]
            .as_array()
            .map(|items| items.iter().map(|item| self.identity(item)).collect())
            .unwrap_or_default();
        (items, answer["NextToken"].as_str().map(ToString::to_string))
    }

    /// The output member this read reports its page in.
    fn items_member(&self) -> &'static str {
        match self.read {
            Read::AuditFindings => "AuditFindings",
            Read::ExclusionWindows => "ExclusionWindows",
            Read::Objectives => "SloSummaries",
        }
    }

    /// What tells one reported item of this read's projection from another: the audited
    /// operation, the window's duration, the objective's name.
    fn identity(&self, item: &Value) -> String {
        let member = match self.read {
            Read::AuditFindings => item["Operation"].clone(),
            Read::ExclusionWindows => item["Window"]["Duration"].clone(),
            Read::Objectives => item["Name"].clone(),
        };
        member
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| member.to_string())
    }

    /// Walk this read from its first page to its last at `max_results`, answering every item
    /// it reported in order.
    async fn walk(&self, max_results: Option<usize>, case: &str) -> Vec<String> {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for visited in 0..=self.stored + 1 {
            let (items, next) = self.page(max_results, token.as_deref(), case).await;
            walked.extend(items);
            token = next;
            if token.is_none() {
                return walked;
            }
            assert!(
                visited < self.stored,
                "case {case:?}: expected the pages to run out within {} pages, got {} items so far",
                self.stored,
                walked.len()
            );
        }
        unreachable!("the loop returns or asserts")
    }
}

/// The key attributes of one Application Signals service entity, as `sli_config` names it.
fn key_attributes(service: &str) -> Value {
    json!({ "Type": "Service", "Name": service, "Environment": "prod" })
}

/// The query string for a bound and a cursor, each omitted when the caller states none.
fn query(max_results: Option<usize>, token: Option<&str>) -> String {
    let mut tail = String::new();
    if let Some(max_results) = max_results {
        tail.push_str(&format!("MaxResults={max_results}&"));
    }
    if let Some(token) = token {
        tail.push_str(&format!("NextToken={token}&"));
    }
    if tail.is_empty() {
        tail
    } else {
        format!("?{}", tail.trim_end_matches('&'))
    }
}

/// Seconds since the epoch, for the window the time-ranged read is taken over.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// Store `count` pageable items for `read` on a fresh router.
///
/// An objective list is seeded with `count` objectives; an audit of one service entity reports
/// one finding per objective monitoring it, so it is seeded with `count` objectives of ONE
/// service at distinct operations; a window read is seeded with one objective carrying `count`
/// windows of distinct durations.
async fn seed(read: Read, count: usize) -> Seeded {
    let router = router();
    let now = now_seconds();

    match read {
        Read::AuditFindings | Read::Objectives => {
            for index in 0..count {
                let name = format!("objective-{index:03}");
                let (status, created) = create(
                    &router,
                    json!({
                        "Name": name,
                        "SliConfig": sli_config(SERVICE, &format!("GET /page/{index:03}"), 100.0),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected the setup create to answer 200, got {status} with {created}"
                );
            }
        }
        Read::ExclusionWindows => {
            let (status, created) = create(
                &router,
                json!({
                    "Name": WINDOWED_SLO,
                    "SliConfig": sli_config(SERVICE, "GET /cart", 100.0),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "expected the setup create to answer 200, got {status} with {created}"
            );
            let windows: Vec<Value> = (1..=count)
                .map(|duration| {
                    window("MINUTE", duration as i64, Some(now - 7_200), None, None)
                })
                .collect();
            let (status, applied) =
                batch(&router, &[WINDOWED_SLO.to_string()], &windows, &[]).await;
            assert_eq!(
                status, 200,
                "expected the setup batch of {count} windows to answer 200, got {status} with {applied}"
            );
        }
    }

    Seeded {
        read,
        router,
        stored: count,
        now,
    }
}

/// Every bound the member's range admits is served as the page it names, on all three reads:
/// the minimum of one, bounds inside the range, exactly the stored count, one past it, and the
/// published maximum. A cursor is offered only while items remain after the page, and the bound
/// is a page size rather than a filter — the walk still reports every stored item exactly once
/// whichever bound it is taken at.
#[tokio::test]
async fn a_stated_bound_is_served_as_the_page_it_names() {
    const STORED: usize = 8;

    for read in Read::ALL {
        let seeded = seed(read, STORED).await;
        for (why, bound) in [
            ("the minimum bound", 1_usize),
            ("two, inside the range", 2),
            ("three, inside the range", 3),
            ("exactly the stored count", STORED),
            ("one past the stored count", STORED + 1),
            ("the published maximum", read.modelled_maximum()),
        ] {
            let case = format!("{} with MaxResults={bound} ({why})", read.operation());
            let (items, token) = seeded.page(Some(bound), None, &case).await;
            let expected = bound.min(STORED);
            assert_eq!(
                items.len(),
                expected,
                "case {case:?}: expected {expected} of the {STORED} stored on the page, got {items:?}"
            );
            assert_eq!(
                token.is_some(),
                expected < STORED,
                "case {case:?}: expected a cursor only while items remain, got {token:?}"
            );

            let walked = seeded.walk(Some(bound), &case).await;
            let mut distinct = walked.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(
                (walked.len(), distinct.len()),
                (STORED, STORED),
                "case {case:?}: expected each of the {STORED} stored items exactly once, got {walked:?}"
            );
        }
    }
}

/// A request that states NO `MaxResults` is served that read's OWN default page, and a cursor
/// onto whatever that page left behind.
///
/// Each read is seeded with one item more than its default where its projection can hold that
/// many; one SLO holds at most ten exclusion windows, which is also that read's default, so
/// there the whole set is the page and no cursor follows it.
#[tokio::test]
async fn an_absent_bound_serves_this_reads_own_default_page() {
    for read in Read::ALL {
        let default = read.documented_default();
        let stored = read
            .capacity()
            .map_or(default + 1, |capacity| capacity.min(default + 1));
        let case = format!(
            "{} holding {stored} items with no MaxResults at all",
            read.operation()
        );
        let seeded = seed(read, stored).await;

        let (items, token) = seeded.page(None, None, &case).await;
        let expected = default.min(stored);
        assert_eq!(
            items.len(),
            expected,
            "case {case:?}: expected the default page of {expected}, got {} items",
            items.len()
        );
        assert_eq!(
            token.is_some(),
            expected < stored,
            "case {case:?}: expected a cursor only while items remain after the default page, got {token:?}"
        );

        let walked = seeded.walk(None, &case).await;
        let mut distinct = walked.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            (walked.len(), distinct.len()),
            (stored, stored),
            "case {case:?}: expected each of the {stored} stored items exactly once, got {walked:?}"
        );
    }
}

/// A read whose store holds FEWER items than its default serves all of them and no cursor: the
/// default is a bound on the page, not a demand for that many items.
#[tokio::test]
async fn an_absent_bound_under_the_default_serves_every_item_without_a_cursor() {
    for read in Read::ALL {
        let case = format!("{} holding fewer items than its default", read.operation());
        let seeded = seed(read, 3).await;
        let (items, token) = seeded.page(None, None, &case).await;
        assert_eq!(
            items.len(),
            3,
            "case {case:?}: expected all three stored items on one default page, got {items:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: expected no cursor once the items run out, got {token:?}"
        );
    }
}

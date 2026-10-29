// SPDX-License-Identifier: CC0-1.0

//! The page bound the time-windowed reads of this service hold to, over HTTP.
//!
//! The rule under test: where a `MaxResults` member's own model declares the WHOLE range it
//! admits together with the `@default` it carries, the read serves the bound the request
//! states, and a request that states NO bound is served that operation's own default page —
//! never a page of nothing. One rule, so it is asserted over EVERY read of this service whose
//! `MaxResults` is modelled that way and whose set a time window selects:
//!
//! * `ListServices` — `MaxResults` `range 1..100`, `@default` 50;
//! * `ListServiceOperations` — `range 1..100`, `@default` 20;
//! * `ListServiceDependencies` — `range 1..100`, `@default` 20;
//! * `ListServiceDependents` — `range 1..100`, `@default` 20;
//! * `ListEntityEvents` — `range 1..250`, `@default` 20,
//!
//! each from `smithy_query(query="operation",operation="ListServices,ListServiceOperations,
//! ListServiceDependencies,ListServiceDependents,ListEntityEvents")`.
//!
//! `defaulted_page_bound` pins the same rule on the two instrumentation reads that own such a
//! member, and `page_bounds` pins the SIBLING rule for a member whose model states a maximum
//! only, whose bound is refused rather than defaulted.
//!
//! Every set read back here is derived from records created over the same `restJson1` surface,
//! so each assertion is a round trip through stored state. The request helpers (`router`,
//! `call`, `create`) are the ones `slo_lifecycle` drives this service's `restJson1` surface
//! with, as its closest siblings `page_bounds`, `max_results_range` and `optional_page_bound`
//! do, so every suite speaks to ONE harness.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, router};

/// The service whose own operations, dependencies and change history are paged below.
const SUBJECT: &str = "checkout";

/// The service every seeded dependency edge of [`SUBJECT`] points at, and whose dependents
/// are therefore the seeded callers.
const CALLED: &str = "payments";

/// The environment every seeded service entity is hosted in.
const ENVIRONMENT: &str = "prod";

/// Whole seconds since the epoch: every window below is named in whole seconds, which
/// round-trips exactly, and the seeded records' own sub-second instants fall strictly inside.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": ENVIRONMENT })
}

/// Create one SLO and assert it was stored. `indicator` is the `SliMetricConfig` the SLO
/// monitors, which is what makes the entity, operation or dependency edge observable.
async fn create_slo(router: &MultiServiceRouter, name: &str, indicator: Value) {
    let (status, answer) = create(
        router,
        json!({ "Name": name, "SliConfig": { "SliMetricConfig": indicator } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the setup SLO create to answer 200, got {status} with {answer}"
    );
}

/// One time-windowed read whose `MaxResults` carries both its range and a `@default`.
#[derive(Clone, Copy, Debug)]
enum Read {
    /// `ListServices`, paging the discovered services.
    Services,
    /// `ListServiceOperations`, paging one service's operations.
    Operations,
    /// `ListServiceDependencies`, paging the edges out of one service.
    Dependencies,
    /// `ListServiceDependents`, paging the edges into one service.
    Dependents,
    /// `ListEntityEvents`, paging one entity's change history.
    EntityEvents,
}

impl Read {
    /// Every read the rule covers.
    const ALL: [Read; 5] = [
        Read::Services,
        Read::Operations,
        Read::Dependencies,
        Read::Dependents,
        Read::EntityEvents,
    ];

    fn operation(self) -> &'static str {
        match self {
            Read::Services => "ListServices",
            Read::Operations => "ListServiceOperations",
            Read::Dependencies => "ListServiceDependencies",
            Read::Dependents => "ListServiceDependents",
            Read::EntityEvents => "ListEntityEvents",
        }
    }

    /// The output member carrying the page.
    fn items(self) -> &'static str {
        match self {
            Read::Services => "ServiceSummaries",
            Read::Operations => "ServiceOperations",
            Read::Dependencies => "ServiceDependencies",
            Read::Dependents => "ServiceDependents",
            Read::EntityEvents => "ChangeEvents",
        }
    }

    /// The page this read serves a caller who states no `MaxResults`: the `@default` its own
    /// member carries (see this module's header for each one).
    fn documented_default(self) -> usize {
        match self {
            Read::Services => 50,
            Read::Operations | Read::Dependencies | Read::Dependents | Read::EntityEvents => 20,
        }
    }

    /// The maximum its `MaxResults` member's own `@range` declares.
    fn modelled_maximum(self) -> i64 {
        match self {
            Read::Services | Read::Operations | Read::Dependencies | Read::Dependents => 100,
            Read::EntityEvents => 250,
        }
    }

    /// The identity of one reported entry, so a cursor walk can be asserted to report each
    /// stored entry exactly once.
    fn identity(self, item: &Value) -> String {
        match self {
            Read::Services => item["KeyAttributes"]["Name"].to_string(),
            Read::Operations => item["Name"].to_string(),
            Read::Dependencies => item["DependencyKeyAttributes"]["Name"].to_string(),
            Read::Dependents => item["DependentKeyAttributes"]["Name"].to_string(),
            // A change event carries no id of its own ("For other events, this will be
            // `Unknown`"), so an event is identified by the instant it is dated at together
            // with the name of the change it is evidence of — the pair the store's own total
            // order over the history is built on.
            Read::EntityEvents => {
                format!("{}/{}", item["Timestamp"], item["EventName"])
            }
        }
    }
}

/// A router holding `stored` entries for one read, over a window covering them all.
struct Seeded {
    read: Read,
    router: MultiServiceRouter,
    stored: usize,
    window: (i64, i64),
}

impl Seeded {
    /// One page of this read, as the identity of each entry it reported and the cursor that
    /// follows it. Asserts only that the read answered 200, so a caller can assert the bound.
    async fn page(
        &self,
        max_results: Option<i64>,
        token: Option<&str>,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        let (start, end) = self.window;
        let mut query = String::new();
        if let Some(max_results) = max_results {
            query.push_str(&format!("&MaxResults={max_results}"));
        }
        if let Some(token) = token {
            query.push_str(&format!("&NextToken={token}"));
        }
        let window = format!("StartTime={start}&EndTime={end}");

        let (method, uri, body) = match self.read {
            Read::Services => ("GET", format!("/services?{window}{query}"), None),
            Read::Operations => (
                "POST",
                format!("/service-operations?{window}{query}"),
                Some(json!({ "KeyAttributes": service_entity(SUBJECT) })),
            ),
            Read::Dependencies => (
                "POST",
                format!("/service-dependencies?{window}{query}"),
                Some(json!({ "KeyAttributes": service_entity(SUBJECT) })),
            ),
            Read::Dependents => (
                "POST",
                format!("/service-dependents?{window}{query}"),
                Some(json!({ "KeyAttributes": service_entity(CALLED) })),
            ),
            // `ListEntityEvents` binds its window to the BODY, so only the page controls are
            // query-bound here.
            Read::EntityEvents => (
                "POST",
                format!("/events?{}", query.trim_start_matches('&')),
                Some(json!({
                    "Entity": service_entity(SUBJECT),
                    "StartTime": start,
                    "EndTime": end,
                })),
            ),
        };

        let (status, answer) = call(&self.router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} with {answer}"
        );
        let items = answer[self.read.items()]
            .as_array()
            .map(|items| items.iter().map(|item| self.read.identity(item)).collect())
            .unwrap_or_default();
        (items, answer["NextToken"].as_str().map(ToString::to_string))
    }

    /// Walk this read from its first page to its last with `max_results`, answering every
    /// entry it reported in order.
    async fn walk(&self, max_results: Option<i64>, case: &str) -> Vec<String> {
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
                "case {case:?}: expected the pages to run out within {} pages, got {} entries so far",
                self.stored,
                walked.len()
            );
        }
        unreachable!("the loop returns or asserts")
    }

    /// Every entry this read reports when ONE page covers them all, in the read's own order:
    /// the whole stored set, taken at the member's published maximum, which is above every
    /// count seeded here. It is what a paged walk is compared against, so the comparison does
    /// not rest on two entries being distinguishable from each other.
    async fn everything(&self, case: &str) -> Vec<String> {
        let (items, token) = self
            .page(Some(self.read.modelled_maximum()), None, case)
            .await;
        assert_eq!(
            (items.len(), token.as_deref()),
            (self.stored, None),
            "case {case:?}: expected one page at the published maximum to carry all {} stored entries and no cursor, got {items:?} with {token:?}",
            self.stored
        );
        items
    }
}

/// Store `count` entries for `read` on a fresh router.
///
/// Every entry is derived from a stored SLO: a distinct service entity for the services read,
/// a distinct operation of one service for the operations read, a distinct far end for the
/// dependencies read, a distinct caller for the dependents read, and one created SLO per
/// change event for the history read.
async fn seed(read: Read, count: usize) -> Seeded {
    let router = router();
    let before = now_seconds();

    for index in 0..count {
        let name = format!("{index:03}");
        let indicator = match read {
            Read::Services => json!({
                "KeyAttributes": service_entity(&format!("svc-{name}")),
                "MetricType": "LATENCY"
            }),
            Read::Operations => json!({
                "KeyAttributes": service_entity(SUBJECT),
                "OperationName": format!("GET /op-{name}"),
                "MetricType": "LATENCY"
            }),
            Read::Dependencies => json!({
                "KeyAttributes": service_entity(SUBJECT),
                "OperationName": "POST /checkout",
                "DependencyConfig": {
                    "DependencyKeyAttributes": service_entity(&format!("dep-{name}")),
                    "DependencyOperationName": "Charge"
                },
                "MetricType": "AVAILABILITY"
            }),
            Read::Dependents => json!({
                "KeyAttributes": service_entity(&format!("caller-{name}")),
                "OperationName": "POST /call",
                "DependencyConfig": {
                    "DependencyKeyAttributes": service_entity(CALLED),
                    "DependencyOperationName": "Charge"
                },
                "MetricType": "AVAILABILITY"
            }),
            Read::EntityEvents => json!({
                "KeyAttributes": service_entity(SUBJECT),
                "OperationName": format!("GET /op-{name}"),
                "MetricType": "LATENCY"
            }),
        };
        create_slo(&router, &format!("slo-{index:03}"), indicator).await;
    }

    Seeded {
        read,
        router,
        stored: count,
        // One hour either side of the instant the records were written at.
        window: (before - 3_600, now_seconds() + 3_600),
    }
}

/// Every bound the member's range admits is served as the page it names, on every read: the
/// minimum of one, bounds inside the range, exactly the stored count, one past it, and the
/// published maximum. A cursor is offered only while entries remain after the page, and the
/// cursor walk reports the whole stored set in its order whichever bound it is taken at — so
/// the bound is a page size, never a filter.
#[tokio::test]
async fn a_stated_bound_is_served_as_the_page_it_names() {
    const STORED: usize = 5;

    for read in Read::ALL {
        let seeded = seed(read, STORED).await;
        let whole = seeded.everything(read.operation()).await;
        for (why, bound) in [
            ("the minimum bound", 1_i64),
            ("two, inside the range", 2),
            ("three, inside the range", 3),
            ("exactly the stored count", STORED as i64),
            ("one past the stored count", STORED as i64 + 1),
            ("the published maximum", read.modelled_maximum()),
        ] {
            let case = format!("{} with MaxResults={bound} ({why})", read.operation());
            let (items, token) = seeded.page(Some(bound), None, &case).await;
            let expected = (bound as usize).min(STORED);
            assert_eq!(
                items.len(),
                expected,
                "case {case:?}: expected {expected} of the {STORED} stored on the page, got {items:?}"
            );
            assert_eq!(
                items,
                whole[..expected],
                "case {case:?}: expected the page to be the first {expected} of the stored order {whole:?}, got {items:?}"
            );
            assert_eq!(
                token.is_some(),
                expected < STORED,
                "case {case:?}: expected a cursor only while entries remain, got {token:?}"
            );

            let walked = seeded.walk(Some(bound), &case).await;
            assert_eq!(
                walked, whole,
                "case {case:?}: expected the walk to report the whole stored order {whole:?}, got {walked:?}"
            );
        }
    }
}

/// A request that states NO `MaxResults` is served that read's OWN default page — 50 services,
/// 20 for each entity-scoped read — and a cursor onto what the default page left behind.
///
/// Each read is seeded with one more entry than its default, so the page size the answer
/// carries IS the default rather than merely at least the stored count.
#[tokio::test]
async fn an_absent_bound_serves_this_reads_default_page() {
    for read in Read::ALL {
        let default = read.documented_default();
        let case = format!("{} with no MaxResults at all", read.operation());
        let seeded = seed(read, default + 1).await;
        let whole = seeded.everything(&case).await;

        let (items, token) = seeded.page(None, None, &case).await;
        assert_eq!(
            items.len(),
            default,
            "case {case:?}: expected the default page of {default}, got {} entries",
            items.len()
        );
        assert_eq!(
            items,
            whole[..default],
            "case {case:?}: expected the default page to be the first {default} of the stored order {whole:?}, got {items:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: expected a cursor onto the entry the default page left behind")
        });

        let (rest, after) = seeded.page(None, Some(&token), &case).await;
        assert_eq!(
            rest,
            whole[default..],
            "case {case:?}: expected the one entry the default page left behind, got {rest:?}"
        );
        assert_eq!(
            after, None,
            "case {case:?}: expected no cursor after the last entry, got {after:?}"
        );
    }
}

/// A read whose store holds FEWER entries than its default serves all of them and no cursor:
/// the default is a bound on the page, not a demand for that many entries.
#[tokio::test]
async fn an_absent_bound_under_the_default_serves_every_entry_without_a_cursor() {
    const STORED: usize = 3;

    for read in Read::ALL {
        let case = format!("{} holding fewer entries than its default", read.operation());
        let seeded = seed(read, STORED).await;
        let (items, token) = seeded.page(None, None, &case).await;
        assert_eq!(
            items.len(),
            STORED,
            "case {case:?}: expected all {STORED} stored entries on one default page, got {items:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: expected no cursor once the entries run out, got {token:?}"
        );
    }
}

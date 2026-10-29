// SPDX-License-Identifier: CC0-1.0

//! The page bound every read whose `MaxResults` carries a documented `@default` holds to, over
//! HTTP.
//!
//! The rule under test: where a `MaxResults` member's own model declares the WHOLE range it
//! admits together with the `@default` its documentation names, the read serves the bound the
//! request states, and a request that states NO bound serves that operation's documented
//! default page — never a page of nothing. It is one rule, so it is asserted on both reads of
//! this service that own such a member, which differ in the default they document:
//!
//! * `ListInstrumentationConfigurations` — `MaxResults` `range 1..100`, `@default` 50
//!   ("The default is 50 and the maximum is 100");
//! * `GetInstrumentationConfigurationStatus` — `MaxResults` `range 1..100`, `@default` 60
//!   ("The maximum number of status events to return in one call. The default is 60"),
//!
//! both from `smithy_query(query="shape",
//! shape="ListInstrumentationConfigurationsRequest,GetInstrumentationConfigurationStatusRequest")`.
//!
//! `page_bounds` pins the SIBLING rule, for a member whose model states a maximum only and
//! whose bound is therefore refused rather than defaulted. The subject here is the defaulted
//! form: that each read serves every bound its range admits, and that each falls back to its
//! OWN documented default when the member is absent.
//!
//! The harness is the one `instrumentation_configuration_status` drives this service's
//! `restJson1` surface with, so these suites agree about the wire surface.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_status::{
    call, create_configuration, create_request, event_times, get_status, now_minute, report,
    report_entry, router, status_request, Query, BREAKPOINT,
};

/// The service and environment every read here addresses.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/checkout";

/// The one status the seeded events are reported under, so a read of it sees them all.
const STATUS: &str = "READY";

/// The modelled maximum both members admit (`range 1..100` on each).
const MODELLED_MAXIMUM: i64 = 100;

/// One read whose `MaxResults` carries both its full range and a documented `@default`.
#[derive(Clone, Copy, Debug)]
enum Read {
    /// `ListInstrumentationConfigurations`, paging a scope's stored configurations.
    Configurations,
    /// `GetInstrumentationConfigurationStatus`, paging one configuration's status events.
    StatusEvents,
}

impl Read {
    /// Every read the rule covers.
    const ALL: [Read; 2] = [Read::Configurations, Read::StatusEvents];

    fn operation(self) -> &'static str {
        match self {
            Read::Configurations => "ListInstrumentationConfigurations",
            Read::StatusEvents => "GetInstrumentationConfigurationStatus",
        }
    }

    /// The page this read's own documentation says a caller who states no `MaxResults` gets:
    /// the member's modelled `@default` (see this module's header for each doc sentence).
    fn documented_default(self) -> usize {
        match self {
            Read::Configurations => 50,
            Read::StatusEvents => 60,
        }
    }
}

/// A router holding `stored` items for one read, with everything a page request of that read
/// needs to name them.
struct Seeded {
    read: Read,
    router: MultiServiceRouter,
    stored: usize,
    /// The configuration whose status events are paged — unused by the configuration list.
    hash: String,
    /// The window the status read pages over, wide enough to hold every seeded event.
    window: (f64, f64),
}

impl Seeded {
    /// One page of this read, as the identity of each item it reported and the cursor that
    /// follows it. Asserts only that the read answered 200, so a caller can assert the bound.
    async fn page(
        &self,
        max_results: Option<i64>,
        token: Option<&str>,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        let (code, answer) = match self.read {
            Read::Configurations => {
                let mut body = json!({
                    "Service": SERVICE,
                    "Environment": ENVIRONMENT,
                    "InstrumentationType": BREAKPOINT,
                });
                if let Some(max_results) = max_results {
                    body["MaxResults"] = json!(max_results);
                }
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                call(&self.router, "/list-instrumentation-configurations", &body).await
            }
            Read::StatusEvents => {
                let request = status_request(
                    SERVICE,
                    ENVIRONMENT,
                    BREAKPOINT,
                    json!({ "LocationHash": self.hash.clone() }),
                    &Query {
                        status: Some(STATUS),
                        window: Some(self.window),
                        max_results,
                        next_token: token,
                    },
                );
                get_status(&self.router, &request).await
            }
        };
        assert_eq!(
            code, 200,
            "case {case:?}: expected HTTP 200 from the read, got {code} with {answer}"
        );
        let items = match self.read {
            Read::Configurations => answer["LatestConfigurations"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| item["LocationHash"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            Read::StatusEvents => event_times(&answer)
                .into_iter()
                .map(|time| time.to_string())
                .collect(),
        };
        (items, answer["NextToken"].as_str().map(ToString::to_string))
    }

    /// Walk this read from its first page to its last with `max_results`, answering every item
    /// it reported in order.
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
                "case {case:?}: expected the pages to run out within {} pages, got {} items so far",
                self.stored,
                walked.len()
            );
        }
        unreachable!("the loop returns or asserts")
    }
}

/// Store `count` items for `read` on a fresh router.
///
/// A configuration list is seeded with one configuration per distinct code location (the
/// location alone decides the stored identity); a status read is seeded with one configuration
/// and `count` events of one status at distinct whole minutes, since an event is held at the
/// minute its report named.
async fn seed(read: Read, count: usize) -> Seeded {
    let router = router();
    let base = now_minute();
    let hash = create_configuration(
        &router,
        &create_request(SERVICE, ENVIRONMENT, BREAKPOINT, "src/Checkout.java"),
    )
    .await;

    match read {
        Read::Configurations => {
            // The first create above is already one of the `count` stored configurations.
            for index in 1..count {
                create_configuration(
                    &router,
                    &create_request(
                        SERVICE,
                        ENVIRONMENT,
                        BREAKPOINT,
                        &format!("src/Checkout{index}.java"),
                    ),
                )
                .await;
            }
        }
        Read::StatusEvents => {
            let entries: Vec<Value> = (1..=count)
                .map(|step| {
                    report_entry(
                        BREAKPOINT,
                        &hash,
                        STATUS,
                        base - 60.0 * step as f64,
                        None,
                    )
                })
                .collect();
            let (code, body) = report(
                &router,
                &json!({
                    "Service": SERVICE,
                    "Environment": ENVIRONMENT,
                    "Configurations": entries,
                }),
            )
            .await;
            assert_eq!(
                code, 200,
                "expected HTTP 200 from the setup report of {count} events, got {code} with {body}"
            );
            assert_eq!(
                body["UnprocessedStatusEvents"],
                json!([]),
                "expected every seeded event to be recorded, got {body}"
            );
        }
    }

    Seeded {
        read,
        router,
        stored: count,
        hash,
        // One whole minute either side of the oldest and newest seeded event.
        window: (base - 60.0 * (count as f64 + 1.0), base + 60.0),
    }
}

/// Every bound the member's range admits is served as the page it names, on both reads: the
/// minimum of one, bounds inside the range, exactly the stored count, one past it, and the
/// published maximum. A cursor is offered only while items remain after the page.
#[tokio::test]
async fn a_stated_bound_is_served_as_the_page_it_names() {
    const STORED: usize = 8;

    for read in Read::ALL {
        let seeded = seed(read, STORED).await;
        for (why, bound) in [
            ("the minimum bound", 1_i64),
            ("two, inside the range", 2),
            ("three, inside the range", 3),
            ("exactly the stored count", STORED as i64),
            ("one past the stored count", STORED as i64 + 1),
            ("the published maximum", MODELLED_MAXIMUM),
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
                token.is_some(),
                expected < STORED,
                "case {case:?}: expected a cursor only while items remain, got {token:?}"
            );

            // The bound is a page size, not a filter: the walk still reports every stored item
            // exactly once, whichever bound it is taken at.
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

/// A request that states NO `MaxResults` is served that read's OWN documented default page —
/// 50 configurations, 60 status events — and a cursor onto what the default left behind.
///
/// Each read is seeded with one more item than its default, so the page size the answer carries
/// IS the default rather than merely at least the stored count.
#[tokio::test]
async fn an_absent_bound_serves_this_reads_documented_default_page() {
    for read in Read::ALL {
        let default = read.documented_default();
        let case = format!("{} with no MaxResults at all", read.operation());
        let seeded = seed(read, default + 1).await;

        let (items, token) = seeded.page(None, None, &case).await;
        assert_eq!(
            items.len(),
            default,
            "case {case:?}: expected the documented default page of {default}, got {} items",
            items.len()
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: expected a cursor onto the item the default page left behind")
        });

        let (rest, after) = seeded.page(None, Some(&token), &case).await;
        assert_eq!(
            rest.len(),
            1,
            "case {case:?}: expected the one remaining item on the second page, got {rest:?}"
        );
        assert_eq!(
            after, None,
            "case {case:?}: expected no cursor after the last item, got {after:?}"
        );

        let mut walked = items;
        walked.extend(rest);
        let mut distinct = walked.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            (walked.len(), distinct.len()),
            (default + 1, default + 1),
            "case {case:?}: expected each of the {} stored items exactly once, got {walked:?}",
            default + 1
        );
    }
}

/// A read whose store holds FEWER items than the default serves all of them and no cursor: the
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

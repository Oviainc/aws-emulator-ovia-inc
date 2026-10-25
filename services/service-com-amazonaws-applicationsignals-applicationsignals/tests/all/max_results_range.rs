// SPDX-License-Identifier: CC0-1.0

//! One rule, one implementation: a page size outside the `@range` its own `MaxResults`
//! member declares is refused before the operation's handler runs, and worded by this
//! service's single constraint-violation renderer.
//!
//! Every paged read of this service relies on that — none of them re-checks the bound —
//! so the suite drives the SAME shared path through four of them, each with a DIFFERENT
//! modelled range, and asserts both halves of the rule: each published bound is served,
//! and one step past either bound is refused with the member path and the requirement
//! naming THAT operation's bound.
//!
//! The ranges below come from the model, one `MaxResults` target shape per operation
//! (`smithy_query(query="operation",operation="ListServiceLevelObjectives,ListServices,
//! ListServiceOperations,ListAuditFindings")`).
//!
//! The request helpers (`router`, `call`, …) are the ones `slo_lifecycle` drives this
//! service's `restJson1` surface with, so every suite speaks to one harness.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, router, sli_config};

/// How a paged read of this service is asked for a page of `max_results` entries: the URI
/// (built over the epoch-seconds time window `window`, which an operation that binds no
/// time range ignores) and the body that goes with it.
type Request = fn(window: &str, max_results: i64) -> (String, Option<Value>);

/// One paged read: its route, its own modelled `MaxResults` range, and how to ask it for a
/// page of a given size.
struct Listing {
    label: &'static str,
    method: &'static str,
    /// The output member carrying the page, so an accepted bound is asserted to answer a
    /// page of at most that many entries.
    items: &'static str,
    /// The minimum its `MaxResults` member's `@range` declares.
    min: i64,
    /// The maximum its `MaxResults` member's `@range` declares.
    max: i64,
    request: Request,
}

/// Seconds since the epoch, for a window around the instant the seeded SLOs are created at.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": "prod" })
}

/// `ListServiceLevelObjectives`: `MaxResults` is query-bound, and the operation binds no
/// time range.
fn slos_request(_window: &str, max_results: i64) -> (String, Option<Value>) {
    (format!("/slos?MaxResults={max_results}"), Some(json!({})))
}

/// `ListServices`: every member is query-bound, including the required time range.
fn services_request(window: &str, max_results: i64) -> (String, Option<Value>) {
    (
        format!("/services?{window}&MaxResults={max_results}"),
        None,
    )
}

/// `ListServiceOperations`: `MaxResults` and the time range are query-bound, the entity the
/// operations belong to is a required body member.
fn service_operations_request(window: &str, max_results: i64) -> (String, Option<Value>) {
    (
        format!("/service-operations?{window}&MaxResults={max_results}"),
        Some(json!({ "KeyAttributes": service_entity("checkout") })),
    )
}

/// `ListAuditFindings`: the time range is query-bound but `MaxResults` is a BODY member, so
/// this row drives the same rule over a different part of the request.
fn audit_findings_request(window: &str, max_results: i64) -> (String, Option<Value>) {
    (
        format!("/auditFindings?{window}"),
        Some(json!({
            "AuditTargets": [{ "Type": "service", "Data": { "Service": service_entity("checkout") } }],
            "MaxResults": max_results
        })),
    )
}

/// The paged reads this rule holds for, with the range each one's own `MaxResults` declares:
/// three different maxima over four operations, two of which share one.
const LISTINGS: &[Listing] = &[
    // ListServiceLevelObjectivesMaxResults: range 1..50.
    Listing {
        label: "ListServiceLevelObjectives",
        method: "POST",
        items: "SloSummaries",
        min: 1,
        max: 50,
        request: slos_request,
    },
    // ListServicesMaxResults: range 1..100.
    Listing {
        label: "ListServices",
        method: "GET",
        items: "ServiceSummaries",
        min: 1,
        max: 100,
        request: services_request,
    },
    // ListServiceOperationMaxResults: range 1..100.
    Listing {
        label: "ListServiceOperations",
        method: "POST",
        items: "ServiceOperations",
        min: 1,
        max: 100,
        request: service_operations_request,
    },
    // ListAuditFindingMaxResults: range 1..10.
    Listing {
        label: "ListAuditFindings",
        method: "POST",
        items: "AuditFindings",
        min: 1,
        max: 10,
        request: audit_findings_request,
    },
];

/// Seed enough SLOs that every seeded read has more than one entry to page over: two
/// operations of one entity, plus a second entity.
async fn seed(router: &MultiServiceRouter) {
    let fixtures = [
        ("checkout-latency", "checkout", "POST /checkout"),
        ("cart-availability", "checkout", "GET /cart"),
        ("payments-health", "payments", "POST /pay"),
    ];
    for (name, service, operation) in fixtures {
        let (status, answer) = create(
            router,
            json!({ "Name": name, "SliConfig": sli_config(service, operation, 100.0) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the setup create to answer 200, got {status} with {answer}"
        );
    }
}

/// Each published bound of each operation's own `MaxResults` range is SERVED, and answers a
/// page no longer than the size asked for.
#[tokio::test]
async fn every_paged_read_serves_each_published_max_results_bound() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    for listing in LISTINGS {
        for max_results in [listing.min, listing.max] {
            let (uri, body) = (listing.request)(&window, max_results);
            let (status, answer) = call(&router, listing.method, &uri, body).await;
            let case = format!("{} MaxResults={max_results}", listing.label);
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for a page size inside the modelled range, got {status} with {answer}"
            );
            let items = answer[listing.items].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() as i64 <= max_results,
                "case {case:?}: expected at most {max_results} entries, got {}",
                items.len()
            );
        }
    }
}

/// One step past either bound of an operation's own `MaxResults` range — and a value far
/// past the maximum — is refused with HTTP 400 and the one wording this service gives a
/// `@range` violation: the member path and the requirement, naming that operation's own
/// bound and echoing nothing of the rejected value.
#[tokio::test]
async fn every_paged_read_refuses_a_max_results_outside_its_modelled_range() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    for listing in LISTINGS {
        // One step below the minimum, a negative, one step past the maximum, and far past
        // it: the requirement reported names the bound the value crossed.
        let below = format!(
            "Member must have value greater than or equal to {}",
            listing.min
        );
        let above = format!(
            "Member must have value less than or equal to {}",
            listing.max
        );
        let rejected: [(i64, &str); 4] = [
            (listing.min - 1, &below),
            (-7, &below),
            (listing.max + 1, &above),
            (100_000, &above),
        ];

        for (max_results, requirement) in &rejected {
            let (uri, body) = (listing.request)(&window, *max_results);
            let (status, error) = call(&router, listing.method, &uri, body).await;
            let case = format!("{} MaxResults={max_results}", listing.label);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a page size outside the modelled range, got {status} with {error}"
            );
            let expected =
                format!("1 validation error detected: Value at 'maxResults' failed to satisfy constraint: {requirement}");
            let message = error["message"].as_str().unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(&max_results.to_string()),
                "case {case:?}: the message must not echo the rejected value, got {message:?}"
            );
        }
    }
}

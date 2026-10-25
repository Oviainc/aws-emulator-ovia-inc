// SPDX-License-Identifier: CC0-1.0

//! The page bound every paginated list of this service holds to, over HTTP.
//!
//! The rule under test: `MaxResults` is a COUNT of items, so a list serves a bound of at
//! least one and at most the maximum its member models, and a bound outside that range is
//! REFUSED — `ValidationException` (HTTP 400) naming the member and the requirement — rather
//! than coerced into a page the caller never asked for. The rule is a property of the bound,
//! not of one operation: it holds whether the member's own model states the minimum
//! (`ListServices`, `ListServiceLevelObjectives`) or states a maximum only
//! (`ListServiceStates` — `smithy_query(query="shape",
//! shape="ListServiceStatesMaxResults")` -> `range ..250`), and whether the member is bound
//! to the query string or to the JSON body.
//!
//! The harness is the one `slo_lifecycle` drives this service's `restJson1` surface with, so
//! every suite here speaks to one mounted router.
//!
//! `max_results_range` pins the bounds the MODEL states, enforced for those lists by the
//! centralized walk. The subject HERE is the rule itself: that the minimum holds for a member
//! which leaves it unstated as well, that it reads identically either way, and that each
//! admitted bound is still served as the page it names — so `ListServiceStates` is asserted
//! beside two lists whose own model carries the minimum.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, router, sli_config};

/// The member path every refusal of a page bound names, in the camel case the boundary
/// validator spells a member path with.
const MEMBER_PATH: &str = "maxResults";

/// One HTTP request as this suite's harness takes it: method, URI and optional JSON body.
type Request = (&'static str, String, Option<Value>);

/// Builds the request that lists with a page bound and a cursor, for one operation.
type RequestBuilder = fn(i64, Option<i64>, Option<&str>) -> Request;

/// One paginated list under test.
struct Listed {
    operation: &'static str,
    /// The modelled maximum of this list's `MaxResults`, from
    /// `smithy_query(query="shape", shape="<Operation>MaxResults")`: `..250` for
    /// `ListServiceStates`, `1..100` for `ListServices`, `1..50` for
    /// `ListServiceLevelObjectives`.
    maximum: i64,
    /// The output member the page is reported in.
    items: &'static str,
    /// The request listing with `max_results` (absent when `None`) and resuming from
    /// `token`, over a window around `now`, with each member in the place this operation's
    /// model binds it to.
    request: RequestBuilder,
}

/// `ListServiceStates`: every input, `MaxResults` and `NextToken` included, is a JSON body
/// member.
fn list_service_states_request(now: i64, max_results: Option<i64>, token: Option<&str>) -> Request {
    let mut body = json!({ "StartTime": now - 3_600, "EndTime": now + 3_600 });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    ("POST", "/service/states".to_string(), Some(body))
}

/// `ListServices`: a GET whose window, `MaxResults` and `NextToken` are query-bound.
fn list_services_request(now: i64, max_results: Option<i64>, token: Option<&str>) -> Request {
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    ("GET", format!("/services?{window}{}", query(max_results, token)), None)
}

/// `ListServiceLevelObjectives`: a POST with a query-bound `MaxResults` and no window.
fn list_service_level_objectives_request(
    _now: i64,
    max_results: Option<i64>,
    token: Option<&str>,
) -> Request {
    (
        "POST",
        format!("/slos?{}", query(max_results, token).trim_start_matches('&')),
        Some(json!({})),
    )
}

/// The query-string tail for a bound and a cursor, each omitted when absent.
fn query(max_results: Option<i64>, token: Option<&str>) -> String {
    let mut tail = String::new();
    if let Some(max_results) = max_results {
        tail.push_str(&format!("&MaxResults={max_results}"));
    }
    if let Some(token) = token {
        tail.push_str(&format!("&NextToken={token}"));
    }
    tail
}

/// The lists the rule is asserted over: they differ along where the model binds `MaxResults`
/// (body vs. query string), the modelled maximum, whether the member states a MINIMUM at all,
/// and whether the member carries a `@default`.
fn lists() -> Vec<Listed> {
    vec![
        Listed {
            operation: "ListServiceStates",
            maximum: 250,
            items: "ServiceStates",
            request: list_service_states_request,
        },
        Listed {
            operation: "ListServices",
            maximum: 100,
            items: "ServiceSummaries",
            request: list_services_request,
        },
        Listed {
            operation: "ListServiceLevelObjectives",
            maximum: 50,
            items: "SloSummaries",
            request: list_service_level_objectives_request,
        },
    ]
}

/// The SLOs every test here stores: one per service, so each list under test reports more
/// than one item and a bound of one leaves items behind it.
const SEEDED_SERVICES: &[&str] = &["alpha", "beta", "gamma"];

/// Store one SLO per entry of [`SEEDED_SERVICES`] and answer how many items each list then
/// reports — the same count for all three, since each SLO is evidence of exactly one
/// discovered service.
async fn seed(router: &MultiServiceRouter) -> usize {
    for service in SEEDED_SERVICES {
        let (status, created) = create(
            router,
            json!({
                "Name": format!("{service}-latency"),
                "SliConfig": sli_config(service, "GET /cart", 100.0)
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected the setup create to answer 200, got {status} with {created}"
        );
    }
    SEEDED_SERVICES.len()
}

/// Seconds since the epoch, for a window around the instant the seeded SLOs are written at.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// A bound below one is refused, on every list and whatever its magnitude or sign — never
/// served as a page of one plus a cursor, which would answer a bound the caller never asked
/// for.
///
/// The rows differ along the operation (and with it where `MaxResults` is bound and whether
/// the model states the minimum) and along the magnitude and sign of the rejected bound: zero,
/// the first negative value, a larger negative one, and the smallest 32-bit integer the member
/// can carry.
#[tokio::test]
async fn a_bound_below_one_is_refused_on_every_list() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    let expected = format!(
        "1 validation error detected: Value at '{MEMBER_PATH}' failed to satisfy constraint: \
         Member must have value greater than or equal to 1"
    );

    for listed in lists() {
        for (bound_case, max_results) in [
            ("zero", 0_i64),
            ("the first negative value", -1),
            ("a larger negative value", -5),
            ("the smallest 32-bit integer", i32::MIN as i64),
        ] {
            let case = format!("{} with MaxResults={max_results} ({bound_case})", listed.operation);
            let (method, uri, body) = (listed.request)(now, Some(max_results), None);
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a bound below one, got {status} with {answer}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                answer["message"]
            );
            assert!(
                answer.get(listed.items).is_none() && answer.get("NextToken").is_none(),
                "case {case:?}: a refused bound carries no page and no cursor, got {answer}"
            );
        }
    }
}

/// One step past the modelled maximum is refused by the same rule from the other end, stating
/// that list's own maximum.
#[tokio::test]
async fn a_bound_past_the_modelled_maximum_is_refused_on_every_list() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    for listed in lists() {
        let case = format!("{} one step past its maximum", listed.operation);
        let (method, uri, body) = (listed.request)(now, Some(listed.maximum + 1), None);
        let (status, answer) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a bound past the maximum, got {status} with {answer}"
        );
        let expected = format!(
            "1 validation error detected: Value at '{MEMBER_PATH}' failed to satisfy constraint: \
             Member must have value less than or equal to {}",
            listed.maximum
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer["message"]
        );
    }
}

/// Every bound the member admits is SERVED, and serves at most that many items: the minimum of
/// one, the published maximum, and no bound at all (which takes the documented default).
/// A cursor is offered only while items remain after the page.
#[tokio::test]
async fn every_admitted_bound_is_served_as_a_page_of_at_most_that_many_items() {
    let router = router();
    let stored = seed(&router).await;
    let now = now_seconds();

    for listed in lists() {
        for (bound_case, max_results) in [
            ("the minimum bound", Some(1_i64)),
            ("two, inside the range", Some(2)),
            ("the published maximum", Some(listed.maximum)),
            ("no bound at all", None),
        ] {
            let case = format!("{} with {bound_case}", listed.operation);
            let (method, uri, body) = (listed.request)(now, max_results, None);
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for an admitted bound, got {status} with {answer}"
            );
            let items = answer[listed.items].as_array().cloned().unwrap_or_default();
            // An absent bound takes the member's own default, which is at least as large as
            // the seeded set, so every seeded item fits in one page.
            let expected = max_results.unwrap_or(listed.maximum).min(stored as i64) as usize;
            assert_eq!(
                items.len(),
                expected,
                "case {case:?}: expected {expected} item(s) of the {stored} stored, got {items:?}"
            );
            assert_eq!(
                answer["NextToken"].as_str().is_some(),
                expected < stored,
                "case {case:?}: expected a cursor only while items remain, got {:?}",
                answer["NextToken"]
            );
        }
    }
}

/// A bound of one walks the whole list through its cursor, one item per page: the refusal of a
/// non-positive bound is what keeps that walk finite, since a page of nothing could not advance
/// its own cursor.
#[tokio::test]
async fn a_bound_of_one_walks_every_item_exactly_once() {
    let router = router();
    let stored = seed(&router).await;
    let now = now_seconds();

    for listed in lists() {
        let case = listed.operation;
        let (method, uri, body) = (listed.request)(now, Some(listed.maximum), None);
        let (_, whole) = call(&router, method, &uri, body).await;
        let in_order: Vec<Value> = whole[listed.items].as_array().cloned().unwrap_or_default();

        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..stored + 2 {
            let (method, uri, body) = (listed.request)(now, Some(1), token.as_deref());
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 200,
                "case {case:?} page {page}: expected HTTP 200, got {status} with {answer}"
            );
            let items = answer[listed.items].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= 1,
                "case {case:?} page {page}: expected at most one item, got {items:?}"
            );
            walked.extend(items);
            token = answer["NextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: expected the walk to end without a cursor, got {token:?}"
        );
        assert_eq!(
            walked, in_order,
            "case {case:?}: expected every item exactly once in the single-page order, got {walked:?}"
        );
    }
}

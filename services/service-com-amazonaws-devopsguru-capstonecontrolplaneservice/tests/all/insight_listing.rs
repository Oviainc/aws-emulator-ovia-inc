// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInsights`: the status filter it accepts, the insights it
//! selects, and the pages it walks them in.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same [`MultiServiceRouter`] the emulator boots — so the assertions are
//! about the wire behaviour, not about handler internals.
//!
//! DevOps Guru MINTS an insight itself, from anomalies in the covered resources; no
//! modelled operation creates one. So a test that needs insights seeds the store the
//! analysis would write (`state.insights`), exactly as the handler reads it, and then
//! asks the service over HTTP.
//!
//! Each test states the RULE it pins and exercises it over a set of inputs that differ
//! along what the rule generalises over (which selector, insight type, status, time
//! window, page size, token), with the request from the issue report as one row.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord, ResourceCoverage,
    PAGE_SIZE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// An instant well inside the range a `Timestamp` can carry, in epoch milliseconds.
/// Every seeded insight is placed relative to it, so no row depends on the clock.
///
/// `pub(crate)` with [`insight`], [`selection_fixture`] and [`epoch_seconds`], so the
/// organization-wide insight listing is asserted over the SAME fixture this suite uses
/// instead of a second one: the two listings select over one store, so they must agree.
pub(crate) const BASE_MILLIS: i64 = 1_700_000_000_000;

/// One minute, in milliseconds — the step between seeded instants.
pub(crate) const MINUTE_MILLIS: i64 = 60_000;

/// One reply, as a client sees it.
struct Reply {
    status: u16,
    body: Value,
    /// The error SHAPE NAME the reply carries, from the `x-amzn-errortype` header or
    /// the body's `__type`, with any namespace/URL prefix stripped. Empty on a success.
    error_name: String,
    raw: String,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} {}", self.status, self.raw)
    }
}

impl Reply {
    /// Whether the rendered error IS `code` — the parsed error name, not a substring of
    /// the body, so an error name quoted inside a message cannot satisfy it.
    fn names_error(&self, code: &str) -> bool {
        self.error_name == code
    }

    fn at(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer)
    }

    /// The `Id`s of one list member, in the order the reply listed them.
    fn ids_at(&self, pointer: &str) -> Vec<String> {
        self.at(pointer)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.get("Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every `Id` the reply listed, proactive insights first.
    fn listed_ids(&self) -> Vec<String> {
        let mut ids = self.ids_at("/ProactiveInsights");
        ids.extend(self.ids_at("/ReactiveInsights"));
        ids
    }

    fn next_token(&self) -> Option<String> {
        self.at("/NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A fresh emulator holding only this service, over its own engine.
struct Guru {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Guru {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process-wide switch.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_devopsguru_capstonecontrolplaneservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", HOST)
            .header("content-type", "application/json")
            .body(Bytes::from(body.to_string()))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let raw = format!("{errortype} {}", String::from_utf8_lossy(&bytes));
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        // restJson1 names the error in the `x-amzn-errortype` header, the body's
        // `__type`, or both; either may carry a `namespace#Name` or URL prefix.
        let declared = if errortype.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            errortype
        };
        let error_name = declared
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            body,
            error_name,
            raw,
        }
    }

    /// `ListInsights` with this status filter, page size and cursor.
    async fn list_insights(
        &self,
        status_filter: &Value,
        max_results: Option<i32>,
        next_token: Option<&str>,
    ) -> Reply {
        let mut body = json!({ "StatusFilter": status_filter });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(next_token) = next_token {
            body["NextToken"] = json!(next_token);
        }
        self.call("POST", "/insights", &body).await
    }

    /// Put `records` in the insight store the handler reads, the way analysis would.
    fn seed(&self, records: Vec<InsightRecord>) {
        let state = self
            .engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>();
        let mut store = state.lock();
        for record in records {
            store.insights.insert(record.id.clone(), record);
        }
    }

    /// Drop every insight, so a cursor into the earlier collection is stale.
    fn clear_insights(&self) {
        self.engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>()
            .lock()
            .insights
            .clear();
    }

    /// Every id one status filter lists, walking its pages at `max_results` each.
    async fn walk(&self, status_filter: &Value, max_results: Option<i32>) -> Vec<String> {
        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let reply = self
                .list_insights(status_filter, max_results, token.as_deref())
                .await;
            assert_eq!(
                reply.status, 200,
                "walking {status_filter} at {max_results:?} per page must succeed, got {reply:?}"
            );
            walked.extend(reply.listed_ids());
            match reply.next_token() {
                Some(next) => token = Some(next),
                None => return walked,
            }
        }
    }
}

/// One seeded insight: the members every row varies, with the rest left at a fixed
/// value so a difference in the listing can only come from these.
///
/// `severity` is `HIGH` from `INSIGHT_SEVERITY_VALUES`; `insight_type` and `status` are
/// the caller's, from `INSIGHT_TYPE_VALUES` and `INSIGHT_STATUS_VALUES`.
pub(crate) fn insight(
    id: &str,
    insight_type: &str,
    status: &str,
    started_millis: i64,
    ended_millis: Option<i64>,
) -> InsightRecord {
    InsightRecord {
        id: id.to_string(),
        name: format!("DevOps Guru insight {id}"),
        insight_type: insight_type.to_string(),
        severity: "HIGH".to_string(),
        status: status.to_string(),
        start_time_millis: started_millis,
        end_time_millis: ended_millis,
        ..InsightRecord::default()
    }
}

/// A timestamp as restJson1 renders and reads one: epoch SECONDS.
pub(crate) fn epoch_seconds(millis: i64) -> f64 {
    millis as f64 / 1_000.0
}

/// The eight insights the selection rows are asserted against: both types, both
/// statuses, and instants spread over a known grid.
///
/// `started`/`ended` are offsets from [`BASE_MILLIS`], in minutes.
pub(crate) fn selection_fixture() -> Vec<InsightRecord> {
    let at = |minutes: i64| BASE_MILLIS + minutes * MINUTE_MILLIS;
    vec![
        insight("ongoing-proactive-early", "PROACTIVE", "ONGOING", at(0), None),
        insight("ongoing-proactive-late", "PROACTIVE", "ONGOING", at(30), None),
        insight("ongoing-reactive", "REACTIVE", "ONGOING", at(10), None),
        insight(
            "closed-proactive-early",
            "PROACTIVE",
            "CLOSED",
            at(0),
            Some(at(5)),
        ),
        insight(
            "closed-proactive-late",
            "PROACTIVE",
            "CLOSED",
            at(20),
            Some(at(40)),
        ),
        insight(
            "closed-reactive",
            "REACTIVE",
            "CLOSED",
            at(10),
            Some(at(15)),
        ),
        // A closed insight that records no end instant: it lies in no end-time range.
        insight("closed-proactive-open-ended", "PROACTIVE", "CLOSED", at(0), None),
        insight("ongoing-reactive-latest", "REACTIVE", "ONGOING", at(90), None),
    ]
}

// =============================================================================
// 1. Exactly one status selector
// =============================================================================

/// RULE: `ListInsights` answers a request whose `StatusFilter` names exactly ONE of
/// `Ongoing`, `Closed` and `Any`, and refuses any other count with HTTP 400
/// `ValidationException` — `StatusFilter` documents "You can specify one status
/// filter", and the model, which marks all three members optional, cannot express it.
///
/// The rows differ in WHICH selectors are named and in how many: none (the recorded
/// request), each one alone over both insight types and both bounded and unbounded
/// ranges, each pair, and all three.
#[tokio::test]
async fn a_status_filter_names_exactly_one_selector() {
    let from = epoch_seconds(BASE_MILLIS);
    let to = epoch_seconds(BASE_MILLIS + 60 * MINUTE_MILLIS);
    let ongoing = json!({ "Ongoing": { "Type": "REACTIVE" } });
    let closed = json!({ "Closed": { "Type": "REACTIVE", "EndTimeRange": { "FromTime": from, "ToTime": to } } });
    let any = json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": { "FromTime": from } } });

    let cases: [(&str, Value, bool); 10] = [
        // The recorded unit: a StatusFilter carrying no selector at all.
        ("no selector", json!({}), false),
        ("Ongoing REACTIVE", ongoing.clone(), true),
        (
            "Ongoing PROACTIVE",
            json!({ "Ongoing": { "Type": "PROACTIVE" } }),
            true,
        ),
        ("Closed REACTIVE, both bounds", closed.clone(), true),
        (
            "Closed PROACTIVE, no bounds",
            json!({ "Closed": { "Type": "PROACTIVE", "EndTimeRange": {} } }),
            true,
        ),
        ("Any PROACTIVE, open-ended range", any.clone(), true),
        (
            "Any REACTIVE, inverted range",
            json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": { "FromTime": to, "ToTime": from } } }),
            true,
        ),
        (
            "Ongoing + Closed",
            json!({ "Ongoing": ongoing["Ongoing"], "Closed": closed["Closed"] }),
            false,
        ),
        (
            "Closed + Any",
            json!({ "Closed": closed["Closed"], "Any": any["Any"] }),
            false,
        ),
        (
            "all three",
            json!({ "Ongoing": ongoing["Ongoing"], "Closed": closed["Closed"], "Any": any["Any"] }),
            false,
        ),
    ];

    let guru = Guru::new();
    for (case, status_filter, accepted) in cases {
        let reply = guru.list_insights(&status_filter, None, None).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: one selector must be accepted, got {reply:?}"
            );
            assert!(
                reply.listed_ids().is_empty(),
                "case {case:?}: an account whose analysis produced no insight lists none, got {reply:?}"
            );
            assert!(
                reply.next_token().is_none(),
                "case {case:?}: a single page carries no NextToken, got {reply:?}"
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: {} selector(s) must be refused, got {reply:?}",
                status_filter.as_object().map_or(0, serde_json::Map::len)
            );
            assert!(
                reply.names_error("ValidationException"),
                "case {case:?}: the refusal must be a ValidationException, got {reply:?}"
            );
        }
    }
}

/// RULE: a `Type` outside `InsightType` is a client error, not an empty listing.
///
/// The rows differ in FORM of the wrong value — another enum's value, a lowercased
/// value, and the empty string — across the three selectors, so none of them is keyed
/// to one member.
#[tokio::test]
async fn an_insight_type_outside_the_enum_is_refused() {
    let guru = Guru::new();
    guru.seed(selection_fixture());
    let from = epoch_seconds(BASE_MILLIS);

    for (case, status_filter) in [
        (
            "Ongoing, another enum's value",
            json!({ "Ongoing": { "Type": "ONGOING" } }),
        ),
        (
            "Ongoing, lowercased",
            json!({ "Ongoing": { "Type": "proactive" } }),
        ),
        ("Ongoing, empty", json!({ "Ongoing": { "Type": "" } })),
        (
            "Closed, unknown value",
            json!({ "Closed": { "Type": "PREDICTIVE", "EndTimeRange": { "FromTime": from } } }),
        ),
        (
            "Any, unknown value",
            json!({ "Any": { "Type": "PREDICTIVE", "StartTimeRange": { "FromTime": from } } }),
        ),
    ] {
        let reply = guru.list_insights(&status_filter, None, None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a Type outside InsightType must be refused, got {reply:?}"
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {reply:?}"
        );
    }
}

// =============================================================================
// 2. What each selector selects
// =============================================================================

/// RULE: a listing reports exactly the stored insights of the selector's `InsightType`
/// that its status selects — `Ongoing` those still ONGOING, `Closed` those CLOSED whose
/// `EndTime` lies in the given range, `Any` those whose `StartTime` lies in the given
/// range whatever their status — and reports each one in the list for its own type.
///
/// The rows differ in selector, insight type, status and the width and placement of the
/// window, including the bounds of a window that fall exactly on a stored instant, a
/// window that excludes everything, a half-open window and an inverted one.
#[tokio::test]
async fn a_selector_reports_exactly_the_insights_it_selects() {
    let guru = Guru::new();
    guru.seed(selection_fixture());
    let at = |minutes: i64| epoch_seconds(BASE_MILLIS + minutes * MINUTE_MILLIS);

    let cases: [(&str, Value, &[&str]); 10] = [
        (
            "ongoing proactive",
            json!({ "Ongoing": { "Type": "PROACTIVE" } }),
            &["ongoing-proactive-early", "ongoing-proactive-late"],
        ),
        (
            "ongoing reactive",
            json!({ "Ongoing": { "Type": "REACTIVE" } }),
            &["ongoing-reactive", "ongoing-reactive-latest"],
        ),
        (
            "closed proactive, window over every end instant",
            json!({ "Closed": { "Type": "PROACTIVE", "EndTimeRange": { "FromTime": at(0), "ToTime": at(60) } } }),
            &["closed-proactive-early", "closed-proactive-late"],
        ),
        (
            "closed proactive, bounds exactly on one end instant",
            json!({ "Closed": { "Type": "PROACTIVE", "EndTimeRange": { "FromTime": at(5), "ToTime": at(5) } } }),
            &["closed-proactive-early"],
        ),
        (
            "closed proactive, window before every end instant",
            json!({ "Closed": { "Type": "PROACTIVE", "EndTimeRange": { "FromTime": at(-120), "ToTime": at(-60) } } }),
            &[],
        ),
        (
            "closed reactive, unbounded window",
            json!({ "Closed": { "Type": "REACTIVE", "EndTimeRange": {} } }),
            &["closed-reactive"],
        ),
        (
            "any proactive, unbounded window spans both statuses",
            json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": {} } }),
            &[
                "closed-proactive-early",
                "closed-proactive-late",
                "closed-proactive-open-ended",
                "ongoing-proactive-early",
                "ongoing-proactive-late",
            ],
        ),
        (
            "any proactive, half-open window from minute 20",
            json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": { "FromTime": at(20) } } }),
            &["closed-proactive-late", "ongoing-proactive-late"],
        ),
        (
            "any reactive, window up to minute 60",
            json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": { "ToTime": at(60) } } }),
            &["closed-reactive", "ongoing-reactive"],
        ),
        (
            "any reactive, inverted window holds no instant",
            json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": { "FromTime": at(60), "ToTime": at(0) } } }),
            &[],
        ),
    ];

    for (case, status_filter, expected) in cases {
        let reply = guru.list_insights(&status_filter, None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a well-formed filter must succeed, got {reply:?}"
        );
        let mut listed = reply.listed_ids();
        listed.sort();
        assert_eq!(
            listed,
            expected.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "case {case:?}: the listing must hold exactly the selected insights, got {reply:?}"
        );
        // An insight is reported in the list for its OWN type.
        let wrong_list = if status_filter
            .to_string()
            .contains("\"Type\":\"PROACTIVE\"")
        {
            "/ReactiveInsights"
        } else {
            "/ProactiveInsights"
        };
        assert!(
            reply.ids_at(wrong_list).is_empty(),
            "case {case:?}: {wrong_list} must hold no insight of the other type, got {reply:?}"
        );
        // A read reports, it does not consume: repeating it answers the same thing.
        let again = guru.list_insights(&status_filter, None, None).await;
        assert_eq!(
            again.body, reply.body,
            "case {case:?}: repeating the read must report the same insights, got {again:?}"
        );
    }
}

// =============================================================================
// 3. Pagination
// =============================================================================

/// RULE: a walk of a selection reports every matching insight exactly once, in pages of
/// at most the requested `MaxResults`, and the last page carries no `NextToken`.
///
/// The rows differ in page size — one per page, a size that does not divide the total,
/// the modelled maximum, no `MaxResults` at all (the service's own page size), and a
/// size that CHANGES between pages of one walk, which AWS honours per call.
#[tokio::test]
async fn a_walk_reports_every_selected_insight_exactly_once() {
    let guru = Guru::new();
    // More than one page at the service's own page size, so a no-MaxResults walk pages
    // too, and a count that none of the page sizes divides evenly.
    let total = PAGE_SIZE + 3;
    let seeds: Vec<InsightRecord> = (0..total)
        .map(|index| {
            insight(
                &format!("ongoing-{index:04}"),
                "PROACTIVE",
                "ONGOING",
                BASE_MILLIS + index as i64 * MINUTE_MILLIS,
                None,
            )
        })
        .collect();
    let expected: Vec<String> = {
        let mut ids: Vec<String> = seeds.iter().map(|seed| seed.id.clone()).collect();
        ids.sort();
        ids
    };
    guru.seed(seeds);
    let status_filter = json!({ "Ongoing": { "Type": "PROACTIVE" } });

    for (case, max_results) in [
        ("one per page", Some(1)),
        ("seven per page", Some(7)),
        ("the modelled maximum", Some(100)),
        ("no MaxResults", None),
    ] {
        let walked = guru.walk(&status_filter, max_results).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report every selected insight exactly once"
        );
    }

    // A walk whose page size changes between calls: each page is at most what that call
    // asked for, and the walk still reports each insight exactly once.
    let sizes = [3_i32, 11, 1, 50, 7];
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    let mut step = 0_usize;
    loop {
        let size = sizes[step % sizes.len()];
        let reply = guru
            .list_insights(&status_filter, Some(size), token.as_deref())
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"changing page size\" step {step}: the call must succeed, got {reply:?}"
        );
        let page = reply.listed_ids();
        assert!(
            page.len() <= size as usize,
            "case \"changing page size\" step {step}: a page must hold at most {size} insights, got {} in {reply:?}",
            page.len()
        );
        walked.extend(page);
        step += 1;
        match reply.next_token() {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, expected,
        "case \"changing page size\": the walk must report every selected insight exactly once"
    );
}

/// RULE: a `NextToken` names a page of ONE selection, so a token this service did not
/// issue for the selection being read — another selector's cursor, a rewritten token, or
/// a cursor into a collection that has since shrunk — is refused with HTTP 400
/// `ValidationException` instead of seeking into some other page.
///
/// The rows differ in HOW the token fails to name a page: a different insight type, a
/// different selector over the same insights, a different time window, a rewrite outside
/// the modelled lowercase-hex form, a well-formed token of no collection, and a cursor
/// left over from a collection that was emptied.
#[tokio::test]
async fn a_token_that_names_no_page_of_this_selection_is_refused() {
    let guru = Guru::new();
    let seeds: Vec<InsightRecord> = (0..6)
        .map(|index| {
            insight(
                &format!("ongoing-{index}"),
                "PROACTIVE",
                "ONGOING",
                BASE_MILLIS + index * MINUTE_MILLIS,
                None,
            )
        })
        .collect();
    guru.seed(seeds);
    let unbounded_any = json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": {} } });
    let ongoing_proactive = json!({ "Ongoing": { "Type": "PROACTIVE" } });

    let first = guru.list_insights(&ongoing_proactive, Some(2), None).await;
    assert_eq!(
        first.status, 200,
        "the first page must succeed, got {first:?}"
    );
    let token = first
        .next_token()
        .unwrap_or_else(|| panic!("six insights at two per page must leave a cursor: {first:?}"));

    for (case, status_filter, token) in [
        (
            "the other insight type",
            json!({ "Ongoing": { "Type": "REACTIVE" } }),
            token.clone(),
        ),
        (
            "another selector over the same insights",
            unbounded_any.clone(),
            token.clone(),
        ),
        (
            "the same selector, a different window",
            json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": { "FromTime": epoch_seconds(BASE_MILLIS) } } }),
            token.clone(),
        ),
        (
            "the token uppercased",
            ongoing_proactive.clone(),
            token.to_uppercase(),
        ),
        (
            "a well-formed token of no collection",
            ongoing_proactive.clone(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
    ] {
        let reply = guru
            .list_insights(&status_filter, Some(2), Some(&token))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token naming no page of this selection must be refused, got {reply:?}"
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {reply:?}"
        );
    }

    // The same token stays good for the selection it was issued for, until that
    // collection no longer holds the page it names.
    let resumed = guru
        .list_insights(&ongoing_proactive, Some(2), Some(&token))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case \"its own selection\": the cursor must resume the walk, got {resumed:?}"
    );
    guru.clear_insights();
    let stale = guru
        .list_insights(&ongoing_proactive, Some(2), Some(&token))
        .await;
    assert_eq!(
        stale.status, 400,
        "case \"a cursor into an emptied collection\": the stale page must be refused, got {stale:?}"
    );
    assert!(
        stale.names_error("ValidationException"),
        "case \"a cursor into an emptied collection\": the refusal must be a ValidationException, got {stale:?}"
    );
}

// =============================================================================
// 4. Every stored member reaches the summary
// =============================================================================

/// RULE: every member a stored insight carries is reported on its summary — the id, the
/// name, the severity, the status, the insight time range, the resource collection, the
/// service collection and the associated resource ARNs — and the prediction time range
/// only on a proactive insight, which is the one summary shape that models it.
///
/// The two rows differ in insight type, and so in which summary shape carries them.
#[tokio::test]
async fn every_stored_member_reaches_the_summary() {
    let guru = Guru::new();
    let ended = BASE_MILLIS + 30 * MINUTE_MILLIS;
    let predicted_from = BASE_MILLIS + 5 * MINUTE_MILLIS;
    let predicted_to = BASE_MILLIS + 45 * MINUTE_MILLIS;
    // `StackName` matches `^[a-zA-Z*]+[a-zA-Z0-9-]*$`; `ServiceName` values come from
    // `SERVICE_NAME_VALUES` (smithy_query(query="shape",shape="ServiceName")).
    let coverage = ResourceCoverage {
        cloudformation_stack_names: BTreeSet::from(["checkout-stack".to_string()]),
        tag_collections: BTreeMap::from([(
            "devops-guru-app".to_string(),
            BTreeSet::from(["checkout".to_string(), "payments".to_string()]),
        )]),
    };

    let mut proactive = insight(
        "full-proactive",
        "PROACTIVE",
        "CLOSED",
        BASE_MILLIS,
        Some(ended),
    );
    proactive.severity = "MEDIUM".to_string();
    proactive.prediction_start_time_millis = Some(predicted_from);
    proactive.prediction_end_time_millis = Some(predicted_to);
    proactive.resource_collection = Some(coverage.clone());
    proactive.service_names = vec!["LAMBDA".to_string(), "SQS".to_string()];
    proactive.associated_resource_arns =
        vec!["arn:aws:lambda:us-east-1:123456789012:function:checkout".to_string()];

    let mut reactive = insight(
        "full-reactive",
        "REACTIVE",
        "CLOSED",
        BASE_MILLIS,
        Some(ended),
    );
    reactive.severity = "LOW".to_string();
    reactive.resource_collection = Some(coverage);
    reactive.service_names = vec!["DYNAMO_DB".to_string()];
    reactive.associated_resource_arns =
        vec!["arn:aws:dynamodb:us-east-1:123456789012:table/orders".to_string()];

    guru.seed(vec![proactive.clone(), reactive.clone()]);

    for (case, record, list) in [
        ("proactive", &proactive, "/ProactiveInsights/0"),
        ("reactive", &reactive, "/ReactiveInsights/0"),
    ] {
        let status_filter =
            json!({ "Any": { "Type": record.insight_type, "StartTimeRange": {} } });
        let reply = guru.list_insights(&status_filter, None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must succeed, got {reply:?}"
        );
        for (member, expected) in [
            ("Id", json!(record.id)),
            ("Name", json!(record.name)),
            ("Severity", json!(record.severity)),
            ("Status", json!(record.status)),
            (
                "ServiceCollection",
                json!({ "ServiceNames": record.service_names }),
            ),
            (
                "AssociatedResourceArns",
                json!(record.associated_resource_arns),
            ),
            (
                "ResourceCollection",
                json!({
                    "CloudFormation": { "StackNames": ["checkout-stack"] },
                    "Tags": [{ "AppBoundaryKey": "devops-guru-app", "TagValues": ["checkout", "payments"] }],
                }),
            ),
        ] {
            assert_eq!(
                reply.at(&format!("{list}/{member}")),
                Some(&expected),
                "case {case:?}: {member} must be the stored one, got {reply:?}"
            );
        }
        for (member, millis) in [
            ("InsightTimeRange/StartTime", record.start_time_millis),
            ("InsightTimeRange/EndTime", ended),
        ] {
            let reported = reply
                .at(&format!("{list}/{member}"))
                .and_then(Value::as_f64);
            assert_eq!(
                reported,
                Some(epoch_seconds(millis)),
                "case {case:?}: {member} must be the stored instant, got {reply:?}"
            );
        }
        let prediction = reply.at(&format!("{list}/PredictionTimeRange"));
        if record.insight_type == "PROACTIVE" {
            assert_eq!(
                prediction
                    .and_then(|range| range.get("StartTime"))
                    .and_then(Value::as_f64),
                Some(epoch_seconds(predicted_from)),
                "case {case:?}: a proactive summary reports its prediction start, got {reply:?}"
            );
            assert_eq!(
                prediction
                    .and_then(|range| range.get("EndTime"))
                    .and_then(Value::as_f64),
                Some(epoch_seconds(predicted_to)),
                "case {case:?}: a proactive summary reports its prediction end, got {reply:?}"
            );
        } else {
            assert!(
                prediction.is_none(),
                "case {case:?}: ReactiveInsightSummary models no PredictionTimeRange, got {reply:?}"
            );
        }
    }
}

/// RULE: a member the stored insight does not carry is ABSENT from the summary rather
/// than reported as an empty collection — the insight reported nothing for it.
#[tokio::test]
async fn members_a_stored_insight_does_not_carry_are_absent() {
    let guru = Guru::new();
    guru.seed(vec![insight(
        "bare-insight",
        "REACTIVE",
        "ONGOING",
        BASE_MILLIS,
        None,
    )]);

    let reply = guru
        .list_insights(&json!({ "Ongoing": { "Type": "REACTIVE" } }), None, None)
        .await;
    assert_eq!(reply.status, 200, "the listing must succeed, got {reply:?}");
    for member in [
        "ResourceCollection",
        "ServiceCollection",
        "AssociatedResourceArns",
    ] {
        assert!(
            reply
                .at(&format!("/ReactiveInsights/0/{member}"))
                .is_none(),
            "case {member:?}: a member the insight does not carry must be absent, got {reply:?}"
        );
    }
    assert!(
        reply
            .at("/ReactiveInsights/0/InsightTimeRange/EndTime")
            .is_none(),
        "an ongoing insight's behaviour has not ended, so it reports no EndTime, got {reply:?}"
    );
}

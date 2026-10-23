// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for CloudWatch RUM's extended-metrics destinations:
//! `PutRumMetricsDestination` and the `ListRumMetricsDestinations` read-back.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the
//! handler and the response envelope are all exercised, not just the handler body.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the
//! put/read-back table spans both `MetricDestination` values, one and several Evidently
//! experiments, and the order the puts arrive in; the idempotence table re-puts each
//! kind of destination; the refusal table walks every forbidden `Destination` /
//! `DestinationArn` / `IamRoleArn` pairing the model states in prose plus a destination
//! outside the catalog; the pagination test is an invariant check over a grid of
//! store sizes and page sizes; and the `MaxResults` table walks each published bound of
//! the modelled range plus one past each, beside the documented default page size.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`rum`, per `OPERATIONS.md`'s `## Wire protocol`), which is what the router's
/// `can_handle` routes on. Request authentication is switched off below, so only the
/// scope's shape matters.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/rum/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// One emulator: a shared engine, the RUM service, and the router that dispatches to
/// it. Authentication is off so a test needs no signing key.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(engine));
    router
}

/// One HTTP answer, kept as raw bytes so a case can assert on an EMPTY body as well as
/// on JSON.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({e})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: error body has no __type: {:?}", self.body))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let body_bytes = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serializable request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://rum.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", authorization(REGION))
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    Reply {
        status,
        body: response.into_body(),
    }
}

/// Create one app monitor to hang destinations off, asserting the create succeeded.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    let reply = call(
        router,
        "POST",
        "/appmonitor",
        Some(json!({"Name": name, "Domain": "example.com"})),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// The ARN of an emulated Evidently experiment, the resource an `Evidently` destination
/// names. Built with the shared `engine::emulator_arn` helper, so it is a canonical ARN
/// rather than a hand-spelled string.
fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// The ARN of an emulated IAM role, the role RUM assumes to write to an experiment. IAM
/// is global, so the region segment is empty.
fn role_arn(role: &str) -> String {
    engine::emulator_arn("iam", "", &format!("role/{role}"))
}

/// One extended-metrics destination, as a case states it and as the service should
/// report it back.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Destination {
    destination: String,
    destination_arn: Option<String>,
    iam_role_arn: Option<String>,
}

impl Destination {
    /// The `CloudWatch` destination: the model says it carries neither ARN.
    fn cloud_watch() -> Self {
        Self {
            destination: METRIC_DESTINATIONS[0].to_string(),
            destination_arn: None,
            iam_role_arn: None,
        }
    }

    /// An `Evidently` destination: the experiment that receives the metrics plus the
    /// role RUM writes to it through.
    fn evidently(experiment: &str, role: &str) -> Self {
        Self {
            destination: METRIC_DESTINATIONS[1].to_string(),
            destination_arn: Some(experiment_arn(experiment)),
            iam_role_arn: Some(role_arn(role)),
        }
    }

    /// The same destination written through another role — the one member a re-put can
    /// change without naming a different destination.
    fn with_role(&self, role: &str) -> Self {
        Self {
            iam_role_arn: Some(role_arn(role)),
            ..self.clone()
        }
    }

    /// This destination as a `PutRumMetricsDestination` body: a member the destination
    /// does not have is left out of the request entirely.
    fn put_body(&self) -> Value {
        let mut body = json!({"Destination": self.destination});
        if let Some(arn) = &self.destination_arn {
            body["DestinationArn"] = json!(arn);
        }
        if let Some(arn) = &self.iam_role_arn {
            body["IamRoleArn"] = json!(arn);
        }
        body
    }

    /// The `MetricDestinationSummary` `ListRumMetricsDestinations` must report for it:
    /// all three modelled members, with an ARN this destination kind does not carry
    /// reported as the empty string rather than omitted.
    fn summary(&self) -> Value {
        json!({
            "Destination": self.destination,
            "DestinationArn": self.destination_arn.clone().unwrap_or_default(),
            "IamRoleArn": self.iam_role_arn.clone().unwrap_or_default(),
        })
    }
}

/// `PutRumMetricsDestination` for `monitor`, returning the reply so a case can assert on
/// a refusal as well as on a success.
async fn put_destination(
    router: &MultiServiceRouter,
    monitor: &str,
    body: Value,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(body),
    )
    .await
}

/// `PutRumMetricsDestination` for `monitor`, asserting it succeeded with the modelled
/// empty response.
async fn put_destination_ok(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
) {
    let reply = put_destination(router, monitor, destination.put_body()).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumMetricsDestination({destination:?}) to answer 200, \
         got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.json(case),
        json!({}),
        "case {case:?}: PutRumMetricsDestinationResponse models no members, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

/// The `Destinations` list `ListRumMetricsDestinations` reports for `monitor`, with the
/// query string `query` appended.
async fn list_destinations(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
) -> (Vec<Value>, Option<String>) {
    let reply = call(
        router,
        "GET",
        &format!("/rummetrics/{monitor}/metricsdestination{query}"),
        None,
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let destinations = body["Destinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Destinations in {body}"))
        .clone();
    let next_token = body["NextToken"].as_str().map(str::to_string);
    (destinations, next_token)
}

/// What the service reports for `monitor`, as a set: the order of two different
/// destinations is not what these cases are about.
async fn reported_set(router: &MultiServiceRouter, case: &str, monitor: &str) -> BTreeSet<String> {
    let (destinations, next_token) = list_destinations(router, case, monitor, "").await;
    assert_eq!(
        next_token, None,
        "case {case:?}: a listing that fits in one page carries no NextToken, got {next_token:?}"
    );
    destinations
        .iter()
        .map(|summary| summary.to_string())
        .collect()
}

/// The same set, built from what the case put.
fn expected_set(destinations: &[Destination]) -> BTreeSet<String> {
    destinations
        .iter()
        .map(|destination| destination.summary().to_string())
        .collect()
}

#[tokio::test]
async fn list_reports_exactly_the_destinations_that_were_put() {
    // The rule generalises over the destination kind, how many destinations a monitor
    // has, which experiments they name, and the order the puts arrive in.
    let cases: Vec<(&str, Vec<Destination>)> = vec![
        ("no destination", vec![]),
        ("one CloudWatch destination", vec![Destination::cloud_watch()]),
        (
            "one Evidently destination",
            vec![Destination::evidently("checkout-test", "rum-writer")],
        ),
        (
            "both kinds at once",
            vec![
                Destination::cloud_watch(),
                Destination::evidently("checkout-test", "rum-writer"),
            ],
        ),
        (
            "two Evidently experiments, one role each",
            vec![
                Destination::evidently("alpha", "alpha-writer"),
                Destination::evidently("beta", "beta-writer"),
            ],
        ),
        (
            "three destinations put in reverse name order",
            vec![
                Destination::evidently("zulu", "zulu-writer"),
                Destination::evidently("yankee", "yankee-writer"),
                Destination::cloud_watch(),
            ],
        ),
    ];

    for (index, (case, destinations)) in cases.iter().enumerate() {
        let router = emulator();
        let monitor = format!("destinations-{index}");
        create_monitor(&router, case, &monitor).await;

        // A monitor that has had no put reports an empty list, not a refusal.
        let (before, _) = list_destinations(&router, case, &monitor, "").await;
        assert!(
            before.is_empty(),
            "case {case:?}: a monitor with no destination must report none, got {before:?}"
        );

        for destination in destinations {
            put_destination_ok(&router, case, &monitor, destination).await;
        }

        let reported = reported_set(&router, case, &monitor).await;
        assert_eq!(
            reported,
            expected_set(destinations),
            "case {case:?}: the listing must report exactly what was put, got {reported:?}"
        );
        let (page, _) = list_destinations(&router, case, &monitor, "").await;
        assert_eq!(
            page.len(),
            destinations.len(),
            "case {case:?}: expected {} destination(s), got {page:?}",
            destinations.len()
        );
    }
}

#[tokio::test]
async fn a_destination_is_identified_by_its_kind_and_experiment_arn() {
    // Re-putting the SAME identity updates that destination; naming another experiment
    // adds one. The rule holds for either destination kind, so each is a row.
    let cases: Vec<(&str, Destination, Destination)> = vec![
        (
            "re-put the CloudWatch destination unchanged",
            Destination::cloud_watch(),
            Destination::cloud_watch(),
        ),
        (
            "re-put one Evidently experiment through another role",
            Destination::evidently("gamma", "first-writer"),
            Destination::evidently("gamma", "first-writer").with_role("second-writer"),
        ),
    ];

    for (index, (case, first, second)) in cases.iter().enumerate() {
        let router = emulator();
        let monitor = format!("idempotent-{index}");
        create_monitor(&router, case, &monitor).await;

        put_destination_ok(&router, case, &monitor, first).await;
        put_destination_ok(&router, case, &monitor, second).await;

        // One identity, one entry — and the entry carries what the LAST put said.
        let reported = reported_set(&router, case, &monitor).await;
        assert_eq!(
            reported,
            expected_set(std::slice::from_ref(second)),
            "case {case:?}: a second put under one identity must update it, got {reported:?}"
        );

        // A different experiment is a different identity, so it is added beside it.
        let other = Destination::evidently("delta", "delta-writer");
        put_destination_ok(&router, case, &monitor, &other).await;
        let reported = reported_set(&router, case, &monitor).await;
        assert_eq!(
            reported,
            expected_set(&[second.clone(), other.clone()]),
            "case {case:?}: another experiment is another destination, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_destination_pairing_the_model_forbids_is_refused_and_persists_nothing() {
    let router = emulator();
    let monitor = "refusals";
    create_monitor(&router, "setup", monitor).await;

    // Every forbidden pairing of Destination / DestinationArn / IamRoleArn, from the
    // model's prose: DestinationArn and IamRoleArn are required for Evidently and
    // forbidden for CloudWatch. Source: `smithy_query(query="doc",
    // shape="PutRumMetricsDestinationRequest", member="DestinationArn"|"IamRoleArn")`.
    let experiment = experiment_arn("epsilon");
    let role = role_arn("epsilon-writer");
    let cases: Vec<(&str, Value)> = vec![
        (
            "CloudWatch with a DestinationArn",
            json!({"Destination": "CloudWatch", "DestinationArn": experiment}),
        ),
        (
            "CloudWatch with an IamRoleArn",
            json!({"Destination": "CloudWatch", "IamRoleArn": role}),
        ),
        (
            "CloudWatch with both ARNs",
            json!({
                "Destination": "CloudWatch",
                "DestinationArn": experiment,
                "IamRoleArn": role,
            }),
        ),
        (
            "Evidently with no DestinationArn",
            json!({"Destination": "Evidently", "IamRoleArn": role}),
        ),
        (
            "Evidently with no IamRoleArn",
            json!({"Destination": "Evidently", "DestinationArn": experiment}),
        ),
        (
            "Evidently with neither ARN",
            json!({"Destination": "Evidently"}),
        ),
        (
            "a destination outside the MetricDestination catalog",
            json!({"Destination": "Kinesis"}),
        ),
    ];

    for (case, body) in cases {
        let reply = put_destination(&router, monitor, body.clone()).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected HTTP 400 for {body}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected a ValidationException for {body}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // A refused put writes nothing: the monitor still has no destination.
    let (destinations, _) = list_destinations(&router, "after refusals", monitor, "").await;
    assert!(
        destinations.is_empty(),
        "case \"after refusals\": a refused put must persist nothing, got {destinations:?}"
    );
}

#[tokio::test]
async fn a_request_naming_no_stored_app_monitor_is_not_found() {
    let router = emulator();
    // One monitor exists, so the refusal is about the name the request names rather
    // than about an empty store.
    create_monitor(&router, "setup", "present-monitor").await;

    let rows: [(&str, &str, Option<Value>); 2] = [
        (
            "PutRumMetricsDestination",
            "POST",
            Some(Destination::cloud_watch().put_body()),
        ),
        ("ListRumMetricsDestinations", "GET", None),
    ];
    for (case, method, body) in rows {
        let reply = call(
            &router,
            method,
            "/rummetrics/absent-monitor/metricsdestination",
            body,
        )
        .await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected HTTP 404 for an app monitor that does not exist, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// Page through `ListRumMetricsDestinations` with `max_results`, returning every summary
/// in the order the pages reported them. Asserts the per-page invariants on the way.
async fn page_through(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    max_results: Option<usize>,
    expected_total: usize,
) -> Vec<String> {
    let mut reported = Vec::new();
    let mut tokens_seen = HashSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let query = match (&next_token, max_results) {
            (Some(token), Some(max)) => format!("?maxResults={max}&nextToken={token}"),
            (Some(token), None) => format!("?nextToken={token}"),
            (None, Some(max)) => format!("?maxResults={max}"),
            (None, None) => String::new(),
        };
        let (page, token) = list_destinations(router, case, monitor, &query).await;
        if let Some(max) = max_results {
            assert!(
                page.len() <= max,
                "case {case:?}: a page must hold at most maxResults={max} summaries, got {}",
                page.len()
            );
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    max,
                    "case {case:?}: a page that is not the last must be full, got {}",
                    page.len()
                );
            }
        }
        reported.extend(page.iter().map(|summary| summary.to_string()));
        match token {
            None => break,
            Some(token) => {
                assert!(
                    tokens_seen.insert(token.clone()),
                    "case {case:?}: token {token:?} was issued twice, so paging does not advance"
                );
                assert!(
                    reported.len() <= expected_total,
                    "case {case:?}: paging reported more than the {expected_total} stored \
                     destination(s): {reported:?}"
                );
                next_token = Some(token);
            }
        }
    }
    reported
}

#[tokio::test]
async fn list_pages_over_every_destination_without_repeating_or_dropping() {
    // The invariant: for any number of stored destinations and any page size, paging to
    // exhaustion reports every destination exactly once, in one stable order, and only
    // the last page omits the token.
    for store_size in 0..=5_usize {
        let router = emulator();
        let monitor = format!("paged-{store_size}");
        create_monitor(&router, "setup", &monitor).await;

        let mut stored = Vec::new();
        for index in 0..store_size {
            // A CloudWatch destination first, then one Evidently experiment per extra
            // entry, so the store spans both kinds at every size above one.
            let destination = if index == 0 {
                Destination::cloud_watch()
            } else {
                Destination::evidently(&format!("experiment-{index}"), &format!("writer-{index}"))
            };
            put_destination_ok(&router, "setup", &monitor, &destination).await;
            stored.push(destination);
        }
        let expected = expected_set(&stored);

        let mut orders = BTreeSet::new();
        for max_results in [None, Some(1), Some(2), Some(3), Some(100)] {
            let case = format!("store={store_size} maxResults={max_results:?}");
            let reported = page_through(&router, &case, &monitor, max_results, stored.len()).await;
            assert_eq!(
                reported.len(),
                stored.len(),
                "case {case:?}: expected {} destination(s) in total, got {reported:?}",
                stored.len()
            );
            let unique: BTreeSet<String> = reported.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                reported.len(),
                "case {case:?}: paging repeated a destination: {reported:?}"
            );
            assert_eq!(
                unique, expected,
                "case {case:?}: paging must report exactly the stored destinations, got {unique:?}"
            );
            orders.insert(reported.join("|"));
        }
        assert_eq!(
            orders.len(),
            1,
            "case \"store={store_size}\": every page size must report one stable order, got \
             {orders:?}"
        );
    }
}

#[tokio::test]
async fn list_refuses_a_pagination_token_it_never_issued() {
    let router = emulator();
    let monitor = "token-monitor";
    create_monitor(&router, "setup", monitor).await;
    for index in 0..3 {
        put_destination_ok(
            &router,
            "setup",
            monitor,
            &Destination::evidently(&format!("experiment-{index}"), "writer"),
        )
        .await;
    }

    // The token this operation issues does resume the listing.
    let (_, issued) = list_destinations(&router, "issued token", monitor, "?maxResults=1").await;
    let issued = issued.expect("a NextToken while destinations remain");
    let (page, _) = list_destinations(
        &router,
        "issued token",
        monitor,
        &format!("?maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        page.len(),
        1,
        "case \"issued token\": resuming with an issued token returns the next page, got {page:?}"
    );

    // Anything else is refused rather than read as some other resume point: a plain
    // word, a hex string that is not a token, this operation's marker without a key, and
    // another operation's token.
    for case in [
        "not-a-token",
        "deadbeef",
        &hex::encode("ListRumMetricsDestinations/1/"),
        &hex::encode("ListAppMonitors/1/some-monitor"),
    ] {
        let reply = call(
            &router,
            "GET",
            &format!("/rummetrics/{monitor}/metricsdestination?maxResults=1&nextToken={case}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected HTTP 400 for a token this service never issued, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // Destinations are per-monitor and two monitors' keys overlap, so a token issued for
    // one monitor's listing is not a resume point for another's.
    let other = "other-token-monitor";
    create_monitor(&router, "cross-monitor", other).await;
    put_destination_ok(
        &router,
        "cross-monitor",
        other,
        &Destination::evidently("experiment-0", "writer"),
    )
    .await;
    put_destination_ok(&router, "cross-monitor", other, &Destination::cloud_watch()).await;
    let reply = call(
        &router,
        "GET",
        &format!("/rummetrics/{other}/metricsdestination?maxResults=1&nextToken={issued}"),
        None,
    )
    .await;
    assert_eq!(
        reply.status,
        400,
        "case \"cross-monitor\": a token issued for {monitor:?} must not resume {other:?}, got \
         {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code("cross-monitor"),
        "ValidationException",
        "case \"cross-monitor\": expected a ValidationException, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

#[tokio::test]
async fn a_put_past_the_per_monitor_bound_is_refused_while_an_update_still_lands() {
    // The emulator's own per-monitor ceiling: an ADDING put past it is refused, an
    // UPDATING put at it still lands, because it changes no entry count.
    let bound = service_com_amazonaws_rum_rum::state::MAX_METRIC_DESTINATIONS_PER_APP_MONITOR;
    let router = emulator();
    let monitor = "bounded-monitor";
    create_monitor(&router, "setup", monitor).await;

    // Fill the monitor to the bound, spanning both destination kinds.
    let mut stored = vec![Destination::cloud_watch()];
    while stored.len() < bound {
        let index = stored.len();
        stored.push(Destination::evidently(
            &format!("experiment-{index}"),
            &format!("writer-{index}"),
        ));
    }
    for destination in &stored {
        put_destination_ok(&router, "fill", monitor, destination).await;
    }
    let (page, _) = list_destinations(&router, "fill", monitor, "?maxResults=100").await;
    assert_eq!(
        page.len(),
        bound,
        "case \"fill\": expected the monitor to hold the bound of {bound} destinations, got {}",
        page.len()
    );

    // One more identity is refused, and nothing is persisted for it.
    let overflow = Destination::evidently("one-too-many", "one-too-many-writer");
    let reply = put_destination(&router, monitor, overflow.put_body()).await;
    assert_eq!(
        reply.status,
        400,
        "case \"overflow\": expected HTTP 400 past the bound of {bound}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code("overflow"),
        "ValidationException",
        "case \"overflow\": expected a ValidationException, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );

    // An update of a destination already stored is not an addition, so it still lands.
    let updated = stored[1].with_role("rewritten");
    put_destination_ok(&router, "update at the bound", monitor, &updated).await;
    // The store is at the bound, which is above the default page size, so read it back
    // by paging rather than in one call.
    let reported: BTreeSet<String> =
        page_through(&router, "update at the bound", monitor, Some(100), bound)
            .await
            .into_iter()
            .collect();
    assert_eq!(
        reported.len(),
        bound,
        "case \"update at the bound\": an update must not change the entry count, got {}",
        reported.len()
    );
    assert!(
        reported.contains(&updated.summary().to_string()),
        "case \"update at the bound\": the update must be visible, got {reported:?}"
    );
    assert!(
        !reported.contains(&overflow.summary().to_string()),
        "case \"overflow\": a refused put must persist nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn deleting_an_app_monitor_drops_the_destinations_it_held() {
    let router = emulator();
    let monitor = "doomed-monitor";
    create_monitor(&router, "setup", monitor).await;
    let stored = [
        Destination::cloud_watch(),
        Destination::evidently("omega", "omega-writer"),
    ];
    for destination in &stored {
        put_destination_ok(&router, "setup", monitor, destination).await;
    }
    let reported = reported_set(&router, "setup", monitor).await;
    assert_eq!(
        reported,
        expected_set(&stored),
        "case \"setup\": both destinations must be stored before the delete, got {reported:?}"
    );

    let reply = call(&router, "DELETE", &format!("/appmonitor/{monitor}"), None).await;
    assert_eq!(
        reply.status,
        200,
        "case \"delete\": expected DeleteAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    // A destination belongs to its app monitor, so a monitor re-created under the same
    // name starts with none rather than inheriting the deleted monitor's.
    create_monitor(&router, "re-create", monitor).await;
    let (destinations, _) = list_destinations(&router, "re-create", monitor, "").await;
    assert!(
        destinations.is_empty(),
        "case \"re-create\": a re-created monitor must hold no destination, got {destinations:?}"
    );
}

#[tokio::test]
async fn list_pages_at_the_modelled_max_results_range_and_the_documented_default() {
    // `ListRumMetricsDestinationsRequest$MaxResults` is `@range(min: 1, max: 100)` and
    // documents "The default is 50. The maximum that you can specify is 100."
    // Source: `smithy_query(query="operation", operation="ListRumMetricsDestinations")`.
    const DOCUMENTED_DEFAULT: usize = 50;

    let router = emulator();
    let monitor = "max-results-monitor";
    create_monitor(&router, "setup", monitor).await;

    // One destination MORE than the documented default, so a page that stops at the
    // default is distinguishable from one that returns the whole store.
    let mut stored = vec![Destination::cloud_watch()];
    while stored.len() <= DOCUMENTED_DEFAULT {
        let index = stored.len();
        stored.push(Destination::evidently(
            &format!("experiment-{index}"),
            &format!("writer-{index}"),
        ));
    }
    for destination in &stored {
        put_destination_ok(&router, "setup", monitor, destination).await;
    }

    // No `MaxResults`: the page holds the documented default and carries a token, since
    // the store holds more than one page.
    let (page, next_token) = list_destinations(&router, "no maxResults", monitor, "").await;
    assert_eq!(
        page.len(),
        DOCUMENTED_DEFAULT,
        "case \"no maxResults\": expected the documented default page of {DOCUMENTED_DEFAULT}, \
         got {}",
        page.len()
    );
    assert!(
        next_token.is_some(),
        "case \"no maxResults\": a page that left {} destination(s) unreported must carry a \
         NextToken",
        stored.len() - DOCUMENTED_DEFAULT
    );

    // Each published bound of the modelled range, and one past each: inside the range a
    // page holds at most `MaxResults` summaries; outside it the request is refused.
    for (case, max_results, accepted) in [
        ("one below the range", 0_usize, false),
        ("the range's minimum", 1, true),
        ("the range's maximum", 100, true),
        ("one above the range", 101, false),
    ] {
        let reply = call(
            &router,
            "GET",
            &format!("/rummetrics/{monitor}/metricsdestination?maxResults={max_results}"),
            None,
        )
        .await;
        if !accepted {
            assert_eq!(
                reply.status,
                400,
                "case {case:?}: expected maxResults={max_results} to be refused with 400, got \
                 {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            continue;
        }
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected maxResults={max_results} to be accepted, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        let reported = body["Destinations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Destinations in {body}"));
        assert_eq!(
            reported.len(),
            max_results.min(stored.len()),
            "case {case:?}: expected a page of {} summaries, got {}",
            max_results.min(stored.len()),
            reported.len()
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteRumMetricsDestination`: removing ONE of an app
//! monitor's extended-metrics destinations.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the
//! handler and the response envelope are all exercised, not just the handler body. What
//! a delete did is read back through `ListRumMetricsDestinations`, which reads the same
//! per-monitor map `PutRumMetricsDestination` writes.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the removal
//! table spans the whole `MetricDestination` catalog and several Evidently experiments
//! and walks them one delete at a time, asserting after each that EVERY destination not
//! named is still reported; the not-found table walks every way a request can name a
//! `(Destination, DestinationArn)` pair the monitor holds no record under; and the
//! refusal table walks the pairings the model forbids in prose plus a destination
//! outside the catalog.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use super::support::{call, create, emulator, percent_encoded, Reply, REGION};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

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

/// One extended-metrics destination, as a case states it and as the service reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// An `Evidently` destination: the experiment that receives the metrics plus the role
    /// RUM writes to it through.
    fn evidently(experiment: &str, role: &str) -> Self {
        Self {
            destination: METRIC_DESTINATIONS[1].to_string(),
            destination_arn: Some(experiment_arn(experiment)),
            iam_role_arn: Some(role_arn(role)),
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

    /// The `MetricDestinationSummary` `ListRumMetricsDestinations` must report for it.
    fn summary(&self) -> Value {
        self.put_body()
    }

    /// How this case names the destination, for an assertion message.
    fn label(&self) -> String {
        match &self.destination_arn {
            None => self.destination.clone(),
            Some(arn) => format!("{}/{arn}", self.destination),
        }
    }
}

/// Create one app monitor to hang destinations off, through the shared harness.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    create(router, case, json!({"Name": name, "Domain": "example.com"})).await;
}

/// `PutRumMetricsDestination`, asserting it stored the destination.
async fn put_destination(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
) {
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(destination.put_body()),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumMetricsDestination({}) to answer 200, got {} {:?}",
        destination.label(),
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `DeleteRumMetricsDestination` for the `(Destination, DestinationArn)` pair
/// `destination`/`destination_arn` names, returning the reply so a case can assert on a
/// refusal as well as on a success. Both members are `@httpQuery`-bound, so they travel
/// in the query string, percent-encoded.
async fn delete_destination(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) -> Reply {
    let mut query = format!("destination={}", percent_encoded(destination));
    if let Some(arn) = destination_arn {
        query.push_str(&format!("&destinationArn={}", percent_encoded(arn)));
    }
    call(
        router,
        "DELETE",
        &format!("/rummetrics/{monitor}/metricsdestination?{query}"),
        None,
    )
    .await
}

/// `DeleteRumMetricsDestination` for a whole [`Destination`].
async fn delete(router: &MultiServiceRouter, monitor: &str, destination: &Destination) -> Reply {
    delete_destination(
        router,
        monitor,
        &destination.destination,
        destination.destination_arn.as_deref(),
    )
    .await
}

/// Every destination `ListRumMetricsDestinations` reports for `monitor`, read at the
/// modelled maximum page size so one call sees them all.
async fn reported(router: &MultiServiceRouter, case: &str, monitor: &str) -> Vec<Value> {
    let reply = call(
        router,
        "GET",
        &format!("/rummetrics/{monitor}/metricsdestination?maxResults=100"),
        None,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    body["Destinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Destinations in {body}"))
        .clone()
}

/// The destinations a case starts from: both `MetricDestination` kinds and several
/// Evidently experiments, so the removal table varies along the destination kind, the
/// experiment named and how many destinations are left.
fn stored_destinations() -> Vec<Destination> {
    vec![
        Destination::cloud_watch(),
        Destination::evidently("checkout-funnel", "rum-writer"),
        Destination::evidently("signup-funnel", "rum-writer-2"),
        Destination::evidently("search-latency", "rum-writer-3"),
    ]
}

#[tokio::test]
async fn delete_removes_only_the_destination_it_names() {
    let (router, _engine) = emulator();
    let monitor = "removal-monitor";
    create_monitor(&router, "setup", monitor).await;

    let stored = stored_destinations();
    for destination in &stored {
        put_destination(&router, "setup", monitor, destination).await;
    }

    // Delete them one at a time; after each delete the listing must report exactly the
    // destinations that were not named yet. The rule generalises over which kind is
    // removed and over how many are left, so the table is walked to exhaustion.
    let mut remaining = stored.clone();
    while !remaining.is_empty() {
        // Remove from the middle where there is one, so the deleted destination is not
        // always at an end of the stored order.
        let removed = remaining.remove(remaining.len() / 2);
        let case = removed.label();
        let case = case.as_str();

        let reply = delete(&router, monitor, &removed).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected DeleteRumMetricsDestination to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.json(case),
            json!({}),
            "case {case:?}: DeleteRumMetricsDestinationResponse models no members, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );

        let listed = reported(&router, case, monitor).await;
        assert_eq!(
            listed.len(),
            remaining.len(),
            "case {case:?}: expected {} destination(s) left, got {listed:?}",
            remaining.len()
        );
        assert!(
            !listed.contains(&removed.summary()),
            "case {case:?}: the deleted destination must not still be reported, got {listed:?}"
        );
        for survivor in &remaining {
            assert!(
                listed.contains(&survivor.summary()),
                "case {case:?}: deleting one destination must leave {} in place, got {listed:?}",
                survivor.label()
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_destination_the_monitor_does_not_have_is_not_found() {
    let (router, _engine) = emulator();
    let monitor = "absent-destination-monitor";
    create_monitor(&router, "setup", monitor).await;

    // One Evidently destination exists, so each refusal is about the pair the request
    // names rather than about a monitor with no destination at all.
    let present = Destination::evidently("present", "rum-writer");
    put_destination(&router, "setup", monitor, &present).await;

    // Every way a request can name a pair the monitor holds no record under: the other
    // destination kind, the right kind with an experiment it never had, and a pair that
    // WAS stored but has already been deleted (so a retried delete is not a silent
    // success either).
    let deleted = Destination::evidently("deleted", "rum-writer");
    put_destination(&router, "setup", monitor, &deleted).await;
    let reply = delete(&router, monitor, &deleted).await;
    assert_eq!(
        reply.status,
        200,
        "case \"first delete\": expected DeleteRumMetricsDestination to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    let rows: Vec<(&str, Destination)> = vec![
        ("the CloudWatch destination, never put", Destination::cloud_watch()),
        (
            "an Evidently experiment this monitor never named",
            Destination::evidently("never-put", "rum-writer"),
        ),
        ("an Evidently destination already deleted", deleted.clone()),
    ];
    for (case, missing) in rows {
        let reply = delete(&router, monitor, &missing).await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected HTTP 404 for {}, got {} {:?}",
            missing.label(),
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

    // None of the refused deletes touched the destination that IS stored.
    let listed = reported(&router, "after the refusals", monitor).await;
    assert_eq!(
        listed,
        vec![present.summary()],
        "case \"after the refusals\": a refused delete must remove nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_delete_pairing_the_model_forbids_is_refused_and_removes_nothing() {
    let (router, _engine) = emulator();
    let monitor = "delete-refusal-monitor";
    create_monitor(&router, "setup", monitor).await;

    let stored = [
        Destination::cloud_watch(),
        Destination::evidently("kept", "rum-writer"),
    ];
    for destination in &stored {
        put_destination(&router, "setup", monitor, destination).await;
    }

    // `DeleteRumMetricsDestinationRequest$DestinationArn`: "This parameter is required if
    // Destination is Evidently. If Destination is CloudWatch, do not use this parameter."
    // Source: `smithy_query(query="doc", shape="DeleteRumMetricsDestinationRequest",
    // member="DestinationArn")`. A destination outside the `MetricDestination` catalog is
    // refused the same way.
    let experiment = experiment_arn("kept");
    let rows: [(&str, &str, Option<&str>); 4] = [
        (
            "CloudWatch with a DestinationArn",
            METRIC_DESTINATIONS[0],
            Some(experiment.as_str()),
        ),
        ("Evidently with no DestinationArn", METRIC_DESTINATIONS[1], None),
        ("a destination outside the catalog", "Kinesis", None),
        (
            "a destination outside the catalog, with an ARN",
            "Kinesis",
            Some(experiment.as_str()),
        ),
    ];
    for (case, destination, destination_arn) in rows {
        let reply = delete_destination(&router, monitor, destination, destination_arn).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
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

    // A refused delete removed nothing: both destinations are still reported.
    let listed = reported(&router, "after the refusals", monitor).await;
    assert_eq!(
        listed.len(),
        stored.len(),
        "case \"after the refusals\": expected both destinations to survive, got {listed:?}"
    );
    for destination in &stored {
        assert!(
            listed.contains(&destination.summary()),
            "case \"after the refusals\": {} must still be reported, got {listed:?}",
            destination.label()
        );
    }
}

#[tokio::test]
async fn deleting_a_destination_of_an_unknown_app_monitor_is_not_found() {
    let (router, _engine) = emulator();
    // One monitor exists, so the refusal is about the name the request names rather than
    // about an empty store.
    create_monitor(&router, "setup", "present-monitor").await;
    put_destination(
        &router,
        "setup",
        "present-monitor",
        &Destination::cloud_watch(),
    )
    .await;

    // The rule holds for either destination kind, and a stored destination of ANOTHER
    // monitor is not reachable through a name that names no monitor.
    let rows: Vec<(&str, Destination)> = vec![
        ("CloudWatch", Destination::cloud_watch()),
        (
            "Evidently",
            Destination::evidently("absent-monitor-experiment", "rum-writer"),
        ),
    ];
    for (case, destination) in rows {
        let reply = delete(&router, "absent-monitor", &destination).await;
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

    // The monitor that does exist kept its destination.
    let listed = reported(&router, "present monitor", "present-monitor").await;
    assert_eq!(
        listed,
        vec![Destination::cloud_watch().summary()],
        "case \"present monitor\": a delete against another name must change nothing, got \
         {listed:?}"
    );
}

#[tokio::test]
async fn one_monitors_delete_leaves_another_monitors_destinations_alone() {
    let (router, _engine) = emulator();
    // Two monitors hold destinations under the SAME keys, so a delete that leaked across
    // monitors would be visible.
    let monitors = ["first-monitor", "second-monitor"];
    let stored = [
        Destination::cloud_watch(),
        Destination::evidently("shared-experiment", "rum-writer"),
    ];
    for monitor in monitors {
        create_monitor(&router, "setup", monitor).await;
        for destination in &stored {
            put_destination(&router, "setup", monitor, destination).await;
        }
    }

    for destination in &stored {
        let case = format!("delete {} from {}", destination.label(), monitors[0]);
        let case = case.as_str();
        let reply = delete(&router, monitors[0], destination).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected DeleteRumMetricsDestination to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let other = reported(&router, case, monitors[1]).await;
        assert_eq!(
            other.len(),
            stored.len(),
            "case {case:?}: the other monitor must keep every destination, got {other:?}"
        );
        assert!(
            other.contains(&destination.summary()),
            "case {case:?}: {} must still be reported for {}, got {other:?}",
            destination.label(),
            monitors[1]
        );
    }

    let emptied = reported(&router, "emptied monitor", monitors[0]).await;
    assert!(
        emptied.is_empty(),
        "case \"emptied monitor\": every destination of {} was deleted, got {emptied:?}",
        monitors[0]
    );
}

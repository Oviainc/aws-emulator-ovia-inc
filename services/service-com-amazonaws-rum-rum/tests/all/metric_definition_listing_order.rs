// SPDX-License-Identifier: CC0-1.0

//! The ORDER `BatchGetRumMetricDefinitions` reports one extended-metrics destination's
//! metric definitions in.
//!
//! The rule under test: the listing enumerates a destination's definitions in the order
//! they were CREATED — position N of the listing is the Nth definition created — and that
//! position is settled when the definition is created: it does not depend on the
//! `MetricDefinitionId` the service minted for it, it does not move when another
//! definition is deleted, and it is the same on every call and at every page size, so
//! paging to exhaustion concatenates exactly the creation order. A definition created
//! after a delete is the newest, so it is reported last.
//!
//! The model declares no sort order for the operation
//! (`smithy_query(query="operation", operation="BatchGetRumMetricDefinitions")` — `@readonly`,
//! `@paginated` over `MetricDefinitions`, with no ordering trait), and this listing is what
//! a caller correlates a destination's definitions by position in; it is also the only way
//! to learn their ids, because `BatchCreateRumMetricDefinitions` reports none.
//!
//! The tables span what the rule generalises over: how many definitions there are and in
//! how many batches they were created, whether their names sort the other way round from
//! their creation order, which `MetricDestination` holds them, which definitions a delete
//! removes (none, the first, a middle one, the last, several at once, and one id the
//! destination holds nothing under), and which page size the listing is read at. Every call
//! goes over the real wire path.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    batch_create_metric_definitions, call, create, emulator, experiment_arn,
    metrics_destination_query, percent_encoded, put_metrics_destination,
};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` — an enum of
/// `CloudWatch` and `Evidently`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// The page sizes every ordered read is taken at: one entry at a time, a size that splits
/// the store unevenly, a size larger than the store, and no `MaxResults` at all.
const PAGE_SIZES: [Option<usize>; 4] = [Some(1), Some(2), Some(100), None];

/// A custom metric definition named `name`.
///
/// `Name` is the member that tells the seeded definitions apart, so the order a listing
/// reports is readable from the names it carries; `Namespace` makes each of them a custom
/// metric, which admits any name at all.
fn definition(name: &str) -> Value {
    json!({ "Name": name, "Namespace": "Listing/Order" })
}

/// The `(Destination, DestinationArn)` pair the row's definitions live under, as the query
/// string every metrics call binds it to.
fn target(destination: &str, destination_arn: Option<&str>) -> String {
    metrics_destination_query(destination, destination_arn)
}

/// Create `names` as ONE batch under that destination, asserting the batch refused none.
async fn create_batch(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    names: &[&str],
) {
    let definitions: Vec<Value> = names.iter().map(|name| definition(name)).collect();
    let reply =
        batch_create_metric_definitions(router, monitor, destination, destination_arn, &definitions)
            .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected BatchCreateRumMetricDefinitions to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {case:?}: expected the seed to refuse no definition, got {answered}"
    );
}

/// Every definition the destination holds, in the order the listing reports them, read by
/// paging at `page_size` to exhaustion.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
    page_size: Option<usize>,
) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut path = format!("/rummetrics/{monitor}/metrics?{query}");
        if let Some(size) = page_size {
            path.push_str(&format!("&maxResults={size}"));
        }
        if let Some(issued) = &token {
            path.push_str(&format!("&nextToken={}", percent_encoded(issued)));
        }
        let reply = call(router, "GET", &path, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected a page to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        reported.extend(
            page["MetricDefinitions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a page reported no MetricDefinitions"))
                .clone(),
        );
        match page["NextToken"].as_str() {
            Some(issued) => token = Some(issued.to_string()),
            None => return reported,
        }
    }
}

/// The `Name` of every reported definition, in the order they were reported.
fn names(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|definition| {
            definition["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("a reported definition carries no Name: {definition}"))
                .to_string()
        })
        .collect()
}

/// The `MetricDefinitionId` of every reported definition, in the order they were reported.
fn ids(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|definition| {
            definition["MetricDefinitionId"]
                .as_str()
                .unwrap_or_else(|| panic!("a reported definition carries no id: {definition}"))
                .to_string()
        })
        .collect()
}

/// Assert that reading the destination at EVERY page size reports exactly `expected`, in
/// that order — the creation order, concatenated across pages.
async fn assert_order(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
    expected: &[String],
) {
    for page_size in PAGE_SIZES {
        let reported = listed(router, case, monitor, query, page_size).await;
        let reported_names = names(&reported);
        assert_eq!(
            &reported_names, expected,
            "case {case:?}: at page size {page_size:?}, expected {expected:?}, got \
             {reported_names:?}"
        );
    }
}

/// `BatchDeleteRumMetricDefinitions`: `DELETE /rummetrics/{AppMonitorName}/metrics` with the
/// ids bound to the repeated `@httpQuery("metricDefinitionIds")` parameter.
async fn batch_delete(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
    ids: &[String],
) {
    let mut path = format!("/rummetrics/{monitor}/metrics?{query}");
    for id in ids {
        path.push_str(&format!("&metricDefinitionIds={}", percent_encoded(id)));
    }
    let reply = call(router, "DELETE", &path, None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected BatchDeleteRumMetricDefinitions to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// Seed one app monitor carrying one destination, and answer its name and that
/// destination's query string.
async fn seeded(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
) -> (String, String) {
    create(
        router,
        case,
        json!({ "Name": monitor, "Domain": "example.com" }),
    )
    .await;
    // An Evidently destination is identified by the experiment ARN that receives the
    // metrics; a CloudWatch one carries none.
    let destination_arn = (destination == "Evidently").then(|| experiment_arn(monitor));
    put_metrics_destination(router, case, monitor, destination, destination_arn.as_deref()).await;
    (
        monitor.to_string(),
        target(destination, destination_arn.as_deref()),
    )
}

/// RULE: the listing reports a destination's definitions in the order they were created,
/// whatever ids the service minted for them, whatever batches they arrived in and whatever
/// page size the listing is read at — and the order is the same on a second reading.
#[tokio::test]
async fn the_listing_reports_a_destinations_definitions_in_creation_order() {
    // (case, the batches the definitions arrive in, in order)
    let rows: Vec<(&str, Vec<Vec<&str>>)> = vec![
        ("one definition", vec![vec!["Solo"]]),
        ("three in one batch", vec![vec!["MetricA", "MetricB", "MetricC"]]),
        (
            "three, one batch each",
            vec![vec!["MetricA"], vec!["MetricB"], vec!["MetricC"]],
        ),
        (
            "an uneven pair of batches",
            vec![vec!["First"], vec!["Second", "Third", "Fourth"]],
        ),
        (
            // Names whose codepoint order is the REVERSE of the creation order, so a
            // listing that sorted by any member of the definition would be caught.
            "names that sort the other way round",
            vec![vec!["zeta", "mid", "alpha"]],
        ),
        (
            "more definitions than one page can hold",
            vec![vec!["m0", "m1", "m2", "m3", "m4", "m5", "m6"]],
        ),
    ];

    for (index, (case, batches)) in rows.iter().enumerate() {
        for destination in METRIC_DESTINATIONS {
            let (router, _engine) = emulator();
            let case = format!("{case} ({destination})");
            let (monitor, query) = seeded(
                &router,
                &case,
                &format!("creation-order-{index}-{destination}"),
                destination,
            )
            .await;

            let mut created: Vec<String> = Vec::new();
            for batch in batches {
                let destination_arn = (destination == "Evidently").then(|| experiment_arn(&monitor));
                create_batch(
                    &router,
                    &case,
                    &monitor,
                    destination,
                    destination_arn.as_deref(),
                    batch,
                )
                .await;
                created.extend(batch.iter().map(|name| (*name).to_string()));
            }

            assert_order(&router, &case, &monitor, &query, &created).await;
            // A read changes nothing, so a second reading reports the same order — and the
            // ids at each position are the same ones.
            let first = listed(&router, &case, &monitor, &query, None).await;
            let again = listed(&router, &case, &monitor, &query, None).await;
            assert_eq!(
                ids(&first),
                ids(&again),
                "case {case:?}: expected a second reading to report the same ids in the same \
                 order, got {:?} then {:?}",
                ids(&first),
                ids(&again)
            );
            // The order is the creation order and not the minted-id order: with more than
            // one definition the two agree only by accident, so assert the position of each
            // definition is the one its creation gave it rather than its id's rank.
            let mut by_id = ids(&first);
            by_id.sort();
            assert_eq!(
                names(&first),
                created,
                "case {case:?}: expected the creation order {created:?}, got {:?} (ids sorted: \
                 {by_id:?})",
                names(&first)
            );
        }
    }
}

/// RULE (the same order, under deletion): a delete removes the definitions it names and
/// moves no other definition's position, so the listing still reports the survivors in
/// creation order — and a definition created afterwards is the newest, so it is reported
/// last. The recorded partial-delete case is one row: a batch naming ids the destination
/// holds plus one it holds nothing under.
#[tokio::test]
async fn a_delete_moves_no_surviving_definitions_position() {
    const SEEDED: [&str; 5] = ["MetricA", "MetricB", "MetricC", "MetricD", "MetricE"];

    // (case, the indices into SEEDED the delete names, whether it also names an id the
    // destination holds nothing under)
    let rows: Vec<(&str, Vec<usize>, bool)> = vec![
        ("nothing deleted", vec![], false),
        ("the first", vec![0], false),
        ("a middle one", vec![2], false),
        ("the last", vec![4], false),
        ("the first two", vec![0, 1], false),
        ("two either side of a survivor", vec![1, 3], false),
        ("every one but the last", vec![0, 1, 2, 3], false),
        // The recorded shape of the partial delete: valid ids plus one that is not.
        ("a partial batch, named out of order", vec![3, 0], true),
    ];

    for (index, (case, deleted, with_unknown)) in rows.iter().enumerate() {
        for destination in METRIC_DESTINATIONS {
            let (router, _engine) = emulator();
            let case = format!("{case} ({destination})");
            let (monitor, query) = seeded(
                &router,
                &case,
                &format!("delete-order-{index}-{destination}"),
                destination,
            )
            .await;
            let destination_arn = (destination == "Evidently").then(|| experiment_arn(&monitor));
            create_batch(
                &router,
                &case,
                &monitor,
                destination,
                destination_arn.as_deref(),
                &SEEDED,
            )
            .await;

            let before = listed(&router, &case, &monitor, &query, None).await;
            let held_ids = ids(&before);
            assert_eq!(
                names(&before),
                SEEDED.map(str::to_string).to_vec(),
                "case {case:?}: the seed must be listed in creation order, got {:?}",
                names(&before)
            );

            let mut named: Vec<String> = deleted.iter().map(|at| held_ids[*at].clone()).collect();
            if *with_unknown {
                // An id this destination holds nothing under: well-formed, and not one of
                // the ids the seed minted.
                named.push("00000000-0000-4000-8000-000000000000".to_string());
            }
            batch_delete(&router, &case, &monitor, &query, &named).await;

            let survivors: Vec<String> = SEEDED
                .iter()
                .enumerate()
                .filter(|(at, _)| !deleted.contains(at))
                .map(|(_, name)| (*name).to_string())
                .collect();
            assert_order(&router, &case, &monitor, &query, &survivors).await;

            // A definition created after the delete is the newest, so it is reported last —
            // it does not take the position a deleted definition left behind.
            create_batch(
                &router,
                &case,
                &monitor,
                destination,
                destination_arn.as_deref(),
                &["AddedLast"],
            )
            .await;
            let mut expected = survivors.clone();
            expected.push("AddedLast".to_string());
            assert_order(&router, &case, &monitor, &query, &expected).await;
        }
    }
}

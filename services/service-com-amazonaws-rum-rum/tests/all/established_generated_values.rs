// SPDX-License-Identifier: CC0-1.0

//! What a READ reports for the values this service GENERATES for a resource.
//!
//! The rule under test: a value the service chose for a resource when it created it is
//! established once and reported unchanged by every later read of that resource — a read
//! after the resource's configuration has been changed reports the same value as a read
//! taken before it, and a read establishes nothing of its own. Two such values are
//! exercised end to end, because they are the two a caller addresses other resources by:
//!
//! * the vended CloudWatch Logs group an app monitor advertises as
//!   `DataStorage.CwLog.CwLogGroup` — "The name of the log group where the copies are
//!   stored" (`smithy_query(query="doc", shape="CwLog")`), whose service-chosen part no
//!   caller can rebuild from the request, so a client that stored the name from one read
//!   must find the same name on the next one, whatever it has changed about the monitor
//!   meanwhile;
//! * the POSITION a metric definition holds in its destination's listing, which is the
//!   order `BatchGetRumMetricDefinitions` enumerates a destination's definitions in
//!   (`smithy_query(query="operation", operation="BatchGetRumMetricDefinitions")` declares
//!   `@readonly` and `@paginated` over `MetricDefinitions` and no sort order): position N
//!   is settled by the create that made the definition, so replacing a definition's content
//!   through `UpdateRumMetricDefinition` — the one operation that re-writes a stored
//!   definition — must leave it where it is rather than moving it to the end or
//!   re-ordering the listing around it.
//!
//! The tables span what the rule generalises over. For the monitor: every member
//! `UpdateAppMonitor` can change (`smithy_query(query="operation",
//! operation="UpdateAppMonitor")` — `Domain`, `DomainList`, `AppMonitorConfiguration`,
//! `CustomEvents`, `DeobfuscationConfiguration`, `CwLogEnabled`), a change made twice, and
//! a data-plane write (`PutRumEvents`) that copies telemetry into the vended group. For the
//! definitions: which position of the listing is replaced (the first, a middle one, the
//! last, every one of them, and one replaced twice), read back at each page size.
//!
//! Sibling suites own the neighbouring rules: `vended_cw_log_group` pins the SHAPE of the
//! vended name, that the group exists in CloudWatch Logs and that a monitor storing no
//! copies advertises none; `metric_definition_listing_order` pins that the listing is in
//! creation order and that a delete moves no survivor.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    assert_metric_definition, call, create, created_metric_definitions, emulator, get_app_monitor,
    metric_definition_id, metric_definitions_page, metrics_destination_query, percent_encoded,
    put_metrics_destination, reported_metric_definitions, rum_events_body,
    update_metric_definition,
};

/// The prefix a RUM-vended log group's name carries: RUM is a vended-logs service.
const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// The `MetricDestination` the ordered definitions of this suite live under.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` — `CloudWatch` or
/// `Evidently`.
const CLOUD_WATCH: &str = "CloudWatch";

/// The namespace the seeded definitions are published to, which makes each of them a custom
/// metric and so admits any `Name` at all.
const NAMESPACE: &str = "Established/Values";

/// The `CwLogGroup` `GetAppMonitor` reports for `name`, or `None` when it reports none.
async fn advertised_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Option<String> {
    get_app_monitor(router, case, name).await["DataStorage"]["CwLog"]["CwLogGroup"]
        .as_str()
        .map(str::to_string)
}

/// The same read, asserting the monitor advertises a group at all.
async fn vended_group(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    advertised_group(router, case, name)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: a monitor with delivery on advertises a group"))
}

/// `UpdateAppMonitor` carrying `patch`, asserting it was accepted.
async fn patch_monitor(router: &MultiServiceRouter, case: &str, name: &str, patch: &Value) {
    let reply = call(
        router,
        "PATCH",
        &format!("/appmonitor/{name}"),
        Some(patch.clone()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected UpdateAppMonitor {patch} to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `PutRumEvents` for the monitor whose id is `id`, asserting the batch was accepted. The
/// data plane addresses a monitor by its minted `Id` rather than by its name
/// (`POST /appmonitors/{Id}/`).
async fn deliver_telemetry(router: &MultiServiceRouter, case: &str, id: &str) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(id)),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// Assert `advertised` is a vended name for the monitor named `name`: the vended-logs
/// prefix, that name, and then a part the service chose, which is what makes the value one
/// a reader can only have learnt from a read.
fn assert_not_rebuildable(case: &str, name: &str, advertised: &str) {
    let from_the_request = format!("{CW_LOG_GROUP_PREFIX}{name}");
    assert!(
        advertised.starts_with(&from_the_request),
        "case {case:?}: expected the advertised group to start with {from_the_request:?}, got \
         {advertised:?}"
    );
    assert!(
        advertised.len() > from_the_request.len(),
        "case {case:?}: the vended name carries a part the service chose, so it cannot be \
         exactly {from_the_request:?}, got {advertised:?}"
    );
}

/// A custom metric definition named `name`.
fn definition(name: &str) -> Value {
    json!({"Name": name, "Namespace": NAMESPACE})
}

/// Every definition the monitor's CloudWatch destination holds, IN THE ORDER the listing
/// reports them, read at `page_size` and following `NextToken` to exhaustion.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    page_size: Option<usize>,
) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = metrics_destination_query(CLOUD_WATCH, None);
        if let Some(size) = page_size {
            query.push_str(&format!("&maxResults={size}"));
        }
        if let Some(issued) = &token {
            query.push_str(&format!("&nextToken={}", percent_encoded(issued)));
        }
        let reply = metric_definitions_page(router, monitor, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected BatchGetRumMetricDefinitions to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        reported.extend(reported_metric_definitions(&page));
        match page["NextToken"].as_str() {
            Some(issued) => token = Some(issued.to_string()),
            None => break,
        }
    }
    reported
}

/// The `MetricDefinitionId`s of a listing, in the order it reported them.
fn ids(case: &str, reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|entry| metric_definition_id(case, entry))
        .collect()
}

/// The page sizes every ordered read is taken at: one entry at a time, a size that splits
/// the store unevenly, a size past the store, and no `MaxResults` at all.
/// Source: `smithy_query(query="operation", operation="BatchGetRumMetricDefinitions")` —
/// `MaxResults` with `NextToken`.
const PAGE_SIZES: [Option<usize>; 4] = [Some(1), Some(2), Some(100), None];

#[tokio::test]
async fn every_change_to_a_monitor_leaves_the_group_it_vended_alone() {
    // Each row: the case, the `UpdateAppMonitor` patches applied in order, and whether a
    // batch of telemetry is delivered to the monitor afterwards. The patches span the
    // members the update can change, plus the same change made twice and the delivery
    // toggle, which is the one change that alters whether the name is reported at all.
    let rows: [(&str, &[Value], bool); 8] = [
        (
            "the single domain replaced",
            &[json!({"Domain": "changed.example.com"})],
            false,
        ),
        (
            "a domain list replacing the single domain",
            &[json!({"DomainList": ["a.example.com", "b.example.com"]})],
            false,
        ),
        (
            "the collected telemetry and the sampled portion of sessions",
            &[json!({
                "AppMonitorConfiguration": {
                    "SessionSampleRate": 0.5,
                    "Telemetries": ["errors", "performance"],
                    "AllowCookies": true,
                }
            })],
            false,
        ),
        (
            "custom events enabled and disabled again",
            &[
                json!({"CustomEvents": {"Status": "ENABLED"}}),
                json!({"CustomEvents": {"Status": "DISABLED"}}),
            ],
            false,
        ),
        (
            "source-map deobfuscation enabled",
            &[json!({
                "DeobfuscationConfiguration": {
                    "JavaScriptSourceMaps": {
                        "Status": "ENABLED",
                        "S3Uri": "s3://rum-source-maps/maps",
                    }
                }
            })],
            false,
        ),
        (
            "delivery turned off and on again",
            &[
                json!({"CwLogEnabled": false}),
                json!({"CwLogEnabled": true}),
            ],
            false,
        ),
        (
            "the same change made twice",
            &[
                json!({"Domain": "twice.example.com"}),
                json!({"Domain": "twice.example.com"}),
            ],
            false,
        ),
        ("a batch of telemetry delivered", &[], true),
    ];

    for (index, (case, patches, delivers_telemetry)) in rows.into_iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("established-monitor-{index}");
        let id = create(
            &router,
            case,
            json!({"Name": name, "Domain": "example.com", "CwLogEnabled": true}),
        )
        .await;

        // The value the create established, which is all a client has to go on.
        let established = vended_group(&router, case, &name).await;
        assert_not_rebuildable(case, &name, &established);

        let mut delivery_on = true;
        for patch in patches {
            patch_monitor(&router, case, &name, patch).await;
            if let Some(enabled) = patch["CwLogEnabled"].as_bool() {
                delivery_on = enabled;
            }
            let advertised = advertised_group(&router, case, &name).await;
            if delivery_on {
                assert_eq!(
                    advertised.as_deref(),
                    Some(established.as_str()),
                    "case {case:?}: after {patch} the monitor must still report the group it \
                     was vended ({established:?}), got {advertised:?}"
                );
            } else {
                // A monitor that stores no copies names no group; the name it was vended is
                // not re-chosen while delivery is off, which the row's next patch shows.
                assert_eq!(
                    advertised, None,
                    "case {case:?}: after {patch} the monitor stores no copies, so it names no \
                     group, got {advertised:?}"
                );
            }
        }

        if delivers_telemetry {
            deliver_telemetry(&router, case, &id).await;
        }

        // A read establishes nothing: two further reads report the one value the create
        // chose, whatever this row did to the monitor in between.
        for read in 1..=2 {
            let again = vended_group(&router, case, &name).await;
            assert_eq!(
                again, established,
                "case {case:?}: read {read} after the change must report {established:?}, got \
                 {again:?}"
            );
        }
    }
}

#[tokio::test]
async fn replacing_a_definition_moves_no_position_in_the_listing() {
    // The definitions every row seeds, in creation order; the row then replaces the
    // definition at each of `replaced` by a definition of a metric nothing else defines.
    let seeded = ["Alpha", "Bravo", "Charlie", "Delta"];
    // Each row: the case and the positions replaced, in the order they are replaced — the
    // first, a middle one, the last, every one of them, and one position replaced twice.
    let rows: [(&str, &[usize]); 6] = [
        ("nothing replaced", &[]),
        ("the first definition replaced", &[0]),
        ("a middle definition replaced", &[1]),
        ("the last definition replaced", &[3]),
        ("every definition replaced, in order", &[0, 1, 2, 3]),
        ("one definition replaced twice", &[2, 2]),
    ];

    for (case, replaced) in rows {
        let (router, _engine) = emulator();
        let monitor = "established-definitions";
        create(
            &router,
            case,
            json!({"Name": monitor, "Domain": "example.com"}),
        )
        .await;
        put_metrics_destination(&router, case, monitor, CLOUD_WATCH, None).await;
        let definitions: Vec<Value> = seeded.iter().map(|name| definition(name)).collect();
        created_metric_definitions(&router, case, monitor, CLOUD_WATCH, None, &definitions).await;

        // The order and the ids the create established, read through the operation that
        // reports them: position N of the listing is the Nth definition created, which is
        // what makes a position a value the create established rather than a function of
        // the ids the service minted.
        let before = listed(&router, case, monitor, None).await;
        let established_ids = ids(case, &before);
        assert_eq!(
            established_ids.len(),
            seeded.len(),
            "case {case:?}: expected the listing to report {} definitions, got {before:?}",
            seeded.len()
        );
        let created_order: Vec<&str> = seeded.to_vec();
        let reported_order: Vec<&str> = before
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported_order, created_order,
            "case {case:?}: expected the listing to enumerate the definitions in creation \
             order {created_order:?}, got {reported_order:?}"
        );

        // What each position is expected to hold once this row's replacements are done:
        // the definition last written to it, which for an untouched position is the one it
        // was created with.
        let mut expected: Vec<Value> = definitions.clone();
        for (round, position) in replaced.iter().enumerate() {
            let replacement = definition(&format!("{}-v{}", seeded[*position], round + 1));
            update_metric_definition(
                &router,
                case,
                monitor,
                CLOUD_WATCH,
                None,
                &established_ids[*position],
                &replacement,
            )
            .await;
            expected[*position] = replacement;
        }

        for page_size in PAGE_SIZES {
            let after = listed(&router, case, monitor, page_size).await;
            assert_eq!(
                ids(case, &after),
                established_ids,
                "case {case:?} at page size {page_size:?}: replacing a definition moves no \
                 position, so the listing must report the same ids in the same order, got \
                 {after:?}"
            );
            for (position, reported) in after.iter().enumerate() {
                assert_metric_definition(
                    case,
                    reported,
                    &expected[position],
                    &established_ids[position],
                );
            }
        }
    }
}

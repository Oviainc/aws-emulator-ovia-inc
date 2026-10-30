// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `DeleteAppMonitor` establishes for every LATER call that
//! names the monitor it removed.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` the monitor is addressed by, the
//! modelled constraints, the handler and the error envelope are all exercised rather than a
//! handler body.
//!
//! The RULE the assertions pin, over a SET of inputs rather than one request: a delete
//! removes the app monitor from every index this service resolves it through, so each later
//! call is refused by the rule its OWN addressing establishes — a call that resolves the
//! monitor by `Name` or by its ARN answers the not-found refusal (HTTP 404), while the
//! data-plane write addressed by the minted `Id`, which is authorized AGAINST the monitor
//! that id has to resolve to, has no resource left for the grant to be established against
//! and answers `AccessDeniedException` (HTTP 403), disclosing nothing about whether such a
//! monitor exists.
//!
//! The rows differ along what the rule generalises over: the addressing form, the operation
//! within each form (eleven name-addressed, three ARN-addressed, the one id-addressed), the
//! spelling of the removed monitor's name, and — in the second case — whether the name has
//! since been re-created under a NEW id. The monitor that is never deleted, and the calls
//! that keep succeeding against it, are what show the refusals are driven by the removed
//! record alone rather than by the request's shape.

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::{
    CALLER_LACKS_PERMISSIONS_MESSAGE, NOT_FOUND_MESSAGE_PREFIX, TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    call, create, emulator, get_app_monitor, message, percent_encoded, rum_events_body, ACCOUNT,
    REGION,
};

/// How a later call names the monitor the delete removed.
///
/// The refusal is a property of the ADDRESSING rather than of the operation: `Name` and
/// `Arn` resolve a stored record and report that nothing answers to that identifier, while
/// `Id` resolves the monitor the data-plane write is authorized against.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    Name,
    Arn,
    Id,
}

/// One later call: the operation, how it addresses the monitor, and the request itself.
struct Probe {
    operation: &'static str,
    addressing: Addressing,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// The HTTP status, the wire error code and the whole `message` an addressing form is
/// refused with once the monitor is gone.
fn expected_refusal(addressing: Addressing, name: &str) -> (u16, &'static str, String) {
    match addressing {
        Addressing::Name => (
            404,
            "ResourceNotFoundException",
            format!("{NOT_FOUND_MESSAGE_PREFIX}{name}"),
        ),
        // The tagging frontend answers the SAME `ResourceNotFoundException` shape and
        // status under the shorter wire code it puts in `__type`
        // (`crate::ops::tag_resource`'s `NOT_FOUND_ERROR_CODE`, which the recorded tagging
        // units pin and `resource_tagging.rs` already covers for an ARN naming nothing).
        Addressing::Arn => (
            404,
            "NotFoundException",
            TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        ),
        Addressing::Id => (
            403,
            "AccessDeniedException",
            CALLER_LACKS_PERMISSIONS_MESSAGE.to_string(),
        ),
    }
}

/// The ARN `CreateAppMonitor` mints for the monitor named `name`, as the tagging
/// operations address it.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// A well-formed IAM JSON policy, so `PutResourcePolicy` reaches its app-monitor
/// resolution rather than being refused on the document.
fn policy_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
            "Action": "rum:PutRumEvents",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// Every way a later call can name one app monitor, as the requests that do it.
///
/// Source: `services/service-com-amazonaws-rum-rum/OPERATIONS.md` — the route and the
/// required members of each of the service's twenty operations. Covered here is every
/// operation that resolves ONE app monitor: the eleven that bind its `Name` as an
/// `@httpLabel`, the three tagging calls that bind its `ResourceArn`, and the data-plane
/// write that binds its `Id`. `CreateAppMonitor` and `ListAppMonitors` name no existing
/// monitor, so neither has a refusal for this rule to establish.
fn probes(name: &str, id: &str) -> Vec<Probe> {
    let encoded = percent_encoded(name);
    let tags = format!("/tags/{}", percent_encoded(&arn_of(name)));
    let name_addressed: Vec<(&'static str, &'static str, String, Option<Value>)> = vec![
        ("GetAppMonitor", "GET", format!("/appmonitor/{encoded}"), None),
        (
            "UpdateAppMonitor",
            "PATCH",
            format!("/appmonitor/{encoded}"),
            Some(json!({"Domain": "other.example.com"})),
        ),
        (
            "DeleteAppMonitor",
            "DELETE",
            format!("/appmonitor/{encoded}"),
            None,
        ),
        (
            "GetAppMonitorData",
            "POST",
            format!("/appmonitor/{encoded}/data"),
            Some(json!({"TimeRange": {"After": 0}, "MaxResults": 10})),
        ),
        (
            "GetResourcePolicy",
            "GET",
            format!("/appmonitor/{encoded}/policy"),
            None,
        ),
        (
            "PutResourcePolicy",
            "PUT",
            format!("/appmonitor/{encoded}/policy"),
            Some(json!({"PolicyDocument": policy_document()})),
        ),
        (
            "DeleteResourcePolicy",
            "DELETE",
            format!("/appmonitor/{encoded}/policy"),
            None,
        ),
        (
            "PutRumMetricsDestination",
            "POST",
            format!("/rummetrics/{encoded}/metricsdestination"),
            Some(json!({"Destination": "CloudWatch"})),
        ),
        (
            "ListRumMetricsDestinations",
            "GET",
            format!("/rummetrics/{encoded}/metricsdestination"),
            None,
        ),
        (
            "DeleteRumMetricsDestination",
            "DELETE",
            format!("/rummetrics/{encoded}/metricsdestination?destination=CloudWatch"),
            None,
        ),
        (
            "BatchGetRumMetricDefinitions",
            "GET",
            format!("/rummetrics/{encoded}/metrics?destination=CloudWatch"),
            None,
        ),
    ];
    let arn_addressed: Vec<(&'static str, &'static str, String, Option<Value>)> = vec![
        (
            "TagResource",
            "POST",
            tags.clone(),
            Some(json!({"Tags": {"Team": "web"}})),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{tags}?tagKeys=Team"),
            None,
        ),
        ("ListTagsForResource", "GET", tags, None),
    ];

    let mut rows: Vec<Probe> = Vec::new();
    for (operation, method, path, body) in name_addressed {
        rows.push(Probe {
            operation,
            addressing: Addressing::Name,
            method,
            path,
            body,
        });
    }
    for (operation, method, path, body) in arn_addressed {
        rows.push(Probe {
            operation,
            addressing: Addressing::Arn,
            method,
            path,
            body,
        });
    }
    rows.push(Probe {
        operation: "PutRumEvents",
        addressing: Addressing::Id,
        method: "POST",
        path: format!("/appmonitors/{id}/"),
        body: Some(rum_events_body(id)),
    });
    rows
}

/// Drive one probe and assert it is refused the way its addressing establishes.
async fn assert_refused(router: &MultiServiceRouter, case: &str, probe: &Probe, name: &str) {
    let (status, code, expected) = expected_refusal(probe.addressing, name);
    let reply = call(router, probe.method, &probe.path, probe.body.clone()).await;
    assert_eq!(
        reply.status,
        status,
        "case {case:?}: expected HTTP {status}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.error_code(case);
    assert_eq!(
        answered, code,
        "case {case:?}: expected {code}, got {answered:?}"
    );
    let document = reply.json(case);
    assert_eq!(
        message(&document),
        Some(expected.as_str()),
        "case {case:?}: expected the message {expected:?}, got {:?}",
        message(&document)
    );
    // The data-plane refusal is the one that must disclose nothing: it is made because
    // there is no resource to decide against, so the id it was asked about is exactly what
    // it cannot report.
    if matches!(probe.addressing, Addressing::Id) {
        let rendered = document.to_string();
        let id = probe.path.trim_matches('/').replace("appmonitors/", "");
        assert!(
            !rendered.contains(&id),
            "case {case:?}: the refusal must not report the id it was asked about, got \
             {rendered:?}"
        );
    }
}

/// `PutRumEvents` against `id`, asserting the batch is ACCEPTED — the contrast every
/// refusal is read against.
async fn assert_events_accepted(router: &MultiServiceRouter, case: &str, id: &str) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(id)),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `DeleteAppMonitor` for `name`, asserting the delete itself succeeded.
async fn delete_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    let reply = call(
        router,
        "DELETE",
        &format!("/appmonitor/{}", percent_encoded(name)),
        None,
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected DeleteAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// Names that are all valid `AppMonitorName`s, so the probe reaches the handler rather
/// than being refused at the boundary.
///
/// Source: `smithy_query(query="shape", shape="AppMonitorName")` — `@length(min: 1, max:
/// 255)` and `@pattern ^(?!\.)[\.\-_#A-Za-z0-9]+$`. The rows walk both published length
/// bounds and the character classes the pattern admits: letters in both cases, digits, `.`,
/// `-`, `_` and `#`. The `#` row is worth having because `#` cannot travel unencoded in a
/// URI path, so the refusal has to report the DECODED name.
fn deleted_names() -> Vec<(&'static str, String)> {
    vec![
        ("shortest-admissible", "a".to_string()),
        ("longest-admissible", "x".repeat(255)),
        ("dots-dashes-underscores", "Deleted.Monitor-01_X".to_string()),
        ("hash-inside", "deleted#monitor".to_string()),
    ]
}

#[tokio::test]
async fn every_addressing_of_a_deleted_monitor_answers_its_own_refusal() {
    let (router, _engine) = emulator();
    // A monitor that is never deleted, so a refusal means "that record is gone" rather
    // than "this emulator holds nothing".
    let kept = create(
        &router,
        "kept",
        json!({"Name": "addressing-kept", "Domain": "kept.example.com"}),
    )
    .await;

    for (name_case, name) in deleted_names() {
        let id = create(
            &router,
            name_case,
            json!({"Name": name, "Domain": "example.com"}),
        )
        .await;
        // Before the delete the id is live, which is what makes the refusal below about
        // the delete rather than about the id never having resolved.
        assert_events_accepted(&router, name_case, &id).await;
        delete_monitor(&router, name_case, &name).await;

        for probe in probes(&name, &id) {
            let case = format!("{} after deleting {name_case}", probe.operation);
            assert_refused(&router, &case, &probe, &name).await;
        }
    }

    // The monitor that was never deleted is untouched: its name still resolves and its id
    // still accepts telemetry, so the refusals above were about the removed records alone.
    let monitor = get_app_monitor(&router, "kept", "addressing-kept").await;
    assert_eq!(
        monitor["Id"].as_str(),
        Some(kept.as_str()),
        "case \"kept\": the monitor that was never deleted must still be readable, got {:?}",
        monitor["Id"]
    );
    assert_events_accepted(&router, "kept", &kept).await;
}

#[tokio::test]
async fn recreating_the_name_restores_the_name_addressing_but_not_the_retired_id() {
    let (router, _engine) = emulator();
    let name = "addressing-reborn";
    let retired = create(
        &router,
        "first",
        json!({"Name": name, "Domain": "first.example.com"}),
    )
    .await;
    assert_events_accepted(&router, "first", &retired).await;
    delete_monitor(&router, "first", name).await;

    // Re-creating the name mints a NEW id, so the name resolves again while the id the
    // delete retired resolves to nothing: the refusal is driven by what the store holds
    // now, not by which identifiers it once minted.
    let current = create(
        &router,
        "second",
        json!({"Name": name, "Domain": "second.example.com"}),
    )
    .await;
    assert_ne!(
        current, retired,
        "case \"second\": the re-created monitor is a new resource, so its id must differ, \
         got {current:?} twice"
    );

    let monitor = get_app_monitor(&router, "second", name).await;
    assert_eq!(
        monitor["Domain"].as_str(),
        Some("second.example.com"),
        "case \"second\": the re-created monitor reports the second request's Domain, got {:?}",
        monitor["Domain"]
    );
    assert_events_accepted(&router, "second", &current).await;

    // The id-addressed probe is the one still refused, and still as an authorization
    // failure rather than as a missing resource: the monitor that id named is gone for
    // good, even though its name now holds a different one.
    let retired_probe = probes(name, &retired)
        .into_iter()
        .find(|probe| matches!(probe.addressing, Addressing::Id))
        .expect("the data-plane probe");
    assert_refused(
        &router,
        "PutRumEvents with the retired id",
        &retired_probe,
        name,
    )
    .await;

    // And the name-addressed and ARN-addressed reads resolve again, so nothing reports the
    // re-created monitor missing.
    let listed = call(
        &router,
        "GET",
        &format!("/tags/{}", percent_encoded(&arn_of(name))),
        None,
    )
    .await;
    assert_eq!(
        listed.status,
        200,
        "case \"ListTagsForResource after the name was re-created\": expected HTTP 200, got \
         {} {:?}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
}

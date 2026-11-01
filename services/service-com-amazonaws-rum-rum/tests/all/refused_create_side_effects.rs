// SPDX-License-Identifier: CC0-1.0

//! What a `CreateAppMonitor` this service REFUSES leaves behind in another service.
//!
//! The rule under test: a create answers either a stored monitor or a refusal, and a
//! refused one leaves NOTHING — the vended CloudWatch Logs log group RUM provisions for a
//! monitor created with `CwLogEnabled` on is created only for a create that is going to be
//! stored. The refusals RUM reads off its own store (the duplicate name, the per-region
//! app-monitor allowance) are decided, and the name and the slot they grant are held,
//! before the group is provisioned, so a create that loses the name to a concurrent create
//! is refused BEFORE the group exists rather than after it. The invariant a client can see
//! is therefore: the log groups under the vended-logs prefix are exactly the ones the
//! monitors that exist advertise, however many creates competed for one name.
//!
//! The rows span what the rule generalises over: how many creates compete for one name (so
//! the refusal is taken under contention, not only against a monitor that was already
//! there), whether the competing creates ask for log delivery at all, and the name they
//! compete for — including the punctuation `AppMonitorName`'s pattern admits, since the
//! group's name embeds the monitor's.
//!
//! The ALLOWED create, the group's name and the refusal's wording are pinned by
//! `cw_log_delivery.rs` and `vended_cw_log_group.rs`; what is pinned here is the side
//! effect a refused create must not have, and that a refused create leaves the name it
//! competed for usable afterwards.

use bytes::Bytes;
use serde_json::json;

use crate::support::{call, emulator, Reply, REGION};

/// The prefix a RUM-managed log group's name carries: RUM is a vended-logs service.
const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// The wording `ConflictException` carries for a name a monitor of the region already
/// holds, as AWS builds it: the literal `Resource `, the requested name, ` already exists`.
fn already_exists(name: &str) -> String {
    format!("Resource {name} already exists")
}

/// The `logGroupName`s the emulated CloudWatch Logs service reports under `prefix`.
///
/// It is mounted beside RUM by the shared harness, which is how a test sees the group RUM
/// provisioned in process.
async fn log_groups(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    case: &str,
    prefix: &str,
) -> Vec<String> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://logs.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "Logs_20140328.DescribeLogGroups")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/logs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&json!({"logGroupNamePrefix": prefix}))
                .expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let reply = Reply {
        status,
        body: response.into_body(),
        headers,
    };
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected DescribeLogGroups to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["logGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The `CwLogGroup` `GetAppMonitor` reports for `name`, or `None` when it reports none.
async fn advertised_group(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    case: &str,
    name: &str,
) -> Option<String> {
    let reply = call(router, "GET", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitor"]["DataStorage"]["CwLog"]["CwLogGroup"]
        .as_str()
        .map(str::to_string)
}

/// Fire `racers` `CreateAppMonitor` requests for ONE name CONCURRENTLY, each asking for log
/// delivery as `cw_log_enabled` says, and answer their replies.
///
/// The requests are spawned rather than awaited in turn, so they really do overlap: one of
/// them wins the name and the others reach their refusal while the winner is away
/// provisioning its log group — the window this suite is about.
async fn racing_creates(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    name: &str,
    racers: usize,
    cw_log_enabled: bool,
) -> Vec<Reply> {
    let mut racing = Vec::with_capacity(racers);
    for _ in 0..racers {
        let router = router.clone();
        let body = json!({
            "Name": name,
            "Domain": "example.com",
            "CwLogEnabled": cw_log_enabled,
        });
        racing.push(tokio::spawn(async move {
            call(&router, "POST", "/appmonitor", Some(body)).await
        }));
    }
    let mut replies = Vec::with_capacity(racers);
    for racer in racing {
        replies.push(racer.await.expect("a racing create runs to completion"));
    }
    replies
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_create_refused_under_contention_provisions_no_log_group() {
    // (case, the name every racer competes for, how many race for it, whether they ask for
    // CloudWatch Logs delivery)
    let rows: [(&str, &str, usize, bool); 5] = [
        (
            "two creates competing for one name, both asking for delivery",
            "raced-by-two",
            2,
            true,
        ),
        (
            "four creates competing for one name, all asking for delivery",
            "raced-by-four",
            4,
            true,
        ),
        (
            // The vended group's name embeds the monitor's, so the punctuation
            // `AppMonitorName`'s pattern admits must behave the same way.
            "eight creates competing for a name carrying the punctuation the model admits",
            "raced_by-eight.8",
            8,
            true,
        ),
        (
            "two creates competing for one name, neither asking for delivery",
            "raced-no-delivery",
            2,
            false,
        ),
        (
            // One create is no race: the single row where nothing is refused, so the
            // invariant is read against a create that was served.
            "a single create asking for delivery",
            "unraced",
            1,
            true,
        ),
    ];

    for (case, name, racers, delivery) in rows {
        let (router, _engine) = emulator();
        let replies = racing_creates(&router, name, racers, delivery).await;

        let served = replies
            .iter()
            .filter(|reply| reply.status == 200)
            .count();
        assert_eq!(
            served,
            1,
            "case {case:?}: exactly one of {racers} creates of one name may be served, got \
             {served} ({:?})",
            replies
                .iter()
                .map(|reply| (reply.status, String::from_utf8_lossy(&reply.body).to_string()))
                .collect::<Vec<_>>()
        );
        // Every other racer is refused as a duplicate name, in the modelled error and the
        // wording AWS builds from the request.
        for reply in replies.iter().filter(|reply| reply.status != 200) {
            assert_eq!(
                reply.status, 409,
                "case {case:?}: expected a refused create to answer HTTP 409, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let message = crate::support::message(&reply.json(case))
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                message,
                already_exists(name),
                "case {case:?}: expected the refusal {:?}, got {message:?}",
                already_exists(name)
            );
        }

        // The invariant: the groups that exist are exactly the ones the monitor that was
        // stored advertises. A refused racer provisioned none, so a group it had minted a
        // name for is nowhere.
        let advertised = advertised_group(&router, case, name).await;
        let expected: Vec<String> = advertised.clone().into_iter().collect();
        let groups = log_groups(&router, case, CW_LOG_GROUP_PREFIX).await;
        assert_eq!(
            groups, expected,
            "case {case:?}: expected the vended groups to be exactly what the stored monitor \
             advertises ({expected:?}), got {groups:?}"
        );
        assert_eq!(
            advertised.is_some(),
            delivery,
            "case {case:?}: expected the stored monitor to advertise a group only with \
             delivery on, got {advertised:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_a_create_competed_for_and_lost_is_usable_afterwards() {
    // The name a create holds while it provisions its log group is held for that call
    // only: once the race is over the name belongs to the monitor that was stored and to
    // nothing else, so deleting that monitor frees the name for a later create — the
    // refused racers left no claim of their own behind.
    let rows: [(&str, &str, usize); 3] = [
        ("a name two creates competed for", "reusable-by-two", 2),
        ("a name four creates competed for", "reusable-by-four", 4),
        ("a name one create took uncontested", "reusable-unraced", 1),
    ];

    for (case, name, racers) in rows {
        let (router, _engine) = emulator();
        let replies = racing_creates(&router, name, racers, true).await;
        let served = replies.iter().filter(|reply| reply.status == 200).count();
        assert_eq!(
            served, 1,
            "case {case:?}: exactly one of {racers} creates may be served, got {served}"
        );
        let first = advertised_group(&router, case, name).await.unwrap_or_else(|| {
            panic!("case {case:?}: the stored monitor must advertise a vended group")
        });

        let deleted = call(&router, "DELETE", &format!("/appmonitor/{name}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAppMonitor to answer 200, got {} {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );

        // The name is free again: a create under it is served rather than refused as a
        // duplicate, which is what a leaked claim from a refused racer would turn into.
        let recreated = call(
            &router,
            "POST",
            "/appmonitor",
            Some(json!({
                "Name": name,
                "Domain": "example.com",
                "CwLogEnabled": true,
            })),
        )
        .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: expected the name to be free after the delete, got {} {:?}",
            recreated.status,
            String::from_utf8_lossy(&recreated.body)
        );
        let second = advertised_group(&router, case, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the new monitor must advertise a group"));
        assert_ne!(
            second, first,
            "case {case:?}: a monitor created again under one name is a new resource and is \
             vended its own group, got {second:?} twice"
        );
        let groups = log_groups(&router, case, &second).await;
        assert_eq!(
            groups,
            vec![second.clone()],
            "case {case:?}: expected the group {second:?} the new monitor advertises to exist, \
             got {groups:?}"
        );
    }
}

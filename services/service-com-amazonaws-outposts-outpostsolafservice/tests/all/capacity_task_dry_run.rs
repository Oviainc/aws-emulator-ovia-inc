// SPDX-License-Identifier: CC0-1.0

//! What a DRY RUN of `StartCapacityTask` is answered with, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — and the
//! harness is [`crate::capacity_task_lifecycle`]'s, which owns what a started task
//! reports.
//!
//! The rules under test:
//!
//! 1. Resolving the resources a request names comes BEFORE the work the request asks
//!    for, so a reference that resolves to nothing is refused 404 `NotFoundException`
//!    with the identifier echo of the member it arrived in whatever else the request
//!    asked for — asking for a dry run does not change which refusal applies.
//! 2. The one thing a dry run asks for that this emulator cannot answer is the verdict
//!    itself — whether the requested pools are above or below the Outpost's available
//!    instance capacity, which nothing here models — so a dry run whose references all
//!    resolve is answered 501 `NotImplemented` and changes nothing: no task is stored,
//!    and the rules about a task being CREATED (one active task per Outpost and per
//!    order, the store's entry ceiling) are not applied to a call that creates none.
//!
//! The rows span what the rules generalise over: which member carries the reference
//! that resolves to nothing, which of the two identifier forms named the Outpost,
//! whether the other optional reference resolves, how many instance pools the request
//! carries, and — for every refused row — all three ways the defaulted `DryRun` member
//! can arrive (absent, false, true), none of which may change the refusal.

use serde_json::{json, Value};

use crate::capacity_task_lifecycle::{
    an_order, capacity_route, install, outpost, pool, start, start_ok, task_route, ABSENT_ASSET_ID,
    ABSENT_ORDER_ID, INSTALLED,
};
use crate::site_and_outpost_lifecycle::{text, Outposts, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX};

/// An `AssetId` of the modelled `AssetIdInput` form (`^\d{10}$`) recorded as hardware
/// installed on ANOTHER Outpost, so a row can name an asset that exists somewhere
/// without being hardware of the Outpost the task would run on.
const ELSEWHERE_ASSET_ID: &str = "2000000009";

/// How the defaulted `DryRun` member arrives: left out, sent false, or sent true.
/// Rule 1 holds for all three.
const DRY_RUN_FORMS: [(&str, Option<bool>); 3] = [
    ("the DryRun member left out", None),
    ("DryRun sent false", Some(false)),
    ("DryRun sent true", Some(true)),
];

/// `request` with the `DryRun` member as `form` says it arrives.
fn with_dry_run(request: &Value, form: Option<bool>) -> Value {
    let mut request = request.clone();
    if let Some(dry_run) = form {
        request["DryRun"] = json!(dry_run);
    }
    request
}

/// How many capacity tasks the store holds.
fn held(outposts: &Outposts) -> usize {
    outposts.store().lock().capacity_tasks.len()
}

/// Rule 1: a dry run is refused for a reference that resolves to nothing exactly as a
/// real start is, with the same status, the same modelled error and the same identifier
/// echo — the member's validation-path name and the value as supplied.
#[tokio::test]
async fn a_dry_run_is_refused_for_a_reference_that_resolves_to_nothing() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "dry-run-references").await;
    let elsewhere = outpost(&outposts, "dry-run-elsewhere").await;
    install(&outposts, &outpost_id, &INSTALLED, "dry-run-references");
    install(
        &outposts,
        &elsewhere,
        &[ELSEWHERE_ASSET_ID],
        "dry-run-elsewhere",
    );
    // An order the account really placed, so a row can name one reference that
    // resolves beside one that does not.
    let order_id = an_order(&outposts, &outpost_id).await;
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    // Each row: the Outpost the call is addressed to, the request without its `DryRun`
    // member, and the exact refusal body. The echoed member names are the
    // validation-path names of the modelled input members `OutpostIdentifier`,
    // `OrderId` and `AssetId`.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "an Outpost nothing holds, named by bare id",
            ABSENT_OUTPOST_ID,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            format!("outpostIdentifier={ABSENT_OUTPOST_ID}"),
        ),
        (
            "an Outpost nothing holds, named by full ARN",
            &absent_outpost_arn,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            format!("outpostIdentifier={absent_outpost_arn}"),
        ),
        (
            "an OrderId this store holds no order for",
            &outpost_id,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)], "OrderId": ABSENT_ORDER_ID }),
            format!("orderId={ABSENT_ORDER_ID}"),
        ),
        (
            "an OrderId that names nothing, beside an asset that resolves",
            &outpost_id,
            json!({
                "InstancePools": [pool("c5.xlarge", 4), pool("m5.large", 2)],
                "OrderId": ABSENT_ORDER_ID,
                "AssetId": INSTALLED[0],
            }),
            format!("orderId={ABSENT_ORDER_ID}"),
        ),
        (
            "an AssetId no Outpost of this store holds",
            &outpost_id,
            json!({ "InstancePools": [], "AssetId": ABSENT_ASSET_ID }),
            format!("assetId={ABSENT_ASSET_ID}"),
        ),
        (
            "an AssetId installed on another Outpost",
            &outpost_id,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)], "AssetId": ELSEWHERE_ASSET_ID }),
            format!("assetId={ELSEWHERE_ASSET_ID}"),
        ),
        (
            "an AssetId that names nothing, beside an order that resolves",
            &outpost_id,
            json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "OrderId": order_id,
                "AssetId": ABSENT_ASSET_ID,
            }),
            format!("assetId={ABSENT_ASSET_ID}"),
        ),
    ];

    for (case, outpost_identifier, request, expected) in &cases {
        for (form, dry_run) in DRY_RUN_FORMS {
            let case = format!("{case} ({form})");
            let reply = start(
                &outposts,
                outpost_identifier,
                &with_dry_run(request, dry_run),
            )
            .await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: expected 404 NotFoundException, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                &text(&reply.body, "Message"),
                expected,
                "case {case:?}: expected the identifier echo {expected:?}, got {}",
                reply.body
            );
        }
    }

    // No row created anything: a refused start — dry run or not — stores no task.
    let stored = held(&outposts);
    assert_eq!(
        stored, 0,
        "a refused start must store no capacity task: expected 0 held, got {stored}"
    );
}

/// Rule 2: a dry run whose references all resolve is answered 501 `NotImplemented` —
/// the verdict it asks for is about installed instance capacity nothing here models —
/// and it changes nothing a later caller can see.
#[tokio::test]
async fn a_dry_run_that_resolves_is_not_implemented_and_changes_nothing() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "dry-run-verdict").await;
    install(&outposts, &outpost_id, &INSTALLED, "dry-run-verdict");
    let order_id = an_order(&outposts, &outpost_id).await;

    // The rows differ along what the request can carry: how many pools (none, one,
    // several), which optional references are named, and whether the caller said what
    // to do about blocking instances.
    let cases: Vec<(&str, Value)> = vec![
        (
            "one pool and no optional member",
            json!({ "InstancePools": [pool("m5.4xlarge", 1)], "DryRun": true }),
        ),
        (
            "no pools at all",
            json!({ "InstancePools": [], "DryRun": true }),
        ),
        (
            "several pools, an order and an asset that both resolve",
            json!({
                "InstancePools": [pool("m5.large", 3), pool("c5.24xlarge", 9999)],
                "OrderId": order_id,
                "AssetId": INSTALLED[1],
                "DryRun": true,
            }),
        ),
        (
            "instances to exclude and an action for blocking instances",
            json!({
                "InstancePools": [pool("r5.2xlarge", 1)],
                "InstancesToExclude": { "Instances": ["i-0123456789abcdef0"] },
                "TaskActionOnBlockingInstances": "WAIT_FOR_EVACUATION",
                "DryRun": true,
            }),
        ),
    ];

    for (case, request) in &cases {
        let reply = start(&outposts, &outpost_id, request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (501, "NotImplemented"),
            "case {case:?}: expected 501 NotImplemented, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let stored = held(&outposts);
        assert_eq!(
            stored, 0,
            "case {case:?}: a dry run must store no capacity task: expected 0 held, \
             got {stored}"
        );
    }

    // The dry runs left the Outpost able to take a task, which it could not had one of
    // them left a task active on it — and the task the real start creates is the only
    // one the store holds.
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 2)], "OrderId": order_id });
    let started = start_ok(&outposts, &outpost_id, &request).await;
    let task_id = text(&started, "CapacityTaskId");
    let stored = held(&outposts);
    assert_eq!(
        stored, 1,
        "the real start is the only call that stored a task: expected 1 held, got {stored}"
    );
    let read = outposts
        .ok("GET", &task_route(&outpost_id, &task_id), None)
        .await;
    assert_eq!(
        text(&read, "CapacityTaskId"),
        task_id,
        "GetCapacityTask must answer the task the real start persisted, got {read}"
    );
    assert_eq!(
        read.get("DryRun"),
        Some(&Value::Bool(false)),
        "the persisted task is not a dry run, got {read}"
    );

    // And a dry run made WHILE that task is active is still the verdict's 501, not the
    // one-active-task conflict: a dry run creates no task, so it is not a second one.
    let reply = start(
        &outposts,
        &outpost_id,
        &json!({ "InstancePools": [pool("m5.4xlarge", 1)], "DryRun": true }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (501, "NotImplemented"),
        "a dry run beside an active task asks for the verdict, not a second task, \
         got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    let stored = held(&outposts);
    assert_eq!(
        stored, 1,
        "that dry run stored nothing either: expected 1 held, got {stored}"
    );
}

/// The accepting side of rule 1, stated against the same requests: with `DryRun` left
/// out or sent false, every one of them is accepted and persists a task, so what the
/// 501 above withholds is the dry-run verdict alone and not the request's references.
#[tokio::test]
async fn the_same_request_without_the_dry_run_is_accepted() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Option<bool>, bool)> = vec![
        ("the DryRun member left out", None, true),
        ("DryRun sent false", Some(false), true),
        ("DryRun sent true", Some(true), false),
    ];

    for (index, (case, dry_run, accepted)) in cases.iter().enumerate() {
        // One Outpost per row: only one capacity task can be active on an Outpost.
        let outpost_id = outpost(&outposts, &format!("dry-run-accepted-{index}")).await;
        install(&outposts, &outpost_id, &INSTALLED, case);
        let order_id = an_order(&outposts, &outpost_id).await;
        let request = with_dry_run(
            &json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "OrderId": order_id,
                "AssetId": INSTALLED[0],
            }),
            *dry_run,
        );

        let reply = outposts
            .call("POST", &capacity_route(&outpost_id), Some(&request))
            .await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the start to be accepted, got {} {} {}",
                reply.status, reply.error_code, reply.body
            );
            assert_eq!(
                reply.body.get("DryRun"),
                Some(&Value::Bool(false)),
                "case {case:?}: a task that is no dry run reports DryRun false, got {}",
                reply.body
            );
            let read = outposts
                .ok(
                    "GET",
                    &task_route(&outpost_id, &text(&reply.body, "CapacityTaskId")),
                    None,
                )
                .await;
            assert_eq!(
                text(&read, "OrderId"),
                order_id,
                "case {case:?}: the persisted task carries the order it named, got {read}"
            );
            assert_eq!(
                text(&read, "AssetId"),
                INSTALLED[0],
                "case {case:?}: the persisted task carries the asset it named, got {read}"
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (501, "NotImplemented"),
                "case {case:?}: expected 501 NotImplemented, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}

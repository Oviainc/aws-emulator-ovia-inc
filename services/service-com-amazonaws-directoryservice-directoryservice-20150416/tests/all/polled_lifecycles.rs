// SPDX-License-Identifier: CC0-1.0

//! The create-then-poll lifecycles of this service, driven end to end over HTTP the way a
//! client with a waiter drives them: a write, then the REPORTING operation polled until the
//! state it reports has advanced.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is `crate::harness`). Nothing a
//! RESPONSE claims is read from the state struct, so an answer that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test: every transient state this service records is owed to the clock,
//! so each of these sequences CONVERGES — the reporting operation answers each poll, the
//! state it reports advances out of the transient value on its own, and the end of the
//! lifecycle is observable — and it converges within a bound a client's waiter would allow
//! ([`POLL_DEADLINE`]), rather than resting on a transient value no clock ever moves. The
//! sequences are the ones a lifecycle client drives:
//!
//! * `CreateMicrosoftAD` -> `DescribeDirectories` until the directory is in service;
//! * `CreateSnapshot` -> `DescribeSnapshots` until the snapshot is complete -> `DeleteSnapshot`
//!   -> the snapshot is no longer in the inventory;
//! * `AddIpRoutes` -> `ListIpRoutes` until the block is in service -> `RemoveIpRoutes` -> the
//!   block is no longer routed, over a table of blocks of both address families;
//! * `CreateLogSubscription` -> the unfiltered `ListLogSubscriptions` inventory ->
//!   `DeleteLogSubscription`;
//! * `CreateTrust` -> `DescribeTrusts` until the trust is created -> `VerifyTrust` -> until it
//!   is verified -> `DeleteTrust` -> until the trust is no longer there.
//!
//! Each poll also pins that the transient state is REACHABLE: the write answers while the
//! state it starts is still the transient one, which is what a client polling immediately
//! after the call observes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_networking_and_settings::routes_of;
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::minimal_microsoft_ad;

/// How long one polled state may take to advance before the case fails: far longer than
/// any span this service publishes, and well inside what a client's waiter allows.
const POLL_DEADLINE: Duration = Duration::from_secs(20);

/// How often a poll asks: a client polls at human intervals, and a test that drives the
/// same sequence asks often enough to see each state it passes through.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Poll `$probe` — an expression answering `(Option<T>, String)`, the converged value and
/// what was observed this time round — until it converges, or fail naming the case, what was
/// being waited for and the last observation.
macro_rules! poll_until {
    ($case:expr, $what:expr, $probe:expr) => {{
        let started = Instant::now();
        loop {
            let (converged, observed): (Option<_>, String) = $probe;
            if let Some(value) = converged {
                break value;
            }
            assert!(
                started.elapsed() < POLL_DEADLINE,
                "case {case:?}: expected {what} within {deadline:?}, last observed {observed}",
                case = $case,
                what = $what,
                deadline = POLL_DEADLINE,
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }};
}

/// The `Stage` `DescribeDirectories` reports for one directory.
async fn directory_stage(router: &MultiServiceRouter, directory_id: &str, case: &str) -> String {
    let answer = ok(
        router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [directory_id] }),
        case,
    )
    .await;
    let described = answer["DirectoryDescriptions"]
        .as_array()
        .and_then(|page| page.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected a description, got {answer}"));
    string(&described, "Stage", case)
}

/// A Managed Microsoft AD directory, polled until it is in service — the fixture every
/// lifecycle below starts from, and the first wait a lifecycle client makes.
async fn directory_in_service(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let answer = ok(router, "CreateMicrosoftAD", minimal_microsoft_ad(name), case).await;
    let directory_id = string(&answer, "DirectoryId", case);

    // The create answered while the directory was still being built, which is what a client
    // that polls immediately observes: the stage is one of the chain's transient values, not
    // the serving one.
    let first = directory_stage(router, &directory_id, case).await;
    assert!(
        first == "Requested" || first == "Creating",
        "case {case:?}: expected a create to answer before the directory is in service, got \
         stage {first:?}"
    );

    poll_until!(case, "the directory to report itself in service", {
        let stage = directory_stage(router, &directory_id, case).await;
        (
            (stage == "Active").then_some(stage.clone()),
            format!("stage {stage:?}"),
        )
    });
    directory_id
}

/// The snapshots the unfiltered `DescribeSnapshots` inventory reports, by `SnapshotId`.
async fn snapshot_inventory(router: &MultiServiceRouter, case: &str) -> BTreeMap<String, Value> {
    let answer = ok(router, "DescribeSnapshots", json!({}), case).await;
    answer["Snapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Snapshots, got {answer}"))
        .iter()
        .map(|snapshot| (string(snapshot, "SnapshotId", case), snapshot.clone()))
        .collect()
}

#[tokio::test]
async fn the_snapshot_lifecycle_converges_through_describe_snapshots() {
    let router = service();
    let case = "the snapshot lifecycle";
    let directory_id = directory_in_service(&router, "snap.example.com", case).await;

    let created = ok(
        &router,
        "CreateSnapshot",
        json!({ "DirectoryId": &directory_id, "Name": "nightly" }),
        case,
    )
    .await;
    let snapshot_id = string(&created, "SnapshotId", case);

    // The create answered while the snapshot was still being taken.
    let inventory = snapshot_inventory(&router, case).await;
    assert_eq!(
        inventory
            .get(&snapshot_id)
            .map(|snapshot| snapshot["Status"].clone()),
        Some(json!("Creating")),
        "case {case:?}: expected the fresh snapshot to be reported as still being taken, \
         got {inventory:?}"
    );

    // The inventory is what a client polls, and the status advances on the clock alone.
    poll_until!(case, "the snapshot to report itself complete", {
        let inventory = snapshot_inventory(&router, case).await;
        let status = inventory
            .get(&snapshot_id)
            .map(|snapshot| snapshot["Status"].clone())
            .unwrap_or(Value::Null);
        (
            (status == json!("Completed")).then_some(status.clone()),
            format!("status {status}"),
        )
    });

    ok(
        &router,
        "DeleteSnapshot",
        json!({ "SnapshotId": &snapshot_id }),
        case,
    )
    .await;
    let after = poll_until!(case, "the deleted snapshot to leave the inventory", {
        let inventory = snapshot_inventory(&router, case).await;
        let held = inventory.contains_key(&snapshot_id);
        (
            (!held).then_some(inventory.len()),
            format!("the inventory still holds {snapshot_id}: {held}"),
        )
    });
    assert_eq!(
        after, 0,
        "case {case:?}: expected the account to hold no snapshot after the delete, got \
         {after} of them"
    );
}

#[tokio::test]
async fn the_ip_route_lifecycle_converges_through_list_ip_routes() {
    let router = service();
    let case = "the IP route lifecycle";
    let directory_id = directory_in_service(&router, "routes.example.com", case).await;

    // The rows differ along the address family and the prefix length, so the convergence is
    // not asserted on one shape of block. The member a block is REMOVED by is the one of its
    // own family, as `RemoveIpRoutes` declares them (`CidrIps` / `CidrIpv6s`).
    let rows: [(&str, &str, &str, &str); 3] = [
        ("an IPv4 network", "CidrIp", "CidrIps", "198.51.100.0/24"),
        ("an IPv4 host route", "CidrIp", "CidrIps", "203.0.113.7/32"),
        ("an IPv6 network", "CidrIpv6", "CidrIpv6s", "2001:db8::/32"),
    ];

    for (class, member, removal_member, block) in rows {
        let case = format!("{case}: {class}");
        ok(
            &router,
            "AddIpRoutes",
            json!({ "DirectoryId": &directory_id, "IpRoutes": [{ member: block }] }),
            &case,
        )
        .await;

        // The add answered while the block was still being put in service.
        let routes = routes_of(&router, &directory_id, &case).await;
        assert_eq!(
            routes
                .get(block)
                .map(|route| route["IpRouteStatusMsg"].clone()),
            Some(json!("Adding")),
            "case {case:?}: expected the fresh block to be reported as still being added, \
             got {routes:?}"
        );

        // The listing is what a client polls, and the status advances on the clock alone.
        poll_until!(&case, "the block to report itself in service", {
            let routes = routes_of(&router, &directory_id, &case).await;
            let status = routes
                .get(block)
                .map(|route| route["IpRouteStatusMsg"].clone())
                .unwrap_or(Value::Null);
            (
                (status == json!("Added")).then_some(status.clone()),
                format!("status {status}"),
            )
        });

        ok(
            &router,
            "RemoveIpRoutes",
            json!({ "DirectoryId": &directory_id, removal_member: [block] }),
            &case,
        )
        .await;
        poll_until!(&case, "the removed block to stop being routed", {
            let routes = routes_of(&router, &directory_id, &case).await;
            let held = routes.contains_key(block);
            ((!held).then_some(()), format!("still routed: {held}"))
        });
    }

    // Every block has been through the whole lifecycle, so the directory routes nothing.
    let left = routes_of(&router, &directory_id, case).await;
    assert!(
        left.is_empty(),
        "case {case:?}: expected the directory to route nothing after the removals, got \
         {left:?}"
    );
}

#[tokio::test]
async fn the_log_subscription_lifecycle_is_reported_by_the_unfiltered_inventory() {
    let router = service();
    let case = "the log subscription lifecycle";
    let first = directory_in_service(&router, "logs-one.example.com", case).await;
    let second = directory_in_service(&router, "logs-two.example.com", case).await;

    for (directory_id, group) in [(&first, "/aws/ds/one"), (&second, "/aws/ds/two")] {
        ok(
            &router,
            "CreateLogSubscription",
            json!({ "DirectoryId": directory_id, "LogGroupName": group }),
            case,
        )
        .await;
    }

    // The unfiltered inventory is what the lifecycle client reads: both subscriptions are
    // reported, each against the directory that created it.
    let answer = ok(&router, "ListLogSubscriptions", json!({}), case).await;
    let reported: BTreeMap<String, String> = answer["LogSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected LogSubscriptions, got {answer}"))
        .iter()
        .map(|subscription| {
            (
                string(subscription, "DirectoryId", case),
                string(subscription, "LogGroupName", case),
            )
        })
        .collect();
    assert_eq!(
        reported,
        [
            (first.clone(), "/aws/ds/one".to_string()),
            (second.clone(), "/aws/ds/two".to_string()),
        ]
        .into_iter()
        .collect::<BTreeMap<String, String>>(),
        "case {case:?}: expected the inventory to report both subscriptions, got {reported:?}"
    );

    ok(
        &router,
        "DeleteLogSubscription",
        json!({ "DirectoryId": &first }),
        case,
    )
    .await;
    let left = ok(&router, "ListLogSubscriptions", json!({}), case).await;
    let pairs: Vec<Value> = left["LogSubscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        pairs.len(),
        1,
        "case {case:?}: expected the deleted subscription to leave the inventory, got {left}"
    );
    assert_eq!(
        pairs[0]["DirectoryId"],
        json!(&second),
        "case {case:?}: expected the other directory's subscription to remain, got {left}"
    );
}

#[tokio::test]
async fn the_trust_lifecycle_converges_through_describe_trusts() {
    let router = service();
    let case = "the trust lifecycle";
    let directory_id = directory_in_service(&router, "trusted.example.com", case).await;

    let created = ok(
        &router,
        "CreateTrust",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "remote.example.com",
            "TrustPassword": "Tru5t-P@ssword 1",
            "TrustDirection": "Two-Way",
        }),
        case,
    )
    .await;
    let trust_id = string(&created, "TrustId", case);

    /// The `TrustState` `DescribeTrusts` reports for one trust, or `Null` once it reports
    /// none.
    async fn trust_state(router: &MultiServiceRouter, trust_id: &str, case: &str) -> Value {
        let answer = ok(
            router,
            "DescribeTrusts",
            json!({ "TrustIds": [trust_id] }),
            case,
        )
        .await;
        answer["Trusts"]
            .as_array()
            .and_then(|trusts| trusts.first())
            .map(|trust| trust["TrustState"].clone())
            .unwrap_or(Value::Null)
    }

    // The create answered while the Amazon Web Services side was still being established.
    assert_eq!(
        trust_state(&router, &trust_id, case).await,
        json!("Creating"),
        "case {case:?}: expected the create to answer before the trust is established"
    );
    poll_until!(case, "the trust to report itself created", {
        let state = trust_state(&router, &trust_id, case).await;
        (
            (state == json!("Created")).then_some(state.clone()),
            format!("state {state}"),
        )
    });

    ok(
        &router,
        "VerifyTrust",
        json!({ "TrustId": &trust_id }),
        case,
    )
    .await;
    assert_eq!(
        trust_state(&router, &trust_id, case).await,
        json!("Verifying"),
        "case {case:?}: expected the verify to answer while the verification is running"
    );
    poll_until!(case, "the trust to report itself verified", {
        let state = trust_state(&router, &trust_id, case).await;
        (
            (state == json!("Verified")).then_some(state.clone()),
            format!("state {state}"),
        )
    });

    ok(
        &router,
        "DeleteTrust",
        json!({ "TrustId": &trust_id }),
        case,
    )
    .await;
    // The teardown is owed to the clock too: the trust is reported through it and then the
    // relationship is gone, which is what the lifecycle client's last poll reads.
    poll_until!(case, "the deleted trust to leave the service", {
        let (status, parsed) = call(
            &router,
            "DescribeTrusts",
            json!({ "TrustIds": [&trust_id] }),
        )
        .await;
        (
            (status == 400).then_some(()),
            format!("DescribeTrusts answered {status} with {parsed}"),
        )
    });
}

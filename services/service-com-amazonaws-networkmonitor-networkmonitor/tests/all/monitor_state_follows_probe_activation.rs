// SPDX-License-Identifier: CC0-1.0

//! A monitor's `state` follows whether any probe it holds is in service, with
//! `UpdateProbe`'s activate/deactivate toggle as the thing that changes it.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` already owns. The rules
//! under test:
//!
//! * a monitor aggregates what its probes measure, so one left with no probe in
//!   service — every probe it holds deactivated, as `UpdateProbe` does — has
//!   nothing aggregating and is reported `INACTIVE`, while one that still holds
//!   a probe in service keeps the state it had;
//! * a monitor that gains a probe in service again is owed the
//!   `PENDING` -> `ACTIVE` transition, exactly as one that gains its first
//!   probe is;
//! * `GetMonitor` and `ListMonitors` (the `state` filter included) report that
//!   one stored state, so a client polling either observes the transition;
//! * the probes themselves keep reporting the state each was put into.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, get_monitor, list_monitors, page_names, probe_json, probes_of, router,
    string_at, PAST_ACTIVATION,
};

/// One ICMP probe body of the shared form, for the cases here that vary only
/// the destination.
fn icmp_probe_json(destination: &str) -> String {
    probe_json(destination, "ICMP", r#","packetSize":56"#)
}

/// `PATCH /monitors/{monitorName}/probes/{probeId}` with one `state` member:
/// the documented activate/deactivate toggle.
async fn set_probe_state(
    router: &MultiServiceRouter,
    name: &str,
    probe_id: &str,
    state: &str,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/monitors/{name}/probes/{probe_id}"),
        Some(&format!(r#"{{"state":"{state}"}}"#)),
    )
    .await
}

/// Create a monitor holding `probe_count` probes and answer the `probeId` of
/// each, in the order `GetMonitor` reports them.
async fn monitor_with_probes(
    router: &MultiServiceRouter,
    name: &str,
    probe_count: usize,
) -> Vec<String> {
    let probes: Vec<String> = (0..probe_count)
        .map(|i| icmp_probe_json(&format!("10.1.0.{}", i + 1)))
        .collect();
    let body = format!(
        r#"{{"monitorName":"{name}","probes":[{}]}}"#,
        probes.join(",")
    );
    let (status, created) = create_monitor(router, &body).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor for {name:?} expected 200, got {status} {created:?}"
    );
    let (_, got) = get_monitor(router, name).await;
    let ids: Vec<String> = probes_of(&got)
        .iter()
        .map(|probe| string_at(probe, "probeId"))
        .collect();
    assert_eq!(
        ids.len(),
        probe_count,
        "setup for {name:?}: expected {probe_count} probes, got {got:?}"
    );
    ids
}

/// The state `GetMonitor` reports for one monitor.
async fn monitor_state(router: &MultiServiceRouter, name: &str) -> String {
    let (status, got) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "GetMonitor for {name:?} expected 200, got {status} {got:?}"
    );
    string_at(&got, "state")
}

/// A monitor is `INACTIVE` exactly when nothing it holds is in service, so
/// deactivating probes through `UpdateProbe` moves the monitor the moment the
/// last one goes off, and leaves it alone while any other is still measuring.
///
/// The table spans what the rule turns on — how many probes the monitor holds
/// and how many of them the caller switched off (none, some, all) — over probe
/// counts the recorded case (one probe, deactivated) is a single row of.
#[tokio::test]
async fn a_monitor_is_idle_exactly_when_no_probe_of_its_own_is_in_service() {
    // (case, probes held, probes deactivated, the monitor state after)
    let cases: &[(&str, usize, usize, &str)] = &[
        ("one probe, left alone", 1, 0, "ACTIVE"),
        (
            "the recorded case: the only probe deactivated",
            1,
            1,
            "INACTIVE",
        ),
        ("two probes, one deactivated", 2, 1, "ACTIVE"),
        ("two probes, both deactivated", 2, 2, "INACTIVE"),
        ("three probes, two deactivated", 3, 2, "ACTIVE"),
        ("three probes, all deactivated", 3, 3, "INACTIVE"),
    ];

    let router = router();
    // Every monitor is created first and settled once: the span the clock owes a
    // new monitor is not what these rows are about.
    let mut fixtures = Vec::new();
    for (index, (case, probe_count, deactivate, expected)) in cases.iter().enumerate() {
        let name = format!("idle-rule-{index}");
        let ids = monitor_with_probes(&router, &name, *probe_count).await;
        fixtures.push((*case, name, ids, *deactivate, *expected));
    }
    std::thread::sleep(PAST_ACTIVATION);

    for (case, name, ids, deactivate, _) in &fixtures {
        let settled = monitor_state(&router, name).await;
        assert_eq!(
            settled, "ACTIVE",
            "case {case:?}: the monitor must start out ACTIVE, got {settled:?}"
        );
        for probe_id in ids.iter().take(*deactivate) {
            let (status, updated) = set_probe_state(&router, name, probe_id, "INACTIVE").await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateProbe INACTIVE expected 200, got {status} {updated:?}"
            );
            assert_eq!(
                string_at(&updated, "state"),
                "INACTIVE",
                "case {case:?}: the probe must report INACTIVE, got {updated:?}"
            );
        }
    }

    for (case, name, ids, deactivate, expected) in &fixtures {
        let actual = monitor_state(&router, name).await;
        assert_eq!(
            actual, *expected,
            "case {case:?}: with {deactivate} of {} probes deactivated, expected the monitor \
             {expected}, got {actual:?}",
            ids.len()
        );
        // The filter selects on the same stored state the read reports.
        let (_, listed) = list_monitors(&router, &format!("?state={expected}")).await;
        assert!(
            page_names(&listed).contains(name),
            "case {case:?}: ListMonitors(state={expected}) must list {name:?}, got {listed:?}"
        );
        let other = if *expected == "INACTIVE" {
            "ACTIVE"
        } else {
            "INACTIVE"
        };
        let (_, listed) = list_monitors(&router, &format!("?state={other}")).await;
        assert!(
            !page_names(&listed).contains(name),
            "case {case:?}: ListMonitors(state={other}) must omit {name:?}, got {listed:?}"
        );
        // Each probe still reports the state it was put into.
        let (_, got) = get_monitor(&router, name).await;
        for (position, probe) in probes_of(&got).iter().enumerate() {
            let expected_probe = if position < *deactivate {
                "INACTIVE"
            } else {
                "ACTIVE"
            };
            assert_eq!(
                string_at(probe, "state"),
                expected_probe,
                "case {case:?}: probe {position} must report {expected_probe}, got {probe:?}"
            );
        }
    }
}

/// Switching a probe back on gives its monitor something to aggregate again, so
/// the monitor is owed `PENDING` and then `ACTIVE` — the same transition a
/// monitor gaining its first probe is owed — and a repeat of either half of the
/// toggle changes nothing.
#[tokio::test]
async fn reactivating_a_probe_makes_its_monitor_pending_and_then_active_again() {
    let router = router();
    let name = "deactivate-then-reactivate";
    let ids = monitor_with_probes(&router, name, 1).await;
    let probe_id = ids.first().cloned().unwrap_or_default();
    std::thread::sleep(PAST_ACTIVATION);
    let actual = monitor_state(&router, name).await;
    assert_eq!(
        actual, "ACTIVE",
        "case \"before the toggle\": expected ACTIVE, got {actual:?}"
    );

    for case in ["deactivate", "deactivate again"] {
        let (status, updated) = set_probe_state(&router, name, &probe_id, "INACTIVE").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {updated:?}"
        );
        let actual = monitor_state(&router, name).await;
        assert_eq!(
            actual, "INACTIVE",
            "case {case:?}: with nothing in service the monitor is INACTIVE, got {actual:?}"
        );
    }

    let (status, updated) = set_probe_state(&router, name, &probe_id, "ACTIVE").await;
    assert_eq!(
        status, 200,
        "case \"reactivate\": expected 200, got {status} {updated:?}"
    );
    assert_eq!(
        string_at(&updated, "state"),
        "ACTIVE",
        "case \"reactivate\": the probe reports the state it was put into, got {updated:?}"
    );
    // The monitor is provisioning again, and a caller reading straight after the
    // call sees that: the transition is owed to the clock, not to the read.
    let actual = monitor_state(&router, name).await;
    assert_eq!(
        actual, "PENDING",
        "case \"straight after the reactivation\": expected PENDING, got {actual:?}"
    );
    let (_, listed) = list_monitors(&router, "?state=PENDING").await;
    assert!(
        page_names(&listed).contains(&name.to_string()),
        "case \"straight after the reactivation\": ListMonitors(state=PENDING) must list it, \
         got {listed:?}"
    );

    std::thread::sleep(PAST_ACTIVATION);
    let actual = monitor_state(&router, name).await;
    assert_eq!(
        actual, "ACTIVE",
        "case \"once the activation span has passed again\": expected ACTIVE, got {actual:?}"
    );
    let (_, listed) = list_monitors(&router, "?state=ACTIVE").await;
    assert!(
        page_names(&listed).contains(&name.to_string()),
        "case \"once the activation span has passed again\": the ACTIVE filter selects it, \
         got {listed:?}"
    );
}

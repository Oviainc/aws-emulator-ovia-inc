// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for what an idempotency REPLAY reports.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: a
//! response that carries a stored resource's `state` reports the state that
//! resource is in at the INSTANT of the call — the lifecycle transitions the
//! clock has already made due are materialised first — so a replay of an
//! idempotency token and a read of the same resource taken at the same moment
//! never disagree.
//!
//! The replay is the interesting path precisely because it answers from a record
//! an EARLIER call stored: the retry is sent first here, before any read, so
//! nothing else can have settled the resource on its behalf.
//!
//! The tables span the class the rule covers rather than one recorded retry:
//! both replay paths (`CreateMonitor` and `CreateProbe`), a monitor owed an
//! activation and one owed nothing at all, one probe and several, and both
//! protocols and address families a probe can carry.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, get_monitor, probe_json, probes_of, router, string_at,
    PAST_ACTIVATION,
};

async fn get_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

/// One `CreateMonitor` retry case: how many probes the monitor is created with,
/// and the `MonitorState` the model's enum names for each half of its lifecycle
/// (`smithy_query(query="shape",shape="MonitorState")` ->
/// `ACTIVE | DELETING | ERROR | INACTIVE | PENDING`).
struct MonitorCase {
    monitor_name: &'static str,
    probe_count: usize,
    /// The state before the activation span has elapsed.
    transient_state: &'static str,
    /// The state once the clock has made the transition due.
    settled_state: &'static str,
}

/// A `CreateMonitor` retry reports the monitor's state at the instant of the
/// retry: `PENDING` while the activation is still owed, `ACTIVE` once the clock
/// has made it due, and `INACTIVE` for a monitor that is owed nothing — and a
/// `GetMonitor` taken straight afterwards agrees with it.
#[tokio::test]
async fn a_create_monitor_retry_reports_the_state_at_the_instant_of_the_retry() {
    let cases = [
        MonitorCase {
            monitor_name: "replay-no-probes",
            probe_count: 0,
            transient_state: "INACTIVE",
            settled_state: "INACTIVE",
        },
        MonitorCase {
            monitor_name: "replay-one-probe",
            probe_count: 1,
            transient_state: "PENDING",
            settled_state: "ACTIVE",
        },
        MonitorCase {
            monitor_name: "replay-three-probes",
            probe_count: 3,
            transient_state: "PENDING",
            settled_state: "ACTIVE",
        },
    ];

    for case in &cases {
        let name = case.monitor_name;
        let router = router();
        let probes: Vec<String> = (0..case.probe_count)
            .map(|i| probe_json(&format!("10.0.0.{}", i + 1), "ICMP", r#","packetSize":56"#))
            .collect();
        let body = format!(
            r#"{{"monitorName":"{name}","aggregationPeriod":30,"clientToken":"token-{name}",
                 "tags":{{"team":"net"}},"probes":[{}]}}"#,
            probes.join(",")
        );

        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the first CreateMonitor 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "state"),
            case.transient_state,
            "case {name:?}: expected the create to report {:?}, got {created:?}",
            case.transient_state
        );

        // A retry sent while the activation is still owed replays the transient
        // state: settling is the clock's, not the retry's.
        let (status, early) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the early retry 200, got {status} {early:?}"
        );
        assert_eq!(
            early, created,
            "case {name:?}: the early retry must replay the first answer, got {early:?}"
        );

        std::thread::sleep(PAST_ACTIVATION);

        // The retry comes FIRST, before any read of the monitor, so the state it
        // reports can only have been settled by the retry itself.
        let (status, late) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the late retry 200, got {status} {late:?}"
        );
        assert_eq!(
            string_at(&late, "state"),
            case.settled_state,
            "case {name:?}: expected the retry past the activation span to report {:?}, got {late:?}",
            case.settled_state
        );
        // Everything else about the replayed record is the first answer still.
        for member in ["monitorArn", "monitorName"] {
            assert_eq!(
                string_at(&late, member),
                string_at(&created, member),
                "case {name:?}: the retry must replay {member}, got {late:?}"
            );
        }
        assert_eq!(
            late.get("aggregationPeriod"),
            created.get("aggregationPeriod"),
            "case {name:?}: the retry must replay aggregationPeriod, got {late:?}"
        );
        assert_eq!(
            late.get("tags"),
            created.get("tags"),
            "case {name:?}: the retry must replay the tags, got {late:?}"
        );

        let (status, got) = get_monitor(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            string_at(&got, "state"),
            string_at(&late, "state"),
            "case {name:?}: the retry and GetMonitor must agree, got {late:?} and {got:?}"
        );
        assert_eq!(
            probes_of(&got).len(),
            case.probe_count,
            "case {name:?}: the retry must mint no further probe, got {got:?}"
        );
    }
}

/// One `CreateProbe` retry case: the probe body the request carries, named by
/// the protocol and address family it exercises.
struct ProbeCase {
    case: &'static str,
    destination: &'static str,
    protocol: &'static str,
    extra: &'static str,
}

/// A `CreateProbe` retry reports the probe's state at the instant of the retry:
/// `PENDING` while its activation is still owed and `ACTIVE` once the clock has
/// made it due — the same state a `GetProbe` taken straight afterwards reports.
#[tokio::test]
async fn a_create_probe_retry_reports_the_state_at_the_instant_of_the_retry() {
    let cases = [
        ProbeCase {
            case: "ICMP over IPv4",
            destination: "10.0.0.1",
            protocol: "ICMP",
            extra: r#","packetSize":56"#,
        },
        ProbeCase {
            case: "TCP over IPv4",
            destination: "10.0.0.2",
            protocol: "TCP",
            extra: r#","destinationPort":443,"packetSize":8500"#,
        },
        ProbeCase {
            case: "ICMP over IPv6",
            destination: "2001:db8::1",
            protocol: "ICMP",
            extra: r#","packetSize":100"#,
        },
    ];

    for probe in &cases {
        let case = probe.case;
        let router = router();
        let monitor = "probe-replay-host";
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{monitor}"}}"#)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateMonitor, got {status} {created:?}"
        );

        let body = format!(
            r#"{{"clientToken":"probe-token","probe":{}}}"#,
            probe_json(probe.destination, probe.protocol, probe.extra)
        );
        let (status, first) = create_probe(&router, monitor, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateProbe 200, got {status} {first:?}"
        );
        assert_eq!(
            string_at(&first, "state"),
            "PENDING",
            "case {case:?}: expected the create to report PENDING, got {first:?}"
        );
        let probe_id = string_at(&first, "probeId");

        let (status, early) = create_probe(&router, monitor, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the early retry 200, got {status} {early:?}"
        );
        assert_eq!(
            early, first,
            "case {case:?}: the early retry must replay the first answer, got {early:?}"
        );

        std::thread::sleep(PAST_ACTIVATION);

        // The retry comes FIRST, before any read of the probe.
        let (status, late) = create_probe(&router, monitor, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the late retry 200, got {status} {late:?}"
        );
        assert_eq!(
            string_at(&late, "state"),
            "ACTIVE",
            "case {case:?}: expected the retry past the activation span to report ACTIVE, got {late:?}"
        );
        assert_eq!(
            string_at(&late, "probeId"),
            probe_id,
            "case {case:?}: the retry must replay the first probe, got {late:?}"
        );
        for member in ["probeArn", "destination", "protocol", "addressFamily"] {
            assert_eq!(
                string_at(&late, member),
                string_at(&first, member),
                "case {case:?}: the retry must replay {member}, got {late:?}"
            );
        }

        let (status, got) = get_probe(&router, monitor, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProbe 200, got {status} {got:?}"
        );
        assert_eq!(
            string_at(&got, "state"),
            string_at(&late, "state"),
            "case {case:?}: the retry and GetProbe must agree, got {late:?} and {got:?}"
        );
        let (_, monitor_body) = get_monitor(&router, monitor).await;
        assert_eq!(
            probes_of(&monitor_body).len(),
            1,
            "case {case:?}: the retry must mint no second probe, got {monitor_body:?}"
        );
    }
}

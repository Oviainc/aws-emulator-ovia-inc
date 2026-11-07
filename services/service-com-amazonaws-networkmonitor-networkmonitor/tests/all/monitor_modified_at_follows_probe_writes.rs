// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the MONITOR record's `modifiedAt` under the calls
//! that write one of its probes.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: the
//! probes are part of the monitor record every read reports (`GetMonitor`
//! answers them with their own members), so a call that adds a probe, rewrites
//! one or takes one away CHANGED the monitor — the monitor's `modifiedAt` moves
//! to the instant of that call, the same instant the probe's own `modifiedAt`
//! moves to. A call that stored nothing new (a repeat of an `@idempotent`
//! probe write, an update naming no member, a read) changed neither record, so
//! it moves neither `modifiedAt`, and `createdAt` never moves.
//!
//! The table spans the class rather than one recorded call: each of the three
//! operations that write a probe of a stored monitor (`CreateProbe`,
//! `UpdateProbe`, `DeleteProbe`), each member an update can name, each of those
//! sent first as a value that differs from the stored one and then as the value
//! already stored, an update naming no member at all, and a read. The recorded
//! case (an `UpdateProbe` switching the monitor's only probe off) is one row.

use std::time::Duration;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, get_monitor, probes_of, router, source_arn_of, string_at,
    PAST_ACTIVATION,
};
use super::update_probe::timestamp;

/// A pause long enough that two instants either side of it are distinguishable
/// at the precision the wire format reports a timestamp with, and short enough
/// that a whole table of rows stays well inside a probe's teardown. A blocking
/// sleep, for the reason given on `PAST_ACTIVATION`.
const INSTANT_APART: Duration = Duration::from_millis(20);

/// One call a row makes against the monitor's probes.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// `CreateProbe`: the monitor gains a probe, on a subnet of its own so the
    /// row is never refused for the company its probes keep.
    Create,
    /// `UpdateProbe` with this body, on the probe the monitor most recently
    /// gained.
    Update(&'static str),
    /// `DeleteProbe` of that same probe.
    Delete,
    /// `GetMonitor`: a read, which writes nothing.
    Read,
}

async fn update_probe(
    router: &MultiServiceRouter,
    name: &str,
    probe_id: &str,
    body: &str,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/monitors/{name}/probes/{probe_id}"),
        Some(body),
    )
    .await
}

async fn delete_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

async fn get_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

/// The `modifiedAt` and `createdAt` a `GetMonitor` reports for this monitor.
async fn monitor_timestamps(router: &MultiServiceRouter, name: &str) -> (f64, f64) {
    let (status, monitor) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "GetMonitor {name:?}: expected 200, got {status} {monitor:?}"
    );
    (
        timestamp(&monitor, "modifiedAt"),
        timestamp(&monitor, "createdAt"),
    )
}

/// A probe body on a subnet of its own, TCP with a port so a row may name every
/// member an update can change.
fn probe_body(index: usize, destination: &str) -> String {
    format!(
        r#"{{"probe":{{"sourceArn":"{}","destination":"{destination}","protocol":"TCP","destinationPort":443,"packetSize":56}}}}"#,
        source_arn_of(index)
    )
}

/// Every call that writes a probe of a stored monitor moves the MONITOR's
/// `modifiedAt` to the instant of that call, and a call that stored nothing new
/// moves it nowhere.
#[tokio::test]
async fn a_probe_write_moves_the_containing_monitors_modified_at() {
    // (case, the call the row makes, whether it stores something new)
    let rows: &[(&str, Call, bool)] = &[
        ("CreateProbe: the monitor gains its first probe", Call::Create, true),
        ("GetMonitor: a read writes nothing", Call::Read, false),
        (
            "UpdateProbe: packetSize to a new value",
            Call::Update(r#"{"packetSize":1500}"#),
            true,
        ),
        (
            "UpdateProbe: packetSize to the value already stored",
            Call::Update(r#"{"packetSize":1500}"#),
            false,
        ),
        (
            "UpdateProbe: destination to a new address, IPv4 -> IPv6",
            Call::Update(r#"{"destination":"2001:db8::1"}"#),
            true,
        ),
        (
            "UpdateProbe: destination to the address already stored",
            Call::Update(r#"{"destination":"2001:db8::1"}"#),
            false,
        ),
        (
            "UpdateProbe: destinationPort to a new port",
            Call::Update(r#"{"destinationPort":8080}"#),
            true,
        ),
        (
            "UpdateProbe: destinationPort to the port already stored",
            Call::Update(r#"{"destinationPort":8080}"#),
            false,
        ),
        (
            "UpdateProbe: protocol to the one already stored",
            Call::Update(r#"{"protocol":"TCP"}"#),
            false,
        ),
        (
            "the recorded case: UpdateProbe switches the monitor's only probe off",
            Call::Update(r#"{"state":"INACTIVE"}"#),
            true,
        ),
        (
            "UpdateProbe: state to the state already stored",
            Call::Update(r#"{"state":"INACTIVE"}"#),
            false,
        ),
        (
            "UpdateProbe: the probe switched back on",
            Call::Update(r#"{"state":"ACTIVE"}"#),
            true,
        ),
        ("UpdateProbe: no member at all", Call::Update(r#"{}"#), false),
        (
            "UpdateProbe: several members at once, all new",
            Call::Update(r#"{"protocol":"ICMP","destination":"203.0.113.9","packetSize":8500}"#),
            true,
        ),
        ("CreateProbe: a second probe", Call::Create, true),
        (
            "UpdateProbe: that probe switched off, which a delete of it requires",
            Call::Update(r#"{"state":"INACTIVE"}"#),
            true,
        ),
        ("DeleteProbe: the teardown of that probe is accepted", Call::Delete, true),
        ("DeleteProbe: a repeat of the same delete", Call::Delete, false),
    ];

    let router = router();
    let name = "probe-writes";
    let (status, created) = create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor: expected 200, got {status} {created:?}"
    );
    let (mut monitor_modified_at, monitor_created_at) = monitor_timestamps(&router, name).await;
    assert!(
        (monitor_modified_at - monitor_created_at).abs() < f64::EPSILON,
        "a freshly created monitor has modifiedAt == createdAt, got {created:?}"
    );

    let mut probes: Vec<String> = Vec::new();
    for (case, row, stores_change) in rows {
        // Every row's call lands at an instant the wire format can tell from the
        // previous one's, so "moved" and "stayed" are distinguishable.
        std::thread::sleep(INSTANT_APART);
        // The probe record the row's call answered, for the rows that answer one:
        // a write moves the probe's and the monitor's `modifiedAt` to the SAME
        // instant, because one call changed both records.
        let probe_record = match row {
            Call::Create => {
                let body = probe_body(probes.len(), "10.0.9.1");
                let (status, probe) = create_probe(&router, name, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {probe:?}"
                );
                probes.push(string_at(&probe, "probeId"));
                Some(probe)
            }
            Call::Update(body) => {
                let probe_id = probes
                    .last()
                    .unwrap_or_else(|| panic!("case {case:?}: the table creates a probe first"));
                let (status, probe) = update_probe(&router, name, probe_id, body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {probe:?}"
                );
                Some(probe)
            }
            Call::Delete => {
                let probe_id = probes
                    .last()
                    .unwrap_or_else(|| panic!("case {case:?}: the table creates a probe first"))
                    .clone();
                let (status, answer) = delete_probe(&router, name, &probe_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {answer:?}"
                );
                // A delete answers nothing, so the probe's own timestamps are
                // read back through `GetProbe`: a draining probe is still
                // readable.
                let (status, probe) = get_probe(&router, name, &probe_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a draining probe is still readable, got {status} {probe:?}"
                );
                Some(probe)
            }
            Call::Read => {
                let (status, monitor) = get_monitor(&router, name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {monitor:?}"
                );
                None
            }
        };

        let (now_modified_at, now_created_at) = monitor_timestamps(&router, name).await;
        if *stores_change {
            assert!(
                now_modified_at > monitor_modified_at,
                "case {case:?}: the call changed a probe the monitor reports, so the monitor's \
                 modifiedAt must move past {monitor_modified_at}, got {now_modified_at}"
            );
            if let Some(probe) = &probe_record {
                let probe_modified_at = timestamp(probe, "modifiedAt");
                assert!(
                    (now_modified_at - probe_modified_at).abs() < f64::EPSILON,
                    "case {case:?}: one call changed both records, so the monitor's modifiedAt \
                     must be the probe's {probe_modified_at}, got {now_modified_at}"
                );
            }
        } else {
            assert!(
                (now_modified_at - monitor_modified_at).abs() < f64::EPSILON,
                "case {case:?}: the call stored nothing new, so the monitor's modifiedAt must \
                 stay {monitor_modified_at}, got {now_modified_at}"
            );
        }
        assert!(
            (now_created_at - monitor_created_at).abs() < f64::EPSILON,
            "case {case:?}: a monitor's createdAt never moves, expected \
             {monitor_created_at}, got {now_created_at}"
        );
        monitor_modified_at = now_modified_at;
    }
}

/// The recorded reproduction: a monitor whose only probe is switched off reports
/// the state that write left it in AND a `modifiedAt` moved to the instant of
/// that write — while the `PENDING` -> `ACTIVE` transition the CLOCK owes it
/// moves nothing, because a read modifies nothing.
///
/// The two rows are the two directions the toggle runs in: the state a monitor
/// is reported in follows the probes it holds, so switching its last probe off
/// takes it back to `INACTIVE` and switching one back on makes its activation
/// due again.
#[tokio::test]
async fn deactivating_the_last_probe_moves_the_monitors_state_and_modified_at() {
    let router = router();
    let name = "last-probe-off";
    let (status, created) = create_monitor(
        &router,
        &format!(
            r#"{{"monitorName":"{name}","probes":[{{"sourceArn":"{}","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}}]}}"#,
            source_arn_of(7)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor: expected 200, got {status} {created:?}"
    );

    // The activation the clock owes the monitor, materialised by a read: it is
    // owed whether or not anybody calls, so it is not a modification.
    std::thread::sleep(PAST_ACTIVATION);
    let (status, settled) = get_monitor(&router, name).await;
    assert_eq!(
        status, 200,
        "case \"the clock's activation\": expected 200, got {status} {settled:?}"
    );
    let probe_id = string_at(
        probes_of(&settled)
            .first()
            .unwrap_or_else(|| panic!("setup: the monitor must list its probe, got {settled:?}")),
        "probeId",
    );
    assert_eq!(
        string_at(&settled, "state"),
        "ACTIVE",
        "case \"the clock's activation\": a monitor with a probe in service reaches ACTIVE, \
         got {settled:?}"
    );
    let created_at = timestamp(&settled, "createdAt");
    let mut previous = timestamp(&settled, "modifiedAt");
    assert!(
        (previous - created_at).abs() < f64::EPSILON,
        "case \"the clock's activation\": no call changed the monitor, so modifiedAt must stay \
         at createdAt {created_at}, got {settled:?}"
    );

    // (case, the state the probe is put into, the state the monitor must report)
    for (case, probe_state, monitor_state) in [
        ("the only probe switched off", "INACTIVE", "INACTIVE"),
        ("the probe switched back on", "ACTIVE", "PENDING"),
    ] {
        std::thread::sleep(INSTANT_APART);
        let (status, updated) = update_probe(
            &router,
            name,
            &probe_id,
            &format!(r#"{{"state":"{probe_state}"}}"#),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {updated:?}"
        );

        let (status, monitor) = get_monitor(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMonitor expected 200, got {status} {monitor:?}"
        );
        assert_eq!(
            string_at(&monitor, "state"),
            monitor_state,
            "case {case:?}: expected the monitor reported {monitor_state}, got {monitor:?}"
        );
        let now_modified_at = timestamp(&monitor, "modifiedAt");
        assert!(
            now_modified_at > previous,
            "case {case:?}: the update rewrote the monitor's own record, so its modifiedAt must \
             move past {previous}, got {now_modified_at}"
        );
        assert!(
            (now_modified_at - timestamp(&updated, "modifiedAt")).abs() < f64::EPSILON,
            "case {case:?}: one call changed both records, so the monitor's modifiedAt must be \
             the probe's, got {now_modified_at} against {updated:?}"
        );
        assert!(
            (timestamp(&monitor, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: a monitor's createdAt never moves, got {monitor:?}"
        );
        previous = now_modified_at;
    }
}

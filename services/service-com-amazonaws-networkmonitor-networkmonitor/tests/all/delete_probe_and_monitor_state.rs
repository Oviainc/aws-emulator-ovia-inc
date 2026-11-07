// SPDX-License-Identifier: CC0-1.0

//! What an accepted `DeleteProbe` does to the MONITOR's own `state`.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: a
//! monitor's `state` follows the probes it holds that are in service — neither
//! switched off by a caller nor on their way out — so taking a probe away never
//! makes an activation due, and because the draining probe is still held and
//! still reported, the monitor keeps the state it had reached until that record
//! is gone, falling back to `INACTIVE` only then.
//!
//! The drain is SAMPLED rather than read once: the symptom this pins is a state
//! the clock made due during the teardown, so every read across the whole
//! teardown — including the ones after the monitor's activation span would have
//! elapsed — has to report the same state. `GetMonitor` and `ListMonitors`'
//! `state` filter are both checked, since every reporting call settles the
//! monitor before answering.
//!
//! The table spans what the rule turns on: how many probes the monitor holds,
//! how many of them the caller had switched off (none, some, all), and whether
//! the probe had settled into service before it was deleted or was switched off
//! while it was still provisioning. The recorded case — a monitor's only probe
//! deactivated and then deleted — is one row.

use std::time::{Duration, Instant};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, get_monitor, list_monitors, page_names, probe_json, probes_of, router,
    string_at, PAST_ACTIVATION,
};

/// How often the monitor is sampled while a probe drains. Short enough that
/// every stage of the teardown is observed, and the sampling itself is what
/// proves the state is reported rather than computed once.
const SAMPLE_EVERY: Duration = Duration::from_millis(40);

/// The most samples a drain takes before the test gives up waiting for the
/// probe to be gone — far more than the teardown needs, so reaching it is a
/// failure and not a tight budget.
const MAX_SAMPLES: usize = 100;

/// The span this service owes a monitor its `PENDING` -> `ACTIVE` transition
/// (`ACTIVATION_AFTER` in `src/state.rs`, an emulator-chosen span). Mirrored
/// here so a case can assert its drain OUTLASTED it: a monitor that would be
/// activated by the delete would have reached `ACTIVE` within this span, so a
/// drain shorter than it would prove nothing.
const ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The `state` the stored probe is in once a caller has switched it off
/// (`UpdateProbeInput$state`'s documented activate/deactivate toggle).
const PROBE_OFF: &str = "INACTIVE";

/// The first `ProbeState` of the teardown half of a probe's lifecycle
/// (`smithy_query(query="shape",shape="ProbeState")` ->
/// `PENDING | ACTIVE | INACTIVE | ERROR | DELETING | DELETED`).
const PROBE_DRAINING: &str = "DELETING";

/// `PATCH /monitors/{monitorName}/probes/{probeId}` with one `state` member.
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

async fn delete_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

/// Create a monitor holding `probe_count` probes and answer each `probeId`, in
/// the order `GetMonitor` reports them.
async fn monitor_with_probes(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    probe_count: usize,
) -> Vec<String> {
    let probes: Vec<String> = (0..probe_count)
        .map(|i| probe_json(&format!("10.2.0.{}", i + 1), "ICMP", r#","packetSize":56"#))
        .collect();
    let (status, created) = create_monitor(
        router,
        &format!(
            r#"{{"monitorName":"{name}","probes":[{}]}}"#,
            probes.join(",")
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateMonitor expected 200, got {status} {created:?}"
    );
    let (status, got) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup GetMonitor expected 200, got {status} {got:?}"
    );
    let ids: Vec<String> = probes_of(&got)
        .iter()
        .map(|probe| string_at(probe, "probeId"))
        .collect();
    assert_eq!(
        ids.len(),
        probe_count,
        "case {case:?}: setup expected {probe_count} probes, got {got:?}"
    );
    ids
}

/// The monitor's own `state`, and the `state` it lists for `probe_id` (`None`
/// once it no longer lists that probe at all).
async fn sample(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    probe_id: &str,
) -> (String, Option<String>) {
    let (status, monitor) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the monitor itself stays readable, got {status} {monitor:?}"
    );
    let listed = probes_of(&monitor)
        .iter()
        .find(|probe| string_at(probe, "probeId") == probe_id)
        .map(|probe| string_at(probe, "state"));
    (string_at(&monitor, "state"), listed)
}

/// Sample the monitor until it no longer lists `probe_id`, and answer every
/// (monitor state, probe state) pair observed while the probe was still held,
/// in the order they were observed, with how long the drain took.
async fn drain_samples(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    probe_id: &str,
) -> (Vec<(String, String)>, Duration) {
    let started = Instant::now();
    let mut observed: Vec<(String, String)> = Vec::new();
    for _ in 0..MAX_SAMPLES {
        let (monitor_state, probe_state) = sample(router, case, name, probe_id).await;
        match probe_state {
            Some(probe_state) => observed.push((monitor_state, probe_state)),
            None => return (observed, started.elapsed()),
        }
        std::thread::sleep(SAMPLE_EVERY);
    }
    panic!("case {case:?}: the probe was still listed after {MAX_SAMPLES} samples, got {observed:?}")
}

/// `ListMonitors` reports the same stored state `GetMonitor` does, so the state
/// filter selects the monitor on it and on no other.
async fn assert_filter_agrees(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &str,
) {
    let (status, listed) = list_monitors(router, &format!("?state={expected}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMonitors(state={expected}) expected 200, got {status} {listed:?}"
    );
    assert!(
        page_names(&listed).contains(&name.to_string()),
        "case {case:?}: ListMonitors(state={expected}) must list {name:?}, got {listed:?}"
    );
    for other in ["PENDING", "ACTIVE", "INACTIVE"] {
        if other == expected {
            continue;
        }
        let (_, listed) = list_monitors(router, &format!("?state={other}")).await;
        assert!(
            !page_names(&listed).contains(&name.to_string()),
            "case {case:?}: ListMonitors(state={other}) must omit {name:?}, got {listed:?}"
        );
    }
}

/// Deleting a probe never activates its monitor: the monitor keeps the state it
/// had reached for as long as the draining probe is still held, and falls back
/// to `INACTIVE` once nothing it holds is in service any more.
#[tokio::test]
async fn deleting_a_probe_never_activates_its_monitor() {
    // (case, probes held, probes switched off, whether the monitor was left to
    // settle before the delete, the state every read reports while the probe
    // drains, the state once that probe has gone)
    let cases: &[(&str, usize, usize, bool, &str, &str)] = &[
        (
            "the recorded case: the monitor's only probe deactivated, then deleted",
            1,
            1,
            true,
            "INACTIVE",
            "INACTIVE",
        ),
        (
            "two probes, both deactivated, one of them deleted",
            2,
            2,
            true,
            "INACTIVE",
            "INACTIVE",
        ),
        (
            "two probes, the deactivated one deleted while the other still measures",
            2,
            1,
            true,
            "ACTIVE",
            "ACTIVE",
        ),
        (
            "the monitor's only probe deleted while it is measuring",
            1,
            0,
            true,
            "ACTIVE",
            "INACTIVE",
        ),
        (
            "the monitor's only probe switched off while it was still provisioning, \
             then deleted",
            1,
            1,
            false,
            "INACTIVE",
            "INACTIVE",
        ),
    ];

    for (index, (case, probes, switched_off, settle_first, while_draining, once_gone)) in
        cases.iter().enumerate()
    {
        let router = router();
        let name = format!("delete-state-{index}");
        let ids = monitor_with_probes(&router, case, &name, *probes).await;
        if *settle_first {
            std::thread::sleep(PAST_ACTIVATION);
        }
        for probe_id in ids.iter().take(*switched_off) {
            let (status, updated) = set_probe_state(&router, &name, probe_id, PROBE_OFF).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateProbe {PROBE_OFF} expected 200, got {status} {updated:?}"
            );
        }
        let probe_id = ids.first().cloned().unwrap_or_default();
        // The state the delete is about to be judged against: whatever the
        // probes it holds have left the monitor in.
        let (before, _) = sample(&router, case, &name, &probe_id).await;
        assert_eq!(
            before, *while_draining,
            "case {case:?}: before the delete the monitor must already be {while_draining}, \
             got {before:?}"
        );

        let (status, accepted) = delete_probe(&router, &name, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteProbe 200, got {status} {accepted:?}"
        );
        // The very next reporting call, with the teardown just accepted.
        let (immediately, listed) = sample(&router, case, &name, &probe_id).await;
        assert_eq!(
            listed.as_deref(),
            Some(PROBE_DRAINING),
            "case {case:?}: the deleted probe must be listed in {PROBE_DRAINING}, got {listed:?}"
        );
        assert_eq!(
            immediately, *while_draining,
            "case {case:?}: straight after the delete the monitor must still be \
             {while_draining}, got {immediately:?}"
        );
        assert_filter_agrees(&router, case, &name, while_draining).await;

        let (observed, took) = drain_samples(&router, case, &name, &probe_id).await;
        assert!(
            took >= ACTIVATION_SPAN,
            "case {case:?}: the drain must outlast the {ACTIVATION_SPAN:?} activation span for \
             this to say anything, took {took:?} over {observed:?}"
        );
        for (monitor_state, probe_state) in &observed {
            assert_eq!(
                monitor_state, while_draining,
                "case {case:?}: every read while the probe is held reports {while_draining}, \
                 got {monitor_state:?} with the probe in {probe_state:?} over {observed:?}"
            );
        }

        // Once the draining probe is gone the monitor follows what it still
        // holds: nothing in service is INACTIVE, a probe still measuring is not.
        let (settled, listed) = sample(&router, case, &name, &probe_id).await;
        assert_eq!(
            listed, None,
            "case {case:?}: the torn-down probe must be gone from GetMonitor, got {listed:?}"
        );
        assert_eq!(
            settled, *once_gone,
            "case {case:?}: once the probe has gone the monitor must be {once_gone}, \
             got {settled:?}"
        );
        assert_filter_agrees(&router, case, &name, once_gone).await;
    }
}

/// The teardown is idempotent at the monitor level too: a repeated delete of the
/// same probe leaves the monitor exactly where the first one did, and so does
/// the deactivation of the one probe that is left.
#[tokio::test]
async fn repeating_the_delete_of_a_deactivated_probe_leaves_the_monitor_idle() {
    let router = router();
    let case = "the only probe deactivated, then deleted twice";
    let name = "delete-state-repeat";
    let ids = monitor_with_probes(&router, case, name, 1).await;
    let probe_id = ids.first().cloned().unwrap_or_default();
    std::thread::sleep(PAST_ACTIVATION);
    let (status, updated) = set_probe_state(&router, name, &probe_id, PROBE_OFF).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateProbe {PROBE_OFF} expected 200, got {status} {updated:?}"
    );

    for attempt in ["the first delete", "the repeat"] {
        let (status, accepted) = delete_probe(&router, name, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}, {attempt}: expected DeleteProbe 200, got {status} {accepted:?}"
        );
        let (state, listed) = sample(&router, case, name, &probe_id).await;
        assert_eq!(
            listed.as_deref(),
            Some(PROBE_DRAINING),
            "case {case:?}, {attempt}: the probe must still be listed in {PROBE_DRAINING}, \
             got {listed:?}"
        );
        assert_eq!(
            state, "INACTIVE",
            "case {case:?}, {attempt}: a monitor with nothing in service stays INACTIVE, \
             got {state:?}"
        );
    }

    let (observed, took) = drain_samples(&router, case, name, &probe_id).await;
    assert!(
        took >= ACTIVATION_SPAN,
        "case {case:?}: the drain must outlast the {ACTIVATION_SPAN:?} activation span for this \
         to say anything, took {took:?} over {observed:?}"
    );
    for (monitor_state, probe_state) in &observed {
        assert_eq!(
            monitor_state, "INACTIVE",
            "case {case:?}: every read while the probe is held reports INACTIVE, got \
             {monitor_state:?} with the probe in {probe_state:?} over {observed:?}"
        );
    }
    let (settled, listed) = sample(&router, case, name, &probe_id).await;
    assert_eq!(
        listed, None,
        "case {case:?}: the torn-down probe must be gone from GetMonitor, got {listed:?}"
    );
    assert_eq!(
        settled, "INACTIVE",
        "case {case:?}: a monitor whose last probe has gone is INACTIVE, got {settled:?}"
    );
}

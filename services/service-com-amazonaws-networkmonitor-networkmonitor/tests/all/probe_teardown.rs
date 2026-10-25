// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for what an accepted `DeleteProbe` does.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: an
//! accepted probe delete starts an ASYNCHRONOUS teardown — the probe record
//! stays readable, moves through the teardown half of the modelled lifecycle
//! (`DELETING`, then `DELETED`) over spans the clock owes, and only once the
//! teardown has run its course does every read of that probe report
//! `ResourceNotFoundException`. While it is draining the probe is still part of
//! its monitor (`GetMonitor` lists it, its ARN still resolves for
//! `ListTagsForResource`), the teardown is irreversible (no update puts the
//! probe back into service) and idempotent (a repeat neither restarts nor
//! postpones it), and the monitor returns to `INACTIVE` only when the last probe
//! has actually gone.
//!
//! The tables span the class the rule covers rather than one recorded probe:
//! both address families, both protocols, a probe deleted while it is still
//! provisioning and one deleted once it is live, one probe and several, and both
//! states a caller may ask a probe to be put into.

use std::time::{Duration, Instant};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, get_monitor, probe_json, probes_of, router,
    string_at, PAST_ACTIVATION, PAST_TEARDOWN,
};
use super::resource_tags::list_tags;

/// The `ProbeState` values the model declares for the TEARDOWN half of a
/// probe's lifecycle, in the order a teardown reaches them
/// (`smithy_query(query="shape",shape="ProbeState")` ->
/// `PENDING | ACTIVE | INACTIVE | ERROR | DELETING | DELETED`; the other four
/// belong to a probe that is being provisioned, is in service, or failed).
const TEARDOWN_STATES: &[&str] = &["DELETING", "DELETED"];

/// How often a drain is sampled. Short enough that every stage of a teardown is
/// observed at least once, and the sampling itself is what proves the states are
/// reported rather than computed from one read.
const SAMPLE_EVERY: Duration = Duration::from_millis(40);

/// The most samples a drain takes before the test gives up waiting for the
/// probe to be gone — far more than the teardown needs, so reaching it is a
/// failure and not a tight budget.
const MAX_SAMPLES: usize = 100;

/// Longer than the FIRST stage of a probe's teardown and shorter than the whole
/// of it, so a read after this sleep observes a probe whose teardown has
/// completed while its record is still readable. A blocking sleep, for the
/// reason given on `PAST_ACTIVATION`.
const PAST_FIRST_TEARDOWN_STAGE: Duration = Duration::from_millis(400);

async fn get_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
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

/// Create a monitor holding one probe built from `probe` (a bare probe body) and
/// answer the probe's minted id and ARN.
async fn probe_fixture(
    router: &MultiServiceRouter,
    name: &str,
    probe: &str,
) -> (String, String) {
    let (status, created) = create_monitor(router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor {name:?}: expected 200, got {status} {created:?}"
    );
    let (status, created) = create_probe(router, name, &format!(r#"{{"probe":{probe}}}"#)).await;
    assert_eq!(
        status, 200,
        "setup CreateProbe on {name:?}: expected 200, got {status} {created:?}"
    );
    (
        string_at(&created, "probeId"),
        string_at(&created, "probeArn"),
    )
}

/// The `state` a `GetMonitor` body reports for the probe with this id, or
/// `"<not listed>"` when the monitor does not list it at all.
fn listed_state(monitor: &Value, probe_id: &str) -> String {
    probes_of(monitor)
        .iter()
        .find(|probe| string_at(probe, "probeId") == probe_id)
        .map_or_else(|| "<not listed>".to_string(), |probe| string_at(probe, "state"))
}

/// Sample `GetProbe` until it reports the probe not found, and answer every
/// state it reported on the way, in the order it reported them.
async fn drain(router: &MultiServiceRouter, name: &str, probe_id: &str) -> Vec<String> {
    let mut observed = Vec::new();
    for _ in 0..MAX_SAMPLES {
        let (status, body) = get_probe(router, name, probe_id).await;
        if status == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "a probe whose teardown finished is reported not found, got {body:?}"
            );
            return observed;
        }
        assert_eq!(
            status, 200,
            "a draining probe is still readable, got {status} {body:?}"
        );
        observed.push(string_at(&body, "state"));
        std::thread::sleep(SAMPLE_EVERY);
    }
    panic!(
        "the probe was still readable after {} samples, states {observed:?}",
        MAX_SAMPLES
    );
}

/// An accepted delete drains the probe through the modelled teardown states and
/// only then makes every read of it not-found.
///
/// The table spans the class: both address families, both protocols (so a probe
/// with a `destinationPort` and one without), a tagged probe, `packetSize` at
/// both published bounds of its `@range(56..8500)`, and a probe deleted while it
/// is still provisioning beside ones deleted once they are live. The recorded
/// case (a live ICMP IPv4 probe) is one row.
#[tokio::test]
async fn an_accepted_delete_drains_the_probe_through_the_modelled_teardown_states() {
    // (case, probe body, whether the probe is live before the delete)
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "the recorded case: a live ICMP IPv4 probe",
            probe_json("10.0.0.1", "ICMP", r#","packetSize":56"#),
            true,
        ),
        (
            "a live TCP probe with a destinationPort, packetSize at the ceiling",
            probe_json("192.0.2.77", "TCP", r#","destinationPort":443,"packetSize":8500"#),
            true,
        ),
        (
            "a live, tagged IPv6 probe",
            probe_json("2001:db8::1", "ICMP", r#","packetSize":56,"tags":{"role":"edge"}"#),
            true,
        ),
        (
            "a probe deleted while it is still provisioning",
            probe_json("10.0.0.2", "ICMP", r#","packetSize":56"#),
            false,
        ),
    ];

    for (case, probe, live) in cases {
        let router = router();
        let (probe_id, probe_arn) = probe_fixture(&router, "teardown-host", &probe).await;
        if live {
            std::thread::sleep(PAST_ACTIVATION);
            let (_, got) = get_probe(&router, "teardown-host", &probe_id).await;
            assert_eq!(
                string_at(&got, "state"),
                "ACTIVE",
                "case {case:?}: the probe must be live before the delete, got {got:?}"
            );
        }

        let (status, accepted) = delete_probe(&router, "teardown-host", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteProbe 200, got {status} {accepted:?}"
        );

        // The very next read still answers, with the teardown under way.
        let (status, got) = get_probe(&router, "teardown-host", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deleted probe is still readable, got {status} {got:?}"
        );
        assert_eq!(
            string_at(&got, "state"),
            TEARDOWN_STATES[0],
            "case {case:?}: expected {:?} straight after the delete, got {got:?}",
            TEARDOWN_STATES[0]
        );
        // Its configuration is untouched while it drains, and it is still part
        // of the monitor and still a resource its ARN resolves to.
        assert_eq!(
            string_at(&got, "destination"),
            string_at(&serde_json::from_str::<Value>(&probe).expect("a probe body"), "destination"),
            "case {case:?}: a draining probe keeps its configuration, got {got:?}"
        );
        let (_, monitor) = get_monitor(&router, "teardown-host").await;
        assert_eq!(
            listed_state(&monitor, &probe_id),
            TEARDOWN_STATES[0],
            "case {case:?}: the monitor still lists the draining probe, got {monitor:?}"
        );
        let (status, tags) = list_tags(&router, &probe_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a draining probe's ARN still resolves, got {status} {tags:?}"
        );

        let observed = drain(&router, "teardown-host", &probe_id).await;
        assert_eq!(
            observed.first().map(String::as_str),
            Some(TEARDOWN_STATES[0]),
            "case {case:?}: the drain starts in {:?}, observed {observed:?}",
            TEARDOWN_STATES[0]
        );
        assert!(
            observed.iter().all(|state| TEARDOWN_STATES.contains(&state.as_str())),
            "case {case:?}: a deleted probe is only ever reported in {TEARDOWN_STATES:?}, \
             observed {observed:?}"
        );
        for state in TEARDOWN_STATES {
            assert!(
                observed.iter().any(|seen| seen == state),
                "case {case:?}: the teardown must be observable in {state:?}, observed {observed:?}"
            );
        }
        // The lifecycle only ever moves forward: no sample reports an earlier
        // teardown state than one already seen.
        let mut reached = 0usize;
        for state in &observed {
            let position = TEARDOWN_STATES
                .iter()
                .position(|known| known == state)
                .unwrap_or(0);
            assert!(
                position >= reached,
                "case {case:?}: the teardown went backwards to {state:?}, observed {observed:?}"
            );
            reached = position;
        }

        // Once it is gone it is gone from every read of it.
        let (status, monitor) = get_monitor(&router, "teardown-host").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the monitor itself is untouched, got {status} {monitor:?}"
        );
        assert_eq!(
            listed_state(&monitor, &probe_id),
            "<not listed>",
            "case {case:?}: the torn-down probe must be gone from GetMonitor, got {monitor:?}"
        );
        assert_eq!(
            string_at(&monitor, "state"),
            "INACTIVE",
            "case {case:?}: a monitor whose last probe is gone has nothing to aggregate, \
             got {monitor:?}"
        );
        let (status, tags) = list_tags(&router, &probe_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a torn-down probe's ARN resolves to nothing, got {status} {tags:?}"
        );
    }
}

/// A monitor keeps every probe it still holds while one of them drains, and
/// returns to `INACTIVE` only when the last one has actually gone.
///
/// The table spans how many probes the monitor holds, since that is what the
/// monitor's own state follows.
#[tokio::test]
async fn a_monitor_keeps_aggregating_until_its_last_probe_has_gone() {
    let router = router();
    let (status, created) = create_monitor(
        &router,
        &format!(
            r#"{{"monitorName":"draining","probes":[{},{}]}}"#,
            probe_json("10.0.1.1", "ICMP", r#","packetSize":56"#),
            probe_json("10.0.1.2", "TCP", r#","destinationPort":443,"packetSize":56"#)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateMonitor 200, got {status} {created:?}"
    );
    std::thread::sleep(PAST_ACTIVATION);

    let (_, monitor) = get_monitor(&router, "draining").await;
    let mut ids: Vec<String> = probes_of(&monitor)
        .iter()
        .map(|probe| string_at(probe, "probeId"))
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "setup: the monitor must hold both probes, got {monitor:?}"
    );
    let second = ids.pop().expect("two probes");
    let first = ids.pop().expect("two probes");

    // (case, the probe deleted, how many probes the monitor keeps while it
    // drains, the monitor state during the drain, the state once it has gone)
    let cases: Vec<(&str, String, usize, &str, &str)> = vec![
        (
            "one of two probes deleted",
            first,
            2,
            "ACTIVE",
            "ACTIVE",
        ),
        (
            "the monitor's last probe deleted",
            second,
            1,
            "ACTIVE",
            "INACTIVE",
        ),
    ];

    for (case, probe_id, listed_while_draining, draining_state, settled_state) in cases {
        let (status, accepted) = delete_probe(&router, "draining", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteProbe 200, got {status} {accepted:?}"
        );

        let (_, monitor) = get_monitor(&router, "draining").await;
        assert_eq!(
            probes_of(&monitor).len(),
            listed_while_draining,
            "case {case:?}: expected {listed_while_draining} probes while one drains, \
             got {monitor:?}"
        );
        assert_eq!(
            listed_state(&monitor, &probe_id),
            TEARDOWN_STATES[0],
            "case {case:?}: the draining probe is listed in {:?}, got {monitor:?}",
            TEARDOWN_STATES[0]
        );
        assert_eq!(
            string_at(&monitor, "state"),
            draining_state,
            "case {case:?}: expected the monitor in {draining_state:?} while a probe drains, \
             got {monitor:?}"
        );

        std::thread::sleep(PAST_TEARDOWN);
        let (_, monitor) = get_monitor(&router, "draining").await;
        assert_eq!(
            probes_of(&monitor).len(),
            listed_while_draining - 1,
            "case {case:?}: expected {} probes once the teardown finished, got {monitor:?}",
            listed_while_draining - 1
        );
        assert_eq!(
            string_at(&monitor, "state"),
            settled_state,
            "case {case:?}: expected the monitor in {settled_state:?} once the teardown \
             finished, got {monitor:?}"
        );
    }
}

/// The teardown is IRREVERSIBLE: a probe on its way out is no longer a target
/// for an update, so no call puts it back into service or changes what it still
/// reports.
///
/// The table spans both states a caller may ask a probe to be put into
/// (`UpdateProbeInput$state`'s documented activate/deactivate toggle) and an
/// update of a plain configuration member.
#[tokio::test]
async fn a_probe_being_torn_down_cannot_be_put_back_into_service() {
    // (case, PATCH body)
    let cases: &[(&str, &str)] = &[
        ("asked back into service", r#"{"state":"ACTIVE"}"#),
        ("asked to be deactivated", r#"{"state":"INACTIVE"}"#),
        ("asked for a bigger packetSize", r#"{"packetSize":1500}"#),
    ];

    for (case, body) in cases {
        let router = router();
        let (probe_id, _) = probe_fixture(
            &router,
            "irreversible",
            &probe_json("10.0.2.1", "ICMP", r#","packetSize":56"#),
        )
        .await;
        std::thread::sleep(PAST_ACTIVATION);
        let (status, accepted) = delete_probe(&router, "irreversible", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteProbe 200, got {status} {accepted:?}"
        );

        let (status, refused) = update_probe(&router, "irreversible", &probe_id, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the update of a draining probe to be 404, \
             got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );

        // The refused update changed nothing: the probe is still draining and
        // still reports the configuration it was created with.
        let (status, got) = get_probe(&router, "irreversible", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the draining probe is still readable, got {status} {got:?}"
        );
        assert!(
            TEARDOWN_STATES.contains(&string_at(&got, "state").as_str()),
            "case {case:?}: a draining probe stays in {TEARDOWN_STATES:?}, got {got:?}"
        );
        assert_eq!(
            got.get("packetSize"),
            Some(&Value::from(56)),
            "case {case:?}: a refused update stores nothing, got {got:?}"
        );

        std::thread::sleep(PAST_TEARDOWN);
        let (status, gone) = get_probe(&router, "irreversible", &probe_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the teardown still finishes, got {status} {gone:?}"
        );
    }
}

/// `DeleteProbe` is `@idempotent`, and so is the teardown it starts: a repeat
/// answers the same 200, does not put the probe back into an earlier teardown
/// state, and does not move the instant the probe stops being readable.
#[tokio::test]
async fn repeating_the_delete_neither_restarts_nor_postpones_the_teardown() {
    let router = router();
    let (probe_id, _) = probe_fixture(
        &router,
        "repeat-delete",
        &probe_json("10.0.3.1", "ICMP", r#","packetSize":56"#),
    )
    .await;
    std::thread::sleep(PAST_ACTIVATION);

    let accepted_at = Instant::now();
    // (case, how long after the accepted delete the repeat is sent, the state
    // the probe must still be reported in afterwards)
    let cases: Vec<(&str, Duration, &str)> = vec![
        ("a repeat sent straight away", Duration::ZERO, TEARDOWN_STATES[0]),
        (
            "a repeat sent once the teardown completed",
            PAST_FIRST_TEARDOWN_STAGE,
            TEARDOWN_STATES[1],
        ),
    ];

    let (status, accepted) = delete_probe(&router, "repeat-delete", &probe_id).await;
    assert_eq!(
        status, 200,
        "case \"the accepted delete\": expected 200, got {status} {accepted:?}"
    );

    for (case, after, want_state) in cases {
        if let Some(remaining) = after.checked_sub(accepted_at.elapsed()) {
            std::thread::sleep(remaining);
        }
        let (status, repeated) = delete_probe(&router, "repeat-delete", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated delete answers the same 200, got {status} {repeated:?}"
        );
        let (status, got) = get_probe(&router, "repeat-delete", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the probe is still draining, got {status} {got:?}"
        );
        assert_eq!(
            string_at(&got, "state"),
            want_state,
            "case {case:?}: a repeat must not restart the teardown, so the probe stays in \
             {want_state:?}, got {got:?}"
        );
    }

    // The deadline the FIRST delete set still holds: the repeats did not push it
    // out.
    if let Some(remaining) = PAST_TEARDOWN.checked_sub(accepted_at.elapsed()) {
        std::thread::sleep(remaining);
    }
    let (status, gone) = get_probe(&router, "repeat-delete", &probe_id).await;
    assert_eq!(
        status, 404,
        "case \"the deadline the first delete set\": the probe must be gone {PAST_TEARDOWN:?} \
         after that delete, got {status} {gone:?}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "case \"the deadline the first delete set\": expected ResourceNotFoundException, \
         got {gone:?}"
    );
}

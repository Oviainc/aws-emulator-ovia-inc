// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the two PRE-DELETE guards: which deletes this service
//! accepts, and which it refuses because something it holds is still in service.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rules under test:
//!
//! * `DeleteMonitor` gives a monitor up only once nothing of it is still
//!   measuring: while the monitor holds a probe that is IN SERVICE — one the
//!   caller has neither switched off nor already asked to delete — the call is
//!   refused with HTTP 400 `ValidationException` and nothing is removed, so the
//!   monitor and every probe it holds are still readable afterwards. A monitor
//!   whose probes are all deactivated, all already being torn down, or simply
//!   absent, is deleted, and every read of that name is then 404;
//! * `DeleteProbe` tears a probe down only once it has SETTLED: while the probe
//!   is still being provisioned (`PENDING`) the call is refused with HTTP 400
//!   `ValidationException` and the probe is left exactly as it was — no teardown
//!   is started. A probe that has left `PENDING`, whether the clock carried it
//!   into service or the caller switched it off, is torn down.
//!
//! Both refusals are `ValidationException`, which is a SERVICE-COMMON error of
//! this model (`smithy_query(query="service_info")` lists it under
//! `common_errors`), and both are judged BEFORE anything is written, which is
//! what the "still readable afterwards" assertions pin.
//!
//! The tables span what the rules turn on rather than one recorded call: how
//! many probes the monitor holds (none, one, several), which state each of them
//! is in (provisioning, measuring, switched off, draining), and — for the probe
//! guard — both protocols, both address families and `packetSize` at both
//! published bounds of its `@range(56..8500)`.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, error_message, get_monitor, probe_json,
    probes_of, router, source_arn_of, string_at, PAST_ACTIVATION,
};
use super::probe_teardown::get_probe;

/// The error code both guards state their refusal with.
const REFUSAL_CODE: &str = "ValidationException";

/// The code every read of a resource this service no longer holds answers with.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// `DELETE /monitors/{monitorName}` — `DeleteMonitor`.
async fn delete_monitor(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/monitors/{name}"), None).await
}

/// `DELETE /monitors/{monitorName}/probes/{probeId}` — `DeleteProbe`.
async fn delete_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

/// `PATCH /monitors/{monitorName}/probes/{probeId}` — `UpdateProbe`.
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

/// The sentence `DeleteMonitor` refuses with while the monitor still holds a
/// probe in service, built from the name the request carried.
fn monitor_refusal(monitor_name: &str) -> String {
    format!("All probes on monitor {monitor_name} must be inactive before deletion.")
}

/// The sentence `DeleteProbe` refuses with while the probe is still being
/// provisioned, built from the id the request carried.
fn probe_refusal(probe_id: &str) -> String {
    format!("Probe {probe_id} must not be in pending state before deletion.")
}

/// Create the monitor `name` holding `probes` probes, each on a subnet of its
/// own so no per-subnet limit is in the way, and answer their ids in creation
/// order.
async fn monitor_with_probes(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    probes: usize,
) -> Vec<String> {
    let (status, created) = create_monitor(router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateMonitor expected 200, got {status} {created:?}"
    );
    let mut ids = Vec::new();
    for index in 0..probes {
        let body = format!(
            r#"{{"probe":{{"sourceArn":"{}","destination":"10.0.{index}.1","protocol":"ICMP","packetSize":56}}}}"#,
            source_arn_of(index)
        );
        let (status, probe) = create_probe(router, name, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateProbe expected 200, got {status} {probe:?}"
        );
        ids.push(string_at(&probe, "probeId"));
    }
    ids
}

/// What a row does to each probe of its fixture before the monitor's delete is
/// tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prepare {
    /// Leave it provisioning: no wait, no call.
    Provisioning,
    /// Wait for the clock to carry it into service.
    Live,
    /// Switch it off through `UpdateProbe`.
    SwitchedOff,
    /// Accept its own teardown through `DeleteProbe`, once it is live.
    Draining,
}

/// RULE: a monitor is deleted only once nothing it holds is still in service.
/// While it holds a probe that is neither switched off nor already being torn
/// down the delete is refused with 400 `ValidationException`, and nothing is
/// removed; otherwise it is accepted and every read of that name is 404.
#[tokio::test]
async fn a_monitor_is_deleted_only_once_nothing_it_holds_is_in_service() {
    // (case, what each probe of the fixture is put into, whether the delete is
    // refused)
    let rows: &[(&str, &[Prepare], bool)] = &[
        ("a monitor holding no probe at all", &[], false),
        (
            "the recorded case: one probe still provisioning",
            &[Prepare::Provisioning],
            true,
        ),
        ("one probe measuring", &[Prepare::Live], true),
        (
            "two probes, one switched off and one still measuring",
            &[Prepare::SwitchedOff, Prepare::Live],
            true,
        ),
        (
            "two probes, one switched off and one still provisioning",
            &[Prepare::SwitchedOff, Prepare::Provisioning],
            true,
        ),
        ("one probe switched off", &[Prepare::SwitchedOff], false),
        (
            "two probes, both switched off",
            &[Prepare::SwitchedOff, Prepare::SwitchedOff],
            false,
        ),
        (
            "one probe whose own teardown is already under way",
            &[Prepare::Draining],
            false,
        ),
        (
            "two probes, one switched off and one already draining",
            &[Prepare::SwitchedOff, Prepare::Draining],
            false,
        ),
    ];

    for (index, (case, prepare, refused)) in rows.iter().enumerate() {
        let router = router();
        let name = format!("guarded-{index}");
        let ids = monitor_with_probes(&router, case, &name, prepare.len()).await;
        // A probe that has to leave `PENDING` for its row waits once for the
        // whole fixture: the clock owes every probe of it the same transition.
        if prepare
            .iter()
            .any(|step| matches!(step, Prepare::Live | Prepare::Draining))
        {
            std::thread::sleep(PAST_ACTIVATION);
        }
        for (probe_id, step) in ids.iter().zip(prepare.iter()) {
            match step {
                Prepare::Provisioning | Prepare::Live => {}
                Prepare::SwitchedOff => {
                    let (status, off) =
                        update_probe(&router, &name, probe_id, r#"{"state":"INACTIVE"}"#).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: setup UpdateProbe expected 200, got {status} {off:?}"
                    );
                }
                Prepare::Draining => {
                    let (status, accepted) = delete_probe(&router, &name, probe_id).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: setup DeleteProbe expected 200, got {status} {accepted:?}"
                    );
                }
            }
        }

        let (status, answered) = delete_monitor(&router, &name).await;
        if *refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the delete to be refused with 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                REFUSAL_CODE,
                "case {case:?}: expected {REFUSAL_CODE}, got {answered:?}"
            );
            assert_eq!(
                error_message(&answered),
                monitor_refusal(&name),
                "case {case:?}: expected the guard's wording, got {answered:?}"
            );

            // Nothing was removed: the monitor and every probe it holds are
            // still readable, and the probes are still the ones it had.
            let (status, read) = get_monitor(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a refused delete leaves the monitor readable, got {status} {read:?}"
            );
            let listed: Vec<String> = probes_of(&read)
                .iter()
                .map(|probe| string_at(probe, "probeId"))
                .collect();
            assert_eq!(
                listed, ids,
                "case {case:?}: a refused delete leaves every probe where it was, got {read:?}"
            );
            for probe_id in &ids {
                let (status, probe) = get_probe(&router, &name, probe_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: probe {probe_id} is still readable, got {status} {probe:?}"
                );
            }

            // Switching the remaining probes off is what the refusal asks for,
            // and the very same delete is then accepted.
            for probe_id in &ids {
                let (status, off) =
                    update_probe(&router, &name, probe_id, r#"{"state":"INACTIVE"}"#).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected UpdateProbe 200 for {probe_id}, got {status} {off:?}"
                );
            }
            let (status, accepted) = delete_monitor(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: with nothing in service the same delete is accepted, \
                 got {status} {accepted:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteMonitor 200, got {status} {answered:?}"
            );
        }

        // Either way the monitor is gone by now, and reads of it say so.
        let (status, gone) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted monitor is gone, got {status} {gone:?}"
        );
        assert_eq!(
            error_code(&gone),
            NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {gone:?}"
        );
    }
}

/// RULE: a probe is torn down only once it has left `PENDING`. The refusal
/// leaves the probe exactly as it was, and the same delete is accepted once the
/// probe has settled — whether the clock carried it into service or the caller
/// switched it off.
///
/// The table spans both protocols (so a probe with a `destinationPort` and one
/// without), both address families, and `packetSize` at both published bounds of
/// its `@range(56..8500)`.
#[tokio::test]
async fn a_probe_is_torn_down_only_once_it_has_left_pending() {
    // (case, the probe body, how the probe leaves PENDING after the refusal)
    let rows: Vec<(&str, String, Prepare)> = vec![
        (
            "the recorded case: an ICMP IPv4 probe, left to settle",
            probe_json("192.168.64.64", "ICMP", r#","packetSize":56"#),
            Prepare::Live,
        ),
        (
            "a TCP probe with a destinationPort, left to settle",
            probe_json("192.0.2.77", "TCP", r#","destinationPort":443,"packetSize":8500"#),
            Prepare::Live,
        ),
        (
            "an IPv6 probe switched off instead of settled",
            probe_json("2001:db8::7", "ICMP", r#","packetSize":56"#),
            Prepare::SwitchedOff,
        ),
        (
            "a tagged ICMP probe switched off instead of settled",
            probe_json("10.0.9.9", "ICMP", r#","packetSize":56,"tags":{"role":"edge"}"#),
            Prepare::SwitchedOff,
        ),
    ];

    for (index, (case, probe, leaves_pending)) in rows.into_iter().enumerate() {
        let router = router();
        let name = format!("pending-guard-{index}");
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateMonitor expected 200, got {status} {created:?}"
        );
        let (status, minted) = create_probe(&router, &name, &format!(r#"{{"probe":{probe}}}"#)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateProbe expected 200, got {status} {minted:?}"
        );
        let probe_id = string_at(&minted, "probeId");
        assert_eq!(
            string_at(&minted, "state"),
            "PENDING",
            "case {case:?}: a freshly created probe is still provisioning, got {minted:?}"
        );

        // The delete of a probe that is still provisioning is refused, and the
        // refusal names the probe the request addressed.
        let (status, answered) = delete_probe(&router, &name, &probe_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused with 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            REFUSAL_CODE,
            "case {case:?}: expected {REFUSAL_CODE}, got {answered:?}"
        );
        assert_eq!(
            error_message(&answered),
            probe_refusal(&probe_id),
            "case {case:?}: expected the guard's wording, got {answered:?}"
        );

        // The refused delete started no teardown: the probe is readable, still
        // provisioning, and its stored configuration is untouched.
        let (status, untouched) = get_probe(&router, &name, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete leaves the probe readable, got {status} {untouched:?}"
        );
        assert_eq!(
            string_at(&untouched, "state"),
            "PENDING",
            "case {case:?}: a refused delete leaves the probe provisioning, got {untouched:?}"
        );
        let requested: Value = serde_json::from_str(&probe).expect("a probe body");
        assert_eq!(
            string_at(&untouched, "destination"),
            string_at(&requested, "destination"),
            "case {case:?}: a refused delete leaves the configuration alone, got {untouched:?}"
        );

        // Once the probe has left `PENDING` the SAME delete is accepted.
        match leaves_pending {
            Prepare::Live => {
                std::thread::sleep(PAST_ACTIVATION);
                let (_, settled) = get_probe(&router, &name, &probe_id).await;
                assert_eq!(
                    string_at(&settled, "state"),
                    "ACTIVE",
                    "case {case:?}: the clock owes the probe its activation, got {settled:?}"
                );
            }
            Prepare::SwitchedOff => {
                let (status, off) =
                    update_probe(&router, &name, &probe_id, r#"{"state":"INACTIVE"}"#).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected UpdateProbe 200, got {status} {off:?}"
                );
                assert_eq!(
                    string_at(&off, "state"),
                    "INACTIVE",
                    "case {case:?}: the probe is switched off, got {off:?}"
                );
            }
            other => panic!("case {case:?}: {other:?} is not a way out of PENDING"),
        }

        let (status, accepted) = delete_probe(&router, &name, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a settled probe is torn down, got {status} {accepted:?}"
        );
        let (status, draining) = get_probe(&router, &name, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accepted delete leaves the record readable, got {status} {draining:?}"
        );
        assert_eq!(
            string_at(&draining, "state"),
            "DELETING",
            "case {case:?}: the accepted delete started the teardown, got {draining:?}"
        );
    }
}

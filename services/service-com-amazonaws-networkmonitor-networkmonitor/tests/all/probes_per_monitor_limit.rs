// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the bound on how many probes ONE monitor holds.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: the
//! probes a monitor already holds and the probes a request asks to add are
//! counted TOGETHER against [`MAX_PROBES_PER_MONITOR`], and a request that
//! would take the monitor past that bound is refused with HTTP 400
//! `ValidationException` and changes nothing — whichever operation adds the
//! probes, `CreateProbe` one at a time or `CreateMonitor`'s inline `probes`
//! list. Both paths reach that one decision through
//! `crate::state::check_probes_per_monitor`.
//!
//! The tables span the class the rule covers rather than one request: how many
//! probes the monitor already holds, how many the call adds, where the total
//! falls relative to the bound (below it, at it, one past it, far past it),
//! which path put the earlier probes there, and which monitor holds them.

use serde_json::Value;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probes_of, router,
    source_arn_of,
};

/// The ceiling the service applies to the probes of one monitor
/// (`crate::state::MAX_PROBES_PER_MONITOR`).
const MAX_PROBES_PER_MONITOR: usize = 100;

/// The sibling per-subnet ceiling (`crate::state::MAX_PROBES_PER_SUBNET`), which
/// is why a case that is not about it spreads its probes over subnets.
const MAX_PROBES_PER_SUBNET: usize = 4;

/// What the refusal of the per-monitor ceiling says
/// (`crate::state::PROBES_PER_MONITOR_MESSAGE`).
const LIMIT_MESSAGE: &str = "Maximum probes per monitor limit exceeded.";

/// One probe body for the `position`th probe of a monitor: spread over subnets
/// so a run of probes stays below [`MAX_PROBES_PER_SUBNET`] in every subnet and
/// is therefore judged only on the per-monitor count, with a destination of its
/// own so no two probes are the same request twice.
fn probe_at(position: usize) -> String {
    probe_in_subnet(position / MAX_PROBES_PER_SUBNET, position)
}

/// One probe body sourced from subnet `subnet`, with the destination
/// [`probe_at`] would give the `position`th probe.
fn probe_in_subnet(subnet: usize, position: usize) -> String {
    format!(
        r#"{{"sourceArn":"{}","destination":"203.0.{}.{}","protocol":"ICMP","packetSize":56}}"#,
        source_arn_of(subnet),
        position / 250,
        position % 250 + 1
    )
}

/// A `CreateMonitor` body whose inline `probes` list holds `total` probes.
fn create_monitor_body(name: &str, total: usize) -> String {
    let probes: Vec<String> = (0..total).map(probe_at).collect();
    format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
        probes.join(",")
    )
}

/// Assert that `body` is the refusal of this ceiling.
fn assert_limit_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 past the per-monitor bound, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    assert_eq!(
        error_message(body),
        LIMIT_MESSAGE,
        "case {case:?}: expected the refusal to say {LIMIT_MESSAGE:?}, got {body:?}"
    );
}

/// `CreateMonitor`'s inline probes are bounded as a whole, and a refused create
/// stores no monitor at all.
///
/// The table spans where the requested count falls relative to the bound: one
/// probe, one below it, exactly at it, one past it, far past it.
#[tokio::test]
async fn create_monitor_bounds_its_inline_probes_per_monitor() {
    // (case, probes the request asks for, accepted)
    let cases: &[(&str, usize, bool)] = &[
        ("one probe", 1, true),
        ("one probe below the bound", MAX_PROBES_PER_MONITOR - 1, true),
        ("exactly the bound", MAX_PROBES_PER_MONITOR, true),
        ("one probe past the bound", MAX_PROBES_PER_MONITOR + 1, false),
        ("far past the bound", MAX_PROBES_PER_MONITOR * 2 + 1, false),
    ];
    let router = router();
    for (index, (case, total, accepted)) in cases.iter().enumerate() {
        let name = format!("inline-{index}");
        let (status, created) = create_monitor(&router, &create_monitor_body(&name, *total)).await;
        if !*accepted {
            assert_limit_refusal(case, status, &created);
            // A refused create stores nothing: the monitor it named was never
            // made, so every read of that name is not-found.
            let (status, missing) = get_monitor(&router, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create stores no monitor, got {status} {missing:?}"
            );
            assert_eq!(
                error_code(&missing),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {missing:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            probes_of(&got).len(),
            *total,
            "case {case:?}: expected {total} stored probes, got {}",
            probes_of(&got).len()
        );
    }
}

/// The ceiling counts the probes a monitor ALREADY holds: `CreateProbe` is
/// accepted while the monitor is below the bound and refused once it is at it,
/// even for a probe whose own subnet is empty.
///
/// The table spans how many of the probes already there `CreateMonitor` created
/// inline and how many `CreateProbe` added on top, from all of them inline to
/// three added one at a time.
#[tokio::test]
async fn create_probe_counts_the_probes_the_monitor_already_holds() {
    // (case, probes created inline, probes then added one at a time)
    let cases: &[(&str, usize, usize)] = &[
        (
            "every earlier probe created inline",
            MAX_PROBES_PER_MONITOR,
            0,
        ),
        (
            "the last probe added through CreateProbe",
            MAX_PROBES_PER_MONITOR - 1,
            1,
        ),
        (
            "the last three probes added through CreateProbe",
            MAX_PROBES_PER_MONITOR - 3,
            3,
        ),
    ];

    for (index, (case, inline, added)) in cases.iter().enumerate() {
        let router = router();
        let name = format!("mixed-{index}");
        let (status, created) = create_monitor(&router, &create_monitor_body(&name, *inline)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the setup CreateMonitor 200, got {status} {created:?}"
        );

        // Up to the bound, every add is accepted. Each goes into one subnet of
        // its own, which holds no more than the per-subnet bound, so the only
        // count that can refuse it is the monitor's.
        for position in 0..*added {
            let body = format!(r#"{{"probe":{}}}"#, probe_in_subnet(900, position));
            let (status, probe) = create_probe(&router, &name, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: probe {} of the monitor must be accepted, got {status} {probe:?}",
                inline + position + 1
            );
        }

        // One past the bound is refused, in a subnet of its own so the refusal
        // is the monitor-wide count and not the per-subnet one.
        let body = format!(r#"{{"probe":{}}}"#, probe_in_subnet(901, 0));
        let (status, refused) = create_probe(&router, &name, &body).await;
        assert_limit_refusal(case, status, &refused);
        let (_, got) = get_monitor(&router, &name).await;
        assert_eq!(
            probes_of(&got).len(),
            MAX_PROBES_PER_MONITOR,
            "case {case:?}: the refused create must leave \
             {MAX_PROBES_PER_MONITOR} probes stored, got {}",
            probes_of(&got).len()
        );
    }
}

/// The ceiling is counted per MONITOR: one monitor at the bound leaves every
/// other monitor free to hold its own full complement.
#[tokio::test]
async fn the_ceiling_is_counted_per_monitor() {
    let router = router();
    for name in ["tenant-one", "tenant-two"] {
        let (status, created) =
            create_monitor(&router, &create_monitor_body(name, MAX_PROBES_PER_MONITOR)).await;
        assert_eq!(
            status, 200,
            "monitor {name:?}: a full monitor elsewhere leaves this one free, \
             got {status} {created:?}"
        );
        let body = format!(r#"{{"probe":{}}}"#, probe_in_subnet(902, 0));
        let (status, refused) = create_probe(&router, name, &body).await;
        assert_limit_refusal(name, status, &refused);
        let (_, got) = get_monitor(&router, name).await;
        assert_eq!(
            probes_of(&got).len(),
            MAX_PROBES_PER_MONITOR,
            "monitor {name:?}: expected {MAX_PROBES_PER_MONITOR} probes stored, got {}",
            probes_of(&got).len()
        );
    }
}

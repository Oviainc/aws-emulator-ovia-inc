// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the bound on how many probes of one monitor may name
//! one source subnet.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: a
//! monitor holds at most [`MAX_PROBES_PER_SUBNET`] probes whose `sourceArn`
//! names one subnet, counted per (monitor, subnet) pair, and a request that
//! would take any subnet past that bound is refused with HTTP 400
//! `ValidationException` and changes nothing — whichever operation adds the
//! probes, `CreateProbe` one at a time or `CreateMonitor`'s inline `probes`
//! list.
//!
//! The tables span the class the rule covers rather than one recorded request:
//! how many probes the request asks for (at the bound, one past it, far past
//! it), how they are spread over subnets (one subnet, several, several with only
//! one of them over the bound), which path created the probes already there
//! (inline, `CreateProbe`, a mix), and which monitor holds them.

use serde_json::Value;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probes_of, router,
    source_arn_of, string_at,
};

/// The default value of the AWS quota "Number of probes per subnet for each
/// monitor", which is the bound the service applies
/// (`crate::state::MAX_PROBES_PER_SUBNET`).
const MAX_PROBES_PER_SUBNET: usize = 4;

/// What the refusal of that bound says.
const LIMIT_MESSAGE: &str = "Maximum probes per subnet limit exceeded.";

/// One probe body sourced from the subnet `subnet`, with a destination of its
/// own so no two probes of a case are the same request twice.
fn probe_in(subnet: usize, destination_host: usize) -> String {
    format!(
        r#"{{"sourceArn":"{}","destination":"203.0.113.{destination_host}","protocol":"ICMP","packetSize":56}}"#,
        source_arn_of(subnet)
    )
}

/// A `CreateMonitor` body whose inline `probes` list holds `counts[i]` probes in
/// subnet `i`.
fn create_monitor_body(name: &str, counts: &[usize]) -> String {
    let mut probes: Vec<String> = Vec::new();
    for (subnet, count) in counts.iter().enumerate() {
        for _ in 0..*count {
            let host = probes.len() + 1;
            probes.push(probe_in(subnet, host));
        }
    }
    format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
        probes.join(",")
    )
}

/// How many probes a `GetMonitor` body reports whose `sourceArn` names subnet
/// `subnet`.
fn probes_in_subnet(body: &Value, subnet: usize) -> usize {
    let wanted = source_arn_of(subnet);
    probes_of(body)
        .iter()
        .filter(|probe| string_at(probe, "sourceArn") == wanted)
        .count()
}

/// Assert that `body` is the refusal of this bound.
fn assert_limit_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a subnet past the bound, got {status} {body:?}"
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

/// `CreateMonitor`'s inline probes are bounded per source subnet, and a refused
/// create stores no monitor at all.
///
/// The table spans the probe count and how it is spread over subnets: the bound
/// itself, one past it, far past it, the same count spread over separate
/// subnets, two subnets each at the bound, and two subnets of which only the
/// second is over it.
#[tokio::test]
async fn create_monitor_bounds_its_inline_probes_per_source_subnet() {
    // (case, probes per subnet, accepted)
    let cases: &[(&str, &[usize], bool)] = &[
        ("one probe in one subnet", &[1], true),
        ("one subnet at the bound", &[MAX_PROBES_PER_SUBNET], true),
        (
            "one subnet one probe past the bound",
            &[MAX_PROBES_PER_SUBNET + 1],
            false,
        ),
        (
            "one subnet far past the bound",
            &[MAX_PROBES_PER_SUBNET * 3],
            false,
        ),
        (
            "the same count spread one probe per subnet",
            &[1, 1, 1, 1, 1],
            true,
        ),
        (
            "two subnets each at the bound",
            &[MAX_PROBES_PER_SUBNET, MAX_PROBES_PER_SUBNET],
            true,
        ),
        (
            "two subnets, only the second past the bound",
            &[MAX_PROBES_PER_SUBNET, MAX_PROBES_PER_SUBNET + 1],
            false,
        ),
        (
            "three subnets below the bound",
            &[3, 3, 3],
            true,
        ),
    ];
    let router = router();
    for (index, (case, counts, accepted)) in cases.iter().enumerate() {
        let name = format!("inline-{index}");
        let (status, created) = create_monitor(&router, &create_monitor_body(&name, counts)).await;
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
        let expected_total: usize = counts.iter().sum();
        assert_eq!(
            probes_of(&got).len(),
            expected_total,
            "case {case:?}: expected {expected_total} stored probes, got {got:?}"
        );
        for (subnet, count) in counts.iter().enumerate() {
            assert_eq!(
                probes_in_subnet(&got, subnet),
                *count,
                "case {case:?}: expected {count} probes in subnet {subnet}, got {got:?}"
            );
        }
    }
}

/// The bound counts the probes a monitor ALREADY holds, whichever path put them
/// there: a `CreateProbe` is refused once its subnet is at the bound, and a
/// probe in another subnet of the same monitor is still accepted.
///
/// The table spans how many of the probes already there were created inline by
/// `CreateMonitor` and how many by `CreateProbe`, from all of them to none.
#[tokio::test]
async fn create_probe_counts_the_probes_the_monitor_already_holds() {
    // (case, probes created inline in the subnet under test)
    let cases: &[(&str, usize)] = &[
        ("every earlier probe created inline", MAX_PROBES_PER_SUBNET),
        ("some inline, the rest through CreateProbe", 2),
        ("one inline, the rest through CreateProbe", 1),
        ("every earlier probe through CreateProbe", 0),
    ];

    for (index, (case, inline)) in cases.iter().enumerate() {
        let router = router();
        let name = format!("mixed-{index}");
        let (status, created) = create_monitor(&router, &create_monitor_body(&name, &[*inline])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the setup CreateMonitor 200, got {status} {created:?}"
        );

        // Up to the bound, every add is accepted.
        for position in *inline..MAX_PROBES_PER_SUBNET {
            let body = format!(r#"{{"probe":{}}}"#, probe_in(0, position + 10));
            let (status, added) = create_probe(&router, &name, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: probe {} of the subnet must be accepted, got {status} {added:?}",
                position + 1
            );
        }

        // One past it is refused, and the probe is not minted.
        let body = format!(r#"{{"probe":{}}}"#, probe_in(0, 90));
        let (status, refused) = create_probe(&router, &name, &body).await;
        assert_limit_refusal(case, status, &refused);
        let (_, got) = get_monitor(&router, &name).await;
        assert_eq!(
            probes_in_subnet(&got, 0),
            MAX_PROBES_PER_SUBNET,
            "case {case:?}: the refused create must leave {MAX_PROBES_PER_SUBNET} probes in the subnet, got {got:?}"
        );

        // The bound is per subnet, so another subnet of the SAME monitor is
        // still free.
        let body = format!(r#"{{"probe":{}}}"#, probe_in(1, 91));
        let (status, elsewhere) = create_probe(&router, &name, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a probe in another subnet is unaffected, got {status} {elsewhere:?}"
        );
        let (_, got) = get_monitor(&router, &name).await;
        assert_eq!(
            probes_in_subnet(&got, 1),
            1,
            "case {case:?}: expected the other subnet to hold one probe, got {got:?}"
        );
    }
}

/// The bound is per (monitor, subnet) pair: one subnet at the bound in one
/// monitor leaves that same subnet free in every other monitor.
#[tokio::test]
async fn the_bound_is_counted_per_monitor() {
    let router = router();
    for name in ["tenant-one", "tenant-two", "tenant-three"] {
        let (status, created) =
            create_monitor(&router, &create_monitor_body(name, &[MAX_PROBES_PER_SUBNET])).await;
        assert_eq!(
            status, 200,
            "monitor {name:?}: the same subnet at the bound is free in a new monitor, \
             got {status} {created:?}"
        );
        let body = format!(r#"{{"probe":{}}}"#, probe_in(0, 99));
        let (status, refused) = create_probe(&router, name, &body).await;
        assert_limit_refusal(name, status, &refused);
    }
}

/// A probe that breaks a rule of its own is refused for THAT rule, even in a
/// request that also takes a subnet past the bound: the count is judged once
/// every probe the request describes is a probe at all.
#[tokio::test]
async fn a_malformed_probe_outranks_the_count() {
    let router = router();
    let mut probes: Vec<String> = (0..MAX_PROBES_PER_SUBNET + 1)
        .map(|index| probe_in(0, index + 1))
        .collect();
    probes.push(format!(
        r#"{{"sourceArn":"{}","destination":"example.internal","protocol":"ICMP","packetSize":56}}"#,
        source_arn_of(0)
    ));
    let body = format!(
        r#"{{"monitorName":"malformed-and-too-many","probes":[{}]}}"#,
        probes.join(",")
    );
    let (status, refused) = create_monitor(&router, &body).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a malformed probe, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused:?}"
    );
    let message = error_message(&refused);
    assert!(
        message.contains("destination"),
        "expected the refusal to name the destination member, got {message:?}"
    );
    let (status, missing) = get_monitor(&router, "malformed-and-too-many").await;
    assert_eq!(
        status, 404,
        "a refused create stores no monitor, got {status} {missing:?}"
    );
}

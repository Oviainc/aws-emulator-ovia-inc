// SPDX-License-Identifier: CC0-1.0

//! In what ORDER a request that adds probes is judged: each probe against the
//! rules it must satisfy on its own, and only then the request against how many
//! probes it asks for.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`create_monitor`/…) is the one `monitor_and_probe` already owns.
//!
//! THE RULE: a request is refused for the member it got WRONG. Every per-member
//! rule a probe has to satisfy is applied before either count ceiling a probe
//! occupies a slot in (the probes of a monitor, and the probes of one source
//! subnet of a monitor), so a request that both carries a malformed probe and
//! asks for too many probes is refused with the clause naming that probe's
//! offending member — never with a count sentence, which names no member and so
//! tells the caller nothing about what it has to change. A request whose probes
//! are all well-formed is still judged on the counts, so the ceilings keep
//! applying.
//!
//! The tables span the class the rule covers: which per-member rule the probe
//! breaks, which ceiling the request would otherwise be refused by, where the
//! malformed probe sits in the request (first, middle, last), and which
//! operation adds the probes (`CreateMonitor`'s inline `probes` list and
//! `CreateProbe`).

use serde_json::Value;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probes_of, router,
    source_arn_of, ACCOUNT, REGION,
};

/// The ceiling on the probes of one monitor
/// (`crate::state::MAX_PROBES_PER_MONITOR`).
const MAX_PROBES_PER_MONITOR: usize = 100;

/// The ceiling on the probes one monitor holds in ONE source subnet
/// (`crate::state::MAX_PROBES_PER_SUBNET`).
const MAX_PROBES_PER_SUBNET: usize = 4;

/// What the refusal of the per-monitor ceiling says
/// (`crate::state::PROBES_PER_MONITOR_MESSAGE`).
const PER_MONITOR_MESSAGE: &str = "Maximum probes per monitor limit exceeded.";

/// What the refusal of the per-subnet ceiling says
/// (`crate::state::PROBES_PER_SUBNET_MESSAGE`).
const PER_SUBNET_MESSAGE: &str = "Maximum probes per subnet limit exceeded.";

/// The requirement a `destination` that is not an IP literal breaks, from the
/// model doc for `ProbeInput$destination` ("The destination IP address. This
/// must be either `IPV4` or `IPV6`").
const DESTINATION_REQUIREMENT: &str = "Member must be an IPv4 or IPv6 address";

/// The requirement a `destinationPort` of `0` breaks, from the model doc for
/// `ProbeInput$destinationPort` ("must be a number between `1` and `65536`").
const PORT_FLOOR_REQUIREMENT: &str = "Member must have value greater than or equal to 1";

/// The whole sentence the cross-member TCP rule states
/// (`crate::state::TCP_PORT_REQUIRED_MESSAGE`).
const TCP_PORT_REQUIRED_MESSAGE: &str = "Destination port must be provided for TCP protocol.";

/// The subnet a malformed probe of [`spread_body`] is sourced from: one no
/// well-formed probe of that body uses, so the probe is alone in its subnet and
/// is never refused by the per-subnet count.
const LONE_SUBNET: usize = 999;

/// AWS's construction for ONE violated constraint, as
/// `authored_refusal_messages` pins it.
fn one_error(value: &str, member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' \
         failed to satisfy constraint: {requirement}"
    )
}

/// The dotted path of `member` within the probe sitting at `probe_path`.
fn member_of(probe_path: &str, member: &str) -> String {
    format!("{probe_path}.{member}")
}

/// An ARN of this account that names something OTHER than a VPC subnet, so a
/// probe sourced from it breaks the `ProbeInput$sourceArn` rule ("The ARN of the
/// subnet").
fn non_subnet_arn() -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc/vpc-0123456789abcdef0")
}

/// One probe body: the members `ProbeInput` and `CreateMonitorProbeInput` both
/// require, plus whatever `extra` adds (`,"destinationPort":0`).
fn probe_body(source_arn: &str, destination: &str, protocol: &str, extra: &str) -> String {
    format!(
        r#"{{"sourceArn":"{source_arn}","destination":"{destination}","protocol":"{protocol}","packetSize":56{extra}}}"#
    )
}

/// A destination of its own for the `position`th well-formed probe, so no two
/// probes of one request are the same probe twice.
fn destination_of(position: usize) -> String {
    format!("203.0.{}.{}", position / 250, position % 250 + 1)
}

/// The `position`th well-formed probe of a monitor, sourced from `subnet`.
fn well_formed_probe(subnet: usize, position: usize) -> String {
    probe_body(
        &source_arn_of(subnet),
        &destination_of(position),
        "ICMP",
        "",
    )
}

/// One way a probe can be malformed, with the refusal its own rule states.
struct Flaw {
    /// What the row is.
    case: &'static str,
    /// The probe body, sourced from the subnet ARN given — except where the
    /// flaw IS the `sourceArn` ([`Flaw::keeps_subnet`]).
    body: fn(&str) -> String,
    /// The refusal, given the dotted path of this probe within the request
    /// (`probe`, or `probes.<position>.member`).
    refusal: fn(&str) -> String,
    /// Whether the probe is still sourced from the subnet it was given, i.e.
    /// whether it can be used to saturate a subnet's own ceiling.
    keeps_subnet: bool,
}

/// Every per-member probe rule this service applies beyond the modelled
/// constraints the framework refuses before a handler runs
/// (`crate::state::validate_probe_spec`): the `destination` format, the TCP port
/// requirement, the documented port floor, and `sourceArn` naming a subnet.
fn flaws() -> Vec<Flaw> {
    vec![
        Flaw {
            case: "a destination that is not an IP literal",
            body: |source_arn| probe_body(source_arn, "not-an-ip", "ICMP", ""),
            refusal: |probe_path| {
                one_error(
                    "not-an-ip",
                    &member_of(probe_path, "destination"),
                    DESTINATION_REQUIREMENT,
                )
            },
            keeps_subnet: true,
        },
        Flaw {
            case: "a dotted quad with an octet out of range",
            body: |source_arn| probe_body(source_arn, "10.0.0.256", "ICMP", ""),
            refusal: |probe_path| {
                one_error(
                    "10.0.0.256",
                    &member_of(probe_path, "destination"),
                    DESTINATION_REQUIREMENT,
                )
            },
            keeps_subnet: true,
        },
        Flaw {
            case: "TCP with no destinationPort",
            body: |source_arn| probe_body(source_arn, "198.51.100.7", "TCP", ""),
            refusal: |_| TCP_PORT_REQUIRED_MESSAGE.to_string(),
            keeps_subnet: true,
        },
        Flaw {
            case: "a destinationPort below the documented floor",
            body: |source_arn| {
                probe_body(
                    source_arn,
                    "198.51.100.8",
                    "TCP",
                    r#","destinationPort":0"#,
                )
            },
            refusal: |probe_path| {
                one_error(
                    "0",
                    &member_of(probe_path, "destinationPort"),
                    PORT_FLOOR_REQUIREMENT,
                )
            },
            keeps_subnet: true,
        },
        Flaw {
            case: "a sourceArn that names no subnet",
            body: |_| probe_body(&non_subnet_arn(), "198.51.100.9", "ICMP", ""),
            refusal: |_| {
                "The specified subnet vpc-0123456789abcdef0 does not exist or could not be found."
                    .to_string()
            },
            keeps_subnet: false,
        },
    ]
}

/// A `CreateMonitor` body asking for `total` probes spread over subnets, with
/// the probe at 1-based `position` replaced by `flaw`'s body when one is given.
///
/// The malformed probe is sourced from [`LONE_SUBNET`], a subnet no well-formed
/// probe of these bodies uses, so the only rules that can refuse it are its own
/// and the per-MONITOR count.
fn spread_body(name: &str, total: usize, flawed_position: Option<(usize, &Flaw)>) -> String {
    let probes: Vec<String> = (0..total)
        .map(|index| match flawed_position {
            Some((position, flaw)) if index + 1 == position => {
                (flaw.body)(&source_arn_of(LONE_SUBNET))
            }
            _ => well_formed_probe(index / MAX_PROBES_PER_SUBNET, index),
        })
        .collect();
    format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
        probes.join(",")
    )
}

/// The same, with every probe sourced from ONE subnet, so the count that can
/// refuse the request is the per-SUBNET one.
fn one_subnet_body(
    name: &str,
    subnet: usize,
    total: usize,
    flawed_position: Option<(usize, &Flaw)>,
) -> String {
    let probes: Vec<String> = (0..total)
        .map(|index| match flawed_position {
            Some((position, flaw)) if index + 1 == position => {
                (flaw.body)(&source_arn_of(subnet))
            }
            _ => well_formed_probe(subnet, index),
        })
        .collect();
    format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
        probes.join(",")
    )
}

/// Assert that `body` is the 400 `ValidationException` stating `expected`, and
/// that it is not a count sentence.
fn assert_refusal(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected message {expected:?}, got {message:?}"
    );
}

/// Assert that `body` says nothing about a count: a malformed probe is refused
/// for what it says, not for the company it keeps.
fn assert_not_a_count(case: &str, body: &Value) {
    let message = error_message(body);
    for sentence in [PER_MONITOR_MESSAGE, PER_SUBNET_MESSAGE] {
        assert_ne!(
            message, sentence,
            "case {case:?}: a malformed probe must not be refused by a count, got {message:?}"
        );
    }
}

/// `CreateMonitor`: a malformed probe in a list that is ALSO past the
/// per-monitor ceiling is refused for the member it got wrong.
///
/// The table spans every per-member probe rule and where the malformed probe
/// sits in the request: first (the reported case), middle and last of the
/// request's own `probes` list.
#[tokio::test]
async fn create_monitor_judges_each_probe_before_the_per_monitor_count() {
    let total = MAX_PROBES_PER_MONITOR + 1;
    let positions = [1usize, total / 2, total];

    let router = router();
    for (index, flaw) in flaws().iter().enumerate() {
        for position in positions {
            let case = format!("{} at position {position} of {total}", flaw.case);
            let name = format!("over-monitor-{index}-{position}");
            let (status, refused) =
                create_monitor(&router, &spread_body(&name, total, Some((position, flaw)))).await;
            let probe_path = format!("probes.{position}.member");
            assert_refusal(&case, status, &refused, &(flaw.refusal)(&probe_path));
            assert_not_a_count(&case, &refused);
            // A refused create stores nothing.
            let (status, missing) = get_monitor(&router, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create stores no monitor, got {status} {missing:?}"
            );
        }
    }

    // The ceiling still applies: the SAME over-long list, every probe
    // well-formed, is refused by the count — so the rule is about precedence
    // and not about dropping the bound.
    let (status, refused) = create_monitor(&router, &spread_body("over-monitor", total, None)).await;
    assert_refusal("every probe well-formed", status, &refused, PER_MONITOR_MESSAGE);

    // And a list within the ceiling whose probes are all well-formed is stored,
    // read back through `GetMonitor`.
    let (status, created) = create_monitor(
        &router,
        &spread_body("at-the-bound", MAX_PROBES_PER_MONITOR, None),
    )
    .await;
    assert_eq!(
        status, 200,
        "a well-formed list at the bound must be accepted, got {status} {created:?}"
    );
    let (status, got) = get_monitor(&router, "at-the-bound").await;
    assert_eq!(
        status, 200,
        "expected GetMonitor 200 for the stored monitor, got {status} {got:?}"
    );
    assert_eq!(
        probes_of(&got).len(),
        MAX_PROBES_PER_MONITOR,
        "expected {MAX_PROBES_PER_MONITOR} stored probes, got {}",
        probes_of(&got).len()
    );
}

/// `CreateMonitor`: the sibling ceiling resolves the same way — a malformed
/// probe in a list that is also past the per-SUBNET ceiling is refused for the
/// member it got wrong.
///
/// The rows are the per-member rules that leave the probe in the saturated
/// subnet (the `sourceArn` rule moves it out of that subnet by definition), at
/// the first and the last position of the list.
#[tokio::test]
async fn create_monitor_judges_each_probe_before_the_per_subnet_count() {
    let total = MAX_PROBES_PER_SUBNET + 1;

    let router = router();
    for (index, flaw) in flaws().iter().filter(|flaw| flaw.keeps_subnet).enumerate() {
        for position in [1usize, total] {
            let case = format!("{} at position {position} of one subnet", flaw.case);
            let name = format!("over-subnet-{index}-{position}");
            let (status, refused) = create_monitor(
                &router,
                &one_subnet_body(&name, 11, total, Some((position, flaw))),
            )
            .await;
            let probe_path = format!("probes.{position}.member");
            assert_refusal(&case, status, &refused, &(flaw.refusal)(&probe_path));
            assert_not_a_count(&case, &refused);
            let (status, missing) = get_monitor(&router, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create stores no monitor, got {status} {missing:?}"
            );
        }
    }

    // The per-subnet ceiling still applies to a list of well-formed probes.
    let (status, refused) =
        create_monitor(&router, &one_subnet_body("over-subnet", 11, total, None)).await;
    assert_refusal(
        "every probe well-formed in one subnet",
        status,
        &refused,
        PER_SUBNET_MESSAGE,
    );
}

/// `CreateProbe`: the one rule, on the path that adds probes one at a time. A
/// malformed probe sent to a monitor already AT a ceiling is refused for the
/// member it got wrong, and a well-formed one is refused by the ceiling.
///
/// The table spans every per-member probe rule and both ceilings: a monitor
/// holding its full complement of probes, and a monitor whose target subnet is
/// full while the monitor itself is nearly empty.
#[tokio::test]
async fn create_probe_judges_the_probe_before_the_count_ceilings() {
    // (case, the monitor's name, its setup body, the subnet a probe is sent to,
    // the ceiling sentence a well-formed probe is refused by)
    let scenarios: [(&str, &str, String, usize, &str); 2] = [
        (
            "a monitor at the per-monitor ceiling",
            "full-monitor",
            spread_body("full-monitor", MAX_PROBES_PER_MONITOR, None),
            700,
            PER_MONITOR_MESSAGE,
        ),
        (
            "a monitor whose target subnet is full",
            "full-subnet",
            one_subnet_body("full-subnet", 12, MAX_PROBES_PER_SUBNET, None),
            12,
            PER_SUBNET_MESSAGE,
        ),
    ];

    for (scenario, name, setup, subnet, ceiling) in scenarios {
        let router = router();
        let (status, created) = create_monitor(&router, &setup).await;
        assert_eq!(
            status, 200,
            "scenario {scenario:?}: expected the setup CreateMonitor 200, got {status} {created:?}"
        );
        let held = probes_of(&get_monitor(&router, name).await.1).len();

        for flaw in flaws() {
            let case = format!("{scenario}: {}", flaw.case);
            let body = format!(r#"{{"probe":{}}}"#, (flaw.body)(&source_arn_of(subnet)));
            let (status, refused) = create_probe(&router, name, &body).await;
            assert_refusal(&case, status, &refused, &(flaw.refusal)("probe"));
            assert_not_a_count(&case, &refused);
        }

        // A well-formed probe sent to the same place IS refused by the ceiling,
        // and every refusal left the monitor holding exactly what it held.
        let body = format!(
            r#"{{"probe":{}}}"#,
            well_formed_probe(subnet, MAX_PROBES_PER_MONITOR + 7)
        );
        let (status, refused) = create_probe(&router, name, &body).await;
        assert_refusal(scenario, status, &refused, ceiling);
        let (status, got) = get_monitor(&router, name).await;
        assert_eq!(
            status, 200,
            "scenario {scenario:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            probes_of(&got).len(),
            held,
            "scenario {scenario:?}: a refused create must store nothing, expected {held} \
             probes, got {}",
            probes_of(&got).len()
        );
    }
}

/// A probe that breaks no rule and no ceiling is still accepted on the path
/// these tests refuse on, and is read back by `GetMonitor`: the ordering rule
/// refuses nothing on its own.
#[tokio::test]
async fn a_well_formed_probe_below_every_ceiling_is_stored() {
    let router = router();
    let (status, created) = create_monitor(
        &router,
        &one_subnet_body("accepting", 13, MAX_PROBES_PER_SUBNET - 1, None),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the setup CreateMonitor 200, got {status} {created:?}"
    );

    let body = format!(r#"{{"probe":{}}}"#, well_formed_probe(13, 300));
    let (status, probe) = create_probe(&router, "accepting", &body).await;
    assert_eq!(
        status, 200,
        "expected CreateProbe 200 below every ceiling, got {status} {probe:?}"
    );
    let (status, got) = get_monitor(&router, "accepting").await;
    assert_eq!(
        status, 200,
        "expected GetMonitor 200, got {status} {got:?}"
    );
    assert_eq!(
        probes_of(&got).len(),
        MAX_PROBES_PER_SUBNET,
        "expected the added probe to be listed, got {got:?}"
    );
}

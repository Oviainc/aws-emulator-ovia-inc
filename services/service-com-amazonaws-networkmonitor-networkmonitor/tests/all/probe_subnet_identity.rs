// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for WHICH source subnet a probe counts against.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test: a
//! probe's source subnet is the subnet its `sourceArn` NAMES, so a bound stated
//! per (monitor, source subnet) pair — the bound on how many probes of one
//! monitor may name one source subnet — counts every `sourceArn` naming one
//! subnet against that subnet's single count, however the ARNs are otherwise
//! spelled, and keeps separate counts for ARNs naming different subnets however
//! alike they are spelled.
//!
//! The spellings span the ARN segments the subnet's own identity does not
//! depend on: the partition and the region of the `arn:<partition>:ec2:<region>:
//! <account>:subnet/<subnetId>` form the service requires of a `sourceArn`
//! (the model documents `ProbeInput$sourceArn` as "The ARN of the subnet" —
//! `smithy_query(query="doc",shape="ProbeInput",member="sourceArn")`), and a
//! region segment left empty. The tables span how the probes of one subnet are
//! divided between spellings, which path created them (`CreateMonitor`'s inline
//! list, `CreateProbe`, a mix), and how many the request asks for (below the
//! bound, at it, past it).

use serde_json::Value;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probes_of, router,
    string_at, ACCOUNT, REGION,
};

/// The default value of the AWS quota "Number of probes per subnet for each
/// monitor", which is the bound the service applies
/// (`crate::state::MAX_PROBES_PER_SUBNET`).
const MAX_PROBES_PER_SUBNET: usize = 4;

/// What the refusal of that bound says.
const LIMIT_MESSAGE: &str = "Maximum probes per subnet limit exceeded.";

/// Ways of spelling the ARN of one subnet that leave the subnet it names
/// unchanged: `(case, partition, region)`. The first row is the plain spelling
/// every other suite uses; the rest differ from it only in segments that are
/// not part of a subnet's identity.
const SPELLINGS: &[(&str, &str, &str)] = &[
    ("the plain spelling", "aws", REGION),
    ("another region", "aws", "eu-west-1"),
    ("another partition", "aws-us-gov", REGION),
    ("another partition and region", "aws-cn", "cn-north-1"),
    ("an empty region segment", "aws", ""),
];

/// The subnet id of subnet `subnet`, in the 17-character form EC2 ids take.
fn subnet_id(subnet: usize) -> String {
    format!("subnet-0123456789abcd{subnet:03}")
}

/// The ARN of subnet `subnet`, spelled as [`SPELLINGS`] row `spelling` spells
/// it.
fn source_arn_spelled(spelling: usize, subnet: usize) -> String {
    let (_, partition, region) = SPELLINGS[spelling];
    format!(
        "arn:{partition}:ec2:{region}:{ACCOUNT}:subnet/{}",
        subnet_id(subnet)
    )
}

/// One probe body sourced from subnet `subnet` through spelling `spelling`,
/// with a destination of its own so no two probes of a case are the same
/// request twice.
fn probe_in(spelling: usize, subnet: usize, destination_host: usize) -> String {
    format!(
        r#"{{"sourceArn":"{}","destination":"203.0.113.{destination_host}","protocol":"ICMP","packetSize":56}}"#,
        source_arn_spelled(spelling, subnet)
    )
}

/// How many probes a `GetMonitor` body reports whose `sourceArn` NAMES subnet
/// `subnet`, whichever way that ARN is spelled.
fn probes_naming_subnet(body: &Value, subnet: usize) -> usize {
    let wanted = format!("subnet/{}", subnet_id(subnet));
    probes_of(body)
        .iter()
        .filter(|probe| string_at(probe, "sourceArn").ends_with(&wanted))
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

/// What a case asks a request to carry: how many probes it wants in one subnet
/// through one spelling of that subnet's ARN, as `(spelling, subnet, count)`
/// where `spelling` indexes [`SPELLINGS`].
type ProbeCounts = [(usize, usize, usize)];

/// A `CreateMonitor` body whose inline `probes` list holds the probes
/// `probes` describes.
fn create_monitor_body(name: &str, probes: &ProbeCounts) -> String {
    let mut bodies: Vec<String> = Vec::new();
    for (spelling, subnet, count) in probes {
        for _ in 0..*count {
            let host = bodies.len() + 1;
            bodies.push(probe_in(*spelling, *subnet, host));
        }
    }
    format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
        bodies.join(",")
    )
}

/// `CreateProbe` counts a probe against the subnet its `sourceArn` names, so
/// the probes a monitor already holds in one subnet bound a new probe named
/// through ANY spelling of that subnet's ARN.
///
/// The table spans which spelling the probes already there used and which the
/// new probe uses, and how the earlier probes were divided between the two.
#[tokio::test]
async fn create_probe_counts_every_spelling_of_one_subnet_together() {
    // (held, adding): the spelling the probes already there use, and the
    // spelling the probe being added uses.
    let pairs: &[(usize, usize)] = &[(0, 1), (1, 0), (0, 2), (2, 3), (0, 4), (4, 1)];

    for (index, (held, adding)) in pairs.iter().enumerate() {
        // How the probes already there are divided between the two spellings:
        // all through the held spelling, then progressively more through the
        // one the final probe uses.
        for split in 0..=MAX_PROBES_PER_SUBNET {
            let case = format!(
                "{} filled, {} added, {split} of the earlier probes already spelled that way",
                SPELLINGS[*held].0, SPELLINGS[*adding].0
            );
            let router = router();
            let name = format!("spelled-{index}-{split}");
            // The first probes land inline, the rest through CreateProbe, so
            // the count a create reads covers both paths that store a probe.
            let inline = split.min(1);
            let (status, created) = create_monitor(
                &router,
                &create_monitor_body(&name, &[(*adding, 0, inline)]),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the setup CreateMonitor 200, got {status} {created:?}"
            );
            for position in inline..MAX_PROBES_PER_SUBNET {
                // Each earlier probe is spelled the way the final probe is
                // until `split` of them are, then the way the subnet was
                // filled.
                let spelling = if position < split { *adding } else { *held };
                let body = format!(r#"{{"probe":{}}}"#, probe_in(spelling, 0, position + 10));
                let (status, added) = create_probe(&router, &name, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: probe {} of the subnet must be accepted, got {status} {added:?}",
                    position + 1
                );
            }

            // The subnet is at the bound, so the next probe is refused however
            // its ARN spells that subnet — and nothing is minted.
            let body = format!(r#"{{"probe":{}}}"#, probe_in(*adding, 0, 90));
            let (status, refused) = create_probe(&router, &name, &body).await;
            assert_limit_refusal(&case, status, &refused);
            let (status, got) = get_monitor(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
            );
            assert_eq!(
                probes_naming_subnet(&got, 0),
                MAX_PROBES_PER_SUBNET,
                "case {case:?}: the refused create must leave {MAX_PROBES_PER_SUBNET} probes \
                 naming the subnet, got {got:?}"
            );

            // The bound is per subnet, so a DIFFERENT subnet is still free,
            // including through the spelling the full subnet was named by.
            let body = format!(r#"{{"probe":{}}}"#, probe_in(*held, 1, 91));
            let (status, elsewhere) = create_probe(&router, &name, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a probe in another subnet is unaffected, got {status} {elsewhere:?}"
            );
        }
    }
}

/// `CreateMonitor`'s inline list counts its probes against the subnets their
/// `sourceArn`s name, so one subnet reached through several spellings in ONE
/// call is bounded as one subnet.
///
/// The table spans how the probes are divided between spellings, how many
/// subnets the list names, and whether the total for any one subnet is below
/// the bound, at it or past it.
#[tokio::test]
async fn create_monitor_counts_every_spelling_of_one_subnet_together() {
    // (case, probes as (spelling, subnet, count), accepted)
    let cases: &[(&str, &ProbeCounts, bool)] = &[
        (
            "one subnet at the bound through two spellings",
            &[(0, 0, 2), (1, 0, 2)],
            true,
        ),
        (
            "one subnet one past the bound through two spellings",
            &[(0, 0, 4), (1, 0, 1)],
            false,
        ),
        (
            "one subnet one past the bound, the spellings interleaved",
            &[(1, 0, 1), (0, 0, 1), (2, 0, 1), (3, 0, 1), (4, 0, 1)],
            false,
        ),
        (
            "one subnet far past the bound through two spellings",
            &[(2, 0, 6), (3, 0, 6)],
            false,
        ),
        (
            "one subnet below the bound through three spellings",
            &[(0, 0, 1), (2, 0, 1), (4, 0, 1)],
            true,
        ),
        (
            "two subnets each at the bound through two spellings",
            &[(0, 0, 2), (4, 0, 2), (1, 1, 2), (3, 1, 2)],
            true,
        ),
        (
            "two subnets, only the second past the bound through two spellings",
            &[(0, 0, 2), (1, 0, 2), (2, 1, 3), (3, 1, 2)],
            false,
        ),
        (
            "every spelling naming a subnet of its own",
            &[(0, 0, 4), (1, 1, 4), (2, 2, 4), (3, 3, 4), (4, 4, 4)],
            true,
        ),
    ];

    let router = router();
    for (index, (case, probes, accepted)) in cases.iter().enumerate() {
        let name = format!("inline-spelled-{index}");
        let (status, created) = create_monitor(&router, &create_monitor_body(&name, probes)).await;
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
        let expected_total: usize = probes.iter().map(|(_, _, count)| count).sum();
        assert_eq!(
            probes_of(&got).len(),
            expected_total,
            "case {case:?}: expected {expected_total} stored probes, got {got:?}"
        );
        // Every stored probe names the subnet it was asked for, and each
        // subnet holds no more than the bound.
        for (_, subnet, _) in probes.iter() {
            let held = probes_naming_subnet(&got, *subnet);
            let asked: usize = probes
                .iter()
                .filter(|(_, named, _)| named == subnet)
                .map(|(_, _, count)| count)
                .sum();
            assert_eq!(
                held, asked,
                "case {case:?}: expected {asked} probes naming subnet {subnet}, got {got:?}"
            );
            assert!(
                held <= MAX_PROBES_PER_SUBNET,
                "case {case:?}: subnet {subnet} must hold at most {MAX_PROBES_PER_SUBNET} \
                 probes, got {held}"
            );
        }
    }
}

/// Re-spelling the ARN of a subnet that is already at the bound buys no
/// capacity: once a monitor holds the bound in one subnet, a probe named
/// through EVERY other spelling of that subnet is refused in turn, and the
/// monitor still holds exactly the bound.
#[tokio::test]
async fn no_spelling_of_a_full_subnet_adds_capacity() {
    let router = router();
    let name = "every-spelling";
    let (status, created) =
        create_monitor(&router, &create_monitor_body(name, &[(0, 0, MAX_PROBES_PER_SUBNET)])).await;
    assert_eq!(
        status, 200,
        "expected the setup CreateMonitor 200, got {status} {created:?}"
    );

    for (spelling, (case, _, _)) in SPELLINGS.iter().enumerate() {
        let body = format!(r#"{{"probe":{}}}"#, probe_in(spelling, 0, spelling + 20));
        let (status, refused) = create_probe(&router, name, &body).await;
        assert_limit_refusal(case, status, &refused);
    }
    let (_, got) = get_monitor(&router, name).await;
    assert_eq!(
        probes_naming_subnet(&got, 0),
        MAX_PROBES_PER_SUBNET,
        "the refused creates must leave {MAX_PROBES_PER_SUBNET} probes naming the subnet, \
         got {got:?}"
    );
    assert_eq!(
        probes_of(&got).len(),
        MAX_PROBES_PER_SUBNET,
        "the refused creates must mint no probe, got {got:?}"
    );
}

/// Subnets that differ only in their id are different subnets, so each keeps a
/// count of its own even when every probe of every one of them is named
/// through the same spelling.
#[tokio::test]
async fn subnets_that_differ_in_id_keep_separate_counts() {
    for (spelling, (case, _, _)) in SPELLINGS.iter().enumerate() {
        let router = router();
        let name = format!("per-subnet-{spelling}");
        let (status, created) = create_monitor(
            &router,
            &create_monitor_body(&name, &[(spelling, 0, MAX_PROBES_PER_SUBNET)]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the setup CreateMonitor 200, got {status} {created:?}"
        );
        for subnet in 1..4 {
            for position in 0..MAX_PROBES_PER_SUBNET {
                let body = format!(
                    r#"{{"probe":{}}}"#,
                    probe_in(spelling, subnet, subnet * 10 + position)
                );
                let (status, added) = create_probe(&router, &name, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: probe {} of subnet {subnet} must be accepted, \
                     got {status} {added:?}",
                    position + 1
                );
            }
            let body = format!(r#"{{"probe":{}}}"#, probe_in(spelling, subnet, 99));
            let (status, refused) = create_probe(&router, &name, &body).await;
            assert_limit_refusal(case, status, &refused);
        }
        let (_, got) = get_monitor(&router, &name).await;
        for subnet in 0..4 {
            assert_eq!(
                probes_naming_subnet(&got, subnet),
                MAX_PROBES_PER_SUBNET,
                "case {case:?}: expected subnet {subnet} to hold {MAX_PROBES_PER_SUBNET} \
                 probes, got {got:?}"
            );
        }
    }
}

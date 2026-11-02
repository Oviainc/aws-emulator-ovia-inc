// SPDX-License-Identifier: CC0-1.0

//! The refusals Classic Elastic Load Balancing states itself: the error CODE a rule its
//! model documents but declares no error shape for answers under, and the MESSAGE each
//! conflict refusal states its own rule with.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * A request whose VALUES this service cannot accept — a rule stated in the
//!   documentation of a member rather than as a modelled error — is refused HTTP 400 under
//!   the error code `ValidationError`, which is the code this awsQuery service's clients
//!   branch on and the one the framework already answers a value outside a MODELLED bound
//!   with. One logical failure, one code.
//! * `CreateLoadBalancerListeners` refuses a listener that would change a configured
//!   `LoadBalancerPort` with `DuplicateListener`, whose message names the load balancer and
//!   that port and states which members the request may differ in — the three the modelled
//!   error's own documentation names.
//! * `CreateLoadBalancer` refuses a name the region already holds with
//!   `DuplicateLoadBalancerName`, whose message names the load balancer as the request
//!   spelled it and states the rule that produced the refusal: the name is held by a load
//!   balancer configured with different parameters.
//! * `DetachLoadBalancerFromSubnets` refuses a detach that would leave a load balancer
//!   attached to no subnet with `InvalidConfigurationRequest`, whose message names the load
//!   balancer from the request and states THAT rule — one of the many rules that one error
//!   code answers, which is why the message is what tells them apart.
//!
//! Every refusal is asserted on what a LATER read reports too, so a refusal that changed
//! the record would fail here.

use super::wire::{
    call, emulator, error_message, find, kid_text, member_texts, param, parse, Element, Wire,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The error code this service answers a request whose values it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The sentence a health check whose timeout is not below its interval is refused with:
/// the RELATION the two members have to each other, which is the rule the caller has to
/// satisfy whichever pair of values broke it.
const TIMEOUT_RELATION_REFUSAL: &str = "HealthCheck timeout must be less than interval.";

/// One row of a refusal table that spans several operations: the case, the action, and the
/// form fields the request carries beside it.
type OperationRow = (&'static str, &'static str, Vec<(String, String)>);

/// One row of the duplicate-listener table: the case, the listener the request declares,
/// and the `LoadBalancerPort` it conflicts on — `None` where it conflicts with nothing.
type ListenerConflictRow = (&'static str, Vec<(String, String)>, Option<i32>);

/// One row of the duplicate-name table: the case, the load balancer name, and the form
/// fields the SECOND create carries.
type DuplicateNameRow = (&'static str, &'static str, Vec<(String, String)>);

/// One row of the zero-subnet table: the case, the load balancer name, the subnets it is
/// created in, and the subnets the detach names.
type ZeroSubnetRow = (
    &'static str,
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
);

/// Create `name` with one HTTP 80 -> `instance_port` listener in one Availability Zone.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    extra: &[(String, String)],
) -> Wire {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("AvailabilityZones.member.1", &format!("{REGION}a")),
        param("Listeners.member.1.Protocol", "HTTP"),
        param("Listeners.member.1.LoadBalancerPort", "80"),
        param("Listeners.member.1.InstancePort", "80"),
    ];
    params.extend_from_slice(extra);
    call(router, REGION, "CreateLoadBalancer", &params).await
}

/// One health check, as `ConfigureHealthCheck` carries it.
#[derive(Debug, Clone, Copy)]
struct HealthCheck {
    target: &'static str,
    interval: i32,
    timeout: i32,
}

impl HealthCheck {
    fn params(&self, name: &str) -> Vec<(String, String)> {
        vec![
            param("LoadBalancerName", name),
            param("HealthCheck.Target", self.target),
            param("HealthCheck.Interval", &self.interval.to_string()),
            param("HealthCheck.Timeout", &self.timeout.to_string()),
            param("HealthCheck.UnhealthyThreshold", "2"),
            param("HealthCheck.HealthyThreshold", "2"),
        ]
    }
}

/// `HealthCheck$Timeout` is documented "This value must be less than the Interval value",
/// which the model states in prose and declares no error for — so the service's own
/// refusal is what answers it, under the code its clients branch on, and a check that
/// satisfies the rule is accepted and stored.
///
/// The rows walk the boundary of the relation at several points of the two members' own
/// modelled ranges (`Interval` 5..300, `Timeout` 2..60): one below, equal, and one above,
/// at the bottom of the ranges, in the middle and at the top.
#[tokio::test]
async fn a_health_check_timeout_is_accepted_only_below_the_interval() {
    let router = emulator();
    let created = create(&router, "hc-relation-elb", &[]).await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {}",
        created.body
    );

    // (case, the check, whether it is accepted)
    let rows: [(&str, HealthCheck, bool); 9] = [
        (
            "the smallest interval, with the smallest timeout below it",
            HealthCheck {
                target: "HTTP:80/ok",
                interval: 5,
                timeout: 4,
            },
            true,
        ),
        (
            "the smallest interval, with a timeout equal to it",
            HealthCheck {
                target: "HTTP:80/ok",
                interval: 5,
                timeout: 5,
            },
            false,
        ),
        (
            "a timeout one below the interval",
            HealthCheck {
                target: "HTTP:80/ok",
                interval: 10,
                timeout: 9,
            },
            true,
        ),
        (
            "a timeout equal to the interval",
            HealthCheck {
                target: "HTTP:80/ok",
                interval: 10,
                timeout: 10,
            },
            false,
        ),
        (
            "a timeout one above the interval",
            HealthCheck {
                target: "HTTP:80/ok",
                interval: 10,
                timeout: 11,
            },
            false,
        ),
        (
            "the largest timeout, with an interval one above it",
            HealthCheck {
                target: "TCP:80",
                interval: 61,
                timeout: 60,
            },
            true,
        ),
        (
            "the largest timeout, with an interval equal to it",
            HealthCheck {
                target: "TCP:80",
                interval: 60,
                timeout: 60,
            },
            false,
        ),
        (
            "the largest interval with the largest timeout",
            HealthCheck {
                target: "HTTPS:443/health",
                interval: 300,
                timeout: 60,
            },
            true,
        ),
        (
            "the smallest timeout with an interval equal to it is still not below it",
            HealthCheck {
                target: "SSL:443",
                interval: 5,
                timeout: 5,
            },
            false,
        ),
    ];

    let mut last_accepted: Option<HealthCheck> = None;
    for (case, check, accepted) in rows {
        let answered = call(
            &router,
            REGION,
            "ConfigureHealthCheck",
            &check.params("hc-relation-elb"),
        )
        .await;
        if accepted {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: a timeout below the interval must be accepted, got {}",
                answered.body
            );
            last_accepted = Some(check);
        } else {
            assert_eq!(
                (answered.status, answered.error_code().as_str()),
                (400, VALIDATION_ERROR),
                "case {case:?}: expected 400 {VALIDATION_ERROR}, got {} with {}",
                answered.status,
                answered.body
            );
            let stated = error_message(&answered);
            assert_eq!(
                stated, TIMEOUT_RELATION_REFUSAL,
                "case {case:?}: expected the message {TIMEOUT_RELATION_REFUSAL:?}, got \
                 {stated:?}"
            );
        }

        // A refused check leaves the stored one as the last accepted call wrote it.
        let described = call(
            &router,
            REGION,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", "hc-relation-elb")],
        )
        .await;
        let doc = parse(&described.body);
        let check_element = find(described.result(&doc), "HealthCheck")
            .unwrap_or_else(|| panic!("case {case:?}: no HealthCheck in {}", described.body));
        let expected = last_accepted.expect("a check is accepted before any is refused");
        assert_eq!(
            (
                kid_text(check_element, "Target"),
                kid_text(check_element, "Interval"),
                kid_text(check_element, "Timeout"),
            ),
            (
                Some(expected.target.to_string()),
                Some(expected.interval.to_string()),
                Some(expected.timeout.to_string()),
            ),
            "case {case:?}: the stored check must be the last accepted one, got {}",
            described.body
        );
    }
}

/// The SAME code answers every other rule this model states in prose rather than as an
/// error shape, across the operations that state one — so a client branching on the code
/// sees one code for one logical failure wherever it happens.
#[tokio::test]
async fn every_documented_rule_the_model_declares_no_error_for_answers_one_code() {
    let router = emulator();
    let created = create(&router, "rule-code-elb", &[]).await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {}",
        created.body
    );

    // (case, action, the form fields beyond the action)
    let mut rows: Vec<OperationRow> = vec![
        (
            // `HealthCheck$Target`: "TCP is the default, specified as a TCP: port pair".
            "a health-check target naming no protocol",
            "ConfigureHealthCheck",
            HealthCheck {
                target: "80/ping",
                interval: 10,
                timeout: 5,
            }
            .params("rule-code-elb"),
        ),
        (
            // "For HTTP/HTTPS, you must include a ping path in the string."
            "an HTTP health-check target with no ping path",
            "ConfigureHealthCheck",
            HealthCheck {
                target: "HTTP:80",
                interval: 10,
                timeout: 5,
            }
            .params("rule-code-elb"),
        ),
        (
            // "The range of valid ports is one (1) through 65535."
            "a health-check target port past the documented maximum",
            "ConfigureHealthCheck",
            HealthCheck {
                target: "TCP:65536",
                interval: 10,
                timeout: 5,
            }
            .params("rule-code-elb"),
        ),
        (
            // `AvailabilityZones`: "You must specify at least one Availability Zone",
            // and `Subnets` is the other way to place one — a request naming neither
            // places the load balancer nowhere.
            "a create that places the load balancer nowhere",
            "CreateLoadBalancer",
            vec![
                param("LoadBalancerName", "nowhere-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
            ],
        ),
        (
            // "Policy names must consist of alphanumeric characters and dashes (-)."
            "a policy name outside the documented character set",
            "CreateLBCookieStickinessPolicy",
            vec![
                param("LoadBalancerName", "rule-code-elb"),
                param("PolicyName", "bad_policy_name"),
            ],
        ),
    ];
    // A placement list past the bound this emulator holds, built here because it is longer
    // than a literal row.
    let mut too_many_subnets = vec![
        param("LoadBalancerName", "placement-bound-elb"),
        param("Listeners.member.1.Protocol", "HTTP"),
        param("Listeners.member.1.LoadBalancerPort", "80"),
        param("Listeners.member.1.InstancePort", "80"),
    ];
    for slot in 1..=51 {
        too_many_subnets.push(param(
            &format!("Subnets.member.{slot}"),
            &format!("subnet-{slot:017}"),
        ));
    }
    rows.push((
        "a placement list past the bound the store holds",
        "CreateLoadBalancer",
        too_many_subnets,
    ));

    for (case, action, params) in rows {
        let answered = call(&router, REGION, action, &params).await;
        assert_eq!(
            (answered.status, answered.error_code().as_str()),
            (400, VALIDATION_ERROR),
            "case {case:?}: expected 400 {VALIDATION_ERROR} from {action}, got {} with {}",
            answered.status,
            answered.body
        );
        assert!(
            !error_message(&answered).is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {}",
            answered.body
        );
    }
}

/// `DuplicateListenerException` is documented "A listener already exists for the specified
/// load balancer name and port, but with a different instance port, protocol, or SSL
/// certificate", so the refusal names the load balancer and the conflicting
/// `LoadBalancerPort` and states which members the request may differ in — not the stored
/// listener's own settings, which are not what the caller has to fix.
#[tokio::test]
async fn the_duplicate_listener_refusal_states_which_members_may_differ() {
    let router = emulator();
    let name = "dup-listener-elb";
    let created = create(
        &router,
        name,
        &[
            // A second listener, so the rows can conflict on either port and on a
            // certificate as well as on the back end.
            param("Listeners.member.2.Protocol", "HTTPS"),
            param("Listeners.member.2.LoadBalancerPort", "443"),
            param("Listeners.member.2.InstanceProtocol", "HTTPS"),
            param("Listeners.member.2.InstancePort", "443"),
            param(
                "Listeners.member.2.SSLCertificateId",
                "arn:aws:iam::000000000000:server-certificate/first",
            ),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {}",
        created.body
    );

    // The rows differ along WHICH of the three members the documented rule names differs,
    // and on which of the two configured ports — plus a repeat that differs in none of
    // them, which is a no-op rather than a conflict.
    //
    // (case, the listener the request declares, the port it conflicts on — none when it
    // is accepted)
    let rows: [ListenerConflictRow; 6] = [
        (
            "the same port with a different InstancePort",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "8080"),
            ],
            Some(80),
        ),
        (
            "the same port with a different Protocol",
            vec![
                param("Listeners.member.1.Protocol", "TCP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstanceProtocol", "TCP"),
                param("Listeners.member.1.InstancePort", "80"),
            ],
            Some(80),
        ),
        (
            "the same port with a different InstanceProtocol",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstanceProtocol", "HTTPS"),
                param("Listeners.member.1.InstancePort", "80"),
            ],
            Some(80),
        ),
        (
            "the other port with a different SSLCertificateId",
            vec![
                param("Listeners.member.1.Protocol", "HTTPS"),
                param("Listeners.member.1.LoadBalancerPort", "443"),
                param("Listeners.member.1.InstanceProtocol", "HTTPS"),
                param("Listeners.member.1.InstancePort", "443"),
                param(
                    "Listeners.member.1.SSLCertificateId",
                    "arn:aws:iam::000000000000:server-certificate/second",
                ),
            ],
            Some(443),
        ),
        (
            "a new port beside a conflicting one is refused with the conflicting one",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "8081"),
                param("Listeners.member.1.InstancePort", "8081"),
                param("Listeners.member.2.Protocol", "HTTP"),
                param("Listeners.member.2.LoadBalancerPort", "80"),
                param("Listeners.member.2.InstancePort", "9090"),
            ],
            Some(80),
        ),
        (
            "a repeat of a configured listener differs in none of them",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstanceProtocol", "HTTP"),
                param("Listeners.member.1.InstancePort", "80"),
            ],
            None,
        ),
    ];

    for (case, listeners, conflict) in rows {
        let mut params = vec![param("LoadBalancerName", name)];
        params.extend(listeners);
        let answered = call(&router, REGION, "CreateLoadBalancerListeners", &params).await;
        match conflict {
            Some(port) => {
                assert_eq!(
                    (answered.status, answered.error_code().as_str()),
                    (400, "DuplicateListener"),
                    "case {case:?}: expected 400 DuplicateListener, got {} with {}",
                    answered.status,
                    answered.body
                );
                let expected = format!(
                    "A listener already exists for {name} with LoadBalancerPort {port}, \
                     but with a different InstancePort, Protocol, or SSLCertificateId"
                );
                let stated = error_message(&answered);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected the message {expected:?}, got {stated:?}"
                );
            }
            None => assert_eq!(
                answered.status, 200,
                "case {case:?}: an identical repeat must be accepted, got {}",
                answered.body
            ),
        }

        // Whatever the answer, the stored listeners are still the two the create
        // configured: a refused request adds none of them, and an identical repeat adds
        // nothing either.
        let described = call(
            &router,
            REGION,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", name)],
        )
        .await;
        let doc = parse(&described.body);
        let reported = find(described.result(&doc), "ListenerDescriptions")
            .map(|listeners| {
                listeners
                    .children()
                    .filter_map(|member| find(member, "LoadBalancerPort"))
                    .filter_map(Element::text)
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        assert_eq!(
            reported,
            vec!["80".to_string(), "443".to_string()],
            "case {case:?}: the stored listeners must be the two the create configured, \
             got {}",
            described.body
        );
    }
}

/// `DuplicateAccessPointNameException` is documented "The specified load balancer name
/// already exists for this account", and a Classic load balancer name is unique within one
/// region — so a create naming one the region already holds is refused with a message that
/// names the load balancer AS THE REQUEST SPELLED IT and states the rule that produced the
/// refusal: the name is held by a load balancer configured with different parameters.
#[tokio::test]
async fn the_duplicate_name_refusal_states_the_differing_parameters_rule() {
    let router = emulator();

    // The rows differ along the name's own form (length, case, hyphens) and along WHAT the
    // second create asks for differently, because the rule is stated over the name
    // whatever the request carries beside it.
    let rows: [DuplicateNameRow; 5] = [
        (
            "a differing listener port",
            "dup-name-elb",
            vec![
                param("Listeners.member.1.Protocol", "TCP"),
                param("Listeners.member.1.LoadBalancerPort", "81"),
                param("Listeners.member.1.InstancePort", "8080"),
                param("AvailabilityZones.member.1", &format!("{REGION}a")),
            ],
        ),
        (
            "a differing Availability Zone",
            "a",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", &format!("{REGION}b")),
            ],
        ),
        (
            "a differing scheme",
            "Mixed-Case-ELB",
            vec![
                param("Scheme", "internal"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", &format!("{REGION}a")),
            ],
        ),
        (
            "a differing placement member",
            "a-long-hyphenated-elb-name-here",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("Subnets.member.1", "subnet-0123456789abcdef0"),
            ],
        ),
        (
            // Byte-identical to the create that holds the name: the name is taken either
            // way, and the refusal states the same rule.
            "the same parameters again",
            "identical-repeat-elb",
            vec![
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", &format!("{REGION}a")),
            ],
        ),
    ];

    for (case, name, second) in rows {
        let created = create(&router, name, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create must be accepted, got {}",
            created.body
        );
        let created_doc = parse(&created.body);
        let first_dns = kid_text(created.result(&created_doc), "DNSName");

        let mut params = vec![param("LoadBalancerName", name)];
        params.extend(second);
        let refused = call(&router, REGION, "CreateLoadBalancer", &params).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "DuplicateLoadBalancerName"),
            "case {case:?}: expected 400 DuplicateLoadBalancerName, got {} with {}",
            refused.status,
            refused.body
        );
        let expected = format!(
            "Load Balancer named {name} already exists and it is configured with \
             different parameters."
        );
        let stated = error_message(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );

        // The refused create left the load balancer the name DOES hold exactly as it was.
        let described = call(
            &router,
            REGION,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", name)],
        )
        .await;
        let doc = parse(&described.body);
        let descriptions = find(described.result(&doc), "LoadBalancerDescriptions")
            .map(|list| list.children().count())
            .unwrap_or_default();
        let description = find(described.result(&doc), "LoadBalancerDescriptions")
            .and_then(|list| list.children().next())
            .unwrap_or_else(|| panic!("case {case:?}: no description in {}", described.body));
        assert_eq!(
            (
                descriptions,
                kid_text(description, "DNSName"),
                kid_text(description, "Scheme"),
                member_texts(description, "AvailabilityZones"),
            ),
            (
                1,
                first_dns,
                Some("internet-facing".to_string()),
                vec![format!("{REGION}a")],
            ),
            "case {case:?}: the refused create must leave the held record as it was, got {}",
            described.body
        );
    }
}

/// Create `name` placed in `subnets` (and nothing else), with one HTTP 80 listener.
async fn create_in_subnets(
    router: &MultiServiceRouter,
    name: &str,
    subnets: &[&str],
) -> Wire {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("Listeners.member.1.Protocol", "HTTP"),
        param("Listeners.member.1.LoadBalancerPort", "80"),
        param("Listeners.member.1.InstancePort", "80"),
    ];
    for (slot, subnet) in subnets.iter().enumerate() {
        params.push(param(&format!("Subnets.member.{}", slot + 1), subnet));
    }
    call(router, REGION, "CreateLoadBalancer", &params).await
}

/// Ask `name` to detach `subnets`.
async fn detach(router: &MultiServiceRouter, name: &str, subnets: &[&str]) -> Wire {
    let mut params = vec![param("LoadBalancerName", name)];
    for (slot, subnet) in subnets.iter().enumerate() {
        params.push(param(&format!("Subnets.member.{}", slot + 1), subnet));
    }
    call(router, REGION, "DetachLoadBalancerFromSubnets", &params).await
}

/// The subnets `DescribeLoadBalancers` reports for `name`.
async fn reported_subnets(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let described = call(
        router,
        REGION,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    let doc = parse(&described.body);
    let description = find(described.result(&doc), "LoadBalancerDescriptions")
        .and_then(|list| list.children().next())
        .unwrap_or_else(|| panic!("no description in {}", described.body));
    member_texts(description, "Subnets")
}

/// `DetachLoadBalancerFromSubnets` is documented "the load balancer balances the traffic
/// among the remaining routable subnets", so there has to BE a remaining one: a detach that
/// would leave the load balancer attached to no subnet is refused with
/// `InvalidConfigurationRequestException` (awsQuery `InvalidConfigurationRequest`, HTTP
/// 409) and detaches nothing.
///
/// That one error code answers EVERY configuration rule this operation has, so the MESSAGE
/// is what tells them apart: it names the load balancer the request asked about and states
/// the rule the change would have broken. The rows differ along the name's own form, along
/// how many subnets the load balancer is attached to, and along the shape of the detach
/// that would empty it — because the message is built from the request, not from one
/// recorded call.
#[tokio::test]
async fn the_zero_subnet_detach_refusal_names_the_load_balancer_and_its_rule() {
    let router = emulator();

    // (case, the load balancer name, the subnets it is created in, the subnets the detach
    // names)
    let rows: [ZeroSubnetRow; 5] = [
        (
            "the only subnet of a one-subnet load balancer",
            "zero-subnet-elb",
            vec!["subnet-0123456789abcdef0"],
            vec!["subnet-0123456789abcdef0"],
        ),
        (
            "both subnets of a two-subnet load balancer",
            "z",
            vec!["subnet-0123456789abcdef0", "subnet-1a2b3c4d"],
            vec!["subnet-0123456789abcdef0", "subnet-1a2b3c4d"],
        ),
        (
            "all three subnets, named in another order",
            "Mixed-Case-Detach-ELB",
            vec![
                "subnet-0123456789abcdef0",
                "subnet-0123456789abcdef1",
                "subnet-1a2b3c4d",
            ],
            vec![
                "subnet-1a2b3c4d",
                "subnet-0123456789abcdef1",
                "subnet-0123456789abcdef0",
            ],
        ),
        (
            "the attached set with one id named twice",
            "a-long-hyphenated-detach-elb-name",
            vec!["subnet-0123456789abcdef0", "subnet-1a2b3c4d"],
            vec![
                "subnet-0123456789abcdef0",
                "subnet-1a2b3c4d",
                "subnet-0123456789abcdef0",
            ],
        ),
        (
            "the attached set beside a subnet it was never attached to",
            "detach-superset-elb",
            vec!["subnet-0123456789abcdef0"],
            vec!["subnet-0123456789abcdef0", "subnet-00000000"],
        ),
    ];

    for (case, name, attached, detached) in rows {
        let created = create_in_subnets(&router, name, &attached).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the fixture must be created, got {}",
            created.body
        );

        let refused = detach(&router, name, &detached).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (409, "InvalidConfigurationRequest"),
            "case {case:?}: expected 409 InvalidConfigurationRequest, got {} with {}",
            refused.status,
            refused.body
        );
        let expected = format!(
            "Requested configuration change for LoadBalancer \"{name}\" is invalid because \
             you attempted to detach all the subnets for this LoadBalancer and a \
             LoadBalancer cannot be attached to zero subnets in VPC."
        );
        let stated = error_message(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );

        // The refused detach detached nothing: every subnet the create placed it in is
        // still attached, in the order the record holds them.
        let after = reported_subnets(&router, name).await;
        let placed: Vec<String> = attached.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            after, placed,
            "case {case:?}: the refused detach must leave the placement as it was"
        );

        // The other side of the same rule: a detach that leaves ONE subnet behind is
        // applied, so the refusal is about reaching zero rather than about detaching.
        if let Some((last, rest)) = placed.split_last() {
            if !rest.is_empty() {
                let leaving_one: Vec<&str> = rest.iter().map(String::as_str).collect();
                let accepted = detach(&router, name, &leaving_one).await;
                assert_eq!(
                    accepted.status, 200,
                    "case {case:?}: a detach leaving one subnet must be accepted, got {}",
                    accepted.body
                );
                let after = reported_subnets(&router, name).await;
                assert_eq!(
                    after,
                    vec![last.clone()],
                    "case {case:?}: the accepted detach must leave the last subnet attached"
                );
            }
        }
    }
}

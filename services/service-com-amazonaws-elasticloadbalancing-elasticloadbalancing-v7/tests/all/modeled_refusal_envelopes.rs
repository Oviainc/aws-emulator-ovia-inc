// SPDX-License-Identifier: CC0-1.0

//! Every request Classic Elastic Load Balancing REFUSES is answered as the awsQuery
//! refusal its model declares: a 4xx status, `Content-Type: text/xml`, an
//! `ErrorResponse/Error` carrying `Type` `Sender` and the error's own `Code` beside the
//! envelope's `RequestId`, and the message Elastic Load Balancing states that refusal
//! with.
//!
//! Each row below is one refusal CLASS, and the rows of a class differ along what the
//! rule generalises over — which member carried the bad value, which value it carried,
//! which side of a published bound it fell on, which operation applied the rule — because
//! the status, the code and the sentence belong to the class rather than to one recorded
//! request. The accepted tables beside them pin the other side of each bound, so a
//! refusal that swallowed a valid value would fail here too.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.

use super::wire::{call, emulator, error_message, find, kid_text, param, parse, Element, Wire};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The error code this service answers a request whose values it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// One row of the refusal table: the case, the action, the form fields beside the action,
/// the HTTP status, the error code and the message the refusal states.
struct Refusal {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    message: String,
}

/// One health-check request for `name`.
fn health_check(name: &str, target: &str, interval: i32, timeout: i32) -> Vec<(String, String)> {
    vec![
        param("LoadBalancerName", name),
        param("HealthCheck.Target", target),
        param("HealthCheck.Interval", &interval.to_string()),
        param("HealthCheck.Timeout", &timeout.to_string()),
        param("HealthCheck.UnhealthyThreshold", "2"),
        param("HealthCheck.HealthyThreshold", "2"),
    ]
}

/// One listener, as a create carries it in slot `slot`.
fn listener(slot: u32, protocol: &str, port: i32, instance_port: i32) -> Vec<(String, String)> {
    vec![
        param(&format!("Listeners.member.{slot}.Protocol"), protocol),
        param(
            &format!("Listeners.member.{slot}.LoadBalancerPort"),
            &port.to_string(),
        ),
        param(
            &format!("Listeners.member.{slot}.InstancePort"),
            &instance_port.to_string(),
        ),
    ]
}

/// Create `name` with one HTTP 80 listener, placed by `placement`.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    placement: &[(String, String)],
) -> Wire {
    let mut params = vec![param("LoadBalancerName", name)];
    params.extend(listener(1, "HTTP", 80, 80));
    params.extend_from_slice(placement);
    call(router, REGION, "CreateLoadBalancer", &params).await
}

/// One Availability Zone placement.
fn in_zone() -> Vec<(String, String)> {
    vec![param("AvailabilityZones.member.1", &format!("{REGION}a"))]
}

/// A placement in two subnets, so a detach can take one away and still leave one.
fn in_subnets() -> Vec<(String, String)> {
    vec![
        param("Subnets.member.1", "subnet-0123456789abcdef0"),
        param("Subnets.member.2", "subnet-0123456789abcdef1"),
    ]
}

/// One attribute modification of `name`.
fn draining_timeout(name: &str, timeout: i32) -> Vec<(String, String)> {
    vec![
        param("LoadBalancerName", name),
        param("LoadBalancerAttributes.ConnectionDraining.Enabled", "true"),
        param(
            "LoadBalancerAttributes.ConnectionDraining.Timeout",
            &timeout.to_string(),
        ),
    ]
}

/// The bounds the Classic Load Balancers Guide publishes for a connection timeout, which
/// the refusal below states: "a value between 1 and 3,600 seconds".
const TIMEOUT_BOUNDS: (i32, i32) = (1, 3600);

/// Every refusal this service states is answered in the awsQuery refusal envelope, with
/// the status, the code and the sentence its class carries — and a refusal changes
/// nothing, so the load balancer the rows share is still readable afterwards.
#[tokio::test]
async fn every_refusal_answers_its_modeled_awsquery_envelope() {
    let router = emulator();
    for (name, placement) in [
        ("refusal-envelope-elb", in_zone()),
        ("refusal-subnet-elb", in_subnets()),
    ] {
        let created = create(&router, name, &placement).await;
        assert_eq!(
            created.status, 200,
            "the fixture {name} must be created, got {}",
            created.body
        );
    }
    let timeout_bound = format!(
        "Time out must be at least {} second and no more than {} seconds",
        TIMEOUT_BOUNDS.0, TIMEOUT_BOUNDS.1
    );
    let protocol_set = "HTTP, TCP, HTTPS, SSL";

    let mut rows: Vec<Refusal> = vec![
        Refusal {
            case: "a describe naming a load balancer the region does not hold",
            action: "DescribeLoadBalancers",
            params: vec![param("LoadBalancerNames.member.1", "no-such-elb")],
            status: 400,
            code: "LoadBalancerNotFound",
            message: "There is no ACTIVE Load Balancer named 'no-such-elb'".to_string(),
        },
        Refusal {
            case: "a describe naming a held load balancer beside one it does not hold",
            action: "DescribeLoadBalancers",
            params: vec![
                param("LoadBalancerNames.member.1", "refusal-envelope-elb"),
                param("LoadBalancerNames.member.2", "also-not-here"),
            ],
            status: 400,
            code: "LoadBalancerNotFound",
            message: "There is no ACTIVE Load Balancer named 'also-not-here'".to_string(),
        },
        Refusal {
            case: "a health check whose timeout equals its interval",
            action: "ConfigureHealthCheck",
            params: health_check("refusal-envelope-elb", "HTTP:80/ping", 10, 10),
            status: 400,
            code: VALIDATION_ERROR,
            message: "HealthCheck timeout must be less than interval.".to_string(),
        },
        Refusal {
            case: "a health check whose timeout is above its interval",
            action: "ConfigureHealthCheck",
            params: health_check("refusal-envelope-elb", "TCP:80", 5, 60),
            status: 400,
            code: VALIDATION_ERROR,
            message: "HealthCheck timeout must be less than interval.".to_string(),
        },
        Refusal {
            case: "a health-check target naming a protocol the service does not check with",
            action: "ConfigureHealthCheck",
            params: health_check("refusal-envelope-elb", "BOGUS:80", 30, 5),
            status: 400,
            code: VALIDATION_ERROR,
            message: format!("HealthCheck Target must begin with one of {protocol_set}"),
        },
        Refusal {
            case: "a health-check target naming no protocol at all",
            action: "ConfigureHealthCheck",
            params: health_check("refusal-envelope-elb", "80/ping", 30, 5),
            status: 400,
            code: VALIDATION_ERROR,
            message: format!("HealthCheck Target must begin with one of {protocol_set}"),
        },
        Refusal {
            case: "a create naming a scheme that is neither documented value",
            action: "CreateLoadBalancer",
            params: {
                let mut params = vec![
                    param("LoadBalancerName", "bogus-scheme-elb"),
                    param("Scheme", "bogus-scheme"),
                ];
                params.extend(listener(1, "HTTP", 80, 80));
                params.extend(in_zone());
                params
            },
            status: 400,
            code: "InvalidScheme",
            message: "Scheme can either be internet-facing or internal.".to_string(),
        },
        Refusal {
            case: "a create naming a scheme in the wrong case",
            action: "CreateLoadBalancer",
            params: {
                let mut params = vec![
                    param("LoadBalancerName", "mixed-scheme-elb"),
                    param("Scheme", "Internal"),
                ];
                params.extend(listener(1, "HTTP", 80, 80));
                params.extend(in_zone());
                params
            },
            status: 400,
            code: "InvalidScheme",
            message: "Scheme can either be internet-facing or internal.".to_string(),
        },
        Refusal {
            case: "a create whose listener Protocol is outside the documented set",
            action: "CreateLoadBalancer",
            params: {
                let mut params = vec![param("LoadBalancerName", "udp-elb")];
                params.extend(listener(1, "UDP", 53, 53));
                params.extend(in_zone());
                params
            },
            status: 400,
            code: VALIDATION_ERROR,
            message: format!("Protocol must be one of {protocol_set}"),
        },
        Refusal {
            case: "a listener create whose InstanceProtocol is outside the documented set",
            action: "CreateLoadBalancerListeners",
            params: {
                let mut params = vec![param("LoadBalancerName", "refusal-envelope-elb")];
                params.extend(listener(1, "HTTP", 8080, 8080));
                params.push(param("Listeners.member.1.InstanceProtocol", "SPDY"));
                params
            },
            status: 400,
            code: VALIDATION_ERROR,
            message: format!("InstanceProtocol must be one of {protocol_set}"),
        },
        Refusal {
            case: "a listener create re-declaring a configured port with another back end",
            action: "CreateLoadBalancerListeners",
            params: {
                let mut params = vec![param("LoadBalancerName", "refusal-envelope-elb")];
                params.extend(listener(1, "HTTP", 80, 9090));
                params
            },
            status: 400,
            code: "DuplicateListener",
            message: "A listener already exists for refusal-envelope-elb with \
                      LoadBalancerPort 80, but with a different InstancePort, Protocol, \
                      or SSLCertificateId"
                .to_string(),
        },
        Refusal {
            case: "a create naming a load balancer the region already holds",
            action: "CreateLoadBalancer",
            params: {
                let mut params = vec![param("LoadBalancerName", "refusal-envelope-elb")];
                params.extend(listener(1, "TCP", 81, 81));
                params.extend(in_zone());
                params
            },
            status: 400,
            code: "DuplicateLoadBalancerName",
            message: "Load Balancer named refusal-envelope-elb already exists and it is \
                      configured with different parameters."
                .to_string(),
        },
        Refusal {
            case: "a detach that would leave the load balancer with no subnet",
            action: "DetachLoadBalancerFromSubnets",
            params: vec![
                param("LoadBalancerName", "refusal-subnet-elb"),
                param("Subnets.member.1", "subnet-0123456789abcdef0"),
                param("Subnets.member.2", "subnet-0123456789abcdef1"),
            ],
            status: 409,
            code: "InvalidConfigurationRequest",
            message: "Requested configuration change for LoadBalancer \
                      \"refusal-subnet-elb\" is invalid because you attempted to detach all \
                      the subnets for this LoadBalancer and a LoadBalancer cannot be \
                      attached to zero subnets in VPC."
                .to_string(),
        },
        Refusal {
            case: "an apply naming a load balancer the region does not hold",
            action: "ApplySecurityGroupsToLoadBalancer",
            params: vec![
                param("LoadBalancerName", "no-such-elb"),
                param("SecurityGroups.member.1", "sg-0123456789abcdef0"),
            ],
            status: 400,
            code: "LoadBalancerNotFound",
            message: "There is no ACTIVE Load Balancer named 'no-such-elb'".to_string(),
        },
        Refusal {
            case: "an apply naming more security groups than one load balancer holds",
            action: "ApplySecurityGroupsToLoadBalancer",
            params: {
                let mut params = vec![param("LoadBalancerName", "refusal-envelope-elb")];
                for slot in 1..=6 {
                    params.push(param(
                        &format!("SecurityGroups.member.{slot}"),
                        &format!("sg-{slot:017}"),
                    ));
                }
                params
            },
            status: 400,
            code: VALIDATION_ERROR,
            message: "SecurityGroups must carry at most 5 entries; got 6".to_string(),
        },
    ];
    // One row per side of the published connection-timeout bound, built here because each
    // states the same sentence: the bound itself.
    for timeout in [TIMEOUT_BOUNDS.0 - 1, TIMEOUT_BOUNDS.1 + 1, 99_999] {
        rows.push(Refusal {
            case: "a connection-draining timeout outside the published bound",
            action: "ModifyLoadBalancerAttributes",
            params: draining_timeout("refusal-envelope-elb", timeout),
            status: 400,
            code: VALIDATION_ERROR,
            message: timeout_bound.clone(),
        });
    }

    for row in rows {
        let Refusal {
            case,
            action,
            params,
            status,
            code,
            message: expected,
        } = row;
        let answered = call(&router, REGION, action, &params).await;
        assert_eq!(
            (answered.status, answered.error_code().as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code} from {action}, got {} with {}",
            answered.status,
            answered.body
        );
        assert_eq!(
            answered.content_type, "text/xml",
            "case {case:?}: an awsQuery client must get an XML refusal, got {:?} with {}",
            answered.content_type, answered.body
        );
        let doc = parse(&answered.body);
        let envelope = doc.root_element();
        assert_eq!(
            (
                envelope.name.as_str(),
                find(envelope, "Type").and_then(Element::text),
                find(envelope, "RequestId").and_then(Element::text).is_some(),
            ),
            ("ErrorResponse", Some("Sender"), true),
            "case {case:?}: expected a Sender <ErrorResponse> carrying a <RequestId>, got {}",
            answered.body
        );
        let stated = error_message(&answered);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }

    // Every refusal above left the load balancer the rows share exactly as its create
    // configured it: one HTTP 80 listener and the default health check.
    let described = call(
        &router,
        REGION,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", "refusal-envelope-elb")],
    )
    .await;
    let doc = parse(&described.body);
    let description = find(described.result(&doc), "LoadBalancerDescriptions")
        .and_then(|list| list.children().next())
        .unwrap_or_else(|| panic!("no description in {}", described.body));
    let health = find(description, "HealthCheck")
        .unwrap_or_else(|| panic!("no HealthCheck in {}", described.body));
    assert_eq!(
        (
            find(description, "ListenerDescriptions")
                .map(|list| list.children().count())
                .unwrap_or_default(),
            kid_text(health, "Target"),
            kid_text(health, "Interval"),
            kid_text(health, "Timeout"),
        ),
        (1, Some("TCP:80".to_string()), Some("30".to_string()), Some("5".to_string())),
        "a refused request must change nothing, got {}",
        described.body
    );
}

/// The other side of every bound the rows above refuse: a value ON the published bound is
/// accepted, read back off the record that now holds it.
#[tokio::test]
async fn a_value_on_the_published_bound_is_accepted() {
    let router = emulator();
    let name = "bound-accepted-elb";
    let created = create(&router, name, &in_zone()).await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {}",
        created.body
    );

    // (case, the connection-draining timeout, the health check)
    let rows: [(&str, i32, (&str, i32, i32)); 4] = [
        ("the smallest timeout and the smallest interval", 1, ("TCP:80", 5, 4)),
        ("the largest timeout and the largest interval", 3600, ("HTTPS:443/up", 300, 60)),
        ("a timeout one below the largest", 3599, ("SSL:443", 61, 60)),
        ("a timeout one above the smallest", 2, ("HTTP:80/index.html", 10, 2)),
    ];

    for (case, timeout, (target, interval, check_timeout)) in rows {
        let modified = call(
            &router,
            REGION,
            "ModifyLoadBalancerAttributes",
            &draining_timeout(name, timeout),
        )
        .await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: a timeout on the published bound must be accepted, got {}",
            modified.body
        );
        let configured = call(
            &router,
            REGION,
            "ConfigureHealthCheck",
            &health_check(name, target, interval, check_timeout),
        )
        .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: a timeout below the interval must be accepted, got {}",
            configured.body
        );

        let described = call(
            &router,
            REGION,
            "DescribeLoadBalancerAttributes",
            &[param("LoadBalancerName", name)],
        )
        .await;
        let doc = parse(&described.body);
        let draining = find(described.result(&doc), "ConnectionDraining")
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionDraining in {}", described.body));
        assert_eq!(
            kid_text(draining, "Timeout"),
            Some(timeout.to_string()),
            "case {case:?}: the stored timeout must be the accepted one, got {}",
            described.body
        );
    }
}

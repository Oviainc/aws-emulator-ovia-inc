// SPDX-License-Identifier: CC0-1.0

//! What the `2015-12-01` API of this shared endpoint actually serves: the ELBv2
//! `ip`-target surface, reached the way a client reaches it.
//!
//! The two Elastic Load Balancing models share one endpoint name, and the awsQuery
//! envelope's `Version` names the API a request is for. WHICH model a pair resolves to is
//! pinned by `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs` (both
//! services mounted) and what a resolved pair reaches by
//! [`super::shared_endpoint_api_version_dispatch`]. This suite pins the behaviour the
//! issue is titled after, at that version on that shared name: an ip-type target group is
//! created, targets are registered against the rule the model states for them, and what a
//! later `DescribeTargetHealth` reports is what was stored. Those are the calls an ECS
//! `awsvpc` task or an ENI-backed target makes, and they were unreachable while every
//! `2015-12-01` envelope was refused.
//!
//! The rule under test (`…elasticloadbalancingv2…/src/state/network.rs`, from
//! `CreateTargetGroupInput$TargetType` and `TargetDescription$AvailabilityZone`): an `ip`
//! target must be an IP address of the address type the target group carries its targets
//! in, and where the address falls in that group's VPC decides the Availability Zone it
//! takes and whether it is accepted at all. This crate mounts no EC2 service, so nothing
//! is known here about the VPC the group names — and a refusal is never assumed from that
//! silence: the half of the rule this suite can reach is the one the request alone
//! decides (the id IS an address, or it is no target), and the registration is otherwise
//! stored as it was asked for. The halves that need the account's network are tabled with
//! both services mounted, in `core/router/tests/all/elbv2_network_resolution.rs`.

use crate::wire::{call_at_version, emulator, find, param, parse, Element};

/// The API version the ELBv2 model declares, as that crate publishes it for this shared
/// front door — read rather than restated.
const V2_VERSION: &str =
    service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::service::shared_endpoint::API_VERSION;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A VPC id no EC2 store here holds, which is what leaves the half of the rule the
/// request alone decides as the only half that applies.
const VPC_ID: &str = "vpc-0abc1234";

/// What a row expects of one `RegisterTargets` call.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Registered, and `DescribeTargetHealth` reports this Availability Zone (`None`:
    /// none at all, because none was asked for and none can be detected without the
    /// VPC).
    Registered(Option<&'static str>),
    /// Refused as an invalid request VALUE (`400 ValidationError`), with nothing stored
    /// for the target.
    Refused,
}

/// One registration: the target's id and the `AvailabilityZone` it names, and what the
/// rule must do with them.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    id: &'static str,
    zone: Option<&'static str>,
    expected: Expect,
}

#[tokio::test]
async fn ip_targets_are_registered_through_the_shared_endpoint_by_the_rule() {
    let router = emulator();

    // An ip-type target group, created through the SHARED signing name at the ELBv2
    // version — the call the issue reports as `InvalidAction`.
    let created = call_at_version(
        &router,
        REGION,
        "CreateTargetGroup",
        Some(V2_VERSION),
        &[
            param("Name", "shared-ip-tg"),
            param("Protocol", "HTTP"),
            param("Port", "80"),
            param("TargetType", "ip"),
            param("VpcId", VPC_ID),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create the ip target group: expected 200, got {} with {}",
        created.status, created.body
    );
    let document = parse(&created.body);
    let arn = find(&document, "TargetGroupArn")
        .and_then(Element::text)
        .unwrap_or_else(|| panic!("no TargetGroupArn in {}", created.body))
        .to_string();
    assert_eq!(
        find(&document, "TargetType").and_then(Element::text),
        Some("ip"),
        "case create the ip target group: expected the ip target type, got {}",
        created.body
    );
    // The ARN is the ELBv2 model's own: its `targetgroup/` resource, in the region the
    // request was signed for, under this emulator's account — so the record was minted by
    // that model and not by the front door's.
    assert!(
        arn.starts_with("arn:aws:elasticloadbalancing:")
            && arn.contains(REGION)
            && arn.contains(":targetgroup/shared-ip-tg/"),
        "case create the ip target group: expected an ELBv2 target-group ARN in {REGION}, \
         got {arn}"
    );

    // The rows differ along what the rule generalises over: which range the address is
    // in, whether it is an address at all, and whether the request names a zone.
    let rows: [Row; 5] = [
        Row {
            case: "an RFC 1918 address from the 10/8 range",
            id: "10.1.2.3",
            zone: None,
            expected: Expect::Registered(None),
        },
        Row {
            case: "an RFC 1918 address from the 192.168/16 range, with a zone",
            id: "192.168.5.5",
            zone: Some("all"),
            expected: Expect::Registered(Some("all")),
        },
        Row {
            case: "an RFC 6598 address",
            id: "100.64.0.1",
            zone: None,
            expected: Expect::Registered(None),
        },
        Row {
            // A publicly routable address is one no `ip` target group may carry, but
            // WHICH addresses this one's VPC admits is a fact about that VPC — and no EC2
            // is mounted here to report it. Nothing is refused on that silence, so the
            // registration is stored as it was asked for; the refusal is tabled where the
            // VPC is known, in `core/router/tests/all/elbv2_network_resolution.rs`.
            case: "a publicly routable address, with no VPC known to judge it against",
            id: "54.239.28.85",
            zone: None,
            expected: Expect::Registered(None),
        },
        Row {
            // What this request names is decided by the request alone: a target of an
            // `ip` group IS an address, so an instance id is no target of it whatever any
            // other service holds.
            case: "an instance id, which is not an address at all",
            id: "i-1111111111111111",
            zone: None,
            expected: Expect::Refused,
        },
    ];

    for row in rows {
        let case = row.case;
        let mut params = vec![param("TargetGroupArn", &arn), param("Targets.member.1.Id", row.id)];
        if let Some(zone) = row.zone {
            params.push(param("Targets.member.1.AvailabilityZone", zone));
        }
        let registered =
            call_at_version(&router, REGION, "RegisterTargets", Some(V2_VERSION), &params).await;
        let health = call_at_version(
            &router,
            REGION,
            "DescribeTargetHealth",
            Some(V2_VERSION),
            &[
                param("TargetGroupArn", &arn),
                param("Targets.member.1.Id", row.id),
            ],
        )
        .await;
        let reported = parse(&health.body);
        match row.expected {
            Expect::Registered(expected_zone) => {
                assert_eq!(
                    registered.status, 200,
                    "case {case:?}: expected 200, got {} with {}",
                    registered.status, registered.body
                );
                assert_eq!(
                    find(&reported, "Reason").and_then(Element::text),
                    Some("Target.NotInUse"),
                    "case {case:?}: a target of a group no listener forwards to is reported \
                     not in use, got {}",
                    health.body
                );
                assert_eq!(
                    find(&reported, "AvailabilityZone").and_then(Element::text),
                    expected_zone,
                    "case {case:?}: expected the stored Availability Zone \
                     {expected_zone:?}, got {}",
                    health.body
                );
            }
            Expect::Refused => {
                assert_eq!(
                    (registered.status, registered.error_code().as_str()),
                    (400, "ValidationError"),
                    "case {case:?}: expected 400 ValidationError, got {} with {}",
                    registered.status,
                    registered.body
                );
                assert_eq!(
                    find(&reported, "Reason").and_then(Element::text),
                    Some("Target.NotRegistered"),
                    "case {case:?}: a refused target must not be registered, got {}",
                    health.body
                );
            }
        }
    }
}

/// The version half of the routing key has a DEFAULT, and it is this front door's own
/// API: an envelope that names no `Version` has stated nothing to disagree with, so it is
/// served out of the Classic model (`src/service/api_version.rs`'s `serving_model`), while
/// one naming the ELBv2 version is served out of that model.
///
/// Asserted on `DescribeLoadBalancers`, an action BOTH models declare with different
/// outputs, so the answer can only come from the serving model — and on the `@xmlNamespace`
/// each model declares for its own document, which is that model's signature on the wire.
/// The positive no-`Version` row is the one `core/router`'s suite does not carry: it pins
/// only that a no-`Version` envelope cannot reach an ELBv2-ONLY action.
#[tokio::test]
async fn the_api_version_an_envelope_names_decides_which_model_answers() {
    let router = emulator();

    // The rows differ along the one field that decides: the `Version` the envelope names,
    // absent or the ELBv2 one, and the namespace of the document each must be answered in.
    let rows: [(&str, Option<&str>, &str); 2] = [
        (
            "no Version field at all, served out of this front door's own API",
            None,
            "http://elasticloadbalancing.amazonaws.com/doc/2012-06-01/",
        ),
        (
            "the ELBv2 version, served out of that model",
            Some(V2_VERSION),
            "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/",
        ),
    ];

    for (case, version, namespace) in rows {
        let answered =
            call_at_version(&router, REGION, "DescribeLoadBalancers", version, &[]).await;
        assert_eq!(
            (answered.status, answered.error_code().as_str()),
            (200, ""),
            "case {case:?}: an action both models declare must be served, got {} with {}",
            answered.status,
            answered.body
        );
        assert!(
            answered.body.contains(namespace),
            "case {case:?}: expected the answer in {namespace}, got {}",
            answered.body
        );
    }
}

#[tokio::test]
async fn a_health_check_port_outside_the_modelled_range_is_refused_through_the_shared_endpoint() {
    let router = emulator();

    // `HealthCheckPort` carries either the symbolic `traffic-port` or a port number, so
    // the model gives it no `@range` and the handler enforces the one its documentation
    // states. The rows are the symbolic value, each published bound of the `Port` shape
    // (1 and 65535) and one past the top.
    let rows: [(&str, &str, bool); 4] = [
        ("the symbolic default", "traffic-port", true),
        ("the lowest port", "1", true),
        ("the highest port", "65535", true),
        ("one past the highest port", "65536", false),
    ];

    for (index, (case, port, accepted)) in rows.into_iter().enumerate() {
        let name = format!("shared-hc-tg-{index}");
        let created = call_at_version(
            &router,
            REGION,
            "CreateTargetGroup",
            Some(V2_VERSION),
            &[
                param("Name", &name),
                param("Protocol", "HTTP"),
                param("Port", "80"),
                param("VpcId", VPC_ID),
                param("HealthCheckPort", port),
            ],
        )
        .await;
        if accepted {
            let document = parse(&created.body);
            assert_eq!(
                (
                    created.status,
                    find(&document, "HealthCheckPort").and_then(Element::text)
                ),
                (200, Some(port)),
                "case {case:?} ({port}): expected the stored HealthCheckPort, got {} with \
                 {}",
                created.status,
                created.body
            );
        } else {
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?} ({port}): expected 400 ValidationError, got {} with {}",
                created.status,
                created.body
            );
            // A refused create stores nothing: the name is unknown to the read back.
            let described = call_at_version(
                &router,
                REGION,
                "DescribeTargetGroups",
                Some(V2_VERSION),
                &[param("Names.member.1", &name)],
            )
            .await;
            assert_eq!(
                (described.status, described.error_code().as_str()),
                (400, "TargetGroupNotFound"),
                "case {case:?}: a refused create must store no target group, got {} with {}",
                described.status,
                described.body
            );
        }
    }
}

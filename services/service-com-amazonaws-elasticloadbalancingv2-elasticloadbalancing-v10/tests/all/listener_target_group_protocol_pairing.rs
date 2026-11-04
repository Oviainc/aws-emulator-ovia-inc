// SPDX-License-Identifier: CC0-1.0

//! Which TARGET GROUPS a listener may forward to, driven over the wire exactly as a client
//! drives it.
//!
//! One rule is asserted here, over a SET of inputs rather than over the one call a
//! recording happens to carry: a load balancer carries one KIND of traffic, and
//! `TargetGroup$Protocol` pairs each protocol with the load balancer it belongs to — "For
//! Application Load Balancers, the supported protocols are HTTP and HTTPS. For Network Load
//! Balancers, the supported protocols are TCP, TLS, UDP, TCP_UDP, QUIC, and TCP_QUIC. … For
//! Gateway Load Balancers, the supported protocol is GENEVE" — so an action attached to one
//! of its listeners may forward only to a target group of its own kind, and one naming a
//! target group of another kind is refused with `IncompatibleProtocols` naming that target
//! group.
//!
//! The rows differ along what that rule generalises over: the load balancer family, every
//! protocol the model pairs with a family (plus a Lambda target group, which has no
//! protocol at all), whether the pairing crosses a family boundary, which operation
//! attaches the action (the listener create and modification, the rule create and
//! modification), and whether the forward names its target group through
//! `Action$TargetGroupArn` or through `ForwardConfig$TargetGroups`.
//!
//! After every refusal the resource is read again, to prove the refused call attached
//! nothing.

use crate::forward_action_config_and_mutual_authentication::describe_listener;
use crate::listener_rules::{host_header, Form};
use crate::provisioning_loop::{action, call, emulator, first_text, members, p, Reply};
use crate::target_group_attributes_and_modification::create_target_group;
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `IncompatibleProtocolsException`, which `CreateListener`,
/// `ModifyListener`, `CreateRule` and `ModifyRule` all model
/// (`smithy_query(query="operation",operation="CreateListener")`: `awsQueryError` code
/// `IncompatibleProtocols`).
const INCOMPATIBLE_PROTOCOLS: &str = "IncompatibleProtocols";

/// One target group under test: the case, the fields it is created with, and the load
/// balancer family it serves — read from `TargetGroup$Protocol`'s own pairing of each
/// protocol with a load balancer type, which is the list this table spans.
struct GroupKind {
    case: &'static str,
    fields: &'static [(&'static str, &'static str)],
    family: &'static str,
}

/// Every protocol that pairing names, plus a Lambda target group, which carries no protocol
/// and is an Application Load Balancer's.
const GROUP_KINDS: &[GroupKind] = &[
    GroupKind {
        case: "an HTTP group",
        fields: &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        family: "application",
    },
    GroupKind {
        case: "an HTTPS group",
        fields: &[("Protocol", "HTTPS"), ("Port", "443"), ("VpcId", "vpc-1")],
        family: "application",
    },
    GroupKind {
        case: "a Lambda group, which carries no protocol",
        fields: &[("TargetType", "lambda")],
        family: "application",
    },
    GroupKind {
        case: "a TCP group",
        fields: &[("Protocol", "TCP"), ("Port", "80"), ("VpcId", "vpc-1")],
        family: "network",
    },
    GroupKind {
        case: "a TLS group",
        fields: &[("Protocol", "TLS"), ("Port", "443"), ("VpcId", "vpc-1")],
        family: "network",
    },
    GroupKind {
        case: "a UDP group",
        fields: &[("Protocol", "UDP"), ("Port", "53"), ("VpcId", "vpc-1")],
        family: "network",
    },
    GroupKind {
        case: "a TCP_UDP group",
        fields: &[("Protocol", "TCP_UDP"), ("Port", "80"), ("VpcId", "vpc-1")],
        family: "network",
    },
    GroupKind {
        case: "a QUIC group",
        fields: &[("Protocol", "QUIC"), ("Port", "443"), ("VpcId", "vpc-1")],
        family: "network",
    },
    GroupKind {
        case: "a GENEVE group",
        fields: &[("Protocol", "GENEVE"), ("Port", "6081"), ("VpcId", "vpc-1")],
        family: "gateway",
    },
];

/// One load balancer family a listener is made on: its `Type`, and the listener fields a
/// listener of it takes.
struct FamilyUnderTest {
    lb_type: &'static str,
    listener: &'static [(&'static str, &'static str)],
}

/// Every value `LoadBalancerTypeEnum` declares.
const FAMILIES: &[FamilyUnderTest] = &[
    FamilyUnderTest {
        lb_type: "application",
        listener: &[("Protocol", "HTTP"), ("Port", "80")],
    },
    FamilyUnderTest {
        lb_type: "network",
        listener: &[("Protocol", "TCP"), ("Port", "80")],
    },
    FamilyUnderTest {
        // "You can't specify a protocol for a Gateway Load Balancer."
        lb_type: "gateway",
        listener: &[],
    },
];

/// Create a load balancer of `lb_type` across two subnets and answer its ARN.
async fn load_balancer(router: &MultiServiceRouter, name: &str, lb_type: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", lb_type));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create the {lb_type} load balancer: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create the {lb_type} load balancer: no ARN in {}", reply.body))
}

/// The one target group of each kind, created once for the whole suite.
async fn target_groups(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, kind) in GROUP_KINDS.iter().enumerate() {
        arns.push(create_target_group(router, &format!("{prefix}-{index}"), kind.fields).await);
    }
    arns
}

/// A forward action under `prefix`, naming its target group through `Action$TargetGroupArn`
/// when `shorthand`, and through `ForwardConfig$TargetGroups` when not — the two spellings
/// of one forward, so the pairing is judged whichever one the request used.
fn forward(prefix: &str, target_group: &str, shorthand: bool) -> Form {
    let mut form = vec![p(&format!("{prefix}Type"), "forward")];
    if shorthand {
        form.push(p(&format!("{prefix}TargetGroupArn"), target_group));
    } else {
        form.push(p(
            &format!("{prefix}ForwardConfig.TargetGroups.member.1.TargetGroupArn"),
            target_group,
        ));
    }
    form
}

/// Assert that one reply is the modelled refusal, naming the target group that does not
/// pair with the load balancer.
fn assert_refused(case: &str, where_from: &str, reply: &Reply, target_group: &str) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, INCOMPATIBLE_PROTOCOLS),
        "case {case}: {where_from} expected 400 {INCOMPATIBLE_PROTOCOLS}, got {} {}",
        reply.status,
        reply.body
    );
    let message = first_text(&reply.body, "Message").unwrap_or_default();
    assert!(
        message.contains(target_group),
        "case {case}: {where_from} must name the target group that does not pair, got {message}"
    );
}

/// The `TargetGroupArn` of the one default action of the one listener in a reply.
fn default_action_target(body: &str) -> Option<String> {
    let listeners = members(body, "Listeners");
    let actions = members(listeners.first()?, "DefaultActions");
    first_text(actions.first()?, "TargetGroupArn")
}

/// A listener forwards only to a target group of its load balancer's own kind: a create
/// naming one of another kind is refused, under either spelling of the forward, and the
/// load balancer is left with no such listener.
#[tokio::test]
async fn a_listener_is_created_only_for_a_target_group_of_its_load_balancers_kind() {
    let (router, _engine) = emulator();
    let groups = target_groups(&router, "pairing-create").await;

    for family in FAMILIES {
        let lb = load_balancer(&router, &format!("pairing-create-{}", family.lb_type), family.lb_type)
            .await;
        let mut port = 1000;
        for (kind, group) in GROUP_KINDS.iter().zip(&groups) {
            for shorthand in [true, false] {
                let case = format!(
                    "{} on a {} load balancer, named {}",
                    kind.case,
                    family.lb_type,
                    if shorthand { "directly" } else { "through a forward config" }
                );
                port += 1;
                let mut form = action("CreateListener");
                form.push(p("LoadBalancerArn", &lb));
                for (key, value) in family.listener {
                    form.push(p(key, value));
                }
                if !family.listener.is_empty() {
                    // Each row needs a port of its own: one listener per port.
                    form.retain(|(key, _)| key != "Port");
                    form.push(p("Port", &port.to_string()));
                }
                form.extend(forward("DefaultActions.member.1.", group, shorthand));
                let reply = call(&router, &form).await;

                if kind.family == family.lb_type {
                    assert_eq!(
                        reply.status, 200,
                        "case {case}: a target group of the load balancer's own kind is \
                         admitted, got {} {}",
                        reply.status, reply.body
                    );
                    let listener = first_text(&reply.body, "ListenerArn")
                        .unwrap_or_else(|| panic!("case {case}: no ListenerArn in {}", reply.body));
                    let read = describe_listener(&router, &listener).await;
                    assert_eq!(
                        default_action_target(&read.body).as_deref(),
                        Some(group.as_str()),
                        "case {case}: the stored listener must forward to it, got {}",
                        read.body
                    );
                    // A gateway load balancer takes no port, so only one listener of it is
                    // made; the compatible row is reached once per spelling, and the second
                    // one is the idempotent repeat the model documents.
                    continue;
                }

                assert_refused(&case, "CreateListener", &reply, group);
                // Nothing was attached: the load balancer holds no listener forwarding
                // there.
                let mut form = action("DescribeListeners");
                form.push(p("LoadBalancerArn", &lb));
                let listeners = call(&router, &form).await;
                assert!(
                    !listeners.body.contains(group.as_str()),
                    "case {case}: the refused create must have stored nothing, got {}",
                    listeners.body
                );
            }
        }
    }
}

/// A MODIFICATION is judged by the same pairing: a listener cannot be moved onto a target
/// group of another kind, and the refused call leaves the listener forwarding where it was.
#[tokio::test]
async fn a_listener_is_modified_only_onto_a_target_group_of_its_load_balancers_kind() {
    let (router, _engine) = emulator();
    let groups = target_groups(&router, "pairing-modify").await;
    let application = groups[0].clone();

    let lb = load_balancer(&router, "pairing-modify-lb", "application").await;
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.extend(forward("DefaultActions.member.1.", &application, true));
    let created = call(&router, &form).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the modified listener: no ARN in {}", created.body));

    for (kind, group) in GROUP_KINDS.iter().zip(&groups) {
        for shorthand in [true, false] {
            let case = format!(
                "moving an application listener onto {}, named {}",
                kind.case,
                if shorthand { "directly" } else { "through a forward config" }
            );
            let mut form = action("ModifyListener");
            form.push(p("ListenerArn", &listener));
            form.extend(forward("DefaultActions.member.1.", group, shorthand));
            let reply = call(&router, &form).await;
            if kind.family == "application" {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a target group of its own kind is admitted, got {} {}",
                    reply.status, reply.body
                );
                // Put it back, so each row starts from the same listener.
                let mut form = action("ModifyListener");
                form.push(p("ListenerArn", &listener));
                form.extend(forward("DefaultActions.member.1.", &application, true));
                call(&router, &form).await;
                continue;
            }
            assert_refused(&case, "ModifyListener", &reply, group);
            let read = describe_listener(&router, &listener).await;
            assert_eq!(
                default_action_target(&read.body).as_deref(),
                Some(application.as_str()),
                "case {case}: the refused modification must have changed nothing, got {}",
                read.body
            );
        }
    }
}

/// A RULE's actions are judged by the same pairing, on the create that stores them and on
/// the modification that replaces them.
#[tokio::test]
async fn a_rule_forwards_only_to_a_target_group_of_its_load_balancers_kind() {
    let (router, _engine) = emulator();
    let groups = target_groups(&router, "pairing-rule").await;
    let application = groups[0].clone();

    let lb = load_balancer(&router, "pairing-rule-lb", "application").await;
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.extend(forward("DefaultActions.member.1.", &application, true));
    let created = call(&router, &form).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the rule listener: no ARN in {}", created.body));

    // One rule forwarding where it may, which every modification row starts from.
    let mut form = action("CreateRule");
    form.push(p("ListenerArn", &listener));
    form.push(p("Priority", "1"));
    form.extend(host_header("kept.example.com"));
    form.extend(forward("Actions.member.1.", &application, true));
    let rule = call(&router, &form).await;
    let rule_arn = first_text(&rule.body, "RuleArn")
        .unwrap_or_else(|| panic!("case the modified rule: no RuleArn in {}", rule.body));

    let mut priority = 100;
    for (kind, group) in GROUP_KINDS.iter().zip(&groups) {
        for shorthand in [true, false] {
            let spelling = if shorthand {
                "directly"
            } else {
                "through a forward config"
            };
            priority += 1;

            let case = format!("a rule forwarding to {}, named {spelling}", kind.case);
            let mut form = action("CreateRule");
            form.push(p("ListenerArn", &listener));
            form.push(p("Priority", &priority.to_string()));
            form.extend(host_header(&format!("row-{priority}.example.com")));
            form.extend(forward("Actions.member.1.", group, shorthand));
            let reply = call(&router, &form).await;
            if kind.family == "application" {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a target group of its own kind is admitted, got {} {}",
                    reply.status, reply.body
                );
            } else {
                assert_refused(&case, "CreateRule", &reply, group);
                let mut form = action("DescribeRules");
                form.push(p("ListenerArn", &listener));
                let rules = call(&router, &form).await;
                assert!(
                    !rules.body.contains(group.as_str()),
                    "case {case}: the refused create must have stored nothing, got {}",
                    rules.body
                );
            }

            let case = format!("a rule MOVED onto {}, named {spelling}", kind.case);
            let mut form = action("ModifyRule");
            form.push(p("RuleArn", &rule_arn));
            form.extend(forward("Actions.member.1.", group, shorthand));
            let reply = call(&router, &form).await;
            if kind.family == "application" {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a target group of its own kind is admitted, got {} {}",
                    reply.status, reply.body
                );
                let mut form = action("ModifyRule");
                form.push(p("RuleArn", &rule_arn));
                form.extend(forward("Actions.member.1.", &application, true));
                call(&router, &form).await;
                continue;
            }
            assert_refused(&case, "ModifyRule", &reply, group);
            let mut form = action("DescribeRules");
            form.push(p("RuleArns.member.1", &rule_arn));
            let read = call(&router, &form).await;
            assert!(
                read.body.contains(application.as_str()) && !read.body.contains(group.as_str()),
                "case {case}: the refused modification must have changed nothing, got {}",
                read.body
            );
        }
    }
}

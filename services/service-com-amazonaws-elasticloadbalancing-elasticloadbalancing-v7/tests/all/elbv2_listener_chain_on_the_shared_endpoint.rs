// SPDX-License-Identifier: CC0-1.0

//! The ELBv2 provisioning CHAIN on the shared Elastic Load Balancing endpoint: a load
//! balancer, a target group and a listener that forwards to it, each step naming an ARN
//! the step before it minted, all at `Version=2015-12-01` through the service that claims
//! the signing name both models declare.
//!
//! `shared_endpoint_api_versions.rs` pins WHICH model answers an action/version pair, row
//! by row. This suite pins what a client does with the answers: the ARNs the ELBv2 model
//! mints are the keys its later calls take, so a request served through this front door
//! has to be able to reach the record an earlier one created. That is the sequence an ECS
//! service-to-load-balancer binding runs before it can name a target group — the one the
//! issue's check could never start, because every `2015-12-01` envelope was refused
//! `InvalidAction` — and it cannot be a row table: every step after the first carries a
//! value the emulator generated in the one before it.
//!
//! The chain runs twice, over the two load-balancer types the model declares for a
//! forwarding listener, so what is pinned is the chaining rather than one shape of ARN.

use crate::wire::{call_at_version, emulator, find, kid_text, param, parse, Element, Wire};

/// The API version the ELBv2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// The API version THIS model declares, for the row that reads the Classic store.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// One chain: the case, the load balancer's name and `Type`, the ARN infix that type
/// gives a load balancer's and a listener's ARN, and the listener's protocol and port.
///
/// `application` and `network` are two of the three `LoadBalancerTypeEnum` values
/// (`smithy_query(query="shape",shape="LoadBalancerTypeEnum")`); `gateway` is left out
/// because a Gateway Load Balancer's listener takes neither protocol nor port.
type Chain = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str);

#[test]
fn an_elbv2_listener_chain_resolves_through_the_shared_endpoint() {
    let router = emulator();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");

    let v2 = |action: &str, params: &[(String, String)]| -> Wire {
        runtime.block_on(call_at_version(
            &router,
            REGION,
            action,
            Some(V2_VERSION),
            params,
        ))
    };

    let chains: [Chain; 2] = [
        (
            "an Application Load Balancer with an HTTP listener",
            "chain-alb",
            "application",
            "app",
            "HTTP",
            "80",
        ),
        (
            "a Network Load Balancer with a TCP listener",
            "chain-nlb",
            "network",
            "net",
            "TCP",
            "443",
        ),
    ];

    for (case, lb_name, lb_type, arn_infix, protocol, port) in chains {
        let tg_name = format!("{lb_name}-tg");

        let created = v2(
            "CreateLoadBalancer",
            &[
                param("Name", lb_name),
                param("Type", lb_type),
                param("Subnets.member.1", "subnet-0abc1234"),
                param("Subnets.member.2", "subnet-05ef6789"),
            ],
        );
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateLoadBalancer must succeed, got {} with {}",
            created.status, created.body
        );
        let load_balancer_arn = text_of(&created.body, "LoadBalancerArn");
        assert!(
            load_balancer_arn.contains(&format!(":loadbalancer/{arn_infix}/{lb_name}/")),
            "case {case:?}: expected a {lb_type} load-balancer ARN for {lb_name}, got \
             {load_balancer_arn:?} in {}",
            created.body
        );

        let group = v2(
            "CreateTargetGroup",
            &[
                param("Name", &tg_name),
                param("Protocol", protocol),
                param("Port", port),
                param("VpcId", "vpc-0abc1234"),
            ],
        );
        assert_eq!(
            group.status, 200,
            "case {case:?}: CreateTargetGroup must succeed, got {} with {}",
            group.status, group.body
        );
        let target_group_arn = text_of(&group.body, "TargetGroupArn");
        assert!(
            target_group_arn.contains(&format!(":targetgroup/{tg_name}/")),
            "case {case:?}: expected a target-group ARN for {tg_name}, got \
             {target_group_arn:?} in {}",
            group.body
        );

        // The step that ties the two together, and the one a chain can only reach with
        // both ARNs above: a default action forwarding to the target group just created.
        let listener = v2(
            "CreateListener",
            &[
                param("LoadBalancerArn", &load_balancer_arn),
                param("Protocol", protocol),
                param("Port", port),
                param("DefaultActions.member.1.Type", "forward"),
                param("DefaultActions.member.1.TargetGroupArn", &target_group_arn),
            ],
        );
        assert_eq!(
            listener.status, 200,
            "case {case:?}: CreateListener must succeed, got {} with {}",
            listener.status, listener.body
        );
        let listener_arn = text_of(&listener.body, "ListenerArn");
        assert!(
            listener_arn.contains(&format!(":listener/{arn_infix}/{lb_name}/")),
            "case {case:?}: expected a listener ARN under {lb_name}, got {listener_arn:?} \
             in {}",
            listener.body
        );

        // The read back, by the ARN the create minted: the listener the store holds for
        // this load balancer is the one created, forwarding to the group created.
        let listeners = v2(
            "DescribeListeners",
            &[param("LoadBalancerArn", &load_balancer_arn)],
        );
        assert_eq!(
            listeners.status, 200,
            "case {case:?}: DescribeListeners must read it back, got {} with {}",
            listeners.status, listeners.body
        );
        let document = parse(&listeners.body);
        let reported: Vec<String> = find(&document, "Listeners")
            .map(|list| {
                list.children()
                    .filter_map(|member| kid_text(member, "ListenerArn"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported,
            vec![listener_arn.clone()],
            "case {case:?}: expected exactly the created listener, got {reported:?} in {}",
            listeners.body
        );
        let forwards_to = find(&document, "TargetGroupArn").and_then(Element::text);
        assert_eq!(
            forwards_to,
            Some(target_group_arn.as_str()),
            "case {case:?}: expected the listener to forward to {target_group_arn:?}, got \
             {forwards_to:?} in {}",
            listeners.body
        );

        // The load balancer itself, read back by name through the same front door.
        let balancers = v2("DescribeLoadBalancers", &[param("Names.member.1", lb_name)]);
        assert_eq!(
            balancers.status, 200,
            "case {case:?}: DescribeLoadBalancers must read it back, got {} with {}",
            balancers.status, balancers.body
        );
        let document = parse(&balancers.body);
        let arns: Vec<String> = find(&document, "LoadBalancers")
            .map(|list| {
                list.children()
                    .filter_map(|member| kid_text(member, "LoadBalancerArn"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            arns,
            vec![load_balancer_arn.clone()],
            "case {case:?}: expected exactly the created load balancer, got {arns:?} in {}",
            balancers.body
        );
    }

    // None of the ELBv2 records above is in the Classic model's store, which the same
    // endpoint serves at its own API version.
    let classic = runtime.block_on(call_at_version(
        &router,
        REGION,
        "DescribeLoadBalancers",
        Some(CLASSIC_VERSION),
        &[],
    ));
    assert_eq!(
        classic.status, 200,
        "case \"the Classic store after the ELBv2 chains\": expected HTTP 200, got {} with {}",
        classic.status, classic.body
    );
    let document = parse(&classic.body);
    let classic_names: Vec<String> = find(&document, "LoadBalancerDescriptions")
        .map(|list| {
            list.children()
                .filter_map(|member| kid_text(member, "LoadBalancerName"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        classic_names.is_empty(),
        "case \"the Classic store after the ELBv2 chains\": the ELBv2 records must not \
         appear in the Classic API's answer, got {classic_names:?} in {}",
        classic.body
    );
}

/// The text of the first element named `tag` in an answer, or an empty string when the
/// answer carries none — which the caller's assertion then reports.
fn text_of(body: &str, tag: &str) -> String {
    let document = parse(body);
    find(&document, tag)
        .and_then(Element::text)
        .unwrap_or_default()
        .to_string()
}

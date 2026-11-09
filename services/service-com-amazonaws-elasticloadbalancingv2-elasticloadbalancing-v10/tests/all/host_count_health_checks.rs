// SPDX-License-Identifier: CC0-1.0

//! What one HEALTH CHECK of a registered target observes, and the host counts it is counted
//! into — driven over the wire exactly as a client drives it.
//!
//! The rule asserted here: **a load balancer CHECKS the registrations of every target group
//! it routes to, and a registration is reported healthy only while the emulator holds
//! something that stands behind it — an address a network interface of the account holds, a
//! running instance — while one it holds nothing behind is reported `unhealthy` with the
//! reason its check failed for; a registration nothing could be observed about keeps the
//! health the clock owes it. The counts those verdicts add up to are published as the
//! `HealthyHostCount` / `UnHealthyHostCount` series of the `(target group, load balancer)`
//! pair, and a target group no listener forwards to publishes neither.**
//!
//! The rows differ along what the rule generalises over: whether the account's inventory
//! holds something behind the registered address, how many registrations the group holds,
//! whether the inventory knows the group's network at all, and whether a listener forwards
//! to the group. The recorded configuration an issue happens to mention is a single row.
//!
//! The inventory is seeded through the EC2 store's own writer seam, and the series is read
//! out of the CloudWatch store those datapoints live in — the two stores this service
//! reaches on the shared engine — so what a row asserts is what another service's own
//! readers would answer. The series as a CLIENT reads it, through
//! `cloudwatch:GetMetricStatistics`, is asserted by `core/router/tests/all/`.

use std::time::{Duration, Instant};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::host_counts::{
    APPLICATION_NAMESPACE, HEALTHY_HOST_COUNT, LOAD_BALANCER_DIMENSION, TARGET_GROUP_DIMENSION,
    UNHEALTHY_HOST_COUNT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::{create_listener, create_load_balancer};
use crate::provisioning_loop::{action, call, emulator, first_text, members, p};

/// How long a row waits for the checks the emulator's accelerated schedule performs, and how
/// often it looks. The deadline is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(50);

/// The VPC the seeded inventory holds, and the subnet of it the interface sits in.
const VPC: &str = "vpc-0c0un70000000001";
const SUBNET: &str = "subnet-0c0un70000000001";
const SUBNET_CIDR: &str = "10.60.1.0/24";

/// A VPC id the seeded inventory holds NOTHING for, so nothing can be observed in it.
const UNKNOWN_VPC: &str = "vpc-0c0un70000000002";

/// The address the seeded network interface holds, and two the inventory holds nothing for.
const SERVED: &str = "10.60.1.10";
const DEAD: [&str; 2] = ["10.60.1.200", "10.60.1.201"];

/// Seed the account's EC2 inventory with one VPC, one subnet of it, and one network
/// interface holding [`SERVED`].
///
/// Written through the inventory's own writer seam (`insert_network_interface`), so the
/// interface is the record EC2's own readers answer with rather than a value this suite made
/// up.
fn seed_inventory(engine: &std::sync::Arc<engine::InMemoryEngine>) {
    let state = ec2::service_state(&**engine).expect("the shared engine holds an EC2 inventory");
    let mut store = state.lock();
    store.vpcs.insert(
        VPC.to_string(),
        ec2_types::Vpc {
            vpc_id: Some(VPC.to_string()),
            cidr_block: Some("10.60.0.0/16".to_string()),
            ..ec2_types::Vpc::default()
        },
    );
    store.subnets.insert(
        SUBNET.to_string(),
        ec2_types::Subnet {
            subnet_id: Some(SUBNET.to_string()),
            vpc_id: Some(VPC.to_string()),
            cidr_block: Some(SUBNET_CIDR.to_string()),
            availability_zone: Some("us-east-1a".to_string()),
            ..ec2_types::Subnet::default()
        },
    );
    store
        .insert_network_interface(ec2::NetworkInterfaceSpec {
            subnet_id: SUBNET.to_string(),
            vpc_id: VPC.to_string(),
            availability_zone: "us-east-1a".to_string(),
            region: "us-east-1".to_string(),
            description: "a workload of the account".to_string(),
            interface_type: ec2::NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
            private_ip_addresses: vec![SERVED.to_string()],
            ..ec2::NetworkInterfaceSpec::default()
        })
        .expect("the inventory holds the interface");
}

/// Create an `ip` target group in `vpc` and answer its ARN.
async fn ip_target_group(router: &MultiServiceRouter, name: &str, vpc: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", vpc));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup {name}: expected 200 from CreateTargetGroup, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("setup {name}: no TargetGroupArn in {}", reply.body))
}

/// Register every address in `addresses` with the target group `arn` names.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, addresses: &[&str]) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    for (index, address) in addresses.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), address));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// `(state, reason, description)` for every registration the group reports.
async fn health(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String, String)> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DescribeTargetHealth, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            (
                first_text(description, "State").unwrap_or_default(),
                first_text(description, "Reason").unwrap_or_default(),
                first_text(description, "Description").unwrap_or_default(),
            )
        })
        .collect()
}

/// The health of `arn`'s registrations, once every one of them reports `wanted`.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> Vec<(String, String, String)> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, case, arn).await;
        let settled = !reported.is_empty()
            && reported.iter().all(|(state, _, _)| state == wanted);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every registration to reach {wanted}, got {reported:?}"
            );
            return reported;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `<family>/<name>/<id>` suffix of a load balancer ARN, and the
/// `targetgroup/<name>/<id>` suffix of a target group's: how an Elastic Load Balancing
/// dimension names each of them.
fn dimension_value(arn: &str, resource: &str) -> String {
    let marker = format!(":{resource}/");
    let (_, suffix) = arn
        .split_once(marker.as_str())
        .unwrap_or_else(|| panic!("{arn} carries a {resource} resource field"));
    if resource == "loadbalancer" {
        suffix.to_string()
    } else {
        format!("{resource}/{suffix}")
    }
}

/// The LAST datapoint of one host-count series as the metric store holds it, or `None` when
/// that store holds no such series at all.
fn published(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    metric: &str,
    target_group: &str,
    load_balancer: &str,
) -> Option<f64> {
    let state =
        cloudwatch::service_state(&**engine).expect("the shared engine holds a metric store");
    let store = state.lock();
    let key = cloudwatch::MetricKey {
        namespace: APPLICATION_NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions: [
            (
                TARGET_GROUP_DIMENSION.to_string(),
                dimension_value(target_group, "targetgroup"),
            ),
            (
                LOAD_BALANCER_DIMENSION.to_string(),
                dimension_value(load_balancer, "loadbalancer"),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let series = store.metrics.get(&key)?;
    series.last().map(|datum| datum.maximum)
}

/// The last datapoint of one host-count series, once it reports `wanted`.
///
/// A series is published on the emulator's own accelerated schedule, so the datapoint that
/// carries a verdict the checks have just reached is the next one due rather than the one
/// already stored.
async fn published_reaching(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    metric: &str,
    target_group: &str,
    load_balancer: &str,
    wanted: f64,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = published(engine, metric, target_group, load_balancer);
        if found == Some(wanted) || Instant::now() >= deadline {
            assert_eq!(
                found,
                Some(wanted),
                "case {case:?}: expected the {metric} series to reach {wanted}, got {found:?}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// One row: a target group to build, what the inventory holds behind its registrations, and
/// the verdict and counts that implies.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The VPC the group names, which is the inventory a check of it reads.
    vpc: &'static str,
    /// The addresses registered with it.
    addresses: &'static [&'static str],
    /// Whether a listener of the load balancer forwards to it.
    routed: bool,
    /// The `TargetHealth$State` every one of its registrations must reach.
    state: &'static str,
    /// The reason that state is reported with, or `""` for a state reported without one.
    reason: &'static str,
    /// The counts its series must carry, for a routed group.
    healthy: f64,
    unhealthy: f64,
}

#[tokio::test]
async fn a_registration_is_healthy_only_while_the_emulator_holds_something_behind_it() {
    let (router, engine) = emulator();
    seed_inventory(&engine);

    let rows: [Row; 4] = [
        Row {
            case: "an address a network interface of the account holds",
            name: "probe-served",
            vpc: VPC,
            addresses: &[SERVED],
            routed: true,
            state: "healthy",
            reason: "",
            healthy: 1.0,
            unhealthy: 0.0,
        },
        Row {
            case: "the recorded configuration: two addresses the inventory holds nothing for",
            name: "probe-dead",
            vpc: VPC,
            addresses: &DEAD,
            routed: true,
            state: "unhealthy",
            reason: "Target.Timeout",
            healthy: 0.0,
            unhealthy: 2.0,
        },
        Row {
            case: "a group whose network the inventory knows nothing about",
            name: "probe-unknown-vpc",
            vpc: UNKNOWN_VPC,
            addresses: &[DEAD[0]],
            routed: true,
            // Nothing was observed about the registration, so it keeps the health the clock
            // owes it rather than being failed on silence.
            state: "healthy",
            reason: "",
            healthy: 1.0,
            unhealthy: 0.0,
        },
        Row {
            case: "a group no listener forwards to",
            name: "probe-unrouted",
            vpc: VPC,
            addresses: &DEAD,
            routed: false,
            state: "unused",
            reason: "Target.NotInUse",
            healthy: 0.0,
            unhealthy: 0.0,
        },
    ];

    let load_balancer = create_load_balancer(&router, "probe-alb").await;
    let mut groups: Vec<String> = Vec::new();
    for row in &rows {
        let group = ip_target_group(&router, row.name, row.vpc).await;
        register(&router, row.case, &group, row.addresses).await;
        groups.push(group);
    }
    // One listener per routed group, each forwarding to it: what makes the group one the
    // load balancer checks.
    let mut port = 80;
    for (group, row) in groups.iter().zip(&rows) {
        if row.routed {
            create_listener(&router, &load_balancer, group, port).await;
            port += 1;
        }
    }

    for (group, row) in groups.iter().zip(&rows) {
        let reported = health_reaching(&router, row.case, group, row.state).await;
        assert_eq!(
            reported.len(),
            row.addresses.len(),
            "case {:?}: expected one description per registration, got {reported:?}",
            row.case
        );
        for (state, reason, description) in &reported {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                (row.state, row.reason),
                "case {:?}: expected {}/{} , got {state}/{reason}",
                row.case,
                row.state,
                row.reason
            );
            // "A reason code is not provided" for a healthy target, and a non-healthy state
            // carries the detail its own reason is described by.
            assert_eq!(
                description.is_empty(),
                row.reason.is_empty(),
                "case {:?}: a state reported with a reason carries a description, got \
                 {state}/{reason}/{description:?}",
                row.case
            );
        }

        for (metric, expected) in [
            (HEALTHY_HOST_COUNT, row.healthy),
            (UNHEALTHY_HOST_COUNT, row.unhealthy),
        ] {
            if row.routed {
                published_reaching(
                    &engine,
                    row.case,
                    metric,
                    group,
                    &load_balancer,
                    expected,
                )
                .await;
                continue;
            }
            // A target group no listener forwards to is checked by no load balancer, so it
            // publishes no series at all under this one's dimension — not even a zero.
            let found = published(&engine, metric, group, &load_balancer);
            assert_eq!(
                found, None,
                "case {:?}: expected no {metric} datapoint, got {found:?}",
                row.case
            );
        }
    }

    // The counts are the group's OWN: a deregistration that empties a routed group takes
    // both of its counts to zero, at the instant the clock owes the removal.
    let dead_group = groups[1].clone();
    let mut form = action("DeregisterTargets");
    form.push(p("TargetGroupArn", dead_group.as_str()));
    for (index, address) in DEAD.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), address));
    }
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case \"a deregistration of every registration\": expected 200, got {} body {}",
        reply.status, reply.body
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        let counts = published(&engine, UNHEALTHY_HOST_COUNT, &dead_group, &load_balancer);
        if counts == Some(0.0) || Instant::now() >= deadline {
            assert_eq!(
                counts,
                Some(0.0),
                "case \"a deregistration of every registration\": expected the \
                 {UNHEALTHY_HOST_COUNT} series to reach 0, got {counts:?}"
            );
            break;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

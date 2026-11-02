// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal a `loadBalancers` entry earns when the ELBv2 target group it
//! names carries a target type the service's task-definition revision does not register its
//! tasks as.
//!
//! The sibling suite `all/load_balancer_target_groups.rs` pins WHICH pairings are refused
//! and that nothing is stored when one is; this suite pins the TEXT of that refusal, over
//! the whole class of pairings rather than one request:
//!
//! > The target-group/network-mode incompatibility is refused `400
//! > InvalidParameterException` whose message is ONE sentence — "The provided target group
//! > <targetGroupArn> has target type <TargetType>, which is incompatible with the
//! > <networkMode> network mode specified in the task definition." — whose only variable
//! > parts are the target group the entry named, the target type that group carries in the
//! > Elastic Load Balancing v2 store, and the network mode the revision declares. Nothing
//! > is appended to it, on whichever operation and in whichever position the entry names
//! > the group.
//!
//! `InvalidParameterException` is the error `CreateService` and `UpdateService` model for a
//! parameter that is not valid for the state the request names (`smithy_query(query =
//! "operation", operation = "CreateService,UpdateService")`), and the pairing the message
//! reports is `LoadBalancer$targetGroupArn`'s own rule, quoted in the sibling suite's
//! header.
//!
//! The target type is not a value the request carries, so every row here creates its target
//! group through the ELBv2 API on the shared engine, exactly as that suite does — its
//! harness is reused rather than rebuilt.

use serde_json::{json, Value};

use crate::ecs_wire::{ok, refusal};
use crate::load_balancer_target_groups::{
    attachment, create_body, create_target_group, emulator, register_definition, service_exists,
};

/// The sentence Amazon ECS refuses an incompatible pairing with, composed from the request
/// (the target group the entry named), the Elastic Load Balancing v2 record (its target
/// type) and the registered revision (its network mode).
fn pairing_message(target_group: &str, target_type: &str, network_mode: &str) -> String {
    format!(
        "The provided target group {target_group} has target type {target_type}, which is \
         incompatible with the {network_mode} network mode specified in the task definition."
    )
}

/// One row of the wording table: a network mode the revision declares and a target type the
/// group carries which that mode does not register its tasks as.
struct Row {
    case: &'static str,
    network_mode: &'static str,
    target_type: &'static str,
}

/// The incompatible pairings, spanning both halves of the rule and every target type ECS
/// never registers a task as.
///
/// The network modes are the ECS model's `NetworkMode` enum (`bridge`, `host`, `awsvpc`,
/// `none`) and the target types are the ELBv2 model's `TargetTypeEnum` (`instance`, `ip`,
/// `lambda`, `alb`) — the catalogs the sibling suite's header cites.
const ROWS: [Row; 8] = [
    Row {
        // The issue's own pairing.
        case: "awsvpc with an instance target group",
        network_mode: "awsvpc",
        target_type: "instance",
    },
    Row {
        case: "awsvpc with a lambda target group",
        network_mode: "awsvpc",
        target_type: "lambda",
    },
    Row {
        case: "awsvpc with an alb target group",
        network_mode: "awsvpc",
        target_type: "alb",
    },
    Row {
        case: "bridge with an ip target group",
        network_mode: "bridge",
        target_type: "ip",
    },
    Row {
        case: "bridge with a lambda target group",
        network_mode: "bridge",
        target_type: "lambda",
    },
    Row {
        case: "host with an ip target group",
        network_mode: "host",
        target_type: "ip",
    },
    Row {
        case: "host with an alb target group",
        network_mode: "host",
        target_type: "alb",
    },
    Row {
        case: "none with an ip target group",
        network_mode: "none",
        target_type: "ip",
    },
];

#[tokio::test]
async fn create_service_words_the_pairing_refusal_in_one_sentence() {
    let router = emulator();
    let cluster = "pair-wording";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster every row creates its service in",
    )
    .await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let target_group = create_target_group(
            &router,
            &format!("wording-tg-{index}"),
            row.target_type,
            case,
        )
        .await;
        let definition = register_definition(
            &router,
            &format!("wording-family-{index}"),
            row.network_mode,
            case,
        )
        .await;
        let service_name = format!("wording-svc-{index}");
        let (code, message) = refusal(
            &router,
            "CreateService",
            create_body(
                cluster,
                &service_name,
                &definition,
                row.network_mode,
                attachment(&target_group),
            ),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled InvalidParameterException, got {code:?} \
             with {message:?}"
        );
        let expected = pairing_message(&target_group, row.target_type, row.network_mode);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !service_exists(&router, cluster, &service_name, case).await,
            "case {case:?}: the refused service must not be persisted, but DescribeServices \
             reports {service_name}"
        );
    }
}

#[tokio::test]
async fn every_path_to_the_pairing_refusal_words_it_the_same_way() {
    let router = emulator();
    let cluster = "pair-wording-paths";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    let instance_group =
        create_target_group(&router, "paths-instance-tg", "instance", "the instance group").await;
    let ip_group = create_target_group(&router, "paths-ip-tg", "ip", "the ip group").await;
    let bridge = register_definition(&router, "paths-bridge", "bridge", "the bridge revision").await;
    let awsvpc = register_definition(&router, "paths-awsvpc", "awsvpc", "the awsvpc revision").await;

    // The service the updates below act on: a bridge revision on the instance group, the
    // pairing the rule allows.
    let service_name = "paths-svc";
    ok(
        &router,
        "CreateService",
        create_body(
            cluster,
            service_name,
            &bridge,
            "bridge",
            attachment(&instance_group),
        ),
        "the service the updates act on",
    )
    .await;

    /// One row: the case, the request the operation is called with, and which target group
    /// and pairing the refusal must report.
    type Path = (&'static str, &'static str, Value, String, &'static str, &'static str);
    let paths: [Path; 4] = [
        (
            "an update that moves the service onto an awsvpc revision, keeping its instance \
             target group",
            "UpdateService",
            // The network configuration an `awsvpc` revision requires comes with it, so what
            // this row is refused for is the pairing and not the missing configuration.
            json!({
                "taskDefinition": awsvpc.clone(),
                "networkConfiguration": {
                    "awsvpcConfiguration": { "subnets": ["subnet-0abc1234ecs0lb"] },
                },
            }),
            instance_group.clone(),
            "instance",
            "awsvpc",
        ),
        (
            "an update that attaches an ip target group to a service still running a bridge \
             revision",
            "UpdateService",
            json!({ "loadBalancers": attachment(&ip_group) }),
            ip_group.clone(),
            "ip",
            "bridge",
        ),
        (
            "a create whose alternate target group is the incompatible one",
            "CreateService",
            json!({
                "loadBalancers": [{
                    "targetGroupArn": ip_group.clone(),
                    "containerName": "web",
                    "containerPort": 80,
                    "advancedConfiguration": { "alternateTargetGroupArn": instance_group.clone() },
                }],
            }),
            instance_group.clone(),
            "instance",
            "awsvpc",
        ),
        (
            "an update whose alternate target group is the incompatible one",
            "UpdateService",
            json!({
                "loadBalancers": [{
                    "targetGroupArn": instance_group.clone(),
                    "containerName": "web",
                    "containerPort": 80,
                    "advancedConfiguration": { "alternateTargetGroupArn": ip_group.clone() },
                }],
            }),
            ip_group.clone(),
            "ip",
            "bridge",
        ),
    ];

    for (index, (case, operation, fields, reported, target_type, network_mode)) in
        paths.iter().enumerate()
    {
        let mut request = fields.clone();
        request["cluster"] = json!(cluster);
        if *operation == "CreateService" {
            // A create needs its own name and revision; the awsvpc one, so the alternate
            // group's `instance` type is what it is refused for.
            request["serviceName"] = json!(format!("paths-create-{index}"));
            request["taskDefinition"] = json!(awsvpc.clone());
            request["desiredCount"] = json!(1);
            request["networkConfiguration"] = json!({
                "awsvpcConfiguration": { "subnets": ["subnet-0abc1234ecs0lb"] },
            });
        } else {
            request["service"] = json!(service_name);
        }
        let (code, message) = refusal(&router, operation, request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled InvalidParameterException, got {code:?} \
             with {message:?}"
        );
        let expected = pairing_message(reported, target_type, network_mode);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

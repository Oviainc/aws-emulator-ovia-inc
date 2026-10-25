// SPDX-License-Identifier: CC0-1.0

//! The three CROSS-FIELD load-balancer rules `CreateService` holds a request to, each
//! keyed on something the request itself cannot carry — another member of the same input,
//! the REGISTERED task-definition revision, or the Elastic Load Balancing v2 store — so
//! none of them can be expressed in the Smithy model and all of them are the handler's to
//! enforce. In every case AWS answers `400 InvalidParameterException` and creates NOTHING.
//!
//! The rules under test, each stated over its whole class rather than over one request:
//!
//! 1. A service whose `deploymentController.type` is `CODE_DEPLOY` shifts traffic between
//!    target groups, so it must carry at least one load-balancer binding; the same request
//!    under any other controller is legal. "If the service uses the CODE_DEPLOY deployment
//!    controller, the service is required to use either an …" (`CreateService$loadBalancers`,
//!    `smithy_query(query="doc",shape="CreateServiceRequest",member="loadBalancers")`).
//! 2. Each `loadBalancers` entry attaches ONE container port of the task to a target group,
//!    so the `containerName` must be a container of the revision the `taskDefinition`
//!    reference RESOLVES to and the `containerPort` one that container declares in its
//!    `portMappings`.
//! 3. A service registers its tasks as targets of the group it binds, and only a load
//!    balancer can send traffic to a group, so every group named must be one some load
//!    balancer forwards to — read LIVE from the ELBv2 store, so the same ARN is refused
//!    before the association exists and accepted after.
//!
//! What each suite spans, beyond the one request the rule was found on:
//!
//! * rule 1: the FORM the `loadBalancers` member takes (absent, an explicitly empty list,
//!   one binding, two bindings) crossed with the controller;
//! * rule 2: the FORM of the `taskDefinition` reference the binding is judged against
//!   (family alone, `family:revision` of either revision, a full ARN) over a family whose
//!   two revisions declare DIFFERENT containers and ports, plus the POSITION of the
//!   offending binding in a multi-entry list;
//! * rule 3: the PATH by which a load balancer comes to forward to the group (a listener's
//!   default action, a rule on a listener, a Network Load Balancer's listener) crossed with
//!   the target-type/network-mode pairing, plus a list mixing an associated group with an
//!   orphan one in either order.
//!
//! Everything is driven end to end: the ECS calls through the ECS front door
//! ([`crate::ecs_wire`]) and the Elastic Load Balancing v2 calls through that service's own
//! awsQuery API on the SAME engine ([`crate::load_balancer_target_groups`]), so no suite
//! here reaches into a state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, string};
use crate::load_balancer_target_groups::{
    create_target_group, elb_call, emulator, first_text, VPC,
};

/// The cluster every service in this suite is created in.
const CLUSTER: &str = "crossfield";

/// The subnets every load balancer below is created across, as the sibling harness uses.
const SUBNETS: [&str; 2] = ["subnet-0abc1234ecs0lb0", "subnet-0abc1234ecs0lb1"];

/// AWS's sentence for a `CODE_DEPLOY` service carrying no load-balancer binding.
const CODE_DEPLOY_REQUIRES_A_LOAD_BALANCER: &str =
    "When specifying the CODE_DEPLOY deployment controller type, the service requires an \
     Application Load Balancer or Network Load Balancer.";

/// Register one revision of `family` with `containers` and return its ARN.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    containers: Value,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": containers,
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// One container definition declaring `ports` as its container ports.
fn container(name: &str, ports: &[i64]) -> Value {
    json!({
        "name": name,
        "image": "public.ecr.aws/nginx/nginx:latest",
        "memory": 128,
        "portMappings": ports
            .iter()
            .map(|port| json!({ "containerPort": port, "protocol": "tcp" }))
            .collect::<Vec<_>>(),
    })
}

/// One `loadBalancers` entry.
fn binding(target_group: &str, container_name: &str, container_port: i64) -> Value {
    json!({
        "targetGroupArn": target_group,
        "containerName": container_name,
        "containerPort": container_port,
    })
}

/// Whether `ListServices` reports the service — what proves a refused create stored
/// nothing, and an accepted one stored something.
async fn is_listed(router: &MultiServiceRouter, name: &str, case: &str) -> bool {
    let body = ok(router, "ListServices", json!({ "cluster": CLUSTER }), case).await;
    list(&body, "serviceArns", case).iter().any(|arn| {
        arn.as_str()
            .is_some_and(|arn| arn.ends_with(&format!("/{name}")))
    })
}

/// The `loadBalancers` the stored service holds, read back through `DescribeServices`: the
/// round trip an accepted binding has to make.
async fn stored_bindings(router: &MultiServiceRouter, name: &str, case: &str) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected DescribeServices to report the one service, got {body}"
    );
    list(&services[0], "loadBalancers", case)
}

/// Create one target group of `target_type` speaking `protocol` through the ELBv2 API, with
/// NO load balancer forwarding to it, and return its ARN.
///
/// The sibling harness's `create_unattached_target_group` fixes `Protocol=HTTP`; a group
/// behind a Network Load Balancer speaks `TCP`, which is the axis the orphan suite below
/// spans, so this one takes the protocol as a parameter.
async fn unattached_group(
    router: &MultiServiceRouter,
    name: &str,
    target_type: &str,
    protocol: &str,
    case: &str,
) -> String {
    let (status, body) = elb_call(
        router,
        "CreateTargetGroup",
        &[
            ("Name", name),
            ("TargetType", target_type),
            ("Protocol", protocol),
            ("Port", "80"),
            ("VpcId", VPC),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTargetGroup must succeed, got {status} with {body}"
    );
    let arn = first_text(&body, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {body}"));
    assert_eq!(
        load_balancers_of(router, &arn, case).await,
        "",
        "case {case:?}: a group with nothing forwarding to it reports no load balancer"
    );
    arn
}

/// What ELBv2's own `DescribeTargetGroups` reports as the group's `LoadBalancerArns` — the
/// association the ECS rule reads, read through the same API a client would.
async fn load_balancers_of(router: &MultiServiceRouter, target_group: &str, case: &str) -> String {
    let (status, body) = elb_call(
        router,
        "DescribeTargetGroups",
        &[("TargetGroupArns.member.1", target_group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetGroups must read the group back, got {status} with {body}"
    );
    first_text(&body, "LoadBalancerArns").unwrap_or_default()
}

/// Create a load balancer of `lb_type` and return its ARN.
async fn create_load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    lb_type: &str,
    case: &str,
) -> String {
    let (status, body) = elb_call(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", name),
            ("Type", lb_type),
            ("Subnets.member.1", SUBNETS[0]),
            ("Subnets.member.2", SUBNETS[1]),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLoadBalancer must succeed, got {status} with {body}"
    );
    first_text(&body, "LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {case:?}: no LoadBalancerArn in {body}"))
}

/// Create a listener on `load_balancer` whose default action forwards to `target_group`,
/// and return the listener's ARN.
async fn create_listener(
    router: &MultiServiceRouter,
    load_balancer: &str,
    protocol: &str,
    target_group: &str,
    case: &str,
) -> String {
    let (status, body) = elb_call(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", load_balancer),
            ("Protocol", protocol),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", target_group),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateListener must succeed, got {status} with {body}"
    );
    first_text(&body, "ListenerArn")
        .unwrap_or_else(|| panic!("case {case:?}: no ListenerArn in {body}"))
}

/// Create a rule on `listener` forwarding to `target_group`.
async fn create_rule(
    router: &MultiServiceRouter,
    listener: &str,
    target_group: &str,
    case: &str,
) {
    let (status, body) = elb_call(
        router,
        "CreateRule",
        &[
            ("ListenerArn", listener),
            ("Priority", "10"),
            ("Conditions.member.1.Field", "path-pattern"),
            ("Conditions.member.1.Values.member.1", "/api/*"),
            ("Actions.member.1.Type", "forward"),
            ("Actions.member.1.TargetGroupArn", target_group),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRule must succeed, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_code_deploy_service_requires_a_binding_however_the_member_is_written() {
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    let family = "codedeployed";
    register(
        &router,
        family,
        "bridge",
        json!([container("app", &[80])]),
        "setup",
    )
    .await;
    // Two groups a load balancer forwards to: the member's own doc puts more than one
    // target group under the ECS controller — "If the service uses the ECS deployment
    // controller and using either an Application Load Balancer or Network Load Balancer,
    // you must specify one or more target group ARNs to attach to the service" — so the
    // two-binding row below is an ECS-controller row.
    let first = create_target_group(&router, "cd-first-tg", "instance", "setup").await;
    let second = create_target_group(&router, "cd-second-tg", "instance", "setup").await;

    /// One row: the case, the `deploymentController.type` (absent when `None`), how many
    /// bindings the `loadBalancers` member carries (absent when `None`, so an explicitly
    /// empty list and no member at all are different rows), and whether AWS accepts it.
    type Row = (&'static str, Option<&'static str>, Option<usize>, bool);
    let rows: [Row; 7] = [
        ("CODE_DEPLOY with no loadBalancers member", Some("CODE_DEPLOY"), None, false),
        ("CODE_DEPLOY with an explicitly empty loadBalancers list", Some("CODE_DEPLOY"), Some(0), false),
        ("CODE_DEPLOY with one binding", Some("CODE_DEPLOY"), Some(1), true),
        ("the ECS controller with two bindings", Some("ECS"), Some(2), true),
        ("the ECS controller with no loadBalancers member", Some("ECS"), None, true),
        ("the ECS controller with an explicitly empty list", Some("ECS"), Some(0), true),
        ("no deploymentController at all with an empty list", None, Some(0), true),
    ];

    for (index, (case, controller, bindings, allowed)) in rows.iter().enumerate() {
        let service_name = format!("cdservice{index}");
        let mut request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": family,
            "desiredCount": 0,
        });
        if let Some(controller) = controller {
            request["deploymentController"] = json!({ "type": controller });
        }
        if let Some(count) = bindings {
            let entries: Vec<Value> = [first.as_str(), second.as_str()][..*count]
                .iter()
                .map(|group| binding(group, "app", 80))
                .collect();
            request["loadBalancers"] = json!(entries);
        }

        if *allowed {
            let created = ok(&router, "CreateService", request, case).await;
            assert_eq!(
                list(&created["service"], "loadBalancers", case).len(),
                bindings.unwrap_or_default(),
                "case {case:?}: the accepted service must hold every binding it asked \
                 for, got {created}"
            );
            assert!(
                is_listed(&router, &service_name, case).await,
                "case {case:?}: an accepted create must store the service"
            );
            continue;
        }

        let (code, message) = refusal(&router, "CreateService", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message, CODE_DEPLOY_REQUIRES_A_LOAD_BALANCER,
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );
    }
}

#[tokio::test]
async fn a_binding_is_judged_against_the_revision_the_reference_resolves_to() {
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    // Two revisions of ONE family declaring DIFFERENT containers and ports, so which
    // revision the reference resolves to decides every row's verdict.
    let family = "revisioned";
    let revision_one = register(
        &router,
        family,
        "bridge",
        json!([container("app", &[80, 8443])]),
        "setup",
    )
    .await;
    register(
        &router,
        family,
        "bridge",
        json!([container("api", &[443])]),
        "setup",
    )
    .await;
    let group = create_target_group(&router, "revisioned-tg", "instance", "setup").await;

    let latest = family.to_string();
    let first = format!("{family}:1");
    let second = format!("{family}:2");

    /// One row: the case, the `taskDefinition` reference, the binding's container and
    /// port, and the refusal AWS answers (`None` when the pair resolves).
    type Row<'a> = (&'static str, &'a str, &'static str, i64, Option<&'static str>);
    let rows: [Row; 7] = [
        // "If a revision isn't specified, the latest ACTIVE revision is used."
        ("the family alone, with the latest revision's container and port", &latest, "api", 443, None),
        (
            "the family alone, with the PREVIOUS revision's container",
            &latest,
            "app",
            80,
            Some("The container app does not exist in the task definition."),
        ),
        ("family:revision naming revision 1", &first, "app", 80, None),
        ("family:revision naming revision 1's second port", &first, "app", 8443, None),
        ("the full ARN of revision 1", &revision_one, "app", 80, None),
        (
            "family:revision naming revision 2, with revision 1's port",
            &second,
            "api",
            80,
            Some("The container api did not have a container port 80 defined."),
        ),
        (
            "a container no revision of the family declares",
            &first,
            "sidecar",
            80,
            Some("The container sidecar does not exist in the task definition."),
        ),
    ];

    for (index, (case, reference, container_name, port, refused)) in rows.iter().enumerate() {
        let service_name = format!("revservice{index}");
        let request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": reference,
            "desiredCount": 0,
            "loadBalancers": [binding(&group, container_name, *port)],
        });

        match refused {
            None => {
                ok(&router, "CreateService", request, case).await;
                assert_eq!(
                    stored_bindings(&router, &service_name, case).await,
                    vec![binding(&group, container_name, *port)],
                    "case {case:?}: the accepted binding must round-trip through \
                     DescribeServices"
                );
            }
            Some(expected) => {
                let (code, message) = refusal(&router, "CreateService", request, case).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case:?}: expected an InvalidParameterException, got {code:?} \
                     with {message:?}"
                );
                assert_eq!(
                    message, *expected,
                    "case {case:?}: unexpected refusal message"
                );
                assert!(
                    !is_listed(&router, &service_name, case).await,
                    "case {case:?}: a refused create must store no service"
                );
            }
        }
    }

    // The whole LIST is judged, not just its head: the offending entry is refused wherever
    // it sits, and is the one the message names.
    for (position, case) in [
        (0usize, "the FIRST of two bindings naming an undeclared port"),
        (1usize, "the SECOND of two bindings naming an undeclared port"),
    ] {
        let service_name = format!("revlist{position}");
        let mut entries = vec![binding(&group, "app", 80), binding(&group, "app", 80)];
        entries[position] = binding(&group, "app", 9999);
        let (code, message) = refusal(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": first,
                "desiredCount": 0,
                "loadBalancers": entries,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message, "The container app did not have a container port 9999 defined.",
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );
    }
}

#[tokio::test]
async fn a_bound_target_group_needs_a_live_load_balancer_association() {
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    // One revision per network mode the rows below pair a target type with: an `awsvpc`
    // task is registered by the IP of its own interface (`ip`), every other mode by the
    // container instance that runs it (`instance`).
    for (family, mode) in [("orphanbridge", "bridge"), ("orphanvpc", "awsvpc")] {
        register(
            &router,
            family,
            mode,
            json!([container("app", &[80])]),
            "setup",
        )
        .await;
    }

    /// How a load balancer comes to forward to the group, which is the axis these rows
    /// span: the rule is "some load balancer forwards to it", not "a listener was created".
    #[derive(Debug, Clone, Copy)]
    enum Path {
        /// A listener whose own default action forwards to the group.
        ListenerDefaultAction,
        /// A rule on a listener that default-forwards somewhere else.
        ListenerRule,
    }

    /// One row: the case, how the association is made, the load-balancer type, the
    /// listener protocol, the target type, the revision's network mode and its family.
    type Row = (
        &'static str,
        Path,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    );
    let rows: [Row; 3] = [
        (
            "an Application Load Balancer listener's default action",
            Path::ListenerDefaultAction,
            "application",
            "HTTP",
            "instance",
            "bridge",
            "orphanbridge",
        ),
        (
            "a rule on an Application Load Balancer listener",
            Path::ListenerRule,
            "application",
            "HTTP",
            "instance",
            "bridge",
            "orphanbridge",
        ),
        (
            "a Network Load Balancer listener's default action",
            Path::ListenerDefaultAction,
            "network",
            "TCP",
            "ip",
            "awsvpc",
            "orphanvpc",
        ),
    ];

    for (index, (case, path, lb_type, protocol, target_type, mode, family)) in
        rows.iter().enumerate()
    {
        let group = unattached_group(
            &router,
            &format!("orphan-tg-{index}"),
            target_type,
            protocol,
            case,
        )
        .await;
        let service_name = format!("orphanservice{index}");
        let mut request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": family,
            "desiredCount": 0,
            "loadBalancers": [binding(&group, "app", 80)],
        });
        if *mode == "awsvpc" {
            request["networkConfiguration"] = awsvpc_network_configuration();
        }

        // Nothing forwards to the group yet, so the tasks it would register could never be
        // reached: refused, naming the ARN the request carried.
        let (code, message) = refusal(&router, "CreateService", request.clone(), case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "The target group with targetGroupArn {group} does not have an associated \
                 load balancer."
            ),
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );

        // Now make the association this row is about, and confirm ELBv2 reports it.
        let load_balancer =
            create_load_balancer(&router, &format!("orphan-lb-{index}"), lb_type, case).await;
        match path {
            Path::ListenerDefaultAction => {
                create_listener(&router, &load_balancer, protocol, &group, case).await;
            }
            Path::ListenerRule => {
                // The listener default-forwards to ANOTHER group, so only the rule makes
                // this one reachable.
                let elsewhere = unattached_group(
                    &router,
                    &format!("orphan-other-{index}"),
                    target_type,
                    protocol,
                    case,
                )
                .await;
                let listener =
                    create_listener(&router, &load_balancer, protocol, &elsewhere, case).await;
                create_rule(&router, &listener, &group, case).await;
            }
        }
        let reported = load_balancers_of(&router, &group, case).await;
        assert!(
            reported.contains(&load_balancer),
            "case {case:?}: ELBv2 must report the association {path:?} made, got \
             {reported:?}"
        );

        // The IDENTICAL request is now accepted — the resolution is live, not a property
        // of the ARN.
        ok(&router, "CreateService", request, case).await;
        assert_eq!(
            stored_bindings(&router, &service_name, case).await,
            vec![binding(&group, "app", 80)],
            "case {case:?}: the accepted binding must round-trip through DescribeServices"
        );
    }

    // A list mixing an associated group with an orphan one is refused on the ORPHAN,
    // wherever it sits in the list.
    let associated = create_target_group(&router, "mixed-attached-tg", "instance", "mixed").await;
    for (position, case) in [
        (0usize, "the orphan group FIRST in the list"),
        (1usize, "the orphan group SECOND in the list"),
    ] {
        let orphan = unattached_group(
            &router,
            &format!("mixed-orphan-{position}"),
            "instance",
            "HTTP",
            case,
        )
        .await;
        let mut entries = vec![
            binding(&associated, "app", 80),
            binding(&associated, "app", 80),
        ];
        entries[position] = binding(&orphan, "app", 80);
        let service_name = format!("mixedservice{position}");
        let (code, message) = refusal(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": "orphanbridge",
                "desiredCount": 0,
                "loadBalancers": entries,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "The target group with targetGroupArn {orphan} does not have an associated \
                 load balancer."
            ),
            "case {case:?}: unexpected refusal message"
        );
        assert!(
            !is_listed(&router, &service_name, case).await,
            "case {case:?}: a refused create must store no service"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The CROSS-FIELD rules an ECS service or task launch is held to, driven end to end over
//! the shared wire harness in [`crate::ecs_wire`].
//!
//! Each rule below is keyed on the REGISTERED task definition the launch runs — state an
//! earlier call wrote — or on another member of the same request, so none of them can be
//! expressed in the Smithy model and all of them are the handler's to enforce. In every
//! case AWS refuses with `InvalidParameterException` (HTTP 400) and creates NOTHING, so
//! each table asserts both the refusal and that the store is unchanged.
//!
//! The rules under test:
//!
//! * a launch carries a `networkConfiguration` exactly when the revision's `networkMode`
//!   is `awsvpc` — required for it (the task gets its own elastic network interface) and
//!   refused for every other mode (the task has no interface of its own to configure);
//! * a launch that enables ECS Exec requires the revision to declare a `taskRoleArn`,
//!   which is the identity the session signs with;
//! * a `CODE_DEPLOY`-controlled service requires a load-balancer binding;
//! * a `loadBalancers` binding may only name a container the revision declares, and a
//!   container port that container declares in its `portMappings`.
//!
//! The rows span the operations that launch tasks (`CreateService`, `RunTask`,
//! `UpdateService`) and, within each rule, the inputs the rule generalises over: every
//! value of the closed `NetworkMode` enum, both ways a port can be declared, and both
//! halves of a binding.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, create_cluster, list, ok, refusal,
    refused, service, string, ACCOUNT,
};

/// The cluster every case in this suite acts in.
const CLUSTER: &str = "validated";

/// Register one revision and return its ARN. `members` are the revision's members beside
/// `family` and `containerDefinitions`, and `containers` its container definitions.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    members: Value,
    containers: Value,
    case: &str,
) -> String {
    let mut request = json!({ "family": family, "containerDefinitions": containers });
    for (member, value) in members.as_object().expect("the members are an object") {
        request[member] = value.clone();
    }
    let body = ok(router, "RegisterTaskDefinition", request, case).await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// A container definition with no port mapping at all.
fn plain_container(name: &str) -> Value {
    json!({ "name": name, "image": "public.ecr.aws/nginx/nginx:latest", "memory": 128 })
}

/// The service names a cluster holds, read back through `DescribeServices`: what proves a
/// refused create stored nothing.
async fn stored_services(router: &MultiServiceRouter, names: &[&str], case: &str) -> Vec<String> {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": names }),
        case,
    )
    .await;
    list(&body, "services", case)
        .iter()
        .map(|service| string(service, "serviceName", case))
        .collect()
}

/// How many tasks the cluster holds, read back through `ListTasks`: what proves a refused
/// `RunTask` ran nothing.
async fn task_count(router: &MultiServiceRouter, case: &str) -> usize {
    tasks_in(router, CLUSTER, case).await
}

/// How many tasks ONE named cluster holds.
async fn tasks_in(router: &MultiServiceRouter, cluster: &str, case: &str) -> usize {
    let body = ok(router, "ListTasks", json!({ "cluster": cluster }), case).await;
    list(&body, "taskArns", case).len()
}

#[tokio::test]
async fn a_launch_carries_a_network_configuration_exactly_when_the_mode_is_awsvpc() {
    let router = service();
    // One container instance registered, so a task placed on the EC2 capacity the
    // cluster holds has somewhere to run; what these cases are about is what happens
    // before that placement.
    cluster_with_capacity(&router, CLUSTER, "setup").await;

    // Every value of the closed `NetworkMode` enum (`smithy_query(query="shape",
    // shape="NetworkMode")`: awsvpc, bridge, host, none) plus the documented default a
    // revision that names no mode takes, so the rule is asserted over the whole set
    // rather than over the one mode the defect was found on.
    let modes: [(&str, Option<&str>, bool); 5] = [
        ("awsvpc", Some("awsvpc"), true),
        ("bridge", Some("bridge"), false),
        ("host", Some("host"), false),
        ("none", Some("none"), false),
        ("the default mode of a revision that names none", None, false),
    ];

    for (case, mode, needs_configuration) in modes {
        let family = format!("netmode{}", mode.unwrap_or("default"));
        let mut members = json!({});
        if let Some(mode) = mode {
            members["networkMode"] = json!(mode);
        }
        // `awsvpc` on Fargate needs task-level cpu/memory; set for every row so the rows
        // differ only in the network mode.
        members["cpu"] = json!("256");
        members["memory"] = json!("512");
        register(
            &router,
            &family,
            members,
            json!([plain_container("web")]),
            case,
        )
        .await;

        // With the configuration, and without it: exactly one of the two is accepted, and
        // which one is decided by the mode.
        let rows: [(&str, bool); 2] = [("with a networkConfiguration", true), ("without one", false)];
        for (variant, with_configuration) in rows {
            let case = &format!("{case}, {variant}");
            let service_name = format!(
                "svc{}{}",
                mode.unwrap_or("default"),
                if with_configuration { "with" } else { "without" }
            );
            let mut create = json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": family,
                "desiredCount": 0,
                "launchType": "EC2",
            });
            let mut run = json!({ "cluster": CLUSTER, "taskDefinition": family });
            if with_configuration {
                create["networkConfiguration"] = awsvpc_network_configuration();
                run["networkConfiguration"] = awsvpc_network_configuration();
            }

            if with_configuration == needs_configuration {
                ok(&router, "CreateService", create, case).await;
                ok(&router, "RunTask", run, case).await;
                assert_eq!(
                    stored_services(&router, &[&service_name], case).await,
                    vec![service_name.clone()],
                    "case {case:?}: the accepted service must be stored"
                );
                continue;
            }

            // Both operations refuse the same pair, with the same code.
            for (operation, request) in [("CreateService", create), ("RunTask", run)] {
                let (code, message) = refusal(&router, operation, request, case).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case:?}: expected {operation} to answer an \
                     InvalidParameterException, got {code:?} with {message:?}"
                );
                // Both sentences are FIXED text: the one for a missing configuration names
                // the one mode that requires one, and the one for a configuration the
                // revision's mode cannot take points at that task definition's mode
                // instead of quoting it — so neither carries a value from this request.
                let expected = if needs_configuration {
                    "Network Configuration must be provided when networkMode 'awsvpc' is \
                     specified."
                } else {
                    "Network Configuration is not valid for the given networkMode of this \
                     task definition."
                };
                assert_eq!(
                    message, expected,
                    "case {case:?}: unexpected message from {operation}"
                );
            }
            assert!(
                stored_services(&router, &[&service_name], case).await.is_empty(),
                "case {case:?}: a refused create must store no service"
            );
        }
    }
}

#[tokio::test]
async fn enabling_execute_command_requires_the_revision_to_declare_a_task_role() {
    let router = service();
    // One container instance registered, so a task placed on the EC2 capacity the
    // cluster holds has somewhere to run; what these cases are about is what happens
    // before that placement.
    cluster_with_capacity(&router, CLUSTER, "setup").await;

    // The rows differ along what the revision declares for `taskRoleArn`: nothing, a
    // blank string (a member that is present but names no role), and a real role ARN.
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("no taskRoleArn at all", None, false),
        ("a blank taskRoleArn", Some("   "), false),
        ("a task role ARN", Some("role/ecsTaskRole"), true),
    ];

    for (case, task_role, allowed) in rows {
        let family = format!("exec{}", task_role.unwrap_or("none").trim().len());
        let mut members = json!({ "networkMode": "bridge" });
        if let Some(task_role) = task_role {
            members["taskRoleArn"] = json!(if task_role.trim().is_empty() {
                task_role.to_string()
            } else {
                format!("arn:aws:iam::{ACCOUNT}:{task_role}")
            });
        }
        register(
            &router,
            &family,
            members,
            json!([plain_container("web")]),
            case,
        )
        .await;

        let service_name = format!("exec{}", family);
        let create = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": family,
            "desiredCount": 0,
            "enableExecuteCommand": true,
        });
        let run = json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "enableExecuteCommand": true,
        });

        if allowed {
            let created = ok(&router, "CreateService", create, case).await;
            assert_eq!(
                created["service"]["enableExecuteCommand"],
                json!(true),
                "case {case:?}: an exec-enabled service reports it, got {created}"
            );
            ok(&router, "RunTask", run, case).await;
            continue;
        }

        for (operation, request) in [("CreateService", create), ("RunTask", run)] {
            let (code, message) = refusal(&router, operation, request, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected {operation} to answer an \
                 InvalidParameterException, got {code:?} with {message:?}"
            );
            assert!(
                message.contains("taskRoleArn"),
                "case {case:?}: expected {operation} to name the member at fault, got \
                 {message:?}"
            );
            // The remediation clause names WHERE the role belongs — in the task
            // definition, which is the resource the caller has to change — and is the
            // same clause whichever launch earned the refusal.
            assert!(
                message.ends_with(
                    "Specify a valid task role in your task definition and try again."
                ),
                "case {case:?}: expected {operation} to point the caller at the task \
                 definition, got {message:?}"
            );
        }
        assert!(
            stored_services(&router, &[&service_name], case).await.is_empty(),
            "case {case:?}: a refused create must store no service"
        );

        // The same revision WITHOUT execute command is accepted: the rule is about the
        // pair, not about the revision.
        let case = &format!("{case}, execute command off");
        let service_name = format!("noexec{}", family);
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": family,
                "desiredCount": 0,
            }),
            case,
        )
        .await;
        // And turning it ON through UpdateService is refused for the same reason, so the
        // rule cannot be walked around by a second call.
        let (code, message) = refusal(
            &router,
            "UpdateService",
            json!({
                "cluster": CLUSTER,
                "service": service_name,
                "enableExecuteCommand": true,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected UpdateService to refuse the same pair"
        );
        assert!(
            message.ends_with("Specify a valid task role in your task definition and try again."),
            "case {case:?}: expected UpdateService to point the caller at the task \
             definition, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_load_balancer_binding_names_a_container_and_port_the_revision_declares() {
    // BOTH stores: a binding names a target group the Elastic Load Balancing v2 service
    // has to hold, and whose target type has to match the revision's network mode, so the
    // rows below bind a real `instance` group created through that service's own API.
    let router = crate::load_balancer_target_groups::emulator();
    // One container instance registered, so a task placed on the EC2 capacity the
    // cluster holds has somewhere to run; what these cases are about is what happens
    // before that placement.
    cluster_with_capacity(&router, CLUSTER, "setup").await;

    // One revision whose containers declare a port in each of the two documented forms:
    // `PortMapping$containerPort` ("The port number on the container") and
    // `PortMapping$containerPortRange` ("The port number range on the container").
    let family = "bound";
    register(
        &router,
        family,
        json!({ "networkMode": "bridge" }),
        json!([
            {
                "name": "app",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
                "portMappings": [{ "containerPort": 80 }, { "containerPort": 8443 }],
            },
            {
                "name": "sidecar",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 64,
                "portMappings": [{ "containerPortRange": "9000-9010" }],
            },
        ]),
        "setup",
    )
    .await;

    let target_group =
        crate::load_balancer_target_groups::create_target_group(&router, "bound-tg", "instance", "setup")
            .await;
    /// One row: the case, the binding's container name and port, and whether the revision
    /// declares that pair.
    type Row = (&'static str, Option<&'static str>, Option<i64>, bool);
    let rows: [Row; 8] = [
        ("the first declared port of a declared container", Some("app"), Some(80), true),
        ("the second declared port of the same container", Some("app"), Some(8443), true),
        ("a port inside a declared container port range", Some("sidecar"), Some(9005), true),
        ("the lower bound of that range", Some("sidecar"), Some(9000), true),
        ("one port past that range", Some("sidecar"), Some(9011), false),
        ("a port the container does not declare", Some("app"), Some(8080), false),
        ("a container the revision does not declare", Some("web"), Some(80), false),
        ("a binding that names no container at all", None, Some(80), false),
    ];

    for (index, (case, container_name, container_port, allowed)) in rows.iter().enumerate() {
        let mut binding = json!({ "targetGroupArn": target_group });
        if let Some(container_name) = container_name {
            binding["containerName"] = json!(container_name);
        }
        if let Some(container_port) = container_port {
            binding["containerPort"] = json!(container_port);
        }
        let service_name = format!("bound{index}");
        let request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": family,
            "desiredCount": 0,
            "loadBalancers": [binding.clone()],
        });

        if *allowed {
            let created = ok(&router, "CreateService", request, case).await;
            assert_eq!(
                list(&created["service"], "loadBalancers", case),
                vec![binding],
                "case {case:?}: the accepted binding must round-trip, got {created}"
            );
            continue;
        }

        let (code, message) = refusal(&router, "CreateService", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        // The refusal is derived from the request, so it names what is at fault.
        let named = container_name.unwrap_or("containerName");
        assert!(
            message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
        );
        assert!(
            stored_services(&router, &[&service_name], case).await.is_empty(),
            "case {case:?}: a refused create must store no service"
        );
    }

    // An UpdateService that rebinds the service to a pair the revision does not declare
    // is refused the same way.
    let case = "rebinding to an undeclared port";
    let (code, _) = refusal(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": "bound0",
            "loadBalancers": [{
                "targetGroupArn": target_group,
                "containerName": "app",
                "containerPort": 8080,
            }],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected UpdateService to refuse an undeclared pair"
    );

    // A binding whose container and port the revision DOES declare is still refused while
    // no load balancer forwards to the group: only a listener can send traffic to it, so
    // the tasks it registers could never be reached. The same group, named in the same
    // request, is accepted once a listener forwards to it.
    let case = "a declared pair on a target group no load balancer forwards to";
    let orphan = crate::load_balancer_target_groups::create_unattached_target_group(
        &router,
        "orphan-tg",
        "instance",
        case,
    )
    .await;
    let request = json!({
        "cluster": CLUSTER,
        "serviceName": "orphaned",
        "taskDefinition": family,
        "desiredCount": 0,
        "loadBalancers": [{
            "targetGroupArn": orphan,
            "containerName": "app",
            "containerPort": 80,
        }],
    });
    let (code, message) = refusal(&router, "CreateService", request.clone(), case).await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message,
        format!(
            "The target group with targetGroupArn {orphan} does not have an associated \
             load balancer."
        ),
        "case {case:?}: unexpected message"
    );
    assert!(
        stored_services(&router, &["orphaned"], case).await.is_empty(),
        "case {case:?}: a refused create must store no service"
    );

    let case = "the same group once a listener forwards to it";
    crate::load_balancer_target_groups::attach_load_balancer(&router, "orphan-tg", &orphan, case)
        .await;
    let created = ok(&router, "CreateService", request, case).await;
    assert_eq!(
        list(&created["service"], "loadBalancers", case).len(),
        1,
        "case {case:?}: the accepted service must hold its binding, got {created}"
    );
}

#[tokio::test]
async fn a_code_deploy_service_requires_a_load_balancer() {
    // BOTH stores, for the reason the binding suite above gives: the binding names a real
    // target group.
    let router = crate::load_balancer_target_groups::emulator();
    // One container instance registered, so a task placed on the EC2 capacity the
    // cluster holds has somewhere to run; what these cases are about is what happens
    // before that placement.
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    let family = "controlled";
    register(
        &router,
        family,
        json!({ "networkMode": "bridge" }),
        json!([{
            "name": "app",
            "image": "public.ecr.aws/nginx/nginx:latest",
            "memory": 128,
            "portMappings": [{ "containerPort": 80 }],
        }]),
        "setup",
    )
    .await;
    let target_group = crate::load_balancer_target_groups::create_target_group(
        &router,
        "controlled-tg",
        "instance",
        "setup",
    )
    .await;
    let binding = json!([{
        "targetGroupArn": target_group,
        "containerName": "app",
        "containerPort": 80,
    }]);

    // The rule is keyed on the CONTROLLER: the same request is legal under the ECS and
    // EXTERNAL controllers and refused under CODE_DEPLOY, and CODE_DEPLOY with a binding
    // is accepted. The rows are every value of the closed `DeploymentControllerType`
    // enum, crossed with whether the request carries a binding.
    let rows: [(&str, Option<&str>, bool, bool); 6] = [
        ("the default controller with no binding", None, false, true),
        ("the ECS controller with no binding", Some("ECS"), false, true),
        ("the EXTERNAL controller with no binding", Some("EXTERNAL"), false, true),
        ("the ECS controller with a binding", Some("ECS"), true, true),
        ("CODE_DEPLOY with a binding", Some("CODE_DEPLOY"), true, true),
        ("CODE_DEPLOY with no binding", Some("CODE_DEPLOY"), false, false),
    ];

    for (index, (case, controller, bound, allowed)) in rows.iter().enumerate() {
        let service_name = format!("controller{index}");
        let mut request = json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "desiredCount": 0,
        });
        // An EXTERNAL-controller service's tasks come from a task set, so it names no
        // task definition — and then no binding can be checked against one either.
        if *controller != Some("EXTERNAL") {
            request["taskDefinition"] = json!(family);
        }
        if let Some(controller) = controller {
            request["deploymentController"] = json!({ "type": controller });
        }
        if *bound {
            request["loadBalancers"] = binding.clone();
        }

        if *allowed {
            ok(&router, "CreateService", request, case).await;
            continue;
        }
        let (code, message) = refusal(&router, "CreateService", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            "When specifying the CODE_DEPLOY deployment controller type, the service \
             requires an Application Load Balancer or Network Load Balancer.",
            "case {case:?}: unexpected message"
        );
        assert!(
            stored_services(&router, &[&service_name], case).await.is_empty(),
            "case {case:?}: a refused create must store no service"
        );
    }

    // An update that would take the binding off a CODE_DEPLOY service is refused too.
    let case = "clearing the binding of a CODE_DEPLOY service";
    let (code, _) = refusal(
        &router,
        "UpdateService",
        json!({ "cluster": CLUSTER, "service": "controller4", "loadBalancers": [] }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected UpdateService to refuse a CODE_DEPLOY service with no \
         binding"
    );
}

#[tokio::test]
async fn a_refused_run_task_runs_nothing() {
    let router = service();
    // One container instance registered, so a task placed on the EC2 capacity the
    // cluster holds has somewhere to run; what these cases are about is what happens
    // before that placement.
    cluster_with_capacity(&router, CLUSTER, "setup").await;
    let family = "runnable";
    register(
        &router,
        family,
        json!({ "networkMode": "awsvpc", "cpu": "256", "memory": "512" }),
        json!([plain_container("web")]),
        "setup",
    )
    .await;

    // Every count the request may ask for leaves the task store untouched when the
    // launch is refused, so the refusal cannot run a partial batch.
    for count in [1, 3, 10] {
        let case = &format!("a refused RunTask of {count} tasks");
        refused(
            &router,
            "RunTask",
            json!({ "cluster": CLUSTER, "taskDefinition": family, "count": count }),
            "InvalidParameterException",
            case,
        )
        .await;
        assert_eq!(
            task_count(&router, case).await,
            0,
            "case {case:?}: a refused RunTask must run no task"
        );
    }

    // The same request WITH the configuration runs exactly what it asked for.
    let case = "the accepted RunTask";
    let body = ok(
        &router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "count": 2,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    assert_eq!(
        list(&body, "tasks", case).len(),
        2,
        "case {case:?}: expected both tasks, got {body}"
    );
    assert_eq!(
        task_count(&router, case).await,
        2,
        "case {case:?}: expected both tasks in the store"
    );
}

#[tokio::test]
async fn a_task_on_an_auto_scaling_group_provider_needs_capacity_unless_it_scales_itself() {
    let router = service();
    let family = "provided";
    // The revision is registered in the default cluster-independent way; each row gets
    // its OWN cluster with NO container instance registered, since associating capacity
    // providers with a cluster puts its attachments in an update this suite must not wait
    // out between rows.
    create_cluster(&router, CLUSTER, "setup").await;
    register(
        &router,
        family,
        json!({ "networkMode": "bridge" }),
        json!([plain_container("web")]),
        "setup",
    )
    .await;

    // The rows differ along the provider's managed scaling, which is what decides whether
    // Amazon ECS adds the capacity itself: absent, explicitly DISABLED, and ENABLED (the
    // whole `ManagedScalingStatus` enum plus the documented default of off).
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("no managed scaling at all", None, false),
        ("managed scaling DISABLED", Some("DISABLED"), false),
        ("managed scaling ENABLED", Some("ENABLED"), true),
    ];

    for (index, (case, managed_scaling, accepted)) in rows.iter().enumerate() {
        let provider = format!("provider{index}");
        let cluster = format!("provided{index}");
        create_cluster(&router, &cluster, case).await;
        let mut group = json!({ "autoScalingGroupArn": format!("asg-{provider}") });
        if let Some(status) = managed_scaling {
            group["managedScaling"] = json!({ "status": status, "targetCapacity": 100 });
        }
        ok(
            &router,
            "CreateCapacityProvider",
            json!({ "name": provider, "autoScalingGroupProvider": group }),
            case,
        )
        .await;
        ok(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": cluster,
                "capacityProviders": [provider],
                "defaultCapacityProviderStrategy": [
                    { "capacityProvider": provider, "weight": 1 },
                ],
            }),
            case,
        )
        .await;

        let request = json!({
            "cluster": cluster,
            "taskDefinition": family,
            "capacityProviderStrategy": [{ "capacityProvider": provider, "weight": 1 }],
        });
        let before = tasks_in(&router, &cluster, case).await;
        if *accepted {
            // Managed scaling is what adds the capacity, so the placement is one Amazon
            // ECS can satisfy.
            let body = ok(&router, "RunTask", request, case).await;
            assert_eq!(
                list(&body, "tasks", case).len(),
                1,
                "case {case:?}: expected the task to be placed, got {body}"
            );
            assert_eq!(
                string(&list(&body, "tasks", case)[0], "capacityProviderName", case),
                provider,
                "case {case:?}: the task must name the provider that placed it, got {body}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        // The refusal is built from the request: it names the PROVIDER the placement went
        // through, which is the Auto Scaling group the caller has to scale, so the
        // expected sentence differs row by row with the provider's name.
        assert_eq!(
            message,
            format!("The specified capacity provider '{provider}' contains no container instances."),
            "case {case:?}: expected the refusal to name the empty provider"
        );
        assert_eq!(
            tasks_in(&router, &cluster, case).await,
            before,
            "case {case:?}: a refused RunTask must run no task"
        );
    }

    // A cluster can carry SEVERAL providers, and the refusal names the one the placement
    // asked for — not the cluster, and not whichever provider happens to be first.
    let cluster = "multiprovider";
    create_cluster(&router, cluster, "two providers").await;
    let names = ["alpha", "beta"];
    for name in names {
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": name,
                "autoScalingGroupProvider": { "autoScalingGroupArn": format!("asg-{name}") },
            }),
            "two providers",
        )
        .await;
    }
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": names,
            "defaultCapacityProviderStrategy": [],
        }),
        "two providers",
    )
    .await;
    for name in names {
        let case = &format!("the strategy names {name}");
        let (code, message) = refusal(
            &router,
            "RunTask",
            json!({
                "cluster": cluster,
                "taskDefinition": family,
                "capacityProviderStrategy": [{ "capacityProvider": name, "weight": 1 }],
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
            format!("The specified capacity provider '{name}' contains no container instances."),
            "case {case:?}: expected the refusal to name the provider the placement used"
        );
    }
}

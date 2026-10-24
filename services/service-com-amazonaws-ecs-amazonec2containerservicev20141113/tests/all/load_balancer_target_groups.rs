// SPDX-License-Identifier: CC0-1.0

//! The pairing an ECS service's `loadBalancers` entry has to satisfy: the ELBv2 target
//! group it names must carry the target type the service's task-definition revision
//! registers its tasks as.
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A `loadBalancers` entry of `CreateService` or `UpdateService` names an Elastic Load
//! > Balancing v2 target group that must EXIST and whose `TargetType` must be the one the
//! > task definition's `networkMode` registers tasks as — `ip` for `awsvpc`, whose tasks
//! > each get their own elastic network interface, and `instance` for every other mode,
//! > whose tasks are reached at the container instance that runs them — and any other
//! > pairing is refused `400 InvalidParameterException` with nothing persisted.
//!
//! `LoadBalancer$targetGroupArn`'s own doc is the `awsvpc` half: "If your service's task
//! definition uses the `awsvpc` network mode, you must choose `ip` as the target type, not
//! `instance`. Do this when creating your target groups because tasks that use the
//! `awsvpc` network mode are associated with an elastic network interface, not an Amazon
//! EC2 instance."
//!
//! The target type is not a value the request carries: it lives in the ELBv2 service's own
//! store. So every row here creates its target group through the ELBv2 API
//! (`CreateTargetGroup` at `Version=2015-12-01`, awsQuery) on the SAME engine the ECS
//! service runs on, and the ECS call is made through the ECS front door — a cross-service
//! READ, pinned end to end rather than at the state struct.
//!
//! The `TargetType` catalog each row draws from is that service's `TargetTypeEnum`
//! (`smithy_query(query="shape", shape="TargetTypeEnum",
//! model_path="services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json")`:
//! `instance`, `ip`, `lambda`, `alb`), and the network modes are the ECS model's
//! `NetworkMode` enum (`bridge`, `host`, `awsvpc`, `none`).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, string, SIGNED_REGION};
use crate::task_lifecycle::container;

/// The ELBv2 API version every awsQuery request below carries, from that model's service
/// version.
const ELB_VERSION: &str = "2015-12-01";

/// The VPC every target group below is created in.
const VPC: &str = "vpc-0abc1234ecs0lb";

/// A router carrying the ECS service AND the Elastic Load Balancing v2 service over ONE
/// shared engine — the two stores this rule reads across, mounted the way the production
/// router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one awsQuery form component. Escaping every non-alphanumeric byte is
/// always a valid form encoding.
fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(char::from(*byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The text of the first element named `tag` in an awsQuery answer.
fn first_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// One ELBv2 awsQuery call through the shared router, returning the status and the body.
async fn elb_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut form = vec![
        format!("Action={}", encode(action)),
        format!("Version={}", encode(ELB_VERSION)),
    ];
    for (key, value) in params {
        form.push(format!("{}={}", encode(key), encode(value)));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://elasticloadbalancingv2.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDELBTEST/20260101/{SIGNED_REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(form.join("&")))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Create one target group of `target_type` through the ELBv2 API and return its ARN, as
/// that service minted it.
async fn create_target_group(
    router: &MultiServiceRouter,
    name: &str,
    target_type: &str,
    case: &str,
) -> String {
    // A `lambda` target group takes no protocol, port or VPC; every other type does.
    let mut params: Vec<(&str, &str)> = vec![("Name", name), ("TargetType", target_type)];
    if target_type != "lambda" {
        params.extend([("Protocol", "HTTP"), ("Port", "80"), ("VpcId", VPC)]);
    }
    let (status, body) = elb_call(router, "CreateTargetGroup", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTargetGroup for a {target_type} target group must succeed, \
         got {status} with {body}"
    );
    let arn = first_text(&body, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {body}"));
    // The read back through the ELBv2 model confirms the stored target type the ECS check
    // will read — the row's own premise.
    let (status, described) =
        elb_call(router, "DescribeTargetGroups", &[("Names.member.1", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetGroups must read it back, got {status} with {described}"
    );
    assert_eq!(
        first_text(&described, "TargetType").as_deref(),
        Some(target_type),
        "case {case:?}: expected the stored target type {target_type}, got {described}"
    );
    arn
}

/// Register one revision of `family` in `network_mode` and return its ARN. A revision in
/// the `none` network mode gets a container with no port mappings, which is what that mode
/// allows ("If the network mode is set to none, you cannot specify port mappings").
async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    case: &str,
) -> String {
    let mut web = container("web", "public.ecr.aws/nginx/nginx:latest");
    if network_mode != "none" {
        web["portMappings"] = json!([{ "containerPort": 80, "protocol": "tcp" }]);
    }
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": [web],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// The `CreateService` request body of a service in `cluster` attached to `target_group`.
fn create_body(
    cluster: &str,
    service_name: &str,
    task_definition: &str,
    network_mode: &str,
    load_balancers: Value,
) -> Value {
    let mut body = json!({
        "cluster": cluster,
        "serviceName": service_name,
        "taskDefinition": task_definition,
        "desiredCount": 1,
        "loadBalancers": load_balancers,
    });
    if network_mode == "awsvpc" {
        // "This parameter is required for task definitions that use the awsvpc network
        // mode."
        body["networkConfiguration"] = json!({
            "awsvpcConfiguration": { "subnets": ["subnet-0abc1234ecs0lb"] },
        });
    }
    body
}

/// One `loadBalancers` entry naming `target_group`.
fn attachment(target_group: &str) -> Value {
    json!([{
        "targetGroupArn": target_group,
        "containerName": "web",
        "containerPort": 80,
    }])
}

/// The `loadBalancers` of the service `cluster`/`service_name` holds, read back through
/// `DescribeServices`.
async fn stored_load_balancers(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly one described service, got {body}"
    );
    list(&services[0], "loadBalancers", case)
}

/// Whether `DescribeServices` reports the named service at all.
async fn service_exists(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> bool {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    !list(&body, "services", case).is_empty()
}

/// One row of the pairing table: a network mode, a target type, and whether a service
/// pairing the two is created or refused.
struct Row {
    case: &'static str,
    network_mode: &'static str,
    target_type: &'static str,
    created: bool,
}

#[tokio::test]
async fn create_service_pairs_a_target_group_with_the_task_definitions_network_mode() {
    let router = emulator();
    let cluster = "lb-pairing";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster every row creates its service in",
    )
    .await;

    // The rows span the two halves of the rule — both directions of the `awsvpc`/`ip`
    // pairing and of the instance-registered modes — plus the target types ECS registers
    // nothing as (`lambda` and `alb` carry a Lambda function and a load balancer, not a
    // task). The `none` mode appears only as a refusal: a load-balanced service in that
    // mode also needs a port mapping it cannot have, which is a different rule.
    let rows: [Row; 10] = [
        Row {
            case: "awsvpc with an ip target group",
            network_mode: "awsvpc",
            target_type: "ip",
            created: true,
        },
        Row {
            // The issue's own case.
            case: "awsvpc with an instance target group",
            network_mode: "awsvpc",
            target_type: "instance",
            created: false,
        },
        Row {
            case: "awsvpc with a lambda target group",
            network_mode: "awsvpc",
            target_type: "lambda",
            created: false,
        },
        Row {
            case: "awsvpc with an alb target group",
            network_mode: "awsvpc",
            target_type: "alb",
            created: false,
        },
        Row {
            case: "bridge with an instance target group",
            network_mode: "bridge",
            target_type: "instance",
            created: true,
        },
        Row {
            case: "bridge with an ip target group",
            network_mode: "bridge",
            target_type: "ip",
            created: false,
        },
        Row {
            case: "host with an instance target group",
            network_mode: "host",
            target_type: "instance",
            created: true,
        },
        Row {
            case: "host with an ip target group",
            network_mode: "host",
            target_type: "ip",
            created: false,
        },
        Row {
            case: "host with an alb target group",
            network_mode: "host",
            target_type: "alb",
            created: false,
        },
        Row {
            case: "none with an ip target group",
            network_mode: "none",
            target_type: "ip",
            created: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let group_name = format!("pair-tg-{index}");
        let family = format!("pair-family-{index}");
        let service_name = format!("pair-svc-{index}");
        let target_group = create_target_group(&router, &group_name, row.target_type, case).await;
        let definition = register_definition(&router, &family, row.network_mode, case).await;
        let body = create_body(
            cluster,
            &service_name,
            &definition,
            row.network_mode,
            attachment(&target_group),
        );

        if row.created {
            ok(&router, "CreateService", body, case).await;
            let stored = stored_load_balancers(&router, cluster, &service_name, case).await;
            assert_eq!(
                stored.len(),
                1,
                "case {case:?}: expected the stored service to hold its one attachment, \
                 got {stored:?}"
            );
            let stored_arn = string(&stored[0], "targetGroupArn", case);
            assert_eq!(
                stored_arn, target_group,
                "case {case:?}: expected the stored attachment to name {target_group}, got \
                 {stored_arn}"
            );
        } else {
            let (code, message) = refusal(&router, "CreateService", body, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the modelled InvalidParameterException, got {code} \
                 with {message:?}"
            );
            for expected in [row.target_type, row.network_mode] {
                assert!(
                    message.contains(expected),
                    "case {case:?}: expected the refusal to name {expected:?}, got {message:?}"
                );
            }
            assert!(
                !service_exists(&router, cluster, &service_name, case).await,
                "case {case:?}: the refused service must not be persisted, but \
                 DescribeServices reports {service_name}"
            );
        }
    }
}

#[tokio::test]
async fn create_service_refuses_a_target_group_no_load_balancer_holds() {
    let router = emulator();
    let cluster = "lb-unknown";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    // An ARN of the right shape for every field a target-group ARN has, naming a target
    // group the ELBv2 store does not hold — and one that names a target group only after
    // it is created, so the refusal is about the RESOURCE and not about the text.
    let real = create_target_group(&router, "unknown-real-tg", "ip", "the real group").await;
    let definition = register_definition(&router, "unknown-family", "awsvpc", "the revision").await;
    let cases: [(&str, String); 2] = [
        (
            "a target group ARN nothing created",
            real.replace("unknown-real-tg", "never-created-tg"),
        ),
        (
            "a target group ARN with an id no mint produced",
            format!("{real}deadbeef"),
        ),
    ];

    for (index, (case, arn)) in cases.iter().enumerate() {
        let service_name = format!("unknown-svc-{index}");
        let (code, message) = refusal(
            &router,
            "CreateService",
            create_body(
                cluster,
                &service_name,
                &definition,
                "awsvpc",
                attachment(arn),
            ),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled InvalidParameterException, got {code} with \
             {message:?}"
        );
        assert!(
            message.contains(arn),
            "case {case:?}: expected the refusal to name {arn:?}, got {message:?}"
        );
        assert!(
            !service_exists(&router, cluster, &service_name, case).await,
            "case {case:?}: the refused service must not be persisted, but DescribeServices \
             reports {service_name}"
        );
    }

    // The same request against the target group that DOES exist is created: the refusals
    // above are the resource's absence, not the attachment itself.
    ok(
        &router,
        "CreateService",
        create_body(
            cluster,
            "unknown-svc-ok",
            &definition,
            "awsvpc",
            attachment(&real),
        ),
        "the attachment that names a stored target group",
    )
    .await;
}

#[tokio::test]
async fn an_alternate_target_group_is_held_to_the_same_pairing() {
    let router = emulator();
    let cluster = "lb-alternate";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    // `AdvancedConfiguration$alternateTargetGroupArn` is the second target group a
    // blue/green deployment registers the SAME tasks into, so the pairing rule covers it.
    let primary = create_target_group(&router, "alt-primary-tg", "ip", "the primary group").await;
    let good = create_target_group(&router, "alt-ip-tg", "ip", "the alternate ip group").await;
    let bad =
        create_target_group(&router, "alt-instance-tg", "instance", "the instance group").await;
    let definition = register_definition(&router, "alt-family", "awsvpc", "the revision").await;

    let entry = |alternate: &str| {
        json!([{
            "targetGroupArn": primary.clone(),
            "containerName": "web",
            "containerPort": 80,
            "advancedConfiguration": { "alternateTargetGroupArn": alternate },
        }])
    };

    let case = "an awsvpc service whose alternate target group is instance-typed";
    let (code, message) = refusal(
        &router,
        "CreateService",
        create_body(cluster, "alt-svc-bad", &definition, "awsvpc", entry(&bad)),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected the modelled InvalidParameterException, got {code} with \
         {message:?}"
    );
    assert!(
        message.contains(&bad) && message.contains("instance"),
        "case {case:?}: expected the refusal to name the alternate group and its target \
         type, got {message:?}"
    );

    let case = "an awsvpc service whose alternate target group is ip-typed";
    ok(
        &router,
        "CreateService",
        create_body(cluster, "alt-svc-good", &definition, "awsvpc", entry(&good)),
        case,
    )
    .await;
    let stored = stored_load_balancers(&router, cluster, "alt-svc-good", case).await;
    let alternate = string(
        &stored[0]["advancedConfiguration"],
        "alternateTargetGroupArn",
        case,
    );
    assert_eq!(
        alternate, good,
        "case {case:?}: expected the stored alternate target group {good}, got {alternate}"
    );
}

#[tokio::test]
async fn the_number_of_attachments_one_request_may_name_is_bounded() {
    let router = emulator();
    let cluster = "lb-quota";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    let definition = register_definition(&router, "quota-family", "awsvpc", "the revision").await;
    // `state::MAX_LOAD_BALANCERS` is the Amazon ECS quota for target groups per service.
    // The rows are the bound itself and one past it: a list AT the bound is created, one
    // PAST it is refused before any of its entries is looked up.
    let cap = service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_LOAD_BALANCERS;
    let mut groups = Vec::new();
    for index in 0..cap {
        groups.push(
            create_target_group(
                &router,
                &format!("quota-tg-{index}"),
                "ip",
                "the groups at the bound",
            )
            .await,
        );
    }
    let entries = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    json!({
                        "targetGroupArn": groups[index % groups.len()].clone(),
                        "containerName": "web",
                        "containerPort": 80,
                    })
                })
                .collect(),
        )
    };

    let case = "a service with as many attachments as the quota allows";
    ok(
        &router,
        "CreateService",
        create_body(cluster, "quota-svc-ok", &definition, "awsvpc", entries(cap)),
        case,
    )
    .await;
    let stored = stored_load_balancers(&router, cluster, "quota-svc-ok", case).await;
    assert_eq!(
        stored.len(),
        cap,
        "case {case:?}: expected the stored service to hold {cap} attachments, got \
         {stored:?}"
    );

    let case = "a service with one attachment more than the quota allows";
    let (code, message) = refusal(
        &router,
        "CreateService",
        create_body(
            cluster,
            "quota-svc-over",
            &definition,
            "awsvpc",
            entries(cap + 1),
        ),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected the modelled ClientException, got {code} with {message:?}"
    );
    assert!(
        message.contains(&cap.to_string()),
        "case {case:?}: expected the refusal to name the bound {cap}, got {message:?}"
    );
    assert!(
        !service_exists(&router, cluster, "quota-svc-over", case).await,
        "case {case:?}: the refused service must not be persisted"
    );

    // The same bound on the update path.
    let case = "an update that names one attachment more than the quota allows";
    let (code, message) = refusal(
        &router,
        "UpdateService",
        json!({
            "cluster": cluster,
            "service": "quota-svc-ok",
            "loadBalancers": entries(cap + 1),
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected the modelled ClientException, got {code} with {message:?}"
    );
    let still = stored_load_balancers(&router, cluster, "quota-svc-ok", case).await;
    assert_eq!(
        still.len(),
        cap,
        "case {case:?}: expected the refused update to leave {cap} attachments, got \
         {still:?}"
    );
}

#[tokio::test]
async fn update_service_holds_the_new_pairing_to_the_same_rule() {
    let router = emulator();
    let cluster = "lb-update";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": cluster }),
        "the cluster",
    )
    .await;

    let instance_group =
        create_target_group(&router, "update-instance-tg", "instance", "the instance group").await;
    let ip_group = create_target_group(&router, "update-ip-tg", "ip", "the ip group").await;
    let bridge = register_definition(&router, "update-bridge", "bridge", "the bridge revision")
        .await;
    let awsvpc = register_definition(&router, "update-awsvpc", "awsvpc", "the awsvpc revision")
        .await;

    // A bridge service on the instance target group: the pairing the rule allows.
    let service_name = "update-svc";
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
        "the service the updates below act on",
    )
    .await;

    // Each row is one UPDATE of that service: what it names, and whether it is refused.
    let rows: [(&str, Value, bool); 4] = [
        (
            "moving a bridge service onto an awsvpc revision, keeping its instance target \
             group",
            json!({ "taskDefinition": awsvpc.clone() }),
            false,
        ),
        (
            "attaching an ip target group to a service that still runs a bridge revision",
            json!({ "loadBalancers": attachment(&ip_group) }),
            false,
        ),
        (
            "attaching a target group nothing created",
            json!({ "loadBalancers": attachment(&format!("{ip_group}-gone")) }),
            false,
        ),
        (
            "moving onto the awsvpc revision AND its ip target group together",
            json!({
                "taskDefinition": awsvpc.clone(),
                "loadBalancers": attachment(&ip_group),
                "networkConfiguration": {
                    "awsvpcConfiguration": { "subnets": ["subnet-0abc1234ecs0lb"] },
                },
            }),
            true,
        ),
    ];

    for (case, mut request, accepted) in rows {
        request["cluster"] = json!(cluster);
        request["service"] = json!(service_name);
        if accepted {
            ok(&router, "UpdateService", request, case).await;
        } else {
            let (code, message) = refusal(&router, "UpdateService", request, case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the modelled InvalidParameterException, got {code} \
                 with {message:?}"
            );
            // Nothing is half-applied: the service still runs the revision and holds the
            // attachment it had.
            let body = ok(
                &router,
                "DescribeServices",
                json!({ "cluster": cluster, "services": [service_name] }),
                case,
            )
            .await;
            let described = list(&body, "services", case);
            let running = string(&described[0], "taskDefinition", case);
            assert_eq!(
                running, bridge,
                "case {case:?}: expected the refused update to leave the service on \
                 {bridge}, got {running}"
            );
            let stored = list(&described[0], "loadBalancers", case);
            let stored_arn = string(&stored[0], "targetGroupArn", case);
            assert_eq!(
                stored_arn, instance_group,
                "case {case:?}: expected the refused update to leave the attachment on \
                 {instance_group}, got {stored_arn}"
            );
        }
    }

    // The accepted update is what the service reports afterwards.
    let case = "the service after the accepted update";
    let stored = stored_load_balancers(&router, cluster, service_name, case).await;
    let stored_arn = string(&stored[0], "targetGroupArn", case);
    assert_eq!(
        stored_arn, ip_group,
        "case {case:?}: expected the stored attachment to name {ip_group}, got {stored_arn}"
    );
}

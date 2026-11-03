// SPDX-License-Identifier: CC0-1.0

//! The `awsvpcConfiguration` a launch STORES, read back through every projection of it.
//!
//! One rule, asserted over a table of configurations rather than over one recorded
//! request: Amazon ECS stores the network configuration a launch supplies with the
//! optional members it fills in already filled in — `securityGroups` is materialised as
//! an EMPTY LIST when the request names none (the interface then takes "the default
//! security group for the VPC", whose id belongs to the VPC and not to this
//! configuration), and `assignPublicIp` takes the value its own doc gives it ("When you
//! use `create-service` or `update-service`, the default is `DISABLED`") — so every read
//! of that configuration reports the same complete structure, which a caller can diff or
//! re-apply unchanged.
//!
//! The rule is about the stored representation, not about one operation, so the table runs
//! each configuration through BOTH operations that store one — `CreateService` and
//! `CreateTaskSet` — and asserts it on every projection those two feed: the create's own
//! response, the service's `PRIMARY` deployment, `DescribeServices` and
//! `DescribeTaskSets`.
//!
//! Member and limit source: `smithy_query(query="shape", shape="AwsVpcConfiguration")` —
//! `subnets` is required with "a limit of 16 subnets", `securityGroups` is an optional
//! list with "a limit of 5 security groups", and `assignPublicIp` carries the
//! `create-service`/`update-service` default quoted above.
//!
//! Every case goes through the HTTP front door in [`crate::ecs_wire`], so a member that is
//! not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, service, string};

/// The image every container here starts from; irrelevant to the rule, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// EC2-shaped subnet ids, as many as the case needs. ECS never resolves them: it stores
/// the configuration it is given, so the ids only have to be distinct and well shaped.
fn subnet_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("subnet-{index:017x}")).collect()
}

/// EC2-shaped security-group ids, as many as the case needs.
fn security_group_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("sg-{index:017x}")).collect()
}

/// One row of the table: the configuration the request carries, and what every read of
/// the stored configuration must then report.
#[derive(Debug, Clone)]
struct ConfigurationCase {
    name: &'static str,
    /// How many subnets the request names (`subnets` is required, so never none).
    subnets: usize,
    /// How many security groups the request names, or `None` when it omits the member.
    security_groups: Option<usize>,
    /// The `assignPublicIp` the request names, or `None` when it omits the member.
    assign_public_ip: Option<&'static str>,
    /// The `assignPublicIp` every read must report.
    expected_assign_public_ip: &'static str,
    /// The launch type the row places its tasks with. A public IP is attached to the
    /// task's own elastic network interface, which only Fargate does, so the row that asks
    /// for one is placed there and every other row runs on the cluster's own capacity.
    launch_type: &'static str,
}

impl ConfigurationCase {
    /// The `awsvpcConfiguration` this row sends, carrying only the members it names.
    fn request(&self) -> Value {
        let mut configuration = json!({ "subnets": subnet_ids(self.subnets) });
        if let Some(groups) = self.security_groups {
            configuration["securityGroups"] = json!(security_group_ids(groups));
        }
        if let Some(assign) = self.assign_public_ip {
            configuration["assignPublicIp"] = json!(assign);
        }
        json!({ "awsvpcConfiguration": configuration })
    }

    /// The security groups every read must report: the ones the request named, and an
    /// empty list both when it named none and when it named an empty list.
    fn expected_security_groups(&self) -> Vec<String> {
        security_group_ids(self.security_groups.unwrap_or(0))
    }
}

/// Register one `awsvpc` revision of `family`, which every launch below runs. Task-level
/// `cpu` and `memory` make it Fargate-capable as well, so the row that asks for a public IP
/// can be placed there (`RegisterTaskDefinition$cpu`: the task size Fargate requires).
async fn register_awsvpc_definition(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [crate::task_lifecycle::container("web", IMAGE)],
        }),
        case,
    )
    .await;
}

/// Describe one service and return the view.
async fn describe_service(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described, got {body}"))
}

/// Describe the task sets of one service and return the view of the only one it holds.
async fn describe_only_task_set(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeTaskSets",
        json!({ "cluster": cluster, "service": service_name }),
        case,
    )
    .await;
    let sets = list(&body, "taskSets", case);
    assert_eq!(
        sets.len(),
        1,
        "case {case:?}: expected exactly the one task set that was created, got {body}"
    );
    sets[0].clone()
}

/// The `PRIMARY` deployment of a service view.
fn primary_deployment(service_view: &Value, case: &str) -> Value {
    list(service_view, "deployments", case)
        .into_iter()
        .find(|deployment| deployment["status"].as_str() == Some("PRIMARY"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a PRIMARY deployment, got {service_view}")
        })
}

/// Assert that `view` — any projection carrying a `networkConfiguration` — reports the
/// configuration `test` asked for, with the members ECS fills in materialised.
fn assert_stored_configuration(view: &Value, source: &str, test: &ConfigurationCase) {
    let case = test.name;
    let configured = &view["networkConfiguration"]["awsvpcConfiguration"];
    assert!(
        configured.is_object(),
        "case {case:?}: {source} reports the stored awsvpcConfiguration, got {view}"
    );

    let reported_subnets: Vec<String> = list(configured, "subnets", case)
        .iter()
        .map(|subnet| subnet.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported_subnets,
        subnet_ids(test.subnets),
        "case {case:?}: {source} expected the subnets to round-trip in order, got \
         {configured}"
    );

    // The member a request may leave out is a LIST on the wire whether or not it was
    // named: absent is not the same answer as empty to a client that dereferences it.
    assert!(
        configured["securityGroups"].is_array(),
        "case {case:?}: {source} materialises securityGroups as a list, got {configured}"
    );
    let reported_groups: Vec<String> = list(configured, "securityGroups", case)
        .iter()
        .map(|group| group.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported_groups,
        test.expected_security_groups(),
        "case {case:?}: {source} expected securityGroups {:?}, got {configured}",
        test.expected_security_groups()
    );

    assert_eq!(
        configured["assignPublicIp"].as_str(),
        Some(test.expected_assign_public_ip),
        "case {case:?}: {source} expected assignPublicIp {:?}, got {configured}",
        test.expected_assign_public_ip
    );
}

/// The rows: they differ along how many subnets the configuration names, whether it names
/// `securityGroups` at all (omitted, an explicit empty list, or one of several sizes) and
/// whether it names `assignPublicIp`. The counts reach the documented maximum of each
/// list (16 subnets, 5 security groups) as well as the single-entry case.
fn cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            name: "one subnet, neither optional member",
            subnets: 1,
            security_groups: None,
            assign_public_ip: None,
            expected_assign_public_ip: "DISABLED",
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "the documented maximum of sixteen subnets, neither optional member",
            subnets: 16,
            security_groups: None,
            assign_public_ip: None,
            expected_assign_public_ip: "DISABLED",
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "an explicitly empty security-group list",
            subnets: 1,
            security_groups: Some(0),
            assign_public_ip: None,
            expected_assign_public_ip: "DISABLED",
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "one security group",
            subnets: 1,
            security_groups: Some(1),
            assign_public_ip: None,
            expected_assign_public_ip: "DISABLED",
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "the documented maximum of five security groups across two subnets",
            subnets: 2,
            security_groups: Some(5),
            assign_public_ip: None,
            expected_assign_public_ip: "DISABLED",
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "no security groups, assignPublicIp ENABLED on Fargate",
            subnets: 1,
            security_groups: None,
            assign_public_ip: Some("ENABLED"),
            expected_assign_public_ip: "ENABLED",
            launch_type: "FARGATE",
        },
        ConfigurationCase {
            name: "no security groups, the default assignPublicIp named explicitly",
            subnets: 1,
            security_groups: None,
            assign_public_ip: Some("DISABLED"),
            expected_assign_public_ip: "DISABLED",
            launch_type: "EC2",
        },
    ]
}

#[tokio::test]
async fn create_service_stores_an_awsvpc_configuration_with_the_members_ecs_fills_in() {
    let router = service();
    let cluster = "awsvpcsvc";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_awsvpc_definition(&router, "awsvpcsvcfamily", "setup").await;

    for (index, test) in cases().iter().enumerate() {
        let case = test.name;
        let name = format!("svc{index}");
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": name,
                "taskDefinition": "awsvpcsvcfamily",
                "desiredCount": 0,
                "launchType": test.launch_type,
                "networkConfiguration": test.request(),
            }),
            case,
        )
        .await;
        let created = created["service"].clone();
        assert_eq!(
            string(&created, "serviceName", case),
            name,
            "case {case:?}: the create reports the service it was asked for, got {created}"
        );

        // The create's own answer, the deployment that rollout is, and the later read of
        // the same service are all projections of ONE stored configuration, so all three
        // report it the same way.
        assert_stored_configuration(&created, "CreateService", test);
        assert_stored_configuration(
            &primary_deployment(&created, case),
            "the PRIMARY deployment of CreateService",
            test,
        );
        let read = describe_service(&router, cluster, &name, case).await;
        assert_stored_configuration(&read, "DescribeServices", test);
        assert_stored_configuration(
            &primary_deployment(&read, case),
            "the PRIMARY deployment of DescribeServices",
            test,
        );
    }
}

#[tokio::test]
async fn create_task_set_stores_an_awsvpc_configuration_with_the_members_ecs_fills_in() {
    let router = service();
    let cluster = "awsvpcset";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_awsvpc_definition(&router, "awsvpcsetfamily", "setup").await;

    for (index, test) in cases().iter().enumerate() {
        let case = test.name;
        // A task set belongs to a service the EXTERNAL deployment controller owns ("This
        // is used when a service uses the EXTERNAL deployment controller type"), and one
        // service per row keeps the rows from reading each other's task sets.
        let service_name = format!("ext{index}");
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": service_name,
                "desiredCount": 0,
                "deploymentController": { "type": "EXTERNAL" },
            }),
            case,
        )
        .await;

        let created = ok(
            &router,
            "CreateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskDefinition": "awsvpcsetfamily",
                "launchType": test.launch_type,
                "networkConfiguration": test.request(),
            }),
            case,
        )
        .await;
        let created = created["taskSet"].clone();
        assert_stored_configuration(&created, "CreateTaskSet", test);

        let read = describe_only_task_set(&router, cluster, &service_name, case).await;
        assert_eq!(
            read["taskSetArn"], created["taskSetArn"],
            "case {case:?}: the read reports the task set that was created, got {read}"
        );
        assert_stored_configuration(&read, "DescribeTaskSets", test);
    }
}

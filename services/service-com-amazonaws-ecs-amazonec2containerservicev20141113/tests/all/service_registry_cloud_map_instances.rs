// SPDX-License-Identifier: CC0-1.0

//! The Cloud Map instances Amazon ECS registers for the tasks of a service that names a
//! `serviceRegistries` entry, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`] with the Cloud Map (service discovery) service mounted beside this
//! one on ONE engine, the way the production router mounts them.
//!
//! The rule under test: `ServiceRegistry` is "The details for the service registry ... When
//! you add, update, or remove the service registries configuration, Amazon ECS starts a new
//! deployment. New tasks are registered and deregistered to the updated service registry
//! configuration" — so a Cloud Map service an ECS service registers into holds one instance
//! per live task of that service, at the address the task's own elastic network interface
//! was given and the port the entry named, and holds none for a task that has stopped or a
//! service that has been deleted.
//!
//! The rows differ along how the entry names its port (`port` and the
//! `containerName`+`containerPort` pair), along how many tasks the service runs, and along
//! what changes the registration (a create, a scale down, a delete) — and the instances are
//! read back through CLOUD MAP's own reads (`DiscoverInstances`, `ListInstances` and
//! `GetService`'s `InstanceCount`), so an instance that is not in Cloud Map's own store
//! cannot pass. An instance registered through Cloud Map's own `RegisterInstance` is one
//! this reconcile must never touch, which is its own row.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cloud_map, cluster_with_capacity, count, list, ok, string,
};

/// The cluster every service below runs in.
const CLUSTER: &str = "registryinstances";

/// The `awsvpc` family every service runs, its one container and the port that container
/// publishes.
const FAMILY: &str = "registryinstancefamily";
const CONTAINER: &str = "app";
const CONTAINER_PORT: i64 = 80;

/// The `port` an entry names when it names one directly instead of a container pair.
const REGISTRY_PORT: i64 = 8080;

/// The private DNS namespace the registries live in, and a VPC id of the shape
/// `CreatePrivateDnsNamespace$Vpc` takes.
const NAMESPACE: &str = "registryinstances.example.com";
const VPC: &str = "vpc-0123456789abcdef0";

/// Long enough for every clock-owed transition an `awsvpc` task's start takes to fall due
/// ([`crate::ecs_wire::AWSVPC_START`]): the interface it was allocated is created and
/// attached, and only then is the task placed and started.
const TWO_SPANS: Duration = crate::ecs_wire::AWSVPC_START;

/// A Cloud Map service in [`NAMESPACE`] publishing an `SRV` record, and its `(Id, Arn)`.
async fn srv_registry(router: &MultiServiceRouter, name: &str, case: &str) -> (String, String) {
    let created = cloud_map(
        router,
        "CreatePrivateDnsNamespace",
        json!({ "Name": NAMESPACE, "Vpc": VPC }),
        case,
    )
    .await;
    let filed = cloud_map(
        router,
        "GetOperation",
        json!({ "OperationId": string(&created, "OperationId", case) }),
        case,
    )
    .await;
    let namespace_id = string(&filed["Operation"]["Targets"], "NAMESPACE", case);
    let service = cloud_map(
        router,
        "CreateService",
        json!({
            "Name": name,
            "NamespaceId": namespace_id,
            // `DnsRecord$Type` is the model's `RecordType` enum; `SRV` is the one that
            // publishes a port beside the address.
            "DnsConfig": { "DnsRecords": [{ "Type": "SRV", "TTL": 60 }] },
        }),
        case,
    )
    .await;
    (
        string(&service["Service"], "Id", case),
        string(&service["Service"], "Arn", case),
    )
}

/// A cluster with capacity, one `awsvpc` revision and one `SRV` Cloud Map service.
async fn fixture(registry_name: &str) -> (MultiServiceRouter, String, String) {
    let case = "fixture";
    let router = crate::ecs_wire::emulator_with_cloud_map();
    cluster_with_capacity(&router, CLUSTER, case).await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
                "portMappings": [{ "containerPort": CONTAINER_PORT, "protocol": "tcp" }],
            }],
        }),
        case,
    )
    .await;
    let (service_id, registry_arn) = srv_registry(&router, registry_name, case).await;
    (router, service_id, registry_arn)
}

/// Create one ECS service with `registry` attached, running `desired_count` tasks.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    registry: Value,
    desired_count: i64,
    case: &str,
) {
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": FAMILY,
            "desiredCount": desired_count,
            "launchType": "EC2",
            "networkConfiguration": awsvpc_network_configuration(),
            "serviceRegistries": [registry],
        }),
        case,
    )
    .await;
}

/// The addresses the tasks of `service` hold, read off the interfaces their own attachments
/// report. The `DescribeTasks` call is also what materialises the registrations the clock
/// has made due.
async fn task_addresses(router: &MultiServiceRouter, service: &str, case: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": service }),
        case,
    )
    .await;
    let arns: Vec<Value> = list(&listed, "taskArns", case);
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    let mut addresses: Vec<String> = Vec::new();
    for task in list(&described, "tasks", case) {
        for attachment in list(&task, "attachments", case) {
            if let Some(address) = list(&attachment, "details", case)
                .into_iter()
                .find(|entry| entry["name"].as_str() == Some("privateIPv4Address"))
                .and_then(|entry| entry["value"].as_str().map(str::to_string))
            {
                addresses.push(address);
            }
        }
    }
    addresses.sort();
    addresses
}

/// The instances Cloud Map's own `DiscoverInstances` reports for `registry_name`, keyed by
/// instance id, each with its attributes.
async fn discovered(
    router: &MultiServiceRouter,
    registry_name: &str,
    case: &str,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let answer = cloud_map(
        router,
        "DiscoverInstances",
        json!({ "NamespaceName": NAMESPACE, "ServiceName": registry_name }),
        case,
    )
    .await;
    list(&answer, "Instances", case)
        .into_iter()
        .map(|instance| {
            let id = string(&instance, "InstanceId", case);
            let attributes = instance["Attributes"]
                .as_object()
                .map(|map| {
                    map.iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            (id, attributes)
        })
        .collect()
}

/// `GetService`'s `InstanceCount` for `service_id` — Cloud Map's own count of what it holds.
async fn instance_count(router: &MultiServiceRouter, service_id: &str, case: &str) -> i64 {
    let read = cloud_map(router, "GetService", json!({ "Id": service_id }), case).await;
    count(&read["Service"], "InstanceCount", case)
}

#[tokio::test]
async fn a_services_tasks_are_registered_in_the_cloud_map_service_its_registry_names() {
    // The rows differ along how the entry names its port and along how many tasks the
    // service runs, so the registered `AWS_INSTANCE_PORT` is asserted to come from the
    // entry whichever of its two forms named it.
    let rows: [(&str, Value, i64, String); 3] = [
        (
            "the containerName and containerPort pair, one task",
            json!({ "containerName": CONTAINER, "containerPort": CONTAINER_PORT }),
            1,
            CONTAINER_PORT.to_string(),
        ),
        (
            "the containerName and containerPort pair, three tasks",
            json!({ "containerName": CONTAINER, "containerPort": CONTAINER_PORT }),
            3,
            CONTAINER_PORT.to_string(),
        ),
        (
            "a port value, two tasks",
            json!({ "port": REGISTRY_PORT }),
            2,
            REGISTRY_PORT.to_string(),
        ),
    ];

    for (index, (case, entry, desired_count, expected_port)) in rows.iter().enumerate() {
        let registry_name = format!("regsrv{index}");
        let (router, registry_id, registry_arn) = fixture(&registry_name).await;
        let service_name = format!("regsvc{index}");
        let mut registry = entry.clone();
        registry["registryArn"] = json!(registry_arn);
        create_service(&router, &service_name, registry, *desired_count, case).await;

        // Amazon ECS registers a task's instance once the task is RUNNING, so a service
        // whose tasks are still starting publishes nothing a query could be answered with.
        assert!(
            discovered(&router, &registry_name, case).await.is_empty(),
            "case {case:?}: a task that is not running yet is not a discoverable instance"
        );

        tokio::time::sleep(TWO_SPANS).await;
        let addresses = task_addresses(&router, &service_name, case).await;
        assert_eq!(
            addresses.len() as i64,
            *desired_count,
            "case {case:?}: expected {desired_count} task address(es), got {addresses:?}"
        );

        let instances = discovered(&router, &registry_name, case).await;
        let mut registered: Vec<String> = instances
            .values()
            .map(|attributes| attributes["AWS_INSTANCE_IPV4"].clone())
            .collect();
        registered.sort();
        assert_eq!(
            registered, addresses,
            "case {case:?}: Cloud Map must hold one instance per task, at the task's own \
             interface address: expected {addresses:?}, got {instances:?}"
        );
        for (instance_id, attributes) in &instances {
            assert_eq!(
                attributes.get("AWS_INSTANCE_PORT"),
                Some(expected_port),
                "case {case:?}: the registered port comes from the registry entry, got \
                 {attributes:?}"
            );
            assert_eq!(
                attributes.get("ECS_SERVICE_NAME"),
                Some(&service_name),
                "case {case:?}: the instance names the ECS service it was registered for, \
                 got {attributes:?}"
            );
            assert_eq!(
                attributes.get("ECS_CLUSTER_NAME").map(String::as_str),
                Some(CLUSTER),
                "case {case:?}: the instance names the cluster, got {attributes:?}"
            );
            assert_eq!(
                attributes.get("ECS_TASK_DEFINITION_FAMILY").map(String::as_str),
                Some(FAMILY),
                "case {case:?}: the instance names the task-definition family, got \
                 {attributes:?}"
            );
            assert!(
                !instance_id.is_empty(),
                "case {case:?}: every instance carries an id, got {instances:?}"
            );
        }

        // And Cloud Map counts them as its own.
        assert_eq!(
            instance_count(&router, &registry_id, case).await,
            *desired_count,
            "case {case:?}: GetService must count the instances Cloud Map holds"
        );

        // Reading again registers nothing twice: the reconcile is idempotent.
        let again = discovered(&router, &registry_name, case).await;
        assert_eq!(
            again, instances,
            "case {case:?}: a second read reports the same instances, got {again:?}"
        );
    }
}

#[tokio::test]
async fn scaling_down_and_deleting_a_service_deregisters_the_instances_it_held() {
    let case = "deregistration";
    let registry_name = "deregsrv";
    let (router, registry_id, registry_arn) = fixture(registry_name).await;
    let service_name = "deregsvc";
    create_service(
        &router,
        service_name,
        json!({
            "registryArn": registry_arn,
            "containerName": CONTAINER,
            "containerPort": CONTAINER_PORT,
        }),
        3,
        case,
    )
    .await;

    // An instance registered through CLOUD MAP's own API, which no ECS reconcile may touch.
    cloud_map(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": registry_id,
            "InstanceId": "hand-registered",
            "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.9", "AWS_INSTANCE_PORT": "443" },
        }),
        case,
    )
    .await;

    tokio::time::sleep(TWO_SPANS).await;
    task_addresses(&router, service_name, case).await;
    let instances = discovered(&router, registry_name, case).await;
    assert_eq!(
        instances.len(),
        4,
        "case {case:?}: three tasks plus the hand-registered instance, got {instances:?}"
    );

    // The rows walk the registration DOWN: a scale to one task, then a delete.
    let rows: [(&str, i64); 3] = [("a scale to one task", 1), ("a scale to none", 0), ("a delete", 0)];
    for (step, desired_count) in rows {
        if step == "a delete" {
            ok(
                &router,
                "DeleteService",
                json!({ "cluster": CLUSTER, "service": service_name, "force": true }),
                step,
            )
            .await;
        } else {
            ok(
                &router,
                "UpdateService",
                json!({
                    "cluster": CLUSTER,
                    "service": service_name,
                    "desiredCount": desired_count,
                }),
                step,
            )
            .await;
        }

        let instances = discovered(&router, registry_name, step).await;
        let ecs_owned: Vec<&String> = instances
            .iter()
            .filter(|(_, attributes)| attributes.contains_key("ECS_SERVICE_NAME"))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ecs_owned.len() as i64,
            desired_count,
            "case {step:?}: expected {desired_count} ECS-registered instance(s), got \
             {instances:?}"
        );
        assert!(
            instances.contains_key("hand-registered"),
            "case {step:?}: an instance registered through Cloud Map's own API must be left \
             alone, got {instances:?}"
        );
    }

    // Cloud Map's own count agrees: only the hand-registered instance is left.
    assert_eq!(
        instance_count(&router, registry_id.as_str(), case).await,
        1,
        "case {case:?}: GetService must count only what Cloud Map still holds"
    );
}

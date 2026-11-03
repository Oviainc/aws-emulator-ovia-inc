// SPDX-License-Identifier: CC0-1.0

//! Reading an ECS resource's TAGS back through `ListTagsForResource`, driven end to end
//! over HTTP.
//!
//! The rule under test: `ListTagsForResource` is a read — for a `resourceArn` naming a
//! resource this service holds it answers 200 with that resource's OWN stored tag list,
//! whichever kind of ECS resource the ARN names and whoever wrote the tags, and the list
//! is the same one the resource's own describe reports under `TAGS` (there is one tag set
//! per resource, not a copy per reader). An ARN that names no held resource is refused
//! with the modelled error its KIND already earns elsewhere in this service, and an ARN
//! that is not a taggable ECS resource ARN at all is refused as the bad parameter it is.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, identity_document, list, ok, refusal, register_definition, service,
    string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster the resources that live INSIDE a cluster (a service, a task, a container
/// instance) are created in.
pub(crate) const CLUSTER: &str = "demo";

/// The family of the task-definition revision the service and the task below run.
pub(crate) const RUNNABLE_FAMILY: &str = "runnable";

/// A container image, as a container definition names one.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// One kind of ECS resource that carries its own tag set. These are the kinds the
/// `resourceArn` docs name: "Currently, the supported resources are Amazon ECS tasks,
/// services, task definitions, clusters, and container instances"
/// (`ListTagsForResource$resourceArn`), plus the capacity provider
/// `TagResource$resourceArn` adds.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    CapacityProvider,
    Cluster,
    ContainerInstance,
    Service,
    Task,
    TaskDefinition,
}

/// Create one resource of `kind` carrying `tags` through that kind's OWN create
/// operation, and return the ARN that create minted.
///
/// `name` is the identifier the kind's create takes — a cluster name, a capacity-provider
/// name, a task-definition family, a service name or an EC2 instance id. A task has no
/// caller-chosen name, so the row's value is unused for [`Kind::Task`].
pub(crate) async fn create(
    router: &MultiServiceRouter,
    kind: Kind,
    name: &str,
    tags: &Value,
    case: &str,
) -> String {
    match kind {
        Kind::CapacityProvider => {
            let body = ok(
                router,
                "CreateCapacityProvider",
                json!({
                    "name": name,
                    "autoScalingGroupProvider": { "autoScalingGroupArn": format!("asg-{name}") },
                    "tags": tags,
                }),
                case,
            )
            .await;
            string(&body["capacityProvider"], "capacityProviderArn", case)
        }
        Kind::Cluster => {
            let body = ok(
                router,
                "CreateCluster",
                json!({ "clusterName": name, "tags": tags }),
                case,
            )
            .await;
            string(&body["cluster"], "clusterArn", case)
        }
        Kind::ContainerInstance => {
            let body = ok(
                router,
                "RegisterContainerInstance",
                json!({
                    "cluster": CLUSTER,
                    "instanceIdentityDocument": identity_document(name),
                    "instanceIdentityDocumentSignature":
                        crate::ecs_wire::identity_document_signature(),
                    "tags": tags,
                }),
                case,
            )
            .await;
            string(&body["containerInstance"], "containerInstanceArn", case)
        }
        Kind::Service => {
            let body = ok(
                router,
                "CreateService",
                json!({
                    "cluster": CLUSTER,
                    "serviceName": name,
                    "taskDefinition": RUNNABLE_FAMILY,
                    "desiredCount": 0,
                    "launchType": "EC2",
                    "tags": tags,
                }),
                case,
            )
            .await;
            string(&body["service"], "serviceArn", case)
        }
        Kind::Task => {
            let body = ok(
                router,
                "RunTask",
                json!({
                    "cluster": CLUSTER,
                    "taskDefinition": RUNNABLE_FAMILY,
                    "count": 1,
                    "tags": tags,
                }),
                case,
            )
            .await;
            let tasks = list(&body, "tasks", case);
            assert_eq!(
                tasks.len(),
                1,
                "case {case:?}: expected exactly one placed task, got {body}"
            );
            string(&tasks[0], "taskArn", case)
        }
        Kind::TaskDefinition => {
            let body = ok(
                router,
                "RegisterTaskDefinition",
                json!({
                    "family": name,
                    "containerDefinitions": [container("app", IMAGE)],
                    "tags": tags,
                }),
                case,
            )
            .await;
            string(&body["taskDefinition"], "taskDefinitionArn", case)
        }
    }
}

/// The `{key, value}` pairs a reported tag list carries, SORTED: two readers of one tag
/// set must report the same pairs, which is what this compares, rather than the order a
/// particular map happened to hold them in.
pub(crate) fn pairs(tags: &[Value], case: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|tag| {
            let key = tag["key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag with no key: {tag}"));
            let value = tag["value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag with no value: {tag}"));
            (key.to_string(), value.to_string())
        })
        .collect();
    pairs.sort();
    pairs
}

/// The tags `ListTagsForResource` reports for one ARN, as a sorted pair list.
pub(crate) async fn listed(
    router: &MultiServiceRouter,
    resource_arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let body = ok(
        router,
        "ListTagsForResource",
        json!({ "resourceArn": resource_arn }),
        case,
    )
    .await;
    pairs(&list(&body, "tags", case), case)
}

/// Create the cluster the in-cluster resources live in, with capacity registered so a
/// task placed with the EC2 launch type has somewhere to run, and one plain
/// task-definition revision for the service and the task to run.
pub(crate) async fn setup(router: &MultiServiceRouter) {
    cluster_with_capacity(router, CLUSTER, "setup").await;
    register_definition(router, RUNNABLE_FAMILY, "setup").await;
}

#[tokio::test]
async fn every_taggable_resource_kind_reports_its_stored_tag_set() {
    let router = service();
    setup(&router).await;

    // The documented maxima the rows below sit on: "Maximum number of tags per resource -
    // 50", "Maximum key length - 128", "Maximum value length - 256"
    // (`smithy_query(query="shape", shape="Tag")`: `key` length 1..128, `value` 0..256,
    // `Tags` length 0..50).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let fifty: Value = (0..50)
        .map(|index| json!({ "key": format!("key{index}"), "value": format!("value{index}") }))
        .collect();

    // The rows differ along the resource KIND, how MANY tags the resource carries (none,
    // one, two, three, and the documented maximum of 50), and the character classes and
    // lengths the key and value take — the punctuation the `Tag` pattern allows
    // (`_.:/=+-@` and spaces), an empty value, and each length bound.
    let rows: Vec<(&str, Kind, String, Value)> = vec![
        (
            "an untagged cluster",
            Kind::Cluster,
            "untagged".to_string(),
            json!([]),
        ),
        (
            "a cluster with one tag",
            Kind::Cluster,
            "onetag".to_string(),
            json!([{ "key": "env", "value": "prod" }]),
        ),
        (
            "a cluster carrying the documented maximum of 50 tags",
            Kind::Cluster,
            "fifty".to_string(),
            fifty,
        ),
        (
            "a cluster whose tag sits on both length bounds, value empty",
            Kind::Cluster,
            "bounds".to_string(),
            json!([
                { "key": longest_key, "value": longest_value },
                { "key": "novalue", "value": "" },
            ]),
        ),
        (
            "a capacity provider with two tags",
            Kind::CapacityProvider,
            "pool".to_string(),
            json!([
                { "key": "team", "value": "platform" },
                { "key": "cost:center", "value": "cc-42" },
            ]),
        ),
        (
            "an untagged task definition",
            Kind::TaskDefinition,
            "plain".to_string(),
            json!([]),
        ),
        (
            "a task definition with three tags spanning the allowed punctuation",
            Kind::TaskDefinition,
            "tagged".to_string(),
            json!([
                { "key": "env", "value": "prod" },
                { "key": "owner.name", "value": "team-a/platform" },
                { "key": "budget=code", "value": "a b+c_d@e" },
            ]),
        ),
        (
            "a container instance with two tags",
            Kind::ContainerInstance,
            "i-0123456789abcdef0".to_string(),
            json!([
                { "key": "fleet", "value": "gpu" },
                { "key": "owner", "value": "team-a" },
            ]),
        ),
        (
            "an untagged container instance",
            Kind::ContainerInstance,
            "i-0fedcba9876543210".to_string(),
            json!([]),
        ),
        (
            "a service with one tag",
            Kind::Service,
            "web".to_string(),
            json!([{ "key": "env", "value": "test" }]),
        ),
        (
            "a task with one tag",
            Kind::Task,
            "unused".to_string(),
            json!([{ "key": "owner", "value": "suite" }]),
        ),
    ];

    for (case, kind, name, tags) in &rows {
        let arn = create(&router, *kind, name, tags, case).await;
        let expected = pairs(
            tags.as_array().expect("the row's tags are a list"),
            case,
        );
        let actual = listed(&router, &arn, case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected ListTagsForResource({arn}) to report the stored tags \
             {expected:?}, got {actual:?}"
        );

        // Reading is not writing: the same ARN read twice reports the same tags.
        let again = listed(&router, &arn, case).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second read of {arn} must report the same tags {expected:?}, \
             got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_tags_listed_are_the_ones_the_resources_own_describe_reports() {
    let router = service();
    setup(&router).await;

    // One tag set, two readers: whatever a resource's own describe reports under `TAGS`
    // is what `ListTagsForResource` reports for its ARN, for every kind whose describe
    // carries a tag set.
    let tags = json!([
        { "key": "env", "value": "prod" },
        { "key": "owner", "value": "team-a" },
    ]);

    let cluster_arn = create(&router, Kind::Cluster, "described", &tags, "cluster").await;
    let definition_arn = create(&router, Kind::TaskDefinition, "described", &tags, "definition").await;
    let provider_arn = create(&router, Kind::CapacityProvider, "described", &tags, "provider").await;
    let instance_arn = create(
        &router,
        Kind::ContainerInstance,
        "i-0abcdef0123456789",
        &tags,
        "instance",
    )
    .await;
    let service_arn = create(&router, Kind::Service, "described", &tags, "service").await;
    let task_arn = create(&router, Kind::Task, "unused", &tags, "task").await;

    // `(case, the ARN, the describe request, where the described record sits in the
    // response)`. Every describe asks for `TAGS`: "If TAGS is specified, the tags are
    // included in the response."
    let rows: Vec<(&str, &str, &str, Value, Vec<&str>)> = vec![
        (
            "cluster",
            &cluster_arn,
            "DescribeClusters",
            json!({ "clusters": ["described"], "include": ["TAGS"] }),
            vec!["clusters"],
        ),
        (
            "task definition",
            &definition_arn,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": "described", "include": ["TAGS"] }),
            vec![],
        ),
        (
            "capacity provider",
            &provider_arn,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": ["described"], "include": ["TAGS"] }),
            vec!["capacityProviders"],
        ),
        (
            "container instance",
            &instance_arn,
            "DescribeContainerInstances",
            json!({
                "cluster": CLUSTER,
                "containerInstances": [instance_arn.clone()],
                "include": ["TAGS"],
            }),
            vec!["containerInstances"],
        ),
        (
            "service",
            &service_arn,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": ["described"], "include": ["TAGS"] }),
            vec!["services"],
        ),
        (
            "task",
            &task_arn,
            "DescribeTasks",
            json!({ "cluster": CLUSTER, "tasks": [task_arn.clone()], "include": ["TAGS"] }),
            vec!["tasks"],
        ),
    ];

    for (case, arn, operation, request, path) in &rows {
        let body = ok(&router, operation, request.clone(), case).await;
        // The described record is either the response itself (`DescribeTaskDefinition`
        // reports its tags beside the definition) or the single entry of the response's
        // one list member.
        let described = match path.first() {
            Some(member) => {
                let entries = list(&body, member, case);
                assert_eq!(
                    entries.len(),
                    1,
                    "case {case:?}: expected exactly one described record, got {body}"
                );
                entries[0].clone()
            }
            None => body.clone(),
        };
        let described_tags = pairs(&list(&described, "tags", case), case);
        let listed_tags = listed(&router, arn, case).await;
        assert_eq!(
            listed_tags, described_tags,
            "case {case:?}: ListTagsForResource({arn}) reported {listed_tags:?} but the \
             resource's own describe reports {described_tags:?}"
        );
        assert!(
            !listed_tags.is_empty(),
            "case {case:?}: the resource was created with tags, so neither reader may \
             report an empty list, got {listed_tags:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_held_resource_is_refused_by_its_kind() {
    let router = service();
    setup(&router).await;

    let ecs_arn = |resource: &str| format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:{resource}");

    // The rows span every taggable kind with a well-formed ARN naming nothing, then the
    // values that are not a taggable ECS resource ARN at all: another service's
    // namespace, an ECS resource kind that carries no tag set of its own, another
    // partition's ECS ARN for a resource nothing was created in, and a value that is no
    // ARN. The ERROR CODE is the requirement; each sentence is the one this service
    // already answers for that condition.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a cluster that was never created",
            ecs_arn("cluster/ghost"),
            "ClusterNotFoundException",
        ),
        (
            "a task-definition revision that was never registered",
            ecs_arn("task-definition/ghost:1"),
            "ClientException",
        ),
        (
            "a revision of a family that exists but was never registered",
            ecs_arn(&format!("task-definition/{RUNNABLE_FAMILY}:9")),
            "ClientException",
        ),
        (
            "a service that was never created",
            ecs_arn(&format!("service/{CLUSTER}/ghost")),
            "InvalidParameterException",
        ),
        (
            "a task that was never run",
            ecs_arn(&format!(
                "task/{CLUSTER}/00000000000000000000000000000000"
            )),
            "InvalidParameterException",
        ),
        (
            "a container instance that was never registered",
            ecs_arn(&format!(
                "container-instance/{CLUSTER}/00000000000000000000000000000000"
            )),
            "InvalidParameterException",
        ),
        (
            "a capacity provider that was never created",
            ecs_arn("capacity-provider/ghost"),
            "InvalidParameterException",
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{SIGNED_REGION}:{ACCOUNT}:some-queue"),
            "InvalidParameterException",
        ),
        (
            "an ECS resource kind that carries no tag set",
            ecs_arn(&format!(
                "container/{CLUSTER}/00000000000000000000000000000000/0123456789abcdef"
            )),
            "InvalidParameterException",
        ),
        (
            // Another partition's ARN carries another Region too, and this endpoint
            // resolves only its own Region's resources, so the ARN is what is refused.
            "an ECS ARN of another partition, for a cluster nothing created",
            format!("arn:aws-cn:ecs:cn-north-1:{ACCOUNT}:cluster/{CLUSTER}"),
            "InvalidParameterException",
        ),
        (
            "a value that is no ARN at all",
            CLUSTER.to_string(),
            "InvalidParameterException",
        ),
    ];

    for (case, resource_arn, expected) in &rows {
        let (code, message) = refusal(
            &router,
            "ListTagsForResource",
            json!({ "resourceArn": resource_arn }),
            case,
        )
        .await;
        assert_eq!(
            &code, expected,
            "case {case:?}: expected {expected} for {resource_arn}, got {code} \
             ({message:?})"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: every refusal carries a message, got {message:?}"
        );
    }
}

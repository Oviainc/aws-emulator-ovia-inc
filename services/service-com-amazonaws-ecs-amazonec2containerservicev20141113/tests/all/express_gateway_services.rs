// SPDX-License-Identifier: CC0-1.0

//! The ECS EXPRESS service lifecycle, driven end to end over HTTP:
//! `CreateExpressGatewayService`, `DescribeExpressGatewayService`,
//! `UpdateExpressGatewayService` and `DeleteExpressGatewayService`.
//!
//! The rules under test:
//!
//! 1. An Express service is a persisted, cluster-scoped record whose one active
//!    configuration is RENDERED from the request: every member the request leaves out
//!    takes the value AWS documents as its default (`cpu` 256, `memory` 512,
//!    `cpuArchitecture` X86_64, `healthCheckPath` /ping, the container port 80, the
//!    auto-scaling target value 60), and a service the request did not name gets a name
//!    Amazon ECS generates.
//! 2. A revision runs EITHER a task definition the caller manages or the container this
//!    service renders one for: "If you provide a task definition ARN, you cannot also
//!    specify primaryContainer, executionRoleArn, taskRoleArn, cpu, memory, or
//!    cpuArchitecture."
//! 3. The three operations that address a service by `serviceArn` alone read the CLUSTER
//!    out of that ARN, so an ARN naming a cluster the account does not hold is refused as
//!    a missing cluster, exactly as a `cluster` member naming one is.
//! 4. An update rolls out a NEW revision: the members the request names change, the ones
//!    it leaves out keep what the service ran, and both revisions stay active until the
//!    rollout the CLOCK owes falls due.
//! 5. A delete drains the service (`DRAINING`) and the CLOCK removes it, after which a
//!    describe reports it as gone and its name is free again.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    count, create_cluster, list, ok, refusal, register_definition, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The fixed `ClusterNotFoundException` sentence every ECS operation that resolves a
/// cluster carries (`state`'s `CLUSTER_NOT_FOUND_MESSAGE`).
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";

/// The fixed sentence an Express operation reports a service that is not there with
/// (`state`'s `EXPRESS_GATEWAY_SERVICE_NOT_FOUND_MESSAGE`): the Express operations' own
/// wording for the class, with no trailing period.
const SERVICE_NOT_FOUND: &str = "Service not found";

/// The refusal an Express operation reports an unresolvable `serviceArn` with
/// (`state`'s `express_resource_not_found_message`): fixed text plus the identifier the
/// request named.
fn resource_not_found(reference: &str) -> String {
    format!("Resource not found: {reference}")
}

/// The documented defaults the create applies, from the `CreateExpressGatewayService`
/// member docs ("The default value for an Express service is 256 (.25 vCPU)", "The
/// default value for an express service is 512 MiB", "If you don't specify a value, the
/// default is X86_64", "the default health check path is /ping", "the container port
/// (defaults to 80)", "the default target value for an Express service is 60").
const DEFAULT_CPU: &str = "256";
const DEFAULT_MEMORY: &str = "512";
const DEFAULT_ARCHITECTURE: &str = "X86_64";
const DEFAULT_HEALTH_CHECK_PATH: &str = "/ping";
const DEFAULT_CONTAINER_PORT: i64 = 80;
const DEFAULT_SCALING_TARGET_VALUE: i64 = 60;

/// The infrastructure role every case passes: "This role is only used during Express
/// service creation, updates, and deletion operations" — ECS stores the ARN it is given.
pub(crate) fn infrastructure_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/ecsInfrastructureRole")
}

/// The ARN an Express service of `name` in `cluster` carries, as a client builds one for
/// the operations that take only a `serviceArn`.
fn express_arn(cluster: &str, name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{cluster}/{name}")
}

/// Create one Express service and return the service it reports.
pub(crate) async fn create_express(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> Value {
    let body = ok(router, "CreateExpressGatewayService", request, case).await;
    body["service"].clone()
}

/// Describe one Express service and return it.
async fn describe_express(
    router: &MultiServiceRouter,
    service_arn: &str,
    include_tags: bool,
    case: &str,
) -> Value {
    let mut request = json!({ "serviceArn": service_arn });
    if include_tags {
        request["include"] = json!(["TAGS"]);
    }
    let body = ok(router, "DescribeExpressGatewayService", request, case).await;
    body["service"].clone()
}

/// The service's single active configuration, asserting there is exactly one.
fn only_configuration(service: &Value, case: &str) -> Value {
    let configurations = list(service, "activeConfigurations", case);
    assert_eq!(
        configurations.len(),
        1,
        "case {case:?}: expected exactly one active configuration, got {configurations:?}"
    );
    configurations[0].clone()
}

/// The status code a service reports.
pub(crate) fn status_code(service: &Value, case: &str) -> String {
    string(&service["status"], "statusCode", case)
}

/// One row of the create table: which members the request names. A member left `None` is
/// the one whose documented default the row proves.
#[derive(Debug)]
struct CreateCase {
    case: &'static str,
    /// The cluster the service is created in.
    cluster: &'static str,
    /// Whether the row CREATES that cluster first. The row that does not names the
    /// account's `default` cluster, which is there without anybody creating it.
    precreate_cluster: bool,
    /// Whether the request NAMES that cluster; a row that does not relies on "If you do
    /// not specify a cluster, the default cluster is assumed."
    name_cluster: bool,
    service_name: Option<&'static str>,
    cpu: Option<&'static str>,
    memory: Option<&'static str>,
    architecture: Option<&'static str>,
    health_check_path: Option<&'static str>,
    container_port: Option<i64>,
    scaling_target_value: Option<i64>,
    tags: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_created_express_service_is_stored_with_the_documented_defaults() {
    // The rows differ along WHICH members the request names — none beyond the required
    // role and container, all of them, a query-string health path (the member doc's own
    // example `/ping?format=json`), and a request that names no cluster at all.
    let cases = [
        CreateCase {
            case: "minimal",
            cluster: "express-minimal",
            precreate_cluster: true,
            name_cluster: true,
            service_name: Some("storefront"),
            cpu: None,
            memory: None,
            architecture: None,
            health_check_path: None,
            container_port: None,
            scaling_target_value: None,
            tags: &[],
        },
        CreateCase {
            case: "every-member-named",
            cluster: "express-full",
            precreate_cluster: true,
            name_cluster: true,
            service_name: Some("checkout_api-1"),
            cpu: Some("1024"),
            memory: Some("2048"),
            architecture: Some("ARM64"),
            health_check_path: Some("/health"),
            container_port: Some(8080),
            scaling_target_value: Some(75),
            tags: &[("team", "payments"), ("env", "prod")],
        },
        CreateCase {
            case: "generated-name-and-query-health-path",
            cluster: "express-generated",
            precreate_cluster: true,
            name_cluster: true,
            service_name: None,
            cpu: Some("512"),
            memory: None,
            architecture: None,
            health_check_path: Some("/ping?format=json"),
            container_port: Some(3000),
            scaling_target_value: None,
            tags: &[("owner", "platform")],
        },
        CreateCase {
            case: "default-cluster-assumed",
            cluster: "default",
            precreate_cluster: false,
            name_cluster: false,
            service_name: Some("edge"),
            cpu: None,
            memory: None,
            architecture: None,
            health_check_path: None,
            container_port: None,
            scaling_target_value: None,
            tags: &[],
        },
        CreateCase {
            case: "default-cluster-named",
            cluster: "default",
            precreate_cluster: false,
            name_cluster: true,
            service_name: Some("edge-named"),
            cpu: None,
            memory: None,
            architecture: None,
            health_check_path: None,
            container_port: None,
            scaling_target_value: None,
            tags: &[],
        },
    ];

    for row in cases {
        let case = row.case;
        let router = service();
        let cluster_arn = if row.precreate_cluster {
            create_cluster(&router, row.cluster, case).await
        } else {
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{}", row.cluster)
        };

        let mut container = json!({ "image": "public.ecr.aws/nginx/nginx:latest" });
        if let Some(port) = row.container_port {
            container["containerPort"] = json!(port);
        }
        let mut request = json!({
            "infrastructureRoleArn": infrastructure_role(),
            "primaryContainer": container,
        });
        if row.name_cluster {
            request["cluster"] = json!(row.cluster);
        }
        if let Some(name) = row.service_name {
            request["serviceName"] = json!(name);
        }
        if let Some(cpu) = row.cpu {
            request["cpu"] = json!(cpu);
        }
        if let Some(memory) = row.memory {
            request["memory"] = json!(memory);
        }
        if let Some(architecture) = row.architecture {
            request["cpuArchitecture"] = json!(architecture);
        }
        if let Some(path) = row.health_check_path {
            request["healthCheckPath"] = json!(path);
        }
        if let Some(target) = row.scaling_target_value {
            request["scalingTarget"] = json!({
                "minTaskCount": 1,
                "maxTaskCount": 4,
                "autoScalingMetric": "AVERAGE_CPU",
                "autoScalingTargetValue": target,
            });
        }
        if !row.tags.is_empty() {
            request["tags"] = Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            );
        }

        let created = create_express(&router, request, case).await;

        let name = string(&created, "serviceName", case);
        match row.service_name {
            Some(expected) => assert_eq!(
                name, expected,
                "case {case:?}: expected the requested serviceName, got {name:?}"
            ),
            // "If you don't specify a service name, Amazon ECS generates a unique name."
            None => assert!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
                "case {case:?}: expected a generated service name, got {name:?}"
            ),
        }
        assert_eq!(
            string(&created, "serviceArn", case),
            express_arn(row.cluster, &name),
            "case {case:?}: expected the service ARN to name its cluster and service, got {created}"
        );
        assert_eq!(
            string(&created, "cluster", case),
            cluster_arn,
            "case {case:?}: expected the hosting cluster, got {created}"
        );
        assert_eq!(
            string(&created, "infrastructureRoleArn", case),
            infrastructure_role(),
            "case {case:?}: expected the infrastructure role the request named, got {created}"
        );
        assert_eq!(
            status_code(&created, case),
            "ACTIVE",
            "case {case:?}: expected a created Express service to be ACTIVE, got {created}"
        );
        let deployment = string(&created, "currentDeployment", case);
        assert!(
            !deployment.is_empty(),
            "case {case:?}: expected a current deployment, got {created}"
        );

        let configuration = only_configuration(&created, case);
        let revision_arn = string(&configuration, "serviceRevisionArn", case);
        assert!(
            revision_arn.contains(&format!("service-revision/{}/{name}/", row.cluster)),
            "case {case:?}: expected a service revision ARN of this service, got {revision_arn:?}"
        );
        assert_eq!(
            string(&configuration, "cpu", case),
            row.cpu.unwrap_or(DEFAULT_CPU),
            "case {case:?}: expected the requested or defaulted cpu, got {configuration}"
        );
        assert_eq!(
            string(&configuration, "memory", case),
            row.memory.unwrap_or(DEFAULT_MEMORY),
            "case {case:?}: expected the requested or defaulted memory, got {configuration}"
        );
        assert_eq!(
            string(&configuration, "cpuArchitecture", case),
            row.architecture.unwrap_or(DEFAULT_ARCHITECTURE),
            "case {case:?}: expected the requested or defaulted architecture, got {configuration}"
        );
        assert_eq!(
            string(&configuration, "healthCheckPath", case),
            row.health_check_path.unwrap_or(DEFAULT_HEALTH_CHECK_PATH),
            "case {case:?}: expected the requested or defaulted health check path, got \
             {configuration}"
        );
        assert_eq!(
            count(&configuration["primaryContainer"], "containerPort", case),
            row.container_port.unwrap_or(DEFAULT_CONTAINER_PORT),
            "case {case:?}: expected the requested or defaulted container port, got \
             {configuration}"
        );
        assert_eq!(
            count(
                &configuration["scalingTarget"],
                "autoScalingTargetValue",
                case
            ),
            row.scaling_target_value
                .unwrap_or(DEFAULT_SCALING_TARGET_VALUE),
            "case {case:?}: expected the requested or defaulted scaling target value, got \
             {configuration}"
        );
        assert!(
            configuration["taskDefinitionArn"].is_null(),
            "case {case:?}: expected no task definition on a container-rendered revision, got \
             {configuration}"
        );

        // The create has no `include` member, so it reports the tags it stored; a
        // describe reports them only where the request asks for them.
        let created_tags = list(&created, "tags", case);
        assert_eq!(
            created_tags.len(),
            row.tags.len(),
            "case {case:?}: expected the created service to carry its tags, got {created}"
        );

        let arn = string(&created, "serviceArn", case);
        let described = describe_express(&router, &arn, false, case).await;
        assert!(
            list(&described, "tags", case).is_empty(),
            "case {case:?}: expected no tags without include=TAGS, got {described}"
        );
        let with_tags = describe_express(&router, &arn, true, case).await;
        assert_eq!(
            list(&with_tags, "tags", case),
            created_tags,
            "case {case:?}: expected include=TAGS to report the stored tags, got {with_tags}"
        );
        // The read is of the record the create wrote, so every projected member agrees.
        assert_eq!(
            only_configuration(&with_tags, case),
            configuration,
            "case {case:?}: expected the described configuration to be the stored one, got \
             {with_tags}"
        );
        assert_eq!(
            described["createdAt"], created["createdAt"],
            "case {case:?}: expected createdAt to be stored, got {described}"
        );
        assert_eq!(
            described["updatedAt"], created["createdAt"],
            "case {case:?}: expected a service nothing has updated to report its creation \
             instant, got {described}"
        );
    }
}

#[tokio::test]
async fn a_revision_runs_either_a_task_definition_or_a_container_never_both() {
    let case = "task-definition-revision";
    let router = service();
    create_cluster(&router, "express-td", case).await;
    let task_definition = register_definition(&router, "express-family", case).await;

    let created = create_express(
        &router,
        json!({
            "cluster": "express-td",
            "serviceName": "managed-definition",
            "infrastructureRoleArn": infrastructure_role(),
            "taskDefinitionArn": task_definition,
        }),
        case,
    )
    .await;

    let configuration = only_configuration(&created, case);
    assert_eq!(
        string(&configuration, "taskDefinitionArn", case),
        task_definition,
        "case {case:?}: expected the revision to run the task definition named, got \
         {configuration}"
    );
    for absent in [
        "cpu",
        "memory",
        "cpuArchitecture",
        "primaryContainer",
        "executionRoleArn",
        "taskRoleArn",
    ] {
        assert!(
            configuration[absent].is_null(),
            "case {case:?}: expected no {absent} on a task-definition revision, got \
             {configuration}"
        );
    }
    // The members the task definition itself carries are still defaulted where they
    // belong to the service rather than to the revision's containers.
    assert_eq!(
        string(&configuration, "healthCheckPath", case),
        DEFAULT_HEALTH_CHECK_PATH,
        "case {case:?}: expected the defaulted health check path, got {configuration}"
    );

    // Each member the doc excludes is refused BY NAME when it accompanies a task
    // definition ARN — one row per member of the documented list.
    let exclusive = [
        ("primaryContainer", json!({ "image": "public.ecr.aws/nginx/nginx:latest" })),
        ("executionRoleArn", json!(format!("arn:aws:iam::{ACCOUNT}:role/exec"))),
        ("taskRoleArn", json!(format!("arn:aws:iam::{ACCOUNT}:role/task"))),
        ("cpu", json!("512")),
        ("memory", json!("1024")),
        ("cpuArchitecture", json!("ARM64")),
    ];
    for (member, value) in exclusive {
        let case = member;
        let mut request = json!({
            "cluster": "express-td",
            "serviceName": format!("conflict-{member}"),
            "infrastructureRoleArn": infrastructure_role(),
            "taskDefinitionArn": register_definition(&router, "conflict-family", case).await,
        });
        request[member] = value;
        let (code, message) =
            refusal(&router, "CreateExpressGatewayService", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the documented exclusivity refusal, got {code:?}: \
             {message:?}"
        );
        assert!(
            message.contains(member),
            "case {case:?}: expected the refusal to name {member}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn express_operations_on_a_missing_cluster_report_the_cluster_missing() {
    let case = "missing-cluster";
    let router = service();
    // One cluster EXISTS, so a refusal cannot come from an empty store: the ARNs below
    // name another one.
    create_cluster(&router, "express-present", case).await;

    let absent = express_arn("no-such-cluster", "any-service");
    // The same reference in another Region's spelling: the cluster is read out of the
    // ARN's resource segment, not out of its Region or account.
    let absent_other_region =
        format!("arn:aws:ecs:eu-west-1:{ACCOUNT}:service/no-such-cluster/any-service");

    let rows: [(&str, Value); 5] = [
        (
            "CreateExpressGatewayService",
            json!({
                "cluster": "no-such-cluster",
                "serviceName": "any-service",
                "infrastructureRoleArn": infrastructure_role(),
                "primaryContainer": { "image": "public.ecr.aws/nginx/nginx:latest" },
            }),
        ),
        (
            "DescribeExpressGatewayService",
            json!({ "serviceArn": absent.clone() }),
        ),
        (
            "DeleteExpressGatewayService",
            json!({ "serviceArn": absent.clone() }),
        ),
        (
            "UpdateExpressGatewayService",
            json!({ "serviceArn": absent.clone(), "cpu": "512" }),
        ),
        (
            "DescribeExpressGatewayService",
            json!({ "serviceArn": absent_other_region }),
        ),
    ];

    for (operation, request) in rows {
        let (code, message) = refusal(&router, operation, request, operation).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClusterNotFoundException", CLUSTER_NOT_FOUND),
            "case {operation:?}: expected the missing cluster to be reported as that, got \
             {code:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn a_service_the_cluster_does_not_hold_is_reported_not_found() {
    let case = "no-such-service";
    let router = service();
    create_cluster(&router, "express-present", case).await;
    let absent = express_arn("express-present", "never-created");

    // Each operation reports a service that is not there in ITS OWN recorded terms: the
    // delete as a service that was not found, the describe as a resource that was not
    // found, and the update as the identifier it could not resolve.
    let rows: [(&str, Value, &str, String); 3] = [
        (
            "DescribeExpressGatewayService",
            json!({ "serviceArn": absent.clone() }),
            "ResourceNotFoundException",
            SERVICE_NOT_FOUND.to_string(),
        ),
        (
            "DeleteExpressGatewayService",
            json!({ "serviceArn": absent.clone() }),
            "ServiceNotFoundException",
            SERVICE_NOT_FOUND.to_string(),
        ),
        (
            "UpdateExpressGatewayService",
            json!({ "serviceArn": absent.clone(), "cpu": "512" }),
            "InvalidParameterException",
            resource_not_found(&absent),
        ),
    ];
    for (operation, request, expected_code, expected_message) in rows {
        let (code, message) = refusal(&router, operation, request, operation).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (expected_code, expected_message.as_str()),
            "case {operation:?}: expected the service to be reported missing, got {code:?}: \
             {message:?}"
        );
    }

    // A `serviceArn` that names no cluster-and-service pair names no service either,
    // whichever shape it is in.
    for reference in [
        "storefront",
        "arn:aws:ecs:us-west-2:000000000000:service",
        "arn:aws:ecs:us-west-2:000000000000:service/",
        "service/express-present",
    ] {
        let (code, message) = refusal(
            &router,
            "DescribeExpressGatewayService",
            json!({ "serviceArn": reference }),
            reference,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ResourceNotFoundException", SERVICE_NOT_FOUND),
            "case {reference:?}: expected a reference that names no service to be reported \
             not found, got {code:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn an_express_create_is_refused_where_the_request_cannot_be_served() {
    let router = service();
    let case = "refusals";
    create_cluster(&router, "express-refusals", case).await;
    let container = json!({ "image": "public.ecr.aws/nginx/nginx:latest" });

    // A path that is not a path: "The health check path must start with a forward slash".
    for path in ["ping", "health?x=1", ""] {
        let (code, message) = refusal(
            &router,
            "CreateExpressGatewayService",
            json!({
                "cluster": "express-refusals",
                "serviceName": "bad-path",
                "infrastructureRoleArn": infrastructure_role(),
                "primaryContainer": container.clone(),
                "healthCheckPath": path,
            }),
            path,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {path:?}: expected a health check path refusal, got {code:?}: {message:?}"
        );
        assert!(
            message.contains("forward slash"),
            "case {path:?}: expected the refusal to state the rule, got {message:?}"
        );
    }

    // A name outside the pattern every ECS resource name follows.
    for name in ["bad name", "name!", "tabs\tand\tspaces"] {
        let (code, message) = refusal(
            &router,
            "CreateExpressGatewayService",
            json!({
                "cluster": "express-refusals",
                "serviceName": name,
                "infrastructureRoleArn": infrastructure_role(),
                "primaryContainer": container.clone(),
            }),
            name,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {name:?}: expected a service name refusal, got {code:?}: {message:?}"
        );
        assert!(
            message.contains(name) && message.contains("must match"),
            "case {name:?}: expected the pattern refusal to carry the rejected name, got \
             {message:?}"
        );
    }

    // Neither a container nor a task definition says nothing to run.
    let (code, message) = refusal(
        &router,
        "CreateExpressGatewayService",
        json!({
            "cluster": "express-refusals",
            "serviceName": "nothing-to-run",
            "infrastructureRoleArn": infrastructure_role(),
        }),
        "nothing-to-run",
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case \"nothing-to-run\": expected a refusal, got {code:?}: {message:?}"
    );

    // "This name must be unique within the specified cluster."
    let request = json!({
        "cluster": "express-refusals",
        "serviceName": "storefront",
        "infrastructureRoleArn": infrastructure_role(),
        "primaryContainer": container.clone(),
    });
    create_express(&router, request.clone(), "first").await;
    let (code, message) = refusal(
        &router,
        "CreateExpressGatewayService",
        request.clone(),
        "duplicate",
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case \"duplicate\": expected the name to be taken, got {code:?}: {message:?}"
    );
    assert!(
        message.contains("storefront"),
        "case \"duplicate\": expected the refusal to name the service, got {message:?}"
    );

    // The same name in ANOTHER cluster is free: the name is unique within its cluster.
    create_cluster(&router, "express-other", case).await;
    let mut elsewhere = request;
    elsewhere["cluster"] = json!("express-other");
    let created = create_express(&router, elsewhere, "same-name-other-cluster").await;
    assert_eq!(
        string(&created, "serviceArn", case),
        express_arn("express-other", "storefront"),
        "case \"same-name-other-cluster\": expected a service of the other cluster, got {created}"
    );
}

/// One row of the update table: the member it changes.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    /// The member the update names, and the value it names it with.
    member: &'static str,
    request: fn() -> Value,
    /// The member of the resulting configuration to read, and what it must read.
    expect: fn(&Value, &str) -> String,
    expected: &'static str,
}

#[tokio::test]
async fn an_update_rolls_out_a_new_revision_and_keeps_what_it_does_not_name() {
    let cases = [
        UpdateCase {
            case: "cpu",
            member: "cpu",
            request: || json!("1024"),
            expect: |configuration, case| string(configuration, "cpu", case),
            expected: "1024",
        },
        UpdateCase {
            case: "memory",
            member: "memory",
            request: || json!("4096"),
            expect: |configuration, case| string(configuration, "memory", case),
            expected: "4096",
        },
        UpdateCase {
            case: "architecture",
            member: "cpuArchitecture",
            request: || json!("ARM64"),
            expect: |configuration, case| string(configuration, "cpuArchitecture", case),
            expected: "ARM64",
        },
        UpdateCase {
            case: "health-check-path",
            member: "healthCheckPath",
            request: || json!("/api/status"),
            expect: |configuration, case| string(configuration, "healthCheckPath", case),
            expected: "/api/status",
        },
        UpdateCase {
            case: "image",
            member: "primaryContainer",
            request: || json!({ "image": "public.ecr.aws/nginx/nginx:1.27" }),
            expect: |configuration, case| {
                string(&configuration["primaryContainer"], "image", case)
            },
            expected: "public.ecr.aws/nginx/nginx:1.27",
        },
    ];

    for row in cases {
        let case = row.case;
        let router = service();
        create_cluster(&router, "express-updates", case).await;
        let created = create_express(
            &router,
            json!({
                "cluster": "express-updates",
                "serviceName": "storefront",
                "infrastructureRoleArn": infrastructure_role(),
                "primaryContainer": {
                    "image": "public.ecr.aws/nginx/nginx:latest",
                    "containerPort": 8080,
                },
                "memory": "2048",
            }),
            case,
        )
        .await;
        let arn = string(&created, "serviceArn", case);
        let created_configuration = only_configuration(&created, case);
        let created_revision = string(&created_configuration, "serviceRevisionArn", case);

        let mut request = json!({ "serviceArn": arn.clone() });
        request[row.member] = (row.request)();
        let body = ok(&router, "UpdateExpressGatewayService", request, case).await;
        let updated = body["service"].clone();

        assert_eq!(
            string(&updated, "serviceArn", case),
            arn,
            "case {case:?}: expected the same service, got {updated}"
        );
        assert_eq!(
            status_code(&updated, case),
            "ACTIVE",
            "case {case:?}: expected the service to stay ACTIVE through the rollout, got \
             {updated}"
        );
        let target = updated["targetConfiguration"].clone();
        let target_revision = string(&target, "serviceRevisionArn", case);
        assert_ne!(
            target_revision, created_revision,
            "case {case:?}: expected the update to roll out a NEW revision, got \
             {target_revision:?}"
        );
        assert_eq!(
            (row.expect)(&target, case),
            row.expected,
            "case {case:?}: expected the updated {} to be stored, got {target}",
            row.member
        );
        // A member the update did not name keeps what the service ran.
        if row.member != "memory" {
            assert_eq!(
                string(&target, "memory", case),
                "2048",
                "case {case:?}: expected the unnamed memory to be kept, got {target}"
            );
        }
        if row.member != "primaryContainer" {
            assert_eq!(
                count(&target["primaryContainer"], "containerPort", case),
                8080,
                "case {case:?}: expected the unnamed container port to be kept, got {target}"
            );
        }

        // Both revisions are active while the rolling deployment runs, and only the one
        // it rolled out once the clock settles it.
        let during = describe_express(&router, &arn, false, case).await;
        let rolling = list(&during, "activeConfigurations", case);
        assert_eq!(
            rolling.len(),
            2,
            "case {case:?}: expected the replaced revision to stay active during the \
             rollout, got {during}"
        );
        assert_eq!(
            string(&rolling[0], "serviceRevisionArn", case),
            target_revision,
            "case {case:?}: expected the newest revision first, got {during}"
        );

        tokio::time::sleep(ONE_SPAN).await;
        let settled = describe_express(&router, &arn, false, case).await;
        let settled_configuration = only_configuration(&settled, case);
        assert_eq!(
            string(&settled_configuration, "serviceRevisionArn", case),
            target_revision,
            "case {case:?}: expected only the rolled-out revision to stay active, got \
             {settled}"
        );
        assert_eq!(
            settled["createdAt"], created["createdAt"],
            "case {case:?}: expected createdAt to survive an update, got {settled}"
        );
        assert!(
            settled["updatedAt"].as_f64() >= created["createdAt"].as_f64(),
            "case {case:?}: expected updatedAt to move with the update, got {settled}"
        );
    }
}

#[tokio::test]
async fn deleting_an_express_service_drains_it_and_the_clock_removes_it() {
    let case = "delete-then-describe";
    let router = service();
    create_cluster(&router, "express-delete", case).await;
    let request = json!({
        "cluster": "express-delete",
        "serviceName": "storefront",
        "infrastructureRoleArn": infrastructure_role(),
        "primaryContainer": { "image": "public.ecr.aws/nginx/nginx:latest" },
    });
    let created = create_express(&router, request.clone(), case).await;
    let arn = string(&created, "serviceArn", case);

    let body = ok(
        &router,
        "DeleteExpressGatewayService",
        json!({ "serviceArn": arn.clone() }),
        case,
    )
    .await;
    let deleted = body["service"].clone();
    assert_eq!(
        status_code(&deleted, case),
        "DRAINING",
        "case {case:?}: expected the deleted service to enter DRAINING, got {deleted}"
    );
    assert_eq!(
        string(&deleted, "serviceArn", case),
        arn,
        "case {case:?}: expected the deleted service to be the one named, got {deleted}"
    );

    // While it drains it is still the service it was, and a second delete has nothing
    // left to start.
    let draining = describe_express(&router, &arn, false, case).await;
    assert_eq!(
        status_code(&draining, case),
        "DRAINING",
        "case {case:?}: expected the draining service to report that status, got {draining}"
    );
    let (code, message) = refusal(
        &router,
        "DeleteExpressGatewayService",
        json!({ "serviceArn": arn.clone() }),
        "second-delete",
    )
    .await;
    assert_eq!(
        code, "ServiceNotActiveException",
        "case \"second-delete\": expected a draining service to refuse a second delete, got \
         {code:?}: {message:?}"
    );
    let (code, message) = refusal(
        &router,
        "UpdateExpressGatewayService",
        json!({ "serviceArn": arn.clone(), "cpu": "1024" }),
        "update-while-draining",
    )
    .await;
    assert_eq!(
        code, "ServiceNotActiveException",
        "case \"update-while-draining\": expected a draining service to refuse an update, got \
         {code:?}: {message:?}"
    );

    // "After all tasks stop, the service and infrastructure are permanently removed."
    tokio::time::sleep(ONE_SPAN).await;
    let (code, message) = refusal(
        &router,
        "DescribeExpressGatewayService",
        json!({ "serviceArn": arn.clone() }),
        "after-the-drain",
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("ResourceNotFoundException", SERVICE_NOT_FOUND),
        "case \"after-the-drain\": expected the removed service to be reported gone, got \
         {code:?}: {message:?}"
    );

    // And the name it held is free again.
    let recreated = create_express(&router, request, "recreate").await;
    assert_eq!(
        string(&recreated, "serviceArn", case),
        arn,
        "case \"recreate\": expected the freed name to be usable again, got {recreated}"
    );
    assert_eq!(
        status_code(&recreated, case),
        "ACTIVE",
        "case \"recreate\": expected a fresh ACTIVE service, got {recreated}"
    );
}

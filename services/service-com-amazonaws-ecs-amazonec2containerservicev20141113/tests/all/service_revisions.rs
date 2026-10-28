// SPDX-License-Identifier: CC0-1.0

//! `DescribeServiceRevisions`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: a request is answered from the revisions this service CAPTURED —
//! one `ServiceRevision` per ARN that names a captured revision, holding the workload
//! configuration the call that released it was deploying (never what a later call left the
//! service with), and one `MISSING` failure per ARN that names none.
//!
//! The docs the rows are read off:
//!
//! * `DescribeServiceRevisions`: "Describes one or more service revisions. A service
//!   revision is a version of the service that includes the values for the Amazon ECS
//!   resources (for example, task definition) and the environment resources (for example,
//!   load balancers, subnets, and security groups)."
//! * `DescribeServiceRevisions$serviceRevisionArns`: "The ARN of the service revision. You
//!   can specify a maximum of 20 ARNs."
//! * `ServiceRevision`: "A service revision contains a record of the workload configuration
//!   Amazon ECS is attempting to deploy. Whenever you create or deploy a service, Amazon ECS
//!   automatically creates and captures the configuration that you're trying to deploy."
//! * `ServiceRevision$taskDefinition` ("The task definition the service revision uses"),
//!   `$containerImages` ("The container images the service revision uses"),
//!   `$launchType`, `$platformVersion` ("For the Fargate launch type, the platform version
//!   the service revision uses"), `$platformFamily`, `$loadBalancers`,
//!   `$networkConfiguration`, `$createdAt` and `$resolvedConfiguration` ("the actual
//!   resources your service revision uses, such as which target groups serve traffic").
//! * `Service$platformFamily`/`$platformVersion`: "A platform family is specified only for
//!   tasks using the Fargate launch type", with `LATEST` resolving to the newest platform
//!   version published for the task's operating system (`1.4.0` for Linux).
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, register_definition,
    string,
};
use crate::load_balancer_target_groups::{create_target_group, emulator};

/// The cluster every service below is created in.
const CLUSTER: &str = "demo";

/// The image every registered revision below runs, as
/// [`crate::ecs_wire::register_definition`] and the Fargate helper here register it.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The container name those revisions declare.
const CONTAINER: &str = "web";

/// The port a revision publishes when the service that releases it binds a target group.
const CONTAINER_PORT: i64 = 80;

/// The platform version `LATEST` resolves to for a Linux task, from the "Fargate platform
/// versions" page the `platformVersion` doc points at.
const LATEST_LINUX_PLATFORM_VERSION: &str = "1.4.0";

/// The `platformFamily` a Linux Fargate task reports (the `OSFamily` value `LINUX` in its
/// display spelling).
const LINUX_PLATFORM_FAMILY: &str = "Linux";

/// One row of the capture table: the shape of the configuration the released revision
/// carries.
struct RevisionCase {
    case: &'static str,
    /// Whether the service runs on Fargate — the launch type the Fargate-only members are
    /// reported for — which here also means an `awsvpc` revision with a network
    /// configuration.
    fargate: bool,
    /// Whether the service binds a target group, which is what gives its revision a
    /// resolved configuration.
    load_balanced: bool,
}

/// Register one revision of `family` and return its ARN: `awsvpc` with task-level cpu and
/// memory when it is to run on Fargate, and publishing a container port when the service
/// that releases it binds a target group ("The container `<name>` did not have a container
/// port `<port>` defined" is what binding one without it is refused with).
async fn register_workload(
    router: &MultiServiceRouter,
    family: &str,
    fargate: bool,
    publishes_port: bool,
    case: &str,
) -> String {
    let mut web = crate::task_lifecycle::container(CONTAINER, IMAGE);
    if publishes_port {
        web["portMappings"] = json!([{ "containerPort": CONTAINER_PORT, "protocol": "tcp" }]);
    }
    let mut request = json!({
        "family": family,
        "containerDefinitions": [web],
    });
    if fargate {
        request["networkMode"] = json!("awsvpc");
        request["cpu"] = json!("256");
        request["memory"] = json!("512");
    }
    let body = ok(router, "RegisterTaskDefinition", request, case).await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// The ARN of the service revision the service's current rollout released, read back
/// through `DescribeServiceDeployments` — the reader whose own doc points at it.
async fn released_revision(router: &MultiServiceRouter, deployment_arn: &str, case: &str) -> String {
    let body = ok(
        router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [deployment_arn] }),
        case,
    )
    .await;
    let deployments = list(&body, "serviceDeployments", case);
    let deployment = deployments
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the rollout {deployment_arn}"));
    string(&deployment["targetServiceRevision"], "arn", case)
}

/// The revisions and failures `DescribeServiceRevisions` answers for `arns`.
async fn describe_revisions(
    router: &MultiServiceRouter,
    arns: &[String],
    case: &str,
) -> (Vec<Value>, Vec<Value>) {
    let body = ok(
        router,
        "DescribeServiceRevisions",
        json!({ "serviceRevisionArns": arns }),
        case,
    )
    .await;
    (
        list(&body, "serviceRevisions", case),
        list(&body, "failures", case),
    )
}

/// The ONE revision `arn` names, which must be described.
async fn describe_revision(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let (revisions, failures) = describe_revisions(router, &[arn.to_string()], case).await;
    assert!(
        failures.is_empty(),
        "case {case:?}: expected the revision {arn} to be described, got failures {failures:?}"
    );
    revisions
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the revision {arn} to be described"))
        .clone()
}

/// A release captures the configuration it is deploying, whatever shape that configuration
/// has.
#[tokio::test]
async fn a_release_captures_the_configuration_it_is_deploying() {
    let cases = [
        RevisionCase {
            case: "an EC2 service",
            fargate: false,
            load_balanced: false,
        },
        RevisionCase {
            case: "a Fargate service with a network configuration",
            fargate: true,
            load_balanced: false,
        },
        RevisionCase {
            case: "an EC2 service bound to a target group",
            fargate: false,
            load_balanced: true,
        },
    ];

    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, "capacity").await;

    for (index, expectation) in cases.iter().enumerate() {
        let case = expectation.case;
        let family = format!("captured-{index}");
        let name = format!("captured-service-{index}");
        let definition = register_workload(
            &router,
            &family,
            expectation.fargate,
            expectation.load_balanced,
            case,
        )
        .await;
        let mut request = json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": definition,
            "desiredCount": 1,
            "launchType": if expectation.fargate { "FARGATE" } else { "EC2" },
        });
        if expectation.fargate {
            request["networkConfiguration"] = awsvpc_network_configuration();
        }
        let target_group = if expectation.load_balanced {
            let arn = create_target_group(
                &router,
                &format!("revision-target-{index}"),
                "instance",
                case,
            )
            .await;
            request["loadBalancers"] = json!([{
                "targetGroupArn": arn,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }]);
            Some(arn)
        } else {
            None
        };
        let body = ok(&router, "CreateService", request, case).await;
        let service_arn = string(&body["service"], "serviceArn", case);
        let cluster_arn = string(&body["service"], "clusterArn", case);
        let deployment_arn = string(&body["service"], "currentServiceDeployment", case);
        let revision_arn = released_revision(&router, &deployment_arn, case).await;

        let revision = describe_revision(&router, &revision_arn, case).await;
        assert_eq!(
            revision["serviceRevisionArn"],
            json!(revision_arn),
            "case {case:?}: expected the revision to name itself, got {revision}"
        );
        assert_eq!(
            revision["serviceArn"],
            json!(service_arn),
            "case {case:?}: expected the revision to name its service, got {revision}"
        );
        assert_eq!(
            revision["clusterArn"],
            json!(cluster_arn),
            "case {case:?}: expected the revision to name its cluster, got {revision}"
        );
        assert_eq!(
            revision["taskDefinition"],
            json!(definition),
            "case {case:?}: expected the revision to use the released definition, got \
             {revision}"
        );
        assert_eq!(
            revision["containerImages"],
            json!([{ "containerName": CONTAINER, "image": IMAGE }]),
            "case {case:?}: expected the images of the released definition, got {revision}"
        );
        assert!(
            !revision["createdAt"].is_null(),
            "case {case:?}: a revision reports when it was created, got {revision}"
        );
        assert_eq!(
            revision["guardDutyEnabled"],
            json!(false),
            "case {case:?}: nothing turns Runtime Monitoring on, got {revision}"
        );

        let expected_launch_type = if expectation.fargate { "FARGATE" } else { "EC2" };
        assert_eq!(
            revision["launchType"],
            json!(expected_launch_type),
            "case {case:?}: expected the launch type it was released under, got {revision}"
        );
        if expectation.fargate {
            assert_eq!(
                revision["platformVersion"],
                json!(LATEST_LINUX_PLATFORM_VERSION),
                "case {case:?}: expected the resolved platform version, got {revision}"
            );
            assert_eq!(
                revision["platformFamily"],
                json!(LINUX_PLATFORM_FAMILY),
                "case {case:?}: expected the platform family, got {revision}"
            );
            assert_eq!(
                revision["networkConfiguration"],
                json!({
                    "awsvpcConfiguration": {
                        "subnets": ["subnet-0123456789abcdef0"],
                        "securityGroups": ["sg-0123456789abcdef0"],
                        // `AwsVpcConfiguration$assignPublicIp`'s documented default:
                        // "If you do not specify a value, the default value of DISABLED
                        // is used."
                        "assignPublicIp": "DISABLED",
                    }
                }),
                "case {case:?}: expected the network configuration it was released with, \
                 got {revision}"
            );
        } else {
            assert!(
                revision["platformVersion"].is_null(),
                "case {case:?}: a revision off Fargate reports no platform version, got \
                 {revision}"
            );
            assert!(
                revision["platformFamily"].is_null(),
                "case {case:?}: a revision off Fargate reports no platform family, got \
                 {revision}"
            );
            assert!(
                revision["networkConfiguration"].is_null(),
                "case {case:?}: a revision released with none reports none, got {revision}"
            );
        }

        match &target_group {
            Some(arn) => {
                assert_eq!(
                    revision["loadBalancers"],
                    json!([{
                        "targetGroupArn": arn,
                        "containerName": CONTAINER,
                        "containerPort": CONTAINER_PORT,
                    }]),
                    "case {case:?}: expected the binding it was released with, got {revision}"
                );
                assert_eq!(
                    revision["resolvedConfiguration"]["loadBalancers"],
                    json!([{ "targetGroupArn": arn }]),
                    "case {case:?}: expected the target group that serves traffic, got \
                     {revision}"
                );
            }
            None => {
                assert_eq!(
                    list(&revision, "loadBalancers", case),
                    Vec::<Value>::new(),
                    "case {case:?}: a revision bound to nothing reports no binding, got \
                     {revision}"
                );
                assert!(
                    revision["resolvedConfiguration"].is_null(),
                    "case {case:?}: a revision that names no target group resolves none, \
                     got {revision}"
                );
            }
        }
    }
}

/// Every release captures a revision of its own, and the ones before it keep the
/// configuration they were deploying.
#[tokio::test]
async fn each_release_keeps_the_configuration_it_captured() {
    let case = "two releases";
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, case).await;
    let first_definition = register_definition(&router, "kept", case).await;
    let second_definition = register_definition(&router, "kept", case).await;

    let body = ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "kept-service",
            "taskDefinition": first_definition,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let first_revision = released_revision(
        &router,
        &string(&body["service"], "currentServiceDeployment", case),
        case,
    )
    .await;

    let body = ok(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": "kept-service",
            "taskDefinition": second_definition,
        }),
        case,
    )
    .await;
    let second_revision = released_revision(
        &router,
        &string(&body["service"], "currentServiceDeployment", case),
        case,
    )
    .await;
    assert_ne!(
        first_revision, second_revision,
        "case {case:?}: expected the update to release a revision of its own"
    );

    // Both are described in ONE call, each holding what its own release was deploying.
    let (revisions, failures) = describe_revisions(
        &router,
        &[first_revision.clone(), second_revision.clone()],
        case,
    )
    .await;
    assert!(
        failures.is_empty(),
        "case {case:?}: expected both revisions to be described, got {failures:?}"
    );
    assert_eq!(
        revisions.len(),
        2,
        "case {case:?}: expected two revisions, got {revisions:?}"
    );
    for (arn, definition) in [
        (&first_revision, &first_definition),
        (&second_revision, &second_definition),
    ] {
        let revision = revisions
            .iter()
            .find(|revision| revision["serviceRevisionArn"] == json!(arn))
            .unwrap_or_else(|| panic!("case {case:?}: expected the revision {arn}"));
        assert_eq!(
            revision["taskDefinition"],
            json!(definition),
            "case {case:?}: expected revision {arn} to keep {definition}, got {revision}"
        );
    }
}

/// An ARN that names no captured revision is a failure of its own entry, and the same ARN
/// twice is one answer.
#[tokio::test]
async fn arns_that_name_no_captured_revision_are_reported_as_failures() {
    let case = "unknown revisions";
    let router = emulator();
    cluster_with_capacity(&router, CLUSTER, case).await;
    let definition = register_definition(&router, "described", case).await;
    let body = ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "described-service",
            "taskDefinition": definition,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let revision = released_revision(
        &router,
        &string(&body["service"], "currentServiceDeployment", case),
        case,
    )
    .await;

    let absent = format!("{revision}-absent");
    let (revisions, failures) = describe_revisions(
        &router,
        &[revision.clone(), absent.clone(), revision.clone()],
        case,
    )
    .await;
    assert_eq!(
        revisions.len(),
        1,
        "case {case:?}: expected the same ARN twice to be one answer, got {revisions:?}"
    );
    assert_eq!(
        failures,
        vec![json!({ "arn": absent, "reason": "MISSING" })],
        "case {case:?}: expected one MISSING failure naming the ARN the request sent"
    );

    // "You can specify a maximum of 20 ARNs."
    let too_many: Vec<String> = (0..21).map(|index| format!("{revision}-{index}")).collect();
    let (code, message) = refusal(
        &router,
        "DescribeServiceRevisions",
        json!({ "serviceRevisionArns": too_many }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected a bounded list to be refused, got {code} with {message:?}"
    );
    assert_eq!(
        message, "service revisions can have at most 20 items.",
        "case {case:?}: expected the bound to be named, got {message:?}"
    );
}

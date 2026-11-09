// SPDX-License-Identifier: CC0-1.0

//! What Amazon ECS SAYS when a request names a value an enumerated member does not
//! accept, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: a value outside an enumerated member's value set is refused with
//! HTTP 400 and the modelled `InvalidParameterException`, whose message is built from the
//! MEMBER and the set alone — the member named in spaced lower-case words and the values
//! it accepts listed in ascending order between brackets, with no separator but a comma —
//! so the rejected value, the dotted position it sat at, and any count of how many things
//! were wrong are absent from it.
//!
//! The rule is a property of the MEMBER's enumerated type, not of one operation or one
//! spelling of a wrong value, so the table below varies the operation, the member (a
//! top-level member, a member nested two structures down, and an element of a list), the
//! size of the value set, the casing of the values in it, and the form of the wrong value
//! (the lower-case spelling of a valid value, a mixed-case spelling, a truncation, a
//! value of a different enum, an empty string and a padded one). The recorded divergence
//! — a lower-case `assignPublicIp` on `CreateService` — is one row.
//!
//! Value sets come from the model, each read with
//! `smithy_query(query="shape", shape="<Enum>")`:
//!
//! * `AssignPublicIp` — ENABLED, DISABLED
//! * `AvailabilityZoneRebalancing` — ENABLED, DISABLED
//! * `LaunchType` — EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES
//! * `Compatibility` (the element of `CompatibilityList`) — EC2, FARGATE, EXTERNAL,
//!   MANAGED_INSTANCES
//! * `PropagateTags` — TASK_DEFINITION, SERVICE, NONE
//! * `SchedulingStrategy` — REPLICA, DAEMON
//! * `DeploymentControllerType` — ECS, CODE_DEPLOY, EXTERNAL
//! * `NetworkMode` — bridge, host, awsvpc, none
//! * `TaskDefinitionStatus` — ACTIVE, INACTIVE, DELETE_IN_PROGRESS
//! * `SortOrder` — ASC, DESC
//! * `DesiredStatus` — RUNNING, PENDING, STOPPED (the one set below that is matched without
//!   regard to ASCII case, since Amazon ECS serves a task listing whose `desiredStatus`
//!   names a member in another case; `crate::task_listing_filters` pins that, and the row
//!   here sends a value outside the set in every case)
//!
//! The closing section drives the rest of the recorded check: the refused create leaves no
//! service behind, and the same request with the value spelled as the set spells it is
//! accepted and reads back through `DescribeServices`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, refusal, service, string};
use crate::task_lifecycle::container;

/// The cluster every request below names.
const CLUSTER: &str = "enumplane";

/// The `awsvpc` task-definition family every EC2 launch below runs.
const FAMILY: &str = "enumfamily";

/// The `awsvpc` family a FARGATE launch runs: Fargate compatibility is declared by the
/// revision, and a revision that does not declare it supports no Fargate launch type.
const FARGATE_FAMILY: &str = "enumfargatefamily";

/// The service the `UpdateService` and `CreateTaskSet` rows act on.
const SERVICE: &str = "enum-service";

/// The one error code this service models for a request refused before any handler ran.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// A router holding one cluster with capacity, one `awsvpc` revision of [`FAMILY`] and
/// one service, so every row below is refused for the value under test rather than for a
/// resource it could not resolve.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        "fixture",
    )
    .await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FARGATE_FAMILY,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        "fixture",
    )
    .await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
            "networkConfiguration": awsvpc_with(None),
        }),
        "fixture",
    )
    .await;
    router
}

/// A `networkConfiguration` naming one subnet, with `assignPublicIp` set to whatever the
/// caller passes — `None` leaving the member out.
fn awsvpc_with(assign_public_ip: Option<&str>) -> Value {
    let mut configuration = json!({ "subnets": ["subnet-0123456789abcdef0"] });
    if let Some(value) = assign_public_ip {
        configuration["assignPublicIp"] = json!(value);
    }
    json!({ "awsvpcConfiguration": configuration })
}

/// One row: the case name, the operation, the request carrying exactly one out-of-set
/// value, and the whole sentence the refusal must carry.
type EnumRow = (&'static str, &'static str, Value, &'static str);

/// The sentence every `assignPublicIp` row expects — the string the corpus records.
const ASSIGN_PUBLIC_IP: &str = "assign public ip should be one of [DISABLED,ENABLED]";

/// The sentence every `launchType` row expects.
const LAUNCH_TYPE: &str =
    "launch type should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]";

#[tokio::test]
async fn a_value_outside_an_enumerated_members_set_is_refused_naming_the_member_and_the_set() {
    let router = fixture().await;

    let rows: Vec<EnumRow> = vec![
        (
            "the recorded case: a lower-case assignPublicIp on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "lower-case-assign",
                "taskDefinition": FARGATE_FAMILY,
                "desiredCount": 0,
                "launchType": "FARGATE",
                "networkConfiguration": awsvpc_with(Some("enabled")),
            }),
            ASSIGN_PUBLIC_IP,
        ),
        (
            "a mixed-case assignPublicIp on an update",
            "UpdateService",
            json!({
                "cluster": CLUSTER,
                "service": SERVICE,
                "networkConfiguration": awsvpc_with(Some("Disabled")),
            }),
            ASSIGN_PUBLIC_IP,
        ),
        (
            "a truncated assignPublicIp on a task launch",
            "RunTask",
            json!({
                "cluster": CLUSTER,
                "taskDefinition": FAMILY,
                "networkConfiguration": awsvpc_with(Some("DISABLE")),
            }),
            ASSIGN_PUBLIC_IP,
        ),
        (
            "an empty assignPublicIp on a task set",
            "CreateTaskSet",
            json!({
                "cluster": CLUSTER,
                "service": SERVICE,
                "taskDefinition": FAMILY,
                "networkConfiguration": awsvpc_with(Some("")),
            }),
            ASSIGN_PUBLIC_IP,
        ),
        (
            "an assignPublicIp padded with spaces on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "padded-assign",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "networkConfiguration": awsvpc_with(Some(" ENABLED ")),
            }),
            ASSIGN_PUBLIC_IP,
        ),
        (
            "a lower-case launchType on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "lower-case-launch",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "launchType": "fargate",
            }),
            LAUNCH_TYPE,
        ),
        (
            "a launchType that names no launch type at all on a task launch",
            "RunTask",
            json!({
                "cluster": CLUSTER,
                "taskDefinition": FAMILY,
                "launchType": "SPOT",
                "networkConfiguration": awsvpc_with(None),
            }),
            LAUNCH_TYPE,
        ),
        (
            "a launchType filter outside the set on a list",
            "ListTasks",
            json!({ "cluster": CLUSTER, "launchType": "ec2" }),
            LAUNCH_TYPE,
        ),
        (
            "a lower-case propagateTags on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "lower-case-propagate",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "propagateTags": "service",
            }),
            "propagate tags should be one of [NONE,SERVICE,TASK_DEFINITION]",
        ),
        (
            "a lower-case schedulingStrategy on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "lower-case-strategy",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "schedulingStrategy": "replica",
            }),
            "scheduling strategy should be one of [DAEMON,REPLICA]",
        ),
        (
            "a lower-case availabilityZoneRebalancing on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "lower-case-rebalancing",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "availabilityZoneRebalancing": "enabled",
            }),
            "availability zone rebalancing should be one of [DISABLED,ENABLED]",
        ),
        (
            "a deployment controller type outside its set on a create",
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "unknown-controller",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "deploymentController": { "type": "BLUE_GREEN" },
            }),
            "type should be one of [CODE_DEPLOY,ECS,EXTERNAL]",
        ),
        (
            "an upper-case networkMode on a registration",
            "RegisterTaskDefinition",
            json!({
                "family": "upper-case-mode",
                "networkMode": "AWSVPC",
                "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
            }),
            "network mode should be one of [awsvpc,bridge,host,none]",
        ),
        (
            // A LIST element: the member the sentence names is the list, never the
            // position the element sat at.
            "a lower-case requiresCompatibilities element on a registration",
            "RegisterTaskDefinition",
            json!({
                "family": "lower-case-compat",
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["fargate"],
                "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
            }),
            "requires compatibilities should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]",
        ),
        (
            "a second list element outside the set on a registration",
            "RegisterTaskDefinition",
            json!({
                "family": "second-element-compat",
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE", "SERVERLESS"],
                "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
            }),
            "requires compatibilities should be one of [EC2,EXTERNAL,FARGATE,MANAGED_INSTANCES]",
        ),
        (
            "a lower-case status filter on a list",
            "ListTaskDefinitions",
            json!({ "status": "active" }),
            "status should be one of [ACTIVE,DELETE_IN_PROGRESS,INACTIVE]",
        ),
        (
            "a sort order outside its two values on a list",
            "ListTaskDefinitions",
            json!({ "sort": "ASCENDING" }),
            "sort should be one of [ASC,DESC]",
        ),
        (
            // The one filter whose value set Amazon ECS does NOT hold to the exact
            // spelling: a lower-case spelling of a member is served, so a value outside
            // the set in EVERY case is what this row sends.
            "a desiredStatus filter outside the set on a list",
            "ListTasks",
            json!({ "cluster": CLUSTER, "desiredStatus": "DEACTIVATING" }),
            "desired status should be one of [PENDING,RUNNING,STOPPED]",
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (code, message) = refusal(&router, operation, request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected),
            "case {case:?}: expected {operation} to name the member and its value set, got \
             {code:?} with {message:?}"
        );
        // Nothing of the request's SHAPE reaches the sentence: a dotted position would
        // carry a period, and the structures a nested member sits under would be named.
        assert!(
            !message.contains('.')
                && !message.contains("networkConfiguration")
                && !message.contains("deploymentController"),
            "case {case:?}: the sentence must not state the member's position, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_create_refused_for_an_out_of_set_value_leaves_no_service_and_the_set_value_is_served() {
    let router = fixture().await;
    let refused_name = "case-sensitive-assign";

    let (code, message) = refusal(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": refused_name,
            "taskDefinition": FARGATE_FAMILY,
            "desiredCount": 0,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_with(Some("enabled")),
        }),
        "the lower-case value",
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (INVALID_PARAMETER, ASSIGN_PUBLIC_IP),
        "case \"the lower-case value\": expected the member and its value set, got {code:?} \
         with {message:?}"
    );
    // The two things AWS's sentence never carries: the value the request stated, and the
    // position it stated it at.
    assert!(
        !message.contains("enabled") && !message.contains("assignPublicIp"),
        "case \"the lower-case value\": the sentence must carry neither the rejected value \
         nor the member path, got {message:?}"
    );

    // The refused create stored nothing: the name it asked for describes no service, and
    // the read reports it as one MISSING identifier rather than as a service.
    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [refused_name] }),
        "the lower-case value",
    )
    .await;
    assert!(
        list(&described, "services", "the lower-case value").is_empty(),
        "case \"the lower-case value\": a refused create must leave no service, got \
         {described}"
    );
    let failures = list(&described, "failures", "the lower-case value");
    assert_eq!(
        failures.len(),
        1,
        "case \"the lower-case value\": expected one failure for the name, got {described}"
    );
    assert_eq!(
        failures[0]["reason"].as_str(),
        Some("MISSING"),
        "case \"the lower-case value\": expected the name reported MISSING, got {described}"
    );

    // The same request with the value spelled as the set spells it is served, and the
    // stored configuration reads back through `DescribeServices`.
    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": refused_name,
            "taskDefinition": FARGATE_FAMILY,
            "desiredCount": 0,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_with(Some("ENABLED")),
        }),
        "the set's own spelling",
    )
    .await;
    assert_eq!(
        string(&created["service"], "serviceName", "the set's own spelling"),
        refused_name,
        "case \"the set's own spelling\": expected the service created, got {created}"
    );

    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [refused_name] }),
        "the set's own spelling",
    )
    .await;
    let services = list(&described, "services", "the set's own spelling");
    assert_eq!(
        services.len(),
        1,
        "case \"the set's own spelling\": expected the created service described, got \
         {described}"
    );
    assert_eq!(
        services[0]["networkConfiguration"]["awsvpcConfiguration"]["assignPublicIp"].as_str(),
        Some("ENABLED"),
        "case \"the set's own spelling\": expected the stored assignPublicIp back, got \
         {described}"
    );
}

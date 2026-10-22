// SPDX-License-Identifier: CC0-1.0

//! What a SERVICE projection carries, and the lifecycle the service's own clock drives.
//!
//! Three rules, each asserted over a table of inputs rather than one recorded case:
//!
//! 1. Every member AWS materialises for a service is computed from the request and the
//!    deployment controller and then STORED, so a read reports the same value the create
//!    did: `createdBy` (the calling principal), `deploymentController`, the
//!    `deploymentConfiguration` defaults, `healthCheckGracePeriodSeconds`, `roleArn` and
//!    `currentServiceRevisions`. The two members that belong to the ROLLING update the
//!    ECS controller performs — `strategy` and `deploymentCircuitBreaker` — and the
//!    scheduler-owned rollout members are carried only by a service whose controller is
//!    `ECS`.
//! 2. `Deployment$rolloutState`: "When a service deployment is started, it begins in an
//!    IN_PROGRESS state. When the service reaches a steady state, the deployment
//!    transitions to a COMPLETED state" — the steady state is owed by the CLOCK, and
//!    reaching it is what writes the service's events.
//! 3. `DeleteService` KEEPS the record: "Services in the DRAINING or INACTIVE status can
//!    still be viewed with the DescribeServices API operation", and `UpdateService`
//!    refuses a service that is not `ACTIVE`.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{count, list, ok, refused, service, string, ACCOUNT, SIGNED_REGION};
use crate::task_lifecycle::container;

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to
/// fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The access key id the harness signs every request with, so `createdBy` reports THIS
/// caller rather than a fabricated identity.
const CALLER_AKID: &str = "AKIDECSTEST";

/// Register one revision of `family` with a single container and return its ARN.
async fn register(router: &MultiServiceRouter, family: &str, case: &str) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Describe one service, asserting it is reported (rather than failed) and returning it.
async fn describe(
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
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: a stored service is described, not failed, got {body}"
    );
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly one described service, got {body}"
    );
    services[0].clone()
}

/// The event messages a service reports, newest first.
fn event_messages(service: &Value, case: &str) -> Vec<String> {
    list(service, "events", case)
        .iter()
        .map(|event| string(event, "message", case))
        .collect()
}

/// One row of the projection table: how the service is created, and what the members AWS
/// materialises must then read as.
#[derive(Debug)]
struct ProjectionCase {
    name: &'static str,
    service_name: &'static str,
    /// The `deploymentController` the request names, if any.
    controller: Option<&'static str>,
    /// The `role` the request names, if any.
    role: Option<&'static str>,
    /// The `healthCheckGracePeriodSeconds` the request names, if any.
    grace_period: Option<i64>,
    /// The `deploymentConfiguration` the request names, if any.
    configuration: Option<Value>,
    /// The controller the stored service must report.
    expect_controller: &'static str,
    /// The `roleArn` the stored service must report, if any.
    expect_role_arn: Option<String>,
    /// The `healthCheckGracePeriodSeconds` the stored service must report.
    expect_grace_period: i64,
    /// The `deploymentConfiguration.strategy` the stored service must report, if any.
    expect_strategy: Option<&'static str>,
    /// The `deploymentConfiguration.bakeTimeInMinutes` the stored service must report.
    expect_bake_time: i64,
}

#[tokio::test]
async fn a_created_service_materialises_its_documented_defaults_and_reads_them_back() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "projected" }),
        "setup",
    )
    .await;
    let definition_arn = register(&router, "projectedfamily", "setup").await;

    // The service-linked role ARN, from the `role` doc: "If your account has already
    // created the Amazon ECS service-linked role, that role is used for your service
    // unless you specify a role here. The service-linked role is required if ... the
    // service is configured to use ... an external deployment controller".
    let service_linked_role = format!(
        "arn:aws:iam::{ACCOUNT}:role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS"
    );

    // The rows differ along the deployment controller (which decides the rollout
    // members), the form of the `role` reference (absent, a name, a path-prefixed name, a
    // full ARN), the grace period (absent, the default value, another value) and how much
    // of the deployment configuration the request supplies.
    let cases = [
        ProjectionCase {
            name: "the ECS controller with nothing else supplied",
            service_name: "defaults",
            controller: None,
            role: None,
            grace_period: None,
            configuration: None,
            expect_controller: "ECS",
            expect_role_arn: None,
            expect_grace_period: 0,
            expect_strategy: Some("ROLLING"),
            expect_bake_time: 0,
        },
        ProjectionCase {
            name: "an EXTERNAL controller takes the service-linked role",
            service_name: "external",
            controller: Some("EXTERNAL"),
            role: None,
            grace_period: None,
            configuration: None,
            expect_controller: "EXTERNAL",
            expect_role_arn: Some(service_linked_role.clone()),
            expect_grace_period: 0,
            // The rolling-update members belong to the ECS controller alone.
            expect_strategy: None,
            expect_bake_time: 0,
        },
        ProjectionCase {
            name: "a CODE_DEPLOY controller takes the service-linked role",
            service_name: "codedeploy",
            controller: Some("CODE_DEPLOY"),
            role: None,
            grace_period: Some(0),
            configuration: None,
            expect_controller: "CODE_DEPLOY",
            expect_role_arn: Some(service_linked_role.clone()),
            expect_grace_period: 0,
            expect_strategy: None,
            expect_bake_time: 0,
        },
        ProjectionCase {
            name: "a role named by its name",
            service_name: "namedrole",
            controller: None,
            role: Some("ecsServiceRole"),
            grace_period: Some(120),
            configuration: None,
            expect_controller: "ECS",
            expect_role_arn: Some(format!("arn:aws:iam::{ACCOUNT}:role/ecsServiceRole")),
            expect_grace_period: 120,
            expect_strategy: Some("ROLLING"),
            expect_bake_time: 0,
        },
        ProjectionCase {
            name: "a role named by a path-prefixed name",
            service_name: "pathrole",
            controller: None,
            role: Some("/team/ecsServiceRole"),
            grace_period: Some(2_147_483_647),
            configuration: None,
            expect_controller: "ECS",
            expect_role_arn: Some(format!("arn:aws:iam::{ACCOUNT}:role/team/ecsServiceRole")),
            expect_grace_period: 2_147_483_647,
            expect_strategy: Some("ROLLING"),
            expect_bake_time: 0,
        },
        ProjectionCase {
            name: "a role named by its full ARN",
            service_name: "arnrole",
            controller: None,
            role: Some("arn:aws:iam::123456789012:role/custom"),
            grace_period: None,
            configuration: None,
            expect_controller: "ECS",
            expect_role_arn: Some("arn:aws:iam::123456789012:role/custom".to_string()),
            expect_grace_period: 0,
            expect_strategy: Some("ROLLING"),
            expect_bake_time: 0,
        },
        ProjectionCase {
            name: "a deployment configuration that supplies the rolling members itself",
            service_name: "configured",
            controller: None,
            role: None,
            grace_period: None,
            configuration: Some(json!({
                "strategy": "BLUE_GREEN",
                "bakeTimeInMinutes": 15,
                "deploymentCircuitBreaker": { "enable": true, "rollback": true },
            })),
            expect_controller: "ECS",
            expect_role_arn: None,
            expect_grace_period: 0,
            expect_strategy: Some("BLUE_GREEN"),
            expect_bake_time: 15,
        },
    ];

    for test in &cases {
        let case = test.name;
        let mut request = json!({
            "cluster": "projected",
            "serviceName": test.service_name,
            "desiredCount": 1,
            "launchType": "EC2",
        });
        // An EXTERNAL-controller service's tasks come from a task set, not from the
        // service, so it names no task definition.
        if test.controller != Some("EXTERNAL") {
            request["taskDefinition"] = json!(definition_arn);
        }
        if let Some(controller) = test.controller {
            request["deploymentController"] = json!({ "type": controller });
        }
        if let Some(role) = test.role {
            request["role"] = json!(role);
        }
        if let Some(seconds) = test.grace_period {
            request["healthCheckGracePeriodSeconds"] = json!(seconds);
        }
        if let Some(configuration) = test.configuration.clone() {
            request["deploymentConfiguration"] = configuration;
        }

        let created = ok(&router, "CreateService", request, case).await;
        let created = created["service"].clone();
        // Create and read must agree member for member: the response is the stored
        // record, not the request.
        let read = describe(&router, "projected", test.service_name, case).await;
        for reported in [&created, &read] {
            assert_eq!(
                string(reported, "createdBy", case),
                format!("arn:aws:iam::{ACCOUNT}:user/{CALLER_AKID}"),
                "case {case:?}: createdBy must name the calling principal, got {reported}"
            );
            assert_eq!(
                string(&reported["deploymentController"], "type", case),
                test.expect_controller,
                "case {case:?}: unexpected deploymentController in {reported}"
            );
            assert_eq!(
                reported["roleArn"].as_str(),
                test.expect_role_arn.as_deref(),
                "case {case:?}: unexpected roleArn in {reported}"
            );
            assert_eq!(
                count(reported, "healthCheckGracePeriodSeconds", case),
                test.expect_grace_period,
                "case {case:?}: unexpected healthCheckGracePeriodSeconds in {reported}"
            );

            let configuration = &reported["deploymentConfiguration"];
            assert_eq!(
                count(configuration, "bakeTimeInMinutes", case),
                test.expect_bake_time,
                "case {case:?}: unexpected bakeTimeInMinutes in {reported}"
            );
            assert_eq!(
                configuration["strategy"].as_str(),
                test.expect_strategy,
                "case {case:?}: unexpected deployment strategy in {reported}"
            );
            match test.expect_strategy {
                Some(_) => {
                    let breaker = &configuration["deploymentCircuitBreaker"];
                    assert!(
                        breaker["enable"].is_boolean() && breaker["rollback"].is_boolean(),
                        "case {case:?}: an ECS-controller service carries a deployment \
                         circuit breaker, got {reported}"
                    );
                    // "By default, the threshold configuration uses a type of
                    // BOUNDED_PERCENT with a value of 50."
                    if test.configuration.is_none() {
                        assert_eq!(
                            breaker["enable"], json!(false),
                            "case {case:?}: the circuit breaker is off unless configured, \
                             got {reported}"
                        );
                        let threshold = &breaker["thresholdConfiguration"];
                        assert_eq!(
                            (threshold["type"].as_str(), threshold["value"].as_i64()),
                            (Some("BOUNDED_PERCENT"), Some(50)),
                            "case {case:?}: unexpected default threshold in {reported}"
                        );
                    }
                }
                None => assert!(
                    configuration["deploymentCircuitBreaker"].is_null(),
                    "case {case:?}: the circuit breaker is for the rolling update of the \
                     ECS controller only, got {reported}"
                ),
            }

            // The ECS scheduler owns the rollout: one deployment and one service
            // revision, counted off the task set it placed. The other controllers' tasks
            // come from a task set, so they carry neither.
            let revisions = list(reported, "currentServiceRevisions", case);
            let deployments = list(reported, "deployments", case);
            if test.expect_controller == "ECS" {
                assert_eq!(
                    deployments.len(),
                    1,
                    "case {case:?}: expected one deployment, got {reported}"
                );
                assert_eq!(
                    revisions.len(),
                    1,
                    "case {case:?}: expected one current service revision, got {reported}"
                );
                let revision_arn = string(&revisions[0], "arn", case);
                let prefix = format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service-revision/projected/{}/",
                    test.service_name
                );
                assert!(
                    revision_arn.starts_with(&prefix),
                    "case {case:?}: expected a service-revision ARN under {prefix:?}, \
                     got {revision_arn:?}"
                );
                assert_eq!(
                    count(&revisions[0], "requestedTaskCount", case),
                    1,
                    "case {case:?}: the revision requests the service's count, got {reported}"
                );
            } else {
                assert!(
                    deployments.is_empty() && revisions.is_empty(),
                    "case {case:?}: a service whose tasks come from a task set carries no \
                     deployment and no service revision, got {reported}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_rollout_reaches_its_steady_state_on_the_clock_and_writes_its_events() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "settling" }),
        "setup",
    )
    .await;
    let definition_arn = register(&router, "settlingfamily", "setup").await;

    // The rule generalises over the number of tasks the rollout has to place, zero
    // included: a deployment is never COMPLETED in the instant it was started.
    let counts: [i64; 3] = [0, 1, 3];
    for desired in counts {
        let case = &format!("a rollout of {desired} tasks");
        let service_name = &format!("web{desired}");
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": "settling",
                "serviceName": service_name,
                "taskDefinition": definition_arn,
                "desiredCount": desired,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
        let created = created["service"].clone();
        let deployment = list(&created, "deployments", case)[0].clone();
        assert_eq!(
            string(&deployment, "rolloutState", case),
            "IN_PROGRESS",
            "case {case:?}: a deployment begins IN_PROGRESS, got {created}"
        );
        assert!(
            event_messages(&created, case).is_empty(),
            "case {case:?}: no steady state has been reached yet, so there is no event \
             to report, got {created}"
        );

        // The CLOCK settles the rollout: the same read before the span reports it still
        // in progress.
        let early = describe(&router, "settling", service_name, case).await;
        assert_eq!(
            string(&list(&early, "deployments", case)[0], "rolloutState", case),
            "IN_PROGRESS",
            "case {case:?}: reading a service does not settle its rollout, got {early}"
        );

        tokio::time::sleep(ONE_SPAN).await;
        let settled = describe(&router, "settling", service_name, case).await;
        assert_eq!(
            string(&list(&settled, "deployments", case)[0], "rolloutState", case),
            "COMPLETED",
            "case {case:?}: a rollout that reached its steady state is COMPLETED, got \
             {settled}"
        );
        assert_eq!(
            count(&settled, "runningCount", case),
            desired,
            "case {case:?}: expected every placed task RUNNING, got {settled}"
        );
        let messages = event_messages(&settled, case);
        assert_eq!(
            messages.len(),
            2,
            "case {case:?}: reaching a steady state writes the deployment-completed and \
             steady-state events, got {messages:?}"
        );
        assert!(
            messages[0].contains("steady state") && messages[1].contains("deployment completed"),
            "case {case:?}: the event stream reads newest first, got {messages:?}"
        );
        let events = list(&settled, "events", case);
        assert!(
            events
                .iter()
                .all(|event| !string(event, "id", case).is_empty()
                    && event["createdAt"].is_number()),
            "case {case:?}: every event carries an id and a createdAt, got {events:?}"
        );
        assert_ne!(
            string(&events[0], "id", case),
            string(&events[1], "id", case),
            "case {case:?}: every event has its own id, got {events:?}"
        );

        // Reading again neither duplicates the events nor moves anything on.
        let again = describe(&router, "settling", service_name, case).await;
        assert_eq!(
            event_messages(&again, case),
            messages,
            "case {case:?}: a second read reports the same event stream, got {again}"
        );
    }

    // A new deployment starts a new rollout, which is IN_PROGRESS again and keeps the
    // events the service already recorded.
    let case = "a forced new deployment";
    let updated = ok(
        &router,
        "UpdateService",
        json!({ "cluster": "settling", "service": "web1", "forceNewDeployment": true }),
        case,
    )
    .await;
    let updated = updated["service"].clone();
    assert_eq!(
        string(&list(&updated, "deployments", case)[0], "rolloutState", case),
        "IN_PROGRESS",
        "case {case:?}: a new deployment begins IN_PROGRESS, got {updated}"
    );
    assert_eq!(
        event_messages(&updated, case).len(),
        2,
        "case {case:?}: the service keeps the events it already recorded, got {updated}"
    );
    tokio::time::sleep(ONE_SPAN).await;
    let settled = describe(&router, "settling", "web1", case).await;
    assert_eq!(
        event_messages(&settled, case).len(),
        4,
        "case {case:?}: the second steady state appends its own two events, got {settled}"
    );
}

#[tokio::test]
async fn a_deleted_service_stays_describable_as_inactive_and_refuses_updates() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "deleting" }),
        "setup",
    )
    .await;
    let definition_arn = register(&router, "deletingfamily", "setup").await;

    // The rows differ along what the service still holds when it is deleted and how the
    // request addresses it: a service scaled to zero needs no cleanup and is INACTIVE at
    // once, while one that still runs tasks is DRAINING until they stop.
    let cases: [(&str, &str, i64, bool, bool); 4] = [
        ("a service scaled to zero", "idle", 0, false, false),
        ("a service still running tasks, forced", "busy", 2, true, false),
        ("one task, forced", "single", 1, true, false),
        (
            "addressed by its service ARN",
            "byarn",
            0,
            false,
            true,
        ),
    ];

    for (case, service_name, desired, force, by_arn) in cases {
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": "deleting",
                "serviceName": service_name,
                "taskDefinition": definition_arn,
                "desiredCount": desired,
                "launchType": "EC2",
            }),
            case,
        )
        .await;

        // "If the service is actively maintaining tasks, you can't delete it, and you
        // must update the service to a desired task count of zero."
        if desired > 0 {
            refused(
                &router,
                "DeleteService",
                json!({ "cluster": "deleting", "service": service_name }),
                "InvalidParameterException",
                case,
            )
            .await;
        }

        let mut request = if by_arn {
            json!({
                "service": format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/deleting/{service_name}"
                ),
            })
        } else {
            json!({ "cluster": "deleting", "service": service_name })
        };
        if force {
            request["force"] = json!(true);
        }
        let deleted = ok(&router, "DeleteService", request.clone(), case).await;
        let deleted = deleted["service"].clone();
        assert_eq!(
            count(&deleted, "desiredCount", case),
            0,
            "case {case:?}: a deleted service keeps no tasks, got {deleted}"
        );
        let status = string(&deleted, "status", case);
        let expected = if desired > 0 { "DRAINING" } else { "INACTIVE" };
        assert_eq!(
            status, expected,
            "case {case:?}: expected the deletion to leave the service {expected}, got \
             {deleted}"
        );

        // The record is KEPT: a describe reports the service rather than a MISSING
        // failure, and the clock finishes the drain.
        tokio::time::sleep(ONE_SPAN).await;
        let read = describe(&router, "deleting", service_name, case).await;
        assert_eq!(
            string(&read, "status", case),
            "INACTIVE",
            "case {case:?}: a drained service settles INACTIVE, got {read}"
        );
        assert_eq!(
            count(&read, "runningCount", case),
            0,
            "case {case:?}: a deleted service runs nothing, got {read}"
        );
        assert_eq!(
            string(&read, "serviceArn", case),
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/deleting/{service_name}"),
            "case {case:?}: the kept record is the one that was deleted, got {read}"
        );

        // "The specified service isn't active. You can't update a service that's
        // inactive."
        refused(
            &router,
            "UpdateService",
            json!({ "cluster": "deleting", "service": service_name, "desiredCount": 1 }),
            "ServiceNotActiveException",
            case,
        )
        .await;

        // `@idempotent`: deleting it again answers with the same INACTIVE record.
        let again = ok(&router, "DeleteService", request, case).await;
        assert_eq!(
            string(&again["service"], "status", case),
            "INACTIVE",
            "case {case:?}: a repeated delete reports the same record, got {again}"
        );
    }

    // What a delete cannot name at all.
    let refusals: [(&str, Value, &str); 3] = [
        (
            "a service that was never created",
            json!({ "cluster": "deleting", "service": "nosuchservice" }),
            "ServiceNotFoundException",
        ),
        (
            "a cluster that does not exist",
            json!({ "cluster": "nosuchcluster", "service": "idle" }),
            "ClusterNotFoundException",
        ),
        (
            "an ARN naming another cluster than the request",
            json!({
                "cluster": "deleting",
                "service": format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/elsewhere/idle"
                ),
            }),
            "InvalidParameterException",
        ),
    ];
    for (case, request, expected) in refusals {
        refused(&router, "DeleteService", request, expected, case).await;
    }
}

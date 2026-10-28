// SPDX-License-Identifier: CC0-1.0

//! `StopServiceDeployment`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: a stop is taken on exactly the rollout the ARN names while that
//! rollout is still ONGOING, and where it leaves the service is the stop type's — the stop
//! type that rolls back runs the service revision the rollout before it released again,
//! any other stop ends the rollout where it stands — while a request that names a rollout
//! nothing recorded is refused, and so is one that names a rollout which reached an end
//! nobody asked for, and a second request for a stop a caller already asked for is
//! answered with the stop already in effect and changes nothing.
//!
//! The docs the rows are read off:
//!
//! * `StopServiceDeployment`: "Stops an ongoing service deployment. The following stop
//!   types are avaiable: ROLLBACK - This option rolls back the service deployment to the
//!   previous service revision. You can use this option even if you didn't configure the
//!   service deployment for the rollback option."
//! * `StopServiceDeployment$stopType`: "How you want Amazon ECS to stop the service. The
//!   valid values are ROLLBACK", over the modelled `StopServiceDeploymentStopType`
//!   (smithy_query query="shape" shape="StopServiceDeploymentStopType": ABORT, ROLLBACK).
//! * `StopServiceDeployment$serviceDeploymentArn` (output): "The ARN of the stopped service
//!   deployment."
//! * `ServiceDeploymentStatus` (smithy_query query="shape"
//!   shape="ServiceDeploymentStatus": PENDING, SUCCESSFUL, STOPPED, STOP_REQUESTED,
//!   IN_PROGRESS, ROLLBACK_REQUESTED, ROLLBACK_IN_PROGRESS, ROLLBACK_SUCCESSFUL,
//!   ROLLBACK_FAILED).
//! * `ServiceDeployment$stoppedAt` ("The time the service deployment stopped"),
//!   `$finishedAt` ("The time that the service deployment completed") and
//!   `$rollback` ("The rollback options the service deployment uses when the deployment
//!   fails"), whose `serviceRevisionArn` is "The ARN of the service revision deployed as
//!   part of the rollback".
//! * `ConflictException`: "The request could not be processed because of conflict in the
//!   current state of the resource."
//! * `ServiceDeploymentNotFoundException`: "The service deploy ARN that you specified in
//!   the ContinueServiceDeployment doesn't exist."
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, refusal, register_definition, service, string,
};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The cluster every service below is created in.
const CLUSTER: &str = "demo";

/// One row of the stop table: the stop type a request names, the state the rollout is in
/// when it is stopped, and where the stop leaves both.
struct StopCase {
    case: &'static str,
    /// The `stopType` the request names, or `None` for a request that names none.
    stop_type: Option<&'static str>,
    /// The `PAUSE` lifecycle stages the stopped rollout waits at before it is stopped.
    hooked_stages: &'static [&'static str],
    /// The `ServiceDeploymentStatus` the rollout reports while the stop is being carried
    /// out.
    requested_status: &'static str,
    /// The status the rollout ends in.
    final_status: &'static str,
    /// Whether the stop puts the previous service revision back.
    rolls_back: bool,
}

/// A `PAUSE` lifecycle hook configured for `stages`.
fn pause_hook(stages: &[&str]) -> Value {
    json!({
        "targetType": "PAUSE",
        "lifecycleStages": stages,
    })
}

/// A blue/green deployment configuration whose rollout pauses at `stages`.
fn configuration(stages: &[&str]) -> Value {
    json!({
        "strategy": "BLUE_GREEN",
        // "You must provide this parameter when you use the BLUE_GREEN deployment
        // strategy."
        "bakeTimeInMinutes": 1,
        "lifecycleHooks": [pause_hook(stages)],
    })
}

/// Create a service whose rollout the ECS scheduler owns and return the ARN of the service
/// deployment that rollout is recorded as.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": definition,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    string(&body["service"], "currentServiceDeployment", case)
}

/// Move `name` onto `definition`, under a configuration that pauses at `hooked_stages`
/// when it names any, and return the ARN of the deployment the update started.
async fn update_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    hooked_stages: &[&str],
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": CLUSTER,
        "service": name,
        "taskDefinition": definition,
    });
    if !hooked_stages.is_empty() {
        request["deploymentConfiguration"] = configuration(hooked_stages);
    }
    let body = ok(router, "UpdateService", request, case).await;
    string(&body["service"], "currentServiceDeployment", case)
}

/// The stored service, as `DescribeServices` reports it.
async fn describe_service(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described"))
        .clone()
}

/// The stored rollout `arn` names, as `DescribeServiceDeployments` reports it.
async fn describe_deployment(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [arn] }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
        .first()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the service deployment {arn} to be described")
        })
        .clone()
}

/// The ARNs `ListServiceDeployments` reports for `name` under the `status` filter — the
/// SECOND reader of the same record, so a status only the describe view agrees with cannot
/// pass.
async fn list_deployments(
    router: &MultiServiceRouter,
    name: &str,
    status: &str,
    case: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "ListServiceDeployments",
        json!({ "cluster": CLUSTER, "service": name, "status": [status] }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
        .iter()
        .map(|brief| string(brief, "serviceDeploymentArn", case))
        .collect()
}

/// One successful `StopServiceDeployment`, with `stop_type` when the request names one.
async fn stop_deployment(
    router: &MultiServiceRouter,
    arn: &str,
    stop_type: Option<&str>,
    case: &str,
) -> String {
    let body = ok(router, "StopServiceDeployment", stop_request(arn, stop_type), case).await;
    string(&body, "serviceDeploymentArn", case)
}

/// The request body of a stop of `arn`, naming `stop_type` when there is one.
fn stop_request(arn: &str, stop_type: Option<&str>) -> Value {
    let mut request = json!({ "serviceDeploymentArn": arn });
    if let Some(stop_type) = stop_type {
        request["stopType"] = json!(stop_type);
    }
    request
}

/// The hook of `deployment` that is waiting for an action, with the stage it waits at.
fn awaiting_hook(deployment: &Value, case: &str) -> (Value, String) {
    let hooks = list(deployment, "lifecycleHookDetails", case);
    let waiting: Vec<Value> = hooks
        .iter()
        .filter(|hook| hook["status"] == json!("AWAITING_ACTION"))
        .cloned()
        .collect();
    assert_eq!(
        waiting.len(),
        1,
        "case {case:?}: expected exactly one hook waiting for an action, got {deployment}"
    );
    let stage = deployment["lifecycleStage"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a lifecycleStage, got {deployment}"))
        .to_string();
    (waiting[0].clone(), stage)
}

/// The stop type decides where the stop leaves the service, and the stop itself is a span
/// the clock settles: the rollout reports it as requested, then as carried out.
#[tokio::test]
async fn the_stop_type_decides_where_a_stopped_rollout_leaves_the_service() {
    let cases = [
        StopCase {
            case: "ROLLBACK of an in-flight rollout",
            stop_type: Some("ROLLBACK"),
            hooked_stages: &[],
            requested_status: "ROLLBACK_REQUESTED",
            final_status: "ROLLBACK_SUCCESSFUL",
            rolls_back: true,
        },
        StopCase {
            case: "a request that names no stop type",
            stop_type: None,
            hooked_stages: &[],
            requested_status: "ROLLBACK_REQUESTED",
            final_status: "ROLLBACK_SUCCESSFUL",
            rolls_back: true,
        },
        StopCase {
            case: "ABORT of an in-flight rollout",
            stop_type: Some("ABORT"),
            hooked_stages: &[],
            requested_status: "STOP_REQUESTED",
            final_status: "STOPPED",
            rolls_back: false,
        },
        StopCase {
            case: "ROLLBACK of a rollout waiting at one hooked stage",
            stop_type: Some("ROLLBACK"),
            hooked_stages: &["POST_SCALE_UP"],
            requested_status: "ROLLBACK_REQUESTED",
            final_status: "ROLLBACK_SUCCESSFUL",
            rolls_back: true,
        },
        StopCase {
            case: "ABORT of a rollout waiting at the first of two hooked stages",
            stop_type: Some("ABORT"),
            hooked_stages: &["PRE_SCALE_UP", "PRODUCTION_TRAFFIC_SHIFT"],
            requested_status: "STOP_REQUESTED",
            final_status: "STOPPED",
            rolls_back: false,
        },
    ];

    let router = service();
    cluster_with_capacity(&router, CLUSTER, "capacity").await;

    for (index, expectation) in cases.iter().enumerate() {
        let case = expectation.case;
        let family = format!("stopped-{index}");
        let name = format!("stopped-service-{index}");
        // The revision the service runs first, and the one the stopped rollout releases.
        let first = register_definition(&router, &family, case).await;
        let second = register_definition(&router, &family, case).await;
        let released = create_service(&router, &name, &first, case).await;
        tokio::time::sleep(ONE_SPAN).await;
        let source_revision = {
            let deployment = describe_deployment(&router, &released, case).await;
            string(&deployment["targetServiceRevision"], "arn", case)
        };

        let stopped =
            update_service(&router, &name, &second, expectation.hooked_stages, case).await;
        // A rollout that pauses has to REACH its hooked stage before it stands at one.
        let waiting_hook = if expectation.hooked_stages.is_empty() {
            None
        } else {
            tokio::time::sleep(ONE_SPAN).await;
            let deployment = describe_deployment(&router, &stopped, case).await;
            let (hook, _) = awaiting_hook(&deployment, case);
            Some(string(&hook, "hookId", case))
        };

        let answered = stop_deployment(&router, &stopped, expectation.stop_type, case).await;
        assert_eq!(
            answered, stopped,
            "case {case:?}: expected the stopped deployment's own ARN"
        );

        // The stop has been asked for and not carried out yet: the rollout is on its way
        // to an end it has not reached.
        let deployment = describe_deployment(&router, &stopped, case).await;
        assert_eq!(
            deployment["status"],
            json!(expectation.requested_status),
            "case {case:?}: expected the requested stop to be reported, got {deployment}"
        );
        assert!(
            deployment["finishedAt"].is_null(),
            "case {case:?}: a rollout whose stop is still being carried out has not \
             finished, got {deployment}"
        );
        assert!(
            deployment["stoppedAt"].is_null(),
            "case {case:?}: a rollout whose stop is still being carried out has not \
             stopped, got {deployment}"
        );
        assert!(
            deployment["lifecycleStage"].is_null(),
            "case {case:?}: a rollout that is stopping waits at no stage, got {deployment}"
        );
        // And the hook it stood at is waiting for nothing any more, so nothing can be
        // continued on a rollout that is stopping.
        if let Some(hook_id) = &waiting_hook {
            for hook in list(&deployment, "lifecycleHookDetails", case) {
                assert_ne!(
                    hook["status"],
                    json!("AWAITING_ACTION"),
                    "case {case:?}: a stopping rollout leaves no hook waiting, got {hook}"
                );
            }
            let (code, message) = refusal(
                &router,
                "ContinueServiceDeployment",
                json!({ "serviceDeploymentArn": stopped, "hookId": hook_id }),
                case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the hook of a stopping rollout to take no action, \
                 got {code} with {message:?}"
            );
        }

        // The clock carries the stop out, and where it leaves the service is the stop
        // type's.
        tokio::time::sleep(ONE_SPAN).await;
        let stored = describe_service(&router, &name, case).await;
        let expected_definition = if expectation.rolls_back { &first } else { &second };
        assert_eq!(
            stored["taskDefinition"],
            json!(expected_definition),
            "case {case:?}: expected the service on {expected_definition}, got {stored}"
        );

        // A rollback takes one more span to run its course; a stop that puts nothing back
        // is over as soon as it is carried out.
        if expectation.rolls_back {
            tokio::time::sleep(ONE_SPAN).await;
        }
        let deployment = describe_deployment(&router, &stopped, case).await;
        assert_eq!(
            deployment["status"],
            json!(expectation.final_status),
            "case {case:?}: expected the stopped rollout to end in \
             {:?}, got {deployment}",
            expectation.final_status
        );
        assert!(
            !deployment["finishedAt"].is_null(),
            "case {case:?}: a rollout that has ended reports when it ended, got {deployment}"
        );
        if expectation.rolls_back {
            assert_eq!(
                deployment["rollback"]["serviceRevisionArn"],
                json!(source_revision),
                "case {case:?}: expected the rollback to name the previous revision, got \
                 {deployment}"
            );
            assert!(
                !deployment["rollback"]["startedAt"].is_null(),
                "case {case:?}: a rollback reports when it started, got {deployment}"
            );
            assert!(
                deployment["stoppedAt"].is_null(),
                "case {case:?}: a rolled-back rollout did not merely stop, got {deployment}"
            );
        } else {
            assert_eq!(
                deployment["stoppedAt"], deployment["finishedAt"],
                "case {case:?}: a rollout stopped where it stood reports the instant it \
                 stopped, got {deployment}"
            );
            assert!(
                deployment["rollback"].is_null(),
                "case {case:?}: a stop that put no revision back reports no rollback, got \
                 {deployment}"
            );
        }
        // The listing reads the same record, so it reports the same end.
        let listed = list_deployments(&router, &name, expectation.final_status, case).await;
        assert!(
            listed.contains(&stopped),
            "case {case:?}: expected {stopped} among the {:?} deployments, got {listed:?}",
            expectation.final_status
        );
    }
}

/// Every stop of a rollout that reached an end nobody asked for is refused, and changes
/// nothing.
#[tokio::test]
async fn stops_of_a_rollout_that_is_not_ongoing_are_refused() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "capacity").await;

    // A rollout that reached the steady state it was rolling out, so it has FINISHED.
    let settled_definition = register_definition(&router, "settled", "finished").await;
    let settled = create_service(
        &router,
        "settled-service",
        &settled_definition,
        "finished",
    )
    .await;
    tokio::time::sleep(ONE_SPAN).await;
    assert_eq!(
        describe_deployment(&router, &settled, "finished").await["status"],
        json!("SUCCESSFUL"),
        "case \"finished\": expected the rollout to have reached its steady state"
    );

    // A rollout a later deployment replaced, so it ended as STOPPED without a stop.
    let replaced_definition = register_definition(&router, "replaced", "replaced").await;
    let next_definition = register_definition(&router, "replaced", "replaced").await;
    let replaced = create_service(
        &router,
        "replaced-service",
        &replaced_definition,
        "replaced",
    )
    .await;
    update_service(
        &router,
        "replaced-service",
        &next_definition,
        &[],
        "replaced",
    )
    .await;

    let cases: Vec<(&str, String, Option<&str>, &str, bool)> = vec![
        (
            "an ARN no rollout was ever recorded under",
            format!("{settled}-absent"),
            Some("ROLLBACK"),
            "ServiceDeploymentNotFoundException",
            false,
        ),
        (
            "a rollout that reached its steady state",
            settled.clone(),
            Some("ROLLBACK"),
            "ConflictException",
            true,
        ),
        (
            "a rollout a later deployment replaced",
            replaced.clone(),
            Some("ABORT"),
            "ConflictException",
            true,
        ),
        (
            "a stop with no stop type of a rollout that already finished",
            settled.clone(),
            None,
            "ConflictException",
            true,
        ),
    ];

    for (case, arn, stop_type, expected, conflict) in cases {
        let (code, message) =
            refusal(&router, "StopServiceDeployment", stop_request(&arn, stop_type), case).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected a {expected}, got {code} with {message:?}"
        );
        if conflict {
            assert_eq!(
                message,
                format!("ServiceDeployment is stopping or has already finished for arn: {arn}"),
                "case {case:?}: expected the conflict to name the rollout it refused"
            );
        } else {
            assert!(
                message.contains(&arn),
                "case {case:?}: expected the refusal to name {arn}, got {message:?}"
            );
        }
    }

    // Nothing the refusals touched moved: the rollout that finished still carries the end
    // it reached.
    let deployment = describe_deployment(&router, &settled, "after the refusals").await;
    assert_eq!(
        deployment["status"],
        json!("SUCCESSFUL"),
        "case \"after the refusals\": expected the finished rollout to keep its end, got \
         {deployment}"
    );
    assert!(
        deployment["rollback"].is_null(),
        "case \"after the refusals\": a refused rollback rolls nothing back, got {deployment}"
    );
    let deployment = describe_deployment(&router, &replaced, "after the refusals").await;
    assert_eq!(
        deployment["status"],
        json!("STOPPED"),
        "case \"after the refusals\": expected the replaced rollout to keep its end, got \
         {deployment}"
    );
}

/// A stop a caller already asked for is in effect: asking again is answered with it and
/// changes nothing, whether the stop is still being carried out or has run its course.
#[tokio::test]
async fn a_stop_already_asked_for_is_answered_with_the_stop_in_effect() {
    let case = "a repeated stop";
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    let first = register_definition(&router, "repeated", case).await;
    let second = register_definition(&router, "repeated", case).await;
    create_service(&router, "repeated-service", &first, case).await;
    tokio::time::sleep(ONE_SPAN).await;
    let stopped = update_service(&router, "repeated-service", &second, &[], case).await;

    for round in 0..3 {
        let answered = stop_deployment(&router, &stopped, Some("ROLLBACK"), case).await;
        assert_eq!(
            answered, stopped,
            "case {case:?} round {round}: expected the stopped deployment's own ARN"
        );
        let deployment = describe_deployment(&router, &stopped, case).await;
        let status = deployment["status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a status, got {deployment}"));
        // The one stop that was asked for is the only thing moving this rollout, so
        // wherever the clock has taken it, it is one of that stop's own statuses — never
        // back to the rollout being under way.
        assert!(
            ["ROLLBACK_REQUESTED", "ROLLBACK_IN_PROGRESS", "ROLLBACK_SUCCESSFUL"]
                .contains(&status),
            "case {case:?} round {round}: expected the rollout to stand where the one stop \
             it was asked for left it, got {deployment}"
        );
    }

    // The repeats changed nothing, so the ONE stop that was asked for runs its course.
    tokio::time::sleep(ONE_SPAN).await;
    tokio::time::sleep(ONE_SPAN).await;
    let deployment = describe_deployment(&router, &stopped, case).await;
    assert_eq!(
        deployment["status"],
        json!("ROLLBACK_SUCCESSFUL"),
        "case {case:?}: expected the one stop to run its course, got {deployment}"
    );
    let finished_at = deployment["finishedAt"].clone();
    assert!(
        !finished_at.is_null(),
        "case {case:?}: a rollout that has ended reports when it ended, got {deployment}"
    );
    let stored = describe_service(&router, "repeated-service", case).await;
    assert_eq!(
        stored["taskDefinition"],
        json!(first),
        "case {case:?}: expected the service back on the previous revision, got {stored}"
    );

    // And a stop of the rollout that stop already ended is answered with it too, leaving
    // the end it reached exactly where it was.
    let answered = stop_deployment(&router, &stopped, Some("ABORT"), case).await;
    assert_eq!(
        answered, stopped,
        "case {case:?}: expected the stopped deployment's own ARN"
    );
    let deployment = describe_deployment(&router, &stopped, case).await;
    assert_eq!(
        deployment["status"],
        json!("ROLLBACK_SUCCESSFUL"),
        "case {case:?}: expected the end it reached to stand, got {deployment}"
    );
    assert_eq!(
        deployment["finishedAt"], finished_at,
        "case {case:?}: expected the instant it ended to stand, got {deployment}"
    );
    let stored = describe_service(&router, "repeated-service", case).await;
    assert_eq!(
        stored["taskDefinition"],
        json!(first),
        "case {case:?}: expected the service to stay where the stop left it, got {stored}"
    );
}

/// A stop that rolls back is taken on an ongoing rollout whatever lies behind it — "you can
/// use this option even if you didn't configure the service deployment for the rollback
/// option" — so the FIRST rollout of a service accepts one, and the rollback it cannot
/// carry out (there is no earlier service revision to put back) is what ends it.
#[tokio::test]
async fn a_rollback_with_no_earlier_revision_ends_the_rollout_as_failed() {
    let case = "the first rollout of a service";
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    let definition = register_definition(&router, "only", case).await;
    let arn = create_service(&router, "only-service", &definition, case).await;

    let answered = stop_deployment(&router, &arn, Some("ROLLBACK"), case).await;
    assert_eq!(
        answered, arn,
        "case {case:?}: expected the stopped deployment's own ARN"
    );
    let deployment = describe_deployment(&router, &arn, case).await;
    assert_eq!(
        deployment["status"],
        json!("ROLLBACK_REQUESTED"),
        "case {case:?}: expected the requested rollback to be reported, got {deployment}"
    );

    tokio::time::sleep(ONE_SPAN).await;
    let deployment = describe_deployment(&router, &arn, case).await;
    assert_eq!(
        deployment["status"],
        json!("ROLLBACK_FAILED"),
        "case {case:?}: expected a rollback with nothing to put back to fail, got \
         {deployment}"
    );
    assert!(
        !deployment["finishedAt"].is_null(),
        "case {case:?}: a rollout that has ended reports when it ended, got {deployment}"
    );
    assert!(
        string(&deployment, "statusReason", case).contains("no previous service revision"),
        "case {case:?}: expected the end to say why the rollback failed, got {deployment}"
    );
    assert!(
        deployment["rollback"].is_null(),
        "case {case:?}: a rollback that put no revision back reports none, got {deployment}"
    );

    // The service keeps the revision it was rolled out on: nothing was put back, so
    // nothing moved.
    let stored = describe_service(&router, "only-service", case).await;
    assert_eq!(
        stored["taskDefinition"],
        json!(definition),
        "case {case:?}: expected the service on its own revision, got {stored}"
    );

    // And a rollout that ended there was ended by a stop a caller asked for, so a stop of
    // it is answered with that stop rather than conflicting with it.
    let answered = stop_deployment(&router, &arn, Some("ROLLBACK"), case).await;
    assert_eq!(
        answered, arn,
        "case {case:?}: expected the stopped deployment's own ARN"
    );
    let deployment = describe_deployment(&router, &arn, case).await;
    assert_eq!(
        deployment["status"],
        json!("ROLLBACK_FAILED"),
        "case {case:?}: expected the end it reached to stand, got {deployment}"
    );
}

/// The stop types this operation serves are exactly the ones the model declares, so a stop
/// type the model adds cannot be served silently as the one that rolls back.
#[test]
fn the_served_stop_types_are_the_modelled_ones() {
    let modelled =
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::STOP_SERVICE_DEPLOYMENT_STOP_TYPE_VALUES;
    let served = [
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::STOP_TYPE_ABORT,
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::STOP_TYPE_ROLLBACK,
    ];
    let mut expected: Vec<&str> = modelled.to_vec();
    expected.sort_unstable();
    let mut served: Vec<&str> = served.to_vec();
    served.sort_unstable();
    assert_eq!(
        served, expected,
        "case \"stop type catalog\": expected the served stop types to be the modelled \
         ones, got {served:?} against {expected:?}"
    );
}

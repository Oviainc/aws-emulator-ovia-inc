// SPDX-License-Identifier: CC0-1.0

//! WHAT A REFUSAL SAYS WHEN THE SERVICE A REQUEST NAMES IS NOT `ACTIVE`.
//!
//! One rule, asserted over a table rather than one recorded case: an operation that
//! resolves a service and finds it in a status other than `ACTIVE` answers
//! `ServiceNotActiveException` with ONE fixed sentence for that condition — the same text
//! for every request that meets it, carrying nothing from the request (not the service it
//! named, not the cluster, not the form it named them in) and not reporting WHICH
//! non-`ACTIVE` status (`DRAINING` or `INACTIVE`) the service holds.
//!
//! The rows differ along every dimension the sentence has to be independent of: the
//! operation sent (all six of the cluster/service pair's operations that model the error,
//! plus the two Express-service ones), the service's name (a short name, a name with
//! digits and an underscore, and the longest name the member accepts — "Up to 255
//! letters (uppercase and lowercase), numbers, underscores, and hyphens are allowed"),
//! the form the request names it by (bare name or full ARN), and the non-`ACTIVE` status
//! the service holds. The behavioural check's case — `CreateTaskSet` on one `DRAINING`
//! service — is one row.
//!
//! The positive control comes first: while the service IS active the same `CreateTaskSet`
//! succeeds and the task set reads back through `DescribeTaskSets`, so the rows below
//! refuse because of the status and not because the request was malformed.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, list, ok, refusal, register_capacity, register_definition, service, string,
    ACCOUNT, SIGNED_REGION,
};
use crate::express_gateway_services::{create_express, infrastructure_role, status_code};
use crate::task_sets::create_external_service;

/// Long enough for the clock-owed move from `DRAINING` to `INACTIVE`
/// (`state::TRANSITION_SPAN` is 500 ms) to fall due, with margin for a loaded machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The cluster every row points at, and the task-definition family a `CreateTaskSet` row
/// names.
const CLUSTER: &str = "notactive";
const FAMILY: &str = "notactivefamily";

/// The task set a row names. The status guard is reached before any task-set reference is
/// resolved, so the id only has to be well formed.
const TASK_SET_ID: &str = "ecs-svc/0000000000000000001";

/// The fixed sentence a `ServiceNotActiveException` carries, whatever request met the
/// condition (`crate::state::SERVICE_NOT_ACTIVE_MESSAGE`).
const SERVICE_NOT_ACTIVE: &str = "Service was not ACTIVE.";

/// One row of the operation table: the operation's name, and the request a row builds for
/// it from the service reference it is given.
type OperationRow = (&'static str, fn(&str) -> Value);

/// Every operation of the cluster/service pair that models `ServiceNotActiveException`
/// (`smithy_query(query="operation")` on each: `CreateTaskSet`, `UpdateService`,
/// `DescribeTaskSets`, `UpdateTaskSet`, `DeleteTaskSet`, `UpdateServicePrimaryTaskSet`),
/// each paired with a request that is well formed apart from the service's status.
const REQUESTS: [OperationRow; 6] = [
    ("CreateTaskSet", |reference| {
        json!({
            "cluster": CLUSTER,
            "service": reference,
            "taskDefinition": FAMILY,
            "launchType": "EC2",
            "scale": { "value": 100, "unit": "PERCENT" },
        })
    }),
    ("UpdateService", |reference| {
        json!({ "cluster": CLUSTER, "service": reference, "desiredCount": 1 })
    }),
    ("DescribeTaskSets", |reference| {
        json!({ "cluster": CLUSTER, "service": reference })
    }),
    ("UpdateTaskSet", |reference| {
        json!({
            "cluster": CLUSTER,
            "service": reference,
            "taskSet": TASK_SET_ID,
            "scale": { "value": 50, "unit": "PERCENT" },
        })
    }),
    ("DeleteTaskSet", |reference| {
        json!({ "cluster": CLUSTER, "service": reference, "taskSet": TASK_SET_ID })
    }),
    ("UpdateServicePrimaryTaskSet", |reference| {
        json!({ "cluster": CLUSTER, "service": reference, "primaryTaskSet": TASK_SET_ID })
    }),
];

/// The service names the table spans: a short one, one with digits and an underscore, and
/// the longest the `serviceName` member accepts (255 characters).
fn service_names() -> Vec<(&'static str, String)> {
    vec![
        ("short name", "web".to_string()),
        (
            "name with digits and an underscore",
            "svc_42-blue".to_string(),
        ),
        ("longest name", "s".repeat(255)),
    ]
}

/// The two forms a request names a service by: the bare name, and the full ARN (which
/// carries the cluster as well).
fn references(name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("by name", name.to_string()),
        (
            "by ARN",
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{CLUSTER}/{name}"),
        ),
    ]
}

/// The status one service reports through `DescribeServices`.
async fn service_status(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    let described = list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described, got {body}"));
    string(&described, "status", case)
}

/// Assert one refusal is the error class's fixed sentence, and that the sentence discloses
/// neither the service the request named nor the status it holds.
fn assert_fixed_sentence(answered: (String, String), name: &str, status: &str, case: &str) {
    let (code, message) = answered;
    assert_eq!(
        code, "ServiceNotActiveException",
        "case {case:?}: expected a ServiceNotActiveException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message, SERVICE_NOT_ACTIVE,
        "case {case:?}: expected {SERVICE_NOT_ACTIVE:?}, got {message:?}"
    );
    assert!(
        !message.contains(name),
        "case {case:?}: the sentence must not carry the service the request named \
         ({name:?}), got {message:?}"
    );
    assert!(
        !message.contains(status),
        "case {case:?}: the sentence must not disclose the service's status ({status:?}), \
         got {message:?}"
    );
}

/// Send every operation of [`REQUESTS`] at one non-`ACTIVE` service, in both reference
/// forms, and assert each refusal carries the fixed sentence.
async fn assert_every_operation_refuses(
    router: &MultiServiceRouter,
    name: &str,
    form: &str,
    status: &str,
) {
    for (operation, build) in REQUESTS {
        for (naming, reference) in references(name) {
            let case = format!("{operation} on a {status} service with a {form}, {naming}");
            let answered = refusal(router, operation, build(&reference), &case).await;
            assert_fixed_sentence(answered, name, status, &case);
        }
    }
}

#[tokio::test]
async fn an_operation_on_a_service_that_is_not_active_is_refused_with_one_fixed_sentence() {
    let router = service();
    let case = "setup";
    create_cluster(&router, CLUSTER, case).await;
    register_capacity(&router, CLUSTER, case).await;
    register_definition(&router, FAMILY, case).await;

    let names = service_names();
    for (form, name) in &names {
        // `desiredCount` 0, so the delete below needs no `force` ("You can delete a
        // service if you have no running tasks in it and the desired task count is zero").
        create_external_service(&router, CLUSTER, name, 0, form).await;
    }

    // The positive control, on the first service while it is still ACTIVE: the very
    // request the rows are refused with succeeds, and the task set it creates reads back.
    let (form, name) = names.first().expect("the table has rows").clone();
    let case = format!("{form}, while ACTIVE");
    assert_eq!(
        service_status(&router, &name, &case).await,
        "ACTIVE",
        "case {case:?}: expected a freshly created service to be ACTIVE"
    );
    let create_task_set = REQUESTS[0].1;
    let created = ok(&router, "CreateTaskSet", create_task_set(&name), &case).await;
    let created_id = string(&created["taskSet"], "id", &case);
    let read = ok(
        &router,
        "DescribeTaskSets",
        json!({ "cluster": CLUSTER, "service": name }),
        &case,
    )
    .await;
    let described: Vec<String> = list(&read, "taskSets", &case)
        .iter()
        .map(|task_set| string(task_set, "id", &case))
        .collect();
    assert_eq!(
        described,
        vec![created_id.clone()],
        "case {case:?}: expected the created task set {created_id:?} to read back, got {read}"
    );

    // Deleting each service starts the drain the clock finishes, so each one is DRAINING
    // when its rows run.
    for (form, name) in &names {
        ok(
            &router,
            "DeleteService",
            json!({ "cluster": CLUSTER, "service": name, "force": true }),
            form,
        )
        .await;
        let case = format!("{form}, after the delete");
        let status = service_status(&router, name, &case).await;
        assert_eq!(
            status, "DRAINING",
            "case {case:?}: expected the deleted service to be DRAINING"
        );
        assert_every_operation_refuses(&router, name, form, &status).await;
    }

    // "After all tasks have transitioned to either STOPPING or STOPPED status, the service
    // status moves from DRAINING to INACTIVE": the terminal non-ACTIVE status answers the
    // same sentence.
    tokio::time::sleep(ONE_SPAN).await;
    for (form, name) in &names {
        let case = format!("{form}, after the drain");
        let status = service_status(&router, name, &case).await;
        assert_eq!(
            status, "INACTIVE",
            "case {case:?}: expected the drained service to settle INACTIVE"
        );
        assert_every_operation_refuses(&router, name, form, &status).await;
    }
}

#[tokio::test]
async fn an_express_service_that_is_not_active_is_refused_with_the_same_fixed_sentence() {
    let router = service();
    let case = "express";
    create_cluster(&router, CLUSTER, case).await;

    // The two Express operations that model `ServiceNotActiveException`
    // (`UpdateExpressGatewayService`, `DeleteExpressGatewayService`) answer the same
    // sentence for the same condition. An Express service is removed outright once its
    // drain finishes, so DRAINING is the only non-ACTIVE status it reports.
    let name = "storefront";
    let created = create_express(
        &router,
        json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "infrastructureRoleArn": infrastructure_role(),
            "primaryContainer": { "image": "public.ecr.aws/nginx/nginx:latest" },
        }),
        case,
    )
    .await;
    let arn = string(&created, "serviceArn", case);
    let deleted = ok(
        &router,
        "DeleteExpressGatewayService",
        json!({ "serviceArn": arn.clone() }),
        case,
    )
    .await;
    let status = status_code(&deleted["service"], case);
    assert_eq!(
        status, "DRAINING",
        "case {case:?}: expected the delete to start the drain, got {deleted}"
    );

    let rows: [(&str, Value); 2] = [
        (
            "DeleteExpressGatewayService",
            json!({ "serviceArn": arn.clone() }),
        ),
        (
            "UpdateExpressGatewayService",
            json!({ "serviceArn": arn.clone(), "cpu": "1024" }),
        ),
    ];
    for (operation, request) in rows {
        let case = format!("{operation} on a {status} Express service");
        let answered = refusal(&router, operation, request, &case).await;
        assert_fixed_sentence(answered, name, &status, &case);
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What a FORCED `DeleteService` answers with, over a table of services rather than one
//! case.
//!
//! The rule under test: a delete that still has cleanup to do answers with the resource in
//! the TRANSIENT teardown status, and only the CLOCK moves it to the terminal one — so the
//! terminal status is never reachable in the instant the call that started the teardown
//! answers, whatever made the teardown legal. For a service that is `DeleteService`'s own
//! sentence — "When you delete a service, if there are still running tasks that require
//! cleanup, the service status moves from ACTIVE to DRAINING ... After all tasks have
//! transitioned to either STOPPING or STOPPED status, the service status moves from
//! DRAINING to INACTIVE" — and `force` only lifts the precondition ("allows you to delete a
//! service even if it wasn't scaled down to zero tasks"), it does not shorten the drain.
//! `Service$status`: "The valid values are ACTIVE, DRAINING, or INACTIVE."
//!
//! The same rule's other half is that the REFUSED delete is not a teardown at all: a
//! service the emulator refuses to delete is left exactly as it was, so the status and the
//! desired count a later read reports are the ones the refusal did not touch.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::json;

use crate::ecs_wire::{
    count, create_cluster, list, ok, refusal, register_capacity, register_definition, service,
    string, ACCOUNT, SIGNED_REGION,
};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The fixed sentence `DeleteService` refuses a still-scaled service with
/// (`state::SERVICE_SCALED_ABOVE_ZERO_MESSAGE`): a condition of the service, so it names
/// nothing from the request.
const SCALED_ABOVE_ZERO: &str = "The service cannot be stopped while it is scaled above 0.";

/// One row of the forced-delete table. Each row gets its OWN cluster, so the capacity one
/// row's tasks were placed on cannot place another row's.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The cluster this row creates, and the service it creates in it.
    cluster: &'static str,
    name: &'static str,
    /// Whether a container instance is registered into the cluster — and so whether the
    /// scheduler had anywhere to place the service's tasks at all.
    capacity: bool,
    /// What the service wanted when it was deleted.
    desired: i64,
    /// How many tasks the drain therefore has to stop.
    placed: usize,
}

#[tokio::test]
async fn a_forced_delete_reports_draining_and_only_the_clock_reports_inactive() {
    let router = service();

    // The rows differ along how much cleanup the forced delete has to do: a task the
    // cluster held no capacity to place, one task, and several tasks. Each row is still
    // scaled above zero when it is deleted, which is what makes the delete need `force`
    // and what leaves the scheduler work to undo — the service wants a task it must now
    // stop wanting. A service already scaled to zero is the sibling suite's table
    // (`crate::service_delete_drain`), not this one's.
    let rows: [Row; 3] = [
        Row {
            case: "forced, one task wanted and no capacity to place it on",
            cluster: "forcednocapacity",
            name: "nocapacity",
            capacity: false,
            desired: 1,
            placed: 0,
        },
        Row {
            case: "forced, one task running",
            cluster: "forcedonetask",
            name: "onetask",
            capacity: true,
            desired: 1,
            placed: 1,
        },
        Row {
            case: "forced, three tasks running",
            cluster: "forcedthreetasks",
            name: "threetasks",
            capacity: true,
            desired: 3,
            placed: 3,
        },
    ];

    for row in &rows {
        let case = row.case;
        create_cluster(&router, row.cluster, case).await;
        if row.capacity {
            // "The EC2 launch type runs your tasks on Amazon EC2 instances registered to
            // your cluster", so only a cluster with capacity places anything.
            register_capacity(&router, row.cluster, case).await;
        }
        register_definition(&router, row.cluster, case).await;
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": row.cluster,
                "serviceName": row.name,
                "taskDefinition": row.cluster,
                "desiredCount": row.desired,
                "launchType": "EC2",
            }),
            case,
        )
        .await;

        // An unforced delete of a service that is still scaled up is refused — and the
        // refusal is not a teardown, so the service it refused to delete is untouched.
        let (code, message) = refusal(
            &router,
            "DeleteService",
            json!({ "cluster": row.cluster, "service": row.name }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", SCALED_ABOVE_ZERO),
            "case {case:?}: unexpected refusal of the unforced delete"
        );
        let read = describe(&router, row.cluster, row.name, case).await;
        assert_eq!(
            (string(&read, "status", case), count(&read, "desiredCount", case)),
            ("ACTIVE".to_string(), row.desired),
            "case {case:?}: a refused delete leaves the service as it was, got {read}"
        );

        let deleted = ok(
            &router,
            "DeleteService",
            json!({ "cluster": row.cluster, "service": row.name, "force": true }),
            case,
        )
        .await;
        let deleted = &deleted["service"];
        assert_eq!(
            string(deleted, "serviceName", case),
            row.name,
            "case {case:?}: the delete must answer with the service it was given, got \
             {deleted}"
        );
        assert_eq!(
            string(deleted, "serviceArn", case),
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{}/{}",
                row.cluster, row.name
            ),
            "case {case:?}: unexpected serviceArn, got {deleted}"
        );
        assert_eq!(
            string(deleted, "status", case),
            "DRAINING",
            "case {case:?}: expected the forced delete to leave the service draining, got \
             {deleted}"
        );
        assert_eq!(
            count(deleted, "desiredCount", case),
            0,
            "case {case:?}: a deleted service wants no task, got {deleted}"
        );

        // The terminal status is owed by the CLOCK, so a caller reading the service
        // straight back still sees the transient one.
        let read = describe(&router, row.cluster, row.name, case).await;
        assert_eq!(
            string(&read, "status", case),
            "DRAINING",
            "case {case:?}: the drain is not finished by whoever reads next, got {read}"
        );
    }

    tokio::time::sleep(ONE_SPAN).await;

    for row in &rows {
        let case = row.case;
        // "Services in the DRAINING or INACTIVE status can still be viewed with the
        // DescribeServices API operation" — and the drain the delete started has now
        // fallen due.
        let read = describe(&router, row.cluster, row.name, case).await;
        assert_eq!(
            string(&read, "status", case),
            "INACTIVE",
            "case {case:?}: expected the owed move to INACTIVE to have fallen due, got \
             {read}"
        );

        // "After all tasks have transitioned to either STOPPING or STOPPED status": every
        // task the service held was stopped by the drain, and by nothing else.
        let stopped = ok(
            &router,
            "ListTasks",
            json!({
                "cluster": row.cluster,
                "serviceName": row.name,
                "desiredStatus": "STOPPED",
            }),
            case,
        )
        .await;
        assert_eq!(
            list(&stopped, "taskArns", case).len(),
            row.placed,
            "case {case:?}: expected the {} task(s) of the service to have stopped, got \
             {stopped}",
            row.placed
        );
    }
}

/// The one service `DescribeServices` reports for `name`, which must be reported rather
/// than missing.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    cluster: &str,
    name: &str,
    case: &str,
) -> serde_json::Value {
    let read = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [name] }),
        case,
    )
    .await;
    assert!(
        list(&read, "failures", case).is_empty(),
        "case {case:?}: a deleted service is still viewable with DescribeServices, got \
         {read}"
    );
    let services = list(&read, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly the one service asked for, got {read}"
    );
    services[0].clone()
}

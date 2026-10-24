// SPDX-License-Identifier: CC0-1.0

//! MediaLive `StartDeleteMonitorDeployment`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: one call is ONE row of each. The wire harness
//! (`medialive`, `call`, `error_code`, `error_message`), the clock wait and the
//! signal-map helpers are the crate's shared ones — reused, not copied: this is
//! the DELETION half of the monitor-deployment lifecycle whose deploying half
//! lives in `monitor_deployment`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::monitor_deployment::{
    deployment_details_uri, get_signal_map, listed_summary, monitor_deployment_uri,
    signal_map_discovering, signal_map_ready_to_deploy, start_monitor_deployment,
};
use crate::wire::{call, error_code, error_message, medialive, DEFAULT_REGION};

/// `StartDeleteMonitorDeployment` for `identifier` (`DELETE
/// /prod/signal-maps/{Identifier}/monitor-deployment`; `Identifier` is the
/// operation's only input member and an `@httpLabel`, so the request carries no
/// payload).
async fn start_delete_monitor_deployment(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, Value) {
    call(router, "DELETE", &monitor_deployment_uri(identifier), None).await
}

/// Deploy a monitor of `id` once per entry of `sequence` — `true` as a dry run,
/// `false` for real — letting the clock complete each deployment, and answer the
/// `LastSuccessfulMonitorDeployment` the signal map reports afterwards.
async fn deploy_monitor_sequence(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    sequence: &[bool],
) -> Value {
    for (step, dry_run) in sequence.iter().enumerate() {
        let (status, started) =
            start_monitor_deployment(router, id, Some(json!({ "dryRun": dry_run }))).await;
        assert_eq!(
            status, 202,
            "case {case:?} deploy step {step}: expected 202, got {status}: {started}"
        );
        wait_for_the_clock();
    }
    let (status, read) = get_signal_map(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetSignalMap to answer 200, got {status}: {read}"
    );
    read["lastSuccessfulMonitorDeployment"].clone()
}

/// One row of the deletion table: the monitor-deployment history the delete is
/// called from, and how the delete addresses the signal map.
struct Deletion {
    case: &'static str,
    /// The `dryRun` values to deploy a monitor with, in order, BEFORE deleting
    /// it; empty is a signal map nobody ever deployed a monitor for.
    deployed: &'static [bool],
    /// Whether the delete names the signal map by its current name rather than
    /// by its id (`Identifier` is "either be its id or current name").
    by_name: bool,
}

/// Rule: `StartDeleteMonitorDeployment` starts a deployment to delete the
/// monitor of the signal map its `Identifier` resolves to, answers 202 with that
/// signal map as the store then holds it, and makes that deletion the signal
/// map's latest monitor deployment — running in `DELETE_IN_PROGRESS`, which the
/// clock settles into `DELETE_COMPLETE`, at which point the signal map reports
/// no `LastSuccessfulMonitorDeployment` because the monitor a deployment had
/// succeeded in deploying is gone.
///
/// The rows differ along what the rule generalises over: the monitor-deployment
/// history the delete is called from (none at all, one real deployment, one dry
/// run, a dry run then a real one — so whether a successful deployment existed
/// to lose differs per row) and whether the signal map is addressed by id or by
/// name. The statuses are the model's own (`smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
#[tokio::test]
async fn deleting_a_monitor_deployment_runs_a_deletion_and_leaves_no_deployed_monitor() {
    let table = [
        Deletion {
            case: "no monitor ever deployed",
            deployed: &[],
            by_name: false,
        },
        Deletion {
            case: "after a real deployment",
            deployed: &[false],
            by_name: true,
        },
        Deletion {
            case: "after a dry run",
            deployed: &[true],
            by_name: false,
        },
        Deletion {
            case: "after a dry run then a real deployment",
            deployed: &[true, false],
            by_name: true,
        },
    ];

    for Deletion {
        case,
        deployed,
        by_name,
    } in table
    {
        let router = medialive();
        let name = format!("delete-{}", case.replace(' ', "-"));
        let id = signal_map_ready_to_deploy(&router, case, &name).await;
        let successful_before = deploy_monitor_sequence(&router, case, &id, deployed).await;
        // A real deployment is what leaves a successful deployment behind; a dry
        // run and a signal map nobody ever deployed leave none.
        if deployed.contains(&false) {
            assert_eq!(
                successful_before["status"],
                json!("DEPLOYMENT_COMPLETE"),
                "case {case:?}: expected a real deployment to have succeeded before the \
                 deletion, got {successful_before}"
            );
        } else {
            assert_eq!(
                successful_before,
                Value::Null,
                "case {case:?}: expected no successful deployment before the deletion, \
                 got {successful_before}"
            );
        }

        let identifier = if by_name { name.clone() } else { id.clone() };
        let (status, started) = start_delete_monitor_deployment(&router, &identifier).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected StartDeleteMonitorDeployment to answer 202, \
             got {status}: {started}"
        );
        assert_eq!(
            started["id"],
            json!(id),
            "case {case:?}: expected the response to be the signal map {id:?}, got {:?}",
            started["id"]
        );
        let details_uri = deployment_details_uri(
            case,
            &id,
            "DELETE_IN_PROGRESS",
            &started["monitorDeployment"],
        );
        // The deletion has not completed yet, so what a deployment deployed is
        // still deployed.
        assert_eq!(
            started["lastSuccessfulMonitorDeployment"], successful_before,
            "case {case:?}: expected the last successful deployment to survive while the \
             deletion runs, got {:?}",
            started["lastSuccessfulMonitorDeployment"]
        );

        // The deletion is asynchronous, so a caller polling straight after the
        // call still sees it running — and reading it does not complete it.
        let (status, polled) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {polled}"
        );
        assert_eq!(
            deployment_details_uri(case, &id, "DELETE_IN_PROGRESS", &polled["monitorDeployment"]),
            details_uri,
            "case {case:?}: expected the stored detailsUri to be stable across reads"
        );
        let summary = listed_summary(&router, case, &id).await;
        assert_eq!(
            summary["monitorDeploymentStatus"],
            json!("DELETE_IN_PROGRESS"),
            "case {case:?}: expected the summary to report the running deletion, got {:?}",
            summary["monitorDeploymentStatus"]
        );

        // A deployment of the monitor this deletion is deleting, and a second
        // deletion of it, are both already-in-flight work: refused while it runs.
        let (status, refused) = start_monitor_deployment(&router, &id, None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a deployment to be refused while the deletion runs, \
             got {status}: {refused}"
        );
        let (status, refused) = start_delete_monitor_deployment(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a second deletion to be refused while one runs, \
             got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );

        wait_for_the_clock();
        let (status, settled) = get_signal_map(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {settled}"
        );
        assert_eq!(
            deployment_details_uri(case, &id, "DELETE_COMPLETE", &settled["monitorDeployment"]),
            details_uri,
            "case {case:?}: expected the completed deletion to keep its detailsUri"
        );
        assert_eq!(
            settled["lastSuccessfulMonitorDeployment"],
            Value::Null,
            "case {case:?}: expected no deployed monitor left once the deletion completed, \
             got {:?}",
            settled["lastSuccessfulMonitorDeployment"]
        );
        let summary = listed_summary(&router, case, &id).await;
        assert_eq!(
            summary["monitorDeploymentStatus"],
            json!("DELETE_COMPLETE"),
            "case {case:?}: expected the summary to report the completed deletion, got {:?}",
            summary["monitorDeploymentStatus"]
        );
        // The signal map itself is untouched by the deletion of its monitor: it
        // is still the signal map discovery created, and it can be deployed
        // again.
        assert_eq!(
            settled["status"],
            json!("CREATE_COMPLETE"),
            "case {case:?}: expected the signal map to keep its own status, got {:?}",
            settled["status"]
        );
        let (status, redeployed) = start_monitor_deployment(&router, &id, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the monitor to be deployable again after the deletion, \
             got {status}: {redeployed}"
        );
    }
}

/// Rule: a monitor deletion is repeatable — a signal map whose monitor was
/// already deleted takes another deletion rather than refusing it, because a
/// deletion whose target is already gone has nothing left to delete and so
/// completes, and each deletion is the signal map's latest monitor deployment
/// while it runs.
///
/// The rows are the COUNT of deletions in a row, which is what the invariant
/// generalises over: the second and the third must answer exactly as the first.
#[tokio::test]
async fn deleting_a_monitor_deployment_again_is_accepted_and_completes_again() {
    for deletions in [1_usize, 2, 3] {
        let case = format!("{deletions} deletions in a row");
        let router = medialive();
        let name = format!("repeat-delete-{deletions}");
        let id = signal_map_ready_to_deploy(&router, &case, &name).await;
        deploy_monitor_sequence(&router, &case, &id, &[false]).await;

        for round in 0..deletions {
            let (status, started) = start_delete_monitor_deployment(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?} round {round}: expected 202, got {status}: {started}"
            );
            assert_eq!(
                started["monitorDeployment"]["status"],
                json!("DELETE_IN_PROGRESS"),
                "case {case:?} round {round}: expected a running deletion, got {:?}",
                started["monitorDeployment"]
            );
            wait_for_the_clock();
            let (status, settled) = get_signal_map(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?} round {round}: expected GetSignalMap to answer 200, \
                 got {status}: {settled}"
            );
            assert_eq!(
                settled["monitorDeployment"]["status"],
                json!("DELETE_COMPLETE"),
                "case {case:?} round {round}: expected the deletion to have completed, got {:?}",
                settled["monitorDeployment"]
            );
            assert_eq!(
                settled["lastSuccessfulMonitorDeployment"],
                Value::Null,
                "case {case:?} round {round}: expected no deployed monitor left, got {:?}",
                settled["lastSuccessfulMonitorDeployment"]
            );
        }
    }
}

/// Rule: an `Identifier` that resolves to no signal map is refused with
/// `NotFoundException` (404), and a signal map that cannot take a monitor
/// deployment in the state it is in — its discovery still running, or a
/// deployment of its own still running — is refused with `ConflictException`
/// (409); a refused call starts nothing.
///
/// The rows differ along WHY the identifier resolves to nothing (an id of the
/// signal-map range nobody minted, an id of another resource's range, a name no
/// signal map carries, an ARN rather than an identifier) and along WHICH state
/// refuses the deletion, because the rule is about resolution and state, not
/// about any one value.
#[tokio::test]
async fn a_deletion_is_refused_on_an_unknown_identifier_or_a_busy_signal_map() {
    let router = medialive();
    let name = "delete-refusals";
    let id = signal_map_ready_to_deploy(&router, "delete refusals", name).await;

    let unknown: [(&str, String); 4] = [
        ("signal-map id nobody minted", "4999999".to_string()),
        ("a channel's id", "1000000".to_string()),
        ("a name no signal map carries", "no-such-map".to_string()),
        (
            "an ARN rather than an identifier",
            format!("arn:aws:medialive:{DEFAULT_REGION}:000000000000:signal-map:{id}"),
        ),
    ];
    for (case, identifier) in unknown {
        let (status, body) = start_delete_monitor_deployment(&router, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an identifier naming no signal map, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(&identifier),
            "case {case:?}: expected the message to name the requested identifier, got {:?}",
            error_message(&body)
        );
    }

    // The signal map the refused calls named is untouched: no deletion was
    // started on it.
    let (status, untouched) = get_signal_map(&router, &id).await;
    assert_eq!(status, 200, "expected GetSignalMap to answer 200, got {status}");
    assert_eq!(
        untouched["monitorDeployment"],
        Value::Null,
        "expected the refused calls to have started no deployment, got {:?}",
        untouched["monitorDeployment"]
    );

    // A signal map whose discovery has not completed has no monitor of its own
    // to delete yet; the same call succeeds once the clock has settled that
    // discovery, so the refusal is about the state and not about the identifier.
    let discovering =
        signal_map_discovering(&router, "discovery in progress", "delete-still-discovering").await;
    let (status, body) = start_delete_monitor_deployment(&router, &discovering).await;
    assert_eq!(
        status, 409,
        "case \"discovery in progress\": expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "ConflictException",
        "case \"discovery in progress\": expected ConflictException, got {body}"
    );
    wait_for_the_clock();
    let (status, body) = start_delete_monitor_deployment(&router, &discovering).await;
    assert_eq!(
        status, 202,
        "case \"discovery settled\": expected the same call to answer 202, got {status}: {body}"
    );

    // A deployment of either kind that is still running is deploying the monitor
    // a deletion would delete.
    for (case, body) in [
        ("real deployment in progress", None),
        ("dry run in progress", Some(json!({ "dryRun": true }))),
    ] {
        let router = medialive();
        let name = format!("delete-busy-{}", case.replace(' ', "-"));
        let id = signal_map_ready_to_deploy(&router, case, &name).await;
        let (status, started) = start_monitor_deployment(&router, &id, body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the deployment to answer 202, got {status}: {started}"
        );
        let running = started["monitorDeployment"]["status"].clone();

        let (status, refused) = start_delete_monitor_deployment(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 while a deployment runs, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );

        // The running deployment is the one the deploy call started: the refused
        // deletion neither replaced it nor changed its kind.
        let (status, polled) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {polled}"
        );
        assert_eq!(
            polled["monitorDeployment"]["status"], running,
            "case {case:?}: expected the running deployment to be unchanged by the refusal, \
             got {:?}",
            polled["monitorDeployment"]
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! MediaLive `StartMonitorDeployment`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: one recorded call is ONE row of each. The wire harness
//! (`medialive`, `call`, `error_code`, `error_message`) is the crate's shared one
//! in `wire`, and the clock wait is the channel suite's — reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, error_message, medialive, DEFAULT_REGION};

/// The URIs this suite drives (`smithy_query(query="operation",
/// operation="StartMonitorDeployment,GetSignalMap,ListSignalMaps")`:
/// POST /prod/signal-maps/{Identifier}/monitor-deployment,
/// GET /prod/signal-maps/{Identifier}, GET /prod/signal-maps).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// `StartMonitorDeployment` for `identifier`, with `body` sent verbatim (the
/// only member in the payload is `DryRun`; `Identifier` is an `@httpLabel`).
async fn start_monitor_deployment(
    router: &MultiServiceRouter,
    identifier: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("{SIGNAL_MAPS}/{identifier}/monitor-deployment"),
        body,
    )
    .await
}

/// `GetSignalMap` for `identifier`.
async fn get_signal_map(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(router, "GET", &format!("{SIGNAL_MAPS}/{identifier}"), None).await
}

/// The `SignalMapSummary` `ListSignalMaps` reports for `id`.
async fn listed_summary(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", SIGNAL_MAPS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListSignalMaps to answer 200, got {status}: {body}"
    );
    body["signalMaps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["id"] == json!(id))
        .unwrap_or_else(|| panic!("case {case:?}: expected {id:?} to be listed, got {body}"))
}

/// Create a channel to enter discovery from, create the signal map `name` on it
/// and let the clock settle its discovery, so the map has a monitor to deploy.
/// Answers the signal map's id.
async fn signal_map_ready_to_deploy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let id = signal_map_discovering(router, case, name).await;
    wait_for_the_clock();
    let (status, read) = get_signal_map(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetSignalMap to answer 200, got {status}: {read}"
    );
    assert_eq!(
        read["status"],
        json!("CREATE_COMPLETE"),
        "case {case:?}: expected discovery to have settled, got {:?}",
        read["status"]
    );
    id
}

/// Create a channel and a signal map discovered from it, WITHOUT waiting for the
/// clock: the signal map is still `CREATE_IN_PROGRESS` when this returns.
async fn signal_map_discovering(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let (status, channel) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": format!("{name}-entry") })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateChannel to answer 201, got {status}: {channel}"
    );
    let entry_point = channel["channel"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, created) = call(
        router,
        "POST",
        SIGNAL_MAPS,
        Some(json!({ "name": name, "discoveryEntryPointArn": entry_point })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
    );
    assert_eq!(
        created["monitorDeployment"],
        Value::Null,
        "case {case:?}: expected no monitor deployment on a fresh signal map, got {:?}",
        created["monitorDeployment"]
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// Assert `deployment` is the modelled `MonitorDeployment` of `id` in `status`,
/// and answer its `DetailsUri`.
fn deployment_details_uri(case: &str, id: &str, status_name: &str, deployment: &Value) -> String {
    assert_eq!(
        deployment["status"],
        json!(status_name),
        "case {case:?}: expected monitorDeployment.status {status_name}, got {deployment}"
    );
    assert_eq!(
        deployment["errorMessage"],
        Value::Null,
        "case {case:?}: expected no errorMessage on a deployment nothing failed, got {deployment}"
    );
    let details_uri = deployment["detailsUri"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a detailsUri, got {deployment}"))
        .to_string();
    // `MonitorDeployment$DetailsUri` is a `__stringMin1Max2048` URI
    // "associated with a signal map's monitor deployment"; this emulator's form
    // is the console page of the signal map the deployment belongs to, in the
    // region its ARN names, so the URI identifies that signal map.
    assert!(
        details_uri.starts_with("https://") && (1..=2048).contains(&details_uri.chars().count()),
        "case {case:?}: expected an https URI within the modelled length, got {details_uri:?}"
    );
    assert!(
        details_uri.contains(id) && details_uri.contains(DEFAULT_REGION),
        "case {case:?}: expected the detailsUri to name the signal map and its region, \
         got {details_uri:?}"
    );
    details_uri
}

/// One row of the deployment-kind table: the payload a caller sends and the pair
/// of statuses the deployment it starts must move through.
struct Deployment {
    case: &'static str,
    /// The request payload, sent verbatim; `None` is a request with no body at
    /// all, which `DryRun` being optional allows.
    body: Option<Value>,
    /// Whether the row asked for a dry run, which decides both the statuses and
    /// whether a successful deployment is left behind.
    dry_run: bool,
    /// Whether the call addresses the signal map by its name rather than its id
    /// (`Identifier` is "either be its id or current name").
    by_name: bool,
}

/// Rule: `StartMonitorDeployment` starts a monitor deployment of the signal map
/// its `Identifier` resolves to, answers 202 with that signal map as the store
/// now holds it, and reports a `MonitorDeployment` in the IN_PROGRESS status of
/// the kind of deployment the request asked for — the dry-run one when `DryRun`
/// is set, the real one otherwise — which the clock settles into the matching
/// COMPLETE status; only a real deployment becomes the signal map's
/// `LastSuccessfulMonitorDeployment`.
///
/// The rows differ along what the rule generalises over: whether `DryRun` was
/// asked for and HOW (absent payload, empty payload, explicit `false`, explicit
/// `true`), and whether the signal map was addressed by id or by name. The
/// statuses are the model's own (`smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
#[tokio::test]
async fn a_monitor_deployment_runs_the_kind_of_deployment_the_request_asked_for() {
    let table = [
        Deployment {
            case: "no payload at all",
            body: None,
            dry_run: false,
            by_name: false,
        },
        Deployment {
            case: "empty payload",
            body: Some(json!({})),
            dry_run: false,
            by_name: true,
        },
        Deployment {
            case: "dryRun false",
            body: Some(json!({ "dryRun": false })),
            dry_run: false,
            by_name: true,
        },
        Deployment {
            case: "dryRun true",
            body: Some(json!({ "dryRun": true })),
            dry_run: true,
            by_name: false,
        },
    ];

    for Deployment {
        case,
        body,
        dry_run,
        by_name,
    } in table
    {
        let in_progress = if dry_run {
            "DRY_RUN_DEPLOYMENT_IN_PROGRESS"
        } else {
            "DEPLOYMENT_IN_PROGRESS"
        };
        let complete = if dry_run {
            "DRY_RUN_DEPLOYMENT_COMPLETE"
        } else {
            "DEPLOYMENT_COMPLETE"
        };

        let router = medialive();
        let name = format!("deploy-{}", case.replace(' ', "-"));
        let id = signal_map_ready_to_deploy(&router, case, &name).await;
        let identifier = if by_name { name.clone() } else { id.clone() };

        let (status, started) = start_monitor_deployment(&router, &identifier, body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected StartMonitorDeployment to answer 202, got {status}: {started}"
        );
        assert_eq!(
            started["id"],
            json!(id),
            "case {case:?}: expected the response to be the signal map {id:?}, got {:?}",
            started["id"]
        );
        let details_uri = deployment_details_uri(case, &id, in_progress, &started["monitorDeployment"]);
        assert_eq!(
            started["lastSuccessfulMonitorDeployment"],
            Value::Null,
            "case {case:?}: expected no successful deployment while the first one runs, got {:?}",
            started["lastSuccessfulMonitorDeployment"]
        );
        assert_eq!(
            started["monitorChangesPendingDeployment"],
            json!(false),
            "case {case:?}: expected no pending monitor changes, got {:?}",
            started["monitorChangesPendingDeployment"]
        );

        // The deployment is asynchronous, so a caller polling straight after the
        // call still sees it running — and reading it does not complete it.
        let (status, polled) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {polled}"
        );
        assert_eq!(
            deployment_details_uri(case, &id, in_progress, &polled["monitorDeployment"]),
            details_uri,
            "case {case:?}: expected the stored detailsUri to be stable across reads"
        );
        let summary = listed_summary(&router, case, &id).await;
        assert_eq!(
            summary["monitorDeploymentStatus"],
            json!(in_progress),
            "case {case:?}: expected the summary to report the running deployment, got {:?}",
            summary["monitorDeploymentStatus"]
        );

        wait_for_the_clock();
        let (status, settled) = get_signal_map(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {settled}"
        );
        assert_eq!(
            deployment_details_uri(case, &id, complete, &settled["monitorDeployment"]),
            details_uri,
            "case {case:?}: expected the completed deployment to keep its detailsUri"
        );
        // A dry run deploys nothing, so it is not a successful monitor
        // deployment OF the signal map; a real one is.
        let expected_successful = if dry_run {
            Value::Null
        } else {
            json!({ "detailsUri": details_uri, "status": complete })
        };
        assert_eq!(
            settled["lastSuccessfulMonitorDeployment"], expected_successful,
            "case {case:?}: expected lastSuccessfulMonitorDeployment {expected_successful}, got {:?}",
            settled["lastSuccessfulMonitorDeployment"]
        );
        let summary = listed_summary(&router, case, &id).await;
        assert_eq!(
            summary["monitorDeploymentStatus"],
            json!(complete),
            "case {case:?}: expected the summary to report the completed deployment, got {:?}",
            summary["monitorDeploymentStatus"]
        );
    }
}

/// Rule: an `Identifier` that resolves to no signal map is refused with
/// `NotFoundException` (404), and a signal map that cannot take a deployment in
/// the state it is in — its discovery still running, or a deployment of its own
/// still running — is refused with `ConflictException` (409); a refused call
/// starts nothing.
///
/// The rows differ along WHY the identifier resolves to nothing (an id of the
/// signal-map range nobody minted, an id of another resource's range, a name no
/// signal map carries, an ARN rather than an identifier) and along WHICH state
/// refuses the deployment, because the rule is about resolution and state, not
/// about any one value.
#[tokio::test]
async fn a_deployment_is_refused_on_an_unknown_identifier_or_a_busy_signal_map() {
    let router = medialive();
    let name = "refusals";
    let id = signal_map_ready_to_deploy(&router, "refusals", name).await;

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
        let (status, body) = start_monitor_deployment(&router, &identifier, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an identifier naming no signal map, got {status}: {body}"
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

    // The signal map the refused calls named is untouched: no deployment was
    // started on it.
    let (status, untouched) = get_signal_map(&router, &id).await;
    assert_eq!(status, 200, "expected GetSignalMap to answer 200, got {status}");
    assert_eq!(
        untouched["monitorDeployment"],
        Value::Null,
        "expected the refused calls to have started no deployment, got {:?}",
        untouched["monitorDeployment"]
    );

    // A signal map whose discovery has not completed has no monitor to deploy
    // yet; the same call succeeds once the clock has settled that discovery, so
    // the refusal is about the state and not about the identifier.
    let discovering = signal_map_discovering(&router, "discovery in progress", "still-discovering").await;
    let (status, body) = start_monitor_deployment(&router, &discovering, None).await;
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
    let (status, body) = start_monitor_deployment(&router, &discovering, None).await;
    assert_eq!(
        status, 202,
        "case \"discovery settled\": expected the same call to answer 202, got {status}: {body}"
    );

    // A deployment that is still running is already doing what a second call
    // would start — for both kinds of deployment.
    for (case, body) in [
        ("real deployment in progress", None),
        ("dry run in progress", Some(json!({ "dryRun": true }))),
    ] {
        let router = medialive();
        let name = format!("busy-{}", case.replace(' ', "-"));
        let id = signal_map_ready_to_deploy(&router, case, &name).await;
        let (status, started) = start_monitor_deployment(&router, &id, body.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the first deployment to answer 202, got {status}: {started}"
        );
        let running = started["monitorDeployment"]["status"].clone();

        for second in [None, Some(json!({ "dryRun": true }))] {
            let (status, refused) = start_monitor_deployment(&router, &id, second).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected 409 while a deployment runs, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {refused}"
            );
        }

        // The running deployment is the one the first call started: the refused
        // calls neither replaced it nor changed its kind.
        let (status, polled) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {polled}"
        );
        assert_eq!(
            polled["monitorDeployment"]["status"], running,
            "case {case:?}: expected the running deployment to be unchanged by the refusals, got {:?}",
            polled["monitorDeployment"]
        );
    }
}

/// Rule: `StartMonitorDeployment` deploys "the latest monitor" every time it is
/// called, so a signal map whose monitor was already deployed is deployed again
/// rather than refused, and `LastSuccessfulMonitorDeployment` keeps the latest
/// deployment that actually deployed something — never a dry run, and never
/// cleared while a new deployment runs.
///
/// The rows are the ORDERS a caller can deploy in: dry run then real, real then
/// real, and real then dry run. What differs between them is which call leaves a
/// successful deployment behind.
#[tokio::test]
async fn redeploying_keeps_the_latest_deployment_that_deployed_something() {
    // (case, the sequence of `dryRun` values to deploy with)
    let table: [(&str, &[bool]); 4] = [
        ("dry run then real", &[true, false]),
        ("real then real", &[false, false]),
        ("real then dry run", &[false, true]),
        ("dry run twice", &[true, true]),
    ];

    for (case, sequence) in table {
        let router = medialive();
        let name = format!("redeploy-{}", case.replace(' ', "-"));
        let id = signal_map_ready_to_deploy(&router, case, &name).await;
        // The successful deployment expected so far: `None` until a real
        // deployment has completed.
        let mut successful: Value = Value::Null;

        for (step, dry_run) in sequence.iter().enumerate() {
            let (status, started) =
                start_monitor_deployment(&router, &id, Some(json!({ "dryRun": dry_run }))).await;
            assert_eq!(
                status, 202,
                "case {case:?} step {step}: expected 202, got {status}: {started}"
            );
            // A deployment that is running does not erase the last successful
            // one.
            assert_eq!(
                started["lastSuccessfulMonitorDeployment"], successful,
                "case {case:?} step {step}: expected the earlier success to survive a new \
                 deployment, got {:?}",
                started["lastSuccessfulMonitorDeployment"]
            );

            wait_for_the_clock();
            let (status, settled) = get_signal_map(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step}: expected GetSignalMap to answer 200, got {status}: {settled}"
            );
            let complete = if *dry_run {
                "DRY_RUN_DEPLOYMENT_COMPLETE"
            } else {
                "DEPLOYMENT_COMPLETE"
            };
            let details_uri = deployment_details_uri(case, &id, complete, &settled["monitorDeployment"]);
            if !dry_run {
                successful = json!({ "detailsUri": details_uri, "status": complete });
            }
            assert_eq!(
                settled["lastSuccessfulMonitorDeployment"], successful,
                "case {case:?} step {step}: expected lastSuccessfulMonitorDeployment {successful}, \
                 got {:?}",
                settled["lastSuccessfulMonitorDeployment"]
            );
        }
    }
}

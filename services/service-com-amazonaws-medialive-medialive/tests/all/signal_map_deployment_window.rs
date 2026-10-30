// SPDX-License-Identifier: CC0-1.0

//! What a MediaLive signal map's mutations do while a monitor deployment of it
//! is still in flight, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one call is ONE row of each. The wire harness
//! (`medialive`, `call`, `error_code`, `error_message`), the clock wait, the
//! signal-map readers and the entry-point helper are the crate's shared ones —
//! reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::monitor_deployment::{
    get_signal_map, monitor_deployment_uri, signal_map_ready_to_deploy, start_monitor_deployment,
};
use crate::support::entry_point;
use crate::wire::{call, error_code, error_message, medialive};

/// The signal-map collection URI, and the prefix of the one
/// `StartUpdateSignalMap` PATCHes and `DeleteSignalMap` DELETEs
/// (`smithy_query(query="operation",
/// operation="StartUpdateSignalMap,DeleteSignalMap")`: PATCH
/// /prod/signal-maps/{Identifier}, DELETE /prod/signal-maps/{Identifier}).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// The collection URIs of the two template-group resources a signal map can be
/// associated with (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplateGroup,CreateEventBridgeRuleTemplateGroup")`).
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const RULE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// `StartUpdateSignalMap` on `identifier`, with `body` sent verbatim (every
/// input member but the `@httpLabel` `Identifier` travels in the payload).
async fn start_update(router: &MultiServiceRouter, identifier: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("{SIGNAL_MAPS}/{identifier}"),
        Some(body),
    )
    .await
}

/// `DeleteSignalMap` for `identifier` — the sibling mutation whose refusal in
/// this same window is the behaviour an update has to match.
async fn delete_signal_map(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("{SIGNAL_MAPS}/{identifier}"),
        None,
    )
    .await
}

/// `StartDeleteMonitorDeployment` for `identifier`, which starts the one
/// monitor-deployment kind `start_monitor_deployment` does not.
async fn start_delete_monitor_deployment(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, Value) {
    call(router, "DELETE", &monitor_deployment_uri(identifier), None).await
}

/// One template group on `uri`, answering its minted id.
async fn create_group(router: &MultiServiceRouter, case: &str, uri: &str, name: &str) -> String {
    let (status, created) = call(router, "POST", uri, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected the group {name:?} on {uri} to answer 201, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// The ARNs a response's `MediaResourceMap` is keyed by, in sorted order.
fn graph_keys(case: &str, body: &Value) -> Vec<String> {
    let mut keys: Vec<String> = body["mediaResourceMap"]
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a mediaResourceMap object, got {:?}",
                body["mediaResourceMap"]
            )
        })
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The members of a signal map an update may rewrite, as a later read reports
/// them: the snapshot a refused update must leave exactly as it was.
fn snapshot(case: &str, body: &Value) -> Value {
    json!({
        "name": body["name"].clone(),
        "description": body["description"].clone(),
        "discoveryEntryPointArn": body["discoveryEntryPointArn"].clone(),
        "cloudWatchAlarmTemplateGroupIds": body["cloudWatchAlarmTemplateGroupIds"].clone(),
        "eventBridgeRuleTemplateGroupIds": body["eventBridgeRuleTemplateGroupIds"].clone(),
        "lastDiscoveredAt": body["lastDiscoveredAt"].clone(),
        "status": body["status"].clone(),
        "mediaResourceMap": json!(graph_keys(case, body)),
    })
}

/// One kind of monitor deployment a signal map can have in flight: how to start
/// it, the `SignalMapMonitorDeploymentStatus` it runs in and the one it settles
/// to (`smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
struct Deployment {
    case: &'static str,
    /// `Some(dry_run)` deploys the monitor, `None` deletes it.
    kind: Option<bool>,
    running: &'static str,
    settled: &'static str,
}

/// Start this row's deployment on `identifier`.
async fn start_deployment(
    router: &MultiServiceRouter,
    deployment: &Deployment,
    identifier: &str,
) -> (u16, Value) {
    match deployment.kind {
        Some(dry_run) => {
            start_monitor_deployment(router, identifier, Some(json!({ "dryRun": dry_run }))).await
        }
        None => start_delete_monitor_deployment(router, identifier).await,
    }
}

/// Rule: a signal map carries one asynchronous monitor deployment at a time, and
/// while one is in flight it is deploying the very monitor contents a mutation
/// of the signal map would rewrite — so every mutation of that signal map is
/// refused with `ConflictException` (409), writes nothing, and is served once
/// the clock has settled the deployment.
///
/// The rows differ along what the rule generalises over: WHICH deployment kind
/// is in flight (a real deployment, a dry run, a deletion of the monitor), and
/// — inside each window — which mutation is attempted, which update members the
/// request carries (a forced rediscovery, a changed entry point, either
/// template-group association, a description, a rename) and whether the call
/// addresses the signal map by its id or by its current name.
#[tokio::test]
async fn a_running_monitor_deployment_refuses_every_mutation_of_the_signal_map() {
    let table = [
        Deployment {
            case: "real deployment in flight",
            kind: Some(false),
            running: "DEPLOYMENT_IN_PROGRESS",
            settled: "DEPLOYMENT_COMPLETE",
        },
        Deployment {
            case: "dry run in flight",
            kind: Some(true),
            running: "DRY_RUN_DEPLOYMENT_IN_PROGRESS",
            settled: "DRY_RUN_DEPLOYMENT_COMPLETE",
        },
        Deployment {
            case: "monitor deletion in flight",
            kind: None,
            running: "DELETE_IN_PROGRESS",
            settled: "DELETE_COMPLETE",
        },
    ];

    for deployment in table {
        let case = deployment.case;
        let router = medialive();
        let name = format!("deploying-{}", case.replace(' ', "-"));
        let id = signal_map_ready_to_deploy(&router, case, &name).await;

        // The resources the update bodies below name, all created BEFORE the
        // deployment starts, so no row's refusal is about a missing resource.
        let other_entry = entry_point(&router, &format!("{name}-other")).await;
        let alarm_group = create_group(&router, case, ALARM_GROUPS, &format!("{name}-alarm")).await;
        let rule_group = create_group(&router, case, RULE_GROUPS, &format!("{name}-rule")).await;

        let (status, before) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {before}"
        );
        let before = snapshot(case, &before);

        let (status, started) = start_deployment(&router, &deployment, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the deployment to answer 202, got {status}: {started}"
        );
        assert_eq!(
            started["monitorDeployment"]["status"],
            json!(deployment.running),
            "case {case:?}: expected the deployment to be {:?}, got {:?}",
            deployment.running,
            started["monitorDeployment"]["status"]
        );

        // Every mutation of the signal map, attempted inside that window.
        // (what the call changes, by name rather than by id, the update body —
        // `None` for the mutations that are not an update)
        let mutations: [(&str, bool, Option<Value>); 8] = [
            ("forced rediscovery", false, Some(json!({ "forceRediscovery": true }))),
            (
                "changed entry point",
                true,
                Some(json!({ "discoveryEntryPointArn": other_entry })),
            ),
            (
                "alarm template groups",
                false,
                Some(json!({ "cloudWatchAlarmTemplateGroupIdentifiers": [alarm_group] })),
            ),
            (
                "rule template groups",
                true,
                Some(json!({ "eventBridgeRuleTemplateGroupIdentifiers": [rule_group] })),
            ),
            (
                "description only",
                false,
                Some(json!({ "description": "written under a deployment" })),
            ),
            ("rename", true, Some(json!({ "name": format!("{name}-renamed") }))),
            ("empty update", false, Some(json!({}))),
            ("delete", false, None),
        ];

        for (mutation, by_name, body) in mutations {
            let identifier = if by_name { name.clone() } else { id.clone() };
            let (status, refused) = match body {
                Some(body) => start_update(&router, &identifier, body).await,
                None => delete_signal_map(&router, &identifier).await,
            };
            assert_eq!(
                status, 409,
                "case {case:?} / {mutation:?}: expected 409 while {} , got {status}: {refused}",
                deployment.running
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?} / {mutation:?}: expected ConflictException, got {refused}"
            );
            let message = error_message(&refused);
            assert!(
                message.contains(&id) && message.contains(deployment.running),
                "case {case:?} / {mutation:?}: expected the message to name {id:?} and {:?}, \
                 got {message:?}",
                deployment.running
            );
        }

        // A second deployment of any kind is refused in the same window, which
        // is the same rule read from the deployment's own side.
        let (status, second) = start_deployment(&router, &deployment, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a second deployment to be refused, got {status}: {second}"
        );

        // Nothing any of those calls asked for was written: the signal map is
        // still the one the deployment is deploying, and still deploying.
        let (status, during) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the refusals to leave the signal map readable, \
             got {status}: {during}"
        );
        assert_eq!(
            during["monitorDeployment"]["status"],
            json!(deployment.running),
            "case {case:?}: expected the deployment to still be {:?}, got {:?}",
            deployment.running,
            during["monitorDeployment"]["status"]
        );
        assert_eq!(
            snapshot(case, &during),
            before,
            "case {case:?}: expected a refused mutation to have written nothing"
        );

        // The refusals are about the state, not about the requests: once the
        // clock has settled the deployment, the same update is served and the
        // deployment it waited for settled on the monitor it was deploying.
        wait_for_the_clock();
        let (status, settled) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {settled}"
        );
        assert_eq!(
            settled["monitorDeployment"]["status"],
            json!(deployment.settled),
            "case {case:?}: expected the deployment to settle to {:?}, got {:?}",
            deployment.settled,
            settled["monitorDeployment"]["status"]
        );
        assert_eq!(
            snapshot(case, &settled),
            before,
            "case {case:?}: expected the settled deployment to have changed nothing \
             a mutation would change"
        );

        let (status, served) = start_update(
            &router,
            &id,
            json!({ "forceRediscovery": true, "cloudWatchAlarmTemplateGroupIdentifiers": [alarm_group] }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the same update to answer 202 once the deployment settled, \
             got {status}: {served}"
        );
        assert_eq!(
            served["status"],
            json!("UPDATE_IN_PROGRESS"),
            "case {case:?}: expected the served update to be running, got {:?}",
            served["status"]
        );
        assert_eq!(
            served["cloudWatchAlarmTemplateGroupIds"],
            json!([alarm_group]),
            "case {case:?}: expected the served update to write the group association, got {:?}",
            served["cloudWatchAlarmTemplateGroupIds"]
        );

        // And the update it now runs reads back through `GetSignalMap`, so what
        // the refusals withheld is what the served call wrote.
        wait_for_the_clock();
        let (status, updated) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {updated}"
        );
        assert_eq!(
            updated["status"],
            json!("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to have settled, got {:?}",
            updated["status"]
        );
        assert_eq!(
            updated["cloudWatchAlarmTemplateGroupIds"],
            json!([alarm_group]),
            "case {case:?}: expected the stored group association to read back, got {:?}",
            updated["cloudWatchAlarmTemplateGroupIds"]
        );
        assert!(
            !graph_keys(case, &updated).is_empty(),
            "case {case:?}: expected the rediscovered graph to hold the entry point"
        );
    }
}

/// Rule: the refusal is about work in flight on THAT signal map, so a monitor
/// deployment of one signal map leaves every other signal map mutable.
///
/// The rows are the number of OTHER signal maps the store holds beside the one
/// deploying, because the isolation is per resource and not per store.
#[tokio::test]
async fn a_deployment_of_one_signal_map_leaves_the_others_mutable() {
    for others in [1_usize, 2, 3] {
        let case = format!("{others} other signal maps");
        let router = medialive();
        let deploying =
            signal_map_ready_to_deploy(&router, &case, &format!("busy-{others}")).await;

        let mut idle: Vec<String> = Vec::new();
        for other in 0..others {
            idle.push(
                signal_map_ready_to_deploy(&router, &case, &format!("idle-{others}-{other}")).await,
            );
        }

        let (status, started) = start_monitor_deployment(&router, &deploying, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the deployment to answer 202, got {status}: {started}"
        );

        let (status, refused) =
            start_update(&router, &deploying, json!({ "forceRediscovery": true })).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the deploying signal map to refuse an update, \
             got {status}: {refused}"
        );

        for (other, id) in idle.iter().enumerate() {
            let (status, served) = start_update(
                &router,
                id,
                json!({ "description": format!("other {other}") }),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?} other {other}: expected an unrelated signal map to be updatable, \
                 got {status}: {served}"
            );
            assert_eq!(
                served["description"],
                json!(format!("other {other}")),
                "case {case:?} other {other}: expected the description to be written, got {:?}",
                served["description"]
            );
        }
    }
}

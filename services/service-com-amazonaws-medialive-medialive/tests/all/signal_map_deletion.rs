// SPDX-License-Identifier: CC0-1.0

//! MediaLive `DeleteSignalMap`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: one call is ONE row of each. The wire harness
//! (`medialive`, `call`, `error_code`, `error_message`), the clock wait, the
//! template-group helpers and the signal-map readers are the crate's shared ones
//! — reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::monitor_deployment::{
    get_signal_map, monitor_deployment_uri, signal_map_discovering, start_monitor_deployment,
};
use crate::support::{create_in, entry_point, GROUPS};
use crate::wire::{call, error_code, error_message, medialive, DEFAULT_REGION};

/// The signal-map collection URI (`smithy_query(query="operation",
/// operation="DeleteSignalMap,CreateSignalMap,ListSignalMaps")`: DELETE
/// /prod/signal-maps/{Identifier}, POST /prod/signal-maps, GET
/// /prod/signal-maps).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// `DeleteSignalMap` for `identifier` (`Identifier` is the operation's only
/// input member and an `@httpLabel`, so the request carries no payload).
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

/// The ids `ListSignalMaps` reports right now, in the order it reports them.
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", SIGNAL_MAPS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListSignalMaps to answer 200, got {status}: {body}"
    );
    body["signalMaps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `ListTagsForResource` for `arn` (`GET /prod/tags/{ResourceArn}`).
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/prod/tags/{}", crate::support::percent_encoded(arn)),
        None,
    )
    .await
}

/// `CreateSignalMap` on a channel created to enter discovery from, with
/// `extras`' members merged into the request body, letting the clock settle the
/// discovery. Answers the created signal map's `Id` and `Arn`.
async fn signal_map(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    extras: &Value,
) -> (String, String) {
    let entry = entry_point(router, &format!("{name}-entry")).await;
    let mut body = json!({ "name": name, "discoveryEntryPointArn": entry });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    let (status, created) = call(router, "POST", SIGNAL_MAPS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
    );
    wait_for_the_clock();
    (
        created["id"].as_str().unwrap_or_default().to_string(),
        created["arn"].as_str().unwrap_or_default().to_string(),
    )
}

/// One cloudwatch alarm template group named `name`, answering its minted `Id` —
/// a group a signal map can be created against, and whose own delete documents
/// that it must be detached from every signal map first.
async fn alarm_template_group(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = create_in(router, DEFAULT_REGION, json!({ "name": name })).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCloudWatchAlarmTemplateGroup to answer 201, \
         got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// Run the monitor deployments `monitor` names on `id`, letting the clock
/// complete each: `Some(dry_run)` deploys the monitor, `None` deletes it.
async fn settled_monitor_history(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    monitor: &[Option<bool>],
) {
    for (step, deployment) in monitor.iter().enumerate() {
        let (status, started) = match deployment {
            Some(dry_run) => {
                start_monitor_deployment(router, id, Some(json!({ "dryRun": dry_run }))).await
            }
            None => start_delete_monitor_deployment(router, id).await,
        };
        assert_eq!(
            status, 202,
            "case {case:?} monitor step {step}: expected 202, got {status}: {started}"
        );
        wait_for_the_clock();
    }
}

/// One row of the deletion table: the signal map to build, the monitor history
/// to build it through, and how the delete addresses it.
struct Deletion {
    case: &'static str,
    /// Whether the delete names the signal map by its current name rather than
    /// by its id (`Identifier` is "either be its id or current name").
    by_name: bool,
    /// The monitor deployments to run and let complete before the delete:
    /// `Some(dry_run)` deploys the monitor, `None` deletes it. Empty is a signal
    /// map nobody ever deployed a monitor for.
    monitor: &'static [Option<bool>],
    /// Whether the signal map carries tags and an attached cloudwatch alarm
    /// template group.
    attached: bool,
    /// How many OTHER signal maps the store holds beside the one deleted.
    others: usize,
}

/// Rule: `DeleteSignalMap` removes the signal map its `Identifier` resolves to —
/// by id or by current name — answers 204 with no payload, and leaves nothing of
/// it behind: every later read of either of its identifiers misses, it is no
/// longer listed, its ARN names no resource for the tag reader, its name is free
/// for another signal map and the template groups it named are no longer held by
/// it, while every other signal map is untouched.
///
/// The rows differ along what the rule generalises over: the monitor-deployment
/// history the delete is called from (none, a real deployment, a dry run then a
/// monitor deletion), whether the signal map carries tags and an attached
/// template group, how many other signal maps the store holds, and whether the
/// delete addresses the signal map by id or by name.
#[tokio::test]
async fn deleting_a_signal_map_removes_it_and_everything_derived_from_it() {
    let table = [
        Deletion {
            case: "fresh signal map by id",
            by_name: false,
            monitor: &[],
            attached: false,
            others: 0,
        },
        Deletion {
            case: "deployed monitor by name",
            by_name: true,
            monitor: &[Some(false)],
            attached: false,
            others: 1,
        },
        Deletion {
            case: "dry run then monitor deletion",
            by_name: false,
            monitor: &[Some(true), None],
            attached: true,
            others: 2,
        },
        Deletion {
            case: "tagged and attached by name",
            by_name: true,
            monitor: &[],
            attached: true,
            others: 1,
        },
    ];

    for Deletion {
        case,
        by_name,
        monitor,
        attached,
        others,
    } in table
    {
        let router = medialive();
        let name = format!("delete-{}", case.replace(' ', "-"));
        let mut extras = json!({});
        let group = if attached {
            let group = alarm_template_group(&router, case, &format!("{name}-group")).await;
            extras["cloudWatchAlarmTemplateGroupIdentifiers"] = json!([group]);
            extras["tags"] = json!({ "crew": "ops" });
            Some(group)
        } else {
            None
        };
        let (id, arn) = signal_map(&router, case, &name, &extras).await;
        settled_monitor_history(&router, case, &id, monitor).await;

        // The other signal maps this store holds, which the delete must leave
        // exactly as they are.
        let mut survivors: Vec<String> = Vec::new();
        for other in 0..others {
            let (other_id, _) =
                signal_map(&router, case, &format!("{name}-other-{other}"), &json!({})).await;
            survivors.push(other_id);
        }

        let identifier = if by_name { name.clone() } else { id.clone() };
        let (status, deleted) = delete_signal_map(&router, &identifier).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected DeleteSignalMap to answer 204, got {status}: {deleted}"
        );
        assert_eq!(
            deleted,
            Value::Null,
            "case {case:?}: expected no payload on a 204, got {deleted}"
        );

        // Gone by BOTH forms of its identifier, not just the one the delete
        // named.
        for (form, gone) in [("id", id.clone()), ("name", name.clone())] {
            let (status, body) = get_signal_map(&router, &gone).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected GetSignalMap by {form} to answer 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {case:?}: expected NotFoundException by {form}, got {body}"
            );
            assert!(
                error_message(&body).contains(&gone),
                "case {case:?}: expected the message to name {gone:?}, got {:?}",
                error_message(&body)
            );
        }

        let listed = listed_ids(&router, case).await;
        assert!(
            !listed.contains(&id),
            "case {case:?}: expected {id:?} to be gone from ListSignalMaps, got {listed:?}"
        );
        assert_eq!(
            listed, survivors,
            "case {case:?}: expected the other signal maps to survive the delete, got {listed:?}"
        );

        // The ARN the signal map held names no resource any more, so the tag
        // reader misses rather than reporting the tags of a deleted resource.
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected ListTagsForResource on the deleted ARN to answer 404, \
             got {status}: {body}"
        );

        // A second delete of a signal map that is gone finds nothing to delete:
        // the resolution misses, exactly as a read of it does.
        let (status, body) = delete_signal_map(&router, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a repeated delete to answer 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException on a repeated delete, got {body}"
        );

        // A template group the deleted signal map named is no longer held by it,
        // so the group's own delete — which refuses a group any signal map still
        // names — now goes through.
        if let Some(group) = group {
            let (status, body) = call(&router, "DELETE", &format!("{GROUPS}/{group}"), None).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected the detached group {group:?} to be deletable, \
                 got {status}: {body}"
            );
        }

        // The name is free again: a new signal map may take it, and it is a NEW
        // signal map rather than the deleted one coming back.
        let (reused, _) = signal_map(&router, case, &name, &json!({})).await;
        assert_ne!(
            reused, id,
            "case {case:?}: expected a freshly minted id for the reused name, got {reused:?}"
        );
    }
}

/// Rule: a signal map that still has work of its own in flight — its own
/// lifecycle action, or a monitor deployment of it — cannot be deleted out from
/// under that work and is refused with `ConflictException` (409); the refusal
/// names the signal map and the status it is in, deletes nothing, and the same
/// call succeeds once the clock has settled that work.
///
/// The rows differ along WHICH in-flight work refuses the delete, because the
/// rule is about the state the signal map is in and not about any one call. The
/// statuses are the model's own (`smithy_query(query="shape",
/// shape="SignalMapStatus")`: CREATE_IN_PROGRESS | CREATE_COMPLETE |
/// CREATE_FAILED | UPDATE_IN_PROGRESS | UPDATE_COMPLETE | UPDATE_REVERTED |
/// UPDATE_FAILED | READY | NOT_READY; `smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
#[tokio::test]
async fn a_delete_is_refused_while_the_signal_map_has_work_in_flight() {
    // Each row names the work in flight and the status the refusal must report.
    let table: [(&str, Option<Option<bool>>, &str); 4] = [
        ("discovery in progress", None, "CREATE_IN_PROGRESS"),
        (
            "real deployment in progress",
            Some(Some(false)),
            "DEPLOYMENT_IN_PROGRESS",
        ),
        (
            "dry run in progress",
            Some(Some(true)),
            "DRY_RUN_DEPLOYMENT_IN_PROGRESS",
        ),
        (
            "monitor deletion in progress",
            Some(None),
            "DELETE_IN_PROGRESS",
        ),
    ];

    for (case, deployment, running) in table {
        let router = medialive();
        let name = format!("busy-{}", case.replace(' ', "-"));
        let id = signal_map_discovering(&router, case, &name).await;
        if let Some(deployment) = deployment {
            // This row's in-flight work is a deployment, so the signal map's own
            // discovery has to have settled before it can start.
            wait_for_the_clock();
            let (status, started) = match deployment {
                Some(dry_run) => {
                    start_monitor_deployment(&router, &id, Some(json!({ "dryRun": dry_run }))).await
                }
                None => start_delete_monitor_deployment(&router, &id).await,
            };
            assert_eq!(
                status, 202,
                "case {case:?}: expected the deployment to answer 202, got {status}: {started}"
            );
        }

        let (status, refused) = delete_signal_map(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 while work is in flight, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains(&id) && message.contains(running),
            "case {case:?}: expected the message to name {id:?} and {running:?}, got {message:?}"
        );

        // The refused delete removed nothing: the signal map is still there, in
        // the state the refusal reported.
        let (status, polled) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the refused delete to leave the signal map readable, \
             got {status}: {polled}"
        );

        // The refusal is about the state, not the identifier: once the clock has
        // settled that work, the same call deletes the signal map.
        wait_for_the_clock();
        let (status, body) = delete_signal_map(&router, &id).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the settled signal map to be deletable, got {status}: {body}"
        );
        let (status, body) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted signal map to be gone, got {status}: {body}"
        );
    }
}

/// Rule: an `Identifier` that resolves to no signal map is refused with
/// `NotFoundException` (404) whose message names the identifier the request
/// carried, and such a call deletes nothing.
///
/// The rows differ along WHY the identifier resolves to nothing — an id of the
/// signal-map range nobody minted, an id of another resource's range, a name no
/// signal map carries, and an ARN rather than an identifier — because the rule is
/// about resolution and not about any one value.
#[tokio::test]
async fn a_delete_is_refused_on_an_identifier_naming_no_signal_map() {
    let router = medialive();
    let name = "delete-not-found";
    let (id, _) = signal_map(&router, "not found", name, &json!({})).await;

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
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("{SIGNAL_MAPS}/{}", crate::support::percent_encoded(&identifier)),
            None,
        )
        .await;
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

    // The signal map those calls did not name is untouched by their refusals.
    let (status, untouched) = get_signal_map(&router, &id).await;
    assert_eq!(
        status, 200,
        "expected the stored signal map to survive every refused delete, got {status}: {untouched}"
    );
    assert_eq!(
        listed_ids(&router, "not found").await,
        vec![id],
        "expected the store to still hold exactly the one signal map"
    );
}

/// Rule: a `RequestId` (`@idempotencyToken`) never outlives the signal map it
/// created, so once that signal map is deleted the same token creates a NEW
/// signal map rather than answering with the deleted one.
///
/// The rows are the NUMBER of create-then-delete rounds the one token runs
/// through, which is what the invariant generalises over: each round must mint a
/// signal map of its own.
#[tokio::test]
async fn a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token() {
    for rounds in [1_usize, 2, 3] {
        let case = format!("{rounds} rounds on one token");
        let router = medialive();
        let token = "retry-token";
        let mut minted: Vec<String> = Vec::new();

        for round in 0..rounds {
            let name = format!("token-{rounds}-{round}");
            let (id, _) = signal_map(
                &router,
                &case,
                &name,
                &json!({ "requestId": token }),
            )
            .await;
            assert!(
                !minted.contains(&id),
                "case {case:?} round {round}: expected a freshly minted id, got {id:?} \
                 among {minted:?}"
            );
            minted.push(id.clone());

            let (status, body) = delete_signal_map(&router, &id).await;
            assert_eq!(
                status, 204,
                "case {case:?} round {round}: expected 204, got {status}: {body}"
            );
        }

        assert_eq!(
            listed_ids(&router, &case).await,
            Vec::<String>::new(),
            "case {case:?}: expected every signal map the token created to be deleted"
        );
    }
}

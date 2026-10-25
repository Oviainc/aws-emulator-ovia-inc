// SPDX-License-Identifier: CC0-1.0

//! `BatchStop`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: a single running channel is ONE row of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_BATCH_STOP_IDS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::multiplex_lifecycle::{describe_multiplex, settings};
use crate::multiplex_start::{multiplex_with_settings, start_multiplex};
use crate::wire::{call, error_code, medialive};

/// The URI `BatchStop` is bound to (`smithy_query(query="operation",
/// operation="BatchStop")`: POST /prod/batch/stop).
const BATCH_STOP: &str = "/prod/batch/stop";

/// An id no resource of any kind was minted with: the mint starts every id
/// family above a million (`state::FIRST_CHANNEL_ID` and its siblings).
const UNKNOWN_ID: &str = "0000001";

/// One row of the refusal table: the case, the request member the id travels
/// in, the setup that builds the resource, the error code the entry must carry,
/// the state the refused resource keeps (`None` when no resource has that id)
/// and the text its `Message` must contain.
type RefusalRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
);

/// `BatchStop` with `body`, asserting the modelled 200 and returning the two
/// result lists as `(successful, failed)`.
async fn batch_stop(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> (Vec<Value>, Vec<Value>) {
    let (status, response) = call(router, "POST", BATCH_STOP, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchStop: expected 200, got {status}: {response}"
    );
    let successful = response["successful"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a successful list, got {response}"));
    let failed = response["failed"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a failed list, got {response}"));
    (successful, failed)
}

/// A channel created from `body`, still in `CREATING`.
async fn creating_channel(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {response}"
    );
    response["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateChannel reported no id: {response}"))
        .to_string()
}

/// `count` channels, each created and started, left `RUNNING` — the one state a
/// stop acts from. The clock is waited on once per transition rather than once
/// per channel, so a row naming several channels costs no more waiting than a
/// row naming one.
async fn running_channels(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            creating_channel(router, case, json!({ "name": format!("batch-stop-{index}") })).await,
        );
    }
    if ids.is_empty() {
        return ids;
    }
    wait_for_the_clock();
    for id in &ids {
        let (status, started) =
            call(router, "POST", &format!("/prod/channels/{id}/start"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartChannel {id}: expected 200, got {status}: {started}"
        );
    }
    wait_for_the_clock();
    for id in &ids {
        let (status, described) = describe(router, id).await;
        assert_eq!(
            described["state"],
            json!("RUNNING"),
            "case {case:?}: channel {id} did not reach RUNNING ({status}): {described}"
        );
    }
    ids
}

/// `count` multiplexes, each created and started, left `RUNNING`.
async fn running_multiplexes(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            multiplex_with_settings(
                router,
                case,
                &format!("batch-stop-mux-{index}"),
                settings(1_000_000 + i64::try_from(index).unwrap_or_default(), 1),
            )
            .await,
        );
    }
    if ids.is_empty() {
        return ids;
    }
    wait_for_the_clock();
    for id in &ids {
        let (status, started) = start_multiplex(router, id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StartMultiplex {id}: expected 202, got {status}: {started}"
        );
    }
    wait_for_the_clock();
    for id in &ids {
        let (status, described) = describe_multiplex(router, id).await;
        assert_eq!(
            described["state"],
            json!("RUNNING"),
            "case {case:?}: multiplex {id} did not reach RUNNING ({status}): {described}"
        );
    }
    ids
}

/// The reader URI of one resource, per resource kind.
fn reader(member: &str, id: &str) -> String {
    match member {
        "channelIds" => format!("/prod/channels/{id}"),
        "multiplexIds" => format!("/prod/multiplexes/{id}"),
        other => panic!("no implemented reader for {other}"),
    }
}

/// Rule: `BatchStop` stops each running resource the request names and reports
/// it in `Successful` with its id, its ARN and the state the stop left it in,
/// read back out of the store — the `STOPPING` state the resource's own state
/// enum models, which the clock then settles into the stopped `IDLE`.
///
/// The rows differ along the resource kinds the request's members name
/// (`smithy_query(query="operation", operation="BatchStop")`: ChannelIds,
/// MultiplexIds) and along how many of each one call names: a single running
/// channel is one row. The states come from the model's own enums
/// (`smithy_query(query="shape", shape="ChannelState")`, `shape="MultiplexState"`).
#[tokio::test]
async fn every_running_resource_is_stopped_and_reported_in_the_state_it_reached() {
    // (case, running channels to name, running multiplexes to name)
    let table: [(&str, usize, usize); 6] = [
        ("one channel", 1, 0),
        ("two channels", 2, 0),
        ("one multiplex", 0, 1),
        ("two multiplexes", 0, 2),
        ("one of each", 1, 1),
        ("several of both kinds", 2, 3),
    ];

    for (case, channels, multiplexes) in table {
        let router = medialive();
        let channel_ids = running_channels(&router, case, channels).await;
        let multiplex_ids = running_multiplexes(&router, case, multiplexes).await;

        // The ARN each resource reports before the stop is the one its result
        // entry must carry.
        let mut named: Vec<(&str, String, Value)> = Vec::new();
        for id in &channel_ids {
            let (status, before) = call(&router, "GET", &reader("channelIds", id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading channel {id}: got {status}: {before}"
            );
            named.push(("channelIds", id.clone(), before["arn"].clone()));
        }
        for id in &multiplex_ids {
            let (status, before) = call(&router, "GET", &reader("multiplexIds", id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading multiplex {id}: got {status}: {before}"
            );
            named.push(("multiplexIds", id.clone(), before["arn"].clone()));
        }

        let (successful, failed) = batch_stop(
            &router,
            case,
            json!({ "channelIds": channel_ids, "multiplexIds": multiplex_ids }),
        )
        .await;
        assert!(
            failed.is_empty(),
            "case {case:?}: expected no failures, got {failed:?}"
        );
        let expected: Vec<Value> = named
            .iter()
            .map(|(_, id, arn)| json!({ "id": id, "arn": arn, "state": "STOPPING" }))
            .collect();
        assert_eq!(
            successful, expected,
            "case {case:?}: expected every named resource stopped once, got {successful:?}"
        );

        // The store, not the response, is what the stop moved: every reader
        // reports STOPPING and then the stopped IDLE the clock owes.
        for (member, id, arn) in &named {
            let (status, after) = call(&router, "GET", &reader(member, id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {id} to still be readable, got {status}: {after}"
            );
            assert_eq!(
                after["state"],
                json!("STOPPING"),
                "case {case:?}: expected the reader of {id} to report STOPPING, got {:?}",
                after["state"]
            );
            assert_eq!(
                &after["arn"], arn,
                "case {case:?}: expected the ARN of {id} to survive the stop, got {:?}",
                after["arn"]
            );
        }

        wait_for_the_clock();
        for (member, id, _) in &named {
            let (status, settled) = call(&router, "GET", &reader(member, id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {id} to still be readable, got {status}: {settled}"
            );
            assert_eq!(
                settled["state"],
                json!("IDLE"),
                "case {case:?}: expected the stop of {id} to settle into IDLE, got {:?}",
                settled["state"]
            );
        }
    }
}

/// Rule: a resource `BatchStop` cannot stop is reported in `Failed` with the
/// error code the single-resource stop of that resource answers with — an id the
/// account does not hold is `NotFoundException`, and a channel whose state is
/// not the `RUNNING` a stop acts from is `ConflictException` — and the resource
/// is left exactly as it was.
///
/// The rows span the states this service can put a channel in other than
/// `RUNNING`, both request members, and the two shapes an entry takes: with the
/// resource's ARN when the resource exists and without one when no resource has
/// that id. A multiplex has one refusal only, because a multiplex that is not
/// running is a no-op rather than a conflict (asserted by the suite below).
#[tokio::test]
async fn every_resource_a_stop_cannot_touch_is_reported_as_its_own_failure() {
    // (case, request member, setup, expected code, state the resource keeps,
    //  the text the message must contain)
    let table: [RefusalRow; 6] = [
        (
            "unknown channel",
            "channelIds",
            "none",
            "NotFoundException",
            None,
            "Channel {id} not found",
        ),
        (
            "creating channel",
            "channelIds",
            "creating channel",
            "ConflictException",
            Some("CREATING"),
            "Channel {id} is in state CREATING and can only be stopped from RUNNING",
        ),
        (
            "idle channel",
            "channelIds",
            "idle channel",
            "ConflictException",
            Some("IDLE"),
            "Channel {id} is in state IDLE and can only be stopped from RUNNING",
        ),
        (
            "channel already stopping",
            "channelIds",
            "stopping channel",
            "ConflictException",
            Some("STOPPING"),
            "Channel {id} is in state STOPPING and can only be stopped from RUNNING",
        ),
        (
            "deleting channel",
            "channelIds",
            "deleting channel",
            "ConflictException",
            Some("DELETING"),
            "Channel {id} is in state DELETING and can only be stopped from RUNNING",
        ),
        (
            "unknown multiplex",
            "multiplexIds",
            "none",
            "NotFoundException",
            None,
            "Multiplex {id} not found",
        ),
    ];

    for (case, member, setup, expected_code, kept_state, expected_message) in table {
        let router = medialive();
        let id = match setup {
            "none" => UNKNOWN_ID.to_string(),
            "creating channel" => {
                creating_channel(&router, case, json!({ "name": "still-creating" })).await
            }
            "idle channel" => {
                let id = creating_channel(&router, case, json!({ "name": "idle" })).await;
                wait_for_the_clock();
                id
            }
            "deleting channel" => {
                let id = creating_channel(&router, case, json!({ "name": "to-delete" })).await;
                wait_for_the_clock();
                let (status, deleted) =
                    call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannel: got {status}: {deleted}"
                );
                id
            }
            // The effective stop is the FIRST one; the one under test is the
            // second, sent while the STOPPING transition is still owed.
            _ => {
                let id = running_channels(&router, case, 1)
                    .await
                    .pop()
                    .unwrap_or_else(|| panic!("case {case:?}: no channel was started"));
                let (successful, failed) =
                    batch_stop(&router, case, json!({ "channelIds": [id.clone()] })).await;
                assert!(
                    failed.is_empty(),
                    "case {case:?}: the first stop was refused: {failed:?}"
                );
                assert_eq!(
                    successful[0]["state"],
                    json!("STOPPING"),
                    "case {case:?}: the test did not reach STOPPING, it reached {:?}",
                    successful[0]
                );
                id
            }
        };

        let (successful, failed) =
            batch_stop(&router, case, json!({ member: [id.clone()] })).await;
        assert!(
            successful.is_empty(),
            "case {case:?}: expected no accepted stop, got {successful:?}"
        );
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one failed entry, got {failed:?}"
        );
        let entry = &failed[0];
        assert_eq!(
            entry["id"],
            json!(id),
            "case {case:?}: expected the failure to name the requested id, got {entry:?}"
        );
        assert_eq!(
            entry["code"],
            json!(expected_code),
            "case {case:?}: expected code {expected_code}, got {entry:?}"
        );
        let expected_message = expected_message.replace("{id}", &id);
        assert!(
            entry["message"]
                .as_str()
                .is_some_and(|text| text.contains(&expected_message)),
            "case {case:?}: expected a message containing {expected_message:?}, got {entry:?}"
        );

        match kept_state {
            // A resource that exists carries its ARN into the failure, and the
            // refusal left it in the state it was already in.
            Some(state) => {
                let (status, after) = call(&router, "GET", &reader(member, &id), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the refused resource to still be readable, got {status}: {after}"
                );
                assert_eq!(
                    after["state"],
                    json!(state),
                    "case {case:?}: expected the refused resource to keep state {state}, got {:?}",
                    after["state"]
                );
                assert_eq!(
                    entry["arn"], after["arn"],
                    "case {case:?}: expected the failure to carry the resource's ARN, got {entry:?}"
                );
            }
            // No resource has that id, so there is no ARN to report.
            None => assert!(
                entry["arn"].is_null(),
                "case {case:?}: expected no ARN for an id nothing holds, got {entry:?}"
            ),
        }
    }
}

/// Rule: a `BatchStop` entry for a multiplex that is NOT running has no effect —
/// it is reported in `Successful` with the multiplex exactly as it stands, starts
/// no `STOPPING` transition and does not disturb the transition the clock
/// already owed it, which is what `StopMultiplex` documents ("Stops a running
/// multiplex. If the multiplex isn't running, this action has no effect.").
///
/// The rows span the non-running states this service can put a multiplex in: one
/// still coming up, one already stopped and one already stopping (the repeat
/// stop, which is what makes the stop idempotent).
#[tokio::test]
async fn a_multiplex_that_is_not_running_is_reported_unchanged() {
    // (case, the state the multiplex is in when the stop is sent, the states
    //  that stop may report — the state itself, plus the one the clock may
    //  already have settled it into — and the state the clock settles it into)
    let rows: [(&str, &str, &[&str], &str); 3] = [
        ("while still creating", "CREATING", &["CREATING", "IDLE"], "IDLE"),
        ("while already stopped", "IDLE", &["IDLE"], "IDLE"),
        ("while already stopping", "STOPPING", &["STOPPING", "IDLE"], "IDLE"),
    ];

    for (case, state, reported, settled) in rows {
        let router = medialive();
        let id = match state {
            "CREATING" => {
                multiplex_with_settings(&router, case, "not-running", settings(2_000_000, 1)).await
            }
            "IDLE" => {
                let id = multiplex_with_settings(
                    &router,
                    case,
                    "not-running",
                    settings(2_000_000, 1),
                )
                .await;
                wait_for_the_clock();
                id
            }
            // Running it first, then stopping it: the stop under test is the
            // second one, sent while the STOPPING transition is still owed.
            _ => {
                let id = running_multiplexes(&router, case, 1)
                    .await
                    .pop()
                    .unwrap_or_else(|| panic!("case {case:?}: no multiplex was started"));
                let (successful, failed) =
                    batch_stop(&router, case, json!({ "multiplexIds": [id.clone()] })).await;
                assert!(
                    failed.is_empty(),
                    "case {case:?}: the first stop was refused: {failed:?}"
                );
                assert_eq!(
                    successful[0]["state"],
                    json!("STOPPING"),
                    "case {case:?}: the test did not reach STOPPING, it reached {:?}",
                    successful[0]
                );
                id
            }
        };

        let (status, before) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {before}"
        );

        let (successful, failed) =
            batch_stop(&router, case, json!({ "multiplexIds": [id.clone()] })).await;
        assert!(
            failed.is_empty(),
            "case {case:?}: expected a no-op stop to be reported as successful, got {failed:?}"
        );
        assert_eq!(
            successful.len(),
            1,
            "case {case:?}: expected exactly one successful entry, got {successful:?}"
        );
        let entry = &successful[0];
        assert_eq!(
            entry["arn"], before["arn"],
            "case {case:?}: expected the entry to carry the multiplex's ARN, got {entry:?}"
        );
        let answered = entry["state"].as_str().unwrap_or_default();
        assert!(
            reported.contains(&answered),
            "case {case:?}: expected the stop of a {state} multiplex to report one of {reported:?}, got {answered:?}"
        );

        // And it did not disturb what the clock already owed: the multiplex
        // reaches the state it was heading for, not one a stop forced on it.
        wait_for_the_clock();
        let (status, after) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the clock: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after["state"],
            json!(settled),
            "case {case:?}: expected the multiplex to settle into {settled}, got {:?}",
            after["state"]
        );
    }
}

/// Rule: the resources named in one `BatchStop` are attempted independently, so
/// the call answers 200 and reports every accepted stop in `Successful` and
/// every refused one in `Failed`, however the request mixes them.
///
/// The rows differ along how many resources are named, how many of them are
/// running and how many ids name nothing at all — the row naming nothing follows
/// from the model, whose `BatchStopRequest` has no required member.
#[tokio::test]
async fn one_call_reports_the_accepted_and_the_refused_side_by_side() {
    // (case, running channels, idle channels, unknown channel ids)
    let table: [(&str, usize, usize, usize); 6] = [
        ("nothing named", 0, 0, 0),
        ("one running channel and one unknown id", 1, 0, 1),
        ("two running channels and one unknown id", 2, 0, 1),
        ("one running channel and two unknown ids", 1, 0, 2),
        ("running channels only", 3, 0, 0),
        ("running and idle channels and an unknown id", 2, 1, 1),
    ];

    for (case, running, idle, unknown) in table {
        let router = medialive();
        let mut idle_ids = Vec::new();
        for index in 0..idle {
            idle_ids.push(
                creating_channel(&router, case, json!({ "name": format!("idle-{index}") })).await,
            );
        }
        let running_ids = running_channels(&router, case, running).await;
        if running == 0 && idle > 0 {
            // The idle channels still owe the clock their CREATING transition
            // when no running channel was built, which is what waits on it.
            wait_for_the_clock();
        }

        // The ids nothing holds come after the real ones, so the result entries
        // cannot be an artefact of the request's order.
        let unknown_ids: Vec<String> = (0..unknown).map(|index| format!("000000{index}")).collect();
        let mut requested = running_ids.clone();
        requested.extend(idle_ids.clone());
        requested.extend(unknown_ids.clone());

        let (successful, failed) =
            batch_stop(&router, case, json!({ "channelIds": requested })).await;

        let accepted: Vec<&str> = successful
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default())
            .collect();
        let expected_accepted: Vec<&str> = running_ids.iter().map(String::as_str).collect();
        assert_eq!(
            accepted, expected_accepted,
            "case {case:?}: expected every running channel accepted once, got {successful:?}"
        );
        for entry in &successful {
            assert_eq!(
                entry["state"],
                json!("STOPPING"),
                "case {case:?}: expected an accepted stop to report STOPPING, got {entry:?}"
            );
        }

        let refused: Vec<&str> = failed
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default())
            .collect();
        let mut expected_refused: Vec<&str> = idle_ids.iter().map(String::as_str).collect();
        expected_refused.extend(unknown_ids.iter().map(String::as_str));
        assert_eq!(
            refused, expected_refused,
            "case {case:?}: expected every channel a stop cannot touch refused once, got {failed:?}"
        );
        for entry in &failed {
            let code = if idle_ids.iter().any(|id| entry["id"] == json!(id)) {
                "ConflictException"
            } else {
                "NotFoundException"
            };
            assert_eq!(
                entry["code"],
                json!(code),
                "case {case:?}: expected code {code}, got {entry:?}"
            );
        }

        // Nothing but the running channels moved: each of them is STOPPING and
        // every refused one is still exactly where it was.
        for id in &running_ids {
            let (status, described) = describe(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected channel {id} to still be readable, got {status}: {described}"
            );
            assert_eq!(
                described["state"],
                json!("STOPPING"),
                "case {case:?}: expected channel {id} to be STOPPING, got {:?}",
                described["state"]
            );
        }
        for id in &idle_ids {
            let (status, described) = describe(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected channel {id} to still be readable, got {status}: {described}"
            );
            assert_eq!(
                described["state"],
                json!("IDLE"),
                "case {case:?}: expected the refused channel {id} to stay IDLE, got {:?}",
                described["state"]
            );
        }
    }
}

/// Rule: one call may name at most
/// [`MAX_BATCH_STOP_IDS`](service_com_amazonaws_medialive_medialive::state::MAX_BATCH_STOP_IDS)
/// resource ids across its two id lists — the model constrains neither of their
/// lengths, and each id costs a lookup and a projection plus an entry in the
/// response — and a batch above that bound is refused whole with the operation's
/// modelled `BadRequestException` rather than walked.
///
/// The rows span the bound itself and one id past it, each named in one member
/// and spread over both, because the cap is on their sum.
#[tokio::test]
async fn a_batch_above_the_bound_is_refused_whole() {
    let cap = MAX_BATCH_STOP_IDS;
    let half = cap / 2;
    // (case, ids per member in model order, whether the batch is refused)
    let table: [(&str, [usize; 2], bool); 5] = [
        ("at the bound, one member", [cap, 0], false),
        ("one past the bound, one member", [cap + 1, 0], true),
        ("at the bound, spread over both members", [half, cap - half], false),
        (
            "one past the bound, spread over both members",
            [half, cap + 1 - half],
            true,
        ),
        ("far past the bound", [cap + 1, cap + 1], true),
    ];
    let members = ["channelIds", "multiplexIds"];

    for (case, counts, is_refused) in table {
        let router = medialive();
        let mut body = json!({});
        let mut minted = 0;
        for (member, count) in members.iter().zip(counts) {
            // Ids nothing holds, so the row needs no resources: what is under
            // test is the bound, not the per-resource outcome.
            let ids: Vec<String> = (0..count)
                .map(|_| {
                    minted += 1;
                    format!("9{minted:06}")
                })
                .collect();
            body[*member] = json!(ids);
        }

        let (status, response) = call(&router, "POST", BATCH_STOP, Some(body)).await;
        if is_refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a batch of {} ids to be refused, got {status}: {response}",
                counts.iter().sum::<usize>()
            );
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a batch of {} ids to be answered, got {status}: {response}",
                counts.iter().sum::<usize>()
            );
            assert_eq!(
                response["failed"].as_array().map(Vec::len),
                Some(cap),
                "case {case:?}: expected one failure per named id, got {:?}",
                response["failed"]
            );
            assert_eq!(
                response["successful"],
                json!([]),
                "case {case:?}: expected no accepted stop, got {:?}",
                response["successful"]
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `BatchDelete`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded partial-failure case is ONE row of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_BATCH_DELETE_IDS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, medialive};

/// The URI `BatchDelete` is bound to (`smithy_query(query="operation",
/// operation="BatchDelete")`: POST /prod/batch/delete).
const BATCH_DELETE: &str = "/prod/batch/delete";

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

/// The fixed message MediaLive answers a channel delete with while the channel
/// has not stopped — the same sentence `DeleteChannel` carries for that class
/// (see `rejection_messages`), written out here rather than read from the crate
/// so the test pins the text instead of repeating it.
const CHANNEL_NOT_STOPPED: &str =
    "Cannot delete a channel that has not been stopped yet. Please stop the channel and try again.";

/// `BatchDelete` with `body`, asserting the modelled 200 and returning the two
/// result lists as `(successful, failed)`.
async fn batch_delete(router: &MultiServiceRouter, case: &str, body: Value) -> (Vec<Value>, Vec<Value>) {
    let (status, response) = call(router, "POST", BATCH_DELETE, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchDelete: expected 200, got {status}: {response}"
    );
    let successful = response["successful"].as_array().cloned().unwrap_or_else(|| {
        panic!("case {case:?}: expected a successful list, got {response}")
    });
    let failed = response["failed"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a failed list, got {response}"));
    (successful, failed)
}

/// A channel, created and left in `IDLE` — the only state a delete accepts.
async fn idle_channel(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let id = creating_channel(router, case, json!({ "name": name })).await;
    wait_for_the_clock();
    let (status, described) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        described["state"],
        json!("IDLE"),
        "case {case:?}: the channel did not settle to IDLE ({status}): {described}"
    );
    id
}

/// A channel created from `body`, still in `CREATING`.
async fn creating_channel(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let body = crate::support::resolved_channel_body(router, body).await;
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

/// An input created from `body`, with the resources its configuration names put
/// in the account first (`crate::support`).
async fn input(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let body = crate::support::resolved_input_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInput: expected 201, got {status}: {response}"
    );
    response["input"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateInput reported no id: {response}"))
        .to_string()
}

/// An input security group, created with one whitelist rule.
async fn security_group(router: &MultiServiceRouter, case: &str) -> String {
    let (status, response) = call(
        router,
        "POST",
        "/prod/inputSecurityGroups",
        Some(json!({ "whitelistRules": [{ "cidr": "203.0.113.0/24" }] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInputSecurityGroup: expected 201, got {status}: {response}"
    );
    response["securityGroup"]["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateInputSecurityGroup reported no id: {response}")
        })
        .to_string()
}

/// A multiplex, created and left in `IDLE` — the only state a delete accepts.
/// The two availability zones and the two required `MultiplexSettings` members
/// are what `CreateMultiplex` documents as mandatory.
async fn idle_multiplex(router: &MultiServiceRouter, case: &str) -> String {
    let id = creating_multiplex(router, case).await;
    wait_for_the_clock();
    let (status, described) = call(router, "GET", &format!("/prod/multiplexes/{id}"), None).await;
    assert_eq!(
        described["state"],
        json!("IDLE"),
        "case {case:?}: the multiplex did not settle to IDLE ({status}): {described}"
    );
    id
}

/// A multiplex, still in `CREATING`.
async fn creating_multiplex(router: &MultiServiceRouter, case: &str) -> String {
    let (status, response) = call(
        router,
        "POST",
        "/prod/multiplexes",
        Some(json!({
            "name": "batch-delete",
            "requestId": "batch-delete-multiplex",
            "availabilityZones": ["us-west-2a", "us-west-2c"],
            "multiplexSettings": {
                "transportStreamBitrate": 1_000_000,
                "transportStreamId": 1,
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateMultiplex: expected 201, got {status}: {response}"
    );
    response["multiplex"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateMultiplex reported no id: {response}"))
        .to_string()
}

/// The reader URI of one resource, per resource kind.
fn reader(member: &str, id: &str) -> String {
    match member {
        "channelIds" => format!("/prod/channels/{id}"),
        "inputIds" => format!("/prod/inputs/{id}"),
        "inputSecurityGroupIds" => format!("/prod/inputSecurityGroups/{id}"),
        "multiplexIds" => format!("/prod/multiplexes/{id}"),
        other => panic!("no implemented reader for {other}"),
    }
}

/// Rule: `BatchDelete` starts the delete of each resource the request names and
/// reports it in `Successful` with its id, its ARN and the state the delete left
/// it in, read back out of the store; a resource whose state enum models a
/// transient delete step is reported `DELETING` and settles into the terminal
/// `DELETED` it then keeps, and one whose enum models none reaches `DELETED`
/// with the call.
///
/// The table spans the resource kinds the request's members name
/// (`smithy_query(query="operation", operation="BatchDelete")`: ChannelIds,
/// InputIds, InputSecurityGroupIds, MultiplexIds) — the recorded channel case is
/// one row. The states come from the model's own enums
/// (`smithy_query(query="shape", shape="ChannelState")`, `shape="InputState"`,
/// `shape="InputSecurityGroupState"`, `shape="MultiplexState"`).
#[tokio::test]
async fn every_deletable_resource_is_accepted_and_reported_in_the_state_it_reached() {
    // (case, request member, state the accepted delete reports, whether the
    //  terminal DELETED is owed to the clock rather than reached with the call)
    let table: [(&str, &str, &str, bool); 4] = [
        ("idle channel", "channelIds", "DELETING", true),
        ("detached input", "inputIds", "DELETING", true),
        ("unused security group", "inputSecurityGroupIds", "DELETED", false),
        ("idle multiplex", "multiplexIds", "DELETING", true),
    ];

    for (case, member, expected_state, owed_to_the_clock) in table {
        let router = medialive();
        let id = match member {
            "channelIds" => idle_channel(&router, case, "batch-delete").await,
            "multiplexIds" => idle_multiplex(&router, case).await,
            "inputIds" => {
                input(
                    &router,
                    case,
                    json!({ "name": "batch-delete", "type": "URL_PULL" }),
                )
                .await
            }
            _ => security_group(&router, case).await,
        };

        // The ARN the resource reports before the delete is the one the result
        // entry must carry.
        let (status, before) = call(&router, "GET", &reader(member, &id), None).await;
        assert_eq!(status, 200, "case {case:?}: reading the resource: got {status}: {before}");
        let arn = before["arn"].clone();

        let (successful, failed) =
            batch_delete(&router, case, json!({ member: [id.clone()] })).await;
        assert!(
            failed.is_empty(),
            "case {case:?}: expected no failures, got {failed:?}"
        );
        assert_eq!(
            successful.len(),
            1,
            "case {case:?}: expected exactly one successful entry, got {successful:?}"
        );
        assert_eq!(
            successful[0],
            json!({ "id": id, "arn": arn, "state": expected_state }),
            "case {case:?}: expected the resource's id, ARN and {expected_state} state, got {:?}",
            successful[0]
        );

        // The resource stays readable through the delete: the account keeps the
        // record, so a reader reports the state the delete put it in.
        let (status, after) = call(&router, "GET", &reader(member, &id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted resource to still be readable, got {status}: {after}"
        );
        assert_eq!(
            after["state"],
            json!(expected_state),
            "case {case:?}: expected the reader to report {expected_state}, got {:?}",
            after["state"]
        );
        assert_eq!(
            after["arn"], arn,
            "case {case:?}: expected the ARN to survive the delete, got {:?}",
            after["arn"]
        );

        if owed_to_the_clock {
            wait_for_the_clock();
            let (status, settled) = call(&router, "GET", &reader(member, &id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the deleted resource to still be readable, got {status}: {settled}"
            );
            assert_eq!(
                settled["state"],
                json!("DELETED"),
                "case {case:?}: expected the delete to settle into the terminal DELETED, got {:?}",
                settled["state"]
            );
        }

        // Deleting it again follows that resource's own single-resource delete:
        // `DeleteInput` and the group's delete are idempotent once the state the
        // caller asks for is reached or on its way; `DeleteChannel` answers an
        // id whose delete it has already accepted as NOT FOUND, because that id
        // names no channel left to delete; and a multiplex delete refuses a
        // multiplex that is not IDLE with its state conflict.
        let (successful, failed) =
            batch_delete(&router, case, json!({ member: [id.clone()] })).await;
        let repeated_code = match member {
            "inputSecurityGroupIds" | "inputIds" => None,
            "channelIds" => Some("NotFoundException"),
            _ => Some("ConflictException"),
        };
        match repeated_code {
            None => {
                assert_eq!(
                    successful.len(),
                    1,
                    "case {case:?}: expected a repeated delete to be idempotent, got {failed:?}"
                );
                assert!(
                    failed.is_empty(),
                    "case {case:?}: expected no refusal on a repeated delete, got {failed:?}"
                );
                let (status, after) = call(&router, "GET", &reader(member, &id), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the resource to still be readable, got {status}: {after}"
                );
                assert_eq!(
                    after["state"],
                    successful[0]["state"],
                    "case {case:?}: expected the repeated delete to report the stored state, got {:?}",
                    successful[0]
                );
            }
            Some(code) => {
                assert!(
                    successful.is_empty(),
                    "case {case:?}: expected no second accepted delete, got {successful:?}"
                );
                assert_eq!(
                    failed.len(),
                    1,
                    "case {case:?}: expected the repeated delete to be refused, got {failed:?}"
                );
                assert_eq!(
                    failed[0]["code"],
                    json!(code),
                    "case {case:?}: expected {code}, got {:?}",
                    failed[0]
                );
            }
        }
    }
}

/// Rule: a resource `BatchDelete` cannot delete is reported in `Failed` with the
/// error code the single-resource delete of that resource answers with — an id
/// the account does not hold and a channel id whose delete it has already
/// accepted are `NotFoundException`, a resource that still exists whose state
/// does not allow the delete is `ConflictException`, and a resource another one
/// still uses is the client error its own delete models — and the resource is
/// left exactly as it was.
///
/// The table spans every refusal this slice can produce, over all four request
/// members, and the two shapes an entry takes: with the resource's ARN when the
/// resource exists and without one when no resource has that id.
///
/// Each row also carries the text the entry's `Message` must contain, with
/// `{id}` standing for the id the row requested: a refusal whose class carries a
/// FIXED message (a channel that has not stopped) must carry exactly that
/// sentence, and one whose message is composed from the request must name what
/// the request named.
#[tokio::test]
async fn every_resource_a_delete_cannot_touch_is_reported_as_its_own_failure() {
    // (case, request member, setup, expected code, state the resource keeps,
    //  the text the message must contain)
    let table: [RefusalRow; 9] = [
        ("unknown channel", "channelIds", "none", "NotFoundException", None, "Channel {id} not found"),
        (
            "creating channel",
            "channelIds",
            "creating channel",
            "ConflictException",
            Some("CREATING"),
            CHANNEL_NOT_STOPPED,
        ),
        (
            "running channel",
            "channelIds",
            "running channel",
            "ConflictException",
            Some("RUNNING"),
            CHANNEL_NOT_STOPPED,
        ),
        ("unknown input", "inputIds", "none", "NotFoundException", None, "Input {id} not found"),
        (
            "attached input",
            "inputIds",
            "attached input",
            "ConflictException",
            Some("ATTACHED"),
            "Input {id} is busy, it cannot be deleted",
        ),
        (
            "unknown security group",
            "inputSecurityGroupIds",
            "none",
            "NotFoundException",
            None,
            "InputSecurityGroup {id} not found",
        ),
        (
            "security group in use",
            "inputSecurityGroupIds",
            "used security group",
            "BadRequestException",
            Some("IN_USE"),
            "Input security group {id} is in use",
        ),
        (
            "unknown multiplex",
            "multiplexIds",
            "none",
            "NotFoundException",
            None,
            "Multiplex {id} not found",
        ),
        (
            "creating multiplex",
            "multiplexIds",
            "creating multiplex",
            "ConflictException",
            Some("CREATING"),
            "Multiplex {id} is in state CREATING",
        ),
    ];

    for (case, member, setup, expected_code, kept_state, expected_message) in table {
        let router = medialive();
        let id = match setup {
            "none" => UNKNOWN_ID.to_string(),
            "creating multiplex" => creating_multiplex(&router, case).await,
            "creating channel" => {
                creating_channel(&router, case, json!({ "name": "still-creating" })).await
            }
            "running channel" => {
                let id = idle_channel(&router, case, "to-run").await;
                let (status, started) =
                    call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
                assert_eq!(status, 200, "case {case:?}: StartChannel: got {status}: {started}");
                wait_for_the_clock();
                id
            }
            "attached input" => {
                let input_id = input(
                    &router,
                    case,
                    json!({ "name": "attached", "type": "URL_PULL" }),
                )
                .await;
                creating_channel(
                    &router,
                    case,
                    json!({
                        "name": "attaching",
                        "inputAttachments": [
                            { "inputAttachmentName": "primary", "inputId": input_id },
                        ],
                    }),
                )
                .await;
                input_id
            }
            _ => {
                let group_id = security_group(&router, case).await;
                input(
                    &router,
                    case,
                    json!({
                        "name": "user",
                        "type": "URL_PULL",
                        "inputSecurityGroups": [group_id.clone()],
                    }),
                )
                .await;
                group_id
            }
        };

        let (successful, failed) =
            batch_delete(&router, case, json!({ member: [id.clone()] })).await;
        assert!(
            successful.is_empty(),
            "case {case:?}: expected no accepted delete, got {successful:?}"
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

/// Rule: the resources named in one `BatchDelete` are attempted independently,
/// so the call answers 200 and reports every accepted delete in `Successful` and
/// every refused one in `Failed`, however the request mixes them.
///
/// The rows differ along how many resources are named, how many of them exist,
/// which kinds they span and in what order — the recorded one-of-each case is
/// one row, and the row naming nothing follows from the model, whose
/// `BatchDeleteRequest` has no required member.
#[tokio::test]
async fn one_call_reports_the_accepted_and_the_refused_side_by_side() {
    // (case, channels to create, inputs to create, unknown ids per member)
    let table: [(&str, usize, usize, usize); 6] = [
        ("nothing named", 0, 0, 0),
        ("one real channel and one unknown id", 1, 0, 1),
        ("two real channels and one unknown id", 2, 0, 1),
        ("one real channel and two unknown ids", 1, 0, 2),
        ("real channels only", 3, 0, 0),
        ("channels and inputs, both halves", 2, 2, 1),
    ];

    for (case, channels, inputs, unknown) in table {
        let router = medialive();
        let mut channel_ids = Vec::new();
        for index in 0..channels {
            channel_ids.push(idle_channel(&router, case, &format!("channel-{index}")).await);
        }
        let mut input_ids = Vec::new();
        for index in 0..inputs {
            input_ids.push(
                input(
                    &router,
                    case,
                    json!({ "name": format!("input-{index}"), "type": "URL_PULL" }),
                )
                .await,
            );
        }

        // The unknown ids are interleaved after the real ones, so the result
        // entries cannot be an artefact of the request's order.
        let unknown_ids: Vec<String> = (0..unknown)
            .map(|index| format!("000000{index}"))
            .collect();
        let mut requested_channels = channel_ids.clone();
        requested_channels.extend(unknown_ids.clone());
        let mut requested_inputs = input_ids.clone();
        if inputs > 0 {
            requested_inputs.extend(unknown_ids.clone());
        }

        let body = json!({
            "channelIds": requested_channels,
            "inputIds": requested_inputs,
        });
        let (successful, failed) = batch_delete(&router, case, body).await;

        let accepted: Vec<&str> = successful
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default())
            .collect();
        let mut expected_accepted: Vec<&str> =
            channel_ids.iter().map(String::as_str).collect();
        expected_accepted.extend(input_ids.iter().map(String::as_str));
        assert_eq!(
            accepted, expected_accepted,
            "case {case:?}: expected every existing resource accepted once, got {successful:?}"
        );
        for entry in &successful {
            assert_eq!(
                entry["state"],
                json!("DELETING"),
                "case {case:?}: expected an accepted delete to report DELETING, got {entry:?}"
            );
        }

        let refused: Vec<&str> = failed
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default())
            .collect();
        let expected_refusals = unknown * (1 + usize::from(inputs > 0));
        assert_eq!(
            refused.len(),
            expected_refusals,
            "case {case:?}: expected {expected_refusals} refusal(s), got {failed:?}"
        );
        for entry in &failed {
            assert_eq!(
                entry["code"],
                json!("NotFoundException"),
                "case {case:?}: expected NotFoundException for an id nothing holds, got {entry:?}"
            );
            assert!(
                unknown_ids.iter().any(|id| entry["id"] == json!(id)),
                "case {case:?}: a refusal named an id the request did not: {entry:?}"
            );
        }

        // Nothing but the named resources moved: every accepted channel is
        // DELETING and the account still holds exactly the resources that were
        // not named.
        for id in &channel_ids {
            let (status, described) =
                call(&router, "GET", &format!("/prod/channels/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected channel {id} to still be readable while deleting, got {status}: {described}"
            );
            assert_eq!(
                described["state"],
                json!("DELETING"),
                "case {case:?}: expected channel {id} to be DELETING, got {:?}",
                described["state"]
            );
        }
    }
}

/// Rule: one call may name at most
/// [`MAX_BATCH_DELETE_IDS`](service_com_amazonaws_medialive_medialive::state::MAX_BATCH_DELETE_IDS)
/// resource ids across its four id lists — the model constrains none of their
/// lengths, and each id costs a derivation over the stores plus an entry in the
/// response — and a batch above that bound is refused whole with the
/// operation's modelled `BadRequestException` rather than walked.
///
/// The rows span the bound itself and one id past it, each named in one member
/// and spread over all four, because the cap is on their sum.
#[tokio::test]
async fn a_batch_above_the_bound_is_refused_whole() {
    let cap = MAX_BATCH_DELETE_IDS;
    let quarter = cap / 4;
    // (case, ids per member in model order, whether the batch is refused)
    let table: [(&str, [usize; 4], bool); 4] = [
        ("at the bound, one member", [cap, 0, 0, 0], false),
        ("one past the bound, one member", [cap + 1, 0, 0, 0], true),
        (
            "at the bound, spread over every member",
            [quarter, quarter, quarter, cap - 3 * quarter],
            false,
        ),
        (
            "one past the bound, spread over every member",
            [quarter, quarter, quarter, cap + 1 - 3 * quarter],
            true,
        ),
    ];
    let members = [
        "channelIds",
        "inputIds",
        "inputSecurityGroupIds",
        "multiplexIds",
    ];

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

        let (status, response) = call(&router, "POST", BATCH_DELETE, Some(body)).await;
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
                "case {case:?}: expected no accepted delete, got {:?}",
                response["successful"]
            );
        }
    }
}

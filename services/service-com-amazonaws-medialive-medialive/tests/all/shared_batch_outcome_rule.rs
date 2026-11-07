// SPDX-License-Identifier: CC0-1.0

//! The ONE per-entry outcome rule and the ONE id-count bound every Batch
//! operation of this service answers by, driven end to end through each of those
//! operations.
//!
//! The rule: a Batch call reports each resource it names on its own — an id no
//! resource of that kind has in `Failed` with `NotFoundException`, the resource
//! type and the id, and no ARN; an accepted resource in `Successful` with its
//! id, its own ARN and the state the call left it in, read back out of the
//! store — and one call may name at most the ids its cap allows across ALL of
//! its id lists, a batch above that bound being refused whole with the modelled
//! `BadRequestException` naming the operation, the count and the cap. It is the
//! FAMILY's rule, not one operation's, so the table below spans the OPERATION:
//! `BatchDelete`, `BatchStart` and `BatchStop` each run every row.
//!
//! `tests/all/batch_delete.rs`, `tests/all/batch_start.rs` and
//! `tests/all/batch_stop.rs` pin what each operation DOES to each kind of
//! resource it walks; this suite pins that all three reach the same per-entry
//! report and the same bound, whichever one a client calls.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_BATCH_DELETE_IDS, MAX_BATCH_START_IDS, MAX_BATCH_STOP_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::wire::{call, error_code, error_message, medialive};

/// An id no resource of any kind was minted with: the mint starts every id
/// family above a million (`state::FIRST_CHANNEL_ID` and its siblings).
const UNKNOWN_ID: &str = "0000001";

/// The state a channel must stand in for one Batch operation to accept it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// The stopped state a delete and a start act from.
    Idle,
    /// The state a stop acts from.
    Running,
}

/// One row of the table: a Batch operation of this service, the request members
/// its ids travel in, the cap on their sum, and what it does to a channel.
struct Batch {
    /// The operation, as the model names it — also the name its bound refusal
    /// carries.
    op: &'static str,
    /// Its URI, from `smithy_query(query="operation", operation="<Operation>")`.
    uri: &'static str,
    /// Its id members in model order, each with the resource type the not-found
    /// entry for an id of that member names.
    members: &'static [(&'static str, &'static str)],
    /// The most ids one call may name across every member above.
    cap: usize,
    /// The channel standing it accepts.
    accepts: Standing,
    /// The state an accepted call leaves that channel in, from the model's own
    /// `ChannelState` enum (`smithy_query(query="shape", shape="ChannelState")`).
    reaches: &'static str,
}

/// Every Batch operation of this service, with the members and the cap of each:
/// `smithy_query(query="operation", operation="BatchDelete")` (POST
/// /prod/batch/delete, ChannelIds + InputIds + InputSecurityGroupIds +
/// MultiplexIds), `operation="BatchStart"` (POST /prod/batch/start, ChannelIds +
/// MultiplexIds) and `operation="BatchStop"` (POST /prod/batch/stop, the same
/// two members).
fn batch_operations() -> Vec<Batch> {
    vec![
        Batch {
            op: "BatchDelete",
            uri: "/prod/batch/delete",
            members: &[
                ("channelIds", "Channel"),
                ("inputIds", "Input"),
                ("inputSecurityGroupIds", "InputSecurityGroup"),
                ("multiplexIds", "Multiplex"),
            ],
            cap: MAX_BATCH_DELETE_IDS,
            accepts: Standing::Idle,
            reaches: "DELETING",
        },
        Batch {
            op: "BatchStart",
            uri: "/prod/batch/start",
            members: &[("channelIds", "Channel"), ("multiplexIds", "Multiplex")],
            cap: MAX_BATCH_START_IDS,
            accepts: Standing::Idle,
            reaches: "STARTING",
        },
        Batch {
            op: "BatchStop",
            uri: "/prod/batch/stop",
            members: &[("channelIds", "Channel"), ("multiplexIds", "Multiplex")],
            cap: MAX_BATCH_STOP_IDS,
            accepts: Standing::Running,
            reaches: "STOPPING",
        },
    ]
}

/// One Batch call with `body`, asserting the modelled 200 and answering the two
/// result lists as `(successful, failed)`.
async fn batch(
    router: &MultiServiceRouter,
    case: &str,
    row: &Batch,
    body: Value,
) -> (Vec<Value>, Vec<Value>) {
    let (status, response) = call(router, "POST", row.uri, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {} to answer 200, got {status}: {response}",
        row.op
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

/// A channel standing in the state `row` accepts, answering its id and the ARN
/// its reader reports before the Batch call.
async fn channel_in(router: &MultiServiceRouter, case: &str, row: &Batch) -> (String, Value) {
    let body = crate::support::resolved_channel_body(
        router,
        json!({ "name": format!("shared-batch-{}", row.op) }),
    )
    .await;
    let (status, created) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {created}"
    );
    let id = created["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateChannel reported no id: {created}"))
        .to_string();

    // The channel is CREATING until the clock settles it into the stopped IDLE
    // every Batch operation here acts from, directly or by way of a start.
    wait_for_the_clock();
    if row.accepts == Standing::Running {
        let (status, started) =
            call(router, "POST", &format!("/prod/channels/{id}/start"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartChannel {id}: expected 200, got {status}: {started}"
        );
        wait_for_the_clock();
    }

    let expected = match row.accepts {
        Standing::Idle => "IDLE",
        Standing::Running => "RUNNING",
    };
    let (status, described) = describe(router, &id).await;
    assert_eq!(
        described["state"],
        json!(expected),
        "case {case:?}: expected channel {id} to stand in {expected} ({status}): {described}"
    );
    (id, described["arn"].clone())
}

/// Rule: every Batch operation reports an id no resource of that kind has in
/// `Failed` with the not-found code its single-resource form answers (the
/// modelled exception's shape name), the resource type and the id in the
/// message, and no `Arn` — the call itself still answering 200, because each
/// named resource is reported on its own.
///
/// The rows span the OPERATION (all three Batch operations) and, within each,
/// every id member of its request — so each resource kind the family walks is
/// classified by the one shared rule — plus the row that names an unknown id in
/// EVERY member at once, because the entries of one call are independent.
#[tokio::test]
async fn every_batch_operation_reports_an_unknown_id_the_one_shared_way() {
    for row in batch_operations() {
        let router = medialive();

        // One member at a time: one entry, classified by the shared rule.
        for (member, resource_type) in row.members {
            let case = format!("{} {member}", row.op);
            let mut body = json!({});
            body[*member] = json!([UNKNOWN_ID]);
            let (successful, failed) = batch(&router, &case, &row, body).await;
            assert!(
                successful.is_empty(),
                "case {case:?}: expected nothing accepted for an id nothing holds, got \
                 {successful:?}"
            );
            assert_eq!(
                failed,
                vec![json!({
                    "code": "NotFoundException",
                    "id": UNKNOWN_ID,
                    "message": format!("{resource_type} {UNKNOWN_ID} not found"),
                })],
                "case {case:?}: expected the one shared not-found entry, got {failed:?}"
            );
        }

        // Every member at once: one entry per named id, each the same way.
        let case = format!("{} every member", row.op);
        let mut body = json!({});
        let mut expected: Vec<Value> = Vec::new();
        for (index, (member, resource_type)) in row.members.iter().enumerate() {
            let id = format!("000000{index}");
            body[*member] = json!([id]);
            expected.push(json!({
                "code": "NotFoundException",
                "id": id,
                "message": format!("{resource_type} {id} not found"),
            }));
        }
        let (successful, failed) = batch(&router, &case, &row, body).await;
        assert!(
            successful.is_empty(),
            "case {case:?}: expected nothing accepted, got {successful:?}"
        );
        assert_eq!(
            failed, expected,
            "case {case:?}: expected one shared not-found entry per named id, got {failed:?}"
        );
    }
}

/// Rule: every Batch operation reports a resource it accepted in `Successful`
/// with that resource's id, its own ARN and the state the call left it in, all
/// read back out of the store rather than out of the request — so the entry and
/// the resource's own reader agree.
///
/// The rows span the OPERATION: the three Batch operations act on a channel from
/// different standings (a delete and a start from the stopped `IDLE`, a stop
/// from `RUNNING`) and leave it in three different states, and all three report
/// that one entry the same way.
#[tokio::test]
async fn every_batch_operation_reports_an_accepted_resource_the_one_shared_way() {
    for row in batch_operations() {
        let router = medialive();
        let case = row.op.to_string();
        let (id, arn) = channel_in(&router, &case, &row).await;

        let (successful, failed) =
            batch(&router, &case, &row, json!({ "channelIds": [id.clone()] })).await;
        assert!(
            failed.is_empty(),
            "case {case:?}: expected no refusal for a channel in the state it acts from, got \
             {failed:?}"
        );
        assert_eq!(
            successful,
            vec![json!({ "arn": arn, "id": id, "state": row.reaches })],
            "case {case:?}: expected the one shared accepted entry, got {successful:?}"
        );

        // The store, not the response, is what the call moved: the channel's own
        // reader reports the same state and the same ARN the entry carried.
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected channel {id} to still be readable, got {status}: {after}"
        );
        assert_eq!(
            after["state"],
            json!(row.reaches),
            "case {case:?}: expected the reader to report {}, got {:?}",
            row.reaches,
            after["state"]
        );
        assert_eq!(
            after["arn"], arn,
            "case {case:?}: expected the ARN to survive the call, got {:?}",
            after["arn"]
        );
    }
}

/// Rule: one Batch call may name at most the ids its own cap allows, counted
/// across EVERY id list of the request, and a batch above that bound is refused
/// whole with the modelled `BadRequestException` naming the operation, the count
/// and the cap — rather than walked resource by resource.
///
/// The rows span the OPERATION and, within each, where the ids sit and how many
/// there are: at the bound and one past it, each in a single member and spread
/// over all of them, because the cap is on their sum.
#[tokio::test]
async fn every_batch_operation_bounds_its_ids_the_one_shared_way() {
    for row in batch_operations() {
        let cap = row.cap;
        let members = row.members.len();
        // (case, ids per member, whether the batch is refused whole)
        let plan: Vec<(String, Vec<usize>, bool)> = vec![
            (
                "at the bound, one member".to_string(),
                spread(cap, 1, members),
                false,
            ),
            (
                "one past the bound, one member".to_string(),
                spread(cap + 1, 1, members),
                true,
            ),
            (
                "at the bound, spread over every member".to_string(),
                spread(cap, members, members),
                false,
            ),
            (
                "one past the bound, spread over every member".to_string(),
                spread(cap + 1, members, members),
                true,
            ),
        ];

        for (shape, counts, is_refused) in plan {
            // A router of its own per row, so each batch is judged against a
            // store only this row filled.
            let router = medialive();
            let case = format!("{} {shape}", row.op);
            let named: usize = counts.iter().sum();
            let mut body = json!({});
            let mut minted = 0;
            for ((member, _), count) in row.members.iter().zip(&counts) {
                // Ids nothing holds, so the row needs no resources: what is
                // under test is the bound, not the per-resource outcome.
                let ids: Vec<String> = (0..*count)
                    .map(|_| {
                        minted += 1;
                        format!("9{minted:06}")
                    })
                    .collect();
                body[*member] = json!(ids);
            }

            let (status, response) = call(&router, "POST", row.uri, Some(body)).await;
            if is_refused {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a batch of {named} ids to be refused, got {status}: \
                     {response}"
                );
                assert_eq!(
                    error_code(status, &response),
                    "BadRequestException",
                    "case {case:?}: expected the modelled client error, got {response}"
                );
                assert_eq!(
                    error_message(&response),
                    format!(
                        "{} names {named} resources, more than the {cap} one call may name",
                        row.op
                    ),
                    "case {case:?}: expected the shared bound wording, got {:?}",
                    error_message(&response)
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a batch of {named} ids to be answered, got {status}: \
                     {response}"
                );
                assert_eq!(
                    response["failed"].as_array().map(Vec::len),
                    Some(named),
                    "case {case:?}: expected one failure per named id, got {:?}",
                    response["failed"]
                );
            }
        }
    }
}

/// `total` ids laid out over `members` lists, filling the first `over` of them
/// as evenly as their sum allows: the layout a bound row drives, so a row can
/// name its ids in one member or in all of them without a per-operation table.
fn spread(total: usize, over: usize, members: usize) -> Vec<usize> {
    let mut counts = vec![0; members];
    let each = total / over;
    for slot in counts.iter_mut().take(over) {
        *slot = each;
    }
    counts[over - 1] += total - each * over;
    counts
}

// SPDX-License-Identifier: CC0-1.0

//! How long a channel placement group `RequestId` stays a replay:
//! `CreateChannelPlacementGroup` answers a retry with the group its
//! `@idempotencyToken` minted only while that group is still one a caller may
//! use.
//!
//! `DeleteChannelPlacementGroup` is a tombstone delete — the record stays
//! readable, in the transient `DELETING` state and then in the terminal
//! `DELETED` one (`smithy_query(query="shape",
//! shape="ChannelPlacementGroupState")`) — so a retry carrying the token of a
//! group whose delete has been accepted cannot be answered with that record: it
//! names nothing a caller can use or change, and the name it carried is already
//! free for a new group. The retried create mints a group of its own instead.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the reported create, retried once its group had
//! settled into `DELETED`, is ONE row. The clusters, nodes and groups every row
//! works on are created through the service's own creates, so every id here is
//! one the store really minted — the request helpers are the ones the create and
//! update suites beside this one already have
//! ([`crate::channel_placement_groups`], [`crate::channel_placement_group_update`]).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_group_update::update_group;
use crate::channel_placement_groups::{
    create_cluster, create_group, create_nodes, created, described, group_uri, groups_uri, id_of,
    without_state,
};
use crate::wire::{call, medialive};

/// The states a group a delete has been accepted for reports — the transient one
/// the acceptance writes and the terminal one the clock owes it.
const DEAD_STATES: [&str; 2] = ["DELETING", "DELETED"];

/// `DeleteChannelPlacementGroup`, which has to be accepted (`200`, the status
/// the model declares for it).
async fn delete_group(router: &MultiServiceRouter, case: &str, cluster: &str, id: &str) {
    let (status, body) = call(router, "DELETE", &group_uri(cluster, id), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteChannelPlacementGroup: expected 200, got {status}: {body}"
    );
}

/// Every group id one cluster holds, sorted, as `ListChannelPlacementGroups`
/// reports them — a deleted group's record included, since the delete keeps it.
async fn held_ids(router: &MultiServiceRouter, case: &str, cluster: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", &groups_uri(cluster), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListChannelPlacementGroups: expected 200, got {status}: {body}"
    );
    let mut ids: Vec<String> = body["channelPlacementGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(id_of)
        .collect();
    ids.sort();
    ids
}

/// The `state` a reported group carries.
fn state_of(group: &Value) -> String {
    group["state"].as_str().unwrap_or_default().to_string()
}

/// One create of a group in `cluster` carrying `token`, with `name` and however
/// many of the cluster's own nodes the row asked for.
async fn create_with_token(
    router: &MultiServiceRouter,
    cluster: &str,
    token: &str,
    name: &str,
    nodes: &[String],
) -> (u16, Value) {
    let mut body = json!({ "name": name, "requestId": token });
    if !nodes.is_empty() {
        body["nodes"] = json!(nodes);
    }
    create_group(router, cluster, body).await
}

/// Rule: a create's `@idempotencyToken` answers a retry with the resource it
/// minted only while that resource is still one a caller may use — accepting the
/// delete of that resource ends the token's window, in the transient `DELETING`
/// half as much as in the terminal `DELETED` one — so the retry mints a NEW
/// resource and resurrects nothing.
///
/// The rows differ along what the rule generalises over: which half of the delete
/// lifecycle the retry lands in (before the clock has made the deletion due, and
/// after), which of the account's clusters the group belongs to, whether the
/// first group was associated with a node, and whether the retry asks for the
/// name the deleted group carried or a new one.
#[tokio::test]
async fn a_deleted_group_is_not_resurrected_by_its_idempotency_token() {
    // (case, which cluster, how many nodes the first group names, whether the
    //  retry waits for the clock, the name the retry asks for)
    let rows: [(&str, usize, usize, bool, &str); 6] = [
        ("retried inside the DELETING window", 0, 0, false, "group-b"),
        ("retried once the delete settled", 0, 0, true, "group-b"),
        (
            "an associated group, retried inside the DELETING window",
            0,
            1,
            false,
            "group-b",
        ),
        (
            "an associated group, retried once the delete settled",
            0,
            1,
            true,
            "group-b",
        ),
        (
            "the retry asks for the deleted group's own name",
            0,
            0,
            true,
            "group-a",
        ),
        (
            "the second cluster, retried once the delete settled",
            1,
            1,
            true,
            "group-b",
        ),
    ];

    for (case, which, node_count, wait, retried_name) in rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        let cluster = clusters[which].clone();
        let nodes = create_nodes(&router, case, &cluster, node_count).await;

        let token = "token-a";
        let first = created(
            &router,
            case,
            &cluster,
            {
                let mut body = json!({ "name": "group-a", "requestId": token });
                if !nodes.is_empty() {
                    body["nodes"] = json!(nodes);
                }
                body
            },
        )
        .await;
        let first_id = id_of(&first);

        // While the group is live the token IS a replay: the same create carrying
        // it answers with that group and stores nothing new.
        let (status, replayed) =
            create_with_token(&router, &cluster, token, retried_name, &nodes).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the live replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            id_of(&replayed),
            first_id,
            "case {case:?}: expected a live group's token to replay {first_id:?}, got {replayed}"
        );
        assert_eq!(
            held_ids(&router, case, &cluster).await,
            vec![first_id.clone()],
            "case {case:?}: expected the replay to have stored nothing, got {:?}",
            held_ids(&router, case, &cluster).await
        );

        delete_group(&router, case, &cluster, &first_id).await;
        if wait {
            wait_for_the_clock();
        }
        let dead = described(&router, case, &cluster, &first_id).await;
        let expected_state = if wait { "DELETED" } else { "DELETING" };
        assert_eq!(
            state_of(&dead),
            expected_state,
            "case {case:?}: expected the deleted group to report {expected_state}, got {dead}"
        );

        // The window is over: the same create carrying the same token mints a
        // group of its own, under the name the request asked for.
        let (status, retried) =
            create_with_token(&router, &cluster, token, retried_name, &nodes).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the retry to answer 201, got {status}: {retried}"
        );
        let retried_id = id_of(&retried);
        assert_ne!(
            retried_id, first_id,
            "case {case:?}: expected a freshly minted group, got the deleted {first_id:?}"
        );
        assert_eq!(
            retried["name"],
            json!(retried_name),
            "case {case:?}: expected the name the retry asked for, got {retried}"
        );
        assert!(
            !DEAD_STATES.contains(&state_of(&retried).as_str()),
            "case {case:?}: expected the retry to answer with a live group, got {retried}"
        );

        // The retry really STORED that group: the reader reports it, and the
        // deleted record stayed exactly as the clock had left it.
        let read = described(&router, case, &cluster, &retried_id).await;
        assert_eq!(
            without_state(&read),
            without_state(&retried),
            "case {case:?}: expected the reader to report the group the retry stored, got {read}"
        );
        assert!(
            !DEAD_STATES.contains(&state_of(&read).as_str()),
            "case {case:?}: expected the stored retry to be live, got {read}"
        );
        assert_eq!(
            described(&router, case, &cluster, &first_id).await,
            dead,
            "case {case:?}: expected the deleted group to stay as it was, got {:?}",
            described(&router, case, &cluster, &first_id).await
        );

        let mut expected_held = vec![first_id.clone(), retried_id.clone()];
        expected_held.sort();
        assert_eq!(
            held_ids(&router, case, &cluster).await,
            expected_held,
            "case {case:?}: expected the cluster to hold the deleted group and the retry's own, \
             got {:?}",
            held_ids(&router, case, &cluster).await
        );

        // The group the retry answered with is one the caller may CHANGE, which
        // is what the tombstone was not: the token now names the live group, so
        // one more retry replays that one rather than the deleted record.
        let (status, renamed) = update_group(
            &router,
            &cluster,
            &retried_id,
            json!({ "name": "renamed" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry's group to be updatable, got {status}: {renamed}"
        );
        let (status, again) = create_with_token(&router, &cluster, token, "renamed", &nodes).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the third create to answer 201, got {status}: {again}"
        );
        assert_eq!(
            id_of(&again),
            retried_id,
            "case {case:?}: expected the token to replay the live group {retried_id:?}, got {again}"
        );
        assert_eq!(
            held_ids(&router, case, &cluster).await,
            expected_held,
            "case {case:?}: expected the second replay to have stored nothing, got {:?}",
            held_ids(&router, case, &cluster).await
        );
    }
}

/// Rule: the token window a delete ends belongs to the deleted resource alone —
/// every other token of the same collection goes on replaying the group it
/// minted, and a token spent in one cluster says nothing about another's.
///
/// The rows differ along what the rule generalises over: whether the surviving
/// group is in the same cluster as the deleted one or in another, and which half
/// of the delete lifecycle the retries land in.
#[tokio::test]
async fn deleting_one_group_leaves_every_other_token_replaying() {
    // (case, whether the surviving group is in the deleted group's cluster,
    //  whether the retries wait for the clock)
    let rows: [(&str, bool, bool); 4] = [
        ("the same cluster, inside the DELETING window", true, false),
        ("the same cluster, once the delete settled", true, true),
        ("another cluster, inside the DELETING window", false, false),
        ("another cluster, once the delete settled", false, true),
    ];

    for (case, same_cluster, wait) in rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        let survivor_cluster = if same_cluster {
            clusters[0].clone()
        } else {
            clusters[1].clone()
        };

        let doomed = created(
            &router,
            case,
            &clusters[0],
            json!({ "name": "doomed", "requestId": "token-a" }),
        )
        .await;
        let survivor = created(
            &router,
            case,
            &survivor_cluster,
            json!({ "name": "survivor", "requestId": "token-b" }),
        )
        .await;
        let doomed_id = id_of(&doomed);
        let survivor_id = id_of(&survivor);

        delete_group(&router, case, &clusters[0], &doomed_id).await;
        if wait {
            wait_for_the_clock();
        }

        // The survivor's own token still replays its group, member for member.
        let (status, replayed) = create_with_token(
            &router,
            &survivor_cluster,
            "token-b",
            "another-name",
            &[],
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the survivor's replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            id_of(&replayed),
            survivor_id,
            "case {case:?}: expected the surviving token to replay {survivor_id:?}, got {replayed}"
        );
        assert_eq!(
            without_state(&replayed),
            without_state(&survivor),
            "case {case:?}: expected the replay to report the stored group, got {replayed}"
        );

        // And the deleted group's token, presented to the OTHER cluster, replays
        // nothing there either: it mints that cluster's own group.
        let (status, elsewhere) =
            create_with_token(&router, &clusters[1], "token-a", "elsewhere", &[]).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the other cluster's create to answer 201, got {status}: {elsewhere}"
        );
        assert_ne!(
            id_of(&elsewhere),
            doomed_id,
            "case {case:?}: expected a group of the cluster the request named, got the deleted one"
        );
        assert_eq!(
            elsewhere["clusterId"],
            json!(clusters[1]),
            "case {case:?}: expected the new group to belong to the cluster the path named, got {elsewhere}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The `AnywhereSettings` placement a channel write stores, end to end over the
//! restJson1 wire.
//!
//! Rule: every id a channel configuration names has to resolve to a resource this
//! account holds, so an operation that writes a channel's configuration
//! (`CreateChannel`, `UpdateChannel`) refuses one whose `AnywhereSettings` names
//! a cluster this account does not hold, or a channel placement group that
//! cluster does not hold — the same way it already refuses an `InputAttachments`
//! entry naming no input and a `ChannelSecurityGroups` entry naming no input
//! security group of this account: the modelled `UnprocessableEntityException`
//! (422), one `ValidationErrors` entry per problem, naming the element path it
//! was found at, and nothing written.
//!
//! `AnywhereSettings` carries "The ID of the cluster for the channel" and "The ID
//! of the channel placement group for the channel" (`smithy_query(query="shape",
//! shape="AnywhereSettings")`), and a group is named by the two ids TOGETHER —
//! `DescribeChannelPlacementGroup` reads it through
//! `/prod/clusters/{ClusterId}/channelplacementgroups/{ChannelPlacementGroupId}`
//! — so the group half is resolved inside the cluster the same settings name.
//!
//! The cluster and the group derive the mirror of this reference on every read
//! (`Cluster$ChannelIds`, `ChannelPlacementGroup$Channels`), so an accepted write
//! is checked from BOTH sides here: the channel reports the placement it named,
//! and the named cluster and group report that channel.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: an id nothing minted is ONE row, and the other rows are
//! what the rule generalises over — which half of the placement is unresolvable,
//! what makes it unresolvable (never minted, minted and then deleted, held by
//! ANOTHER cluster, named blank, not named at all), how many problems one request
//! carries, which other member carries them, and whether the request asked to be
//! performed or only checked.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    blank_reference_id_message, unknown_reference_message, CHANNEL_CONTAINS_ERRORS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{create_cluster, created, group_uri, id_of};
use crate::support::{attachable_input, channel_encoder_settings};
use crate::wire::{call, error_code, error_message, medialive};

/// An id of the shape this service mints for a cluster (`FIRST_CLUSTER_ID` and
/// up), that no `CreateCluster` ever minted in the account under test.
const UNMINTED_CLUSTER_ID: &str = "8009999";

/// An id of the shape this service mints for a channel placement group
/// (`FIRST_CHANNEL_PLACEMENT_GROUP_ID` and up), that no
/// `CreateChannelPlacementGroup` ever minted in the account under test.
const UNMINTED_GROUP_ID: &str = "9009999";

/// An id of the shape this service mints for an input, that no `CreateInput`
/// ever minted in the account under test — for the row that carries a problem in
/// another member of the same configuration.
const UNMINTED_INPUT_ID: &str = "9999999";

/// One problem a refusal reports: the element path it was found at, and the
/// message that says what it is.
type Problem = (String, String);

/// The cluster collection URI (`smithy_query(query="operation",
/// operation="CreateCluster")`: `POST /prod/clusters`).
const CLUSTERS: &str = "/prod/clusters";

/// `CreateChannel` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/prod/channels", Some(body)).await
}

/// `UpdateChannel` on `id` with `body`.
async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("/prod/channels/{id}"), Some(body)).await
}

/// `DescribeChannel` on `id`, with the read itself asserted to have succeeded.
async fn describe(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel({id}): got {status}: {body}"
    );
    body
}

/// The channels `DescribeCluster` derives for `cluster`.
async fn channels_of_cluster(router: &MultiServiceRouter, case: &str, cluster: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("{CLUSTERS}/{cluster}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCluster({cluster}): got {status}: {body}"
    );
    body["channelIds"].clone()
}

/// The channels `DescribeChannelPlacementGroup` derives for the group named by
/// `(cluster, group)`.
async fn channels_of_group(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    group: &str,
) -> Value {
    let (status, body) = call(router, "GET", &group_uri(cluster, group), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannelPlacementGroup({cluster}, {group}): got {status}: {body}"
    );
    body["channels"].clone()
}

/// One group of `cluster`, created through `CreateChannelPlacementGroup`,
/// answering the id the store minted.
async fn group_in(router: &MultiServiceRouter, cluster: &str, name: &str) -> String {
    let group = created(router, "setup", cluster, json!({ "name": name })).await;
    id_of(&group)
}

/// One cluster this account held and deleted again — a reference that resolved
/// once and does not any more.
async fn deleted_cluster(router: &MultiServiceRouter) -> String {
    let id = create_cluster(router, "setup", "doomed-cluster").await;
    let (status, body) = call(router, "DELETE", &format!("{CLUSTERS}/{id}"), None).await;
    assert_eq!(
        status, 202,
        "setup: DeleteCluster({id}): got {status}: {body}"
    );
    id
}

/// One group this account held and deleted again, inside the live cluster `in` —
/// so the cluster half of the placement resolves and the group half does not.
async fn deleted_group(router: &MultiServiceRouter, cluster: &str) -> String {
    let id = group_in(router, cluster, "doomed-group").await;
    let (status, body) = call(router, "DELETE", &group_uri(cluster, &id), None).await;
    assert_eq!(
        status, 200,
        "setup: DeleteChannelPlacementGroup({cluster}, {id}): got {status}: {body}"
    );
    id
}

/// How many channels this account holds, as `ListChannels` reports them.
async fn channel_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(status, 200, "ListChannels: got {status}: {body}");
    body["channels"].as_array().map_or(0, Vec::len)
}

/// The `(elementPath, errorMessage)` pairs a refusal reported, in the order it
/// reported them.
fn reported_problems(body: &Value) -> Vec<Problem> {
    body["validationErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|problem| {
            (
                problem["elementPath"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                problem["errorMessage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Assert `body` is the modelled 422 refusal carrying exactly `expected`, one
/// entry per problem, in that order.
fn assert_unprocessable(case: &str, status: u16, body: &Value, expected: &[Problem]) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected status 422, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body}"
    );
    assert_eq!(
        error_message(body),
        CHANNEL_CONTAINS_ERRORS_MESSAGE,
        "case {case:?}: expected the message {CHANNEL_CONTAINS_ERRORS_MESSAGE:?}, got {:?}",
        error_message(body)
    );
    assert_eq!(
        reported_problems(body),
        expected,
        "case {case:?}: expected the problems {expected:?}, got {:?}",
        body["validationErrors"]
    );
}

/// The problem an `anywhereSettings` naming the unresolvable cluster `id`
/// carries.
fn unresolvable_cluster(id: &str) -> Problem {
    (
        "anywhereSettings/clusterId".to_string(),
        unknown_reference_message("clusterId", "cluster", id),
    )
}

/// The problem an `anywhereSettings` that names no cluster at all carries.
fn no_cluster() -> Problem {
    (
        "anywhereSettings/clusterId".to_string(),
        blank_reference_id_message("clusterId"),
    )
}

/// The problem an `anywhereSettings` naming the unresolvable group `id` carries.
fn unresolvable_group(id: &str) -> Problem {
    (
        "anywhereSettings/channelPlacementGroupId".to_string(),
        unknown_reference_message("channelPlacementGroupId", "channel placement group", id),
    )
}

/// The problem an `anywhereSettings` whose group half names nothing carries.
fn no_group() -> Problem {
    (
        "anywhereSettings/channelPlacementGroupId".to_string(),
        blank_reference_id_message("channelPlacementGroupId"),
    )
}

/// The problem an `inputAttachments` entry at `index` naming the unresolvable id
/// `id` carries — the reference member this rule already applied to, here to pin
/// that one request carrying both reports both.
fn unresolvable_attachment(index: usize, id: &str) -> Problem {
    (
        format!("inputAttachments/{index}/inputId"),
        unknown_reference_message("inputAttachments", "input", id),
    )
}

/// Rule: `CreateChannel` refuses a configuration whose `AnywhereSettings` names a
/// cluster this account does not hold, or a group that cluster does not hold, and
/// creates NO channel.
#[tokio::test]
async fn a_create_refuses_a_placement_it_cannot_resolve() {
    let router = medialive();
    let held = create_cluster(&router, "setup", "held-cluster").await;
    let held_group = group_in(&router, &held, "studio-a").await;
    let other = create_cluster(&router, "setup", "other-cluster").await;
    let other_group = group_in(&router, &other, "studio-b").await;
    let gone_cluster = deleted_cluster(&router).await;
    let gone_group = deleted_group(&router, &held).await;
    let attachable = attachable_input(&router, "attachable").await;

    // (case, the request body, the problems the refusal must report, in order)
    let table: Vec<(&str, Value, Vec<Problem>)> = vec![
        (
            "a cluster id nothing minted",
            json!({
                "name": "unminted-cluster",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": { "clusterId": UNMINTED_CLUSTER_ID },
            }),
            vec![unresolvable_cluster(UNMINTED_CLUSTER_ID)],
        ),
        (
            "a cluster this account held and deleted",
            json!({
                "name": "deleted-cluster",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": { "clusterId": gone_cluster.clone() },
            }),
            vec![unresolvable_cluster(&gone_cluster)],
        ),
        (
            "a blank cluster id",
            json!({
                "name": "blank-cluster",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": { "clusterId": "" },
            }),
            vec![no_cluster()],
        ),
        (
            // A group is reachable only through its cluster, so settings that
            // name a group and no cluster name a group through no cluster.
            "a group with no cluster beside it",
            json!({
                "name": "group-without-cluster",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": { "channelPlacementGroupId": held_group.clone() },
            }),
            vec![no_cluster()],
        ),
        (
            "a group id nothing minted, in a held cluster",
            json!({
                "name": "unminted-group",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": {
                    "clusterId": held.clone(),
                    "channelPlacementGroupId": UNMINTED_GROUP_ID,
                },
            }),
            vec![unresolvable_group(UNMINTED_GROUP_ID)],
        ),
        (
            // A real group, in the wrong cluster: a group is named by both ids
            // together, so it is no group of the cluster the settings name.
            "a group of another cluster of the same account",
            json!({
                "name": "group-elsewhere",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": {
                    "clusterId": held.clone(),
                    "channelPlacementGroupId": other_group.clone(),
                },
            }),
            vec![unresolvable_group(&other_group)],
        ),
        (
            "a group this account held and deleted",
            json!({
                "name": "deleted-group",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": {
                    "clusterId": held.clone(),
                    "channelPlacementGroupId": gone_group.clone(),
                },
            }),
            vec![unresolvable_group(&gone_group)],
        ),
        (
            "a blank group id, in a held cluster",
            json!({
                "name": "blank-group",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": {
                    "clusterId": held.clone(),
                    "channelPlacementGroupId": "",
                },
            }),
            vec![no_group()],
        ),
        (
            // The group half cannot be resolved at all while the cluster it
            // would be resolved inside does not resolve, so the refusal reports
            // the cluster alone: that is the id the caller has to change.
            "neither half resolves",
            json!({
                "name": "neither-half",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": {
                    "clusterId": UNMINTED_CLUSTER_ID,
                    "channelPlacementGroupId": UNMINTED_GROUP_ID,
                },
            }),
            vec![unresolvable_cluster(UNMINTED_CLUSTER_ID)],
        ),
        (
            // Two reference members of one configuration, so the refusal reports
            // every problem the configuration carries rather than the first one
            // it meets.
            "an unresolvable attachment and an unresolvable cluster",
            json!({
                "name": "both-members",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [
                    { "inputAttachmentName": "ghost", "inputId": UNMINTED_INPUT_ID },
                ],
                "anywhereSettings": { "clusterId": UNMINTED_CLUSTER_ID },
            }),
            vec![
                unresolvable_attachment(0, UNMINTED_INPUT_ID),
                unresolvable_cluster(UNMINTED_CLUSTER_ID),
            ],
        ),
        (
            // `DryRun` asks for the request to be CHECKED, so it earns the same
            // refusal as the request that asked to be performed.
            "a checked-only create",
            json!({
                "name": "checked-only",
                "dryRun": true,
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": {
                    "clusterId": held.clone(),
                    "channelPlacementGroupId": UNMINTED_GROUP_ID,
                },
            }),
            vec![unresolvable_group(UNMINTED_GROUP_ID)],
        ),
        (
            // The resolvable half of a refused request is not written either:
            // the whole configuration is refused, so the held cluster and group
            // go on reporting no channel (asserted after the table).
            "a held attachment beside an unresolvable group",
            json!({
                "name": "mixed",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [
                    { "inputAttachmentName": "held", "inputId": attachable.clone() },
                ],
                "anywhereSettings": {
                    "clusterId": held.clone(),
                    "channelPlacementGroupId": UNMINTED_GROUP_ID,
                },
            }),
            vec![unresolvable_group(UNMINTED_GROUP_ID)],
        ),
    ];

    for (case, body, expected) in table {
        let before = channel_count(&router).await;
        let (status, refused) = create(&router, body).await;
        assert_unprocessable(case, status, &refused, &expected);
        assert_eq!(
            refused.get("channel"),
            None,
            "case {case:?}: expected no channel in the refusal, got {refused}"
        );
        assert_eq!(
            channel_count(&router).await,
            before,
            "case {case:?}: expected the refused create to store nothing, got {} channels",
            channel_count(&router).await
        );
    }

    // Both sides of the reference agree: no refused create left a channel
    // claiming the held cluster or the held group.
    assert_eq!(
        channels_of_cluster(&router, "after the table", &held).await,
        json!([]),
        "expected the held cluster to report no channel after every refusal"
    );
    assert_eq!(
        channels_of_group(&router, "after the table", &held, &held_group).await,
        json!([]),
        "expected the held group to report no channel after every refusal"
    );
}

/// Rule (the other side of the same rule): a placement whose every id resolves is
/// stored, reads back through `DescribeChannel`, and the named cluster and group
/// report the channel in `Cluster$ChannelIds` and
/// `ChannelPlacementGroup$Channels`.
///
/// The rows differ along which halves the settings name, which of the account's
/// clusters and groups they name, and whether the settings are there at all —
/// what the rule generalises over.
#[tokio::test]
async fn a_create_accepts_the_placement_it_can_resolve() {
    // (case, which cluster the settings name, which group of it they name)
    let table: [(&str, Option<usize>, Option<usize>); 6] = [
        ("no anywhere settings at all", None, None),
        ("the first cluster, no group", Some(0), None),
        ("the first cluster and its first group", Some(0), Some(0)),
        ("the first cluster and its second group", Some(0), Some(1)),
        ("the second cluster, no group", Some(1), None),
        ("the second cluster and its own group", Some(1), Some(0)),
    ];

    for (case, cluster_slot, group_slot) in table {
        // Each row starts from its own store, so the derivations below are about
        // this row's channel alone. Two clusters, each with two groups: a row's
        // placement is one of them, and every other one must report no channel.
        let router = medialive();
        let mut clusters = Vec::new();
        for index in 0..2 {
            let cluster = create_cluster(&router, case, &format!("cluster-{index}")).await;
            let mut groups = Vec::new();
            for slot in 0..2 {
                groups.push(group_in(&router, &cluster, &format!("group-{index}-{slot}")).await);
            }
            clusters.push((cluster, groups));
        }

        let mut settings = serde_json::Map::new();
        if let Some(slot) = cluster_slot {
            settings.insert("clusterId".to_string(), json!(clusters[slot].0.clone()));
        }
        if let (Some(cluster), Some(group)) = (cluster_slot, group_slot) {
            settings.insert(
                "channelPlacementGroupId".to_string(),
                json!(clusters[cluster].1[group].clone()),
            );
        }
        let mut body = json!({
            "name": case,
            "encoderSettings": channel_encoder_settings(),
        });
        let expected_settings = if settings.is_empty() {
            None
        } else {
            let value = Value::Object(settings);
            body["anywhereSettings"] = value.clone();
            Some(value)
        };

        let (status, created_channel) = create(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created_channel}"
        );
        let id = created_channel["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The response and the read both report the placement that was stored.
        assert_eq!(
            created_channel["channel"].get("anywhereSettings").cloned(),
            expected_settings,
            "case {case:?}: expected the create to report {expected_settings:?}, got {:?}",
            created_channel["channel"]["anywhereSettings"]
        );
        let read = describe(&router, case, &id).await;
        assert_eq!(
            read.get("anywhereSettings").cloned(),
            expected_settings,
            "case {case:?}: expected the read to report {expected_settings:?}, got {:?}",
            read["anywhereSettings"]
        );

        // The mirror derivations: the cluster the channel names reports it, as
        // does the group it is placed in, and neither of the others does.
        for (index, (cluster, groups)) in clusters.iter().enumerate() {
            let expected = if cluster_slot == Some(index) {
                json!([id.clone()])
            } else {
                json!([])
            };
            assert_eq!(
                channels_of_cluster(&router, case, cluster).await,
                expected,
                "case {case:?}: expected cluster {cluster} to report {expected:?}"
            );
            for (slot, group) in groups.iter().enumerate() {
                let expected = if cluster_slot == Some(index) && group_slot == Some(slot) {
                    json!([id.clone()])
                } else {
                    json!([])
                };
                assert_eq!(
                    channels_of_group(&router, case, cluster, group).await,
                    expected,
                    "case {case:?}: expected group {group} of cluster {cluster} to report \
                     {expected:?}"
                );
            }
        }
    }
}

/// Rule (the boundary of the same rule): `AnywhereSettings` that name NO id at
/// all claim no resource, so there is nothing to resolve and the write is
/// accepted — the refusal is about an id that names nothing, not about the
/// member being present.
#[tokio::test]
async fn a_create_accepts_anywhere_settings_that_name_nothing() {
    let case = "anywhere settings naming neither half";
    let router = medialive();
    let cluster = create_cluster(&router, case, "held-cluster").await;
    let group = group_in(&router, &cluster, "studio-a").await;

    let (status, created_channel) = create(
        &router,
        json!({
            "name": "no-placement",
            "encoderSettings": channel_encoder_settings(),
            "anywhereSettings": {},
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201, got {status}: {created_channel}"
    );
    let id = created_channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let read = describe(&router, case, &id).await;
    assert_eq!(
        read["anywhereSettings"],
        json!({}),
        "case {case:?}: expected the settings to name nothing, got {:?}",
        read["anywhereSettings"]
    );

    // A channel that claims no cluster and no group is reported by neither, so
    // the two views still agree about this channel.
    assert_eq!(
        channels_of_cluster(&router, case, &cluster).await,
        json!([]),
        "case {case:?}: expected the held cluster to report no channel"
    );
    assert_eq!(
        channels_of_group(&router, case, &cluster, &group).await,
        json!([]),
        "case {case:?}: expected the held group to report no channel"
    );
}

/// Rule: `UpdateChannel` applies the same resolution to the configuration the
/// update WOULD leave on the channel — an update may no more leave a channel
/// claiming a placement this account does not hold than a create may create one —
/// and a refused update leaves the stored channel exactly as it was.
///
/// The rows differ along which half is unresolvable, what makes it unresolvable,
/// and whether the update asked to be performed, carried another member too, or
/// only asked to be checked.
#[tokio::test]
async fn an_update_refuses_a_placement_it_cannot_resolve() {
    let router = medialive();
    let held = create_cluster(&router, "setup", "held-cluster").await;
    let held_group = group_in(&router, &held, "studio-a").await;
    let other = create_cluster(&router, "setup", "other-cluster").await;
    let other_group = group_in(&router, &other, "studio-b").await;
    let gone_cluster = deleted_cluster(&router).await;
    let gone_group = deleted_group(&router, &held).await;

    // (case, the patch, the problems the refusal must report, in order)
    let table: Vec<(&str, Value, Vec<Problem>)> = vec![
        (
            "a cluster id nothing minted",
            json!({ "anywhereSettings": { "clusterId": UNMINTED_CLUSTER_ID } }),
            vec![unresolvable_cluster(UNMINTED_CLUSTER_ID)],
        ),
        (
            "a cluster this account held and deleted",
            json!({ "anywhereSettings": { "clusterId": gone_cluster.clone() } }),
            vec![unresolvable_cluster(&gone_cluster)],
        ),
        (
            "a blank cluster id",
            json!({ "anywhereSettings": { "clusterId": "" } }),
            vec![no_cluster()],
        ),
        (
            "a group with no cluster beside it",
            json!({ "anywhereSettings": { "channelPlacementGroupId": held_group.clone() } }),
            vec![no_cluster()],
        ),
        (
            "a group id nothing minted",
            json!({ "anywhereSettings": {
                "clusterId": held.clone(),
                "channelPlacementGroupId": UNMINTED_GROUP_ID,
            } }),
            vec![unresolvable_group(UNMINTED_GROUP_ID)],
        ),
        (
            "a group of another cluster of the same account",
            json!({ "anywhereSettings": {
                "clusterId": held.clone(),
                "channelPlacementGroupId": other_group.clone(),
            } }),
            vec![unresolvable_group(&other_group)],
        ),
        (
            "a group this account held and deleted",
            json!({ "anywhereSettings": {
                "clusterId": held.clone(),
                "channelPlacementGroupId": gone_group.clone(),
            } }),
            vec![unresolvable_group(&gone_group)],
        ),
        (
            "a checked-only update",
            json!({ "dryRun": true, "anywhereSettings": {
                "clusterId": held.clone(),
                "channelPlacementGroupId": UNMINTED_GROUP_ID,
            } }),
            vec![unresolvable_group(UNMINTED_GROUP_ID)],
        ),
        (
            "a renamed channel whose placement does not resolve",
            json!({ "name": "renamed", "anywhereSettings": {
                "clusterId": UNMINTED_CLUSTER_ID,
            } }),
            vec![unresolvable_cluster(UNMINTED_CLUSTER_ID)],
        ),
    ];

    for (case, patch, expected) in table {
        let placement = json!({
            "clusterId": held.clone(),
            "channelPlacementGroupId": held_group.clone(),
        });
        let (status, seeded) = create(
            &router,
            json!({
                "name": "baseline",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": placement.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: seeding the channel: got {status}: {seeded}"
        );
        let id = seeded["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // An update is accepted from a channel that has stopped, which is the
        // `IDLE` the create's transient state settles into.
        wait_for_the_clock();

        let (status, refused) = update(&router, &id, patch).await;
        assert_unprocessable(case, status, &refused, &expected);

        let read = describe(&router, case, &id).await;
        assert_eq!(
            read["anywhereSettings"], placement,
            "case {case:?}: expected the refused update to leave the placement alone, got {:?}",
            read["anywhereSettings"]
        );
        assert_eq!(
            read["name"],
            json!("baseline"),
            "case {case:?}: expected the refused update to have written nothing, got {:?}",
            read["name"]
        );

        // The channel is deleted again, so the next row's derivations are about
        // its own channel alone.
        wait_for_the_clock();
        let (status, deleted) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteChannel: got {status}: {deleted}"
        );
        wait_for_the_clock();
    }
}

/// Rule (the other side of the update): an update whose placement resolves is
/// applied and reads back, the cluster and group derivations follow it — and an
/// update that names no `AnywhereSettings` at all keeps the placement the channel
/// had, which is what makes the check one on the configuration the update WOULD
/// leave rather than on the request alone.
#[tokio::test]
async fn an_update_accepts_the_placement_it_can_resolve() {
    let router = medialive();
    let first = create_cluster(&router, "setup", "first-cluster").await;
    let first_group = group_in(&router, &first, "studio-a").await;
    let second = create_cluster(&router, "setup", "second-cluster").await;
    let second_group = group_in(&router, &second, "studio-b").await;

    let baseline = json!({
        "clusterId": first.clone(),
        "channelPlacementGroupId": first_group.clone(),
    });

    // (case, the patch, the placement the channel must report afterwards)
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "no anywhere settings member at all",
            json!({ "name": "renamed" }),
            baseline.clone(),
        ),
        (
            "the same cluster, no group",
            json!({ "anywhereSettings": { "clusterId": first.clone() } }),
            json!({ "clusterId": first.clone() }),
        ),
        (
            "another cluster and a group of it",
            json!({ "anywhereSettings": {
                "clusterId": second.clone(),
                "channelPlacementGroupId": second_group.clone(),
            } }),
            json!({
                "clusterId": second.clone(),
                "channelPlacementGroupId": second_group.clone(),
            }),
        ),
        (
            "the placement it already had",
            json!({ "anywhereSettings": baseline.clone() }),
            baseline.clone(),
        ),
    ];

    for (case, patch, expected) in table {
        let (status, seeded) = create(
            &router,
            json!({
                "name": "baseline",
                "encoderSettings": channel_encoder_settings(),
                "anywhereSettings": baseline.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: seeding the channel: got {status}: {seeded}"
        );
        let id = seeded["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        wait_for_the_clock();

        let (status, updated) = update(&router, &id, patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {updated}"
        );

        let read = describe(&router, case, &id).await;
        assert_eq!(
            read["anywhereSettings"], expected,
            "case {case:?}: expected the placement {expected:?}, got {:?}",
            read["anywhereSettings"]
        );

        // Both derivations agree with what the update stored: the cluster the
        // channel now names reports it, the one it left does not, and the same
        // for the groups.
        for (cluster, group) in [(&first, &first_group), (&second, &second_group)] {
            let names_cluster = expected["clusterId"] == json!(cluster);
            let reported = channels_of_cluster(&router, case, cluster).await;
            assert_eq!(
                reported == json!([id.clone()]),
                names_cluster,
                "case {case:?}: expected cluster {cluster} to report the channel: \
                 {names_cluster}, got {reported:?}"
            );
            let names_group = names_cluster && expected["channelPlacementGroupId"] == json!(group);
            let reported = channels_of_group(&router, case, cluster, group).await;
            assert_eq!(
                reported == json!([id.clone()]),
                names_group,
                "case {case:?}: expected group {group} to report the channel: {names_group}, \
                 got {reported:?}"
            );
        }

        // Each row starts from a store holding no channel, so the derivations
        // above are about this row's channel alone. A channel is deleted once it
        // has stopped, which is the `IDLE` the update's transient `UPDATING`
        // settles back into.
        wait_for_the_clock();
        let (status, deleted) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteChannel: got {status}: {deleted}"
        );
        wait_for_the_clock();
    }
}

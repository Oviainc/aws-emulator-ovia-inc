// SPDX-License-Identifier: CC0-1.0

//! What `DescribeCluster` reports, end to end over the restJson1 wire.
//!
//! The rule these suites assert: a cluster-shaped response is the STORED cluster
//! read back — the members the create supplied verbatim, the minted `Id` and
//! `Arn`, the `State` the clock owns, and `ChannelIds` derived from the channels
//! whose `AnywhereSettings$ClusterId` names the cluster on every read — reading
//! it is a pure report that changes nothing a later caller sees, and a
//! `ClusterId` that names no stored cluster is refused with the modelled
//! `NotFoundException` (HTTP 404).
//!
//! The suites are TABLES. `cluster_alerts` covers the single-cluster create /
//! describe round trip (one fresh store per row); these rows are the cases that
//! one does not reach: SEVERAL clusters in ONE store read in several orders and
//! more than once, a `ChannelIds` that has to track a channel LEAVING the
//! cluster, and the ids the store holds no cluster for. The network side of the
//! attachment (a network's `AssociatedClusterIds`) has its own table in
//! `networks.rs`.
//!
//! The cluster, channel and id helpers are the ones the sibling suites already
//! own ([`crate::channel_placement_groups`]), so this suite spells no second way
//! of creating a cluster or a channel.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{create_channel, create_cluster as create_named_cluster, id_of};
use crate::support::member;
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT, DEFAULT_REGION};

/// The cluster collection URI (`smithy_query(query="operation",
/// operation="CreateCluster,DescribeCluster")`: POST /prod/clusters, GET
/// /prod/clusters/{ClusterId}).
const CLUSTERS: &str = "/prod/clusters";

/// The `ClusterType` the model declares (`smithy_query(query="shape",
/// shape="ClusterType")`: ON_PREMISES — the one value, so every row carries it).
const ON_PREMISES: &str = "ON_PREMISES";

/// The channel placement group id the placed channels name. A channel's
/// `AnywhereSettings` carries both labels; only `ClusterId` is what `ChannelIds`
/// is derived from, so the group half is the same for every placed channel here.
const PLACEMENT_GROUP: &str = "pg-1";

/// `CreateCluster` with an arbitrary body, answering the created cluster.
///
/// The sibling helper ([`create_named_cluster`]) sends a name and nothing else,
/// which is what every other suite needs; the first table below varies the
/// OPTIONAL members, so it needs the body itself.
async fn create_cluster(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, created) = call(router, "POST", CLUSTERS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCluster to answer 201, got {status}: {created}"
    );
    created
}

/// `DescribeCluster` on `id`, as a client calls it.
async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{CLUSTERS}/{id}"), None).await
}

/// `DescribeCluster` on `id`, answering the reported cluster with the modelled
/// 200 asserted.
async fn describe_cluster(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = describe(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeCluster to answer 200, got {status}: {described}"
    );
    described
}

/// The `ChannelIds` a response reports, as a SET: an absent member and an empty
/// list both mean "no channel runs on this cluster", and the member is an
/// association rather than a sequence, so the reading does not depend on the
/// order the store happens to iterate its channels in.
fn reported_channel_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = match body.get("channelIds") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect(),
        Some(other) => panic!("expected channelIds as a list, got {other}"),
    };
    ids.sort();
    ids
}

/// The same reading of what a row expects, so both sides are compared as sets.
fn expected_channel_ids(ids: &[String]) -> Vec<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted
}

/// One row of the stored-cluster table: the `CreateCluster` body it sends.
struct Stored {
    case: &'static str,
    body: Value,
}

/// Rule: every member `DescribeCluster` reports is the stored cluster read back,
/// and the read is a pure report — several clusters live in one store, each
/// describe resolves the `ClusterId` it was given, and describing one (or the
/// same one again, in any order) changes nothing a later caller sees.
///
/// The rows differ along WHICH optional members the create supplies and along
/// the SIZE of the interface-mapping list (none, empty, one, three), because
/// that is what the rule generalises over; all of them share ONE store, which is
/// what makes the resolution and the read-purity halves assertable.
#[tokio::test]
async fn describe_cluster_reports_each_cluster_of_one_store() {
    let table = [
        Stored {
            case: "fully specified",
            body: json!({
                "name": "cluster-full",
                "clusterType": ON_PREMISES,
                "instanceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaLiveAnywhere"),
                "networkSettings": {
                    "defaultRoute": "primary",
                    "interfaceMappings": [
                        { "logicalInterfaceName": "primary", "networkId": "8000000" }
                    ]
                },
                "tags": { "team": "video" }
            }),
        },
        Stored {
            case: "three interface mappings",
            body: json!({
                "name": "cluster-mapped",
                "clusterType": ON_PREMISES,
                "networkSettings": {
                    "defaultRoute": "primary",
                    "interfaceMappings": [
                        { "logicalInterfaceName": "primary", "networkId": "8000000" },
                        { "logicalInterfaceName": "backup", "networkId": "8000001" },
                        { "logicalInterfaceName": "management", "networkId": "8000002" }
                    ]
                }
            }),
        },
        Stored {
            case: "an empty mapping list",
            body: json!({
                "name": "cluster-empty-mappings",
                "clusterType": ON_PREMISES,
                "networkSettings": { "interfaceMappings": [] }
            }),
        },
        Stored {
            case: "no network settings and no name",
            body: json!({ "clusterType": ON_PREMISES }),
        },
    ];

    let router = medialive();

    // Every row in ONE store, so a later describe has to resolve the id it was
    // given rather than answer with "the cluster".
    let mut created: Vec<(&str, Value, Value)> = Vec::new();
    for Stored { case, body } in &table {
        let response = create_cluster(&router, case, body.clone()).await;
        created.push((*case, body.clone(), response));
    }

    let ids: Vec<String> = created
        .iter()
        .map(|(_, _, response)| id_of(response))
        .collect();
    for (position, id) in ids.iter().enumerate() {
        assert!(
            !id.is_empty(),
            "case {:?}: expected CreateCluster to report a minted id, got {:?}",
            created[position].0,
            created[position].2
        );
        assert!(
            !ids[..position].contains(id),
            "case {:?}: expected a freshly minted id, got {id:?} which another cluster already has",
            created[position].0
        );
    }

    // The creation has come due, so `State` is the terminal `ACTIVE` for the
    // rest of this test and a repeated read has one answer to be stable on.
    wait_for_the_clock();

    // Round 0 reads the rows in order, round 1 in reverse, round 2 in order
    // again: a read of one cluster must not change what a read of another, or a
    // second read of the same one, reports.
    let mut first_round: Vec<Value> = Vec::new();
    for round in 0..3 {
        let order: Vec<usize> = if round == 1 {
            (0..created.len()).rev().collect()
        } else {
            (0..created.len()).collect()
        };

        for position in order {
            let (case, request, create_response) = &created[position];
            let id = &ids[position];
            let described = describe_cluster(&router, case, id).await;
            let case = *case;

            assert_eq!(
                described["id"],
                json!(id),
                "case {case:?} (round {round}): expected the described cluster to be the one the id names, got {described}"
            );
            assert_eq!(
                described["arn"],
                json!(format!(
                    "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:cluster:{id}"
                )),
                "case {case:?} (round {round}): expected the ARN built from the signed region and the minted id, got {:?}",
                described["arn"]
            );
            assert_eq!(
                described["arn"], create_response["arn"],
                "case {case:?} (round {round}): expected the ARN the create minted, got {:?}",
                described["arn"]
            );
            for stored in ["name", "clusterType", "instanceRoleArn", "networkSettings"] {
                assert_eq!(
                    member(&described, stored),
                    member(request, stored),
                    "case {case:?} (round {round}): expected the {stored} the request supplied, got {described}"
                );
            }
            assert_eq!(
                reported_channel_ids(&described),
                Vec::<String>::new(),
                "case {case:?} (round {round}): no channel runs on this cluster, got {:?}",
                described["channelIds"]
            );
            assert_eq!(
                described["state"],
                json!("ACTIVE"),
                "case {case:?} (round {round}): expected the settled state, got {:?}",
                described["state"]
            );

            if round == 0 {
                first_round.push(described);
            } else {
                assert_eq!(
                    described, first_round[position],
                    "case {case:?} (round {round}): expected the read to report exactly what the first read did"
                );
            }
        }
    }
}

/// Rule: a `ClusterId` the store holds no cluster for is the operation's
/// modelled `NotFoundException` (HTTP 404), whatever shape the id has — the read
/// resolves the id against state rather than guessing from its form.
///
/// The rows differ along the SHAPE of the id that misses (a never-minted id of
/// the same numeric form, one of a different width, a non-numeric label, the
/// cluster's NAME rather than its id), and each miss is checked with one real
/// cluster in the store, so the refusal is a resolution failure and not an empty
/// store.
#[tokio::test]
async fn an_id_no_cluster_has_is_refused() {
    let router = medialive();
    let stored = create_named_cluster(&router, "the one stored cluster", "cluster-present").await;

    let misses: [(&str, String); 5] = [
        ("a never-minted id of the same form", "8000999".to_string()),
        ("a shorter numeric id", "42".to_string()),
        ("a longer numeric id", "900000000000".to_string()),
        ("a non-numeric label", "not-a-cluster".to_string()),
        ("the cluster's name, not its id", "cluster-present".to_string()),
    ];

    for (case, id) in &misses {
        assert_ne!(
            id, &stored,
            "case {case:?}: the row must name an id the store does NOT hold, got the stored {stored:?}"
        );
        let (status, body) = describe(&router, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {id:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for {id:?}, got {body}"
        );
        assert!(
            error_message(&body).contains(id.as_str()),
            "case {case:?}: expected the refusal to name the id asked for, got {:?}",
            error_message(&body)
        );
    }

    // None of those refusals disturbed the cluster the store does hold.
    let described = describe_cluster(&router, "the stored cluster after the misses", &stored).await;
    assert_eq!(
        described["id"],
        json!(stored),
        "expected the stored cluster to still read back, got {described}"
    );
}

/// One row of the channel table: the cluster it names, if any.
struct Placed {
    case: &'static str,
    name: &'static str,
    /// Which cluster of the two this channel's `AnywhereSettings` names, as an
    /// index, or `None` for a channel that names none at all.
    cluster: Option<usize>,
}

/// Rule: `ChannelIds` is derived from the channel store on every read, so it
/// reports exactly the channels that currently name the cluster — it grows as
/// channels are created and shrinks as one leaves.
///
/// The rows span the association: two channels on one cluster, one on another,
/// and one on no cluster at all (so the member reports an association, not a
/// channel count), and the read is repeated after a channel has been deleted,
/// which is the case the create/describe round trip cannot reach.
#[tokio::test]
async fn cluster_channel_ids_track_the_channel_store() {
    let table: [Placed; 4] = [
        Placed { case: "first on cluster 0", name: "channel-a", cluster: Some(0) },
        Placed { case: "only channel of cluster 1", name: "channel-b", cluster: Some(1) },
        Placed { case: "second on cluster 0", name: "channel-c", cluster: Some(0) },
        Placed { case: "on no cluster", name: "channel-d", cluster: None },
    ];

    let router = medialive();
    let mut clusters: Vec<String> = Vec::new();
    for index in 0..2 {
        let case = format!("cluster {index}");
        clusters.push(create_named_cluster(&router, &case, &format!("cluster-{index}")).await);
    }

    // Before any channel exists, both clusters report the empty list: zero
    // channels is an empty association, not an unknown one.
    for (index, cluster) in clusters.iter().enumerate() {
        let described = describe_cluster(&router, &format!("cluster {index}"), cluster).await;
        assert_eq!(
            reported_channel_ids(&described),
            Vec::<String>::new(),
            "cluster {index}: expected no channels before any was created, got {:?}",
            described["channelIds"]
        );
    }

    // Each channel is created in turn, and after every one both clusters report
    // exactly the channels that name them.
    let mut expected: Vec<Vec<String>> = vec![Vec::new(), Vec::new()];
    for Placed { case, name, cluster } in &table {
        let case = *case;
        let placement = cluster.map(|index| (clusters[index].as_str(), PLACEMENT_GROUP));
        let channel_id = create_channel(&router, case, name, placement).await;
        if let Some(index) = cluster {
            expected[*index].push(channel_id);
        }

        for (index, stored) in clusters.iter().enumerate() {
            let described = describe_cluster(&router, case, stored).await;
            assert_eq!(
                reported_channel_ids(&described),
                expected_channel_ids(&expected[index]),
                "case {case:?}: expected cluster {index} to report {:?}, got {:?}",
                expected[index],
                described["channelIds"]
            );
        }
    }

    // A channel that has been deleted runs on nothing, so it leaves the
    // association: a channel is deletable once its create has settled into
    // IDLE, and the delete itself settles into the terminal DELETED.
    wait_for_the_clock();
    let doomed = expected[0]
        .first()
        .cloned()
        .expect("the table places at least one channel on cluster 0");
    let (status, deleting) = call(&router, "DELETE", &format!("/prod/channels/{doomed}"), None).await;
    assert_eq!(
        status, 200,
        "expected DeleteChannel to answer 200, got {status}: {deleting}"
    );
    wait_for_the_clock();
    expected[0].remove(0);

    for (index, stored) in clusters.iter().enumerate() {
        let case = format!("after deleting channel {doomed}");
        let described = describe_cluster(&router, &case, stored).await;
        assert_eq!(
            reported_channel_ids(&described),
            expected_channel_ids(&expected[index]),
            "case {case:?}: expected cluster {index} to report {:?}, got {:?}",
            expected[index],
            described["channelIds"]
        );
    }
}

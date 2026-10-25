// SPDX-License-Identifier: CC0-1.0

//! `UpdateCluster`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The clusters the rows change are created through `CreateCluster`, so every id
//! here is an id the store really minted.
//!
//! The rules these suites assert, each over a table that spans its class:
//!
//! 1. An update writes exactly the members the request CARRIES and leaves every
//!    other stored member — "Include this parameter only if you want to change
//!    the current ..." on `Name`, on `NetworkSettings$DefaultRoute` and on
//!    `NetworkSettings$InterfaceMappings` — and what it answers is what a later
//!    `DescribeCluster` reports back out of the store.
//! 2. A request the service cannot apply is refused with the modelled error for
//!    its class and NOTHING is written: a `ClusterId` the store holds no cluster
//!    for is the modelled 400 `BadRequestException` (this operation models no
//!    404), a `Name` another cluster of the account holds is 409
//!    `ConflictException`, and a `Name` or an `InterfaceMappings` list outside
//!    the lengths the store keeps is 400 `BadRequestException`.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_CLUSTER_INTERFACE_MAPPINGS, MAX_RESOURCE_NAME_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// The cluster collection URI `CreateCluster` is bound to
/// (`smithy_query(query="operation",operation="CreateCluster")`: POST
/// /prod/clusters).
const CLUSTERS: &str = "/prod/clusters";

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape",shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// The `ClusterState` a cluster has reached once the clock has made its creation
/// due (`smithy_query(query="shape",shape="ClusterState")`: ACTIVE among them).
const ACTIVE: &str = "ACTIVE";

/// `CreateCluster` with `body`, asserted to have been accepted, answering the
/// created cluster.
async fn create(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, cluster) = call(router, "POST", CLUSTERS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateCluster: expected 201, got {status}: {cluster}"
    );
    cluster
}

/// One `UpdateCluster` call
/// (`smithy_query(query="operation",operation="UpdateCluster")`: `PUT
/// /prod/clusters/{ClusterId}`).
async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("{CLUSTERS}/{id}"), Some(body)).await
}

/// The cluster an update reports, with the update itself asserted to have
/// succeeded. `200 OK` is the status the model declares for it.
async fn updated(router: &MultiServiceRouter, case: &str, id: &str, body: Value) -> Value {
    let (status, cluster) = update(router, id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateCluster: expected 200, got {status}: {cluster}"
    );
    cluster
}

/// The cluster `DescribeCluster` reports, asserted to be readable.
async fn described(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, cluster) = call(router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCluster: expected 200, got {status}: {cluster}"
    );
    cluster
}

/// The `Id` a cluster response reports, which the store minted.
fn id_of(case: &str, cluster: &Value) -> String {
    cluster["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted cluster id, got {cluster}"))
        .to_string()
}

/// `count` `InterfaceMapping` entries, each naming its own logical interface and
/// network, for the rows that vary the length of the list.
fn mappings(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "logicalInterfaceName": format!("if-{index}"),
                    "networkId": format!("net-{index}"),
                })
            })
            .collect(),
    )
}

/// The IAM role every row stores, so the member `UpdateClusterResponse` does not
/// declare can be shown to survive an update.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/MediaLiveAnywhere")
}

/// One row of the update table: the case, the `networkSettings` the cluster is
/// created with, the update's request body, and the `name` and `networkSettings`
/// expected afterwards.
type UpdateRow = (&'static str, Value, Value, Value, Value);

/// Rule: an update writes exactly the members the request carries and leaves
/// every other stored member — including the ones no update request can name at
/// all — and what the update answers is what a later describe reports.
///
/// The rows differ along WHICH members the request carries (the name alone,
/// one network member alone, both, all of them, none) and along the length of the
/// mapping list the update stores (kept, grown, emptied), because that is what
/// the rule generalises over; the members themselves are the model's own
/// (`smithy_query(query="shape",shape="ClusterNetworkSettingsUpdateRequest")`).
#[tokio::test]
async fn an_update_writes_the_members_it_carries() {
    let one_mapping = json!([{ "logicalInterfaceName": "primary", "networkId": "net-1" }]);
    let rows: [UpdateRow; 8] = [
        (
            "a rename, network settings omitted",
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
            json!({ "name": "renamed" }),
            json!("renamed"),
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
        ),
        (
            "the default route alone",
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
            json!({ "networkSettings": { "defaultRoute": "net-2" } }),
            json!("under-test"),
            json!({ "defaultRoute": "net-2", "interfaceMappings": one_mapping }),
        ),
        (
            "a longer mapping list",
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
            json!({ "networkSettings": { "interfaceMappings": mappings(3) } }),
            json!("under-test"),
            json!({ "defaultRoute": "net-1", "interfaceMappings": mappings(3) }),
        ),
        (
            "the mapping list emptied",
            json!({ "defaultRoute": "net-1", "interfaceMappings": mappings(2) }),
            json!({ "networkSettings": { "interfaceMappings": [] } }),
            json!("under-test"),
            json!({ "defaultRoute": "net-1", "interfaceMappings": [] }),
        ),
        (
            "both network members at once",
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
            json!({
                "networkSettings": {
                    "defaultRoute": "net-7",
                    "interfaceMappings": mappings(2),
                }
            }),
            json!("under-test"),
            json!({ "defaultRoute": "net-7", "interfaceMappings": mappings(2) }),
        ),
        (
            "a name and the network settings at once",
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
            json!({
                "name": "renamed-and-rewired",
                "networkSettings": { "defaultRoute": "net-3", "interfaceMappings": mappings(1) },
            }),
            json!("renamed-and-rewired"),
            json!({ "defaultRoute": "net-3", "interfaceMappings": mappings(1) }),
        ),
        (
            "neither member",
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
            json!({}),
            json!("under-test"),
            json!({ "defaultRoute": "net-1", "interfaceMappings": one_mapping }),
        ),
        (
            // A cluster created without network settings GAINS the members the
            // update names, rather than keeping none: the request asked to be
            // connected by them.
            "network settings where there were none",
            Value::Null,
            json!({ "networkSettings": { "defaultRoute": "net-9" } }),
            json!("under-test"),
            json!({ "defaultRoute": "net-9" }),
        ),
    ];

    for (case, created_settings, request, name, settings) in rows {
        let router = medialive();
        let mut create_body = json!({
            "name": "under-test",
            "clusterType": ON_PREMISES,
            "instanceRoleArn": role_arn(),
        });
        if !created_settings.is_null() {
            create_body["networkSettings"] = created_settings;
        }
        let cluster = create(&router, case, create_body).await;
        let id = id_of(case, &cluster);
        // Start from a settled cluster, so the state the update reports is the
        // one the store had reached rather than one left over from the create.
        wait_for_the_clock();

        let after = updated(&router, case, &id, request).await;

        assert_eq!(
            after["name"], name,
            "case {case:?}: expected name {name}, got {:?}",
            after["name"]
        );
        assert_eq!(
            after["networkSettings"], settings,
            "case {case:?}: expected networkSettings {settings}, got {:?}",
            after["networkSettings"]
        );
        // An update neither mints a new cluster nor re-mints its ARN, and neither
        // the hardware type nor the derived channel list is something it can
        // move.
        assert_eq!(
            after["id"],
            json!(id),
            "case {case:?}: expected the same cluster, got {:?}",
            after["id"]
        );
        assert_eq!(
            after["arn"], cluster["arn"],
            "case {case:?}: expected the ARN to be untouched, got {:?}",
            after["arn"]
        );
        assert_eq!(
            after["clusterType"],
            json!(ON_PREMISES),
            "case {case:?}: expected the hardware type to be untouched, got {:?}",
            after["clusterType"]
        );
        assert_eq!(
            after["channelIds"],
            json!([]),
            "case {case:?}: expected no channel to be associated, got {:?}",
            after["channelIds"]
        );
        assert_eq!(
            after["state"],
            json!(ACTIVE),
            "case {case:?}: expected the settled state, got {:?}",
            after["state"]
        );
        // `UpdateClusterResponse` declares no `InstanceRoleArn` member, so the
        // update does not report one.
        assert!(
            after.get("instanceRoleArn").is_none(),
            "case {case:?}: UpdateClusterResponse carries no instanceRoleArn, got {after}"
        );

        // The round trip: what the update answered is what the store holds, and
        // the member the update's own shape does not carry is still there.
        let read = described(&router, case, &id).await;
        for member in [
            "id",
            "arn",
            "name",
            "clusterType",
            "networkSettings",
            "channelIds",
        ] {
            assert_eq!(
                read.get(member),
                after.get(member),
                "case {case:?}: expected the describe to report the {member} the update wrote, \
                 got {:?} after {:?}",
                read.get(member),
                after.get(member)
            );
        }
        assert_eq!(
            read["instanceRoleArn"],
            json!(role_arn()),
            "case {case:?}: expected the stored role to survive the update, got {:?}",
            read["instanceRoleArn"]
        );
    }
}

/// Rule: an update reaches only a cluster this store holds, so a `ClusterId` that
/// names none is refused — with the modelled 400 `BadRequestException`, because
/// this operation models no 404 — and nothing in the store changes.
///
/// The rows differ along the SHAPE of the id that misses (an id in no resource's
/// range, an id of another resource's id space, a non-numeric one), because that
/// is what the rule generalises over: resolution is by what the store holds, not
/// by what an id looks like.
#[tokio::test]
async fn an_update_only_reaches_a_cluster_the_store_holds() {
    let router = medialive();
    let case = "scoping";
    let cluster = create(
        &router,
        case,
        json!({
            "name": "the-only-cluster",
            "clusterType": ON_PREMISES,
            "instanceRoleArn": role_arn(),
            "networkSettings": { "defaultRoute": "net-1", "interfaceMappings": mappings(1) },
        }),
    )
    .await;
    let id = id_of(case, &cluster);
    wait_for_the_clock();
    let before = described(&router, case, &id).await;

    // A channel, so one of the rows can ask for a cluster under an id another
    // resource of this service really holds.
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "elsewhere" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {channel}"
    );
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted channel id, got {channel}"))
        .to_string();

    let misses: [(&str, &str); 4] = [
        ("an id no cluster has", "8999999"),
        ("a channel's id", &channel_id),
        ("a non-numeric id", "not-a-cluster"),
        ("the cluster's id with a digit appended", "89999990"),
    ];

    for (case, missing) in misses {
        let (status, body) = update(&router, missing, json!({ "name": "renamed" })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert!(
            error_message(&body).contains(missing),
            "case {case:?}: expected the refusal to name the id asked for, got {}",
            error_message(&body)
        );
    }

    // None of those refusals touched the cluster the store does hold.
    let after = described(&router, "after the refusals", &id).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the stored cluster to be unchanged, got {after}"
    );
}

/// Rule: an update holds a changed cluster to the same bounds a create holds a
/// stored one to — `Name` is unique in the AWS account and case-sensitive, and
/// both `Name` and `InterfaceMappings` are held to the lengths this emulator
/// stores — refusing with the modelled error for each class and writing nothing.
///
/// The name rows span the clashes around uniqueness (another cluster's name, that
/// name in another case, this cluster's own name, a name nobody holds), the
/// length rows span each published bound and one past it, and the mapping rows
/// span the list lengths around `MAX_CLUSTER_INTERFACE_MAPPINGS`, because that is
/// what the rule generalises over.
#[tokio::test]
async fn an_update_is_held_to_the_bounds_a_create_is() {
    // (case, the name the update asks for, accepted)
    let name_rows: [(&str, &str, bool); 4] = [
        ("another cluster's name", "taken", false),
        ("that name in another case", "Taken", true),
        ("the cluster's own name", "under-test", true),
        ("a name nobody holds", "free", true),
    ];

    for (case, name, accepted) in name_rows {
        let router = medialive();
        let holder = create(
            &router,
            case,
            json!({ "name": "taken", "clusterType": ON_PREMISES }),
        )
        .await;
        let cluster = create(
            &router,
            case,
            json!({ "name": "under-test", "clusterType": ON_PREMISES }),
        )
        .await;
        let id = id_of(case, &cluster);
        let holder_id = id_of(case, &holder);

        let (status, body) = update(&router, &id, json!({ "name": name })).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["name"],
                json!(name),
                "case {case:?}: expected the name the request asked for, got {:?}",
                body["name"]
            );
            // An accepted rename is PERSISTED, not just reported.
            let read = described(&router, case, &id).await;
            assert_eq!(
                read["name"],
                json!(name),
                "case {case:?}: expected the accepted name to be stored, got {:?}",
                read["name"]
            );
        } else {
            assert_eq!(
                status, 409,
                "case {case:?}: expected 409, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {body}"
            );
            let read = described(&router, case, &id).await;
            assert_eq!(
                read["name"],
                json!("under-test"),
                "case {case:?}: expected the refused update to leave the name alone, got {:?}",
                read["name"]
            );
        }

        // The cluster that holds the name is never written to either way.
        let other = described(&router, case, &holder_id).await;
        assert_eq!(
            other["name"],
            json!("taken"),
            "case {case:?}: expected the cluster holding the name to keep it, got {:?}",
            other["name"]
        );
    }

    // (case, the length in characters the row gives `Name`, accepted)
    let length_rows: [(&str, usize, bool); 4] = [
        ("one character long", 1, true),
        ("at the bound", MAX_RESOURCE_NAME_LEN, true),
        ("one past the bound", MAX_RESOURCE_NAME_LEN + 1, false),
        ("below the minimum", 0, false),
    ];

    for (case, length, accepted) in length_rows {
        let router = medialive();
        let cluster = create(
            &router,
            case,
            json!({ "name": "under-test", "clusterType": ON_PREMISES }),
        )
        .await;
        let id = id_of(case, &cluster);
        let value = "c".repeat(length);

        let (status, body) = update(&router, &id, json!({ "name": value.clone() })).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {body}"
            );
            let read = described(&router, case, &id).await;
            assert_eq!(
                read["name"],
                json!(value),
                "case {case:?}: expected the stored name of {length} characters, got {:?}",
                read["name"]
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        // The message is built from the request, so its content IS the rule: it
        // names the member, the bound it broke and the length that broke it.
        let message = error_message(&body);
        for expected in [
            "name".to_string(),
            MAX_RESOURCE_NAME_LEN.to_string(),
            length.to_string(),
        ] {
            assert!(
                message.contains(&expected),
                "case {case:?}: expected the message to carry {expected:?}, got {message:?}"
            );
        }
        let read = described(&router, case, &id).await;
        assert_eq!(
            read["name"],
            json!("under-test"),
            "case {case:?}: expected the refused update to store nothing, got {:?}",
            read["name"]
        );
    }

    // (case, how many mappings the update asks to store, accepted)
    let mapping_rows: [(&str, usize, bool); 4] = [
        ("no mapping", 0, true),
        ("one mapping", 1, true),
        ("at the bound", MAX_CLUSTER_INTERFACE_MAPPINGS, true),
        (
            "one past the bound",
            MAX_CLUSTER_INTERFACE_MAPPINGS + 1,
            false,
        ),
    ];

    for (case, count, accepted) in mapping_rows {
        let router = medialive();
        let cluster = create(
            &router,
            case,
            json!({
                "name": "under-test",
                "clusterType": ON_PREMISES,
                "networkSettings": { "defaultRoute": "net-1", "interfaceMappings": mappings(1) },
            }),
        )
        .await;
        let id = id_of(case, &cluster);
        let before = described(&router, case, &id).await;
        let asked = mappings(count);

        let (status, body) = update(
            &router,
            &id,
            json!({ "networkSettings": { "interfaceMappings": asked.clone() } }),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {body}"
            );
            let read = described(&router, case, &id).await;
            assert_eq!(
                read["networkSettings"]["interfaceMappings"], asked,
                "case {case:?}: expected the accepted mappings to be stored, got {:?}",
                read["networkSettings"]["interfaceMappings"]
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let message = error_message(&body);
        for expected in [
            MAX_CLUSTER_INTERFACE_MAPPINGS.to_string(),
            count.to_string(),
        ] {
            assert!(
                message.contains(&expected),
                "case {case:?}: expected the message to carry {expected:?}, got {message:?}"
            );
        }
        let read = described(&router, case, &id).await;
        assert_eq!(
            read["networkSettings"], before["networkSettings"],
            "case {case:?}: expected the refused update to store nothing, got {:?}",
            read["networkSettings"]
        );
    }
}

/// Rule: an update is a WRITE to the cluster it names and to nothing else — a
/// sibling cluster is left exactly as it was, and the updated cluster's
/// `ChannelIds` stays derived from the channel store rather than being replaced
/// by the update.
#[tokio::test]
async fn an_update_touches_only_the_cluster_it_names() {
    let router = medialive();
    let case = "one cluster at a time";
    let target = create(
        &router,
        case,
        json!({
            "name": "target",
            "clusterType": ON_PREMISES,
            "networkSettings": { "defaultRoute": "net-1", "interfaceMappings": mappings(1) },
        }),
    )
    .await;
    let sibling = create(
        &router,
        case,
        json!({
            "name": "sibling",
            "clusterType": ON_PREMISES,
            "networkSettings": { "defaultRoute": "net-5", "interfaceMappings": mappings(2) },
        }),
    )
    .await;
    let id = id_of(case, &target);
    let sibling_id = id_of(case, &sibling);

    // A channel running in the target cluster, so `ChannelIds` is non-empty and
    // can be shown to survive the update.
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "placed",
            "anywhereSettings": { "clusterId": id, "channelPlacementGroupId": "7000000" },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {channel}"
    );
    let placed = channel["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted channel id, got {channel}"))
        .to_string();

    wait_for_the_clock();
    let before_sibling = described(&router, case, &sibling_id).await;

    let after = updated(
        &router,
        case,
        &id,
        json!({
            "name": "target-renamed",
            "networkSettings": { "defaultRoute": "net-2", "interfaceMappings": mappings(3) },
        }),
    )
    .await;
    assert_eq!(
        after["channelIds"],
        json!([placed]),
        "case {case:?}: expected the channel in the cluster to still be reported, got {:?}",
        after["channelIds"]
    );

    let read_sibling = described(&router, case, &sibling_id).await;
    assert_eq!(
        read_sibling, before_sibling,
        "case {case:?}: expected the sibling cluster to be untouched, got {read_sibling}"
    );
}

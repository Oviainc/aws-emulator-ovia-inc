// SPDX-License-Identifier: CC0-1.0

//! `DeleteNetwork` — the deletion half of the MediaLive Anywhere network's
//! lifecycle — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour, not an internal function's return value.
//!
//! The create and read side of the resource lives in `networks`, whose request
//! helpers these suites reuse so the two cannot disagree about how a network
//! request reaches the emulator.
//!
//! The suites are TABLES: the issue's own reproduction — one `DeleteNetwork` call
//! on a created network — is ONE row of the first of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::networks::{
    cluster_body, created, created_cluster, describe, id_of, list, list_member, summaries,
    ClusterSpec, NETWORKS, STRAY_NETWORK_ID,
};
use crate::wire::{call, error_code, error_message, medialive};

/// `DeleteNetwork` on `id`.
async fn delete(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("{NETWORKS}/{id}"), None).await
}

/// `DeleteNetwork` on `id`, answering the network it reports, with the call
/// itself asserted to have answered the modelled 202 and to report the network in
/// `state`.
async fn accepted(router: &MultiServiceRouter, case: &str, id: &str, state: &str) -> Value {
    let (status, network) = delete(router, id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected DeleteNetwork to answer 202, got {status}: {network}"
    );
    assert_eq!(
        network["state"],
        json!(state),
        "case {case:?}: expected the delete to report {state}, got {:?}",
        network["state"]
    );
    network
}

/// Every member a network response carries beside the `State` the lifecycle owns
/// (`smithy_query(query="operation", operation="DeleteNetwork")`).
const CONFIGURATION: [&str; 6] = [
    "arn",
    "id",
    "name",
    "ipPools",
    "routes",
    "associatedClusterIds",
];

/// Assert every member but `state` reads back in `after` exactly as it did in
/// `before`: a deletion changes the lifecycle member and nothing else.
fn configuration_is_unchanged(case: &str, step: &str, before: &Value, after: &Value) {
    for member in CONFIGURATION {
        assert_eq!(
            after.get(member).cloned().unwrap_or(Value::Null),
            before.get(member).cloned().unwrap_or(Value::Null),
            "case {case:?}: {step}: expected {member} to be unchanged, got {:?}",
            after.get(member)
        );
    }
}

/// The one network `ListNetworks` reports, asserted to be the only one stored.
async fn only_listed(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = list(router, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListNetworks to answer 200, got {status}: {body}"
    );
    let mut page = summaries(case, &body);
    assert_eq!(
        page.len(),
        1,
        "case {case:?}: expected the deleted network to still be listed, got {body}"
    );
    page.remove(0)
}

/// One row of the deletion table: the create the row deletes, when the delete is
/// called and how many times.
struct Deletion {
    case: &'static str,
    /// The `CreateNetwork` body the row stores.
    body: fn() -> Value,
    /// Whether the create's transient `CREATING` is allowed to settle into
    /// `ACTIVE` before the delete is called.
    settle_before_delete: bool,
    /// How many `DeleteNetwork` calls the row makes inside the `DELETING`
    /// window. A row with more than one asserts the delete is idempotent.
    calls: usize,
}

/// Rule: `DeleteNetwork` accepts the deletion of a stored network — it answers
/// 202 with that network in `DELETING`, the clock owes it the terminal `DELETED`
/// it then keeps, and every other member it reports is the stored network read
/// back, which `DescribeNetwork` and `ListNetworks` go on reporting rather than
/// losing. A repeated delete is accepted on the same terms: it neither restarts
/// the clock nor revives the network.
///
/// The rows differ along WHAT the create stored (nothing but a name, no name at
/// all, pools and routes of several sizes), along WHEN the delete is called
/// (inside the transient `CREATING` window and after it has settled) and along
/// HOW MANY deletes are called, because that is what the rule generalises over.
#[tokio::test]
async fn a_deleted_network_moves_through_deleting_to_the_terminal_deleted() {
    let table = [
        Deletion {
            case: "nothing but a name, deleted while still creating",
            body: || json!({ "name": "doomed" }),
            settle_before_delete: false,
            calls: 1,
        },
        Deletion {
            case: "no name at all, deleted once active",
            body: || json!({}),
            settle_before_delete: true,
            calls: 1,
        },
        Deletion {
            case: "one pool, deleted once active",
            body: || json!({ "name": "one-pool", "ipPools": [{ "cidr": "192.0.2.0/24" }] }),
            settle_before_delete: true,
            calls: 1,
        },
        Deletion {
            case: "pools and routes, deleted three times over",
            body: || {
                json!({
                    "name": "pools-and-routes",
                    "ipPools": [
                        { "cidr": "192.0.2.0/24" },
                        { "cidr": "198.51.100.128/25" },
                    ],
                    "routes": [
                        { "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" },
                        { "cidr": "203.0.113.0/24" },
                    ],
                })
            },
            settle_before_delete: true,
            calls: 3,
        },
        Deletion {
            case: "empty lists, deleted twice while still creating",
            body: || json!({ "name": "empty-lists", "ipPools": [], "routes": [] }),
            settle_before_delete: false,
            calls: 2,
        },
    ];

    for Deletion {
        case,
        body,
        settle_before_delete,
        calls,
    } in table
    {
        let router = medialive();
        let network = created(&router, case, body()).await;
        let id = id_of(case, &network);

        if settle_before_delete {
            wait_for_the_clock();
        }

        // What the network looks like one instant before the delete, read back
        // out of the store through its own reader.
        let (status, before) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {before}"
        );
        let state_before = if settle_before_delete {
            "ACTIVE"
        } else {
            "CREATING"
        };
        assert_eq!(
            before["state"],
            json!(state_before),
            "case {case:?}: expected the network to be {state_before} before the delete, got {:?}",
            before["state"]
        );

        for index in 0..calls {
            let step = format!("delete {index}");
            // The delete reports the network in the transient state the clock
            // owes the terminal `DELETED`, and changes nothing else about it.
            let answered = accepted(&router, case, &id, "DELETING").await;
            configuration_is_unchanged(case, &step, &before, &answered);

            // Both readers answer from the one stored record, so each reports
            // exactly what the delete just answered.
            let (status, described) = describe(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {step}: expected the deleted record to stay readable, \
                 got {status}: {described}"
            );
            assert_eq!(
                described, answered,
                "case {case:?}: {step}: expected DescribeNetwork to report what the delete did"
            );
            assert_eq!(
                only_listed(&router, case).await,
                answered,
                "case {case:?}: {step}: expected the listing to report what the delete did"
            );
        }

        wait_for_the_clock();

        // The deletion has come due: the record is still there, in the terminal
        // state it now keeps, with every other member as it was.
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted record to stay readable, got {status}: {after}"
        );
        assert_eq!(
            after["state"],
            json!("DELETED"),
            "case {case:?}: expected the deletion to have settled, got {:?}",
            after["state"]
        );
        configuration_is_unchanged(case, "after the deletion came due", &before, &after);
        assert_eq!(
            only_listed(&router, case).await,
            after,
            "case {case:?}: expected the listing to agree with DescribeNetwork"
        );

        // A delete of a network already in the terminal state is accepted and
        // revives nothing: the state the caller is asking for is the stored one.
        let again = accepted(&router, case, &id, "DELETED").await;
        configuration_is_unchanged(case, "a delete after the terminal state", &before, &again);
        let (status, reread) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the record to stay readable, got {status}: {reread}"
        );
        assert_eq!(
            reread["state"],
            json!("DELETED"),
            "case {case:?}: expected the repeat not to restart the clock, got {:?}",
            reread["state"]
        );
    }
}

/// One row of the association table: the clusters created against the row's two
/// networks, each as the interfaces it maps. `Some(index)` names the row's
/// network at that index and `None` names the id nothing minted; a repeated index
/// is a second interface of the same cluster mapped onto one network.
///
/// The bodies themselves are built by the create suite's own
/// [`crate::networks::cluster_body`], so the two suites cannot disagree about how
/// a cluster is mapped onto a network.
struct Association {
    case: &'static str,
    clusters: &'static [&'static [Option<usize>]],
}

/// Rule: a network is deleted only once nothing is associated with it — "Delete a
/// Network. The Network must have no resources associated with it" — so a network
/// something is still attached to is refused with the operation's modelled
/// `ConflictException` (409) and left exactly as it was, and the association that
/// decides it is the one every read of the network reports through
/// `AssociatedClusterIds`.
///
/// The rows differ along the COUNT of clusters attached to the network under test
/// and the FORM of the mapping that attaches them (none at all, one cluster, two
/// clusters, two interfaces of one cluster, a cluster on the other network only,
/// a cluster with no mapping, a cluster naming an id nothing minted, a cluster
/// across both networks), because that is what the rule generalises over. Which
/// outcome each row expects is read off the REQUESTS, not off the emulator.
#[tokio::test]
async fn a_network_an_associated_resource_still_uses_is_not_deleted() {
    let table = [
        Association {
            case: "no cluster at all",
            clusters: &[],
        },
        Association {
            case: "one cluster on the network",
            clusters: &[&[Some(0)]],
        },
        Association {
            case: "two clusters on the network",
            clusters: &[&[Some(0)], &[Some(0)]],
        },
        Association {
            case: "two interfaces of one cluster on the network",
            clusters: &[&[Some(0), Some(0)]],
        },
        Association {
            case: "one cluster on the other network only",
            clusters: &[&[Some(1)]],
        },
        Association {
            case: "a cluster with no mapping at all",
            clusters: &[&[]],
        },
        Association {
            case: "a cluster mapped onto an id nothing minted",
            clusters: &[&[None]],
        },
        Association {
            case: "a cluster across both networks",
            clusters: &[&[Some(0), Some(1)]],
        },
    ];

    for Association { case, clusters } in table {
        let router = medialive();

        // Two networks, so a row can attach a cluster to one and leave the other
        // alone. The network under test is the first.
        let mut network_ids: Vec<String> = Vec::new();
        for index in 0..2usize {
            let network = created(
                &router,
                case,
                json!({ "name": format!("network-{index}") }),
            )
            .await;
            network_ids.push(id_of(case, &network));
        }

        // What the rule says is attached to the network under test, read off the
        // requests: the clusters one of whose mappings names it, in create order.
        let mut attached: Vec<String> = Vec::new();
        for (index, mappings) in clusters.iter().enumerate() {
            let spec = ClusterSpec {
                settings: true,
                mappings,
                default_route: None,
            };
            let body = cluster_body(format!("cluster-{index}"), &spec, &network_ids);
            let cluster_id = created_cluster(&router, case, body).await;
            if mappings.contains(&Some(0)) {
                attached.push(cluster_id);
            }
        }

        wait_for_the_clock();

        let id = network_ids[0].clone();
        let (status, before) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {before}"
        );
        // The association the delete is decided on is the one the read reports.
        assert_eq!(
            list_member(&before, "associatedClusterIds"),
            json!(attached),
            "case {case:?}: expected the clusters mapped onto the network, got {:?}",
            before["associatedClusterIds"]
        );

        if attached.is_empty() {
            let answered = accepted(&router, case, &id, "DELETING").await;
            configuration_is_unchanged(case, "an unattached network", &before, &answered);
            continue;
        }

        let (status, refused) = delete(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a network with {} associated resource(s) to be refused, \
             got {status}: {refused}",
            attached.len()
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        assert!(
            error_message(&refused).contains(&id),
            "case {case:?}: expected the refusal to name the network, got {refused}"
        );

        // The refusal wrote nothing: the network is still exactly what it was,
        // the delete having been taken before any state was touched.
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {after}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused delete to leave the network alone"
        );
    }
}

/// Rule: a `DeleteNetwork` whose `NetworkId` names no network this store holds
/// answers the operation's modelled `NotFoundException` (404) naming that id, and
/// deletes nothing — each resource type's ids come from a range of its own, so
/// another type's id names no network either.
///
/// The rows differ along the FORM of the id: one in the network id range nothing
/// minted, a channel's id, a non-numeric string and the empty-ish spellings a
/// client can put in the path.
#[tokio::test]
async fn a_delete_of_a_network_id_nothing_minted_is_not_found() {
    let router = medialive();
    let network = created(&router, "the bystander", json!({ "name": "bystander" })).await;
    let bystander = id_of("the bystander", &network);

    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "neighbour" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateChannel to answer 201, got {status}: {channel}"
    );
    let channel_id = id_of("a channel's id", &channel["channel"]);

    for case in [
        STRAY_NETWORK_ID,
        channel_id.as_str(),
        "not-an-id",
        "8000000000",
        "-1",
    ] {
        let (status, body) = delete(&router, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a network id nothing minted, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(case),
            "case {case:?}: expected the refusal to name the id, got {body}"
        );
    }

    // Every refusal above deleted nothing: the one stored network is still there
    // and still has the lifecycle of a network nobody deleted.
    let listed = only_listed(&router, "after the refusals").await;
    assert_eq!(
        listed["id"],
        json!(bystander),
        "expected the bystander to still be the one stored network, got {listed}"
    );
    assert_ne!(
        listed["state"],
        json!("DELETING"),
        "expected the refused deletes to have started no deletion, got {listed}"
    );
    assert_ne!(
        listed["state"],
        json!("DELETED"),
        "expected the refused deletes to have deleted nothing, got {listed}"
    );
}

/// Rule: a network whose delete this service has accepted is on its way out of
/// the account, so the `Name` it held — "unique in the AWS account" — is free for
/// the next create, and the `RequestId` that minted it stops answering once the
/// deletion has come due, while the deleted record itself stays readable under
/// the name it was created with.
///
/// The rows differ along WHEN the name and the token are reused: inside the
/// transient `DELETING` window and after the terminal `DELETED`, because that is
/// what the rule generalises over.
#[tokio::test]
async fn a_deleted_network_releases_the_name_it_held() {
    for (case, settle_after_delete) in [
        ("inside the DELETING window", false),
        ("after the terminal DELETED", true),
    ] {
        let router = medialive();
        let first = created(
            &router,
            case,
            json!({ "name": "shared", "requestId": "token-1" }),
        )
        .await;
        let first_id = id_of(case, &first);

        accepted(&router, case, &first_id, "DELETING").await;
        if settle_after_delete {
            wait_for_the_clock();
        }

        // The name is free again, so a second network takes it rather than being
        // refused the conflict a live namesake earns.
        let second = created(&router, case, json!({ "name": "shared" })).await;
        let second_id = id_of(case, &second);
        assert_ne!(
            second_id, first_id,
            "case {case:?}: expected the reused name to mint a second network"
        );

        // And the deleted record still reports the name it was created under:
        // releasing the name does not rewrite the record.
        let (status, deleted_record) = describe(&router, &first_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted record to stay readable, \
             got {status}: {deleted_record}"
        );
        assert_eq!(
            deleted_record["name"],
            json!("shared"),
            "case {case:?}: expected the deleted record to keep its name, got {:?}",
            deleted_record["name"]
        );

        // The token's idempotency window is over exactly when the deletion has
        // come due: before that a retry still answers with the network it
        // created, and afterwards it mints a new one.
        let retried = created(
            &router,
            case,
            json!({ "name": "token-retry", "requestId": "token-1" }),
        )
        .await;
        let retried_id = id_of(case, &retried);
        if settle_after_delete {
            assert!(
                retried_id != first_id && retried_id != second_id,
                "case {case:?}: expected a retry past the deletion to mint a new network, \
                 got {retried_id}"
            );
        } else {
            assert_eq!(
                retried_id, first_id,
                "case {case:?}: expected a retry inside the window to answer with the network \
                 the token created"
            );
        }
    }
}

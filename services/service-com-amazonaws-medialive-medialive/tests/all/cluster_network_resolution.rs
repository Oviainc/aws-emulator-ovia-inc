// SPDX-License-Identifier: CC0-1.0

//! The networks a MediaLive Anywhere cluster's interface mappings name, end to
//! end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.
//!
//! The rule both suites assert, over one table each operation that WRITES a
//! cluster's mappings is driven through: an `InterfaceMapping`'s `NetworkId` is
//! "the ID of the network that you want to connect to the specified
//! logicalInterfaceName. You can use the ListNetworks operation to discover all
//! the IDs" (`smithy_query(query="shape",
//! shape="InterfaceMappingUpdateRequest")`), so a mapping may only name a network
//! this account holds: one that names anything else is refused with the
//! operation's modelled 400 `BadRequestException` naming the member and the id,
//! and nothing is written.
//!
//! The rows span the FORM of the id that resolves to no network (one in the
//! network id range nothing minted, another resource type's id, a non-numeric
//! label, the empty spelling, a network's `Name` instead of its id, a network on
//! its way out of the account and one whose deletion has come due), the POSITION
//! of the offending entry in the list (only, first, middle, last) and the shapes
//! that must still be accepted (no mapping at all, one network, two interfaces on
//! one network, one interface per network) — because that is what the rule
//! generalises over. Which outcome each row expects is read off the ROW, not off
//! the emulator.
//!
//! The third suite asserts the OTHER side of the same reference, the side a
//! dangling id is invisible from: `DescribeNetwork` reports the
//! `AssociatedClusterIds` of a network (`smithy_query(query="operation",
//! operation="DescribeNetwork")`), and the only association this service knows
//! is the one a cluster's own `InterfaceMapping` entries carry — so the list a
//! network reports must follow the mappings an accepted `UpdateCluster` stored,
//! and must be left exactly as it was by a refused one. Its rows span the SET
//! the update asks for (the other network, both of them, the same one named
//! twice, none at all, the one already stored) and a refused remap, because that
//! is what the association generalises over.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, error_message, medialive};

/// The cluster collection URI `CreateCluster` and `ListClusters` are bound to
/// (`smithy_query(query="operation",operation="CreateCluster,ListClusters")`:
/// POST / GET /prod/clusters).
const CLUSTERS: &str = "/prod/clusters";

/// The network collection URI `CreateNetwork` is bound to
/// (`smithy_query(query="operation",operation="CreateNetwork")`: POST
/// /prod/networks).
const NETWORKS: &str = "/prod/networks";

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape",shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// What one `InterfaceMapping` entry of a row names.
#[derive(Clone, Copy, Debug)]
enum Mapped {
    /// The network at this index of the ones the account holds.
    Held(usize),
    /// An id that names no network this account holds.
    Unresolvable(Unresolvable),
}

/// The forms an id that names no network of the account takes.
#[derive(Clone, Copy, Debug)]
enum Unresolvable {
    /// An id of the network id range that nothing minted.
    NeverMinted,
    /// A held network's id with a digit appended — the right form and one digit
    /// wider, which the store holds no network under.
    DigitAppended,
    /// An id that is not of the numeric form MediaLive mints at all.
    NonNumeric,
    /// No id at all, spelled as the empty string.
    Empty,
    /// A network's `Name`, which is not the `Id` a mapping names.
    HeldNetworkName,
    /// A network whose delete this account has asked for, before the deletion
    /// has come due: it is on its way out, so nothing new connects to it.
    OnItsWayOut,
    /// A network whose deletion has come due.
    Gone,
}

/// One row: the mappings the request carries, as the row names them.
struct Row {
    case: &'static str,
    mappings: &'static [Mapped],
}

/// The table both suites run, spanning the forms, the positions and the shapes
/// the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "no mapping at all",
        mappings: &[],
    },
    Row {
        case: "one interface on one held network",
        mappings: &[Mapped::Held(0)],
    },
    Row {
        case: "two interfaces on the same held network",
        mappings: &[Mapped::Held(0), Mapped::Held(0)],
    },
    Row {
        case: "one interface per held network",
        mappings: &[Mapped::Held(0), Mapped::Held(1)],
    },
    Row {
        case: "the only entry naming an id nothing minted",
        mappings: &[Mapped::Unresolvable(Unresolvable::NeverMinted)],
    },
    Row {
        case: "the last of two entries naming an id a digit wider than a held one",
        mappings: &[
            Mapped::Held(0),
            Mapped::Unresolvable(Unresolvable::DigitAppended),
        ],
    },
    Row {
        case: "the first of two entries naming a non-numeric id",
        mappings: &[
            Mapped::Unresolvable(Unresolvable::NonNumeric),
            Mapped::Held(1),
        ],
    },
    Row {
        case: "the middle of three entries naming no id at all",
        mappings: &[
            Mapped::Held(0),
            Mapped::Unresolvable(Unresolvable::Empty),
            Mapped::Held(1),
        ],
    },
    Row {
        case: "an entry naming a held network's name instead of its id",
        mappings: &[Mapped::Unresolvable(Unresolvable::HeldNetworkName)],
    },
    Row {
        case: "an entry naming a network on its way out of the account",
        mappings: &[Mapped::Unresolvable(Unresolvable::OnItsWayOut)],
    },
    Row {
        case: "an entry naming a network whose deletion has come due",
        mappings: &[
            Mapped::Held(1),
            Mapped::Unresolvable(Unresolvable::Gone),
            Mapped::Held(0),
        ],
    },
];

/// The networks and neighbours the rows name, created through the service's own
/// operations so every id here is one the store really minted.
struct Fixture {
    /// The networks this account holds, in the order they were created.
    held: Vec<String>,
    /// The `Name` the first held network was created under.
    held_name: String,
    /// A network whose delete has been accepted and has not come due yet.
    on_its_way_out: String,
    /// A network whose deletion has come due.
    gone: String,
}

impl Fixture {
    /// Two networks the account keeps, one it is losing, one it has lost, and a
    /// cluster of its own, so the store a refusal is asserted against is not an
    /// empty one.
    async fn new(router: &MultiServiceRouter) -> Self {
        let held_name = "held-0".to_string();
        let mut held = Vec::new();
        for name in [held_name.as_str(), "held-1"] {
            held.push(created_network(router, name).await);
        }
        let gone = created_network(router, "gone").await;
        let on_its_way_out = created_network(router, "going").await;

        deleted_network(router, &gone).await;
        // The deletion the row names has to have COME DUE, which it does once
        // the clock has reached the instant the accepted delete stored.
        wait_for_the_clock();
        deleted_network(router, &on_its_way_out).await;

        let (status, cluster) = call(
            router,
            "POST",
            CLUSTERS,
            Some(json!({ "name": "bystander", "clusterType": ON_PREMISES })),
        )
        .await;
        assert_eq!(
            status, 201,
            "setup: expected CreateCluster to answer 201, got {status}: {cluster}"
        );

        Self {
            held,
            held_name,
            on_its_way_out,
            gone,
        }
    }

    /// The id one entry of a row spells.
    fn spelling(&self, mapped: Mapped) -> String {
        match mapped {
            Mapped::Held(index) => self.held[index].clone(),
            Mapped::Unresolvable(Unresolvable::NeverMinted) => "9999999".to_string(),
            Mapped::Unresolvable(Unresolvable::DigitAppended) => format!("{}0", self.held[0]),
            Mapped::Unresolvable(Unresolvable::NonNumeric) => "not-a-network".to_string(),
            Mapped::Unresolvable(Unresolvable::Empty) => String::new(),
            Mapped::Unresolvable(Unresolvable::HeldNetworkName) => self.held_name.clone(),
            Mapped::Unresolvable(Unresolvable::OnItsWayOut) => self.on_its_way_out.clone(),
            Mapped::Unresolvable(Unresolvable::Gone) => self.gone.clone(),
        }
    }

    /// The `InterfaceMappings` list a row asks for, each entry naming its own
    /// logical interface so the entries differ by more than their network.
    fn mappings(&self, mappings: &[Mapped]) -> Value {
        mappings
            .iter()
            .enumerate()
            .map(|(slot, mapped)| {
                json!({
                    "logicalInterfaceName": format!("eth{slot}"),
                    "networkId": self.spelling(*mapped),
                })
            })
            .collect()
    }
}

/// `CreateNetwork` under `name`, answering the minted id.
async fn created_network(router: &MultiServiceRouter, name: &str) -> String {
    let (status, network) = call(
        router,
        "POST",
        NETWORKS,
        Some(json!({ "name": name, "ipPools": [{ "cidr": "10.0.0.0/16" }] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateNetwork({name}) to answer 201, got {status}: {network}"
    );
    network["id"].as_str().unwrap_or_default().to_string()
}

/// `DeleteNetwork` on `id`, asserted to have been accepted (202, the status the
/// model declares for it).
async fn deleted_network(router: &MultiServiceRouter, id: &str) {
    let (status, body) = call(router, "DELETE", &format!("{NETWORKS}/{id}"), None).await;
    assert_eq!(
        status, 202,
        "setup: expected DeleteNetwork({id}) to be accepted, got {status}: {body}"
    );
}

/// Which entry of a row, if any, names no network the account holds — the first
/// one, because that is the one the refusal is about.
fn first_unresolvable(mappings: &[Mapped]) -> Option<usize> {
    mappings
        .iter()
        .position(|mapped| matches!(mapped, Mapped::Unresolvable(_)))
}

/// A list member as a client reads it: an absent member and an empty list both
/// mean "no entries".
fn list_member(body: &Value, name: &str) -> Value {
    match body.get(name) {
        None | Some(Value::Null) => json!([]),
        Some(other) => other.clone(),
    }
}

/// The mappings a cluster response reports.
fn reported_mappings(cluster: &Value) -> Value {
    list_member(&cluster["networkSettings"], "interfaceMappings")
}

/// The ids `ListClusters` reports, which is what a refused create must leave
/// alone.
async fn cluster_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", CLUSTERS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListClusters to answer 200, got {status}: {body}"
    );
    body["clusters"]
        .as_array()
        .map(|page| {
            page.iter()
                .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `AssociatedClusterIds` a network reports, which is the network side of the
/// same association a mapping writes.
async fn associated_cluster_ids(
    router: &MultiServiceRouter,
    case: &str,
    network_id: &str,
) -> Vec<String> {
    let (status, network) = call(router, "GET", &format!("{NETWORKS}/{network_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {network}"
    );
    network["associatedClusterIds"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `DescribeCluster` on `id`, asserted to be readable.
async fn described(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, cluster) = call(router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeCluster to answer 200, got {status}: {cluster}"
    );
    cluster
}

/// The refusal a write naming a network the account does not hold answers with:
/// the modelled 400 `BadRequestException`, whose message names the member the
/// caller must change — the mapping list, the position of the entry that broke
/// the rule and the id it spelled.
fn refusal_is_about(case: &str, status: u16, body: &Value, position: usize, spelled: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected a mapping naming {spelled:?} to be refused with 400, \
         got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {body}"
    );
    let message = error_message(body);
    for expected in [
        "networkId".to_string(),
        "interfaceMappings".to_string(),
        format!("[{position}]"),
    ] {
        assert!(
            message.contains(&expected),
            "case {case:?}: expected the refusal to name {expected:?}, got {message:?}"
        );
    }
    assert!(
        spelled.is_empty() || message.contains(spelled),
        "case {case:?}: expected the refusal to name the id {spelled:?} the request spelled, \
         got {message:?}"
    );
}

/// Rule: `CreateCluster` stores a cluster only when every `InterfaceMapping` that
/// names a network names one this account holds; a mapping naming anything else
/// is refused with the modelled 400 `BadRequestException` naming the member and
/// the id, and no cluster is stored.
#[tokio::test]
async fn a_created_cluster_is_mapped_only_onto_networks_the_account_holds() {
    let router = medialive();
    let fixture = Fixture::new(&router).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let asked = fixture.mappings(row.mappings);
        let before = cluster_ids(&router, case).await;

        let (status, answer) = call(
            &router,
            "POST",
            CLUSTERS,
            Some(json!({
                "name": format!("cluster-{index}"),
                "clusterType": ON_PREMISES,
                "networkSettings": { "interfaceMappings": asked.clone() },
            })),
        )
        .await;

        if let Some(position) = first_unresolvable(row.mappings) {
            let spelled = fixture.spelling(row.mappings[position]);
            refusal_is_about(case, status, &answer, position, &spelled);
            // A refused create stores NO cluster: the account holds exactly the
            // clusters it held before the call.
            assert_eq!(
                cluster_ids(&router, case).await,
                before,
                "case {case:?}: expected a refused create to store no cluster"
            );
            continue;
        }

        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateCluster to answer 201, got {status}: {answer}"
        );
        let id = answer["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a minted cluster id, got {answer}"))
            .to_string();
        // The accepted mappings are read back out of the store, not echoed.
        let read = described(&router, case, &id).await;
        assert_eq!(
            reported_mappings(&read),
            asked,
            "case {case:?}: expected the stored mappings to read back, got {:?}",
            read["networkSettings"]
        );
        // And the network side of the association agrees: a held network reports
        // this cluster exactly when one of its mappings named that network.
        for (slot, network_id) in fixture.held.iter().enumerate() {
            let mapped = row
                .mappings
                .iter()
                .any(|mapped| matches!(mapped, Mapped::Held(held) if *held == slot));
            let associated = associated_cluster_ids(&router, case, network_id).await;
            assert_eq!(
                associated.contains(&id),
                mapped,
                "case {case:?}: expected network {network_id} to report the cluster: {mapped}, \
                 got {associated:?}"
            );
        }
    }
}

/// Rule: `UpdateCluster` is held to the same resolution — an `InterfaceMappings`
/// list it would store may only name networks this account holds, and a list
/// naming anything else is refused with the modelled 400 `BadRequestException`,
/// leaving the mappings the cluster already carried exactly as they were.
#[tokio::test]
async fn an_updated_cluster_is_mapped_only_onto_networks_the_account_holds() {
    let router = medialive();
    let fixture = Fixture::new(&router).await;
    let original = fixture.mappings(&[Mapped::Held(0)]);

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // Each row starts from a cluster already mapped onto a held network, so a
        // refused update can be shown to have left that mapping alone.
        let (status, created) = call(
            &router,
            "POST",
            CLUSTERS,
            Some(json!({
                "name": format!("under-test-{index}"),
                "clusterType": ON_PREMISES,
                "networkSettings": { "interfaceMappings": original.clone() },
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateCluster to answer 201, got {status}: {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a minted cluster id, got {created}"))
            .to_string();

        let asked = fixture.mappings(row.mappings);
        let (status, answer) = call(
            &router,
            "PUT",
            &format!("{CLUSTERS}/{id}"),
            Some(json!({ "networkSettings": { "interfaceMappings": asked.clone() } })),
        )
        .await;

        if let Some(position) = first_unresolvable(row.mappings) {
            let spelled = fixture.spelling(row.mappings[position]);
            refusal_is_about(case, status, &answer, position, &spelled);
            let read = described(&router, case, &id).await;
            assert_eq!(
                reported_mappings(&read),
                original,
                "case {case:?}: expected the refused update to leave the stored mappings alone, \
                 got {:?}",
                read["networkSettings"]
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateCluster to answer 200, got {status}: {answer}"
        );
        assert_eq!(
            reported_mappings(&answer),
            asked,
            "case {case:?}: expected the update to report the mappings it wrote, got {:?}",
            answer["networkSettings"]
        );
        let read = described(&router, case, &id).await;
        assert_eq!(
            reported_mappings(&read),
            asked,
            "case {case:?}: expected the accepted mappings to be stored, got {:?}",
            read["networkSettings"]
        );
    }
}

/// One row of the remap table: the mappings the cluster is created with, the
/// mappings the update asks for, and the networks that must report the cluster
/// once the call has been answered.
struct Remap {
    case: &'static str,
    created: &'static [Mapped],
    asked: &'static [Mapped],
    /// The positions in [`Fixture::held`] that must report the cluster after the
    /// call — read off the ROW, not off the emulator.
    associated: &'static [usize],
}

/// The remap table, spanning the sets an update may ask a cluster to be mapped
/// onto and the refusals that leave the stored set alone.
const REMAPS: &[Remap] = &[
    Remap {
        case: "a remap onto the other held network moves the association",
        created: &[Mapped::Held(0)],
        asked: &[Mapped::Held(1)],
        associated: &[1],
    },
    Remap {
        case: "a remap onto both held networks associates both",
        created: &[Mapped::Held(0)],
        asked: &[Mapped::Held(0), Mapped::Held(1)],
        associated: &[0, 1],
    },
    Remap {
        case: "two interfaces on one held network associate that network once",
        created: &[Mapped::Held(0), Mapped::Held(1)],
        asked: &[Mapped::Held(1), Mapped::Held(1)],
        associated: &[1],
    },
    Remap {
        case: "an emptied mapping list drops every association",
        created: &[Mapped::Held(0), Mapped::Held(1)],
        asked: &[],
        associated: &[],
    },
    Remap {
        case: "re-sending the mappings the cluster already carries keeps them",
        created: &[Mapped::Held(0)],
        asked: &[Mapped::Held(0)],
        associated: &[0],
    },
    Remap {
        case: "a refused remap leaves the association the cluster already had",
        created: &[Mapped::Held(0)],
        asked: &[Mapped::Unresolvable(Unresolvable::NeverMinted)],
        associated: &[0],
    },
    Remap {
        case: "a refused remap naming a network on its way out keeps both associations",
        created: &[Mapped::Held(0), Mapped::Held(1)],
        asked: &[
            Mapped::Held(1),
            Mapped::Unresolvable(Unresolvable::OnItsWayOut),
        ],
        associated: &[0, 1],
    },
];

/// Rule: the clusters a network reports are derived from the mappings the
/// clusters currently carry, so an accepted `UpdateCluster` moves the
/// association onto exactly the networks its new `InterfaceMappings` name — once
/// each, however many interfaces name one network — and a refused one leaves
/// both the stored mappings and the network side of the association exactly as
/// they were.
#[tokio::test]
async fn the_network_side_association_follows_the_mappings_an_update_stored() {
    let router = medialive();
    let fixture = Fixture::new(&router).await;

    for (index, row) in REMAPS.iter().enumerate() {
        let case = row.case;
        let created_mappings = fixture.mappings(row.created);
        let (status, created) = call(
            &router,
            "POST",
            CLUSTERS,
            Some(json!({
                "name": format!("remapped-{index}"),
                "clusterType": ON_PREMISES,
                "networkSettings": { "interfaceMappings": created_mappings.clone() },
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateCluster to answer 201, got {status}: {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a minted cluster id, got {created}"))
            .to_string();

        let asked = fixture.mappings(row.asked);
        let (status, answer) = call(
            &router,
            "PUT",
            &format!("{CLUSTERS}/{id}"),
            Some(json!({ "networkSettings": { "interfaceMappings": asked.clone() } })),
        )
        .await;

        // What the cluster is left carrying is the list the ROW implies: the one
        // the update asked for when the row resolves, and the one the create
        // stored when it does not.
        let left_carrying = match first_unresolvable(row.asked) {
            Some(position) => {
                let spelled = fixture.spelling(row.asked[position]);
                refusal_is_about(case, status, &answer, position, &spelled);
                created_mappings
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected UpdateCluster to answer 200, got {status}: {answer}"
                );
                asked
            }
        };
        let read = described(&router, case, &id).await;
        assert_eq!(
            reported_mappings(&read),
            left_carrying,
            "case {case:?}: expected the cluster to carry the mappings the call left it with, \
             got {:?}",
            read["networkSettings"]
        );

        // And the network side agrees with that stored list, network by network:
        // a network the mappings name reports the cluster exactly once, and one
        // they do not name does not report it at all.
        for (slot, network_id) in fixture.held.iter().enumerate() {
            let expected = usize::from(row.associated.contains(&slot));
            let associated = associated_cluster_ids(&router, case, network_id).await;
            let times = associated.iter().filter(|other| **other == id).count();
            assert_eq!(
                times, expected,
                "case {case:?}: expected network {network_id} to report cluster {id} \
                 {expected} time(s), got {associated:?}"
            );
        }
    }
}

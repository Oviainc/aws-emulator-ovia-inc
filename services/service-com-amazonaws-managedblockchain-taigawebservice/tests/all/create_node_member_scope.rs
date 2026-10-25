// SPDX-License-Identifier: CC0-1.0

//! `CreateNode` resolves the `MemberId` a request names before it mints anything.
//!
//! The rule asserted here: a `MemberId` a request carries names the member that is to own
//! the node, so it must resolve to a member the NAMED network holds — whatever framework
//! that network runs. A framework whose networks hold no members resolves no member at
//! all, so a request that names one there asks for an owner that cannot exist and is
//! refused as not found; it is never served with the member dropped. On a Hyperledger
//! Fabric network a node belongs to a member, so there the request must name one.
//!
//! The table below spans the classes the rule generalises over: the framework of the
//! network (Hyperledger Fabric, and both AWS-owned public Ethereum networks) crossed with
//! what the request names for the member — nothing, a member of this network, a member of
//! another network, a well-formed id no network holds, and a value outside the id space
//! this service mints member ids from.
//!
//! Every refusal is also checked to be inert: the network's node listing does not move,
//! and the `ClientRequestToken` the refused call carried is still free to mint a node.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{available_network, call, is_resource_id, new_router, Answer, REGION};

/// An instance type Managed Blockchain offers on every network in this suite (the full
/// `bc.*` catalog is open to a Hyperledger Fabric STANDARD network and to Ethereum).
const INSTANCE_TYPE: &str = "bc.t3.large";

/// A member id of the shape this service mints — `m-` and 26 characters of the base-32
/// alphabet — that no network in this suite holds.
const UNHELD_MEMBER: &str = "m-ABCDEFGHIJKLMNOPQRSTUVWXY2";

/// A member id outside the space this service mints ids from (decimal zeros are not in the
/// base-32 alphabet an id body is drawn from), of the length `ResourceIdString`'s
/// `@length(1, 32)` allows. `MemberId` has no malformed-id refusal of its own, so this is
/// still just a member nothing holds.
const MEMBER_OUTSIDE_MINTED_SPACE: &str = "m-000000000000000000000000000000";

/// What the request names for `MemberId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// `MemberId` is omitted.
    Nothing,
    /// A member the network the node is created on holds.
    MemberOfThisNetwork,
    /// A member that exists — on a DIFFERENT network.
    MemberOfAnotherNetwork,
    /// A well-formed member id no network holds.
    UnheldMember,
    /// A value outside the minted id space.
    MemberOutsideMintedSpace,
}

/// What the emulator is expected to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// 200, and `GetNode` reports the named member as the node's owner (or no owner at
    /// all when the request named none).
    Created,
    /// A refusal: this status and this `x-amzn-errortype`.
    Refused(u16, &'static str),
}

/// One network the table is run against.
#[derive(Debug)]
struct Network {
    /// How a case names the network in its failure message.
    label: &'static str,
    /// The `NetworkId` requests are addressed to.
    id: String,
    /// A member this network holds, when its framework holds members at all.
    member: Option<String>,
}

/// Send one `CreateNode`, naming `member_id` only when there is one to name.
async fn create_node(
    router: &MultiServiceRouter,
    network_id: &str,
    token: &str,
    member_id: Option<&str>,
    state_db: Option<&str>,
) -> Answer {
    let mut configuration = json!({
        "InstanceType": INSTANCE_TYPE,
        // "Required for Ethereum nodes" (`NodeConfiguration$AvailabilityZone`), and
        // accepted on Hyperledger Fabric.
        "AvailabilityZone": format!("{REGION}a"),
    });
    if let Some(state_db) = state_db {
        configuration["StateDB"] = Value::String(state_db.to_string());
    }
    let mut body = json!({
        "ClientRequestToken": token,
        "NodeConfiguration": configuration,
    });
    if let Some(member_id) = member_id {
        body["MemberId"] = Value::String(member_id.to_string());
    }
    call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(body),
    )
    .await
}

/// The node ids `ListNodes` reports for a network, in the order it reports them.
async fn listed_nodes(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: Option<&str>,
) -> Vec<String> {
    let path = match member_id {
        Some(member_id) => format!("/networks/{network_id}/nodes?memberId={member_id}"),
        None => format!("/networks/{network_id}/nodes"),
    };
    let answer = call(router, "GET", &path, None).await;
    assert_eq!(
        answer.status, 200,
        "case ListNodes on {network_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["Id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A `MemberId` is resolved against the network it was named on, whatever that network's
/// framework: a member the network does not hold is a 404 and mints nothing, while a
/// member it does hold owns the node every later read reports.
#[tokio::test]
async fn create_node_resolves_the_member_it_is_asked_to_create_under() {
    let router = new_router();
    let (fabric_id, fabric_member) =
        available_network(&router, "member-scope-a", "scope-net-a").await;
    // A second Fabric network, so one case can name a member that really exists but on
    // another network.
    let (_other_network, other_member) =
        available_network(&router, "member-scope-b", "scope-net-b").await;

    // The networks: one Hyperledger Fabric network this account created, plus both
    // AWS-owned public Ethereum networks `CreateNode` documents — neither of which holds
    // any member.
    let networks = [
        Network {
            label: "HYPERLEDGER_FABRIC",
            id: fabric_id.clone(),
            member: Some(fabric_member.clone()),
        },
        Network {
            label: "ETHEREUM mainnet",
            id: "n-ethereum-mainnet".to_string(),
            member: None,
        },
        Network {
            label: "ETHEREUM goerli",
            id: "n-ethereum-goerli".to_string(),
            member: None,
        },
    ];

    // Rows: what the request names for the member, and what each framework answers for it.
    // A member the network holds is accepted; one it does not hold — including a real
    // member of another network — is not found, whichever framework the network runs. The
    // member may be omitted only where the framework's networks hold no members.
    let rows: &[(Named, Expect, Expect)] = &[
        (
            Named::Nothing,
            Expect::Refused(400, "InvalidRequestException"),
            Expect::Created,
        ),
        (
            Named::MemberOfThisNetwork,
            Expect::Created,
            // No member of a members-less network exists to name, so this row has no
            // Ethereum case; the expectation is unused there.
            Expect::Created,
        ),
        (
            Named::MemberOfAnotherNetwork,
            Expect::Refused(404, "ResourceNotFoundException"),
            Expect::Refused(404, "ResourceNotFoundException"),
        ),
        (
            Named::UnheldMember,
            Expect::Refused(404, "ResourceNotFoundException"),
            Expect::Refused(404, "ResourceNotFoundException"),
        ),
        (
            Named::MemberOutsideMintedSpace,
            Expect::Refused(404, "ResourceNotFoundException"),
            Expect::Refused(404, "ResourceNotFoundException"),
        ),
    ];

    for network in &networks {
        let fabric = network.member.is_some();
        // Every node this network ends up holding, by the member scope it is listed under.
        let mut expected_unscoped: Vec<String> = Vec::new();
        let mut expected_scoped: Vec<String> = Vec::new();

        for (index, (named, on_fabric, on_ethereum)) in rows.iter().enumerate() {
            // Which member id this row names on THIS network; `None` where the row does
            // not apply (a members-less network has no member of its own to name).
            let member_id: Option<Option<String>> = match named {
                Named::Nothing => Some(None),
                Named::MemberOfThisNetwork => network.member.clone().map(Some),
                // The second Fabric network's founding member: a member that exists, but
                // not on the network this case creates its node on.
                Named::MemberOfAnotherNetwork => Some(Some(other_member.clone())),
                Named::UnheldMember => Some(Some(UNHELD_MEMBER.to_string())),
                Named::MemberOutsideMintedSpace => {
                    Some(Some(MEMBER_OUTSIDE_MINTED_SPACE.to_string()))
                }
            };
            let Some(member_id) = member_id else {
                continue;
            };
            let expected = if fabric { *on_fabric } else { *on_ethereum };
            let case = format!("{named:?} on {}", network.label);
            let token = format!("member-scope-{}-{index}", network.id);

            let answer =
                create_node(&router, &network.id, &token, member_id.as_deref(), None).await;

            match expected {
                Expect::Created => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case}: expected CreateNode 200, got {} {:?}",
                        answer.status, answer.body
                    );
                    let node_id = answer.text(&["NodeId"]);
                    assert!(
                        is_resource_id(&node_id, "nd"),
                        "case {case}: expected a minted node id, got {node_id:?}"
                    );

                    // The owner round-trips: the member the request named is the one the
                    // stored node reports, and a request that named none reports none.
                    // `GetNode` takes the owning member as its scope on a Fabric network,
                    // so the read is addressed the same way the create was.
                    let path = match member_id.as_deref() {
                        Some(member_id) => format!(
                            "/networks/{}/nodes/{node_id}?memberId={member_id}",
                            network.id
                        ),
                        None => format!("/networks/{}/nodes/{node_id}", network.id),
                    };
                    let answer = call(&router, "GET", &path, None).await;
                    assert_eq!(
                        answer.body["Node"]["MemberId"].as_str(),
                        member_id.as_deref(),
                        "case {case}: expected GetNode to report the stored owner, got {:?}",
                        answer.body
                    );

                    if member_id.is_some() {
                        expected_scoped.push(node_id.clone());
                    }
                    expected_unscoped.push(node_id);
                }
                Expect::Refused(status, error_type) => {
                    assert_eq!(
                        (answer.status, answer.error_type.as_str()),
                        (status, error_type),
                        "case {case}: expected {status} {error_type}, got {} {:?}",
                        answer.status,
                        answer.body
                    );
                    if let Some(named_member) = member_id.as_deref() {
                        assert_eq!(
                            answer.text(&["Message"]),
                            format!("Member {named_member} not found."),
                            "case {case}: expected the refusal to name the member that did \
                             not resolve, got {:?}",
                            answer.body
                        );
                    }

                    // The refusal minted nothing and consumed nothing: the same token
                    // still mints a node, which it could not if the refused call had
                    // recorded it.
                    let answer =
                        create_node(&router, &network.id, &token, network.member.as_deref(), None)
                            .await;
                    assert_eq!(
                        answer.status, 200,
                        "case {case}: expected the token of a refused create to still mint \
                         a node, got {} {:?}",
                        answer.status, answer.body
                    );
                    let node_id = answer.text(&["NodeId"]);
                    if fabric {
                        expected_scoped.push(node_id.clone());
                    }
                    expected_unscoped.push(node_id);
                }
            }
        }

        // Exactly the accepted nodes exist, so no refusal left a node behind.
        let (scope, mut expected) = match network.member.as_deref() {
            Some(member) => (Some(member), expected_scoped),
            None => (None, expected_unscoped),
        };
        expected.sort();
        assert_eq!(
            listed_nodes(&router, &network.id, scope).await,
            expected,
            "case {}: expected exactly the accepted nodes to exist",
            network.label
        );
    }
}

/// A member a members-less network cannot own is refused BEFORE the node configuration is
/// read, so the refusal names the member rather than some later problem with the request,
/// and a Fabric node created under a resolved member still carries that member's scope on
/// every read.
#[tokio::test]
async fn an_unresolvable_member_is_refused_before_the_node_configuration() {
    let router = new_router();
    let (fabric_id, fabric_member) =
        available_network(&router, "member-first", "member-first-net").await;

    // A request that is wrong in two ways at once — a member an Ethereum network cannot
    // own AND a `StateDB` that framework has no state database for — is answered about
    // the member, the resource the node would be created under.
    let answer = create_node(
        &router,
        "n-ethereum-mainnet",
        "member-first-ethereum",
        Some(UNHELD_MEMBER),
        Some("CouchDB"),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case a member and a StateDB an Ethereum network cannot take: expected 404 \
         ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );

    // Nothing was created by it: the public network holds no node yet.
    assert!(
        listed_nodes(&router, "n-ethereum-mainnet", None)
            .await
            .is_empty(),
        "case a refused CreateNode on a public network: expected no node to exist"
    );

    // The same request shape with a member the network does hold is served, and the node
    // is reachable only under that member's scope.
    let answer = create_node(
        &router,
        &fabric_id,
        "member-first-fabric",
        Some(&fabric_member),
        Some("CouchDB"),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case a Fabric node under a member the network holds: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);
    assert_eq!(
        listed_nodes(&router, &fabric_id, Some(&fabric_member)).await,
        vec![node_id.clone()],
        "case a Fabric node under a member the network holds: expected the node to be \
         listed under its owner"
    );
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{fabric_id}/nodes/{node_id}?memberId={fabric_member}"),
        None,
    )
    .await;
    assert_eq!(
        answer.text(&["Node", "MemberId"]),
        fabric_member,
        "case GetNode under the owning member: expected the stored owner, got {:?}",
        answer.body
    );
}

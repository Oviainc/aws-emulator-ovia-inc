// SPDX-License-Identifier: CC0-1.0

//! One member-scoped node lookup serves `GetNode`, `DeleteNode` and `UpdateNode`.
//!
//! RULE: a `MemberId` a node request carries names the member that owns the node and only
//! SCOPES the lookup — a node that member does not own is not found under it, exactly as an
//! unknown node id is not — and on a Hyperledger Fabric network, where the model documents
//! the member as required, a request that names none is refused instead of answered from an
//! unscoped search.
//!
//! The three operations resolve that rule through ONE lookup, so the table below drives all
//! three down it from a single set of rows and asserts the same answer from each: the member
//! scope cannot mean one thing on a read and another on a write.
//!
//! The rows span what the rule generalises over: the FRAMEWORK of the network the node is on
//! (Hyperledger Fabric, whose nodes have an owning member, and an AWS-owned public Ethereum
//! network, whose nodes have none) crossed with what the request names for the scope —
//! nothing, the member that owns the node, another member of the same network, a member of
//! another network, a well-formed member id no network holds, and a value outside the id
//! space this service mints member ids from. Each row also asserts the refusals are INERT:
//! the node the refused call could not reach is still the node the owning scope reads, in
//! the status and with the configuration it had.
//!
//! Where `UpdateNode` differs is documented, not duplicated: `UpdateNodeInput$MemberId` is
//! "Applies only to Hyperledger Fabric" WITHOUT the "and is required for Hyperledger Fabric"
//! clause `GetNodeInput$MemberId` and `DeleteNodeInput$MemberId` carry, so only the scope
//! half of the rule applies to it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    available_network, call, router_and_engine, Answer, PAST_TRANSITION, REGION,
};
use crate::list_members::{seed_members, Seed};

/// An instance type Managed Blockchain offers on every network in this suite (the `bc.*`
/// catalog a Hyperledger Fabric STANDARD network and an Ethereum network both take).
const INSTANCE_TYPE: &str = "bc.t3.large";

/// The `Framework` value a Hyperledger Fabric network reports (the model's `Framework` enum,
/// `smithy_query(query="shape",shape="Framework")`), which the member-required refusal names.
const FABRIC: &str = "HYPERLEDGER_FABRIC";

/// One of the AWS-owned public Ethereum networks every Region reads, as `CreateNode`
/// documents them: a network whose nodes have no owning member.
const PUBLIC_ETHEREUM_NETWORK: &str = "n-ethereum-mainnet";

/// A member id of the shape this service mints — `m-` and 26 characters of the base-32
/// alphabet — that no network in this suite holds.
const UNHELD_MEMBER: &str = "m-ABCDEFGHIJKLMNOPQRSTUVWXY2";

/// A member id outside the space this service mints ids from (decimal zeros are not in the
/// base-32 alphabet an id body is drawn from), within `ResourceIdString`'s `@length(1, 32)`.
const MEMBER_OUTSIDE_MINTED_SPACE: &str = "m-000000000000000000000000000000";

/// What a request names for the member scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// `MemberId` is omitted.
    Omitted,
    /// The member that owns the node the request addresses.
    Owner,
    /// Another member of the SAME network, which owns no such node.
    OtherMemberOfThisNetwork,
    /// A member that exists — on a different network.
    MemberOfAnotherNetwork,
    /// A well-formed member id no network holds.
    UnheldMember,
    /// A value outside the minted member-id space.
    MemberOutsideMintedSpace,
}

/// What the emulator is expected to answer a scoped node request with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// 200: the scope resolved the node.
    Served,
    /// 400 `InvalidRequestException`: a Fabric network needs the owning member named.
    MemberRequired,
    /// 404 `ResourceNotFoundException` naming the node: the scope resolves no such node.
    NodeNotFound,
}

/// One node operation the member scope is resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Get,
    Update,
    Delete,
}

/// One row of the table: the scope a request names, and what each framework answers it with.
#[derive(Debug)]
struct Row {
    scope: Scope,
    /// What `GetNode`/`DeleteNode` and what `UpdateNode` answer on a network whose nodes
    /// have an owning member.
    on_fabric: (Expect, Expect),
    /// The same two, on a network whose nodes have none.
    on_ethereum: (Expect, Expect),
}

/// One row made ready to run: the node it addresses exists, and the scope it names has been
/// resolved to this network's ids.
#[derive(Debug)]
struct Prepared {
    scope: Scope,
    /// The `MemberId` the request names, or `None` when the row names none.
    named: Option<String>,
    /// What `GetNode` and `DeleteNode` answer.
    reads: Expect,
    /// What `UpdateNode` answers.
    update: Expect,
    node_id: String,
}

/// The network a block of rows runs against.
#[derive(Debug)]
struct Host {
    /// How a case names this network in its failure message.
    label: &'static str,
    /// The `NetworkId` requests are addressed to.
    id: String,
    /// The member that owns the node, where the framework gives a node an owner.
    owner: Option<String>,
    /// Another member of the same network, where the framework holds members.
    other_member: Option<String>,
}

/// A `NodeLogPublishingConfiguration` turning the Fabric peer log stream on or off (the
/// nested member names come from `smithy_query` shapes `NodeLogPublishingConfiguration` ->
/// `Fabric`, `NodeFabricLogPublishingConfiguration` -> `PeerLogs`, `LogConfigurations` ->
/// `Cloudwatch`, `LogConfiguration` -> `Enabled`).
fn peer_logs(enabled: bool) -> Value {
    json!({ "Fabric": { "PeerLogs": { "Cloudwatch": { "Enabled": enabled } } } })
}

/// Create one node on `network_id`, owned by `member_id` when the framework gives it an
/// owner, and return its `NodeId`.
async fn create_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: Option<&str>,
    token: &str,
) -> String {
    let mut body = json!({
        "ClientRequestToken": token,
        "NodeConfiguration": {
            "InstanceType": INSTANCE_TYPE,
            // "Required for Ethereum nodes" (`NodeConfiguration$AvailabilityZone`), and
            // accepted on Hyperledger Fabric.
            "AvailabilityZone": format!("{REGION}a"),
            "LogPublishingConfiguration": peer_logs(false),
        },
    });
    if let Some(member_id) = member_id {
        body["MemberId"] = Value::String(member_id.to_string());
    }
    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {body:?}: expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["NodeId"])
}

/// Send one node operation, scoped by `scope` when the row names a member.
///
/// `GetNode` and `DeleteNode` carry the scope as the `memberId` QUERY parameter;
/// `UpdateNode` carries it in the request body.
async fn node_call(
    router: &MultiServiceRouter,
    op: Op,
    network_id: &str,
    node_id: &str,
    scope: Option<&str>,
) -> Answer {
    let query = match (op, scope) {
        (Op::Update, _) | (_, None) => String::new(),
        (_, Some(member_id)) => format!("?memberId={member_id}"),
    };
    let path = format!("/networks/{network_id}/nodes/{node_id}{query}");
    match op {
        Op::Get => call(router, "GET", &path, None).await,
        Op::Delete => call(router, "DELETE", &path, None).await,
        Op::Update => {
            let mut body = json!({ "LogPublishingConfiguration": peer_logs(true) });
            if let Some(member_id) = scope {
                body["MemberId"] = Value::String(member_id.to_string());
            }
            call(router, "PATCH", &path, Some(body)).await
        }
    }
}

/// Assert one answer against the row's expectation, naming the case and the actual answer.
fn assert_answer(case: &str, expect: Expect, node_id: &str, answer: &Answer) {
    match expect {
        Expect::Served => assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} {:?}",
            answer.status, answer.body
        ),
        Expect::MemberRequired => {
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (400, "InvalidRequestException"),
                "case {case}: expected 400 InvalidRequestException, got {} {:?}",
                answer.status,
                answer.body
            );
            let expected = format!("memberId is required on a {FABRIC} network");
            assert_eq!(
                answer.text(&["Message"]),
                expected,
                "case {case}: expected the refusal {expected:?}, got {:?}",
                answer.body
            );
        }
        Expect::NodeNotFound => {
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
                answer.status,
                answer.body
            );
            let expected = format!("Node {node_id} not found.");
            assert_eq!(
                answer.text(&["Message"]),
                expected,
                "case {case}: expected the refusal to name the node ({expected:?}), got {:?}",
                answer.body
            );
        }
    }
}

/// Every node operation resolves the member scope the same way, and a scope that resolves
/// nothing reaches nothing.
#[tokio::test]
async fn the_member_scope_resolves_the_same_node_for_every_node_operation() {
    let (router, engine) = router_and_engine();
    let (fabric_id, founder) = available_network(&router, "scope-a", "scope-net-a").await;
    // A second member of the SAME Fabric network, so a row can name a member that network
    // really holds but that owns no node — `CreateNetwork` mints only the founding one.
    let seeded = seed_members(
        &engine,
        &fabric_id,
        &[Seed {
            name: "second-org",
            status: "AVAILABLE",
            account: crate::harness::ACCOUNT,
        }],
    );
    let second_member = seeded
        .first()
        .cloned()
        .expect("the seeded member is stored");
    // A second network, so a row can name a member that exists on another network.
    let (_other_network, other_member) =
        available_network(&router, "scope-b", "scope-net-b").await;

    let hosts = [
        Host {
            label: "HYPERLEDGER_FABRIC",
            id: fabric_id.clone(),
            owner: Some(founder.clone()),
            other_member: Some(second_member.clone()),
        },
        Host {
            label: "public ETHEREUM",
            id: PUBLIC_ETHEREUM_NETWORK.to_string(),
            owner: None,
            other_member: None,
        },
    ];

    // Rows: the scope a request names, and what it answers on a network whose nodes have an
    // owning member and on one whose nodes have none. A scope that names no member is the
    // one place the two reads differ from the update: the model makes the member required on
    // Fabric for `GetNode` and `DeleteNode` only.
    let rows = &[
        Row {
            scope: Scope::Omitted,
            // On Fabric: required for the reads, optional for the update.
            on_fabric: (Expect::MemberRequired, Expect::Served),
            // On a framework whose nodes have no owner, naming none addresses the node.
            on_ethereum: (Expect::Served, Expect::Served),
        },
        Row {
            scope: Scope::Owner,
            on_fabric: (Expect::Served, Expect::Served),
            // No owner exists to name on a members-less network: the row is skipped there.
            on_ethereum: (Expect::Served, Expect::Served),
        },
        Row {
            scope: Scope::OtherMemberOfThisNetwork,
            on_fabric: (Expect::NodeNotFound, Expect::NodeNotFound),
            // No member of a members-less network exists to name: skipped there too.
            on_ethereum: (Expect::NodeNotFound, Expect::NodeNotFound),
        },
        Row {
            scope: Scope::MemberOfAnotherNetwork,
            on_fabric: (Expect::NodeNotFound, Expect::NodeNotFound),
            on_ethereum: (Expect::NodeNotFound, Expect::NodeNotFound),
        },
        Row {
            scope: Scope::UnheldMember,
            on_fabric: (Expect::NodeNotFound, Expect::NodeNotFound),
            on_ethereum: (Expect::NodeNotFound, Expect::NodeNotFound),
        },
        Row {
            scope: Scope::MemberOutsideMintedSpace,
            on_fabric: (Expect::NodeNotFound, Expect::NodeNotFound),
            on_ethereum: (Expect::NodeNotFound, Expect::NodeNotFound),
        },
    ];

    for host in &hosts {
        let owned = host.owner.is_some();

        // Each row gets a node of its own, so the delete one row makes cannot change what
        // the next reads. They are created together and settled once, because a node the
        // clock still owes `AVAILABLE` is refused a delete on a status this suite is not
        // about.
        let mut prepared: Vec<Prepared> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            // Which member id this row names on THIS network; `None` where the row cannot
            // apply (a members-less network has no member of its own to name).
            let named: Option<Option<String>> = match row.scope {
                Scope::Omitted => Some(None),
                Scope::Owner => host.owner.clone().map(Some),
                Scope::OtherMemberOfThisNetwork => host.other_member.clone().map(Some),
                Scope::MemberOfAnotherNetwork => Some(Some(other_member.clone())),
                Scope::UnheldMember => Some(Some(UNHELD_MEMBER.to_string())),
                Scope::MemberOutsideMintedSpace => {
                    Some(Some(MEMBER_OUTSIDE_MINTED_SPACE.to_string()))
                }
            };
            let Some(named) = named else {
                continue;
            };
            let (reads, update) = if owned {
                row.on_fabric
            } else {
                row.on_ethereum
            };
            let node_id = create_node(
                &router,
                &host.id,
                host.owner.as_deref(),
                &format!("scope-node-{}-{index}", host.id),
            )
            .await;
            prepared.push(Prepared {
                scope: row.scope,
                named,
                reads,
                update,
                node_id,
            });
        }
        tokio::time::sleep(PAST_TRANSITION).await;

        for case_row in &prepared {
            let Prepared {
                scope,
                named,
                reads,
                update,
                node_id,
            } = case_row;
            let (scope, reads, update) = (*scope, *reads, *update);
            for (op, expect) in [
                (Op::Get, reads),
                (Op::Update, update),
                (Op::Delete, reads),
            ] {
                let case = format!("{op:?} {scope:?} on {}", host.label);
                let answer = node_call(&router, op, &host.id, node_id, named.as_deref()).await;
                assert_answer(&case, expect, node_id, &answer);
                if op == Op::Get && expect == Expect::Served {
                    assert_eq!(
                        answer.text(&["Node", "Id"]),
                        *node_id,
                        "case {case}: expected the scope to resolve the node it addressed, \
                         got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.body["Node"]["MemberId"].as_str(),
                        host.owner.as_deref(),
                        "case {case}: expected the node's stored owner, got {:?}",
                        answer.body
                    );
                }
            }

            // What the refusals left behind: the node is still readable under the scope that
            // resolves it, the removal started only where the delete was served, and the
            // update's configuration landed only where the update was served.
            let case = format!("after {scope:?} on {}", host.label);
            let answer =
                node_call(&router, Op::Get, &host.id, node_id, host.owner.as_deref()).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the node to still be readable, got {} {:?}",
                answer.status, answer.body
            );
            let status = answer.text(&["Node", "Status"]);
            if reads == Expect::Served {
                assert_eq!(
                    status, "DELETING",
                    "case {case}: expected the served delete to have started the removal, \
                     got {status:?}"
                );
            } else {
                assert_ne!(
                    status, "DELETING",
                    "case {case}: expected the refused delete to have started no removal, \
                     got {status:?}"
                );
            }
            let expected_logs = peer_logs(update == Expect::Served);
            assert_eq!(
                answer.body["Node"]["LogPublishingConfiguration"],
                expected_logs,
                "case {case}: expected the stored configuration {expected_logs:?}, got {:?}",
                answer.body
            );
        }
    }
}

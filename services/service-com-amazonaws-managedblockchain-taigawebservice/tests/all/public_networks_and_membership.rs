// SPDX-License-Identifier: CC0-1.0

//! The control-plane rules this service answers beside the create/read lifecycle:
//!
//! * a `CreateNetwork` whose `Framework`/`FrameworkVersion` pair the service does not
//!   support is refused before anything is provisioned;
//! * AWS's standing, AWS-owned public networks resolve for any caller, and are reported
//!   in `ListNetworks` only once the account participates in one;
//! * `ListNodes`, which models no `ResourceNotFoundException`, refuses an unresolvable
//!   member scope as an invalid request;
//! * `ListProposals` pages the proposals stored on a network and 404s an unknown one;
//! * `CreateMember` joins a network by invitation, and refuses a network or an invitation
//!   that cannot be resolved.
//!
//! The tables span the classes those rules generalise over: supported and unsupported
//! framework versions (and the framework that supports none), every public network in the
//! catalog, every shape of unresolvable `ListNodes` scope, page sizes from one to past the
//! item count plus both published `MaxResults` bounds and one past each, and the member
//! configurations a join can carry.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state;

use crate::harness::{
    available_network, call, create_network, create_network_body, is_date_time, is_resource_id,
    member_configuration, new_router, router_and_engine, ACCOUNT, PAST_TRANSITION, REGION,
};

/// An account that is not the one this emulator answers for, so an invitation naming it is
/// not this caller's to use.
const OTHER_ACCOUNT: &str = "210987654321";

/// Record one invitation to `principal` onto `network_id`, created at `created`, through
/// the same state API an approved `InviteAction` goes through, and answer the invitation
/// id the service minted.
///
/// `VoteOnProposal` — which carries out the `InviteAction` of an approved proposal on the
/// real service — is still a scaffold stub (its own issue), so a join is exercised from a
/// recorded invitation rather than a fabricated one.
fn invite(
    engine: &engine::InMemoryEngine,
    network_id: &str,
    principal: &str,
    created: SystemTime,
) -> String {
    state::service_state(engine)
        .expect("this service's own store")
        .lock()
        .record_invitation(network_id, principal, REGION, created)
        .expect("the network exists and the store has room for another invitation")
}

/// The status the store holds for one invitation.
fn invitation_status(engine: &engine::InMemoryEngine, invitation_id: &str) -> String {
    let store = state::service_state(engine).expect("the service's own store");
    let guard = store.lock();
    guard
        .invitations
        .get(invitation_id)
        .map(|invitation| invitation.status.clone())
        .unwrap_or_default()
}

/// How many members the store holds on one network.
fn member_count(engine: &engine::InMemoryEngine, network_id: &str) -> usize {
    let store = state::service_state(engine).expect("the service's own store");
    let guard = store.lock();
    guard
        .networks
        .get(network_id)
        .map_or(0, |network| network.members.len())
}

/// Create `count` proposals on `network_id` as `member_id`, through `CreateProposal`, and
/// return the ids it minted in listing order.
async fn create_proposals(
    router: &smithy_server_runtime::MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for i in 0..count {
        let answer = call(
            router,
            "POST",
            &format!("/networks/{network_id}/proposals"),
            Some(json!({
                "ClientRequestToken": format!("proposal-token-{i}"),
                "MemberId": member_id,
                "Description": format!("proposal {i}"),
                // `InviteAction$Principal` carries no modelled constraint; these are
                // account-id shaped so the row reads like the invite it stands for.
                "Actions": {
                    "Invitations": [{ "Principal": format!("{:012}", 100_000_000_000u64 + i as u64) }]
                },
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case proposal {i}: expected CreateProposal 200, got {} {:?}",
            answer.status, answer.body
        );
        ids.push(answer.text(&["ProposalId"]));
    }
    ids.sort();
    ids
}

/// One row of the framework-version table.
#[derive(Debug)]
struct VersionCase {
    framework: &'static str,
    version: &'static str,
    /// Whether the pair names a network the service can create.
    supported: bool,
}

/// `CreateNetwork` provisions a network only for a `Framework`/`FrameworkVersion` pair the
/// service supports; any other pair is refused 400 `InvalidRequestException` with a
/// message built from the request, and nothing is created.
#[tokio::test]
async fn create_network_refuses_a_framework_version_the_framework_does_not_support() {
    let router = new_router();

    // The supported rows are the catalog Managed Blockchain publishes for Hyperledger
    // Fabric (`state::FABRIC_FRAMEWORK_VERSIONS`: 1.2, 1.4, 2.2 — `FrameworkVersionString`
    // is only `@length(1, 8)`, so the model does not enumerate them). The unsupported
    // rows differ along the shape of the version: the issue's own example, a major-only
    // one, neighbours of each supported version, a non-numeric one, and the version of
    // the framework that supports no create at all.
    let cases: &[VersionCase] = &[
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "1.2",
            supported: true,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "1.4",
            supported: true,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "2.2",
            supported: true,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "99.9",
            supported: false,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "2",
            supported: false,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "1.3",
            supported: false,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "2.3",
            supported: false,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "1.40",
            supported: false,
        },
        VersionCase {
            framework: "HYPERLEDGER_FABRIC",
            version: "latest",
            supported: false,
        },
        // `CreateNetwork` "Applies only to Hyperledger Fabric", so the other `Framework`
        // value supports no creatable version at all.
        VersionCase {
            framework: "ETHEREUM",
            version: "1.9",
            supported: false,
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let name = format!("version-case-{i}");
        let body = json!({
            "ClientRequestToken": format!("version-token-{i}"),
            "Name": name,
            "Framework": case.framework,
            "FrameworkVersion": case.version,
            "VotingPolicy": {
                "ApprovalThresholdPolicy": {
                    "ThresholdPercentage": 50,
                    "ProposalDurationInHours": 24,
                    "ThresholdComparator": "GREATER_THAN"
                }
            },
            "MemberConfiguration": member_configuration("org1"),
        });
        let answer = call(&router, "POST", "/networks", Some(body)).await;

        if case.supported {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected a supported pair to be created, got {} {:?}",
                answer.status, answer.body
            );
            created.push(name.clone());
            continue;
        }

        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
        let message = answer.text(&["Message"]);
        assert!(
            message.contains(case.framework),
            "case {case:?}: expected the message to name the framework, got {message:?}"
        );
        if case.framework == "HYPERLEDGER_FABRIC" {
            assert!(
                message.contains(case.version),
                "case {case:?}: expected the message to name the rejected version, got \
                 {message:?}"
            );
        }
        assert!(
            answer.body["NetworkId"].is_null() && answer.body["MemberId"].is_null(),
            "case {case:?}: a refusal mints no ids, got {:?}",
            answer.body
        );

        // Nothing was provisioned: the refused name names no network.
        let listing = call(&router, "GET", &format!("/networks?name={name}"), None).await;
        assert_eq!(
            listing.body["Networks"].as_array().map_or(0, Vec::len),
            0,
            "case {case:?}: the refused network must not exist, got {:?}",
            listing.body
        );
    }

    // And exactly the supported pairs were stored.
    let listing = call(&router, "GET", "/networks?maxResults=10", None).await;
    assert_eq!(
        listing.body["Networks"].as_array().map_or(0, Vec::len),
        created.len(),
        "expected one network per supported pair ({created:?}), got {:?}",
        listing.body
    );
}

/// One of AWS's standing, AWS-owned public networks, as a caller reads it.
#[derive(Debug)]
struct PublicCase {
    id: &'static str,
    name: &'static str,
    chain_id: &'static str,
}

/// `GetNetwork` resolves AWS's standing, AWS-owned public networks for any caller — they
/// are present without anything being created — while an id nothing holds is still the
/// modelled 404.
#[tokio::test]
async fn get_network_serves_the_aws_owned_public_networks() {
    let (router, engine) = router_and_engine();

    // Rows: the whole catalog the service seeds (`state::PUBLIC_NETWORKS`), whose source
    // is `NetworkEthereumAttributes$ChainId` ("mainnet = 1") together with
    // `AccessorNetworkType`'s public Ethereum networks (ETHEREUM_MAINNET,
    // ETHEREUM_GOERLI). The issue cited only mainnet.
    let cases: &[PublicCase] = &[
        PublicCase {
            id: "n-ethereum-mainnet",
            name: "mainnet",
            chain_id: "1",
        },
        PublicCase {
            id: "n-ethereum-goerli",
            name: "goerli",
            chain_id: "5",
        },
    ];

    for case in cases {
        let answer = call(&router, "GET", &format!("/networks/{}", case.id), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetNetwork 200 for a public network, got {} {:?}",
            answer.status, answer.body
        );
        let network = &answer.body["Network"];
        for (member, expected) in [
            ("Id", case.id.to_string()),
            ("Name", case.name.to_string()),
            ("Framework", "ETHEREUM".to_string()),
            // A standing public network is ready when it is first read, so the clock
            // owes it no transition.
            ("Status", "AVAILABLE".to_string()),
            (
                // Nothing minted these for a request, so their ARNs name the service's
                // default Region (`state::DEFAULT_REGION`).
                "Arn",
                format!(
                    "arn:aws:managedblockchain:{}:{ACCOUNT}:networks/{}",
                    state::DEFAULT_REGION, case.id
                ),
            ),
        ] {
            assert_eq!(
                network[member].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected Network.{member} {expected:?}, got {network:?}"
            );
        }
        assert_eq!(
            network["FrameworkAttributes"]["Ethereum"]["ChainId"].as_str(),
            Some(case.chain_id),
            "case {case:?}: expected the published chain id, got {network:?}"
        );
        assert!(
            network["FrameworkAttributes"]["Fabric"].is_null(),
            "case {case:?}: an Ethereum network carries no Fabric attributes, got \
             {network:?}"
        );
        assert!(
            is_date_time(&network["CreationDate"]),
            "case {case:?}: expected an ISO-8601 CreationDate, got {network:?}"
        );

        // Reading it twice answers the same record: nothing about it is minted per call.
        let again = call(&router, "GET", &format!("/networks/{}", case.id), None).await;
        assert_eq!(
            again.body["Network"], answer.body["Network"],
            "case {case:?}: a public network must read back identically, got {:?}",
            again.body
        );
    }

    // They are not account resources, so `ListNetworks` — "the networks in which the
    // current Amazon Web Services account participates" — reports none of them yet, and
    // reading them did not change that.
    let listing = call(&router, "GET", "/networks?maxResults=10", None).await;
    assert_eq!(
        listing.body["Networks"].as_array().map_or(0, Vec::len),
        0,
        "expected an account with nothing created to participate in no network, got {:?}",
        listing.body
    );

    // An id nothing holds is still the modelled 404: the public ids resolve, not every id.
    let answer = call(&router, "GET", "/networks/n-ethereum-nosuchnetwork", None).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case an unknown public-looking id: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );

    // The record is real state, not a fixed answer: a node created on the public network
    // is stored on it, which is what makes the account participate in it, and the listing
    // of its nodes needs no member scope because an Ethereum network has no members.
    let answer = call(
        &router,
        "POST",
        "/networks/n-ethereum-mainnet/nodes",
        Some(json!({
            "ClientRequestToken": "public-node-1",
            "NodeConfiguration": { "InstanceType": "bc.t3.large" }
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case CreateNode on a public network: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);
    assert!(
        is_resource_id(&node_id, "nd"),
        "case CreateNode on a public network: expected a minted node id, got {node_id:?}"
    );

    // A node reports the attributes of ITS network's framework: an Ethereum node is
    // reached on the JSON-RPC endpoints `NodeEthereumAttributes` names, not on the peer
    // endpoints of a Hyperledger Fabric node, and it has no state database
    // (`NodeConfiguration$StateDB` "Applies only to Hyperledger Fabric").
    let answer = call(
        &router,
        "GET",
        &format!("/networks/n-ethereum-mainnet/nodes/{node_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case GetNode on a public network: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let node = &answer.body["Node"];
    assert_eq!(
        (
            node["FrameworkAttributes"]["Ethereum"]["HttpEndpoint"].as_str(),
            node["FrameworkAttributes"]["Ethereum"]["WebSocketEndpoint"].as_str()
        ),
        (
            Some(
                format!("https://{node_id}.ethereum.managedblockchain.{REGION}.amazonaws.com")
                    .as_str()
            ),
            Some(
                format!("wss://{node_id}.wss.ethereum.managedblockchain.{REGION}.amazonaws.com")
                    .as_str()
            )
        ),
        "case GetNode on a public network: expected the node's JSON-RPC endpoints, got {node:?}"
    );
    assert!(
        node["FrameworkAttributes"]["Fabric"].is_null() && node["StateDB"].is_null(),
        "case GetNode on a public network: an Ethereum node carries no Fabric attributes \
         and no StateDB, got {node:?}"
    );
    assert!(
        node["MemberId"].is_null(),
        "case GetNode on a public network: an Ethereum network has no members, got {node:?}"
    );

    // And a `StateDB` the framework has no state database for is refused rather than
    // silently dropped.
    let answer = call(
        &router,
        "POST",
        "/networks/n-ethereum-mainnet/nodes",
        Some(json!({
            "ClientRequestToken": "public-node-statedb",
            "NodeConfiguration": { "InstanceType": "bc.t3.large", "StateDB": "CouchDB" }
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case CreateNode with a StateDB on an Ethereum network: expected 400 \
         InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );

    let answer = call(&router, "GET", "/networks/n-ethereum-mainnet/nodes", None).await;
    assert_eq!(
        answer.status, 200,
        "case ListNodes on a public network: expected 200 without a member scope, got {} {:?}",
        answer.status, answer.body
    );
    let ids: Vec<&str> = answer.body["Nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["Id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![node_id.as_str()],
        "case ListNodes on a public network: expected the created node, got {:?}",
        answer.body
    );

    let listing = call(&router, "GET", "/networks?maxResults=10", None).await;
    let listed: Vec<&str> = listing.body["Networks"]
        .as_array()
        .map(|networks| {
            networks
                .iter()
                .filter_map(|network| network["Id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec!["n-ethereum-mainnet"],
        "expected the account to participate in the network it put a node on, got {:?}",
        listing.body
    );
    assert_eq!(
        member_count(&engine, "n-ethereum-mainnet"),
        0,
        "an Ethereum network has no members"
    );
}

/// `ListNodes` models no `ResourceNotFoundException`, so every scope it cannot resolve —
/// a missing member, a member of a network nothing holds, or an absent `MemberId` where
/// the framework requires one — is a 400 `InvalidRequestException` whose message is built
/// from the request, never a 404 and never an empty page.
#[tokio::test]
async fn list_nodes_refuses_an_unresolvable_member_scope_as_an_invalid_request() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "scope-rules-1", "scope-rules").await;

    let unknown_network = format!("n-{}", "0".repeat(30));
    let unknown_member = format!("m-{}", "0".repeat(30));

    // Rows: each way the scope fails to resolve, over a network that exists and one that
    // does not, with the member named and omitted. The last row is the one scope that
    // does resolve.
    let cases: Vec<(&str, String, Vec<String>, bool)> = vec![
        (
            "unknown network, unknown member",
            format!("/networks/{unknown_network}/nodes?memberId={unknown_member}"),
            vec![unknown_network.clone(), unknown_member.clone()],
            false,
        ),
        (
            "unknown network, member omitted",
            format!("/networks/{unknown_network}/nodes"),
            vec![unknown_network.clone()],
            false,
        ),
        (
            "known network, unknown member",
            format!("/networks/{network_id}/nodes?memberId={unknown_member}"),
            vec![network_id.clone(), unknown_member.clone()],
            false,
        ),
        (
            "known network, member omitted",
            format!("/networks/{network_id}/nodes"),
            vec![network_id.clone()],
            false,
        ),
        (
            "known network, known member",
            format!("/networks/{network_id}/nodes?memberId={member_id}"),
            Vec::new(),
            true,
        ),
    ];

    for (case, path, named, resolves) in cases {
        let answer = call(&router, "GET", &path, None).await;
        if resolves {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected ListNodes 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["Nodes"].as_array().map_or(0, Vec::len),
                0,
                "case {case}: expected an empty page for a member with no nodes, got {:?}",
                answer.body
            );
            continue;
        }
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case {case}: expected 400 InvalidRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_ne!(
            answer.error_type, "ResourceNotFoundException",
            "case {case}: ListNodes models no ResourceNotFoundException, got {:?}",
            answer.body
        );
        let message = answer.text(&["Message"]);
        for id in named {
            assert!(
                message.contains(&id),
                "case {case}: expected the message to name {id:?}, got {message:?}"
            );
        }
    }
}

/// `ListProposals` answers the modelled 404 for a `NetworkId` nothing holds, and pages
/// the proposals stored on a network it does hold.
#[tokio::test]
async fn list_proposals_pages_a_networks_proposals_and_404s_an_unknown_network() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "proposals-1", "proposal-net").await;

    // A network nothing holds is the modelled 404, not an empty page.
    let unknown_network = format!("n-{}", "0".repeat(30));
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{unknown_network}/proposals"),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case an unknown network: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert!(
        answer.text(&["Message"]).contains(&unknown_network),
        "case an unknown network: expected the message to name the network, got {:?}",
        answer.body
    );

    // A network that exists and carries no proposals is an empty page, not a 404.
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/proposals"),
        None,
    )
    .await;
    assert_eq!(
        (
            answer.status,
            answer.body["Proposals"].as_array().map_or(usize::MAX, Vec::len)
        ),
        (200, 0),
        "case a network with no proposals: expected an empty page, got {} {:?}",
        answer.status,
        answer.body
    );

    // Seven proposals, so a page size at the published maximum (100) exceeds the item
    // count while the smaller sizes do not. They are made through `CreateProposal`, so the
    // listing pages exactly what that call stored.
    let expected_ids = create_proposals(&router, &network_id, &member_id, 7).await;

    // Page sizes from one to past the item count, including the published `MaxResults`
    // maximum (`ProposalListMaxResults` is `@range(1, 100)`).
    for page_size in [1_usize, 2, 3, 7, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!(
                    "/networks/{network_id}/proposals?maxResults={page_size}&nextToken={token}"
                ),
                None => format!("/networks/{network_id}/proposals?maxResults={page_size}"),
            };
            let answer = call(&router, "GET", &query, None).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected ListProposals 200, got {} {:?}",
                answer.status, answer.body
            );
            let items: Vec<Value> = answer.body["Proposals"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page holds at most MaxResults items, got {}",
                items.len()
            );
            for summary in &items {
                let id = summary["ProposalId"].as_str().unwrap_or_default();
                assert_eq!(
                    summary["Status"].as_str(),
                    Some("IN_PROGRESS"),
                    "case proposal {id}: expected the stored status, got {summary:?}"
                );
                assert_eq!(
                    summary["ProposedByMemberId"].as_str(),
                    Some(member_id.as_str()),
                    "case proposal {id}: expected the proposing member, got {summary:?}"
                );
                assert_eq!(
                    summary["Arn"].as_str(),
                    Some(
                        format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:proposals/{id}")
                            .as_str()
                    ),
                    "case proposal {id}: expected the proposal's ARN, got {summary:?}"
                );
                assert!(
                    is_date_time(&summary["CreationDate"])
                        && is_date_time(&summary["ExpirationDate"]),
                    "case proposal {id}: expected ISO-8601 dates, got {summary:?}"
                );
            }
            collected.extend(
                items
                    .iter()
                    .map(|item| item["ProposalId"].as_str().unwrap_or_default().to_string()),
            );
            token = answer.body["NextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected_ids,
            "case page size {page_size}: paging must walk every proposal exactly once, got \
             {collected:?}"
        );
    }

    // One page past each published bound is refused by the modelled range.
    for (case, page_size) in [("below", 0), ("above", 101)] {
        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/proposals?maxResults={page_size}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {case} the range: expected 400, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A token this service never issued cannot resume a page.
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/proposals?nextToken=not-a-token"),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case a forged nextToken: expected 400 InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );

    // Every proposal the create stored is reported, and only those: the page is cut from
    // the network's own records.
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/proposals?maxResults=100"),
        None,
    )
    .await;
    let mut reported: Vec<String> = answer.body["Proposals"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["ProposalId"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    assert_eq!(
        reported, expected_ids,
        "expected the listing to report exactly the created proposals, got {:?}",
        answer.body
    );
}

/// One row of the join table: the member configuration a `CreateMember` carries.
#[derive(Debug)]
struct JoinCase {
    member_name: &'static str,
    description: Option<&'static str>,
    tag_count: usize,
    kms_key: bool,
}

/// `CreateMember` creates the member the configuration describes under the invitation it
/// names, and `GetMember` answers from that record.
#[tokio::test]
async fn create_member_joins_a_network_by_invitation() {
    let (router, engine) = router_and_engine();
    let (network_id, _) = available_network(&router, "join-1", "join-net").await;

    // Rows differ along every member of `MemberConfiguration` the read back projects.
    let cases: &[JoinCase] = &[
        JoinCase {
            member_name: "org-two",
            description: None,
            tag_count: 0,
            kms_key: false,
        },
        JoinCase {
            member_name: "org-three",
            description: Some("a described member"),
            tag_count: 2,
            kms_key: true,
        },
        JoinCase {
            member_name: "org-four",
            description: Some(""),
            tag_count: 1,
            kms_key: false,
        },
    ];

    let key_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    let mut joined: Vec<String> = Vec::new();
    let mut invitations: Vec<String> = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let invitation_id = invite(&engine, &network_id, ACCOUNT, SystemTime::now());
        invitations.push(invitation_id.clone());

        let tags: serde_json::Map<String, Value> = (0..case.tag_count)
            .map(|t| (format!("key{t}"), Value::String(format!("value{t}"))))
            .collect();
        let mut configuration = member_configuration(case.member_name);
        configuration["Tags"] = Value::Object(tags);
        if let Some(description) = case.description {
            configuration["Description"] = Value::String(description.to_string());
        }
        if case.kms_key {
            configuration["KmsKeyArn"] = Value::String(key_arn.clone());
        }
        let body = json!({
            "ClientRequestToken": format!("join-token-{i}"),
            "InvitationId": invitation_id,
            "MemberConfiguration": configuration,
        });

        let answer = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/members"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateMember 200, got {} {:?}",
            answer.status, answer.body
        );
        let member_id = answer.text(&["MemberId"]);
        assert!(
            is_resource_id(&member_id, "m"),
            "case {case:?}: expected a minted member id, got {member_id:?}"
        );
        assert!(
            !joined.contains(&member_id),
            "case {case:?}: every join mints a fresh id, got the repeat {member_id:?}"
        );
        joined.push(member_id.clone());

        // The invitation is now being accepted, and the clock owes it ACCEPTED once the
        // member it created has finished creating. (`ListInvitations` is still a scaffold
        // stub — its own issue — so the status is read from the store.)
        assert_eq!(
            invitation_status(&engine, &invitation_id),
            "ACCEPTING",
            "case {case:?}: expected the invitation to be ACCEPTING"
        );

        // The member reads back as a resource of its own.
        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members/{member_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetMember 200, got {} {:?}",
            answer.status, answer.body
        );
        let member = &answer.body["Member"];
        for (field, expected) in [
            ("Id", member_id.clone()),
            ("NetworkId", network_id.clone()),
            ("Name", case.member_name.to_string()),
            (
                "Arn",
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:members/{member_id}"),
            ),
            (
                "KmsKeyArn",
                if case.kms_key {
                    key_arn.clone()
                } else {
                    // `MemberConfiguration$KmsKeyArn` undefined means "use an KMS key
                    // that is owned and managed by Amazon Web Services on your behalf".
                    "AWS Owned KMS Key".to_string()
                },
            ),
            // The join has only just returned, so the member is still in the non-terminal
            // status the service reports it in.
            ("Status", "CREATING".to_string()),
        ] {
            assert_eq!(
                member[field].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected Member.{field} {expected:?}, got {member:?}"
            );
        }
        assert_eq!(
            member["Description"].as_str(),
            case.description,
            "case {case:?}: expected Member.Description {:?}, got {member:?}",
            case.description
        );
        assert_eq!(
            member["Tags"].as_object().map(serde_json::Map::len),
            Some(case.tag_count),
            "case {case:?}: expected {} tags back, got {member:?}",
            case.tag_count
        );
        assert_eq!(
            member["FrameworkAttributes"]["Fabric"]["CaEndpoint"].as_str(),
            Some(
                format!(
                    "ca.{member_id}.{network_id}.managedblockchain.{REGION}.amazonaws.com:30002"
                )
                .as_str()
            ),
            "case {case:?}: expected the member's CA endpoint, got {member:?}"
        );
        // The admin password the request carried is never reported back.
        assert_eq!(
            member["FrameworkAttributes"]["Fabric"]["AdminUsername"].as_str(),
            Some("AdminUser"),
            "case {case:?}: expected the admin username back, got {member:?}"
        );
        assert!(
            !serde_json::to_string(member)
                .unwrap_or_default()
                .contains("MyPassword1"),
            "case {case:?}: the admin password must never be reported, got {member:?}"
        );

        // The same token replays the id its first call minted rather than joining twice.
        let again = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/members"),
            Some(body),
        )
        .await;
        assert_eq!(
            again.text(&["MemberId"]),
            member_id,
            "case {case:?}: a repeated ClientRequestToken must replay the same id, got {:?}",
            again.body
        );
    }

    // The first member came from `CreateNetwork`; each join added exactly one more.
    assert_eq!(
        member_count(&engine, &network_id),
        1 + cases.len(),
        "expected one member per join beside the network's first"
    );

    // Once the member has finished creating, the clock owes the invitation ACCEPTED and
    // the member AVAILABLE.
    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/members/{}", joined[0]),
        None,
    )
    .await;
    assert_eq!(
        answer.body["Member"]["Status"].as_str(),
        Some("AVAILABLE"),
        "expected the joined member to settle to AVAILABLE, got {:?}",
        answer.body
    );
    let invitation_id = invitations[0].clone();
    assert_eq!(
        invitation_status(&engine, &invitation_id),
        "ACCEPTED",
        "expected the invitation to settle to ACCEPTED"
    );
}

/// `CreateMember` refuses what it cannot resolve, and creates nothing when it does.
#[tokio::test]
async fn create_member_refuses_an_unresolvable_network_or_invitation() {
    let (router, engine) = router_and_engine();
    let (network_id, _) = available_network(&router, "refuse-1", "refuse-net").await;
    let (other_network_id, _) = available_network(&router, "refuse-2", "other-net").await;

    let unknown_network = format!("n-{}", "0".repeat(30));
    let unknown_invitation = format!("i-{}", "0".repeat(30));
    let now = SystemTime::now();
    let pending = invite(&engine, &network_id, ACCOUNT, now);
    let for_other_network = invite(&engine, &other_network_id, ACCOUNT, now);
    let for_other_account = invite(&engine, &network_id, OTHER_ACCOUNT, now);
    // Recorded far enough in the past that its `ExpirationDate` (the create's
    // `ProposalDurationInHours`) has passed, so the clock owes it EXPIRED and "the invitee
    // can no longer create a member and join the network using this InvitationId".
    let expired = invite(
        &engine,
        &network_id,
        ACCOUNT,
        now - Duration::from_secs(48 * 3_600),
    );

    // Rows: each way the request fails to resolve, plus the id it must name in the
    // message.
    let cases: Vec<(&str, String, String, u16, &str, String)> = vec![
        (
            "unknown network and unknown invitation",
            unknown_network.clone(),
            unknown_invitation.clone(),
            404,
            "ResourceNotFoundException",
            unknown_network.clone(),
        ),
        (
            "known network, unknown invitation",
            network_id.clone(),
            unknown_invitation.clone(),
            404,
            "ResourceNotFoundException",
            unknown_invitation.clone(),
        ),
        (
            "invitation to another network",
            network_id.clone(),
            for_other_network.clone(),
            404,
            "ResourceNotFoundException",
            for_other_network.clone(),
        ),
        (
            "invitation to another account",
            network_id.clone(),
            for_other_account.clone(),
            404,
            "ResourceNotFoundException",
            for_other_account.clone(),
        ),
        (
            "expired invitation",
            network_id.clone(),
            expired.clone(),
            400,
            "InvalidRequestException",
            expired.clone(),
        ),
    ];

    for (case, network, invitation, status, code, named) in cases {
        let answer = call(
            &router,
            "POST",
            &format!("/networks/{network}/members"),
            Some(json!({
                "ClientRequestToken": format!("refuse-{case}"),
                "InvitationId": invitation,
                "MemberConfiguration": member_configuration("org-refused"),
            })),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (status, code),
            "case {case}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.text(&["Message"]).contains(&named),
            "case {case}: expected the message to name {named:?}, got {:?}",
            answer.body
        );
        assert!(
            answer.body["MemberId"].is_null(),
            "case {case}: a refusal mints no member, got {:?}",
            answer.body
        );
        // The resource the request named by id was not found, so there is no ARN to
        // report: `ResourceNotFoundException$ResourceName` stays absent rather than
        // carrying the ARN of some other resource the request mentioned.
        assert_eq!(
            answer.body.get("ResourceName"),
            None,
            "case {case}: expected no ResourceName member, got {:?}",
            answer.body
        );
    }

    // Nothing was created by any refusal: the unknown network is still unknown, and the
    // networks that exist still hold only the member their create minted.
    let answer = call(&router, "GET", &format!("/networks/{unknown_network}"), None).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "expected the unknown network to stay unknown, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        (
            member_count(&engine, &network_id),
            member_count(&engine, &other_network_id)
        ),
        (1, 1),
        "expected the refusals to have created no member"
    );

    // A member name the network already holds is a conflict, not a second member.
    let joined = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/members"),
        Some(json!({
            "ClientRequestToken": "conflict-first",
            "InvitationId": pending,
            "MemberConfiguration": member_configuration("org-unique"),
        })),
    )
    .await;
    assert_eq!(
        joined.status, 200,
        "expected the pending invitation to be usable, got {} {:?}",
        joined.status, joined.body
    );
    let second = invite(&engine, &network_id, ACCOUNT, SystemTime::now());
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/members"),
        Some(json!({
            "ClientRequestToken": "conflict-second",
            "InvitationId": second,
            "MemberConfiguration": member_configuration("org-unique"),
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (409, "ResourceAlreadyExistsException"),
        "case a name the network already holds: expected 409 ResourceAlreadyExistsException, \
         got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        member_count(&engine, &network_id),
        2,
        "expected the conflict to have created no member"
    );

    // A network still CREATING cannot take a member yet.
    let (creating_network, _) = create_network(
        &router,
        create_network_body("refuse-creating", "creating-net"),
    )
    .await;
    let not_ready = invite(&engine, &creating_network, ACCOUNT, SystemTime::now());
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{creating_network}/members"),
        Some(json!({
            "ClientRequestToken": "refuse-not-ready",
            "InvitationId": not_ready,
            "MemberConfiguration": member_configuration("org-early"),
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (409, "ResourceNotReadyException"),
        "case a network still CREATING: expected 409 ResourceNotReadyException, got {} {:?}",
        answer.status,
        answer.body
    );
}

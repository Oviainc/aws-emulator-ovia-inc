// SPDX-License-Identifier: CC0-1.0

//! Managed Blockchain is a REGIONAL service: every resource belongs to the Region the call
//! that created it was signed for, and a request addressed to any other Region resolves
//! none of it — an id-addressed read, create-on, update or delete answers the modelled
//! `ResourceNotFoundException`, a listing scoped by a resource of another Region reports
//! that scope as unresolvable (`ListNodes` and `ListProposalVotes` name the scope they
//! reach the reported resource through, as an invalid request), and a listing of this
//! Region's own reports none of it.
//!
//! The tables span the classes that rule generalises over:
//!
//! * the REGION: four rows of different AWS Region shapes, each the home of its own
//!   resources and a foreign Region for every other row's, so the isolation is asserted
//!   both ways round rather than from one privileged Region;
//! * the RESOURCE KIND: network, member, node, proposal, accessor and invitation, plus the
//!   tag plane that addresses all of them by ARN;
//! * the CALL KIND: id-addressed read, listing, create-on, update, delete and vote.
//!
//! AWS's standing public networks are the one carve-out — no request created them and they
//! belong to no account, so every Region reads the same ones — and that is pinned here
//! too, together with the fact that a node created ON one is an account resource again and
//! is reached only through the Region it was created in.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state;

use crate::harness::{
    call_in_region, create_network_body, router_and_engine, ACCOUNT, PAST_TRANSITION,
};
use crate::resource_tags::label;

/// The Regions the tables run over.
///
/// A handler learns which Region a request addresses only from that request's SigV4
/// credential scope, so any Region id is a row; these are AWS Region ids of different
/// shapes — the Region this service falls back on when a request carries no scope at all
/// (`state::DEFAULT_REGION`), a European one, an Asia-Pacific one whose geography code is
/// two words, and a GovCloud one, which names its partition in the id.
const REGIONS: &[&str] = &[
    state::DEFAULT_REGION,
    "eu-west-1",
    "ap-southeast-2",
    "us-gov-west-1",
];

/// The `AccessorType` every accessor in these tables is created with: `AccessorType`'s
/// only value (smithy_query: shape AccessorType = BILLING_TOKEN).
const ACCESSOR_TYPE: &str = "BILLING_TOKEN";

/// One `bc.*` node size a Hyperledger Fabric `STANDARD` network runs, as the sibling
/// suites create nodes with.
const INSTANCE_TYPE: &str = "bc.t3.small";

/// The whole set of resources one Region holds, as the creates signed for it minted them.
#[derive(Debug)]
struct Held {
    region: &'static str,
    network_id: String,
    member_id: String,
    node_id: String,
    proposal_id: String,
    accessor_id: String,
    invitation_id: String,
}

/// The ARN a resource of `region` carries:
/// `arn:aws:managedblockchain:<region>:<account>:<type>/<id>`, as the create that minted it
/// stored it.
fn arn_in(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:managedblockchain:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// Record one invitation onto `network_id` in `region`, through the same state API an
/// approved `InviteAction` goes through, and answer the id the service minted.
///
/// The sibling suites have the same helper pinned to their one Region; this one takes the
/// Region, which is the whole point of these tables. `VoteOnProposal` is what mints an
/// invitation on the wire, and it is exercised below — the invitation addressed by the
/// tables is recorded here so each Region's set is built the same way.
fn invite(engine: &engine::InMemoryEngine, region: &str, network_id: &str) -> String {
    state::service_state(engine)
        .expect("this service's own store")
        .lock()
        .record_invitation(network_id, ACCOUNT, region, SystemTime::now())
        .expect("the network exists in this region and the store has room")
}

/// One call addressed at the resources `Held` names: how a case names it, the request, and
/// the refusal every Region but their own must answer it with.
#[derive(Debug)]
struct Case {
    name: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The status and `x-amzn-errortype` a foreign Region answers.
    refusal: (u16, &'static str),
}

/// The modelled refusal for a resource a Region does not hold: the same answer as an id
/// that was never created.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// `ListNodes` and `ListProposalVotes` name the scope they reach the reported resource
/// through — the member that owns the nodes, the proposal whose votes are listed — and
/// report it as an invalid request, which is what a `NetworkId` of another Region makes it.
const INVALID_REQUEST: (u16, &str) = (400, "InvalidRequestException");

/// The id-addressed READS and listings of one Region's resources: safe to send to any
/// Region, because a read changes nothing wherever it lands.
fn read_cases(held: &Held) -> Vec<Case> {
    let Held {
        network_id,
        member_id,
        node_id,
        proposal_id,
        accessor_id,
        ..
    } = held;
    vec![
        Case {
            name: "GetNetwork".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "GetMember".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/members/{member_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "GetNode".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "GetProposal".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/proposals/{proposal_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "GetAccessor".to_string(),
            method: "GET",
            path: format!("/accessors/{accessor_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "ListMembers".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/members"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "ListNodes".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/nodes?memberId={member_id}"),
            body: None,
            refusal: INVALID_REQUEST,
        },
        Case {
            name: "ListProposals".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/proposals"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "ListProposalVotes".to_string(),
            method: "GET",
            path: format!("/networks/{network_id}/proposals/{proposal_id}/votes"),
            body: None,
            refusal: INVALID_REQUEST,
        },
        Case {
            name: "ListTagsForResource".to_string(),
            method: "GET",
            path: format!(
                "/tags/{}",
                label(&arn_in(held.region, "networks", network_id))
            ),
            body: None,
            refusal: NOT_FOUND,
        },
    ]
}

/// The MUTATIONS and creates-on addressed at one Region's resources: sent only to foreign
/// Regions, where each must refuse and leave the record exactly as it was.
fn mutation_cases(held: &Held) -> Vec<Case> {
    let Held {
        network_id,
        member_id,
        node_id,
        proposal_id,
        accessor_id,
        invitation_id,
        ..
    } = held;
    vec![
        Case {
            name: "UpdateNode".to_string(),
            method: "PATCH",
            path: format!("/networks/{network_id}/nodes/{node_id}"),
            body: Some(json!({
                "MemberId": member_id,
                "LogPublishingConfiguration": {
                    "Fabric": { "PeerLogs": { "Cloudwatch": { "Enabled": true } } }
                }
            })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "UpdateMember".to_string(),
            method: "PATCH",
            path: format!("/networks/{network_id}/members/{member_id}"),
            body: Some(json!({
                "LogPublishingConfiguration": {
                    "Fabric": { "CaLogs": { "Cloudwatch": { "Enabled": true } } }
                }
            })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "DeleteNode".to_string(),
            method: "DELETE",
            path: format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "DeleteMember".to_string(),
            method: "DELETE",
            path: format!("/networks/{network_id}/members/{member_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "DeleteAccessor".to_string(),
            method: "DELETE",
            path: format!("/accessors/{accessor_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "CreateNode".to_string(),
            method: "POST",
            path: format!("/networks/{network_id}/nodes"),
            body: Some(json!({
                "ClientRequestToken": format!("foreign-node-{network_id}"),
                "MemberId": member_id,
                "NodeConfiguration": { "InstanceType": INSTANCE_TYPE },
            })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "CreateProposal".to_string(),
            method: "POST",
            path: format!("/networks/{network_id}/proposals"),
            body: Some(json!({
                "ClientRequestToken": format!("foreign-proposal-{network_id}"),
                "MemberId": member_id,
                "Actions": { "Invitations": [{ "Principal": "100000000001" }] },
            })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "CreateMember".to_string(),
            method: "POST",
            path: format!("/networks/{network_id}/members"),
            body: Some(json!({
                "ClientRequestToken": format!("foreign-member-{network_id}"),
                "InvitationId": invitation_id,
                "MemberConfiguration": {
                    "Name": "foreignjoin",
                    "FrameworkConfiguration": {
                        "Fabric": { "AdminUsername": "AdminUser", "AdminPassword": "MyPassword1" }
                    }
                },
            })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "VoteOnProposal".to_string(),
            method: "POST",
            path: format!("/networks/{network_id}/proposals/{proposal_id}/votes"),
            body: Some(json!({ "VoterMemberId": member_id, "Vote": "YES" })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "RejectInvitation".to_string(),
            method: "DELETE",
            path: format!("/invitations/{invitation_id}"),
            body: None,
            refusal: NOT_FOUND,
        },
        Case {
            name: "TagResource".to_string(),
            method: "POST",
            path: format!(
                "/tags/{}",
                label(&arn_in(held.region, "networks", network_id))
            ),
            body: Some(json!({ "Tags": { "written-from": "another-region" } })),
            refusal: NOT_FOUND,
        },
        Case {
            name: "UntagResource".to_string(),
            method: "DELETE",
            path: format!(
                "/tags/{}?tagKeys=home",
                label(&arn_in(held.region, "networks", network_id))
            ),
            body: None,
            refusal: NOT_FOUND,
        },
    ]
}

/// Create one network (with its first member), one node, one proposal, one accessor and one
/// invitation, all through `region`'s endpoint.
async fn hold_resources(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &'static str,
) -> Held {
    let mut body = create_network_body(&format!("network-token-{region}"), &format!("net{region}"));
    body["Tags"] = json!({ "home": region });
    let answer = call_in_region(router, region, "POST", "/networks", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case {region}: expected CreateNetwork 200, got {} {:?}",
        answer.status, answer.body
    );
    let network_id = answer.text(&["NetworkId"]);
    let member_id = answer.text(&["MemberId"]);

    // A node and a proposal can only be created on a network that has settled to
    // `AVAILABLE`.
    tokio::time::sleep(PAST_TRANSITION).await;

    let answer = call_in_region(
        router,
        region,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(json!({
            "ClientRequestToken": format!("node-token-{region}"),
            "MemberId": member_id,
            "NodeConfiguration": { "InstanceType": INSTANCE_TYPE },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {region}: expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);

    let answer = call_in_region(
        router,
        region,
        "POST",
        &format!("/networks/{network_id}/proposals"),
        Some(json!({
            "ClientRequestToken": format!("proposal-token-{region}"),
            "MemberId": member_id,
            "Actions": { "Invitations": [{ "Principal": "100000000001" }] },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {region}: expected CreateProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    let proposal_id = answer.text(&["ProposalId"]);

    let answer = call_in_region(
        router,
        region,
        "POST",
        "/accessors",
        Some(json!({
            "ClientRequestToken": format!("accessor-token-{region}"),
            "AccessorType": ACCESSOR_TYPE,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {region}: expected CreateAccessor 200, got {} {:?}",
        answer.status, answer.body
    );
    let accessor_id = answer.text(&["AccessorId"]);

    let invitation_id = invite(engine, region, &network_id);

    Held {
        region,
        network_id,
        member_id,
        node_id,
        proposal_id,
        accessor_id,
        invitation_id,
    }
}

/// The ids one listing reported, under `member`.
fn listed_ids(body: &Value, member: &str, id_member: &str) -> Vec<String> {
    body[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item[id_member].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every id-addressed read of a resource resolves through the Region its create was signed
/// for and through no other, and every mutation addressed at another Region's resource is
/// refused and writes nothing — over the whole table of Regions, resource kinds and calls.
#[tokio::test]
async fn a_resource_is_reached_only_through_the_region_it_was_created_in() {
    let (router, engine) = router_and_engine();

    // One store, one set of resources per Region row: every row's resources are a foreign
    // Region's for every other row, so the isolation is asserted in both directions and
    // between co-resident records rather than between separate stores.
    let mut held = Vec::new();
    for region in REGIONS {
        held.push(hold_resources(&router, &engine, region).await);
    }

    for owner in &held {
        // Its own Region reads every one of its resources.
        for case in read_cases(owner) {
            let answer =
                call_in_region(&router, owner.region, case.method, &case.path, case.body).await;
            assert_eq!(
                answer.status, 200,
                "case {} in its own region {}: expected 200, got {} {:?}",
                case.name, owner.region, answer.status, answer.body
            );
        }

        // No other Region reads or mutates any of them.
        for region in REGIONS.iter().filter(|region| **region != owner.region) {
            for case in read_cases(owner).into_iter().chain(mutation_cases(owner)) {
                let answer =
                    call_in_region(&router, region, case.method, &case.path, case.body).await;
                assert_eq!(
                    (answer.status, answer.error_type.as_str()),
                    (case.refusal.0, case.refusal.1),
                    "case {} on {}'s resources from {region}: expected {:?}, got {} {} {:?}",
                    case.name,
                    owner.region,
                    case.refusal,
                    answer.status,
                    answer.error_type,
                    answer.body
                );
            }
        }
    }

    // Nothing the foreign calls asked for was carried out: every resource still reads back
    // in its own Region, with the record its create left.
    for owner in &held {
        for case in read_cases(owner) {
            let answer =
                call_in_region(&router, owner.region, case.method, &case.path, case.body).await;
            assert_eq!(
                answer.status, 200,
                "case {} in {} after the foreign calls: expected 200, got {} {:?}",
                case.name, owner.region, answer.status, answer.body
            );
        }

        let answer = call_in_region(
            &router,
            owner.region,
            "GET",
            &format!("/accessors/{}", owner.accessor_id),
            None,
        )
        .await;
        assert_eq!(
            answer.body["Accessor"]["Status"].as_str(),
            Some("AVAILABLE"),
            "case {}: a foreign DeleteAccessor must not have started a deletion, got {:?}",
            owner.region,
            answer.body
        );

        let answer = call_in_region(
            &router,
            owner.region,
            "GET",
            &format!(
                "/networks/{}/nodes/{}?memberId={}",
                owner.network_id, owner.node_id, owner.member_id
            ),
            None,
        )
        .await;
        assert!(
            answer.body["Node"]["LogPublishingConfiguration"].is_null(),
            "case {}: a foreign UpdateNode must not have written a log configuration, got {:?}",
            owner.region,
            answer.body
        );

        let answer = call_in_region(
            &router,
            owner.region,
            "GET",
            &format!("/tags/{}", label(&arn_in(owner.region, "networks", &owner.network_id))),
            None,
        )
        .await;
        assert_eq!(
            answer.body["Tags"],
            json!({ "home": owner.region }),
            "case {}: a foreign TagResource/UntagResource must not have changed the tag set, \
             got {:?}",
            owner.region,
            answer.body
        );

        let answer = call_in_region(
            &router,
            owner.region,
            "GET",
            &format!("/networks/{}/members", owner.network_id),
            None,
        )
        .await;
        assert_eq!(
            listed_ids(&answer.body, "Members", "Id"),
            vec![owner.member_id.clone()],
            "case {}: a foreign CreateMember/DeleteMember must have left the membership as it \
             was, got {:?}",
            owner.region,
            answer.body
        );
    }
}

/// Every ARN and endpoint a resource reports names the Region its create was signed for —
/// so a record is never reported as living somewhere the caller is not addressing.
#[tokio::test]
async fn a_resource_reports_the_region_it_was_created_in() {
    let (router, engine) = router_and_engine();

    for region in REGIONS {
        let owner = hold_resources(&router, &engine, region).await;

        for (case, path, shape, member, expected) in [
            (
                "Network.Arn",
                format!("/networks/{}", owner.network_id),
                "Network",
                "Arn",
                arn_in(region, "networks", &owner.network_id),
            ),
            (
                "Network.VpcEndpointServiceName",
                format!("/networks/{}", owner.network_id),
                "Network",
                "VpcEndpointServiceName",
                format!("com.amazonaws.{region}.managedblockchain.{}", owner.network_id),
            ),
            (
                "Member.Arn",
                format!("/networks/{}/members/{}", owner.network_id, owner.member_id),
                "Member",
                "Arn",
                arn_in(region, "members", &owner.member_id),
            ),
            (
                "Node.Arn",
                format!(
                    "/networks/{}/nodes/{}?memberId={}",
                    owner.network_id, owner.node_id, owner.member_id
                ),
                "Node",
                "Arn",
                arn_in(region, "nodes", &owner.node_id),
            ),
            (
                "Proposal.Arn",
                format!(
                    "/networks/{}/proposals/{}",
                    owner.network_id, owner.proposal_id
                ),
                "Proposal",
                "Arn",
                arn_in(region, "proposals", &owner.proposal_id),
            ),
            (
                "Accessor.Arn",
                format!("/accessors/{}", owner.accessor_id),
                "Accessor",
                "Arn",
                arn_in(region, "accessors", &owner.accessor_id),
            ),
        ] {
            let answer = call_in_region(&router, region, "GET", &path, None).await;
            assert_eq!(
                answer.body[shape][member].as_str(),
                Some(expected.as_str()),
                "case {case} in {region}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// A listing reports the resources of the Region it is addressed to and no others.
#[tokio::test]
async fn a_listing_reports_only_the_region_it_is_addressed_to() {
    let (router, engine) = router_and_engine();

    let mut held = Vec::new();
    for region in REGIONS {
        held.push(hold_resources(&router, &engine, region).await);
    }

    for owner in &held {
        for (case, path, member, id_member, expected) in [
            (
                "ListNetworks",
                "/networks?maxResults=10".to_string(),
                "Networks",
                "Id",
                owner.network_id.clone(),
            ),
            (
                "ListAccessors",
                "/accessors?maxResults=50".to_string(),
                "Accessors",
                "Id",
                owner.accessor_id.clone(),
            ),
            (
                "ListInvitations",
                "/invitations?maxResults=100".to_string(),
                "Invitations",
                "InvitationId",
                owner.invitation_id.clone(),
            ),
            (
                "ListMembers",
                format!("/networks/{}/members", owner.network_id),
                "Members",
                "Id",
                owner.member_id.clone(),
            ),
            (
                "ListNodes",
                format!(
                    "/networks/{}/nodes?memberId={}",
                    owner.network_id, owner.member_id
                ),
                "Nodes",
                "Id",
                owner.node_id.clone(),
            ),
            (
                "ListProposals",
                format!("/networks/{}/proposals", owner.network_id),
                "Proposals",
                "ProposalId",
                owner.proposal_id.clone(),
            ),
        ] {
            let answer = call_in_region(&router, owner.region, "GET", &path, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case} in {}: expected 200, got {} {:?}",
                owner.region, answer.status, answer.body
            );
            assert_eq!(
                listed_ids(&answer.body, member, id_member),
                vec![expected.clone()],
                "case {case} in {}: expected exactly that Region's own {member}, got {:?}",
                owner.region,
                answer.body
            );
        }
    }
}

/// A `ClientRequestToken` is scoped to the Region it was used in: the same token through
/// two Regions' endpoints creates one resource in each, and neither Region replays the
/// other's ids — which it could only hand back as ids nothing it holds answers to.
#[tokio::test]
async fn an_idempotency_token_is_scoped_to_its_region() {
    let (router, _engine) = router_and_engine();

    let mut network_ids = Vec::new();
    let mut accessor_ids = Vec::new();
    for region in REGIONS {
        let answer = call_in_region(
            &router,
            region,
            "POST",
            "/networks",
            Some(create_network_body("one-token", "onetoken")),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {region}: expected CreateNetwork 200 for a token first used here, got {} {:?}",
            answer.status, answer.body
        );
        let network_id = answer.text(&["NetworkId"]);

        let answer = call_in_region(
            &router,
            region,
            "POST",
            "/accessors",
            Some(json!({
                "ClientRequestToken": "one-token",
                "AccessorType": ACCESSOR_TYPE,
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {region}: expected CreateAccessor 200 for a token first used here, got {} {:?}",
            answer.status, answer.body
        );
        let accessor_id = answer.text(&["AccessorId"]);

        assert!(
            !network_ids.contains(&network_id) && !accessor_ids.contains(&accessor_id),
            "case {region}: the same token in another Region must mint its own ids, got \
             {network_id:?} / {accessor_id:?} among {network_ids:?} / {accessor_ids:?}"
        );
        network_ids.push(network_id);
        accessor_ids.push(accessor_id);
    }

    // Replayed in its OWN Region, the token still answers from the resource it created
    // there: Region scoping narrows the key, it does not break idempotency.
    for (index, region) in REGIONS.iter().enumerate() {
        let answer = call_in_region(
            &router,
            region,
            "POST",
            "/accessors",
            Some(json!({
                "ClientRequestToken": "one-token",
                "AccessorType": ACCESSOR_TYPE,
            })),
        )
        .await;
        assert_eq!(
            answer.text(&["AccessorId"]),
            accessor_ids[index],
            "case {region}: a replayed token must answer from its own Region's accessor, got \
             {:?}",
            answer.body
        );
    }
}

/// AWS's standing public networks are the carve-out: no request created them and they
/// belong to no account, so every Region reads the same ones — while a node created ON one
/// is an account resource again, reached only through the Region it was created in, and the
/// account participates in the public network only in that Region.
#[tokio::test]
async fn the_standing_public_networks_are_read_in_every_region_but_their_nodes_are_not() {
    let (router, _engine) = router_and_engine();

    // Rows: the whole catalog this service seeds (`state::PUBLIC_NETWORKS`).
    for public in state::PUBLIC_NETWORKS {
        for region in REGIONS {
            let answer =
                call_in_region(&router, region, "GET", &format!("/networks/{}", public.id), None)
                    .await;
            assert_eq!(
                (answer.status, answer.body["Network"]["Id"].as_str()),
                (200, Some(public.id)),
                "case public network {} from {region}: expected it to resolve, got {} {:?}",
                public.id,
                answer.status,
                answer.body
            );
        }
    }

    // A node on the public network, created through ONE Region's endpoint.
    let home = REGIONS[1];
    let public_id = state::PUBLIC_NETWORKS[0].id;
    let answer = call_in_region(
        &router,
        home,
        "POST",
        &format!("/networks/{public_id}/nodes"),
        Some(json!({
            "ClientRequestToken": "public-node",
            // `NodeConfiguration$AvailabilityZone` is "Required for Ethereum nodes".
            "NodeConfiguration": {
                "InstanceType": INSTANCE_TYPE,
                "AvailabilityZone": format!("{home}a"),
            }
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case CreateNode on a public network: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);

    for region in REGIONS {
        let answer = call_in_region(
            &router,
            region,
            "GET",
            &format!("/networks/{public_id}/nodes/{node_id}"),
            None,
        )
        .await;
        let expected = if *region == home {
            (200, "")
        } else {
            NOT_FOUND
        };
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            expected,
            "case GetNode on a public network from {region}: expected {expected:?}, got {} {} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );

        let listing = call_in_region(
            &router,
            region,
            "GET",
            &format!("/networks/{public_id}/nodes"),
            None,
        )
        .await;
        let expected_nodes: Vec<String> = if *region == home {
            vec![node_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_ids(&listing.body, "Nodes", "Id"),
            expected_nodes,
            "case ListNodes on a public network from {region}: expected {expected_nodes:?}, got \
             {:?}",
            listing.body
        );

        // "the networks in which the current Amazon Web Services account participates":
        // the account participates in the public network only where it holds a node on it.
        let listing =
            call_in_region(&router, region, "GET", "/networks?maxResults=10", None).await;
        let expected_networks: Vec<String> = if *region == home {
            vec![public_id.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_ids(&listing.body, "Networks", "Id"),
            expected_networks,
            "case ListNetworks from {region}: expected {expected_networks:?}, got {:?}",
            listing.body
        );
    }
}

/// `VoteOnProposal` carries its approved `InviteAction` out in the Region the vote was cast
/// in: the invitation it mints is listed and rejectable there and nowhere else.
#[tokio::test]
async fn an_approved_invitation_is_recorded_in_the_region_the_vote_was_cast_in() {
    let (router, engine) = router_and_engine();

    let home = REGIONS[2];
    let owner = hold_resources(&router, &engine, home).await;

    // One member, and `ApprovalThresholdPolicy` 50% GREATER_THAN, so this one `YES` vote
    // decides the proposal and its `InviteAction` is carried out.
    let answer = call_in_region(
        &router,
        home,
        "POST",
        &format!(
            "/networks/{}/proposals/{}/votes",
            owner.network_id, owner.proposal_id
        ),
        Some(json!({ "VoterMemberId": owner.member_id, "Vote": "YES" })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case VoteOnProposal in {home}: expected 200, got {} {:?}",
        answer.status, answer.body
    );

    for region in REGIONS {
        let listing =
            call_in_region(&router, region, "GET", "/invitations?maxResults=100", None).await;
        let listed = listed_ids(&listing.body, "Invitations", "InvitationId");
        // The approved `InviteAction` named another account, so the only invitation this
        // account is reported is the one `hold_resources` recorded for it — in `home`.
        let expected = if *region == home {
            vec![owner.invitation_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed, expected,
            "case ListInvitations from {region}: expected {expected:?}, got {:?}",
            listing.body
        );
    }
}

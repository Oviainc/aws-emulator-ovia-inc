// SPDX-License-Identifier: CC0-1.0

//! `VoteOnProposal`, end to end over HTTP: a member's vote is recorded on the stored
//! proposal, and the network's own `ApprovalThresholdPolicy` decides the proposal from
//! the votes cast so far.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures. The proposal is read back through
//! `GetProposal`, and an approved `InviteAction` through `ListInvitations` and
//! `CreateMember`, so nothing is asserted against the store that a caller cannot see.
//!
//! The tables span the classes the rules generalise over: the membership a threshold is
//! measured against (one member up to the ten of the documented worked example), both
//! published `ThresholdPercentage` bounds (`@range(0, 100)`) and values in between, both
//! `ThresholdComparator` values and its omission, a policy that names no threshold at
//! all, `YES` and `NO` vote orders that approve, reject and leave a proposal open, both
//! action kinds an approved proposal carries out, and every way a vote is refused.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    member_arn, service_state, StoredMember, PROPOSAL_ACTION_FAILED, PROPOSAL_APPROVED,
    PROPOSAL_EXPIRED, PROPOSAL_IN_PROGRESS, PROPOSAL_REJECTED, STATUS_AVAILABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_network, create_network_body, router_and_engine, ACCOUNT, PAST_TRANSITION, REGION,
};
use crate::network_and_node_lifecycle::{create_node, Answer};

/// An id with the shape every Managed Blockchain resource id takes that no create ever
/// minted: used for the rows naming a network, proposal or member nothing stored holds.
const ABSENT_SUFFIX: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// An account that is not the one this emulator answers for.
const OTHER_ACCOUNT: &str = "210987654321";

/// A `CreateNetwork` body whose `ApprovalThresholdPolicy` names `percentage` and
/// `comparator` — the policy the proposals on that network are decided by. `comparator`
/// of `None` leaves `ThresholdComparator` out, and `percentage` of `None` leaves
/// `ThresholdPercentage` out, which fixes no threshold at all.
fn network_body(
    token: &str,
    name: &str,
    percentage: Option<i64>,
    comparator: Option<&str>,
) -> Value {
    let mut body = create_network_body(token, name);
    let mut policy = json!({ "ProposalDurationInHours": 24 });
    if let Some(percentage) = percentage {
        policy["ThresholdPercentage"] = json!(percentage);
    }
    if let Some(comparator) = comparator {
        policy["ThresholdComparator"] = json!(comparator);
    }
    body["VotingPolicy"] = json!({ "ApprovalThresholdPolicy": policy });
    body
}

/// Create one proposal on `network_id` as `member_id` and answer its `ProposalId`.
async fn create_proposal(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
    actions: Value,
) -> String {
    let body = json!({
        "ClientRequestToken": token,
        "MemberId": member_id,
        "Actions": actions,
    });
    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/proposals"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {body:?}: expected CreateProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["ProposalId"])
}

/// Cast one vote.
async fn vote(
    router: &MultiServiceRouter,
    network_id: &str,
    proposal_id: &str,
    voter_member_id: &str,
    value: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/networks/{network_id}/proposals/{proposal_id}/votes"),
        Some(json!({ "VoterMemberId": voter_member_id, "Vote": value })),
    )
    .await
}

/// Read one proposal back.
async fn get_proposal(
    router: &MultiServiceRouter,
    network_id: &str,
    proposal_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/networks/{network_id}/proposals/{proposal_id}"),
        None,
    )
    .await
}

/// Put one more `AVAILABLE` member on `network_id`, copied from the member the create
/// minted, and answer its id.
///
/// A member joins a network through `CreateMember`, which takes the invitation an approved
/// proposal mints — the very flow under test here — so the rows that need a membership of
/// a given SIZE seed it through this service's own store instead, to keep each row's
/// arrangement independent of the rule it asserts. The end-to-end join through
/// `ListInvitations` and `CreateMember` is asserted in
/// [`an_approved_invite_proposal_mints_the_invitation_the_invitee_joins_with`].
fn seed_member(engine: &engine::InMemoryEngine, network_id: &str, name: &str) -> String {
    let state = service_state(engine).expect("this service's own store");
    let mut guard = state.lock();
    let id = guard.mint_id("m");
    let network = guard
        .networks
        .get_mut(network_id)
        .expect("the network the create persisted");
    let founder = network
        .members
        .values()
        .next()
        .cloned()
        .expect("the member CreateNetwork minted");
    let member = StoredMember {
        id: id.clone(),
        name: name.to_string(),
        status: STATUS_AVAILABLE.to_string(),
        arn: member_arn(REGION, &id),
        ..founder
    };
    network
        .insert_member(member)
        .expect("room for another member on the network");
    id
}

/// One row of the voting table: a membership, the policy that decides its proposals, and
/// the votes cast on one proposal in order.
#[derive(Debug)]
struct VoteCase {
    name: &'static str,
    /// How many members the network holds when the votes are cast.
    members: usize,
    /// `ApprovalThresholdPolicy$ThresholdPercentage`; `None` names none at all.
    percentage: Option<i64>,
    /// `ApprovalThresholdPolicy$ThresholdComparator`; `None` leaves it out.
    comparator: Option<&'static str>,
    /// The votes cast, in order: the index of the voting member, its `VoteValue`, and the
    /// `Proposal$Status` the proposal must report once that vote is in.
    votes: &'static [(usize, &'static str, &'static str)],
}

/// Each vote is recorded against the proposal, and the network's `ApprovalThresholdPolicy`
/// decides it: approved once the `YES` votes reach the share the policy requires, rejected
/// once the `NO` votes put that share out of reach, open until then.
#[tokio::test]
async fn each_vote_is_recorded_and_the_policy_decides_the_proposal() {
    // Rows differ along the membership the threshold is measured against, the
    // `ThresholdPercentage` (both published bounds of its `@range(0, 100)` and values in
    // between), the `ThresholdComparator` (both values and its omission), whether a
    // threshold is named at all, and the order and value of the votes cast.
    let cases: &[VoteCase] = &[
        VoteCase {
            // One member, 50% `GREATER_THAN`: one `YES` is 100% of the membership.
            name: "single-member-unanimous-yes",
            members: 1,
            percentage: Some(50),
            comparator: Some("GREATER_THAN"),
            votes: &[(0, "YES", PROPOSAL_APPROVED)],
        },
        VoteCase {
            // The worked example `ThresholdPercentage` documents: "a ThresholdPercentage
            // value of 50 is specified on a network with 10 members, along with a
            // ThresholdComparator value of GREATER_THAN, this indicates that 6 YES votes
            // are required for the proposal to be approved".
            name: "documented-example-ten-members-need-six",
            members: 10,
            percentage: Some(50),
            comparator: Some("GREATER_THAN"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_IN_PROGRESS),
                (2, "YES", PROPOSAL_IN_PROGRESS),
                (3, "YES", PROPOSAL_IN_PROGRESS),
                (4, "YES", PROPOSAL_IN_PROGRESS),
                (5, "YES", PROPOSAL_APPROVED),
            ],
        },
        VoteCase {
            // 50% of 4 is 2 votes: `GREATER_THAN` takes a third, where
            // `GREATER_THAN_OR_EQUAL_TO` (the row below) is met by the second.
            name: "four-members-greater-than-takes-three",
            members: 4,
            percentage: Some(50),
            comparator: Some("GREATER_THAN"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_IN_PROGRESS),
                (2, "YES", PROPOSAL_APPROVED),
            ],
        },
        VoteCase {
            name: "four-members-greater-than-or-equal-to-takes-two",
            members: 4,
            percentage: Some(50),
            comparator: Some("GREATER_THAN_OR_EQUAL_TO"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_APPROVED),
            ],
        },
        VoteCase {
            // An omitted comparator is the documented example's `GREATER_THAN`: 50% of 2
            // members is one vote, so the second `YES` is what approves it.
            name: "omitted-comparator-is-greater-than",
            members: 2,
            percentage: Some(50),
            comparator: None,
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_APPROVED),
            ],
        },
        VoteCase {
            // The lower published bound of `ThresholdPercentageInt`'s `@range(0, 100)`:
            // the percentage is of the votes "that must be YES", so one `YES` carries it.
            name: "zero-percent-takes-one-yes",
            members: 3,
            percentage: Some(0),
            comparator: Some("GREATER_THAN_OR_EQUAL_TO"),
            votes: &[(0, "YES", PROPOSAL_APPROVED)],
        },
        VoteCase {
            // The upper published bound: 100% `GREATER_THAN_OR_EQUAL_TO` is unanimity.
            name: "hundred-percent-is-unanimity",
            members: 3,
            percentage: Some(100),
            comparator: Some("GREATER_THAN_OR_EQUAL_TO"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_IN_PROGRESS),
                (2, "YES", PROPOSAL_APPROVED),
            ],
        },
        VoteCase {
            // Past the upper bound in effect: no share of the membership is GREATER THAN
            // 100%, so no number of `YES` votes can approve this proposal. Rejection is
            // what `NO` votes do ("the number of NO votes required to reject it"), so a
            // `YES` vote leaves it open and only the clock closes it, on its
            // `ExpirationDate`.
            name: "hundred-percent-greater-than-is-unreachable",
            members: 2,
            percentage: Some(100),
            comparator: Some("GREATER_THAN"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_IN_PROGRESS),
            ],
        },
        VoteCase {
            // The same unreachable threshold, closed by a `NO` vote: one `NO` among two
            // members leaves at most one `YES`, short of the count required.
            name: "hundred-percent-greater-than-rejected-by-a-no",
            members: 2,
            percentage: Some(100),
            comparator: Some("GREATER_THAN"),
            votes: &[(0, "NO", PROPOSAL_REJECTED)],
        },
        VoteCase {
            // 50% of 3 members needs 2 `YES` under `GREATER_THAN`, so one `NO` still
            // leaves 2 members able to carry it and the second `NO` is what rejects it.
            name: "three-members-rejected-by-two-no",
            members: 3,
            percentage: Some(50),
            comparator: Some("GREATER_THAN"),
            votes: &[
                (0, "NO", PROPOSAL_IN_PROGRESS),
                (1, "NO", PROPOSAL_REJECTED),
            ],
        },
        VoteCase {
            // A `YES` and a `NO` among 3 members: 2 are still needed, only 2 remain
            // un-voted, so the proposal is still open on the votes cast.
            name: "mixed-votes-leave-the-proposal-open",
            members: 3,
            percentage: Some(50),
            comparator: Some("GREATER_THAN"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "NO", PROPOSAL_IN_PROGRESS),
            ],
        },
        VoteCase {
            // A policy that names no `ThresholdPercentage` fixes no share of the
            // membership, so no number of votes decides the proposal: it stays open to
            // its `ExpirationDate` however every member votes.
            name: "no-threshold-decides-nothing",
            members: 2,
            percentage: None,
            comparator: Some("GREATER_THAN"),
            votes: &[
                (0, "YES", PROPOSAL_IN_PROGRESS),
                (1, "YES", PROPOSAL_IN_PROGRESS),
            ],
        },
    ];

    for case in cases {
        let (router, engine) = router_and_engine();
        let name = case.name;
        let (network_id, founder) = create_network(
            &router,
            network_body(
                &format!("net-{name}"),
                "voting-net",
                case.percentage,
                case.comparator,
            ),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;

        let mut members = vec![founder.clone()];
        for i in 1..case.members {
            members.push(seed_member(&engine, &network_id, &format!("org{}", i + 1)));
        }

        let proposal_id = create_proposal(
            &router,
            &network_id,
            &founder,
            &format!("prop-{name}"),
            json!({ "Invitations": [{ "Principal": OTHER_ACCOUNT }] }),
        )
        .await;

        let mut yes = 0;
        let mut no = 0;
        for (cast, (voter, value, expected_status)) in case.votes.iter().enumerate() {
            let voter_id = &members[*voter];
            let answer = vote(&router, &network_id, &proposal_id, voter_id, value).await;
            assert_eq!(
                answer.status, 200,
                "case {name}/vote {cast}: expected VoteOnProposal 200, got {} {:?}",
                answer.status, answer.body
            );

            if *value == "YES" {
                yes += 1;
            } else {
                no += 1;
            }
            let read = get_proposal(&router, &network_id, &proposal_id).await;
            let proposal = &read.body["Proposal"];
            assert_eq!(
                proposal["Status"].as_str(),
                Some(*expected_status),
                "case {name}/vote {cast}: expected Status {expected_status:?} after {yes} YES \
                 and {no} NO among {} members, got {proposal:?}",
                case.members
            );
            // "OutstandingVoteCount: the number of members minus the sum of YES votes and
            // NO votes."
            let outstanding = i64::try_from(case.members).expect("a small membership") - yes - no;
            assert_eq!(
                (
                    proposal["YesVoteCount"].as_i64(),
                    proposal["NoVoteCount"].as_i64(),
                    proposal["OutstandingVoteCount"].as_i64()
                ),
                (Some(yes), Some(no), Some(outstanding)),
                "case {name}/vote {cast}: expected {yes} YES, {no} NO and {outstanding} \
                 outstanding, got {proposal:?}"
            );
        }
    }
}

/// An approved `InviteAction` is carried out: the invitation it owes its principal is
/// recorded, `ListInvitations` reports it, and the invitee joins the network with it.
#[tokio::test]
async fn an_approved_invite_proposal_mints_the_invitation_the_invitee_joins_with() {
    let (router, _engine) = router_and_engine();
    // 50% `GREATER_THAN` on a one-member network: the founding member's `YES` is the
    // unanimous vote that approves the proposal.
    let (network_id, founder) = create_network(
        &router,
        network_body("invite-net", "invite-net", Some(50), Some("GREATER_THAN")),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let proposal_id = create_proposal(
        &router,
        &network_id,
        &founder,
        "invite-prop",
        json!({ "Invitations": [{ "Principal": ACCOUNT }] }),
    )
    .await;

    let before = call(&router, "GET", "/invitations", None).await;
    assert_eq!(
        before.body["Invitations"].as_array().map(Vec::len),
        Some(0),
        "case before the vote: an IN_PROGRESS proposal mints no invitation, got {:?}",
        before.body
    );

    let answer = vote(&router, &network_id, &proposal_id, &founder, "YES").await;
    assert_eq!(
        answer.status, 200,
        "case the approving vote: expected VoteOnProposal 200, got {} {:?}",
        answer.status, answer.body
    );

    let read = get_proposal(&router, &network_id, &proposal_id).await;
    let proposal = &read.body["Proposal"];
    assert_eq!(
        (
            proposal["Status"].as_str(),
            proposal["YesVoteCount"].as_i64(),
            proposal["OutstandingVoteCount"].as_i64()
        ),
        (Some(PROPOSAL_APPROVED), Some(1), Some(0)),
        "case the approving vote: expected the proposal {PROPOSAL_APPROVED} with its one YES \
         vote counted, got {proposal:?}"
    );

    let listing = call(&router, "GET", "/invitations", None).await;
    let invitations = listing.body["Invitations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        invitations.len(),
        1,
        "case once approved: expected the one InviteAction to have been carried out, got {:?}",
        listing.body
    );
    let invitation = &invitations[0];
    assert_eq!(
        (
            invitation["Status"].as_str(),
            invitation["NetworkSummary"]["Id"].as_str()
        ),
        (Some("PENDING"), Some(network_id.as_str())),
        "case once approved: expected a PENDING invitation onto {network_id}, got {invitation:?}"
    );
    let invitation_id = invitation["InvitationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The invitation the approval minted is usable: the invitee joins the network with it.
    let joined = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/members"),
        Some(json!({
            "ClientRequestToken": "join-with-the-invitation",
            "InvitationId": invitation_id,
            "MemberConfiguration": {
                "Name": "org2",
                "FrameworkConfiguration": {
                    "Fabric": { "AdminUsername": "AdminUser", "AdminPassword": "MyPassword1" }
                }
            }
        })),
    )
    .await;
    assert_eq!(
        joined.status, 200,
        "case joining with the minted invitation: expected CreateMember 200, got {} {:?}",
        joined.status, joined.body
    );
    let joined_member = joined.text(&["MemberId"]);
    tokio::time::sleep(PAST_TRANSITION).await;

    let member = call(
        &router,
        "GET",
        &format!("/networks/{network_id}/members/{joined_member}"),
        None,
    )
    .await;
    assert_eq!(
        (member.status, member.text(&["Member", "Name"])),
        (200, "org2".to_string()),
        "case reading the joined member back: expected GetMember 200 for org2, got {} {:?}",
        member.status,
        member.body
    );

    // The approval is final: the proposal is no longer open, so the same member cannot
    // vote on it again and no second invitation is minted.
    let again = vote(&router, &network_id, &proposal_id, &founder, "YES").await;
    assert_eq!(
        (again.status, again.error_type.as_str()),
        (400, "IllegalActionException"),
        "case voting again after approval: expected 400 IllegalActionException, got {} {:?}",
        again.status,
        again.body
    );
    let listing = call(&router, "GET", "/invitations", None).await;
    assert_eq!(
        listing.body["Invitations"].as_array().map(Vec::len),
        Some(1),
        "case voting again after approval: the actions must be carried out once, got {:?}",
        listing.body
    );
}

/// An approved `RemoveAction` deletes the member it names "and all associated resources"
/// from the network; one that names a member the network no longer holds cannot be carried
/// out, which is `ACTION_FAILED`.
#[tokio::test]
async fn an_approved_removal_proposal_deletes_the_member_and_its_resources() {
    // Rows: the removal of a member the network holds, and the removal of a member id
    // nothing on the network holds.
    let absent_member = format!("m-{ABSENT_SUFFIX}");
    let cases: &[(&str, bool, &str)] = &[
        ("removes a member the network holds", true, PROPOSAL_APPROVED),
        (
            "removes a member the network does not hold",
            false,
            PROPOSAL_ACTION_FAILED,
        ),
    ];

    for (case, present, expected_status) in cases {
        let (router, engine) = router_and_engine();
        // 50% `GREATER_THAN_OR_EQUAL_TO` on a two-member network: one `YES` is half the
        // membership, which meets the threshold.
        let (network_id, founder) = create_network(
            &router,
            network_body(
                "removal-net",
                "removal-net",
                Some(50),
                Some("GREATER_THAN_OR_EQUAL_TO"),
            ),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;
        let other = seed_member(&engine, &network_id, "org2");
        let node_id = create_node(
            &router,
            &network_id,
            &other,
            "removal-node",
            json!({ "InstanceType": "bc.t3.small" }),
        )
        .await;

        let target = if *present {
            other.clone()
        } else {
            absent_member.clone()
        };
        let proposal_id = create_proposal(
            &router,
            &network_id,
            &founder,
            &format!("removal-{case}"),
            json!({ "Removals": [{ "MemberId": target }] }),
        )
        .await;

        let answer = vote(&router, &network_id, &proposal_id, &founder, "YES").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected VoteOnProposal 200, got {} {:?}",
            answer.status, answer.body
        );

        let read = get_proposal(&router, &network_id, &proposal_id).await;
        assert_eq!(
            read.text(&["Proposal", "Status"]),
            *expected_status,
            "case {case}: expected the proposal {expected_status}, got {:?}",
            read.body
        );

        // The member named by the removal, and the node it owned, are gone exactly when
        // the action was carried out.
        let expected_lookup = if *present { 404 } else { 200 };
        let member = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members/{other}"),
            None,
        )
        .await;
        assert_eq!(
            member.status, expected_lookup,
            "case {case}: expected GetMember {expected_lookup} for {other}, got {} {:?}",
            member.status, member.body
        );
        let node = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/nodes/{node_id}?memberId={other}"),
            None,
        )
        .await;
        assert_eq!(
            node.status, expected_lookup,
            "case {case}: expected GetNode {expected_lookup} for the removed member's node, got \
             {} {:?}",
            node.status,
            node.body
        );

        // The founding member that made the proposal is untouched either way.
        let founding = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members/{founder}"),
            None,
        )
        .await;
        assert_eq!(
            founding.status, 200,
            "case {case}: expected the proposing member to survive, got {} {:?}",
            founding.status, founding.body
        );
    }
}

/// A vote the service cannot accept is refused with the modelled error, and the proposal
/// is left as it was — over a table of every way `VoteOnProposal` is refused.
#[tokio::test]
async fn a_vote_the_service_cannot_accept_is_refused() {
    let (router, engine) = router_and_engine();
    // 100% `GREATER_THAN_OR_EQUAL_TO` on a two-member network: unanimity, so one vote
    // never decides a proposal and every row below votes on an open proposal.
    let (network_id, founder) = create_network(
        &router,
        network_body(
            "refuse-net",
            "refuse-net",
            Some(100),
            Some("GREATER_THAN_OR_EQUAL_TO"),
        ),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let other = seed_member(&engine, &network_id, "org2");
    let invite = json!({ "Invitations": [{ "Principal": OTHER_ACCOUNT }] });

    // An open proposal the founding member has already voted on.
    let voted = create_proposal(&router, &network_id, &founder, "voted", invite.clone()).await;
    let first = vote(&router, &network_id, &voted, &founder, "YES").await;
    assert_eq!(
        first.status, 200,
        "arranging the already-voted proposal: expected VoteOnProposal 200, got {} {:?}",
        first.status, first.body
    );

    // A proposal both members rejected, and one whose `ExpirationDate` has passed: the
    // model's shortest proposal duration is an hour (`ProposalDurationInHours` is
    // `@range(1, 168)`), so the stored expiry is backdated rather than waited out.
    let rejected =
        create_proposal(&router, &network_id, &founder, "rejected", invite.clone()).await;
    let answer = vote(&router, &network_id, &rejected, &founder, "NO").await;
    assert_eq!(
        answer.status, 200,
        "arranging the rejected proposal: expected VoteOnProposal 200, got {} {:?}",
        answer.status, answer.body
    );

    let expired = create_proposal(&router, &network_id, &founder, "expired", invite).await;
    {
        let state = service_state(&*engine).expect("this service's own store");
        let mut guard = state.lock();
        let proposal = guard
            .networks
            .get_mut(&network_id)
            .and_then(|network| network.proposals.get_mut(&expired))
            .expect("the proposal the create persisted");
        proposal.expiration_date = Some(SystemTime::now() - Duration::from_secs(1));
    }

    let absent_network = format!("n-{ABSENT_SUFFIX}");
    let absent_proposal = format!("p-{ABSENT_SUFFIX}");
    let absent_member = format!("m-{ABSENT_SUFFIX}");

    // Rows: the three ids a vote names that can name nothing stored (and a member that
    // belongs to another network), a proposal in each status that is no longer open for
    // voting, and the member voting a second time. The last row is the vote that is
    // accepted, so the table shows the refusals are not refusing everything.
    let cases: &[(&str, &str, &str, &str, u16, &str)] = &[
        (
            "unknown network",
            absent_network.as_str(),
            voted.as_str(),
            founder.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown proposal",
            network_id.as_str(),
            absent_proposal.as_str(),
            founder.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown member",
            network_id.as_str(),
            voted.as_str(),
            absent_member.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "member voting twice",
            network_id.as_str(),
            voted.as_str(),
            founder.as_str(),
            400,
            "IllegalActionException",
        ),
        (
            "rejected proposal",
            network_id.as_str(),
            rejected.as_str(),
            other.as_str(),
            400,
            "IllegalActionException",
        ),
        (
            "expired proposal",
            network_id.as_str(),
            expired.as_str(),
            other.as_str(),
            400,
            "IllegalActionException",
        ),
        (
            "the member that has not voted yet",
            network_id.as_str(),
            voted.as_str(),
            other.as_str(),
            200,
            "",
        ),
    ];

    for (case, network, proposal, member, expected_status, expected_error) in cases {
        let answer = vote(&router, network, proposal, member, "YES").await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (*expected_status, *expected_error),
            "case {case}: expected {expected_status} {expected_error:?}, got {} {:?}",
            answer.status,
            answer.body
        );
        if *expected_status != 200 {
            assert!(
                !answer.text(&["Message"]).is_empty() || !answer.text(&["message"]).is_empty(),
                "case {case}: a refusal must say what was wrong, got {:?}",
                answer.body
            );
        }
    }

    // The refused rows changed nothing: the rejected and expired proposals kept their
    // status and their one vote, and the open proposal counts exactly the two votes the
    // accepted rows cast.
    for (proposal, expected_status, expected_yes, expected_no) in [
        (&voted, PROPOSAL_APPROVED, 2, 0),
        (&rejected, PROPOSAL_REJECTED, 0, 1),
        (&expired, PROPOSAL_EXPIRED, 0, 0),
    ] {
        let read = get_proposal(&router, &network_id, proposal).await;
        let body = &read.body["Proposal"];
        assert_eq!(
            (
                body["Status"].as_str(),
                body["YesVoteCount"].as_i64(),
                body["NoVoteCount"].as_i64()
            ),
            (
                Some(expected_status),
                Some(expected_yes),
                Some(expected_no)
            ),
            "case the proposal after the refusals: expected {expected_status} with \
             {expected_yes} YES and {expected_no} NO, got {body:?}"
        );
    }
}

/// "The member to vote as, specified by `VoterMemberId`, must be in the same Amazon Web
/// Services account as the principal that calls the action": a member another account
/// holds cannot be voted as.
#[tokio::test]
async fn voting_as_a_member_another_account_holds_is_denied() {
    let (router, engine) = router_and_engine();
    let (network_id, founder) = create_network(
        &router,
        network_body("denied-net", "denied-net", Some(100), Some("GREATER_THAN")),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let other = seed_member(&engine, &network_id, "org2");

    // A member whose stored ARN names another account is a member that account holds —
    // the ARN its create minted is this service's record of whose member it is.
    {
        let state = service_state(&*engine).expect("this service's own store");
        let mut guard = state.lock();
        let member = guard
            .networks
            .get_mut(&network_id)
            .and_then(|network| network.members.get_mut(&other))
            .expect("the seeded member");
        member.arn = member.arn.replace(ACCOUNT, OTHER_ACCOUNT);
    }

    let proposal_id = create_proposal(
        &router,
        &network_id,
        &founder,
        "denied-prop",
        json!({ "Invitations": [{ "Principal": OTHER_ACCOUNT }] }),
    )
    .await;

    let answer = vote(&router, &network_id, &proposal_id, &other, "YES").await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (403, "AccessDeniedException"),
        "case voting as another account's member: expected 403 AccessDeniedException, got {} {:?}",
        answer.status,
        answer.body
    );

    let read = get_proposal(&router, &network_id, &proposal_id).await;
    let proposal = &read.body["Proposal"];
    assert_eq!(
        (
            proposal["Status"].as_str(),
            proposal["YesVoteCount"].as_i64(),
            proposal["OutstandingVoteCount"].as_i64()
        ),
        (Some(PROPOSAL_IN_PROGRESS), Some(0), Some(2)),
        "case voting as another account's member: the refused vote must not be counted, got \
         {proposal:?}"
    );

    // The account's own member still votes.
    let answer = vote(&router, &network_id, &proposal_id, &founder, "YES").await;
    assert_eq!(
        answer.status, 200,
        "case voting as this account's own member: expected VoteOnProposal 200, got {} {:?}",
        answer.status, answer.body
    );
}

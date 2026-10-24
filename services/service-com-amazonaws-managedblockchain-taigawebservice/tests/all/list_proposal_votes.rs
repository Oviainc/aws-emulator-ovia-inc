// SPDX-License-Identifier: CC0-1.0

//! `ListProposalVotes`, end to end over HTTP: the page it answers with is cut from the
//! votes `VoteOnProposal` stored on one proposal, every `VoteSummary` member is read off
//! those records, and a `ProposalId` that resolves to no stored proposal is refused
//! instead of answered with an empty page.
//!
//! The tables below span the classes the rules generalise over: vote counts from none to
//! six, every `VoteValue` the model declares and both orders of mixing them, the member
//! name forms `NetworkMemberNameString` admits, page sizes from one to past the vote
//! count plus both published `MaxResults` bounds and one past each, the empty
//! `NextToken` `PaginationToken`'s `@length(0, 128)` allows and a forged one, and the id
//! forms that resolve to no stored proposal.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together. The votes are cast through `VoteOnProposal` and cross-checked against
//! `GetProposal`'s own counts, so nothing is asserted against the store that a caller
//! cannot see.

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    PROPOSAL_MAX_RESULTS, PUBLIC_NETWORKS, VOTE_NO, VOTE_YES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_network, create_network_body, router_and_engine, ACCOUNT, PAST_TRANSITION,
};
use crate::list_members::{seed_members, Seed};
use crate::network_and_node_lifecycle::Answer;

/// Member names the seeded voters take, spanning the forms
/// `NetworkMemberNameString`'s `@pattern ^(?!-|[0-9])(?!.*-$)(?!.*?--)[a-zA-Z0-9-]+$` and
/// `@length(1, 64)` admit (smithy_query: shape VoteSummary, member MemberName): a bare
/// word, a hyphenated one, mixed case, the shortest legal name, and a long one.
const VOTER_NAMES: &[&str] = &[
    "org-two",
    "OrgThree",
    "o",
    "org-four-with-a-rather-longer-name",
    "Org-Five",
    "organisation6",
];

/// A `CreateNetwork` body whose `ApprovalThresholdPolicy` names a proposal duration but
/// NO `ThresholdPercentage`.
///
/// Nothing then fixes how many `YES` votes approve a proposal
/// (`ApprovalThresholdPolicy$ThresholdPercentage` is what does), so no vote closes it and
/// each row can cast every vote it wants on one open proposal — the arrangement a listing
/// row needs, kept independent of the voting rules asserted in `vote_on_proposal.rs`.
fn open_network_body(token: &str, name: &str) -> Value {
    let mut body = create_network_body(token, name);
    body["VotingPolicy"] = json!({
        "ApprovalThresholdPolicy": { "ProposalDurationInHours": 24 }
    });
    body
}

/// One `ListProposalVotes` call: `query` is the already-encoded query string (empty for
/// none).
async fn list_votes(
    router: &MultiServiceRouter,
    network_id: &str,
    proposal_id: &str,
    query: &str,
) -> Answer {
    let path = format!("/networks/{network_id}/proposals/{proposal_id}/votes");
    let path = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, "GET", &path, None).await
}

/// The `(MemberId, Vote, MemberName)` of every vote the answer reports, in the order it
/// reported them.
fn listed_votes(answer: &Answer) -> Vec<(String, String, String)> {
    answer.body["ProposalVotes"]
        .as_array()
        .map(|votes| {
            votes
                .iter()
                .map(|vote| {
                    (
                        vote["MemberId"].as_str().unwrap_or_default().to_string(),
                        vote["Vote"].as_str().unwrap_or_default().to_string(),
                        vote["MemberName"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create one proposal on `network_id` as `member_id` and answer its `ProposalId`.
async fn create_proposal(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
) -> String {
    let body = json!({
        "ClientRequestToken": token,
        "MemberId": member_id,
        // One well-formed `InviteAction`: `Actions` must name at least one action, and
        // which action it names is nothing this listing reads.
        "Actions": { "Invitations": [{ "Principal": ACCOUNT }] },
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

/// One network with `voters.len()` members (the one `CreateNetwork` minted plus seeded
/// ones named from [`VOTER_NAMES`]) and one open proposal on it.
///
/// Answers the router, the network id, the proposal id, and the `(MemberId, MemberName)`
/// of every member in the order [`VOTER_NAMES`] names them.
async fn network_with_voters(
    token: &str,
    voters: usize,
) -> (MultiServiceRouter, String, String, Vec<(String, String)>) {
    let (router, engine) = router_and_engine();
    let (network_id, founder) =
        create_network(&router, open_network_body(token, "votes-net")).await;
    // `CreateProposal` needs a settled network and a settled proposing member.
    tokio::time::sleep(PAST_TRANSITION).await;

    let seeds: Vec<Seed> = VOTER_NAMES
        .iter()
        .copied()
        .take(voters.saturating_sub(1))
        .map(|name| Seed {
            name,
            status: "AVAILABLE",
            account: ACCOUNT,
        })
        .collect();
    let seeded = seed_members(&engine, &network_id, &seeds);

    // The member `CreateNetwork` minted is named by `member_configuration("org1")`.
    let mut members = vec![(founder.clone(), "org1".to_string())];
    for (id, seed) in seeded.iter().zip(&seeds) {
        members.push((id.clone(), seed.name.to_string()));
    }

    let proposal_id = create_proposal(&router, &network_id, &founder, &format!("{token}-p")).await;
    (router, network_id, proposal_id, members)
}

/// Cast one vote, asserting it was accepted.
async fn vote(
    router: &MultiServiceRouter,
    network_id: &str,
    proposal_id: &str,
    voter_member_id: &str,
    value: &str,
) {
    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/proposals/{proposal_id}/votes"),
        Some(json!({ "VoterMemberId": voter_member_id, "Vote": value })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case vote {value} by {voter_member_id}: expected VoteOnProposal 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// Every vote cast on a proposal is reported with the value that was cast and the id and
/// current name of the member that cast it; a proposal nobody voted on reports an empty
/// list, and reading twice answers the same thing.
#[tokio::test]
async fn every_vote_cast_is_reported_with_its_value_and_voting_member() {
    // Rows differ along the number of votes cast (none up to one per member), the
    // `VoteValue` of each (smithy_query: shape VoteValue = YES | NO) and the order the
    // values are mixed in.
    let cases: &[(&str, &[&str])] = &[
        ("no-votes", &[]),
        ("one-yes", &[VOTE_YES]),
        ("one-no", &[VOTE_NO]),
        ("yes-then-no", &[VOTE_YES, VOTE_NO]),
        ("no-then-yes", &[VOTE_NO, VOTE_YES]),
        ("every-vote-no", &[VOTE_NO, VOTE_NO, VOTE_NO]),
        ("mixed-five", &[VOTE_YES, VOTE_NO, VOTE_NO, VOTE_YES, VOTE_NO]),
    ];

    for (case, votes) in cases {
        // One more member than the row casts votes, so a member that did not vote is
        // never reported as having voted.
        let (router, network_id, proposal_id, members) =
            network_with_voters(&format!("votes-{case}"), votes.len() + 1).await;

        for (value, (member_id, _)) in votes.iter().zip(&members) {
            vote(&router, &network_id, &proposal_id, member_id, value).await;
        }

        let answer = list_votes(&router, &network_id, &proposal_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected ListProposalVotes 200, got {} {:?}",
            answer.status, answer.body
        );

        // The listing pages in member-id order, so the expectation is sorted by id.
        let mut expected: Vec<(String, String, String)> = votes
            .iter()
            .zip(&members)
            .map(|(value, (member_id, name))| {
                (member_id.clone(), (*value).to_string(), name.clone())
            })
            .collect();
        expected.sort();
        assert_eq!(
            listed_votes(&answer),
            expected,
            "case {case}: expected exactly the votes cast, in member-id order, got {:?}",
            answer.body
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no NextToken on a single page, got {:?}",
            answer.body
        );

        for reported in answer.body["ProposalVotes"].as_array().unwrap_or(&Vec::new()) {
            let mut keys: Vec<String> = reported
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default();
            keys.sort();
            // The members `VoteSummary` declares (smithy_query: shape VoteSummary).
            assert_eq!(
                keys,
                vec![
                    "MemberId".to_string(),
                    "MemberName".to_string(),
                    "Vote".to_string()
                ],
                "case {case}: expected exactly the VoteSummary members the model declares, got {reported:?}"
            );
        }

        // The counts `GetProposal` reports are the counts of the very votes listed here:
        // both readers answer from the one stored record.
        let proposal = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/proposals/{proposal_id}"),
            None,
        )
        .await;
        let yes = votes.iter().filter(|value| **value == VOTE_YES).count() as i64;
        let no = votes.iter().filter(|value| **value == VOTE_NO).count() as i64;
        assert_eq!(
            (
                proposal.body["Proposal"]["YesVoteCount"].as_i64(),
                proposal.body["Proposal"]["NoVoteCount"].as_i64(),
                proposal.body["Proposal"]["OutstandingVoteCount"].as_i64(),
            ),
            (
                Some(yes),
                Some(no),
                Some(members.len() as i64 - yes - no)
            ),
            "case {case}: expected GetProposal's counts to match the votes listed, got {:?}",
            proposal.body
        );

        // A listing reports: it does not change what a later caller sees.
        let again = list_votes(&router, &network_id, &proposal_id, "").await;
        assert_eq!(
            again.body, answer.body,
            "case {case}: expected a repeated ListProposalVotes to answer identically, got {:?}",
            again.body
        );
    }
}

/// The listing pages by `MaxResults` through `NextToken`: walking the pages reports every
/// vote exactly once, in member-id order, and the last page carries no token.
#[tokio::test]
async fn list_proposal_votes_pages_through_every_vote() {
    let voters = VOTER_NAMES.len() + 1;
    let (router, network_id, proposal_id, members) = network_with_voters("paging", voters).await;
    for (index, (member_id, _)) in members.iter().enumerate() {
        let value = if index % 2 == 0 { VOTE_YES } else { VOTE_NO };
        vote(&router, &network_id, &proposal_id, member_id, value).await;
    }

    let mut everything: Vec<String> = members.iter().map(|(id, _)| id.clone()).collect();
    everything.sort();

    // Rows: a page per vote, sizes either side of the vote count, both published
    // `MaxResults` bounds (`@range(1, 100)`) and no `MaxResults` at all.
    let cases: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(voters as i32 - 1),
        Some(voters as i32),
        Some(voters as i32 + 1),
        Some(PROPOSAL_MAX_RESULTS),
        None,
    ];

    for max_results in cases {
        let case = format!("maxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match max_results {
                Some(limit) => format!("maxResults={limit}"),
                None => String::new(),
            };
            if let Some(token) = &next_token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_votes(&router, &network_id, &proposal_id, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {pages}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let ids: Vec<String> = listed_votes(&answer)
                .into_iter()
                .map(|(member_id, _, _)| member_id)
                .collect();
            let limit = max_results.unwrap_or(PROPOSAL_MAX_RESULTS) as usize;
            assert!(
                ids.len() <= limit && !ids.is_empty(),
                "case {case} page {pages}: expected 1..={limit} votes, got {:?}",
                answer.body
            );
            seen.extend(ids);
            next_token = answer.body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= everything.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, everything,
            "case {case}: expected every stored vote exactly once, in member-id order, got {seen:?}"
        );
    }

    // `NextToken` is `@length(0, 128)`, so the empty string is a legal value: it names no
    // position, which is the start.
    let answer = list_votes(&router, &network_id, &proposal_id, "nextToken=").await;
    let ids: Vec<String> = listed_votes(&answer)
        .into_iter()
        .map(|(member_id, _, _)| member_id)
        .collect();
    assert_eq!(
        (answer.status, ids),
        (200, everything.clone()),
        "case an empty nextToken: expected the first page of every vote, got {:?}",
        answer.body
    );

    // A token this service never issued cannot resume a page.
    let answer = list_votes(&router, &network_id, &proposal_id, "nextToken=not-a-token").await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case a forged nextToken: expected 400 InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );

    // One past each published `MaxResults` bound is refused at the boundary, before the
    // handler runs. Only the status is asserted: which shape the boundary validator
    // refuses with is not this operation's to choose.
    for out_of_range in [0, PROPOSAL_MAX_RESULTS + 1] {
        let answer = list_votes(
            &router,
            &network_id,
            &proposal_id,
            &format!("maxResults={out_of_range}"),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={out_of_range}: expected a refusal, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// A request that resolves to no stored proposal is refused with this operation's own
/// `InvalidRequestException` — the model declares no `ResourceNotFoundException` for it —
/// naming the proposal, never answered with an empty page.
#[tokio::test]
async fn a_request_that_resolves_to_no_proposal_is_refused() {
    let (router, network_id, proposal_id, members) = network_with_voters("missing", 1).await;
    vote(
        &router,
        &network_id,
        &proposal_id,
        &members[0].0,
        VOTE_YES,
    )
    .await;

    // Rows: the id forms a caller can pass that resolve to no stored proposal — a
    // well-shaped proposal id on the stored network, the shortest and longest
    // `ResourceIdString`'s `@length(1, 32)` allows, a proposal id scoped by a network
    // nothing stored holds, the stored proposal under the WRONG stored network, and a
    // proposal id on one of the standing public networks, which hold no proposals at all.
    let public = PUBLIC_NETWORKS[1].id;
    let cases: &[(&str, &str)] = &[
        (&network_id, "p-NOSUCHPROPOSALNOSUCHPROPOSA"),
        (&network_id, "x"),
        (&network_id, "p-doesnotexist000000000000000000"),
        ("n-NOSUCHNETWORKNOSUCHNETWOR", "p-NOSUCHPROPOSALNOSUCHPROPOSA"),
        ("n-NOSUCHNETWORKNOSUCHNETWOR", &proposal_id),
        (public, &proposal_id),
    ];

    for (network, proposal) in cases {
        let answer = list_votes(&router, network, proposal, "").await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case {network}/{proposal}: expected 400 InvalidRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.text(&["Message"]).contains(*proposal),
            "case {network}/{proposal}: expected a Message naming the proposal, got {:?}",
            answer.body
        );
    }

    // And the refusals changed nothing: the proposal that does exist still lists its one
    // vote.
    let answer = list_votes(&router, &network_id, &proposal_id, "").await;
    assert_eq!(
        (answer.status, listed_votes(&answer).len()),
        (200, 1),
        "case the stored proposal: expected its one vote, got {} {:?}",
        answer.status,
        answer.body
    );
}

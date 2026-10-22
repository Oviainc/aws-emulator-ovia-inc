// SPDX-License-Identifier: CC0-1.0

//! The Managed Blockchain proposal lifecycle, end to end over HTTP: `CreateProposal`
//! persists the proposal it was given against the network the request names, and
//! `GetProposal` answers from that record.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures. The request harness (`new_router`,
//! `call`, `create_network`, the id and date-time shape checks) is the one the
//! network/node suite beside this file already defines.
//!
//! The tables below span the classes the rules generalise over: the action kinds a
//! proposal can carry and how many of each, descriptions and tag counts up to the
//! modelled maxima, every published `ProposalDurationInHours` bound (and a policy that
//! names none), and every way a proposal cannot be created.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    service_state, PROPOSAL_EXPIRED, TRANSITION_SPAN,
};

use super::network_and_node_lifecycle::{
    available_network, call, create_network, create_network_body, is_date_time, is_resource_id,
    new_router, router_over, Answer, ACCOUNT, PAST_TRANSITION, REGION,
};

/// An id with the shape every Managed Blockchain resource id takes, that no create ever
/// minted: used for the rows that name a network, member or proposal nothing stored holds.
const ABSENT_SUFFIX: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The name `create_network_body` gives the network's first member, which every proposal
/// it creates is therefore `ProposedByMemberName`.
const MEMBER_NAME: &str = "org1";

/// What a row's `Description` is: absent, a literal, or the longest one
/// `DescriptionString`'s `@length(0, 128)` allows.
#[derive(Debug)]
enum Description {
    Absent,
    Text(&'static str),
    Longest,
}

impl Description {
    /// The `Description` the stored proposal must report back.
    fn expected(&self) -> Option<String> {
        match self {
            Description::Absent => None,
            Description::Text(text) => Some((*text).to_string()),
            Description::Longest => Some("d".repeat(128)),
        }
    }
}

/// One row of the proposal table: what the request configures, so the read back can be
/// asserted against it.
#[derive(Debug)]
struct ProposalCase {
    name: &'static str,
    /// How many `InviteAction`s the proposal carries.
    invitations: usize,
    /// How many `RemoveAction`s it carries; each names the network's own first member,
    /// the only member a freshly created network has.
    removals: usize,
    description: Description,
    tag_count: usize,
    /// The `ApprovalThresholdPolicy$ProposalDurationInHours` the network is created with;
    /// `None` creates it with a `VotingPolicy` that names no threshold policy at all,
    /// which fixes no proposal expiry.
    duration_hours: Option<i64>,
}

/// A `CreateNetwork` body whose voting policy names `duration_hours` — the duration every
/// proposal on that network expires after.
fn network_body(token: &str, name: &str, duration_hours: Option<i64>) -> Value {
    let mut body = create_network_body(token, name);
    match duration_hours {
        Some(hours) => {
            body["VotingPolicy"]["ApprovalThresholdPolicy"]["ProposalDurationInHours"] =
                json!(hours);
        }
        // `VotingPolicy$ApprovalThresholdPolicy` is optional, and it is the only place a
        // proposal duration is specified.
        None => body["VotingPolicy"] = json!({}),
    }
    body
}

/// The `Actions` for one row: `invitations` invites of distinct account ids, or
/// `removals` removals of `member_id`.
fn actions_of(case: &ProposalCase, member_id: &str) -> Value {
    let mut actions = json!({});
    if case.invitations > 0 {
        actions["Invitations"] = Value::Array(
            (0..case.invitations)
                // `PrincipalString` carries no modelled constraint; these are account-id
                // shaped so the row reads like the invite it stands for.
                .map(|i| json!({ "Principal": format!("{:012}", 100_000_000_000u64 + i as u64) }))
                .collect(),
        );
    }
    if case.removals > 0 {
        actions["Removals"] = Value::Array(
            (0..case.removals)
                .map(|_| json!({ "MemberId": member_id }))
                .collect(),
        );
    }
    actions
}

/// `tag_count` distinct tags, within `InputTagMap`'s `@length(0, 50)`.
fn tags_of(tag_count: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for i in 0..tag_count {
        tags.insert(format!("key-{i}"), json!(format!("value-{i}")));
    }
    Value::Object(tags)
}

/// Create one proposal on `network_id` as `member_id` and return what the emulator
/// answered.
///
/// `pub(crate)` so a sibling suite in this target drives a create through this one helper
/// rather than a second copy of the request.
pub(crate) async fn create_proposal(
    router: &smithy_server_runtime::MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
    actions: Value,
    extra: &[(&str, Value)],
) -> Answer {
    let mut body = json!({
        "ClientRequestToken": token,
        "MemberId": member_id,
        "Actions": actions,
    });
    for (member, value) in extra {
        body[*member] = value.clone();
    }
    call(
        router,
        "POST",
        &format!("/networks/{network_id}/proposals"),
        Some(body),
    )
    .await
}

/// Read one proposal back.
async fn get_proposal(
    router: &smithy_server_runtime::MultiServiceRouter,
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

/// The epoch seconds of an ISO-8601 date-time member, or `None` when it is absent or is
/// not a date-time.
fn epoch_secs(value: &Value) -> Option<i64> {
    let text = value.as_str()?;
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|parsed| parsed.secs())
}

/// `CreateProposal` persists what it was given and `GetProposal` answers from that record
/// — over a table spanning the action kinds and counts, descriptions, tag counts and
/// proposal durations a proposal is created with.
#[tokio::test]
async fn create_proposal_persists_what_it_was_given() {
    let router = new_router();

    // Rows differ along every member the response is projected from: which kind of
    // action the proposal carries and how many, the description (absent, a literal, the
    // empty string `DescriptionString`'s @length(0, 128) minimum allows, and one at its
    // maximum), the tag count (none, some, and `InputTagMap`'s @length(0, 50) maximum)
    // and the proposal duration — both published bounds of `ProposalDurationInHours`'s
    // @range(1, 168), one in between, and a voting policy that names no duration at all.
    let cases: &[ProposalCase] = &[
        ProposalCase {
            name: "one-invite",
            invitations: 1,
            removals: 0,
            description: Description::Absent,
            tag_count: 0,
            duration_hours: Some(24),
        },
        ProposalCase {
            name: "three-invites",
            invitations: 3,
            removals: 0,
            description: Description::Text("Proposal to add Example Corp. as member."),
            tag_count: 2,
            duration_hours: Some(1),
        },
        ProposalCase {
            name: "one-removal",
            invitations: 0,
            removals: 1,
            description: Description::Text("Proposal to remove the founding member."),
            tag_count: 1,
            duration_hours: Some(168),
        },
        ProposalCase {
            name: "max-tags-and-description",
            invitations: 2,
            removals: 0,
            description: Description::Longest,
            tag_count: 50,
            duration_hours: Some(72),
        },
        ProposalCase {
            name: "no-duration",
            invitations: 1,
            removals: 0,
            description: Description::Text(""),
            tag_count: 0,
            duration_hours: None,
        },
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for case in cases {
        let name = case.name;
        // One network per row, so each row's own voting policy decides its proposal's
        // expiry.
        let (network_id, member_id) = create_network(
            &router,
            network_body(&format!("net-{name}"), "proposal-net", case.duration_hours),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;

        let mut extra: Vec<(&str, Value)> = vec![("Tags", tags_of(case.tag_count))];
        let description = case.description.expected();
        if let Some(description) = &description {
            extra.push(("Description", json!(description)));
        }

        let answer = create_proposal(
            &router,
            &network_id,
            &member_id,
            &format!("prop-{name}"),
            actions_of(case, &member_id),
            &extra,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected CreateProposal 200, got {} {:?}",
            answer.status, answer.body
        );
        let proposal_id = answer.text(&["ProposalId"]);
        assert!(
            is_resource_id(&proposal_id, "p"),
            "case {name}: expected a minted proposal id, got {proposal_id:?}"
        );
        assert!(
            !seen_ids.contains(&proposal_id),
            "case {name}: every create must mint a fresh proposal id, got a repeat of \
             {proposal_id:?}"
        );
        seen_ids.push(proposal_id.clone());

        let answer = get_proposal(&router, &network_id, &proposal_id).await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected GetProposal 200 for the id the create minted, got {} {:?}",
            answer.status, answer.body
        );
        let proposal = &answer.body["Proposal"];

        for (field, expected) in [
            ("ProposalId", proposal_id.clone()),
            ("NetworkId", network_id.clone()),
            ("ProposedByMemberId", member_id.clone()),
            ("ProposedByMemberName", MEMBER_NAME.to_string()),
            // "IN_PROGRESS - The proposal is active and open for member voting."
            ("Status", "IN_PROGRESS".to_string()),
            (
                "Arn",
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:proposals/{proposal_id}"),
            ),
        ] {
            assert_eq!(
                proposal[field].as_str(),
                Some(expected.as_str()),
                "case {name}: expected Proposal.{field} {expected:?}, got {proposal:?}"
            );
        }
        assert_eq!(
            proposal["Description"].as_str().map(ToString::to_string),
            description,
            "case {name}: expected Proposal.Description {description:?}, got {proposal:?}"
        );

        // No vote has been cast, and "OutstandingVoteCount" is "the number of members
        // minus the sum of YES votes and NO votes" — one member, so one vote outstanding.
        for (field, expected) in [
            ("YesVoteCount", 0),
            ("NoVoteCount", 0),
            ("OutstandingVoteCount", 1),
        ] {
            assert_eq!(
                proposal[field].as_i64(),
                Some(expected),
                "case {name}: expected Proposal.{field} {expected}, got {proposal:?}"
            );
        }

        let invitations = proposal["Actions"]["Invitations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let removals = proposal["Actions"]["Removals"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            (invitations.len(), removals.len()),
            (case.invitations, case.removals),
            "case {name}: expected {} invitations and {} removals back, got {proposal:?}",
            case.invitations,
            case.removals
        );
        for (i, invitation) in invitations.iter().enumerate() {
            let expected = format!("{:012}", 100_000_000_000u64 + i as u64);
            assert_eq!(
                invitation["Principal"].as_str(),
                Some(expected.as_str()),
                "case {name}: expected invitation {i} of {expected:?}, got {proposal:?}"
            );
        }
        for (i, removal) in removals.iter().enumerate() {
            assert_eq!(
                removal["MemberId"].as_str(),
                Some(member_id.as_str()),
                "case {name}: expected removal {i} of {member_id:?}, got {proposal:?}"
            );
        }

        let tags = proposal["Tags"].as_object().cloned().unwrap_or_default();
        assert_eq!(
            tags.len(),
            case.tag_count,
            "case {name}: expected {} tags back, got {proposal:?}",
            case.tag_count
        );
        for i in 0..case.tag_count {
            assert_eq!(
                tags.get(&format!("key-{i}")).and_then(Value::as_str),
                Some(format!("value-{i}").as_str()),
                "case {name}: expected tag key-{i} back, got {proposal:?}"
            );
        }

        assert!(
            is_date_time(&proposal["CreationDate"]),
            "case {name}: expected an ISO-8601 Proposal.CreationDate, got {proposal:?}"
        );
        // "ExpirationDate ... is the CreationDate plus the ProposalDurationInHours that
        // is specified in the ProposalThresholdPolicy"; a policy that specifies none
        // fixes no expiry, so none is reported.
        let created = epoch_secs(&proposal["CreationDate"]);
        let expires = epoch_secs(&proposal["ExpirationDate"]);
        match case.duration_hours {
            Some(hours) => assert_eq!(
                expires,
                created.map(|created| created + hours * 60 * 60),
                "case {name}: expected ExpirationDate {hours}h after CreationDate, got \
                 {proposal:?}"
            ),
            None => assert!(
                expires.is_none(),
                "case {name}: a voting policy with no proposal duration fixes no expiry, got \
                 {proposal:?}"
            ),
        }

        // A read reports; it must not change what the next caller sees.
        let again = get_proposal(&router, &network_id, &proposal_id).await;
        assert_eq!(
            again.body, answer.body,
            "case {name}: two reads of one proposal must answer the same, got {:?} then {:?}",
            answer.body, again.body
        );
    }
}

/// A repeated `ClientRequestToken` replays the proposal id its first call minted, and
/// changes nothing on the stored proposal; a fresh token mints a new one — over a table of
/// tokens.
#[tokio::test]
async fn a_repeated_client_request_token_replays_the_same_proposal_id() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "idem-net", "idem-net").await;

    // Rows differ along the token and along whether the retry carries the same request
    // body: "an idempotent operation completes no more than one time" either way.
    let cases: &[(&str, &str)] = &[
        ("tok-1", "same body"),
        ("tok-2", "same body"),
        ("tok-3", "different description"),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (token, retry) in cases {
        let first = create_proposal(
            &router,
            &network_id,
            &member_id,
            token,
            json!({ "Invitations": [{ "Principal": "111111111111" }] }),
            &[("Description", json!("the first body"))],
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {token}: expected CreateProposal 200, got {} {:?}",
            first.status, first.body
        );
        let proposal_id = first.text(&["ProposalId"]);
        assert!(
            !minted.contains(&proposal_id),
            "case {token}: a fresh token must mint a fresh proposal id, got a repeat of \
             {proposal_id:?}"
        );

        let description = if *retry == "same body" {
            "the first body"
        } else {
            "a different body"
        };
        let replay = create_proposal(
            &router,
            &network_id,
            &member_id,
            token,
            json!({ "Invitations": [{ "Principal": "222222222222" }] }),
            &[("Description", json!(description))],
        )
        .await;
        assert_eq!(
            (replay.status, replay.text(&["ProposalId"])),
            (200, proposal_id.clone()),
            "case {token}/{retry}: a repeated token must replay the original id, got {} {:?}",
            replay.status,
            replay.body
        );

        let stored = get_proposal(&router, &network_id, &proposal_id).await;
        assert_eq!(
            stored.text(&["Proposal", "Description"]),
            "the first body",
            "case {token}/{retry}: the replayed call must not have changed the proposal, got \
             {:?}",
            stored.body
        );
        assert_eq!(
            stored.body["Proposal"]["Actions"]["Invitations"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {token}/{retry}: the replayed call must not have added an action, got {:?}",
            stored.body
        );
        minted.push(proposal_id);
    }

    assert_eq!(
        minted.len(),
        cases.len(),
        "expected one proposal per token, got {minted:?}"
    );
}

/// A proposal the service cannot carry out is refused with the modelled error, and
/// nothing is stored for it — over a table of every way `CreateProposal` is refused.
#[tokio::test]
async fn a_proposal_that_cannot_be_carried_out_is_refused() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "refuse-net", "refuse-net").await;
    let absent_network = format!("n-{ABSENT_SUFFIX}");
    let absent_member = format!("m-{ABSENT_SUFFIX}");
    let invite = json!({ "Invitations": [{ "Principal": "111111111111" }] });

    // Rows: a network and a member id that name nothing stored; the two `Actions`
    // combinations that ask for nothing carriable ("the types of Actions in a proposal
    // are mutually exclusive", and an `Actions` naming no action at all, with and without
    // empty lists); and the request that is valid, so the table shows the refusals are
    // not refusing everything.
    let cases: &[(&str, &str, &str, Value, u16, &str)] = &[
        (
            "unknown network",
            absent_network.as_str(),
            member_id.as_str(),
            invite.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown member",
            network_id.as_str(),
            absent_member.as_str(),
            invite.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "both action kinds",
            network_id.as_str(),
            member_id.as_str(),
            json!({
                "Invitations": [{ "Principal": "111111111111" }],
                "Removals": [{ "MemberId": member_id }],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "no actions",
            network_id.as_str(),
            member_id.as_str(),
            json!({}),
            400,
            "InvalidRequestException",
        ),
        (
            "empty action lists",
            network_id.as_str(),
            member_id.as_str(),
            json!({ "Invitations": [], "Removals": [] }),
            400,
            "InvalidRequestException",
        ),
        (
            "one invitation",
            network_id.as_str(),
            member_id.as_str(),
            invite.clone(),
            200,
            "",
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, network, member, actions, expected_status, expected_error) in cases {
        let answer = create_proposal(
            &router,
            network,
            member,
            &format!("refuse-{case}"),
            actions.clone(),
            &[],
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (*expected_status, *expected_error),
            "case {case}: expected {expected_status} {expected_error:?}, got {} {:?}",
            answer.status,
            answer.body
        );
        if *expected_status == 200 {
            created.push(answer.text(&["ProposalId"]));
            continue;
        }
        assert!(
            !answer.text(&["message"]).is_empty() || !answer.text(&["Message"]).is_empty(),
            "case {case}: a refusal must say what was wrong, got {:?}",
            answer.body
        );
        assert!(
            answer.text(&["ProposalId"]).is_empty(),
            "case {case}: a refused create must mint no proposal id, got {:?}",
            answer.body
        );
    }

    // Only the one valid row was stored, and it reads back.
    assert_eq!(
        created.len(),
        1,
        "expected exactly one proposal to have been created, got {created:?}"
    );
    let answer = get_proposal(&router, &network_id, &created[0]).await;
    assert_eq!(
        answer.status, 200,
        "expected the accepted proposal to read back, got {} {:?}",
        answer.status, answer.body
    );
}

/// A network that has not settled yet has no membership to put a proposal to, so the
/// create is refused with the modelled `ResourceNotReadyException` until the clock owes
/// the transition.
#[tokio::test]
async fn a_network_still_creating_cannot_take_a_proposal() {
    let router = new_router();
    let started = std::time::Instant::now();
    let (network_id, member_id) =
        create_network(&router, create_network_body("not-ready", "not-ready-net")).await;
    let invite = json!({ "Invitations": [{ "Principal": "111111111111" }] });

    let answer = create_proposal(
        &router,
        &network_id,
        &member_id,
        "while-creating",
        invite.clone(),
        &[],
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (409, "ResourceNotReadyException"),
        "case CreateProposal while CREATING: expected 409 ResourceNotReadyException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert!(
        started.elapsed() < TRANSITION_SPAN,
        "case CreateProposal while CREATING: the transient status must outlive the create call \
         ({TRANSITION_SPAN:?}), but {:?} had already passed",
        started.elapsed()
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = create_proposal(
        &router,
        &network_id,
        &member_id,
        "after-settling",
        invite,
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case CreateProposal once AVAILABLE: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// `GetProposal` is scoped to the network that holds the proposal: an id that names
/// nothing on that network is the modelled `ResourceNotFoundException` — over a table of
/// ids, including a proposal that exists on a different network.
#[tokio::test]
async fn a_proposal_id_that_names_nothing_on_the_network_is_a_404() {
    let router = new_router();
    let (first_network, first_member) = available_network(&router, "scope-a", "scope-a").await;
    let (second_network, _) = available_network(&router, "scope-b", "scope-b").await;

    let answer = create_proposal(
        &router,
        &first_network,
        &first_member,
        "scoped",
        json!({ "Invitations": [{ "Principal": "111111111111" }] }),
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected CreateProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    let proposal_id = answer.text(&["ProposalId"]);

    // Rows: an unknown network holding a real proposal id, a real network holding an
    // unknown proposal id, and the other network holding a proposal that is not its own.
    let cases: &[(&str, String, String)] = &[
        (
            "unknown network",
            format!("n-{ABSENT_SUFFIX}"),
            proposal_id.clone(),
        ),
        (
            "unknown proposal",
            first_network.clone(),
            format!("p-{ABSENT_SUFFIX}"),
        ),
        (
            "another network's proposal",
            second_network.clone(),
            proposal_id.clone(),
        ),
    ];

    for (case, network, proposal) in cases {
        let answer = get_proposal(&router, network, proposal).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // The proposal still reads back on its own network.
    let answer = get_proposal(&router, &first_network, &proposal_id).await;
    assert_eq!(
        answer.status, 200,
        "expected the proposal to read back on its own network, got {} {:?}",
        answer.status, answer.body
    );
}

/// A proposal is `IN_PROGRESS` until its `ExpirationDate` passes, and `EXPIRED` after —
/// a transition the CLOCK owes, so a read materialises it although no call closed the
/// proposal, and it changes nothing else on the record.
#[tokio::test]
async fn a_proposal_past_its_expiration_date_is_expired() {
    // The router over an engine this test keeps a handle on, so it can reach the very
    // record the handlers read: the shortest duration the model allows is an hour
    // (`ProposalDurationInHours` is @range(1, 168)), so the stored expiry is backdated
    // rather than waited out.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let (network_id, member_id) =
        create_network(&router, network_body("expiry", "expiry-net", Some(1))).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let answer = create_proposal(
        &router,
        &network_id,
        &member_id,
        "expiring",
        json!({ "Invitations": [{ "Principal": "111111111111" }] }),
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected CreateProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    let proposal_id = answer.text(&["ProposalId"]);

    let answer = get_proposal(&router, &network_id, &proposal_id).await;
    assert_eq!(
        answer.text(&["Proposal", "Status"]),
        "IN_PROGRESS",
        "case before the expiry falls due: expected the proposal still open, got {:?}",
        answer.body
    );

    let backdated = SystemTime::now() - Duration::from_secs(1);
    {
        let state = service_state(&*engine).expect("this service's own store");
        let mut guard = state.lock();
        let proposal = guard
            .networks
            .get_mut(&network_id)
            .and_then(|network| network.proposals.get_mut(&proposal_id))
            .expect("the proposal the create persisted");
        proposal.expiration_date = Some(backdated);
    }

    let answer = get_proposal(&router, &network_id, &proposal_id).await;
    assert_eq!(
        answer.text(&["Proposal", "Status"]),
        PROPOSAL_EXPIRED,
        "case once the expiry has passed: expected the proposal {PROPOSAL_EXPIRED}, got {:?}",
        answer.body
    );
    let proposal = &answer.body["Proposal"];
    // "The specified ProposalActions aren't carried out" — nothing else moved: the
    // actions, the vote counts and the outstanding count are the stored ones.
    assert_eq!(
        (
            proposal["Actions"]["Invitations"].as_array().map(Vec::len),
            proposal["YesVoteCount"].as_i64(),
            proposal["NoVoteCount"].as_i64(),
            proposal["OutstandingVoteCount"].as_i64()
        ),
        (Some(1), Some(0), Some(0), Some(1)),
        "case once the expiry has passed: the expiry must change only the status, got \
         {proposal:?}"
    );

    // And it stays expired: the transition is materialised once, not re-decided per read.
    let again = get_proposal(&router, &network_id, &proposal_id).await;
    assert_eq!(
        again.body, answer.body,
        "case a second read after the expiry: expected the same answer, got {:?} then {:?}",
        answer.body, again.body
    );
}

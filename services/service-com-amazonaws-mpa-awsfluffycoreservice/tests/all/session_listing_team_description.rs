// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one member a `ListSessions` entry does NOT read off the
//! session record: `Description`.
//!
//! The rule under test: a response member reports the value the model documents THAT
//! member to carry, read off the record that owns it — so
//! `ListSessionsResponseSession$Description` ("Description for the team.") is the
//! description of the approval team whose sessions are listed, the same value
//! `GetApprovalTeam` and `ListApprovalTeams` report for that team, while
//! `GetSessionResponse$Description` ("Description for the session.") stays the session's
//! own and is absent, because no operation of this model records one.
//!
//! The consequences that follow, each a test below:
//!
//! * EVERY entry of EVERY page of a team's listing carries that team's description,
//!   whatever the description's form and however the listing is paged;
//! * a team's listing carries ITS team's description, not another team's, when several
//!   teams with different descriptions are stored;
//! * the entry is read off the team record as it stands at the call, so a change the
//!   team's approvers have not approved yet does not show up in the listing and the
//!   approved one does.
//!
//! The table spans the class the rule generalises over (the description's length — both
//! published bounds of the `Description` shape — its character content, how many sessions
//! the team has and how wide a page the client asks for), not one recorded shape.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_described_team, get_session, get_team, wait_for_the_clock};
use crate::approval_team_lifecycle::{text, Harness};
use crate::session_listing::{entries, list_sessions, page_through, start_sessions};

/// The shared `Description` shape is `length(min: 1, max: 256)` — the minimum.
const MIN_DESCRIPTION: usize = 1;

/// The shared `Description` shape is `length(min: 1, max: 256)` — the maximum.
const MAX_DESCRIPTION: usize = 256;

/// One row of the description table.
struct DescriptionCase {
    case: &'static str,
    team_name: &'static str,
    /// The team's `Description`, which `CreateApprovalTeam` requires.
    description: String,
    /// How many sessions the team is given.
    sessions: usize,
    /// The `MaxResults` each page asks for, or `None` to leave the member absent.
    page_size: Option<i64>,
}

/// The description table: rows differ along the description's length (both published
/// bounds), its character content, the number of sessions on the team and the width of a
/// page — so the member is asserted across pages, not only on a first page of one.
fn description_cases() -> Vec<DescriptionCase> {
    vec![
        DescriptionCase {
            case: "the shortest description, one session",
            team_name: "described-solo",
            description: "d".repeat(MIN_DESCRIPTION),
            sessions: 1,
            // `MaxResults` is `range(min: 1, …)` — the narrowest page.
            page_size: Some(1),
        },
        DescriptionCase {
            case: "a sentence of words, three sessions over two pages",
            team_name: "Described.Team_2",
            description: "Approves production changes (tier 1), 24/7.".to_string(),
            sessions: 3,
            page_size: Some(2),
        },
        DescriptionCase {
            case: "a description outside ASCII, MaxResults absent",
            team_name: "d",
            description: "équipe d'approbation — 承認チーム".to_string(),
            sessions: 2,
            page_size: None,
        },
        DescriptionCase {
            case: "the longest description, five sessions over three pages",
            team_name: "described-two-pages",
            description: "x".repeat(MAX_DESCRIPTION),
            sessions: 5,
            page_size: Some(2),
        },
    ]
}

/// The `Description` a reported entry carries, or `None` when it reports none.
fn described(entry: &Value) -> Option<String> {
    entry
        .get("Description")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `ListApprovalTeams` entry for one team ARN.
async fn listed_team(harness: &Harness, team_arn: &str) -> Value {
    let (status, page) = harness.list("MaxResults=20").await;
    assert_eq!(
        status, 200,
        "case {team_arn:?}: ListApprovalTeams returned {status} with {page:?}"
    );
    page["ApprovalTeams"]
        .as_array()
        .and_then(|teams| {
            teams
                .iter()
                .find(|team| text(team, "Arn") == team_arn)
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {team_arn:?}: the team is not listed: {page:?}"))
}

/// Every entry of every page reports the listed team's description — the value the
/// sibling team reads report — while `GetSession` reports no session description.
#[tokio::test]
async fn every_listed_session_reports_the_team_s_description() {
    let harness = Harness::new();

    for case in description_cases() {
        let label = case.case;
        let team_arn =
            active_described_team(&harness, case.team_name, 2, 1, &case.description).await;
        let started = start_sessions(&harness, &team_arn, case.sessions, label).await;

        // The value the member is documented to carry, as the two team reads report it.
        let (status, team) = get_team(&harness, &team_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetApprovalTeam returned {status} with {team:?}"
        );
        let expected = text(&team, "Description");
        assert_eq!(
            expected, case.description,
            "case {label:?}: expected the created description {:?}, got {expected:?}",
            case.description
        );
        let from_listing = text(&listed_team(&harness, &team_arn).await, "Description");
        assert_eq!(
            from_listing, expected,
            "case {label:?}: expected ListApprovalTeams to agree with GetApprovalTeam on \
             {expected:?}, got {from_listing:?}"
        );

        let collected = page_through(&harness, &team_arn, case.page_size, label).await;
        assert_eq!(
            collected.len(),
            started.len(),
            "case {label:?}: expected {} entries, got {}",
            started.len(),
            collected.len()
        );
        for entry in &collected {
            let listed = described(entry);
            assert_eq!(
                listed.as_deref(),
                Some(expected.as_str()),
                "case {label:?}: expected the team's description {expected:?}, got {listed:?} \
                 in {entry:?}"
            );

            // The single-session read's `Description` is documented as the SESSION's, and
            // no operation of this model records one, so it stays absent.
            let session_arn = text(entry, "SessionArn");
            let (status, session) = get_session(&harness, &session_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetSession({session_arn}) returned {status} with {session:?}"
            );
            let session_description = described(&session);
            assert!(
                session_description.is_none(),
                "case {label:?}: GetSession's Description is the session's own, which nothing \
                 records; got {session_description:?} in {session:?}"
            );
        }
    }
}

/// With several teams stored, each team's listing carries that team's description.
#[tokio::test]
async fn a_listing_carries_the_description_of_the_team_it_is_scoped_to() {
    let harness = Harness::new();

    let rows = [
        ("scoped-first", "the first team's description".to_string()),
        ("scoped.second_2", "second".to_string()),
        ("s", "x".repeat(MAX_DESCRIPTION)),
    ];
    let mut teams = Vec::new();
    for (name, description) in &rows {
        let team_arn = active_described_team(&harness, name, 1, 1, description).await;
        start_sessions(&harness, &team_arn, 2, name).await;
        teams.push((name, description.clone(), team_arn));
    }

    for (name, description, team_arn) in &teams {
        let (status, page) = list_sessions(&harness, team_arn, json!({ "MaxResults": 20 })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the listing returned {status} with {page:?}"
        );
        let page_entries = entries(&page);
        assert!(
            !page_entries.is_empty(),
            "case {name:?}: the team's sessions are not listed: {page:?}"
        );
        for entry in &page_entries {
            let listed = described(entry);
            assert_eq!(
                listed.as_deref(),
                Some(description.as_str()),
                "case {name:?}: expected this team's description {description:?}, got {listed:?} \
                 in {entry:?}"
            );
        }
    }
}

/// The entry is read off the team record as it stands at the call: a change an active
/// team's approvers have not approved does not reach the listing, and the approved one
/// does.
#[tokio::test]
async fn the_listed_description_follows_the_team_record_through_a_change() {
    let harness = Harness::new();
    let first = "the description the team was created with";
    let approved = "the description the team approved";
    let team_arn = active_described_team(&harness, "described-update", 2, 1, first).await;
    start_sessions(&harness, &team_arn, 1, "before the change").await;

    let (status, requested) = harness
        .update(&team_arn, &json!({ "Description": approved }))
        .await;
    assert_eq!(
        status, 200,
        "the update returned {status} with {requested:?}"
    );

    // Straight after the call the change is only REQUESTED, so the team — and with it the
    // listing — keeps the description it carries.
    assert_described(&harness, &team_arn, "a change the team has not approved", first).await;

    // The clock alone carries a requested change through its approval and then its
    // activation; a call is what materialises each.
    wait_for_the_clock(state::UPDATE_APPROVAL_DELAY);
    let (status, _) = get_team(&harness, &team_arn).await;
    assert_eq!(status, 200, "the approval read returned {status}");
    wait_for_the_clock(state::UPDATE_ACTIVATION_DELAY);

    assert_described(
        &harness,
        &team_arn,
        "the change once the clock carried it through",
        approved,
    )
    .await;
}

/// Assert that the team carries `expected` as its description and that every entry of its
/// session listing reports that same value.
async fn assert_described(harness: &Harness, team_arn: &str, case: &str, expected: &str) {
    let (status, team) = get_team(harness, team_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApprovalTeam returned {status} with {team:?}"
    );
    let live = text(&team, "Description");
    assert_eq!(
        live, expected,
        "case {case:?}: expected the team to carry {expected:?}, got {live:?}"
    );

    let (status, page) = list_sessions(harness, team_arn, json!({ "MaxResults": 20 })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the listing returned {status} with {page:?}"
    );
    let page_entries = entries(&page);
    assert!(
        !page_entries.is_empty(),
        "case {case:?}: the team's sessions are not listed: {page:?}"
    );
    for entry in &page_entries {
        let listed = described(entry);
        assert_eq!(
            listed.as_deref(),
            Some(expected),
            "case {case:?}: expected the team record's description {expected:?}, got {listed:?} \
             in {entry:?}"
        );
    }
}

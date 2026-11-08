// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMembers`.
//!
//! The rule under test: a member listing reports one `MemberSummary` per party the
//! collaboration holds a grant for — its creator first, then its declared members, in
//! declaration order — and every member of a summary is derived from the stored
//! collaboration: the grant that party was declared with (display name, abilities, ML
//! abilities, the payment responsibilities the creator set for it), the status it holds
//! (ACTIVE once that account has joined with a membership, INVITED while it has not),
//! that membership's id and ARN once it exists, and that party's own
//! `createTime`/`updateTime`. The listing pages in that same order through
//! `maxResults`/`nextToken`, and a collaboration the caller is no party to is refused
//! rather than reported on.
//!
//! The tables vary what the rule generalises over: how many parties a collaboration
//! holds, which abilities each one was granted (the whole `MemberAbility` catalog),
//! whether a party holds ML abilities, whether the creator configured a party's payment
//! responsibilities, whether a party has joined, the page size (both published bounds of
//! `MaxResults` and one past each) and whether a party was declared at creation or added
//! by a committed change request. Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, minimal_collaboration,
    Emulator, ALL_MEMBER_ABILITIES, OTHER_CREATOR_ACCOUNT, REGION,
};
use super::collaboration_change_requests::{
    create_change_request, declared_member, member_change, NEWCOMER_ACCOUNT, PARTNER_ACCOUNT,
};

/// The published bounds of `ListMembers$maxResults` (model shape `MaxResults`,
/// range 1..100).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// Two further 12-digit `AccountId` values (model shape `AccountId`, len 12..12, pattern
/// `^\d+$`), so a collaboration in these tables can hold more parties than one page.
const THIRD_ACCOUNT: &str = "555566667777";
const FOURTH_ACCOUNT: &str = "888899990000";

/// Every `CustomMLMemberAbility` the model declares (shape `CustomMLMemberAbility`), so
/// the ML row covers the catalog rather than one value.
const ALL_CUSTOM_ML_ABILITIES: [&str; 2] =
    ["CAN_RECEIVE_MODEL_OUTPUT", "CAN_RECEIVE_INFERENCE_OUTPUT"];

/// `ListMembers` over the wire, with `query` appended to the path (`""` for none).
async fn list_members(emulator: &Emulator, collaboration_id: &str, query: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/collaborations/{collaboration_id}/members{query}"),
            None,
        )
        .await
}

/// The `memberSummaries` a listing carries.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["memberSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected memberSummaries, got {body}"))
        .clone()
}

/// The account ids a listing reports, in the order it reported them.
fn accounts(case: &str, body: &Value) -> Vec<String> {
    summaries(case, body)
        .iter()
        .map(|summary| {
            summary["accountId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary carries an accountId"))
                .to_string()
        })
        .collect()
}

/// One timestamp member of a summary, as epoch seconds.
fn timestamp(case: &str, summary: &Value, member: &str) -> f64 {
    summary[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected an epoch-seconds {member}, got {summary}")
    })
}

/// Create a collaboration for one row: `minimal_collaboration` with the creator abilities
/// and declared members the row prescribes. Returns its id and the created collaboration.
async fn collaboration_with(
    emulator: &Emulator,
    case: &str,
    creator_abilities: &[&str],
    members: Value,
) -> (String, Value) {
    let mut body = minimal_collaboration(case);
    body["creatorMemberAbilities"] = json!(creator_abilities);
    body["members"] = members;
    let (status, created) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration = created["collaboration"].clone();
    let id = collaboration["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created collaboration carries an id"))
        .to_string();
    (id, collaboration)
}

/// What the rule says one party's summary must report.
struct ExpectedMember {
    account_id: String,
    display_name: &'static str,
    abilities: Vec<&'static str>,
    ml_abilities: Option<Vec<&'static str>>,
    /// Whether this party is the one responsible for the collaboration's query compute
    /// costs.
    pays_query_compute: bool,
}

/// One row of the roster table.
struct RosterCase {
    case: &'static str,
    creator_abilities: Vec<&'static str>,
    members: Value,
    expect: Vec<ExpectedMember>,
}

/// A declared member carrying `abilities` plus the extra members `extra` states.
fn declared_member_with(account: &str, abilities: &[&str], extra: Value) -> Value {
    let mut member = declared_member(account, abilities);
    for (key, value) in extra.as_object().expect("an object of extra members") {
        member[key.clone()] = value.clone();
    }
    member
}

#[tokio::test]
async fn a_member_listing_reports_every_party_of_the_stored_collaboration() {
    let creator = account_id().to_string();

    let cases = vec![
        RosterCase {
            case: "the creator alone, able to query",
            creator_abilities: vec![ALL_MEMBER_ABILITIES[0]],
            members: json!([]),
            expect: vec![ExpectedMember {
                account_id: creator.clone(),
                display_name: "creator",
                abilities: vec![ALL_MEMBER_ABILITIES[0]],
                ml_abilities: None,
                pays_query_compute: true,
            }],
        },
        RosterCase {
            case: "the creator cannot query and the declared member can",
            creator_abilities: vec![ALL_MEMBER_ABILITIES[1]],
            members: json!([declared_member(PARTNER_ACCOUNT, &[ALL_MEMBER_ABILITIES[0]])]),
            expect: vec![
                ExpectedMember {
                    account_id: creator.clone(),
                    display_name: "creator",
                    abilities: vec![ALL_MEMBER_ABILITIES[1]],
                    ml_abilities: None,
                    pays_query_compute: false,
                },
                ExpectedMember {
                    account_id: PARTNER_ACCOUNT.to_string(),
                    display_name: "partner",
                    abilities: vec![ALL_MEMBER_ABILITIES[0]],
                    ml_abilities: None,
                    pays_query_compute: true,
                },
            ],
        },
        RosterCase {
            case: "three parties spanning every ability the catalog names",
            creator_abilities: vec![ALL_MEMBER_ABILITIES[0], ALL_MEMBER_ABILITIES[1]],
            members: json!([
                declared_member(PARTNER_ACCOUNT, &[ALL_MEMBER_ABILITIES[2]]),
                declared_member(NEWCOMER_ACCOUNT, &[ALL_MEMBER_ABILITIES[3]]),
            ]),
            expect: vec![
                ExpectedMember {
                    account_id: creator.clone(),
                    display_name: "creator",
                    abilities: vec![ALL_MEMBER_ABILITIES[0], ALL_MEMBER_ABILITIES[1]],
                    ml_abilities: None,
                    pays_query_compute: true,
                },
                ExpectedMember {
                    account_id: PARTNER_ACCOUNT.to_string(),
                    display_name: "partner",
                    abilities: vec![ALL_MEMBER_ABILITIES[2]],
                    ml_abilities: None,
                    pays_query_compute: false,
                },
                ExpectedMember {
                    account_id: NEWCOMER_ACCOUNT.to_string(),
                    display_name: "partner",
                    abilities: vec![ALL_MEMBER_ABILITIES[3]],
                    ml_abilities: None,
                    pays_query_compute: false,
                },
            ],
        },
        RosterCase {
            case: "the creator made a declared member responsible for query compute",
            creator_abilities: vec![ALL_MEMBER_ABILITIES[0]],
            members: json!([declared_member_with(
                PARTNER_ACCOUNT,
                &[ALL_MEMBER_ABILITIES[0]],
                json!({ "paymentConfiguration": { "queryCompute": { "isResponsible": true } } }),
            )]),
            expect: vec![
                ExpectedMember {
                    account_id: creator.clone(),
                    display_name: "creator",
                    abilities: vec![ALL_MEMBER_ABILITIES[0]],
                    ml_abilities: None,
                    pays_query_compute: false,
                },
                ExpectedMember {
                    account_id: PARTNER_ACCOUNT.to_string(),
                    display_name: "partner",
                    abilities: vec![ALL_MEMBER_ABILITIES[0]],
                    ml_abilities: None,
                    pays_query_compute: true,
                },
            ],
        },
        RosterCase {
            case: "a declared member granted every ML ability, holding no member ability",
            creator_abilities: vec![ALL_MEMBER_ABILITIES[0]],
            members: json!([declared_member_with(
                PARTNER_ACCOUNT,
                &[],
                json!({
                    "mlMemberAbilities": {
                        "customMLMemberAbilities": ALL_CUSTOM_ML_ABILITIES,
                    }
                }),
            )]),
            expect: vec![
                ExpectedMember {
                    account_id: creator.clone(),
                    display_name: "creator",
                    abilities: vec![ALL_MEMBER_ABILITIES[0]],
                    ml_abilities: None,
                    pays_query_compute: true,
                },
                ExpectedMember {
                    account_id: PARTNER_ACCOUNT.to_string(),
                    display_name: "partner",
                    abilities: vec![],
                    ml_abilities: Some(ALL_CUSTOM_ML_ABILITIES.to_vec()),
                    pays_query_compute: false,
                },
            ],
        },
    ];

    for case in cases {
        let label = case.case;
        let emulator = Emulator::new();
        let (collaboration_id, collaboration) =
            collaboration_with(&emulator, label, &case.creator_abilities, case.members).await;

        let (status, body) = list_members(&emulator, &collaboration_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListMembers 200, got {status} with {body}"
        );
        let listed = summaries(label, &body);
        assert_eq!(
            listed.len(),
            case.expect.len(),
            "case {label:?}: expected {} parties, got {body}",
            case.expect.len()
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {label:?}: a complete page must carry no nextToken, got {:?}",
            body.get("nextToken")
        );

        for (expected, summary) in case.expect.iter().zip(&listed) {
            let who = format!("{label:?} / member {}", expected.account_id);
            assert_eq!(
                summary["accountId"].as_str(),
                Some(expected.account_id.as_str()),
                "case {who}: parties are reported creator-first in declaration order, got {summary}"
            );
            assert_eq!(
                summary["displayName"].as_str(),
                Some(expected.display_name),
                "case {who}: expected the declared display name, got {summary}"
            );
            assert_eq!(
                summary["abilities"],
                json!(expected.abilities),
                "case {who}: expected the declared abilities {:?}, got {summary}",
                expected.abilities
            );
            match &expected.ml_abilities {
                Some(ml) => assert_eq!(
                    summary["mlAbilities"],
                    json!({ "customMLMemberAbilities": ml }),
                    "case {who}: expected the declared ML abilities {ml:?}, got {summary}"
                ),
                None => assert!(
                    summary.get("mlAbilities").is_none(),
                    "case {who}: a party granted no ML abilities reports none, got {summary}"
                ),
            }
            assert_eq!(
                summary["status"].as_str(),
                Some("INVITED"),
                "case {who}: a party that has not joined is INVITED, got {summary}"
            );
            for absent in ["membershipId", "membershipArn"] {
                assert!(
                    summary.get(absent).is_none(),
                    "case {who}: a party holding no membership reports no {absent}, got {summary}"
                );
            }
            assert_eq!(
                summary["paymentConfiguration"],
                json!({ "queryCompute": { "isResponsible": expected.pays_query_compute } }),
                "case {who}: expected queryCompute.isResponsible {}, got {summary}",
                expected.pays_query_compute
            );
            // Every party here was declared when the collaboration was created, so its
            // member record is as old as the collaboration and has never been updated.
            assert_eq!(
                summary["createTime"], collaboration["createTime"],
                "case {who}: a party declared at creation carries the collaboration's createTime {}, got {summary}",
                collaboration["createTime"]
            );
            assert_eq!(
                summary["updateTime"], summary["createTime"],
                "case {who}: a party never updated reports updateTime == createTime, got {summary}"
            );
        }
    }
}

#[tokio::test]
async fn a_party_that_joins_is_reported_active_with_its_membership() {
    // Rows differ in how many other parties the collaboration declares, so the status of
    // the account that joins is reported against rosters of different sizes.
    let rosters = [
        ("no other party", json!([])),
        (
            "one other party",
            json!([declared_member(PARTNER_ACCOUNT, &[ALL_MEMBER_ABILITIES[1]])]),
        ),
        (
            "two other parties",
            json!([
                declared_member(PARTNER_ACCOUNT, &[ALL_MEMBER_ABILITIES[1]]),
                declared_member(NEWCOMER_ACCOUNT, &[ALL_MEMBER_ABILITIES[2]]),
            ]),
        ),
    ];

    for (label, members) in rosters {
        let emulator = Emulator::new();
        let (collaboration_id, _) = collaboration_with(
            &emulator,
            label,
            &[ALL_MEMBER_ABILITIES[0]],
            members.clone(),
        )
        .await;

        let (status, before) = list_members(&emulator, &collaboration_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListMembers 200, got {status} with {before}"
        );
        let created_at = timestamp(label, &summaries(label, &before)[0], "createTime");

        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateMembership 200, got {status} with {joined}"
        );
        let membership_id = joined["membership"]["id"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: the created membership carries an id: {joined}")
        });
        let membership_arn = joined["membership"]["arn"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: the created membership carries an arn: {joined}")
        });

        let (status, after) = list_members(&emulator, &collaboration_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListMembers 200, got {status} with {after}"
        );
        let listed = summaries(label, &after);
        assert_eq!(
            listed.len(),
            1 + members.as_array().expect("the declared members").len(),
            "case {label:?}: joining changes no party count, got {after}"
        );

        let caller = &listed[0];
        assert_eq!(
            caller["accountId"].as_str(),
            Some(account_id()),
            "case {label:?}: the creator is the first party reported, got {after}"
        );
        assert_eq!(
            caller["status"].as_str(),
            Some("ACTIVE"),
            "case {label:?}: a party that holds a membership is ACTIVE, got {after}"
        );
        assert_eq!(
            caller["membershipId"].as_str(),
            Some(membership_id),
            "case {label:?}: expected the membership just created, got {after}"
        );
        assert_eq!(
            caller["membershipArn"].as_str(),
            Some(membership_arn),
            "case {label:?}: expected the ARN of the membership just created, got {after}"
        );
        assert_eq!(
            timestamp(label, caller, "createTime"),
            created_at,
            "case {label:?}: joining must not move a party's createTime, got {after}"
        );

        for other in &listed[1..] {
            assert_eq!(
                other["status"].as_str(),
                Some("INVITED"),
                "case {label:?}: a party that has not joined stays INVITED, got {after}"
            );
            assert!(
                other.get("membershipId").is_none(),
                "case {label:?}: a party that has not joined reports no membershipId, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_party_a_committed_change_request_adds_carries_its_own_timestamps() {
    let emulator = Emulator::new();
    // A collaboration whose only party is the caller: it is the only account whose
    // approval a change request needs, so the request commits on creation and the member
    // it adds becomes a party straight away.
    let (collaboration_id, collaboration) =
        collaboration_with(&emulator, "change-request-add", &[ALL_MEMBER_ABILITIES[0]], json!([]))
            .await;

    let (status, request) = create_change_request(
        &emulator,
        &collaboration_id,
        json!([member_change(PARTNER_ACCOUNT, &[ALL_MEMBER_ABILITIES[1]])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case add-member: expected CreateCollaborationChangeRequest 200, got {status} with {request}"
    );
    assert_eq!(
        request["collaborationChangeRequest"]["status"].as_str(),
        Some("COMMITTED"),
        "case add-member: the only party's request commits on creation, got {request}"
    );

    let (status, body) = list_members(&emulator, &collaboration_id, "").await;
    assert_eq!(
        status, 200,
        "case add-member: expected ListMembers 200, got {status} with {body}"
    );
    let listed = summaries("add-member", &body);
    assert_eq!(
        accounts("add-member", &body),
        vec![account_id().to_string(), PARTNER_ACCOUNT.to_string()],
        "case add-member: the added party is reported after the creator, got {body}"
    );

    let creator = &listed[0];
    let added = &listed[1];
    assert_eq!(
        added["abilities"],
        json!([ALL_MEMBER_ABILITIES[1]]),
        "case add-member: the added party holds the abilities the change stated, got {body}"
    );
    assert_eq!(
        added["status"].as_str(),
        Some("INVITED"),
        "case add-member: a party that has not joined is INVITED, got {body}"
    );
    assert_eq!(
        timestamp("add-member", added, "createTime"),
        timestamp("add-member", added, "updateTime"),
        "case add-member: a party just added reports updateTime == createTime, got {body}"
    );
    assert!(
        timestamp("add-member", added, "createTime")
            >= timestamp("add-member", creator, "createTime"),
        "case add-member: the added party cannot predate the creator, got {body}"
    );
    // The change named one account, so the OTHER party's member record did not change —
    // even though the collaboration itself was updated by the commit.
    assert_eq!(
        creator["createTime"], collaboration["createTime"],
        "case add-member: the creator keeps the collaboration's createTime, got {body}"
    );
    assert_eq!(
        creator["updateTime"], creator["createTime"],
        "case add-member: a party no change named keeps its updateTime, got {body}"
    );
}

#[tokio::test]
async fn a_member_listing_pages_every_party_exactly_once() {
    let emulator = Emulator::new();
    // Five parties, so the page sizes below divide the set evenly and unevenly.
    let declared = [
        PARTNER_ACCOUNT,
        NEWCOMER_ACCOUNT,
        THIRD_ACCOUNT,
        FOURTH_ACCOUNT,
    ];
    let (collaboration_id, _) = collaboration_with(
        &emulator,
        "paging",
        &[ALL_MEMBER_ABILITIES[0]],
        json!(declared
            .iter()
            .map(|account| declared_member(account, &[ALL_MEMBER_ABILITIES[1]]))
            .collect::<Vec<Value>>()),
    )
    .await;
    let expected: Vec<String> = std::iter::once(account_id().to_string())
        .chain(declared.iter().map(|account| account.to_string()))
        .collect();

    // Rows differ in page size, including both published bounds of `maxResults`.
    for max_results in [MAX_RESULTS_MIN, 2, 3, 5, 6, MAX_RESULTS_MAX] {
        let case = format!("maxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
                None => format!("?maxResults={max_results}"),
            };
            let (status, body) = list_members(&emulator, &collaboration_id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListMembers 200, got {status} with {body}"
            );
            let page = summaries(&case, &body);
            assert!(
                page.len() as i64 <= max_results,
                "case {case:?}: a page carried {} rows, more than asked",
                page.len()
            );
            seen.extend(accounts(&case, &body));
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(pages <= 20, "case {case:?}: pagination did not terminate");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every party once, creator first in declaration order"
        );
    }

    // Unpaged: the whole set, and no token.
    let (status, body) = list_members(&emulator, &collaboration_id, "").await;
    assert_eq!(
        status, 200,
        "case unpaged: expected ListMembers 200, got {status} with {body}"
    );
    assert_eq!(
        accounts("unpaged", &body),
        expected,
        "case unpaged: expected every party in declaration order, got {body}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case unpaged: a complete page must carry no nextToken, got {:?}",
        body.get("nextToken")
    );

    // One past each published bound of `MaxResults` is refused, and each bound itself is
    // accepted.
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) =
            list_members(&emulator, &collaboration_id, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={max_results}: expected a 400 refusal, got {status} with {body}"
        );
    }
    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) =
            list_members(&emulator, &collaboration_id, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={max_results}: a published bound must be accepted, got {status} with {body}"
        );
    }

    // A token this listing never handed out names no row to resume at.
    for token in ["not-hex", "6e6f742d612d6b6e6f776e2d6964", ""] {
        let (status, body) =
            list_members(&emulator, &collaboration_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case nextToken={token:?}: expected a 400 refusal, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case nextToken={token:?}: expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_member_listing_refuses_a_collaboration_the_caller_is_no_party_to() {
    let emulator = Emulator::new();

    // A collaboration another account owns that this one is no party to, and two
    // well-formed identifiers no create minted: all three are one answer, because a
    // caller's authorization to a collaboration lives in the collaboration itself.
    let foreign = collaboration_owned_elsewhere(&emulator, "members-foreign", None).await;
    for (why, collaboration_id) in [
        ("owned by another account", foreign),
        (
            "never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "never minted either",
            "0123456f-89ab-cdef-0123-456789abcdef".to_string(),
        ),
    ] {
        let (status, body) = list_members(&emulator, &collaboration_id, "").await;
        assert_eq!(
            status, 403,
            "case {why:?}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {why:?}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "User: arn:aws:iam::{}:user/emulator is not authorized to perform: \
                 cleanrooms:ListMembers on resource: \
                 arn:aws:cleanrooms:{REGION}:{}:collaboration/{collaboration_id}",
                account_id(),
                account_id()
            ),
            "case {why:?}: expected the authorization-failure wording, got {body}"
        );
    }

    // A party that is not the creator sees the listing, creator first.
    let participant =
        collaboration_owned_elsewhere(&emulator, "members-participant", Some(&["CAN_QUERY"])).await;
    let (status, body) = list_members(&emulator, &participant, "").await;
    assert_eq!(
        status, 200,
        "case participant: expected ListMembers 200, got {status} with {body}"
    );
    assert_eq!(
        accounts("participant", &body),
        vec![OTHER_CREATOR_ACCOUNT.to_string(), account_id().to_string()],
        "case participant: expected the other creator first, then this account, got {body}"
    );

    // An identifier outside the modelled form is refused at the boundary, before any
    // authorization decision.
    for malformed in ["not-a-uuid", "0123456F-89AB-CDEF-0123-456789ABCDEF"] {
        let (status, body) = list_members(&emulator, malformed, "").await;
        assert_eq!(
            status, 400,
            "case {malformed:?}: expected a 400 refusal, got {status} with {body}"
        );
    }
}

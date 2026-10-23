// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the behavior-graph and member-account slice of this
//! service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! behavior-graph record, so anything `CreateGraph` or `CreateMembers` stored is what
//! a later `ListGraphs`, `ListMembers` or `GetMembers` reports, and anything never
//! stored (or since deleted) is a modelled 404 or an `UnprocessedAccount` — never a
//! fabricated member.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    GRAPH_ID_LEN, MAX_PAGE_SIZE, MEMBER_VERIFICATION_MILLIS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, assert_iso8601, call, create_graph_ok, emulator, error_code, ACCOUNT,
    REGION,
};

/// Assert `arn` satisfies the modelled `GraphArn` pattern
/// `^arn:aws[-\w]{0,10}?:detective:[-\w]{2,20}?:\d{12}?:graph:[abcdef\d]{32}?$` for
/// `region`, and answer its 32-character id segment.
fn assert_graph_arn(case: &str, region: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:detective:{region}:{ACCOUNT}:graph:");
    let id = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN starting {prefix}, got {arn:?}"));
    assert_eq!(
        id.len(),
        GRAPH_ID_LEN,
        "case {case:?}: expected a {GRAPH_ID_LEN}-character id segment, got {id:?}"
    );
    assert!(
        id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "case {case:?}: expected an [abcdef0-9] id segment, got {id:?}"
    );
    id.to_string()
}

/// The `GraphList` of one `ListGraphs` page.
fn graph_list(body: &Value) -> &Vec<Value> {
    body.get("GraphList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListGraphs carried no GraphList, got {body:?}"))
}

/// Walk every `ListGraphs` page at `max_results`, answering the ARNs in the order
/// they were served. Panics if the listing does not terminate well inside the number
/// of pages the store can produce.
async fn all_graph_arns(router: &MultiServiceRouter, max_results: Option<i32>) -> Vec<String> {
    let mut arns = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..1_000 {
        let mut body = Map::new();
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let (status, page) = call(router, "/graphs/list", &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "ListGraphs(max_results={max_results:?}): expected 200, got {status} {page:?}"
        );
        for graph in graph_list(&page) {
            arns.push(
                graph
                    .get("Arn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("a listed graph carried no Arn, got {graph:?}"))
                    .to_string(),
            );
        }
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return arns;
        }
    }
    panic!("ListGraphs(max_results={max_results:?}) never ran out of pages");
}

/// Walk every `ListMembers` page of `graph_arn` at `max_results`.
async fn all_member_ids(
    router: &MultiServiceRouter,
    graph_arn: &str,
    max_results: Option<i32>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..1_000 {
        let mut body = Map::new();
        body.insert("GraphArn".to_string(), json!(graph_arn));
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let (status, page) = call(router, "/graph/members/list", &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "ListMembers(max_results={max_results:?}): expected 200, got {status} {page:?}"
        );
        let details = page
            .get("MemberDetails")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("ListMembers carried no MemberDetails, got {page:?}"));
        for member in details {
            ids.push(
                member
                    .get("AccountId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("a listed member carried no AccountId, got {member:?}"))
                    .to_string(),
            );
        }
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return ids;
        }
    }
    panic!("ListMembers(max_results={max_results:?}) never ran out of pages");
}

/// A row of the create/read-back table: a `CreateGraph` request body, and the region
/// it is signed for.
struct GraphCase {
    name: &'static str,
    region: &'static str,
    request: Value,
}

/// The table: the rule is "a later read reports what the create stored", so the rows
/// differ along what the rule generalises over — whether tags are present at all, how
/// many there are (one, and the modelled `TagMap` maximum of 50), the form of a tag
/// value (`TagValue` length is 0..256, so empty is a legal value), and the region the
/// ARN is minted in.
fn graph_cases() -> Vec<GraphCase> {
    let mut many_tags = Map::new();
    for n in 0..50 {
        many_tags.insert(format!("Key{n}"), json!(format!("value-{n}")));
    }
    vec![
        GraphCase {
            name: "no-tags",
            region: REGION,
            request: json!({}),
        },
        GraphCase {
            name: "single-tag",
            region: "us-east-1",
            request: json!({ "Tags": { "Env": "test" } }),
        },
        GraphCase {
            name: "empty-tag-value",
            region: "eu-west-1",
            request: json!({ "Tags": { "Env": "" } }),
        },
        GraphCase {
            // 50 tags: the modelled `TagMap` length maximum.
            name: "fifty-tags",
            region: "ap-southeast-2",
            request: json!({ "Tags": Value::Object(many_tags) }),
        },
    ]
}

#[tokio::test]
async fn created_graphs_are_listed_with_the_arn_and_created_time_they_were_created_with() {
    let router = emulator();
    let mut expected: Vec<String> = Vec::new();

    for case in graph_cases() {
        let arn = create_graph_ok(&router, case.region, &case.request).await;
        assert_graph_arn(case.name, case.region, &arn);
        assert!(
            !expected.contains(&arn),
            "case {:?}: expected a fresh ARN, got one already minted: {arn:?}",
            case.name
        );
        expected.push(arn);
    }

    let (status, page) = call(&router, "/graphs/list", &json!({})).await;
    assert_eq!(status, 200, "ListGraphs: expected 200, got {status} {page:?}");
    let listed = graph_list(&page);
    assert_eq!(
        listed.len(),
        expected.len(),
        "ListGraphs: expected the {} created graphs, got {listed:?}",
        expected.len()
    );

    for arn in &expected {
        let entry = listed
            .iter()
            .find(|graph| graph.get("Arn").and_then(Value::as_str) == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {arn:?}: ListGraphs did not report it, got {listed:?}"));
        let created_time = assert_iso8601(arn, entry.get("CreatedTime"));

        // A read does not change what a later caller sees: the second listing
        // reports the identical CreatedTime.
        let (_, again) = call(&router, "/graphs/list", &json!({})).await;
        let repeated = graph_list(&again)
            .iter()
            .find(|graph| graph.get("Arn").and_then(Value::as_str) == Some(arn.as_str()))
            .and_then(|graph| graph.get("CreatedTime"))
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            repeated.as_deref(),
            Some(created_time.as_str()),
            "case {arn:?}: expected a stable CreatedTime, got {repeated:?}"
        );
    }
}

#[tokio::test]
async fn list_graphs_pages_through_every_stored_graph_in_arn_order() {
    let router = emulator();
    let mut expected: Vec<String> = Vec::new();
    for _ in 0..7 {
        expected.push(create_graph_ok(&router, REGION, &json!({})).await);
    }
    expected.sort();

    // Rows differ along the page size, including both published bounds of the
    // modelled `MemberResultsLimit` range (1..200) and "no MaxResults at all".
    let cases: Vec<Option<i32>> = vec![
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(7),
        Some(8),
        Some(MAX_PAGE_SIZE as i32),
    ];
    for max_results in cases {
        let served = all_graph_arns(&router, max_results).await;
        assert_eq!(
            served, expected,
            "case max_results={max_results:?}: expected every stored graph once in ARN order, got {served:?}"
        );
    }
}

#[tokio::test]
async fn list_graphs_refuses_a_max_results_outside_the_modelled_range() {
    let router = emulator();
    // One past each published bound of `MemberResultsLimit` (range 1..200).
    for max_results in [0, 201, -1] {
        let (status, body) = call(&router, "/graphs/list", &json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case MaxResults={max_results}: expected ValidationException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_pagination_token_this_service_never_issued_is_refused() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "list-graphs-not-hex",
            "/graphs/list",
            json!({ "NextToken": "zzzz" }),
        ),
        (
            "list-graphs-hex-but-not-an-arn",
            "/graphs/list",
            json!({ "NextToken": ::hex::encode("not-an-arn") }),
        ),
        (
            "list-members-not-hex",
            "/graph/members/list",
            json!({ "GraphArn": graph_arn, "NextToken": "zzzz" }),
        ),
        (
            "list-members-hex-but-not-an-account-id",
            "/graph/members/list",
            json!({ "GraphArn": graph_arn, "NextToken": ::hex::encode("12345") }),
        ),
    ];
    for (case, uri, body) in cases {
        let (status, answered) = call(&router, uri, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn created_members_are_read_back_by_list_members_and_get_members() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({ "Tags": { "Env": "test" } })).await;

    // Rows differ along the number of accounts invited in one call: one, a handful,
    // and a batch — the rule is the same for each.
    let batches: Vec<Vec<String>> = vec![
        vec![account_id(0)],
        vec![account_id(1), account_id(2), account_id(3)],
        (10..20).map(account_id).collect(),
    ];
    let mut expected: Vec<String> = Vec::new();

    for batch in &batches {
        let case = format!("batch of {}", batch.len());
        let accounts: Vec<Value> = batch.iter().map(|id| account(id)).collect();
        let (status, created) = call(
            &router,
            "/graph/members",
            &json!({ "GraphArn": graph_arn, "Accounts": accounts }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let members = created
            .get("Members")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no Members, got {created:?}"));
        assert_eq!(
            members.len(),
            batch.len(),
            "case {case:?}: expected {} members, got {members:?}",
            batch.len()
        );
        for (member, id) in members.iter().zip(batch) {
            assert_eq!(
                member.get("AccountId").and_then(Value::as_str),
                Some(id.as_str()),
                "case {case:?}: expected AccountId {id}, got {member:?}"
            );
            assert_eq!(
                member.get("GraphArn").and_then(Value::as_str),
                Some(graph_arn.as_str()),
                "case {case:?}: expected GraphArn {graph_arn}, got {member:?}"
            );
            assert_eq!(
                member.get("AdministratorId").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case:?}: expected AdministratorId {ACCOUNT}, got {member:?}"
            );
            // `MasterId` is the deprecated spelling of the same value, and the model
            // still binds it.
            assert_eq!(
                member.get("MasterId").and_then(Value::as_str),
                member.get("AdministratorId").and_then(Value::as_str),
                "case {case:?}: expected MasterId to equal AdministratorId, got {member:?}"
            );
            assert_eq!(
                member.get("InvitationType").and_then(Value::as_str),
                Some("INVITATION"),
                "case {case:?}: expected InvitationType INVITATION, got {member:?}"
            );
            // "CreateMembers verifies the accounts and then invites the verified
            // accounts": the membership is created mid-verification.
            assert_eq!(
                member.get("Status").and_then(Value::as_str),
                Some("VERIFICATION_IN_PROGRESS"),
                "case {case:?}: expected Status VERIFICATION_IN_PROGRESS, got {member:?}"
            );
            assert_iso8601(&case, member.get("InvitedTime"));
            assert_iso8601(&case, member.get("UpdatedTime"));
            assert_eq!(
                member.get("EmailAddress").and_then(Value::as_str),
                Some(format!("member-{id}@example.com").as_str()),
                "case {case:?}: expected the email the request carried, got {member:?}"
            );
            assert_eq!(
                created.get("UnprocessedAccounts").and_then(Value::as_array),
                Some(&Vec::new()),
                "case {case:?}: expected no unprocessed accounts, got {created:?}"
            );
        }
        expected.extend(batch.iter().cloned());
    }

    expected.sort();
    let listed = all_member_ids(&router, &graph_arn, None).await;
    assert_eq!(
        listed, expected,
        "ListMembers: expected every stored member once in account-id order, got {listed:?}"
    );

    // GetMembers reports exactly the accounts asked about, in the order asked.
    let asked: Vec<String> = vec![account_id(3), account_id(0), account_id(12)];
    let (status, got) = call(
        &router,
        "/graph/members/get",
        &json!({ "GraphArn": graph_arn, "AccountIds": asked }),
    )
    .await;
    assert_eq!(status, 200, "GetMembers: expected 200, got {status} {got:?}");
    let returned: Vec<String> = got
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetMembers carried no MemberDetails, got {got:?}"))
        .iter()
        .map(|member| {
            member
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        returned, asked,
        "GetMembers: expected exactly the accounts asked about, got {returned:?}"
    );
}

#[tokio::test]
async fn list_members_pages_through_every_member_in_account_id_order() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let accounts: Vec<String> = (0..9).map(account_id).collect();
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({
            "GraphArn": graph_arn,
            "Accounts": accounts.iter().map(|id| account(id)).collect::<Vec<_>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateMembers: expected 200, got {status} {created:?}"
    );

    let mut expected = accounts.clone();
    expected.sort();
    for max_results in [None, Some(1), Some(4), Some(9), Some(10), Some(MAX_PAGE_SIZE as i32)] {
        let served = all_member_ids(&router, &graph_arn, max_results).await;
        assert_eq!(
            served, expected,
            "case max_results={max_results:?}: expected every member once in account-id order, got {served:?}"
        );
    }
}

#[tokio::test]
async fn an_account_that_cannot_be_processed_is_reported_rather_than_stored() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(0);

    // Row 1: the same account twice in one request. "The accounts that CreateMembers
    // was unable to process ... includes accounts that were already invited."
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({
            "GraphArn": graph_arn,
            "Accounts": [account(&member), account(&member)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "duplicate-in-one-request: expected 200, got {status} {created:?}"
    );
    assert_eq!(
        created
            .get("Members")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "duplicate-in-one-request: expected one stored member, got {created:?}"
    );
    let unprocessed = created
        .get("UnprocessedAccounts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("duplicate-in-one-request: no UnprocessedAccounts in {created:?}"));
    assert_eq!(
        unprocessed.len(),
        1,
        "duplicate-in-one-request: expected one unprocessed account, got {unprocessed:?}"
    );
    assert_eq!(
        unprocessed[0].get("AccountId").and_then(Value::as_str),
        Some(member.as_str()),
        "duplicate-in-one-request: expected the repeated account, got {unprocessed:?}"
    );
    assert!(
        unprocessed[0]
            .get("Reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.is_empty()),
        "duplicate-in-one-request: expected a Reason, got {unprocessed:?}"
    );

    // Row 2: an account that is already a member, alongside one that is not.
    let fresh = account_id(1);
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({
            "GraphArn": graph_arn,
            "Accounts": [account(&member), account(&fresh)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "already-a-member: expected 200, got {status} {created:?}"
    );
    let stored: Vec<&str> = created
        .get("Members")
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(|m| m.get("AccountId").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stored,
        vec![fresh.as_str()],
        "already-a-member: expected only the new account to be stored, got {created:?}"
    );
    assert_eq!(
        created
            .get("UnprocessedAccounts")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|entry| entry.get("AccountId"))
            .and_then(Value::as_str),
        Some(member.as_str()),
        "already-a-member: expected the existing member reported unprocessed, got {created:?}"
    );

    // Row 3: the administrator account itself cannot also be a member.
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": [account(ACCOUNT)] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "administrator-account: expected 200, got {status} {created:?}"
    );
    assert_eq!(
        created
            .get("Members")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "administrator-account: expected nothing stored, got {created:?}"
    );
    assert_eq!(
        created
            .get("UnprocessedAccounts")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|entry| entry.get("AccountId"))
            .and_then(Value::as_str),
        Some(ACCOUNT),
        "administrator-account: expected it reported unprocessed, got {created:?}"
    );

    // The store agrees: exactly the two real memberships exist.
    let listed = all_member_ids(&router, &graph_arn, None).await;
    let mut expected = vec![member.clone(), fresh.clone()];
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected exactly the accounts that were stored, got {listed:?}"
    );

    // Row 4: GetMembers on an account with no membership reports it unprocessed
    // rather than inventing a member for it.
    let stranger = account_id(99);
    let (status, got) = call(
        &router,
        "/graph/members/get",
        &json!({ "GraphArn": graph_arn, "AccountIds": [member, stranger] }),
    )
    .await;
    assert_eq!(status, 200, "get-members: expected 200, got {status} {got:?}");
    let detail_ids: BTreeSet<String> = got
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("get-members: no MemberDetails in {got:?}"))
        .iter()
        .filter_map(|m| m.get("AccountId").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert!(
        !detail_ids.contains(&stranger),
        "get-members: expected no member detail for {stranger}, got {got:?}"
    );
    assert_eq!(
        got.get("UnprocessedAccounts")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|entry| entry.get("AccountId"))
            .and_then(Value::as_str),
        Some(stranger.as_str()),
        "get-members: expected {stranger} reported unprocessed, got {got:?}"
    );
}

#[tokio::test]
async fn a_membership_moves_from_verification_to_invited_once_the_clock_makes_it_due() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(0);
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": [account(&member)] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateMembers: expected 200, got {status} {created:?}"
    );
    assert_eq!(
        created
            .get("Members")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|m| m.get("Status"))
            .and_then(Value::as_str),
        Some("VERIFICATION_IN_PROGRESS"),
        "CreateMembers: expected the transient status, got {created:?}"
    );

    // The move to INVITED is owed by the clock, not by any later call, so once the
    // due instant has passed every read reports it — and keeps reporting it.
    tokio::time::sleep(std::time::Duration::from_millis(
        (MEMBER_VERIFICATION_MILLIS as u64) + 250,
    ))
    .await;

    let mut seen = Vec::new();
    for read in 0..2 {
        let (status, got) = call(
            &router,
            "/graph/members/get",
            &json!({ "GraphArn": graph_arn, "AccountIds": [member.clone()] }),
        )
        .await;
        assert_eq!(status, 200, "read {read}: expected 200, got {status} {got:?}");
        let detail = got
            .get("MemberDetails")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .cloned()
            .unwrap_or_else(|| panic!("read {read}: no MemberDetails in {got:?}"));
        assert_eq!(
            detail.get("Status").and_then(Value::as_str),
            Some("INVITED"),
            "read {read}: expected Status INVITED once verification was due, got {detail:?}"
        );
        seen.push(assert_iso8601("invited", detail.get("UpdatedTime")));
    }
    assert_eq!(
        seen[0], seen[1],
        "expected a stable UpdatedTime across reads, got {seen:?}"
    );
}

#[tokio::test]
async fn an_unknown_or_malformed_graph_arn_is_refused_by_every_operation_that_takes_one() {
    let router = emulator();
    // Well formed per the modelled `GraphArn` pattern, but nothing was ever stored
    // under it.
    let unknown = format!("arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}", "a".repeat(32));
    let member = account_id(0);
    let unknown_cases: Vec<(&str, Value)> = vec![
        (
            "/graph/members",
            json!({ "GraphArn": unknown, "Accounts": [account(&member)] }),
        ),
        ("/graph/members/list", json!({ "GraphArn": unknown })),
        (
            "/graph/members/get",
            json!({ "GraphArn": unknown, "AccountIds": [member.clone()] }),
        ),
        ("/graph/removal", json!({ "GraphArn": unknown })),
    ];
    for (uri, body) in unknown_cases {
        let (status, answered) = call(&router, uri, &body).await;
        assert_eq!(
            status, 404,
            "case {uri:?} with an unknown ARN: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {uri:?} with an unknown ARN: expected ResourceNotFoundException, got {answered:?}"
        );
    }

    // Outside the modelled pattern: refused as a bad request, not as a missing one.
    // The rows differ along which field of the ARN is wrong.
    let malformed = [
        "not-an-arn",
        "arn:aws:detective:us-west-2:000000000000:graph:tooshort",
        "arn:aws:guardduty:us-west-2:000000000000:graph:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "arn:aws:detective:us-west-2:00000000000:graph:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "arn:aws:detective:us-west-2:000000000000:graph:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    ];
    for arn in malformed {
        let (status, answered) = call(&router, "/graph/removal", &json!({ "GraphArn": arn })).await;
        assert_eq!(
            status, 400,
            "case {arn:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {arn:?}: expected ValidationException, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_graph_removes_it_and_its_members_from_the_store() {
    let router = emulator();
    let kept = create_graph_ok(&router, REGION, &json!({})).await;
    let doomed = create_graph_ok(&router, REGION, &json!({ "Tags": { "Env": "test" } })).await;
    let member = account_id(0);
    let (status, _) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": doomed, "Accounts": [account(&member)] }),
    )
    .await;
    assert_eq!(status, 200, "CreateMembers on the graph about to be deleted");

    let (status, body) = call(&router, "/graph/removal", &json!({ "GraphArn": doomed })).await;
    assert_eq!(
        status, 200,
        "DeleteGraph: expected 200, got {status} {body:?}"
    );
    assert!(
        body.is_null() || body == json!({}),
        "DeleteGraph: expected an empty body, got {body:?}"
    );

    let listed = all_graph_arns(&router, None).await;
    assert_eq!(
        listed,
        vec![kept.clone()],
        "ListGraphs: expected only the graph that was not deleted, got {listed:?}"
    );

    // Everything that resolves the deleted ARN now answers the modelled 404, and the
    // member records went with it.
    let gone: Vec<(&str, Value)> = vec![
        (
            "/graph/members",
            json!({ "GraphArn": doomed, "Accounts": [account(&member)] }),
        ),
        ("/graph/members/list", json!({ "GraphArn": doomed })),
        (
            "/graph/members/get",
            json!({ "GraphArn": doomed, "AccountIds": [member.clone()] }),
        ),
        ("/graph/removal", json!({ "GraphArn": doomed })),
    ];
    for (uri, request_body) in gone {
        let (status, answered) = call(&router, uri, &request_body).await;
        assert_eq!(
            status, 404,
            "case {uri:?} after delete: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {uri:?} after delete: expected ResourceNotFoundException, got {answered:?}"
        );
    }

    // The graph that was not deleted is untouched, and holds no member from the one
    // that was.
    let survivors = all_member_ids(&router, &kept, None).await;
    assert!(
        survivors.is_empty(),
        "expected the surviving graph to hold no members, got {survivors:?}"
    );
}

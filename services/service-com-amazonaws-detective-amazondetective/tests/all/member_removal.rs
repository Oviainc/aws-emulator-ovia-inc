// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for removing member accounts from a behavior graph.
//!
//! The rule under test, from `DeleteMembers`' own documentation ("Removes the specified
//! member accounts from the behavior graph. The removed accounts no longer contribute
//! data to the behavior graph. ... For invited accounts, the removed accounts are deleted
//! from the list of accounts in the behavior graph. ... An administrator account cannot
//! use DeleteMembers to remove their own account from the behavior graph"): a
//! member-removal batch DROPS the stored membership of every account it names that the
//! graph holds one for and reports those accounts in `AccountIds` — which is always
//! served, empty when the batch removed nothing — and reports every
//! other account it names in `UnprocessedAccounts` with the reason it could not be
//! processed — so the membership readers stop reporting the removed accounts, every
//! membership the batch did not name is untouched, and repeating the call changes nothing
//! further.
//!
//! The recorded cases are two rows of the table below (one member removed; one member
//! removed beside one account that is not a member). The other rows span what the rule
//! generalises over: how many accounts one call names (its published 1 and 50 bounds),
//! how many of them hold a membership, whether an account is named twice, whether it was
//! already removed, and whether it is the graph's own administrator.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    ADMINISTRATOR_NOT_REMOVABLE_REASON, NO_SUCH_MEMBERSHIP_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, call, create_graph_ok, emulator, error_code, tags_of, ACCOUNT, REGION,
};

/// The URI `DeleteMembers` binds (`POST /graph/members/removal`).
const REMOVAL_URI: &str = "/graph/members/removal";

/// The modelled `AccountIdList` length bound both `DeleteMembers`' input and its output
/// carry (`len 1..50`):
/// `smithy_query(query="operation",operation="DeleteMembers")`.
const MAX_ACCOUNT_IDS: usize = 50;

/// An account id this suite never invites into the graph under test.
fn stranger(n: u64) -> String {
    account_id(500 + n)
}

/// `CreateMembers` for `accounts`, which must succeed and must invite every one of them.
async fn invite(router: &MultiServiceRouter, graph_arn: &str, accounts: &[String]) {
    let entries: Vec<Value> = accounts.iter().map(|id| account(id)).collect();
    let (status, created) = call(
        router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateMembers({accounts:?}): expected 200, got {status} {created:?}"
    );
    assert_eq!(
        created.get("UnprocessedAccounts").and_then(Value::as_array),
        Some(&Vec::new()),
        "CreateMembers({accounts:?}): expected every account invited, got {created:?}"
    );
}

/// The account ids `ListMembers` reports for `graph_arn`.
async fn listed_members(router: &MultiServiceRouter, graph_arn: &str) -> BTreeSet<String> {
    let (status, listed) = call(
        router,
        "/graph/members/list",
        &json!({ "GraphArn": graph_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListMembers: expected 200, got {status} {listed:?}"
    );
    listed
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListMembers carried no MemberDetails, got {listed:?}"))
        .iter()
        .map(|detail| {
            detail
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a member detail carried no AccountId: {detail:?}"))
                .to_string()
        })
        .collect()
}

/// `(the account ids GetMembers reported a member detail for, the reason it reported for
/// each account it could not process)`.
async fn get_members(
    router: &MultiServiceRouter,
    graph_arn: &str,
    account_ids: &[String],
) -> (BTreeSet<String>, BTreeMap<String, String>) {
    let (status, got) = call(
        router,
        "/graph/members/get",
        &json!({ "GraphArn": graph_arn, "AccountIds": account_ids }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMembers({account_ids:?}): expected 200, got {status} {got:?}"
    );
    let reported = got
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetMembers carried no MemberDetails, got {got:?}"))
        .iter()
        .map(|detail| {
            detail
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    (reported, unprocessed_reasons("GetMembers", &got))
}

/// The `UnprocessedAccounts` of one answer as `account id -> reason`.
fn unprocessed_reasons(case: &str, body: &Value) -> BTreeMap<String, String> {
    body.get("UnprocessedAccounts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no UnprocessedAccounts in {body:?}"))
        .iter()
        .map(|entry| {
            (
                entry
                    .get("AccountId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an unprocessed account carried no AccountId: {entry:?}")
                    })
                    .to_string(),
                entry
                    .get("Reason")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an unprocessed account carried no Reason: {entry:?}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The `AccountIds` of one `DeleteMembers` answer. The member is always served — empty
/// when the call removed nothing — so an answer carrying no such list at all is a
/// failure, not an empty set.
fn removed_account_ids(case: &str, body: &Value) -> BTreeSet<String> {
    body.get("AccountIds")
        .unwrap_or_else(|| panic!("case {case:?}: the answer carried no AccountIds: {body:?}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: AccountIds is not a list: {body:?}"))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: AccountIds carried a non-string: {body:?}")
                })
                .to_string()
        })
        .collect()
}

/// One row of the removal table: the accounts one `DeleteMembers` call names, the
/// memberships it must remove, and the reason it must report for every other account it
/// named.
struct RemovalCase {
    name: &'static str,
    /// The `AccountIds` the request carries, in order (duplicates included).
    requested: Vec<String>,
    /// The accounts the call must report in its `AccountIds` output.
    expect_removed: BTreeSet<String>,
    /// `account id -> reason`, the `UnprocessedAccounts` the call must report.
    expect_unprocessed: BTreeMap<String, String>,
}

/// The members this suite invites before the table runs: enough for every row to act on
/// memberships of its own, plus the ones a later row re-names.
fn invited_members() -> Vec<String> {
    (0..12).map(account_id).collect()
}

/// The table. The rows run IN ORDER against one graph, so a later row can act on a
/// membership an earlier row removed (`already-removed-member` does exactly that).
fn removal_cases() -> Vec<RemovalCase> {
    let not_a_member = NO_SUCH_MEMBERSHIP_MESSAGE.to_string();
    let administrator = ADMINISTRATOR_NOT_REMOVABLE_REASON.to_string();
    // The modelled lower bound of `AccountIds` (1) and its upper bound (50) are both
    // rows: 45 strangers beside 5 members makes a batch of exactly 50.
    let max_batch: Vec<String> = (7..12)
        .map(account_id)
        .chain((0..45).map(stranger))
        .collect();
    assert_eq!(
        max_batch.len(),
        MAX_ACCOUNT_IDS,
        "the largest batch row must name exactly the modelled maximum"
    );

    vec![
        RemovalCase {
            name: "one-member-the-modelled-minimum-batch",
            requested: vec![account_id(0)],
            expect_removed: [account_id(0)].into_iter().collect(),
            expect_unprocessed: BTreeMap::new(),
        },
        RemovalCase {
            name: "three-members-at-once",
            requested: vec![account_id(1), account_id(2), account_id(3)],
            expect_removed: [account_id(1), account_id(2), account_id(3)]
                .into_iter()
                .collect(),
            expect_unprocessed: BTreeMap::new(),
        },
        RemovalCase {
            name: "partial-batch-one-member-one-non-member",
            requested: vec![account_id(4), stranger(100)],
            expect_removed: [account_id(4)].into_iter().collect(),
            expect_unprocessed: [(stranger(100), not_a_member.clone())]
                .into_iter()
                .collect(),
        },
        RemovalCase {
            name: "no-account-named-holds-a-membership",
            requested: vec![stranger(101), stranger(102)],
            expect_removed: BTreeSet::new(),
            expect_unprocessed: [
                (stranger(101), not_a_member.clone()),
                (stranger(102), not_a_member.clone()),
            ]
            .into_iter()
            .collect(),
        },
        RemovalCase {
            name: "the-same-member-named-twice-is-one-account",
            requested: vec![account_id(5), account_id(5)],
            expect_removed: [account_id(5)].into_iter().collect(),
            expect_unprocessed: BTreeMap::new(),
        },
        RemovalCase {
            name: "already-removed-member",
            requested: vec![account_id(0)],
            expect_removed: BTreeSet::new(),
            expect_unprocessed: [(account_id(0), not_a_member.clone())]
                .into_iter()
                .collect(),
        },
        RemovalCase {
            name: "the-graphs-own-administrator",
            requested: vec![ACCOUNT.to_string()],
            expect_removed: BTreeSet::new(),
            expect_unprocessed: [(ACCOUNT.to_string(), administrator.clone())]
                .into_iter()
                .collect(),
        },
        RemovalCase {
            name: "the-administrator-beside-a-member",
            requested: vec![ACCOUNT.to_string(), account_id(6)],
            expect_removed: [account_id(6)].into_iter().collect(),
            expect_unprocessed: [(ACCOUNT.to_string(), administrator)].into_iter().collect(),
        },
        RemovalCase {
            name: "the-modelled-maximum-batch-of-fifty",
            expect_removed: (7..12).map(account_id).collect(),
            expect_unprocessed: (0..45)
                .map(|n| (stranger(n), not_a_member.clone()))
                .collect(),
            requested: max_batch,
        },
    ]
}

#[tokio::test]
async fn a_removal_batch_drops_the_memberships_it_names_and_reports_the_rest_unprocessed() {
    let router = emulator();
    let tags = json!({ "Tags": { "Env": "test", "Team": "secops" } });
    let graph_arn = create_graph_ok(&router, REGION, &tags).await;
    let members = invited_members();
    invite(&router, &graph_arn, &members).await;

    let mut survivors: BTreeSet<String> = members.iter().cloned().collect();
    // The graph also holds the administrator's own membership, which no removal can
    // take away: it is listed before any member is removed and after every one is.
    survivors.insert(ACCOUNT.to_string());
    assert_eq!(
        listed_members(&router, &graph_arn).await,
        survivors,
        "before any removal: expected every invited account and the administrator's own \
         membership to be listed"
    );

    for case in removal_cases() {
        let name = case.name;
        let (status, answered) = call(
            &router,
            REMOVAL_URI,
            &json!({ "GraphArn": graph_arn, "AccountIds": case.requested }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteMembers expected 200, got {status} {answered:?}"
        );

        // The accounts removed are exactly the ones the graph held a membership for —
        // reported in `AccountIds`, which is served whether or not it is empty.
        assert_eq!(
            removed_account_ids(name, &answered),
            case.expect_removed,
            "case {name:?}: expected {:?} removed, got {answered:?}",
            case.expect_removed
        );
        // Every other account it named is reported with the reason it was not processed.
        assert_eq!(
            unprocessed_reasons(name, &answered),
            case.expect_unprocessed,
            "case {name:?}: expected {:?} unprocessed, got {answered:?}",
            case.expect_unprocessed
        );

        // The removal is in the store, not only in the answer: the membership readers
        // stop reporting the removed accounts and still report every other one.
        for removed in &case.expect_removed {
            survivors.remove(removed);
        }
        assert_eq!(
            listed_members(&router, &graph_arn).await,
            survivors,
            "case {name:?}: expected exactly the surviving memberships listed"
        );
        let (reported, unprocessed) = get_members(&router, &graph_arn, &case.requested).await;
        let still_members: BTreeSet<String> = case
            .requested
            .iter()
            .filter(|id| survivors.contains(*id))
            .cloned()
            .collect();
        assert_eq!(
            reported, still_members,
            "case {name:?}: GetMembers must report exactly the memberships that survived"
        );
        for id in case.expect_removed.iter().chain(
            case.expect_unprocessed
                .keys()
                .filter(|id| !survivors.contains(*id)),
        ) {
            assert_eq!(
                unprocessed.get(id).map(String::as_str),
                Some(NO_SUCH_MEMBERSHIP_MESSAGE),
                "case {name:?}: GetMembers must report {id:?} as no longer a member, got {unprocessed:?}"
            );
        }
    }

    // Removing members changes nothing else about the graph: it still exists, and the
    // tags it was created with read back unchanged.
    let read_tags = tags_of(&router, REGION, &graph_arn).await;
    assert_eq!(
        Value::Object(read_tags.clone()),
        tags["Tags"],
        "expected the graph's tags untouched by member removal, got {read_tags:?}"
    );
}

#[tokio::test]
async fn a_batch_outside_the_modelled_account_id_bounds_is_refused() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    invite(&router, &graph_arn, &[account_id(0)]).await;

    // One past each published bound of the modelled `AccountIds` (`len 1..50`): the
    // framework refuses both with a 400 before the handler runs, so neither can remove
    // anything.
    let past_bounds: Vec<(&str, Vec<String>)> = vec![
        ("empty-batch", Vec::new()),
        (
            "one-past-the-maximum",
            (0..=MAX_ACCOUNT_IDS as u64).map(stranger).collect(),
        ),
        ("not-an-account-id", vec!["not-an-account".to_string()]),
    ];

    for (name, requested) in past_bounds {
        let (status, answered) = call(
            &router,
            REMOVAL_URI,
            &json!({ "GraphArn": graph_arn, "AccountIds": requested }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {answered:?}"
        );
    }

    // Nothing was removed by any refused request.
    assert_eq!(
        listed_members(&router, &graph_arn).await,
        [account_id(0), ACCOUNT.to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "a refused removal must leave every membership in place"
    );
}

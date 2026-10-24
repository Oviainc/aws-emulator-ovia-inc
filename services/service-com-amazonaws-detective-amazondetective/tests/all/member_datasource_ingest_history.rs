// SPDX-License-Identifier: CC0-1.0

//! What `BatchGetGraphMemberDatasources` reports for a batch of accounts.
//!
//! The rule under test: for each DISTINCT account a request names, the behavior graph
//! either ingests data for it — its administrator account, which `CreateGraph` "creates a
//! new behavior graph for and sets as the administrator account", or an account with a
//! membership in it — and then the call reports that account's data source package
//! information, which is the graph's own package history: per modelled
//! `DatasourcePackage` the graph holds a record for, one ISO8601 `date-time` per ingest
//! state that package has entered (`LastIngestStateChangeDates` is modelled as a map
//! keyed by `DatasourcePackageIngestState`) — or the graph ingests nothing for it, and
//! then the account is reported in `UnprocessedAccounts` with the reason, never
//! fabricated as a contributing account. Reporting writes nothing: a second call answers
//! identically.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    GRAPH_NOT_FOUND_MESSAGE, NOT_INGESTED_FOR_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, assert_iso8601, call, create_graph_ok, emulator, error_code,
    error_message, ACCOUNT, REGION,
};

/// One `BatchGetGraphMemberDatasources` call that must succeed.
async fn datasources_of(
    router: &MultiServiceRouter,
    graph_arn: &str,
    account_ids: &[String],
    case: &str,
) -> Value {
    let (status, got) = call(
        router,
        "/graph/datasources/get",
        &json!({ "GraphArn": graph_arn, "AccountIds": account_ids }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {got:?}"
    );
    got
}

/// The `AccountId` of each entry of one answer's list member, in the order it reports
/// them.
fn reported_ids(answer: &Value, member: &str, case: &str) -> Vec<String> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: answer carried no {member}, got {answer:?}"))
        .iter()
        .map(|entry| {
            entry
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a {member} entry carried no AccountId: {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// One account's `DatasourcePackageIngestHistory` as `{package: {state: timestamp}}`,
/// asserting each timestamp is the modelled ISO8601 `date-time`.
fn ingest_history(answer: &Value, account: &str, case: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let entry = answer
        .get("MemberDatasources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: answer carried no MemberDatasources"))
        .iter()
        .find(|entry| entry.get("AccountId").and_then(Value::as_str) == Some(account))
        .unwrap_or_else(|| panic!("case {case:?}: nothing reported for {account:?}: {answer:?}"));
    let history = entry
        .get("DatasourcePackageIngestHistory")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {account:?} carried no DatasourcePackageIngestHistory: {entry:?}")
        });
    history
        .iter()
        .map(|(package, states)| {
            let states = states.as_object().unwrap_or_else(|| {
                panic!("case {case:?}: {package:?} carried no state map, got {states:?}")
            });
            (
                package.clone(),
                states
                    .iter()
                    .map(|(state, collected)| {
                        let rendered = assert_iso8601(
                            &format!("{case}/{package}/{state}"),
                            collected.get("Timestamp"),
                        );
                        (state.clone(), rendered)
                    })
                    .collect(),
            )
        })
        .collect()
}

/// One row of the batch table: the accounts a request names, and what the rule says is
/// reported for them.
struct BatchCase {
    name: String,
    /// The `AccountIds` the request lists, in order.
    account_ids: Vec<String>,
    /// The accounts expected in `MemberDatasources`, in order.
    expected_members: Vec<String>,
    /// The accounts expected in `UnprocessedAccounts`, in order.
    expected_unprocessed: Vec<String>,
}

/// The table. The rule generalises over which accounts a batch names and how many, so the
/// rows differ along the batch size (one, three, two hundred — the modelled
/// `AccountIdExtendedList` maximum), along whether the graph ingests data for each named
/// account (its administrator, a member, or neither), along the order they are named in,
/// and along naming one account twice.
fn batch_cases(members: &[String], strangers: &[String]) -> Vec<BatchCase> {
    let mut cases = vec![
        BatchCase {
            name: "one-member".to_string(),
            account_ids: vec![members[0].clone()],
            expected_members: vec![members[0].clone()],
            expected_unprocessed: Vec::new(),
        },
        BatchCase {
            name: "every-member-in-reverse-order".to_string(),
            account_ids: vec![members[2].clone(), members[1].clone(), members[0].clone()],
            expected_members: vec![members[2].clone(), members[1].clone(), members[0].clone()],
            expected_unprocessed: Vec::new(),
        },
        BatchCase {
            name: "one-non-member".to_string(),
            account_ids: vec![strangers[0].clone()],
            expected_members: Vec::new(),
            expected_unprocessed: vec![strangers[0].clone()],
        },
        BatchCase {
            name: "members-and-non-members-interleaved".to_string(),
            account_ids: vec![
                members[0].clone(),
                strangers[0].clone(),
                members[1].clone(),
                strangers[1].clone(),
            ],
            expected_members: vec![members[0].clone(), members[1].clone()],
            expected_unprocessed: vec![strangers[0].clone(), strangers[1].clone()],
        },
        BatchCase {
            // The account that created the graph administers it, and the graph ingests
            // its data too, so it is reported even though it holds no MEMBERSHIP.
            name: "the-administrator-account".to_string(),
            account_ids: vec![ACCOUNT.to_string()],
            expected_members: vec![ACCOUNT.to_string()],
            expected_unprocessed: Vec::new(),
        },
        BatchCase {
            name: "the-administrator-beside-a-member-and-a-stranger".to_string(),
            account_ids: vec![
                strangers[3].clone(),
                ACCOUNT.to_string(),
                members[2].clone(),
            ],
            expected_members: vec![ACCOUNT.to_string(), members[2].clone()],
            expected_unprocessed: vec![strangers[3].clone()],
        },
        BatchCase {
            name: "a-member-named-twice".to_string(),
            account_ids: vec![members[1].clone(), members[1].clone()],
            expected_members: vec![members[1].clone()],
            expected_unprocessed: Vec::new(),
        },
        BatchCase {
            name: "a-non-member-named-three-times".to_string(),
            account_ids: vec![
                strangers[2].clone(),
                strangers[2].clone(),
                strangers[2].clone(),
            ],
            expected_members: Vec::new(),
            expected_unprocessed: vec![strangers[2].clone()],
        },
    ];

    // The modelled maximum batch: 200 accounts, the administrator and the three
    // memberships among them.
    let mut full: Vec<String> = strangers.iter().take(196).cloned().collect();
    assert_eq!(full.len(), 196, "the stranger pool must fill a full batch");
    full.insert(0, members[0].clone());
    full.insert(50, ACCOUNT.to_string());
    full.insert(100, members[1].clone());
    full.push(members[2].clone());
    cases.push(BatchCase {
        name: "the-modelled-maximum-batch".to_string(),
        account_ids: full.clone(),
        expected_members: vec![
            members[0].clone(),
            ACCOUNT.to_string(),
            members[1].clone(),
            members[2].clone(),
        ],
        expected_unprocessed: full
            .iter()
            .filter(|id| !members.contains(id) && id.as_str() != ACCOUNT)
            .cloned()
            .collect(),
    });

    cases
}

#[tokio::test]
async fn a_batch_reports_the_graphs_packages_for_the_accounts_it_ingests_for() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    let members: Vec<String> = (1..=3).map(account_id).collect();
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({
            "GraphArn": graph_arn,
            "Accounts": members.iter().map(|id| account(id)).collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateMembers: expected 200, got {status} {created:?}"
    );
    // Accounts the graph stores nothing at all for.
    let strangers: Vec<String> = (500..700).map(account_id).collect();

    for case in batch_cases(&members, &strangers) {
        let name = case.name.as_str();
        let answer = datasources_of(&router, &graph_arn, &case.account_ids, name).await;
        assert_eq!(
            reported_ids(&answer, "MemberDatasources", name),
            case.expected_members,
            "case {name:?}: expected exactly the accounts the graph ingests for, got {answer:?}"
        );
        assert_eq!(
            reported_ids(&answer, "UnprocessedAccounts", name),
            case.expected_unprocessed,
            "case {name:?}: expected exactly the accounts the graph ingests nothing for, got {answer:?}"
        );
        for entry in answer
            .get("UnprocessedAccounts")
            .and_then(Value::as_array)
            .expect("UnprocessedAccounts")
        {
            assert_eq!(
                entry.get("Reason").and_then(Value::as_str),
                Some(NOT_INGESTED_FOR_REASON),
                "case {name:?}: expected the no-membership reason, got {entry:?}"
            );
        }
        for member in &case.expected_members {
            let reported = answer
                .get("MemberDatasources")
                .and_then(Value::as_array)
                .expect("MemberDatasources")
                .iter()
                .find(|entry| entry.get("AccountId").and_then(Value::as_str) == Some(member))
                .unwrap_or_else(|| panic!("case {name:?}: nothing reported for {member:?}"));
            assert_eq!(
                reported.get("GraphArn").and_then(Value::as_str),
                Some(graph_arn.as_str()),
                "case {name:?}: expected the graph asked about, got {reported:?}"
            );
            // A freshly created graph ingests exactly the mandatory package, started.
            assert_eq!(
                ingest_history(&answer, member, name)
                    .keys()
                    .cloned()
                    .collect::<Vec<String>>(),
                vec!["DETECTIVE_CORE".to_string()],
                "case {name:?}: expected the graph's one package, got {reported:?}"
            );
        }
    }
}

/// One row of the package table: the set `UpdateDatasourcePackages` is asked for, the
/// history the read must then report (package, in key order, with the ingest states it
/// has entered), and the state `EKS_AUDIT` is left in by that update.
struct PackageCase {
    name: &'static str,
    requested: Vec<&'static str>,
    expected: Vec<(&'static str, Vec<&'static str>)>,
    eks_state: &'static str,
}

#[tokio::test]
async fn the_reported_history_follows_the_graphs_package_changes_and_a_read_changes_nothing() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(7);
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
    let named = std::slice::from_ref(&member);

    // A new graph ingests the mandatory package and nothing else.
    let first = ingest_history(
        &datasources_of(&router, &graph_arn, named, "created").await,
        &member,
        "created",
    );
    let core_started = first
        .get("DETECTIVE_CORE")
        .and_then(|states| states.get("STARTED"))
        .unwrap_or_else(|| panic!("created: expected DETECTIVE_CORE STARTED, got {first:?}"))
        .clone();
    assert_eq!(
        first.keys().cloned().collect::<Vec<String>>(),
        vec!["DETECTIVE_CORE".to_string()],
        "created: expected only the mandatory package, got {first:?}"
    );

    // The rows walk the package set: every state change a package goes through adds its
    // own date, and a package the graph never ingested has no entry at all.
    //
    // `DatasourcePackage` value set:
    // `smithy_query(query="shape",shape="DatasourcePackage")` — DETECTIVE_CORE,
    // EKS_AUDIT, ASFF_SECURITYHUB_FINDING.
    // The last column is the state `EKS_AUDIT` is in after the row's update: its date
    // must be the LATEST in that package's history, and every other state's date must be
    // the one the row before reported.
    let updates = vec![
        PackageCase {
            name: "started-eks",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT"],
            expected: vec![
                ("DETECTIVE_CORE", vec!["STARTED"]),
                ("EKS_AUDIT", vec!["STARTED"]),
            ],
            eks_state: "STARTED",
        },
        PackageCase {
            name: "stopped-eks",
            requested: vec!["DETECTIVE_CORE"],
            expected: vec![
                ("DETECTIVE_CORE", vec!["STARTED"]),
                ("EKS_AUDIT", vec!["STARTED", "STOPPED"]),
            ],
            eks_state: "STOPPED",
        },
        PackageCase {
            // The same package started AGAIN: the restart moves its `STARTED` date and
            // leaves the `STOPPED` date it already carried where it was.
            name: "restarted-eks-beside-every-other-package",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT", "ASFF_SECURITYHUB_FINDING"],
            expected: vec![
                ("ASFF_SECURITYHUB_FINDING", vec!["STARTED"]),
                ("DETECTIVE_CORE", vec!["STARTED"]),
                ("EKS_AUDIT", vec!["STARTED", "STOPPED"]),
            ],
            eks_state: "STARTED",
        },
    ];

    let mut previous_eks: Option<BTreeMap<String, String>> = None;
    for PackageCase {
        name: case,
        requested: packages,
        expected,
        eks_state,
    } in updates
    {
        // Distinct instants, so a second state of one package is visibly later than the
        // first rather than sharing its millisecond.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let (status, updated) = call(
            &router,
            "/graph/datasources/update",
            &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateDatasourcePackages expected 200, got {status} {updated:?}"
        );

        let history = ingest_history(
            &datasources_of(&router, &graph_arn, named, case).await,
            &member,
            case,
        );
        let shape: Vec<(String, Vec<String>)> = history
            .iter()
            .map(|(package, states)| (package.clone(), states.keys().cloned().collect()))
            .collect();
        let want: Vec<(String, Vec<String>)> = expected
            .iter()
            .map(|(package, states)| {
                (
                    package.to_string(),
                    states.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape, want,
            "case {case:?}: expected one date per state each package entered, got {history:?}"
        );
        // The instant a package entered a state it is still in never moves.
        assert_eq!(
            history
                .get("DETECTIVE_CORE")
                .and_then(|states| states.get("STARTED")),
            Some(&core_started),
            "case {case:?}: expected the mandatory package's start instant to be stable, got {history:?}"
        );
        let eks = history
            .get("EKS_AUDIT")
            .unwrap_or_else(|| panic!("case {case:?}: expected an EKS_AUDIT record"))
            .clone();
        let entered = eks
            .get(eks_state)
            .unwrap_or_else(|| panic!("case {case:?}: expected EKS_AUDIT {eks_state}, got {eks:?}"))
            .clone();
        for (state, when) in &eks {
            if state == eks_state {
                continue;
            }
            // Every date renders in the same ISO8601 `date-time` form, so they compare in
            // chronological order as strings: the state the package is in now is the one
            // it entered last.
            assert!(
                when.as_str() < entered.as_str(),
                "case {case:?}: expected {state} earlier than {eks_state}, got {eks:?}"
            );
            // A state the package did not re-enter keeps the date it already carried.
            if let Some(earlier) = previous_eks.as_ref().and_then(|prev| prev.get(state)) {
                assert_eq!(
                    when, earlier,
                    "case {case:?}: expected a stable EKS_AUDIT {state} instant, got {eks:?}"
                );
            }
        }
        previous_eks = Some(eks);
    }

    // A read does not change what a later caller sees.
    let once = datasources_of(&router, &graph_arn, named, "repeat-first").await;
    let twice = datasources_of(&router, &graph_arn, named, "repeat-second").await;
    assert_eq!(
        once, twice,
        "expected a repeated read to answer identically, got {once:?} then {twice:?}"
    );
}

#[tokio::test]
async fn a_graph_this_service_does_not_hold_is_refused() {
    let router = emulator();
    let held = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(9);
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": held, "Accounts": [account(&member)] }),
    )
    .await;
    assert_eq!(status, 200, "CreateMembers: got {status} {created:?}");

    // The rows differ along which part of a WELL-FORMED graph ARN names nothing: the id,
    // the Region and the account. Every one of them satisfies the modelled `GraphArn`
    // pattern, so the refusal is this operation's, not the constraint walk's.
    let absent = [
        (
            "unknown-id",
            format!("arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}", "a".repeat(32)),
        ),
        (
            "another-region",
            held.replace(REGION, "eu-central-1"),
        ),
        (
            "another-account",
            held.replace(ACCOUNT, "123456789012"),
        ),
    ];
    for (case, graph_arn) in absent {
        let (status, answered) = call(
            &router,
            "/graph/datasources/get",
            &json!({ "GraphArn": graph_arn, "AccountIds": [member] }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered:?}"
        );
        assert_eq!(
            error_message(&answered),
            GRAPH_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the graph-not-found message, got {answered:?}"
        );
    }
}

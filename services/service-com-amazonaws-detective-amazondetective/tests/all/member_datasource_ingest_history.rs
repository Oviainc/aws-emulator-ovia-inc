// SPDX-License-Identifier: CC0-1.0

//! What `BatchGetGraphMemberDatasources` reports for a batch of accounts.
//!
//! The rule under test: for each DISTINCT account a request names, the behavior graph
//! either holds a membership for it — an account invited into it, or its administrator
//! account, whose own membership `CreateGraph` creates with the graph — and then the call
//! reports that MEMBERSHIP's data source package information: per modelled
//! `DatasourcePackage` whose ingest has been started for the membership, the ISO8601
//! `date-time` it was started at (`DatasourcePackageIngestHistory` is "details on when a
//! data source package was added to a behavior graph", so it records additions and no
//! other state change) — or the graph holds no membership for it, and then the account is
//! reported in `UnprocessedAccounts` with the reason, never fabricated as a contributing
//! account. Reporting writes nothing: a second call answers identically.
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

/// Every value the modelled `DatasourcePackage` enum accepts, from
/// `smithy_query(query="shape",shape="DatasourcePackage")`, in the order a package name
/// sorts \u2014 which is the order a history map is walked in.
const PACKAGES: [&str; 3] = ["ASFF_SECURITYHUB_FINDING", "DETECTIVE_CORE", "EKS_AUDIT"];

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
            // The account that created the graph administers it, and holds its own
            // membership in it, so it is reported like any other member.
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
            // The administrator's own membership started every package the graph was
            // created ingesting; an invited membership has started the mandatory
            // package alone, so that is the only addition its history records.
            let expected: Vec<String> = if member == ACCOUNT {
                PACKAGES.iter().map(|p| (*p).to_string()).collect()
            } else {
                vec!["DETECTIVE_CORE".to_string()]
            };
            assert_eq!(
                ingest_history(&answer, member, name)
                    .keys()
                    .cloned()
                    .collect::<Vec<String>>(),
                expected,
                "case {name:?}: expected the packages started for this membership, got {reported:?}"
            );
        }
    }
}

/// One row of the package table: the set `UpdateDatasourcePackages` is asked for, and
/// the state that update leaves `EKS_AUDIT` in ON THE GRAPH — which is what makes the
/// row's assertion about the membership's history non-vacuous.
struct PackageCase {
    name: &'static str,
    requested: Vec<&'static str>,
    eks_state: &'static str,
}

/// The ingest state `ListDatasourcePackages` reports for `package` on `graph_arn`.
async fn graph_state_of(
    router: &MultiServiceRouter,
    graph_arn: &str,
    package: &str,
    case: &str,
) -> String {
    let (status, listed) = call(
        router,
        "/graph/datasources/list",
        &json!({ "GraphArn": graph_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListDatasourcePackages expected 200, got {status} {listed:?}"
    );
    listed
        .get("DatasourcePackages")
        .and_then(Value::as_object)
        .and_then(|packages| packages.get(package))
        .and_then(|detail| detail.get("DatasourcePackageIngestState"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no ingest state for {package} on the graph: {listed:?}")
        })
        .to_string()
}

#[tokio::test]
async fn a_memberships_history_records_what_was_started_for_it_and_a_read_changes_nothing() {
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

    // An invited membership has started the mandatory package alone, so that is the one
    // addition its history records.
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
    // The administrator's own membership started every package the graph was created
    // ingesting, so its history records one addition per modelled package.
    let administrator = vec![ACCOUNT.to_string()];
    let admin_first = ingest_history(
        &datasources_of(&router, &graph_arn, &administrator, "created-administrator").await,
        ACCOUNT,
        "created-administrator",
    );
    assert_eq!(
        admin_first.keys().cloned().collect::<Vec<String>>(),
        PACKAGES
            .iter()
            .map(|p| (*p).to_string())
            .collect::<Vec<String>>(),
        "created-administrator: expected every package the graph was created ingesting, \
         got {admin_first:?}"
    );
    for (package, states) in admin_first.iter().chain(first.iter()) {
        assert_eq!(
            states.keys().cloned().collect::<Vec<String>>(),
            vec!["STARTED".to_string()],
            "created: {package}'s history records the addition and nothing else, got {states:?}"
        );
    }

    // The rows walk the GRAPH's package set. A membership's history is the record of
    // what was started FOR THAT MEMBERSHIP, so changing which packages the graph
    // ingests moves the graph's own ingest state (asserted per row, so no row is
    // vacuous) and adds nothing to either membership's history.
    //
    // `DatasourcePackage` value set:
    // `smithy_query(query="shape",shape="DatasourcePackage")` — DETECTIVE_CORE,
    // EKS_AUDIT, ASFF_SECURITYHUB_FINDING.
    let updates = vec![
        PackageCase {
            name: "stopped-the-optional-packages",
            requested: vec!["DETECTIVE_CORE"],
            eks_state: "STOPPED",
        },
        PackageCase {
            name: "started-eks-again",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT"],
            eks_state: "STARTED",
        },
        PackageCase {
            name: "every-modelled-package",
            requested: vec!["DETECTIVE_CORE", "EKS_AUDIT", "ASFF_SECURITYHUB_FINDING"],
            eks_state: "STARTED",
        },
    ];

    for PackageCase {
        name: case,
        requested: packages,
        eks_state,
    } in updates
    {
        // Distinct instants, so a date the rule says must not move would be visibly
        // different if it did.
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
        assert_eq!(
            graph_state_of(&router, &graph_arn, "EKS_AUDIT", case).await,
            eks_state,
            "case {case:?}: expected the update to leave EKS_AUDIT {eks_state} on the graph"
        );

        let history = ingest_history(
            &datasources_of(&router, &graph_arn, named, case).await,
            &member,
            case,
        );
        assert_eq!(
            history, first,
            "case {case:?}: the graph's package change must not touch the membership's \
             addition history, got {history:?}"
        );
        assert_eq!(
            history
                .get("DETECTIVE_CORE")
                .and_then(|states| states.get("STARTED")),
            Some(&core_started),
            "case {case:?}: expected the mandatory package's start instant to be stable, got {history:?}"
        );
        let admin_history = ingest_history(
            &datasources_of(&router, &graph_arn, &administrator, case).await,
            ACCOUNT,
            case,
        );
        assert_eq!(
            admin_history, admin_first,
            "case {case:?}: the administrator's addition history must be stable too, \
             got {admin_history:?}"
        );
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

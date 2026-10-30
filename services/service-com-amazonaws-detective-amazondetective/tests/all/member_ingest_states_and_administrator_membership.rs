// SPDX-License-Identifier: CC0-1.0

//! What every `MemberDetail` of a behavior graph reports, and the membership a graph
//! holds for the account that administers it.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one a
//! recording happened to carry:
//!
//! 1. A `MemberDetail` carries `DatasourcePackageIngestStates` with one entry for EVERY
//!    data source package the graph holds a record for — never a short map — and the
//!    entry is the state ingest of that package is in FOR THAT MEMBERSHIP: `STARTED`
//!    when the graph is ingesting the package and ingest of it has been started for the
//!    account, `STOPPED` otherwise. The administrator's own membership started every
//!    package the graph was created ingesting; an invited membership starts with the
//!    package a graph may never switch off and nothing else.
//! 2. A behavior graph holds an `ENABLED` membership for its administrator account from
//!    the moment it is created: every membership reader reports it, and no removal can
//!    take it away, so a graph is never left with no contributing account.
//! 3. A membership reports its `InvitationType` only once the verification
//!    `CreateMembers` starts has settled — the create response itself carries none —
//!    and the administrator's own membership, which no invitation and no organization
//!    created, reports none at all and carries no root-user email address.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    DATASOURCE_PACKAGE_STATE_STARTED, DATASOURCE_PACKAGE_STATE_STOPPED,
    INVITATION_TYPE_INVITATION, MANDATORY_DATASOURCE_PACKAGE, MEMBER_STATUS_ENABLED,
    MEMBER_STATUS_VERIFICATION_FAILED, MEMBER_STATUS_VERIFICATION_IN_PROGRESS,
    MEMBER_VERIFICATION_MILLIS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, assert_iso8601, call, create_graph_ok, emulator, ACCOUNT, REGION,
};

/// Every value the modelled `DatasourcePackage` enum accepts, from
/// `smithy_query(query="shape",shape="DatasourcePackage")`, in the order a package name
/// sorts.
const PACKAGES: [&str; 3] = ["ASFF_SECURITYHUB_FINDING", "DETECTIVE_CORE", "EKS_AUDIT"];

/// The URIs of the three operations that report a `MemberDetail`, and of the writers
/// these suites drive, from
/// `smithy_query(query="operation",operation="CreateMembers,GetMembers,ListMembers")`.
const CREATE_URI: &str = "/graph/members";
const GET_URI: &str = "/graph/members/get";
const LIST_URI: &str = "/graph/members/list";
const REMOVAL_URI: &str = "/graph/members/removal";
const UPDATE_PACKAGES_URI: &str = "/graph/datasources/update";

/// `CreateMembers` for `accounts`, which must succeed; answers the `Members` it reported.
async fn create_members(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    accounts: &[String],
) -> Vec<Value> {
    let entries: Vec<Value> = accounts.iter().map(|id| account(id)).collect();
    let (status, created) = call(
        router,
        CREATE_URI,
        &json!({ "GraphArn": graph_arn, "Accounts": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateMembers expected 200, got {status} {created:?}"
    );
    created
        .get("Members")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: CreateMembers carried no Members: {created:?}"))
        .clone()
}

/// `UpdateDatasourcePackages` naming `packages`, which must be accepted.
async fn update_packages(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    packages: &[&str],
) {
    let (status, answered) = call(
        router,
        UPDATE_PACKAGES_URI,
        &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateDatasourcePackages {packages:?} expected 200, got {status} {answered:?}"
    );
}

/// Every `MemberDetail` `ListMembers` reports for `graph_arn`, keyed by account id.
async fn listed_members(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
) -> BTreeMap<String, Value> {
    let (status, listed) = call(router, LIST_URI, &json!({ "GraphArn": graph_arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMembers expected 200, got {status} {listed:?}"
    );
    by_account_id(case, "ListMembers", &listed)
}

/// Every `MemberDetail` `GetMembers` reports for `account_ids`, keyed by account id.
async fn got_members(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    account_ids: &[String],
) -> BTreeMap<String, Value> {
    let (status, got) = call(
        router,
        GET_URI,
        &json!({ "GraphArn": graph_arn, "AccountIds": account_ids }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMembers expected 200, got {status} {got:?}"
    );
    by_account_id(case, "GetMembers", &got)
}

/// The `MemberDetails` of one answer, keyed by account id.
fn by_account_id(case: &str, operation: &str, answer: &Value) -> BTreeMap<String, Value> {
    answer
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {operation} carried no MemberDetails: {answer:?}")
        })
        .iter()
        .map(|detail| {
            let id = detail
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a {operation} detail carried no AccountId: {detail:?}")
                });
            (id.to_string(), detail.clone())
        })
        .collect()
}

/// One member's `DatasourcePackageIngestStates` as `{package: state}`.
fn ingest_states(case: &str, detail: &Value) -> BTreeMap<String, String> {
    detail
        .get("DatasourcePackageIngestStates")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected DatasourcePackageIngestStates, got {detail:?}")
        })
        .iter()
        .map(|(package, state)| {
            let state = state.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {package}'s ingest state is not a string: {state:?}")
            });
            (package.clone(), state.to_string())
        })
        .collect()
}

/// The ingest states the rule says a membership reports: one entry per modelled package,
/// `STARTED` for the packages in `started` and `STOPPED` for every other one.
fn expected_states(started: &[&str]) -> BTreeMap<String, String> {
    PACKAGES
        .iter()
        .map(|package| {
            let state = if started.contains(package) {
                DATASOURCE_PACKAGE_STATE_STARTED
            } else {
                DATASOURCE_PACKAGE_STATE_STOPPED
            };
            ((*package).to_string(), state.to_string())
        })
        .collect()
}

/// One row of the graph-package table: the set the graph is left ingesting, the states
/// the administrator's own membership then reports, and the states an invited membership
/// reports.
struct PackageCase {
    name: &'static str,
    graph_packages: Vec<&'static str>,
    administrator_started: Vec<&'static str>,
    invited_started: Vec<&'static str>,
}

#[tokio::test]
async fn every_member_detail_reports_one_ingest_state_per_package_the_graph_holds() {
    // The rows differ along WHICH packages the graph is ingesting and HOW MANY — every
    // modelled package (what a graph is created ingesting), the mandatory package alone,
    // and each optional package beside it. Each row is its own emulator, and so its own
    // graph, and each one is read through all three readers of a `MemberDetail`.
    let rows = [
        PackageCase {
            name: "every-package-the-graph-was-created-ingesting",
            graph_packages: PACKAGES.to_vec(),
            administrator_started: PACKAGES.to_vec(),
            invited_started: vec![MANDATORY_DATASOURCE_PACKAGE],
        },
        PackageCase {
            name: "only-the-package-a-graph-may-not-switch-off",
            graph_packages: vec![MANDATORY_DATASOURCE_PACKAGE],
            // A package the graph stopped is stopped for every membership, the
            // administrator's own included.
            administrator_started: vec![MANDATORY_DATASOURCE_PACKAGE],
            invited_started: vec![MANDATORY_DATASOURCE_PACKAGE],
        },
        PackageCase {
            name: "the-mandatory-package-and-eks-audit",
            graph_packages: vec![MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"],
            administrator_started: vec![MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"],
            invited_started: vec![MANDATORY_DATASOURCE_PACKAGE],
        },
        PackageCase {
            // The package the issue's recording never cited on its own.
            name: "the-mandatory-package-and-asff-security-hub-findings",
            graph_packages: vec![MANDATORY_DATASOURCE_PACKAGE, "ASFF_SECURITYHUB_FINDING"],
            administrator_started: vec![MANDATORY_DATASOURCE_PACKAGE, "ASFF_SECURITYHUB_FINDING"],
            invited_started: vec![MANDATORY_DATASOURCE_PACKAGE],
        },
    ];

    for row in rows {
        let case = row.name;
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
        update_packages(&router, case, &graph_arn, &row.graph_packages).await;

        // The rows also differ along how many accounts one call invites: one, then two
        // more in a second call.
        let invited: Vec<String> = (0..3).map(account_id).collect();
        let created = create_members(&router, case, &graph_arn, &invited[..1]).await;
        let created_more = create_members(&router, case, &graph_arn, &invited[1..]).await;

        let want_invited = expected_states(&row.invited_started);
        let want_administrator = expected_states(&row.administrator_started);

        // Reader 1: the create response itself.
        for detail in created.iter().chain(created_more.iter()) {
            let states = ingest_states(case, detail);
            assert_eq!(
                states, want_invited,
                "case {case:?}: CreateMembers expected {want_invited:?}, got {states:?}"
            );
        }

        // Reader 2: GetMembers, asked about every invited account and the administrator.
        let mut asked = invited.clone();
        asked.push(ACCOUNT.to_string());
        let got = got_members(&router, case, &graph_arn, &asked).await;
        // Reader 3: ListMembers, which reports every membership the graph holds.
        let listed = listed_members(&router, case, &graph_arn).await;

        for (reader, details) in [("GetMembers", &got), ("ListMembers", &listed)] {
            for account_id in &invited {
                let detail = details.get(account_id).unwrap_or_else(|| {
                    panic!("case {case:?}: {reader} reported nothing for {account_id}")
                });
                let states = ingest_states(case, detail);
                assert_eq!(
                    states, want_invited,
                    "case {case:?}: {reader} expected {want_invited:?} for the invited \
                     account {account_id}, got {states:?}"
                );
            }
            let administrator = details.get(ACCOUNT).unwrap_or_else(|| {
                panic!("case {case:?}: {reader} reported no membership for the administrator")
            });
            let states = ingest_states(case, administrator);
            assert_eq!(
                states, want_administrator,
                "case {case:?}: {reader} expected {want_administrator:?} for the \
                 administrator's own membership, got {states:?}"
            );
            // No map is ever short: every package the graph holds a record for is in it.
            assert_eq!(
                states.len(),
                PACKAGES.len(),
                "case {case:?}: {reader} expected one entry per modelled package, got {states:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_graph_holds_an_enabled_membership_for_its_administrator_that_no_removal_takes_away() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // Before any account is invited, the graph's one membership is the administrator's
    // own, and it is ENABLED: "the member account currently contributes data to the
    // behavior graph".
    let case = "freshly-created";
    let listed = listed_members(&router, case, &graph_arn).await;
    assert_eq!(
        listed.keys().cloned().collect::<Vec<String>>(),
        vec![ACCOUNT.to_string()],
        "case {case:?}: expected the administrator's own membership alone, got {listed:?}"
    );
    let administrator = &listed[ACCOUNT];
    assert_eq!(
        administrator.get("Status").and_then(Value::as_str),
        Some(MEMBER_STATUS_ENABLED),
        "case {case:?}: got {administrator:?}"
    );
    assert_eq!(
        administrator.get("AdministratorId").and_then(Value::as_str),
        Some(ACCOUNT),
        "case {case:?}: got {administrator:?}"
    );
    assert_eq!(
        administrator.get("MasterId").and_then(Value::as_str),
        Some(ACCOUNT),
        "case {case:?}: the deprecated spelling carries the same value, got {administrator:?}"
    );
    assert_eq!(
        administrator.get("GraphArn").and_then(Value::as_str),
        Some(graph_arn.as_str()),
        "case {case:?}: got {administrator:?}"
    );
    // Nobody invited the account and no organization enrolled it, so there is no
    // root-user email address and no membership type to report for it.
    assert_eq!(
        administrator.get("EmailAddress"),
        None,
        "case {case:?}: got {administrator:?}"
    );
    assert_eq!(
        administrator.get("InvitationType"),
        None,
        "case {case:?}: got {administrator:?}"
    );
    // Its two instants are the one instant the membership came into being — the graph's
    // own creation — so they are reported, as the modelled `date-time`, and equal.
    let invited_time = assert_iso8601(
        &format!("{case}/InvitedTime"),
        administrator.get("InvitedTime"),
    );
    let updated_time = assert_iso8601(
        &format!("{case}/UpdatedTime"),
        administrator.get("UpdatedTime"),
    );
    assert_eq!(
        invited_time, updated_time,
        "case {case:?}: nothing has updated the membership since the graph created it, \
         got {administrator:?}"
    );

    // The same record is what `GetMembers` answers for that account.
    let got = got_members(&router, case, &graph_arn, &[ACCOUNT.to_string()]).await;
    assert_eq!(
        got.get(ACCOUNT),
        Some(administrator),
        "case {case:?}: expected GetMembers and ListMembers to report one record, got {got:?}"
    );

    // The rows differ along what a removal batch names: the administrator alone, the
    // administrator beside a member, and every invited account at once. None of them can
    // take the administrator's own membership away.
    let invited: Vec<String> = (0..2).map(account_id).collect();
    create_members(&router, "invite", &graph_arn, &invited).await;
    let rows: [(&str, Vec<String>); 3] = [
        ("the-administrator-alone", vec![ACCOUNT.to_string()]),
        (
            "the-administrator-beside-a-member",
            vec![ACCOUNT.to_string(), invited[0].clone()],
        ),
        ("every-invited-account", invited.clone()),
    ];
    for (case, requested) in rows {
        let (status, answered) = call(
            &router,
            REMOVAL_URI,
            &json!({ "GraphArn": graph_arn, "AccountIds": requested }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMembers expected 200, got {status} {answered:?}"
        );
        let unprocessed = answered
            .get("UnprocessedAccounts")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: no UnprocessedAccounts in {answered:?}")
            });
        if requested.contains(&ACCOUNT.to_string()) {
            assert!(
                unprocessed
                    .iter()
                    .any(|entry| entry.get("AccountId").and_then(Value::as_str) == Some(ACCOUNT)),
                "case {case:?}: expected the administrator reported unprocessed, got {answered:?}"
            );
        }
        let listed = listed_members(&router, case, &graph_arn).await;
        assert!(
            listed.contains_key(ACCOUNT),
            "case {case:?}: the administrator's own membership must survive, got {listed:?}"
        );
        assert_eq!(
            listed[ACCOUNT].get("Status").and_then(Value::as_str),
            Some(MEMBER_STATUS_ENABLED),
            "case {case:?}: and it is still ENABLED, got {listed:?}"
        );
    }

    // Every invited account is gone, and the listing is the administrator's own
    // membership rather than empty.
    let listed = listed_members(&router, "every-member-removed", &graph_arn).await;
    assert_eq!(
        listed.keys().cloned().collect::<Vec<String>>(),
        vec![ACCOUNT.to_string()],
        "expected only the administrator's own membership to remain, got {listed:?}"
    );
}

#[tokio::test]
async fn a_memberships_type_is_reported_only_once_its_verification_has_settled() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // The rows differ along how many accounts one call invites, which is what the rule
    // generalises over: each membership carries its own verification.
    let invited: Vec<String> = (0..3).map(account_id).collect();
    let case = "created";
    let created = create_members(&router, case, &graph_arn, &invited).await;
    for detail in &created {
        assert_eq!(
            detail.get("Status").and_then(Value::as_str),
            Some(MEMBER_STATUS_VERIFICATION_IN_PROGRESS),
            "case {case:?}: the membership is created mid-verification, got {detail:?}"
        );
        assert_eq!(
            detail.get("InvitationType"),
            None,
            "case {case:?}: no type is reported while the verification is in progress, \
             got {detail:?}"
        );
    }

    // Once the clock has made the verification due, the settled membership reports the
    // type that created it — on every reader.
    tokio::time::sleep(std::time::Duration::from_millis(
        (MEMBER_VERIFICATION_MILLIS as u64) + 250,
    ))
    .await;
    let case = "settled";
    let mut asked = invited.clone();
    asked.push(ACCOUNT.to_string());
    let got = got_members(&router, case, &graph_arn, &asked).await;
    let listed = listed_members(&router, case, &graph_arn).await;
    for (reader, details) in [("GetMembers", &got), ("ListMembers", &listed)] {
        for account_id in &invited {
            let detail = details.get(account_id).unwrap_or_else(|| {
                panic!("case {case:?}: {reader} reported nothing for {account_id}")
            });
            assert_eq!(
                detail.get("Status").and_then(Value::as_str),
                Some(MEMBER_STATUS_VERIFICATION_FAILED),
                "case {case:?}: {reader} expected the settled status for {account_id}, \
                 got {detail:?}"
            );
            assert_eq!(
                detail.get("InvitationType").and_then(Value::as_str),
                Some(INVITATION_TYPE_INVITATION),
                "case {case:?}: {reader} expected the settled membership to report the \
                 type that created it for {account_id}, got {detail:?}"
            );
        }
        // The administrator's own membership came from no invitation at all, so it never
        // reports a type.
        let administrator = details.get(ACCOUNT).unwrap_or_else(|| {
            panic!("case {case:?}: {reader} reported no membership for the administrator")
        });
        assert_eq!(
            administrator.get("InvitationType"),
            None,
            "case {case:?}: {reader} expected no type on the administrator's own \
             membership, got {administrator:?}"
        );
    }
}

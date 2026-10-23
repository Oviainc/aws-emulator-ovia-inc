// SPDX-License-Identifier: CC0-1.0

//! `DescribeSourceServers`, end to end over HTTP.
//!
//! The rule under test: for an account `InitializeService` has run for,
//! `DescribeSourceServers` reports exactly that account's stored source servers, in
//! registration order, narrowed by every filter the request carries, one
//! `maxResults`-sized page at a time, and reports nothing it computes per read; for an
//! account it has not run for, it answers the modelled `UninitializedAccountException`
//! (400). Each test below spans a CLASS of inputs — a filter table, the whole
//! `LifeCycleState` catalog, every page size — rather than one recorded request.
//!
//! Nothing in this service registers a source server yet (in AWS the replication agent
//! does, and that protocol is not part of this API), so the fleet a read reports is
//! seeded straight into the store, exactly as `application_lifecycle.rs` seeds a
//! precondition no sequence of calls can reach.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::ops::types::LifeCycle;
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    source_server_arn, SourceServer,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT, REGION};

/// The model's `LifeCycleState` values, in model order
/// (`smithy_query(query="shape", shape="LifeCycleState")` -> `values`).
const LIFE_CYCLE_STATES: [&str; 10] = [
    "STOPPED",
    "NOT_READY",
    "READY_FOR_TEST",
    "TESTING",
    "READY_FOR_CUTOVER",
    "CUTTING_OVER",
    "CUTOVER",
    "DISCONNECTED",
    "DISCOVERED",
    "PENDING_INSTALLATION",
];

/// The model's `ReplicationType` values
/// (`smithy_query(query="shape", shape="ReplicationType")` -> `values`).
const REPLICATION_TYPES: [&str; 2] = ["AGENT_BASED", "SNAPSHOT_SHIPPING"];

/// A `sourceServerID` of the shape the model requires: `^s-[0-9a-zA-Z]{17}$`,
/// length 19..19 — so every id this test seeds is one the service could have minted.
fn server_id(ordinal: usize) -> String {
    let id = format!("s-{ordinal:017}");
    assert_eq!(
        id.len(),
        19,
        "case {ordinal:?}: a seeded sourceServerID must be 19 characters, got {id:?}"
    );
    id
}

/// An `applicationID` of the shape the model requires: `^app-[0-9a-zA-Z]{17}$`.
fn application_id(ordinal: usize) -> String {
    format!("app-{ordinal:017}")
}

/// One row of the seeded fleet: the stored record whatever registers a source server
/// (an agent installation, a vCenter discovery, an import) would have left behind.
struct Seeded {
    ordinal: usize,
    account: &'static str,
    is_archived: bool,
    replication_type: Option<&'static str>,
    life_cycle_state: Option<&'static str>,
    application: Option<usize>,
}

/// The fleet every filter case below selects from: it varies along each member a
/// filter selects on — archived or not, both `ReplicationType` values and none at all,
/// four different life cycle states and none at all, two applications and none — plus
/// one server belonging to ANOTHER account.
fn fleet() -> Vec<Seeded> {
    vec![
        Seeded {
            ordinal: 1,
            account: EMULATOR_ACCOUNT,
            is_archived: false,
            replication_type: Some("AGENT_BASED"),
            life_cycle_state: Some("DISCOVERED"),
            application: None,
        },
        Seeded {
            ordinal: 2,
            account: EMULATOR_ACCOUNT,
            is_archived: false,
            replication_type: Some("AGENT_BASED"),
            life_cycle_state: Some("READY_FOR_TEST"),
            application: Some(1),
        },
        Seeded {
            ordinal: 3,
            account: EMULATOR_ACCOUNT,
            is_archived: true,
            replication_type: Some("SNAPSHOT_SHIPPING"),
            life_cycle_state: Some("CUTOVER"),
            application: Some(1),
        },
        Seeded {
            ordinal: 4,
            account: EMULATOR_ACCOUNT,
            is_archived: false,
            replication_type: Some("SNAPSHOT_SHIPPING"),
            life_cycle_state: Some("PENDING_INSTALLATION"),
            application: Some(2),
        },
        // A record carrying neither a replication type nor a life cycle: a list filter
        // must not select it, and an unfiltered read must.
        Seeded {
            ordinal: 5,
            account: EMULATOR_ACCOUNT,
            is_archived: false,
            replication_type: None,
            life_cycle_state: None,
            application: None,
        },
        Seeded {
            ordinal: 6,
            account: OTHER_ACCOUNT,
            is_archived: false,
            replication_type: Some("AGENT_BASED"),
            life_cycle_state: Some("DISCOVERED"),
            application: None,
        },
    ]
}

/// Put one source server straight into the store, as a registration would have.
fn seed(mgn: &Mgn, row: &Seeded) {
    let id = server_id(row.ordinal);
    let seen = "2026-10-22T09:15:00.000000000Z".to_string();
    let stored = mgn.store().lock().insert_source_server(SourceServer {
        source_server_id: id.clone(),
        arn: source_server_arn(REGION, &id),
        account_id: row.account.to_string(),
        is_archived: row.is_archived,
        tags: BTreeMap::from([("env".to_string(), "dev".to_string())]),
        replication_type: row.replication_type.map(ToString::to_string),
        life_cycle: row.life_cycle_state.map(|state| LifeCycle {
            added_to_service_date_time: Some(seen.clone()),
            last_seen_by_service_date_time: Some(seen.clone()),
            state: Some(state.to_string()),
            ..LifeCycle::default()
        }),
        application_id: row.application.map(application_id),
        user_provided_id: Some(format!("host-{}", row.ordinal)),
        fqdn_for_action_framework: Some(format!("host-{}.example.internal", row.ordinal)),
        ..SourceServer::default()
    });
    assert!(
        stored,
        "case {id:?}: seeding must store the record (the store bound was not reached)"
    );
}

/// Seed `rows` into a freshly initialized emulator, with both accounts initialized so
/// account scoping is tested on reads that are otherwise allowed.
async fn emulator_with(rows: &[Seeded]) -> Mgn {
    let mgn = Mgn::new();
    mgn.initialize().await;
    // `InitializeService` initializes only the caller's own account, so the second
    // account is marked initialized directly — the same seam the application suite uses.
    mgn.store()
        .lock()
        .initialized_accounts
        .insert(OTHER_ACCOUNT.to_string());
    for row in rows {
        seed(&mgn, row);
    }
    mgn
}

/// The `sourceServerID`s a `DescribeSourceServers` answer lists, in answer order.
fn reported_ids(answer: &Value) -> Vec<String> {
    answer["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items array in {answer:?}"))
        .iter()
        .map(|item| {
            item["sourceServerID"]
                .as_str()
                .unwrap_or_else(|| panic!("no sourceServerID in {item:?}"))
                .to_string()
        })
        .collect()
}

/// `DescribeSourceServers` with `request`, asserting 200 and answering the body.
async fn describe(mgn: &Mgn, case: &str, request: &Value) -> Value {
    let (status, body) = mgn.call("DescribeSourceServers", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSourceServers expected 200, got {status} with {body:?}"
    );
    body
}

/// Every modelled filter narrows the stored fleet, and the ordinals it leaves are the
/// ones the answer lists — in registration order. An absent filter, and a
/// present-but-empty list, select everything of the account.
#[tokio::test]
async fn describe_source_servers_honours_every_modelled_filter() {
    let mgn = emulator_with(&fleet()).await;

    // (case, filters, the ordinals of the servers the request must select)
    let cases: Vec<(&str, Value, Vec<usize>)> = vec![
        ("no filters member", json!({}), vec![1, 2, 3, 4, 5]),
        (
            "filters present but empty",
            json!({"filters": {}}),
            vec![1, 2, 3, 4, 5],
        ),
        (
            "sourceServerIDs naming one server",
            json!({"filters": {"sourceServerIDs": [server_id(3)]}}),
            vec![3],
        ),
        (
            "sourceServerIDs naming a stored and an unknown id",
            json!({"filters": {"sourceServerIDs": [server_id(2), server_id(99)]}}),
            vec![2],
        ),
        (
            "sourceServerIDs naming only an unknown id",
            json!({"filters": {"sourceServerIDs": [server_id(99)]}}),
            vec![],
        ),
        (
            "sourceServerIDs empty",
            json!({"filters": {"sourceServerIDs": []}}),
            vec![1, 2, 3, 4, 5],
        ),
        (
            "isArchived true",
            json!({"filters": {"isArchived": true}}),
            vec![3],
        ),
        (
            "isArchived false",
            json!({"filters": {"isArchived": false}}),
            vec![1, 2, 4, 5],
        ),
        (
            "replicationTypes SNAPSHOT_SHIPPING",
            json!({"filters": {"replicationTypes": ["SNAPSHOT_SHIPPING"]}}),
            vec![3, 4],
        ),
        (
            "replicationTypes both modelled values",
            json!({"filters": {"replicationTypes": REPLICATION_TYPES}}),
            vec![1, 2, 3, 4],
        ),
        (
            "replicationTypes empty",
            json!({"filters": {"replicationTypes": []}}),
            vec![1, 2, 3, 4, 5],
        ),
        (
            "lifeCycleStates PENDING_INSTALLATION",
            json!({"filters": {"lifeCycleStates": ["PENDING_INSTALLATION"]}}),
            vec![4],
        ),
        (
            "lifeCycleStates naming two states",
            json!({"filters": {"lifeCycleStates": ["DISCOVERED", "CUTOVER"]}}),
            vec![1, 3],
        ),
        (
            "lifeCycleStates naming a state nothing is in",
            json!({"filters": {"lifeCycleStates": ["TESTING"]}}),
            vec![],
        ),
        (
            "applicationIDs naming one application",
            json!({"filters": {"applicationIDs": [application_id(1)]}}),
            vec![2, 3],
        ),
        (
            "applicationIDs naming both applications",
            json!({"filters": {"applicationIDs": [application_id(1), application_id(2)]}}),
            vec![2, 3, 4],
        ),
        (
            "two filters together",
            json!({"filters": {"isArchived": false, "replicationTypes": ["AGENT_BASED"]}}),
            vec![1, 2],
        ),
        (
            "filters that cannot hold together",
            json!({"filters": {"applicationIDs": [application_id(2)], "lifeCycleStates": ["CUTOVER"]}}),
            vec![],
        ),
        (
            "every filter at once",
            json!({"filters": {
                "sourceServerIDs": [server_id(3), server_id(4)],
                "isArchived": true,
                "replicationTypes": ["SNAPSHOT_SHIPPING"],
                "lifeCycleStates": ["CUTOVER"],
                "applicationIDs": [application_id(1)],
            }}),
            vec![3],
        ),
    ];

    for (case, request, expected) in cases {
        let answer = describe(&mgn, case, &request).await;
        let expected: Vec<String> = expected.into_iter().map(server_id).collect();
        assert_eq!(
            reported_ids(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {answer:?}"
        );
    }
}

/// The `lifeCycleStates` filter covers the WHOLE modelled catalog, not the few states
/// the fleet above uses: with one server in each of the model's ten `LifeCycleState`
/// values, filtering by any one state reports exactly that server.
#[tokio::test]
async fn describe_source_servers_filters_on_every_modelled_life_cycle_state() {
    let rows: Vec<Seeded> = LIFE_CYCLE_STATES
        .iter()
        .enumerate()
        .map(|(index, state)| Seeded {
            ordinal: index + 1,
            account: EMULATOR_ACCOUNT,
            is_archived: false,
            replication_type: Some(REPLICATION_TYPES[index % REPLICATION_TYPES.len()]),
            life_cycle_state: Some(state),
            application: None,
        })
        .collect();
    let mgn = emulator_with(&rows).await;

    for (index, state) in LIFE_CYCLE_STATES.iter().enumerate() {
        let request = json!({"filters": {"lifeCycleStates": [state]}});
        let answer = describe(&mgn, state, &request).await;
        assert_eq!(
            reported_ids(&answer),
            vec![server_id(index + 1)],
            "case {state:?}: exactly the server in that state must be reported, got {answer:?}"
        );
        let reported_state = answer["items"][0]["lifeCycle"]["state"].as_str();
        assert_eq!(
            reported_state,
            Some(*state),
            "case {state:?}: the reported lifeCycle.state must be the stored one, got {reported_state:?}"
        );
    }
}

/// Paging reports every selected server exactly once, in registration order, for any
/// page size: a `nextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn describe_source_servers_pages_without_repeating_or_dropping_items() {
    let mgn = emulator_with(&fleet()).await;
    let all: Vec<String> = (1..=5).map(server_id).collect();

    // Page sizes below, at and above the selection size, plus both published bounds of
    // the modelled `maxResults` range 1..1000.
    for page_size in [1, 2, 3, 4, 5, 6, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let case = format!("maxResults {page_size}, page {pages}");
            let answer = describe(&mgn, &case, &request).await;
            let page = reported_ids(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case {case:?}: a page must hold at most maxResults items, got {page:?}"
            );
            seen.extend(page);
            token = answer["nextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging must terminate, saw {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {page_size:?}: paging must report every server exactly once in registration order, got {seen:?}"
        );
    }

    // A page size that leaves nothing behind carries no token at all.
    let answer = describe(&mgn, "whole selection in one page", &json!({})).await;
    assert!(
        answer["nextToken"].is_null(),
        "the last page must carry no nextToken, got {answer:?}"
    );
}

/// A `nextToken` this operation cannot resume from is refused with the modelled
/// `ValidationException` (400), not served as page one or as some other page.
#[tokio::test]
async fn an_unresumable_next_token_is_refused() {
    let mgn = emulator_with(&fleet()).await;

    let first = describe(&mgn, "first page", &json!({"maxResults": 1})).await;
    let live_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of 5 must carry a nextToken: {first:?}"))
        .to_string();

    let refused: Vec<(&str, Value)> = vec![
        ("a string this service never issued", json!("not-a-token")),
        (
            "another operation's token",
            json!(format!("mgn-applications-{}", application_id(1))),
        ),
        (
            "this operation's marker with an unknown anchor",
            json!(format!("mgn-source-servers-{}", server_id(99))),
        ),
    ];
    for (case, token) in refused {
        let (status, body) = mgn
            .call("DescribeSourceServers", &json!({"nextToken": token}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body:?}"
        );
    }

    // A token whose anchor is no longer in the selection cannot be resumed either: the
    // same token, re-presented with a filter that excludes its anchor, is refused.
    let (status, body) = mgn
        .call(
            "DescribeSourceServers",
            &json!({"nextToken": live_token, "filters": {"isArchived": true}}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "a token whose anchor left the selection must be refused, got {status} with {body:?}"
    );

    // An empty `nextToken` is the modelled minimum length, and means "from the start".
    let answer = describe(&mgn, "empty nextToken", &json!({"nextToken": ""})).await;
    assert_eq!(
        reported_ids(&answer),
        (1..=5).map(server_id).collect::<Vec<String>>(),
        "an empty nextToken must read from the start, got {answer:?}"
    );
}

/// Until `InitializeService` has run for the account the request acts on,
/// `DescribeSourceServers` answers the modelled `UninitializedAccountException` (400)
/// — whether or not the store holds records for it, and for any `accountID`.
#[tokio::test]
async fn describe_source_servers_refuses_an_uninitialized_account() {
    let mgn = Mgn::new();

    // (case, request) — the uninitialized refusal does not depend on the filters, the
    // page size or the account named.
    let requests: Vec<(&str, Value)> = vec![
        ("no members", json!({})),
        ("the caller's own account", json!({"accountID": EMULATOR_ACCOUNT})),
        ("another account", json!({"accountID": OTHER_ACCOUNT})),
        (
            "filters and a page size",
            json!({"maxResults": 10, "filters": {"isArchived": false}}),
        ),
    ];
    for (case, request) in &requests {
        let (status, body) = mgn.call("DescribeSourceServers", request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "UninitializedAccountException"),
            "case {case:?}: DescribeSourceServers before InitializeService must be 400 UninitializedAccountException, got {status} with {body:?}"
        );
        // One text for one condition: the refusal is about the account the request is
        // signed for, so it names no value taken off the request.
        assert_eq!(
            body["message"].as_str(),
            Some("Account not initialized"),
            "case {case:?}: the refusal must carry the service's fixed message, got {body:?}"
        );
    }

    // Records in the store do not make the refusal go away, and initializing one
    // account does not initialize another.
    for row in fleet() {
        seed(&mgn, &row);
    }
    mgn.initialize().await;
    let answer = describe(&mgn, "own account after initialize", &json!({})).await;
    assert_eq!(
        reported_ids(&answer),
        (1..=5).map(server_id).collect::<Vec<String>>(),
        "an initialized account reads its own servers, got {answer:?}"
    );
    let (status, body) = mgn
        .call("DescribeSourceServers", &json!({"accountID": OTHER_ACCOUNT}))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UninitializedAccountException"),
        "an account InitializeService has not run for stays refused, got {status} with {body:?}"
    );
}

/// A read reaches only the records of the account it acts on: neither account sees the
/// other's source server, with or without filters that would otherwise select it.
#[tokio::test]
async fn one_accounts_read_cannot_see_another_accounts_source_server() {
    let mgn = emulator_with(&fleet()).await;
    let foreign = server_id(6);

    for (case, request) in [
        ("no filters", json!({})),
        (
            "a filter naming the foreign server",
            json!({"filters": {"sourceServerIDs": [foreign.clone()]}}),
        ),
    ] {
        let answer = describe(&mgn, case, &request).await;
        assert!(
            !reported_ids(&answer).contains(&foreign),
            "case {case:?}: another account's source server must never be reported, got {answer:?}"
        );
    }

    // And the other account sees only its own.
    let answer = describe(
        &mgn,
        "the other account's own read",
        &json!({"accountID": OTHER_ACCOUNT}),
    )
    .await;
    assert_eq!(
        reported_ids(&answer),
        vec![foreign],
        "an account reads exactly its own source servers, got {answer:?}"
    );
}

/// The answer is the stored record: every member comes from the store, so two reads of
/// an unchanged fleet are identical and no read changes what a later caller sees.
#[tokio::test]
async fn a_read_reports_the_stored_record_and_changes_nothing() {
    let mgn = emulator_with(&fleet()).await;
    let before = format!("{:?}", mgn.store().lock().source_servers_of(EMULATOR_ACCOUNT));

    let first = describe(&mgn, "first read", &json!({})).await;
    let stored = mgn
        .store()
        .lock()
        .source_server_of(EMULATOR_ACCOUNT, &server_id(3))
        .cloned()
        .unwrap_or_else(|| panic!("the seeded server must be in the store"));
    let reported = &first["items"][2];
    let expected = json!({
        "sourceServerID": stored.source_server_id,
        "arn": stored.arn,
        "isArchived": stored.is_archived,
        "replicationType": stored.replication_type,
        "applicationID": stored.application_id,
        "userProvidedID": stored.user_provided_id,
        "fqdnForActionFramework": stored.fqdn_for_action_framework,
        "tags": {"env": "dev"},
        "lifeCycle": {
            "addedToServiceDateTime": "2026-10-22T09:15:00.000000000Z",
            "lastSeenByServiceDateTime": "2026-10-22T09:15:00.000000000Z",
            "state": stored.life_cycle_state(),
        },
    });
    assert_eq!(
        reported, &expected,
        "the reported record must be the stored one, got {reported:?}"
    );

    // Reading again — unfiltered, filtered and paged — answers the same thing and
    // leaves the store byte-for-byte as it was.
    let again = describe(&mgn, "second read", &json!({})).await;
    assert_eq!(
        again, first,
        "two reads of an unchanged fleet must be identical, got {again:?}"
    );
    let _ = describe(&mgn, "paged read", &json!({"maxResults": 2})).await;
    let _ = describe(
        &mgn,
        "filtered read",
        &json!({"filters": {"isArchived": true}}),
    )
    .await;
    let after = format!("{:?}", mgn.store().lock().source_servers_of(EMULATOR_ACCOUNT));
    assert_eq!(
        after, before,
        "a read must not change the store, got {after} after {before}"
    );
}

/// The modelled input constraints are refused at the boundary, on each published bound
/// and one past it, rather than coerced by the handler.
#[tokio::test]
async fn modelled_input_constraints_are_refused() {
    let mgn = emulator_with(&fleet()).await;

    // (case, body) — `maxResults` range 1..1000 and `accountID` length 12 / pattern
    // [0-9]{12,}, both from the model.
    for (case, body) in [
        ("maxResults at the minimum", json!({"maxResults": 1})),
        ("maxResults at the maximum", json!({"maxResults": 1000})),
    ] {
        let (status, answer) = mgn.call("DescribeSourceServers", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value on a published bound must be accepted, got {status} with {answer:?}"
        );
    }

    for (case, body) in [
        ("maxResults below the minimum", json!({"maxResults": 0})),
        ("maxResults above the maximum", json!({"maxResults": 1001})),
        ("negative maxResults", json!({"maxResults": -1})),
        ("accountID that is not 12 digits", json!({"accountID": "1234"})),
        (
            "accountID that is not digits at all",
            json!({"accountID": "abcdefghijkl"}),
        ),
    ] {
        let (status, answer) = mgn.call("DescribeSourceServers", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside a modelled constraint must be refused with 400, got {status} with {answer:?}"
        );
    }
}

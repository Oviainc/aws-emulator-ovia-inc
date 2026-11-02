// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's not-found and validation semantics.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one input
//! a recording happened to carry:
//!
//! 1. A request naming a behavior graph the service does not hold is refused with the
//!    modelled `ResourceNotFoundException` (404) carrying the fixed message that
//!    operation's surface publishes — or, for the DELETE of a graph that is already gone,
//!    no message member at all — and never echoing the caller-supplied ARN, whatever the
//!    operation and however the graph came to be absent.
//! 2. A well-formed id of a resource scoped to a graph that the graph does not hold is
//!    the same modelled 404, told apart from "no such graph" by its own fixed message;
//!    an id outside its modelled constraints is still the framework's generic 400.
//! 3. `CreateMembers` reports a `@pattern` failure of a member of an `Accounts` entry
//!    with Detective's own fixed `<Shape>.<Member> did not match the pattern.`,
//!    independent of the offending value and of its position in the list.
//! 4. `UpdateDatasourcePackages` validates the requested package SET, naming the
//!    values it did not recognise and the mandatory package a request left out.
//! 5. A tag key carrying the reserved `aws:` prefix (case-insensitively) is refused
//!    with `BadRequestException` at HTTP 400 before any tag in the request is applied.
//! 6. An operation that acts on a membership resolves one the member account ACCEPTED:
//!    an account the graph holds no record of is the 404 with the missing-membership
//!    text, a record that never got past invitation or verification is the same 404 under
//!    the text that names that fact, and neither call changes the stored record.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    service_state, DATASOURCE_PACKAGE_STATE_STARTED, DATASOURCE_PACKAGE_STATE_STOPPED,
    GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_ID_LEN,
    INVESTIGATION_RUN_MILLIS, INVITATION_NOT_ACCEPTED_MESSAGE, MANDATORY_DATASOURCE_PACKAGE,
    MAX_TAGS_PER_GRAPH, NO_SUCH_MEMBERSHIP_MESSAGE, RESERVED_TAG_KEY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, assert_iso8601, call, call_with, create_graph_ok, emulator,
    emulator_with_engine, error_code, error_message, ACCOUNT, REGION,
};

/// A `GraphArn` that satisfies the modelled pattern and that nothing was ever stored
/// under.
fn never_created_arn(id_char: char) -> String {
    format!(
        "arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}",
        id_char.to_string().repeat(32)
    )
}

/// One request that resolves a behavior graph: the operation's URI, its HTTP method,
/// and a body builder that puts `graph_arn` where that operation names it (`TagResource`
/// and `ListTagsForResource` name it in the URI, so their builders put it there too).
struct GraphScopedCall {
    name: &'static str,
    method: &'static str,
    uri: fn(&str) -> String,
    body: fn(&str) -> Value,
}

/// The fixed message rule 1 says the operation called `name` refuses an absent graph
/// with, and `None` for the one operation whose refusal states no message at all.
///
/// Each text is written exactly once in `crate::state` and shared by every operation it
/// belongs to: the operations bound under this service's `/investigations/` URIs word an
/// unresolvable graph with the investigation surface's sentence; `RejectInvitation`
/// resolves the CALLING account's membership rather than the graph, so it words the same
/// request as a missing membership and discloses nothing about the graph; `DeleteGraph`
/// states nothing — the recorded delete of a graph that is already gone carries no message
/// member; every other graph-scoped operation carries the graph-not-found sentence.
fn absent_graph_message(name: &str) -> Option<&'static str> {
    match name {
        "StartInvestigation" | "GetInvestigation" => Some(INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE),
        "RejectInvitation" => Some(NO_SUCH_MEMBERSHIP_MESSAGE),
        "DeleteGraph" => None,
        _ => Some(GRAPH_NOT_FOUND_MESSAGE),
    }
}

/// The table for rule 1: EVERY operation of this service that resolves a behavior
/// graph, so the rule is asserted where it holds rather than only on the one operation
/// a recording exercised.
fn graph_scoped_calls() -> Vec<GraphScopedCall> {
    vec![
        GraphScopedCall {
            name: "CreateMembers",
            method: "POST",
            uri: |_| "/graph/members".to_string(),
            body: |arn| json!({ "GraphArn": arn, "Accounts": [account(&account_id(0))] }),
        },
        GraphScopedCall {
            name: "ListMembers",
            method: "POST",
            uri: |_| "/graph/members/list".to_string(),
            body: |arn| json!({ "GraphArn": arn }),
        },
        GraphScopedCall {
            name: "GetMembers",
            method: "POST",
            uri: |_| "/graph/members/get".to_string(),
            body: |arn| json!({ "GraphArn": arn, "AccountIds": [account_id(0)] }),
        },
        GraphScopedCall {
            name: "DeleteMembers",
            method: "POST",
            uri: |_| "/graph/members/removal".to_string(),
            body: |arn| json!({ "GraphArn": arn, "AccountIds": [account_id(0)] }),
        },
        GraphScopedCall {
            name: "DeleteGraph",
            method: "POST",
            uri: |_| "/graph/removal".to_string(),
            body: |arn| json!({ "GraphArn": arn }),
        },
        GraphScopedCall {
            name: "RejectInvitation",
            method: "POST",
            uri: |_| "/invitation/removal".to_string(),
            body: |arn| json!({ "GraphArn": arn }),
        },
        GraphScopedCall {
            name: "StartMonitoringMember",
            method: "POST",
            uri: |_| "/graph/member/monitoringstate".to_string(),
            body: |arn| json!({ "GraphArn": arn, "AccountId": account_id(0) }),
        },
        GraphScopedCall {
            name: "GetInvestigation",
            method: "POST",
            uri: |_| "/investigations/getInvestigation".to_string(),
            body: |arn| json!({ "GraphArn": arn, "InvestigationId": "1".repeat(INVESTIGATION_ID_LEN) }),
        },
        GraphScopedCall {
            name: "StartInvestigation",
            method: "POST",
            uri: |_| "/investigations/startInvestigation".to_string(),
            body: |arn| {
                json!({
                    "GraphArn": arn,
                    "EntityArn": format!("arn:aws:iam::{ACCOUNT}:user/alice"),
                    "ScopeStartTime": "2021-08-18T16:35:56Z",
                    "ScopeEndTime": "2021-08-19T16:35:56Z",
                })
            },
        },
        GraphScopedCall {
            name: "UpdateDatasourcePackages",
            method: "POST",
            uri: |_| "/graph/datasources/update".to_string(),
            body: |arn| {
                json!({ "GraphArn": arn, "DatasourcePackages": [MANDATORY_DATASOURCE_PACKAGE] })
            },
        },
        GraphScopedCall {
            name: "ListDatasourcePackages",
            method: "POST",
            uri: |_| "/graph/datasources/list".to_string(),
            body: |arn| json!({ "GraphArn": arn }),
        },
        GraphScopedCall {
            name: "TagResource",
            method: "POST",
            uri: |arn| format!("/tags/{arn}"),
            body: |_| json!({ "Tags": { "Env": "test" } }),
        },
        GraphScopedCall {
            name: "ListTagsForResource",
            method: "GET",
            uri: |arn| format!("/tags/{arn}"),
            body: |_| Value::Null,
        },
        GraphScopedCall {
            // `UntagResource` binds `TagKeys` to the repeated `tagKeys` query
            // parameter, so its keys live in the URI beside the ARN.
            name: "UntagResource",
            method: "DELETE",
            uri: |arn| format!("/tags/{arn}?tagKeys=Env"),
            body: |_| Value::Null,
        },
    ]
}

#[tokio::test]
async fn a_graph_the_service_does_not_hold_is_refused_with_its_own_fixed_message() {
    let router = emulator();

    // The rows differ along HOW the graph came to be absent: one ARN was never minted
    // at all, the other names a graph that existed and was deleted. Both are "the
    // service does not hold this graph", so both must read identically to a caller.
    let deleted = create_graph_ok(&router, REGION, &json!({ "Tags": { "Env": "test" } })).await;
    let (status, _) = call(&router, "/graph/removal", &json!({ "GraphArn": deleted })).await;
    assert_eq!(status, 200, "DeleteGraph on the graph about to be absent");
    let absent = [("never-created", never_created_arn('a')), ("deleted", deleted)];

    for (how, arn) in &absent {
        for call_case in graph_scoped_calls() {
            let case = format!("{} on a {how} graph", call_case.name);
            let (status, answered) = call_with(
                &router,
                call_case.method,
                REGION,
                &(call_case.uri)(arn),
                &(call_case.body)(arn),
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
            let message = error_message(&answered);
            match absent_graph_message(call_case.name) {
                Some(expected) => assert_eq!(
                    message, expected,
                    "case {case:?}: expected this surface's fixed absent-graph message, \
                     got {message:?}"
                ),
                // The one refusal that states nothing: the body carries no message member,
                // which the harness reads back as this sentinel.
                None => assert_eq!(
                    message, "<no message>",
                    "case {case:?}: expected a refusal carrying no message member, got \
                     {message:?}"
                ),
            }
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?}: the message must not echo the requested ARN, got {message:?}"
            );
        }
    }
}

/// One row of the investigation table: an entity ARN, and the `EntityType` Detective
/// resolves it to — or `None` for an ARN naming neither of the two kinds it supports
/// ("Detective supports IAM user ARNs and IAM role ARNs",
/// `smithy_query(query="operation",operation="StartInvestigation")`).
struct EntityCase {
    name: &'static str,
    entity_arn: String,
    entity_type: Option<&'static str>,
}

fn entity_cases() -> Vec<EntityCase> {
    vec![
        EntityCase {
            name: "iam-user",
            entity_arn: format!("arn:aws:iam::{ACCOUNT}:user/alice"),
            entity_type: Some("IAM_USER"),
        },
        EntityCase {
            name: "iam-user-with-path",
            entity_arn: format!("arn:aws:iam::{ACCOUNT}:user/engineering/bob"),
            entity_type: Some("IAM_USER"),
        },
        EntityCase {
            name: "iam-role",
            entity_arn: format!("arn:aws:iam::{ACCOUNT}:role/auditor"),
            entity_type: Some("IAM_ROLE"),
        },
        EntityCase {
            name: "assumed-role-session",
            entity_arn: format!("arn:aws:sts::{ACCOUNT}:assumed-role/auditor/session-1"),
            entity_type: Some("IAM_ROLE"),
        },
        EntityCase {
            name: "iam-group-is-not-an-entity",
            entity_arn: format!("arn:aws:iam::{ACCOUNT}:group/admins"),
            entity_type: None,
        },
        EntityCase {
            name: "bucket-is-not-an-entity",
            entity_arn: "arn:aws:s3:::example-bucket".to_string(),
            entity_type: None,
        },
        EntityCase {
            name: "nameless-user-is-not-an-entity",
            entity_arn: format!("arn:aws:iam::{ACCOUNT}:user/"),
            entity_type: None,
        },
    ]
}

/// `StartInvestigation` on `graph_arn` for `entity_arn`, scoped to the window the two
/// timestamps name.
async fn start_investigation(
    router: &MultiServiceRouter,
    graph_arn: &str,
    entity_arn: &str,
    scope_start: &str,
    scope_end: &str,
) -> (u16, Value) {
    call(
        router,
        "/investigations/startInvestigation",
        &json!({
            "GraphArn": graph_arn,
            "EntityArn": entity_arn,
            "ScopeStartTime": scope_start,
            "ScopeEndTime": scope_end,
        }),
    )
    .await
}

#[tokio::test]
async fn an_investigation_is_read_back_by_the_id_it_was_minted_with() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    // The rows differ along the entity kind, and one carries fractional seconds in its
    // scope window: an ISO8601 `date-time` is what the model specifies for both scope
    // members ("For example, 2021-08-18T16:35:56.284Z").
    let scope_start = "2021-08-18T16:35:56.284Z";
    let scope_end = "2021-08-19T16:35:56Z";
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in entity_cases() {
        let name = case.name;
        let (status, answered) = start_investigation(
            &router,
            &graph_arn,
            &case.entity_arn,
            scope_start,
            scope_end,
        )
        .await;

        let Some(expected_type) = case.entity_type else {
            // An ARN naming neither an IAM user nor an IAM role is a bad request, not
            // a missing resource and not a stored investigation.
            assert_eq!(
                status, 400,
                "case {name:?}: expected 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {name:?}: expected ValidationException, got {answered:?}"
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {name:?}: expected 200, got {status} {answered:?}"
        );
        let investigation_id = answered
            .get("InvestigationId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: no InvestigationId in {answered:?}"))
            .to_string();
        // The modelled `InvestigationId`: length 21..21, pattern `^[0-9]+$`.
        assert_eq!(
            investigation_id.len(),
            INVESTIGATION_ID_LEN,
            "case {name:?}: expected a {INVESTIGATION_ID_LEN}-character id, got {investigation_id:?}"
        );
        assert!(
            investigation_id.bytes().all(|b| b.is_ascii_digit()),
            "case {name:?}: expected an all-digit id, got {investigation_id:?}"
        );
        assert!(
            minted.insert(investigation_id.clone()),
            "case {name:?}: id {investigation_id:?} was minted twice"
        );

        // The round trip: everything the read reports comes from the record the start
        // stored.
        let (status, got) = call(
            &router,
            "/investigations/getInvestigation",
            &json!({ "GraphArn": graph_arn, "InvestigationId": investigation_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetInvestigation expected 200, got {status} {got:?}"
        );
        assert_eq!(
            got.get("GraphArn").and_then(Value::as_str),
            Some(graph_arn.as_str()),
            "case {name:?}: expected the graph it was started on, got {got:?}"
        );
        assert_eq!(
            got.get("InvestigationId").and_then(Value::as_str),
            Some(investigation_id.as_str()),
            "case {name:?}: expected the id it was minted with, got {got:?}"
        );
        assert_eq!(
            got.get("EntityArn").and_then(Value::as_str),
            Some(case.entity_arn.as_str()),
            "case {name:?}: expected the entity it was started for, got {got:?}"
        );
        assert_eq!(
            got.get("EntityType").and_then(Value::as_str),
            Some(expected_type),
            "case {name:?}: expected EntityType {expected_type}, got {got:?}"
        );
        assert_eq!(
            got.get("State").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {name:?}: a new investigation is not archived, got {got:?}"
        );
        // Every timestamp renders as the ISO8601 `date-time` the model specifies, and
        // the scope window is the one the request named (compared to the second, so
        // the assertion does not depend on how a zero fraction is rendered).
        for (member, sent) in [
            ("ScopeStartTime", scope_start),
            ("ScopeEndTime", scope_end),
        ] {
            let rendered = assert_iso8601(&format!("{name}/{member}"), got.get(member));
            assert_eq!(
                &rendered[..19],
                &sent[..19],
                "case {name:?}: expected {member} {sent:?}, got {rendered:?}"
            );
        }
        assert_iso8601(&format!("{name}/CreatedTime"), got.get("CreatedTime"));
    }
}

#[tokio::test]
async fn an_investigation_finishes_once_the_clock_makes_its_run_due() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let (status, started) = start_investigation(
        &router,
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
        "2021-08-18T16:35:56Z",
        "2021-08-19T16:35:56Z",
    )
    .await;
    assert_eq!(status, 200, "StartInvestigation: got {status} {started:?}");
    let investigation_id = started
        .get("InvestigationId")
        .and_then(Value::as_str)
        .expect("an InvestigationId")
        .to_string();
    let read = json!({ "GraphArn": graph_arn, "InvestigationId": investigation_id });

    // A caller reading straight back still sees the non-terminal status the start
    // returned: the span is never zero.
    let (_, running) = call(&router, "/investigations/getInvestigation", &read).await;
    assert_eq!(
        running.get("Status").and_then(Value::as_str),
        Some("RUNNING"),
        "expected the transient RUNNING status straight after the start, got {running:?}"
    );

    tokio::time::sleep(Duration::from_millis(
        (INVESTIGATION_RUN_MILLIS as u64) + 250,
    ))
    .await;

    // Once the clock has passed the due instant the move is materialised, and every
    // later read reports the same terminal record.
    let mut seen = Vec::new();
    for attempt in 0..2 {
        let (status, got) = call(&router, "/investigations/getInvestigation", &read).await;
        assert_eq!(status, 200, "read {attempt}: got {status} {got:?}");
        // Nothing was ingested for the entity over the scope window, so the run had
        // nothing to assemble a report from and did not complete: the status its
        // completion yields is FAILED.
        assert_eq!(
            got.get("Status").and_then(Value::as_str),
            Some("FAILED"),
            "read {attempt}: expected FAILED once the run was due, got {got:?}"
        );
        // And by the same fact the investigation discovered no indicator of compromise,
        // so the severity that computes to is the lowest modelled one.
        assert_eq!(
            got.get("Severity").and_then(Value::as_str),
            Some("INFORMATIONAL"),
            "read {attempt}: got {got:?}"
        );
        seen.push(got);
    }
    assert_eq!(
        seen[0], seen[1],
        "expected a stable record across reads, got {seen:?}"
    );
}

#[tokio::test]
async fn an_investigation_id_no_investigation_carries_is_the_modelled_not_found() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    // One investigation exists, so a miss below is a miss on a populated graph.
    let (status, _) = start_investigation(
        &router,
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
        "2021-08-18T16:35:56Z",
        "2021-08-19T16:35:56Z",
    )
    .await;
    assert_eq!(status, 200, "StartInvestigation");

    // Well formed per the modelled `InvestigationId` (21 characters, all digits, which
    // permits a leading zero) and never minted: the modelled 404.
    let unknown = [
        "1".repeat(INVESTIGATION_ID_LEN),
        "9".repeat(INVESTIGATION_ID_LEN),
        "0".repeat(INVESTIGATION_ID_LEN),
        "123456789012345678901".to_string(),
    ];
    for id in &unknown {
        let (status, answered) = call(
            &router,
            "/investigations/getInvestigation",
            &json!({ "GraphArn": graph_arn, "InvestigationId": id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert_ne!(
            Some(message.as_str()),
            absent_graph_message("GetInvestigation"),
            "case {id:?}: a missing investigation is not a missing graph, got {message:?}"
        );
        assert!(
            !message.contains(id.as_str()),
            "case {id:?}: the message must not echo the requested id, got {message:?}"
        );
    }

    // Outside the modelled constraints: the framework's generic refusal, which is what
    // AWS answers for this member, so it is deliberately NOT reworded.
    let malformed = [
        "1".repeat(INVESTIGATION_ID_LEN - 1),
        "1".repeat(INVESTIGATION_ID_LEN + 1),
        "abcdefghijklmnopqrstu".to_string(),
        "12345678901234567890x".to_string(),
    ];
    for id in &malformed {
        let (status, answered) = call(
            &router,
            "/investigations/getInvestigation",
            &json!({ "GraphArn": graph_arn, "InvestigationId": id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {id:?}: expected ValidationException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert!(
            message.contains("investigationId"),
            "case {id:?}: expected the generic constraint report naming the member, got {message:?}"
        );
    }
}

/// Detective's own message for a `@pattern` failure of an `Accounts` entry member.
fn account_pattern_message(member: &str) -> String {
    format!("Account.{member} did not match the pattern.")
}

#[tokio::test]
async fn create_members_reports_an_account_id_pattern_failure_with_detectives_own_message() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let valid = account_id(0);

    // The rows differ along the offending value and along WHERE in the list it sits:
    // the message is fixed, so neither may reach it.
    let cases: Vec<(&str, Value)> = vec![
        ("only-account", json!([account("1234567890AB")])),
        ("trailing-letter", json!([account("12345678901A")])),
        ("all-letters", json!([account("ABCDEFGHIJKL")])),
        ("punctuation", json!([account("1234-567-890")])),
        (
            "second-of-three",
            json!([
                account(&valid),
                account("1234567890AB"),
                account(&account_id(1))
            ]),
        ),
        (
            "two-bad-accounts",
            json!([account("1234567890AB"), account("ABCDEFGHIJKL")]),
        ),
    ];
    for (case, accounts) in cases {
        let (status, answered) = call(
            &router,
            "/graph/members",
            &json!({ "GraphArn": graph_arn, "Accounts": accounts, "DisableEmailNotification": true }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message,
            account_pattern_message("AccountId"),
            "case {case:?}: expected Detective's fixed per-member message, got {message:?}"
        );
    }

    // The edge of the rule: a value that fails `AccountId`'s LENGTH rather than its
    // pattern is not a pattern failure, so it keeps the framework's generic report —
    // the recorded rule covers the pattern only.
    let (status, answered) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": [account("12345678901")] }),
    )
    .await;
    assert_eq!(status, 400, "short account id: got {status} {answered:?}");
    let message = error_message(&answered);
    assert_ne!(
        message,
        account_pattern_message("AccountId"),
        "a length failure is not a pattern failure, got {message:?}"
    );
    assert!(
        message.contains("accounts.1.member.accountId"),
        "expected the generic constraint report naming the member, got {message:?}"
    );

    // And the control: a well-formed request still stores its members.
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": [account(&valid)] }),
    )
    .await;
    assert_eq!(status, 200, "valid request: got {status} {created:?}");
    let (status, listed) = call(
        &router,
        "/graph/members/list",
        &json!({ "GraphArn": graph_arn }),
    )
    .await;
    assert_eq!(status, 200, "ListMembers: got {status} {listed:?}");
    let stored: Vec<&str> = listed
        .get("MemberDetails")
        .and_then(Value::as_array)
        .expect("MemberDetails")
        .iter()
        .filter_map(|member| member.get("AccountId").and_then(Value::as_str))
        .collect();
    assert_eq!(
        stored,
        vec![ACCOUNT, valid.as_str()],
        "expected only the account the accepted request named, beside the administrator's \
         own membership, got {stored:?}"
    );
}

#[tokio::test]
async fn update_datasource_packages_validates_the_requested_package_set() {
    let (router, engine) = emulator_with_engine();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // Unknown values: the rows differ along how many there are and where they sit
    // among valid ones, because the message lists every one of them in request order.
    let unknown_cases: Vec<(&str, Value, &str)> = vec![
        (
            "one-unknown",
            json!(["NOT_A_REAL_DATASOURCE_PACKAGE"]),
            "NOT_A_REAL_DATASOURCE_PACKAGE",
        ),
        (
            "unknown-beside-valid",
            json!(["DETECTIVE_CORE", "NOT_A_PACKAGE"]),
            "NOT_A_PACKAGE",
        ),
        (
            "two-unknown",
            json!(["FIRST_UNKNOWN", "DETECTIVE_CORE", "SECOND_UNKNOWN"]),
            "FIRST_UNKNOWN, SECOND_UNKNOWN",
        ),
    ];
    for (case, packages, listed) in unknown_cases {
        let (status, answered) = call(
            &router,
            "/graph/datasources/update",
            &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message,
            format!(
                "UpdateDatasourcePackages request includes unknown datasource package(s): [{listed}]"
            ),
            "case {case:?}: expected the unknown-package message, got {message:?}"
        );
    }

    // The companion set rule: every value is a modelled one, but the request leaves
    // out the package the graph must ingest.
    for (case, packages) in [
        ("only-eks", json!(["EKS_AUDIT"])),
        (
            "both-optional",
            json!(["EKS_AUDIT", "ASFF_SECURITYHUB_FINDING"]),
        ),
    ] {
        let (status, answered) = call(
            &router,
            "/graph/datasources/update",
            &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message,
            format!(
                "UpdateDatasourcePackages request missing mandatory datasource package(s): \
                 [{MANDATORY_DATASOURCE_PACKAGE}]"
            ),
            "case {case:?}: expected the missing-package message, got {message:?}"
        );
    }

    // An accepted set is stored: the packages it names are started, and one the graph
    // was ingesting that a later request leaves out is stopped rather than forgotten.
    let (status, answered) = call(
        &router,
        "/graph/datasources/update",
        &json!({ "GraphArn": graph_arn, "DatasourcePackages": ["DETECTIVE_CORE", "EKS_AUDIT"] }),
    )
    .await;
    assert_eq!(status, 200, "accepted set: got {status} {answered:?}");
    assert_eq!(
        stored_package_states(&engine, &graph_arn),
        vec![
            (
                "ASFF_SECURITYHUB_FINDING".to_string(),
                Some(DATASOURCE_PACKAGE_STATE_STOPPED.to_string())
            ),
            (
                "DETECTIVE_CORE".to_string(),
                Some(DATASOURCE_PACKAGE_STATE_STARTED.to_string())
            ),
            (
                "EKS_AUDIT".to_string(),
                Some(DATASOURCE_PACKAGE_STATE_STARTED.to_string())
            ),
        ],
        "expected both named packages started and the one left out stopped"
    );

    let (status, answered) = call(
        &router,
        "/graph/datasources/update",
        &json!({ "GraphArn": graph_arn, "DatasourcePackages": ["DETECTIVE_CORE"] }),
    )
    .await;
    assert_eq!(status, 200, "narrowed set: got {status} {answered:?}");
    assert_eq!(
        stored_package_states(&engine, &graph_arn),
        vec![
            (
                "ASFF_SECURITYHUB_FINDING".to_string(),
                Some(DATASOURCE_PACKAGE_STATE_STOPPED.to_string())
            ),
            (
                "DETECTIVE_CORE".to_string(),
                Some(DATASOURCE_PACKAGE_STATE_STARTED.to_string())
            ),
            (
                "EKS_AUDIT".to_string(),
                Some(DATASOURCE_PACKAGE_STATE_STOPPED.to_string())
            ),
        ],
        "expected the package the request left out to be stopped"
    );
}

/// The ingest state stored for each modelled `DatasourcePackage` on `graph_arn`, read
/// out of the shared store so this suite asserts what was PERSISTED rather than what a
/// reporting operation chose to render (`datasource_package_ingest` and
/// `datasource_package_reporting` assert the wire view through `ListDatasourcePackages`).
/// `None` means the graph holds no record for that package at all.
fn stored_package_states(
    engine: &engine::InMemoryEngine,
    graph_arn: &str,
) -> Vec<(String, Option<String>)> {
    let state = service_state(engine).expect("this service's store");
    let store = state.lock();
    let graph = store.graph(graph_arn).expect("the created graph");
    [
        "ASFF_SECURITYHUB_FINDING",
        "DETECTIVE_CORE",
        "EKS_AUDIT",
    ]
    .into_iter()
    .map(|package| {
        (
            package.to_string(),
            graph
                .datasource_packages
                .get(package)
                .map(|ingest| ingest.state.clone()),
        )
    })
    .collect()
}

/// Every tag of `graph_arn` in this suite's Region, as `ListTagsForResource` reports
/// them: the shared harness's one reader, bound to the Region this suite signs for.
async fn tags_of(router: &MultiServiceRouter, graph_arn: &str) -> Map<String, Value> {
    crate::support::tags_of(router, REGION, graph_arn).await
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_never_becomes_visible() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // The rows differ along the CASING of the reserved prefix and along what follows
    // it, because the rule is the prefix compared case-insensitively.
    let reserved = [
        "aws:sim2real-reserved",
        "AWS:shouty",
        "Aws:Mixed",
        "aWs:alternating",
        "aws:",
    ];
    for key in reserved {
        // In its own request, and beside an ordinary key: the refusal is raised before
        // ANY tag of the request is applied.
        for (case, tags) in [
            (format!("{key} alone"), json!({ key: "v" })),
            (
                format!("{key} beside an ordinary key"),
                json!({ key: "v", "Ordinary": "w" }),
            ),
        ] {
            let (status, answered) =
                call(&router, &format!("/tags/{graph_arn}"), &json!({ "Tags": tags })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {answered:?}"
            );
            assert_eq!(
                error_message(&answered),
                RESERVED_TAG_KEY_MESSAGE,
                "case {case:?}: expected the reserved-key message, got {answered:?}"
            );
            let visible = tags_of(&router, &graph_arn).await;
            assert!(
                visible.is_empty(),
                "case {case:?}: a refused request must apply no tag at all, got {visible:?}"
            );
        }
    }

    // The same rule on the other operation that takes a tag map: a graph is not
    // created with a reserved key either.
    let (status, answered) = call(&router, "/graph", &json!({ "Tags": { "aws:reserved": "v" } })).await;
    assert_eq!(
        status, 400,
        "CreateGraph with a reserved key: expected 400, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "BadRequestException",
        "CreateGraph with a reserved key: got {answered:?}"
    );

    // An ordinary key on the same resource still tags successfully, and the tags read
    // back are exactly the ones that were applied. The rows differ along the shape of
    // the key, within the modelled `TagKey` pattern `[a-zA-Z+-=._:/]+`.
    let ordinary = [
        ("Env", "test"),
        ("team/name", "detectives"),
        ("a+b-c=d._:/", ""),
        ("not-aws:reserved", "v"),
    ];
    let mut expected = Map::new();
    for (key, value) in ordinary {
        let (status, answered) = call(
            &router,
            &format!("/tags/{graph_arn}"),
            &json!({ "Tags": { key: value } }),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {key:?}: expected the modelled 204, got {status} {answered:?}"
        );
        expected.insert(key.to_string(), json!(value));
        let visible = tags_of(&router, &graph_arn).await;
        assert_eq!(
            visible, expected,
            "case {key:?}: expected the tags that were applied, got {visible:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_store_of_one_graph_is_bounded() {
    let router = emulator();
    // `CreateGraph` with the per-resource tag quota's worth of keys — the same number
    // the modelled `TagMap` length allows in one request — so this graph starts full.
    let mut full = Map::new();
    for n in 0..MAX_TAGS_PER_GRAPH {
        full.insert(format!("Key{n}"), json!("v"));
    }
    let graph_arn =
        create_graph_ok(&router, REGION, &json!({ "Tags": Value::Object(full) })).await;

    // Replacing the value of a key the graph already holds grows the store by nothing,
    // so it is accepted even at the bound.
    let (status, answered) = call(
        &router,
        &format!("/tags/{graph_arn}"),
        &json!({ "Tags": { "Key0": "w" } }),
    )
    .await;
    assert_eq!(
        status, 204,
        "replacing a stored key at the bound: expected 204, got {status} {answered:?}"
    );

    // One key the graph does not hold would take it past the bound: refused with the
    // operation's modelled error, and nothing of the request is stored.
    let (status, answered) = call(
        &router,
        &format!("/tags/{graph_arn}"),
        &json!({ "Tags": { "KeyBeyond": "w", "Key1": "x" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a new key past the bound: expected 400, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "ValidationException",
        "a new key past the bound: got {answered:?}"
    );
    let visible = tags_of(&router, &graph_arn).await;
    assert_eq!(
        visible.len(),
        MAX_TAGS_PER_GRAPH,
        "expected the store to stay at its bound, got {visible:?}"
    );
    assert!(
        !visible.contains_key("KeyBeyond"),
        "a refused request must store none of its keys, got {visible:?}"
    );
    assert_eq!(
        visible.get("Key1"),
        Some(&json!("v")),
        "a refused request must not replace a stored value either, got {visible:?}"
    );
    assert_eq!(
        visible.get("Key0"),
        Some(&json!("w")),
        "expected the accepted replacement to have been stored, got {visible:?}"
    );
}

#[tokio::test]
async fn start_monitoring_member_tells_a_non_member_from_a_member_in_another_status() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(0);
    let (status, _) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": [account(&member)] }),
    )
    .await;
    assert_eq!(status, 200, "CreateMembers");

    // Not a member of this graph: the modelled 404 with the fixed missing-membership
    // message — the graph WAS found, and the refusal is about the membership. The rows
    // differ along the account id.
    for account in [account_id(1), account_id(2), "999999999999".to_string()] {
        let (status, answered) = call(
            &router,
            "/graph/member/monitoringstate",
            &json!({ "GraphArn": graph_arn, "AccountId": account }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {account:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {account:?}: expected ResourceNotFoundException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message, NO_SUCH_MEMBERSHIP_MESSAGE,
            "case {account:?}: expected the fixed missing-membership message, got {message:?}"
        );
        assert!(
            !message.contains(account.as_str()),
            "case {account:?}: the message must not echo the account id, got {message:?}"
        );
    }

    // A record the graph DOES hold, but one the account never accepted: there is no
    // ingest to enable, so this is the same modelled 404 under the text that names that
    // fact — and the stored record is left exactly as it was.
    let (status, answered) = call(
        &router,
        "/graph/member/monitoringstate",
        &json!({ "GraphArn": graph_arn, "AccountId": member }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a membership the account never accepted: expected 404, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "a membership the account never accepted: got {answered:?}"
    );
    assert_eq!(
        error_message(&answered),
        INVITATION_NOT_ACCEPTED_MESSAGE,
        "a membership the account never accepted: got {answered:?}"
    );

    let (status, got) = call(
        &router,
        "/graph/members/get",
        &json!({ "GraphArn": graph_arn, "AccountIds": [member] }),
    )
    .await;
    assert_eq!(status, 200, "GetMembers: got {status} {got:?}");
    let status_after = got
        .get("MemberDetails")
        .and_then(Value::as_array)
        .and_then(|details| details.first())
        .and_then(|detail| detail.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetMembers reported no member, got {got:?}"));
    assert_ne!(
        status_after, "ENABLED",
        "a refused StartMonitoringMember must not enable the membership, got {got:?}"
    );
}

#[tokio::test]
async fn reject_invitation_reports_an_invitation_the_caller_does_not_hold() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    // The graph exists and even holds a member, but none of its invitations is
    // addressed to the calling account — "this operation must be called by an invited
    // member account" — so there is no invitation for this caller to reject.
    let (status, _) = call(
        &router,
        "/graph/members",
        &json!({ "GraphArn": graph_arn, "Accounts": [account(&account_id(0))] }),
    )
    .await;
    assert_eq!(status, 200, "CreateMembers");

    let (status, answered) = call(
        &router,
        "/invitation/removal",
        &json!({ "GraphArn": graph_arn }),
    )
    .await;
    assert_eq!(
        status, 404,
        "no invitation for the caller: expected 404, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "no invitation for the caller: got {answered:?}"
    );
    let message = error_message(&answered);
    assert_eq!(
        message, NO_SUCH_MEMBERSHIP_MESSAGE,
        "an invitation the caller does not hold is the fixed missing-membership \
         message, got {message:?}"
    );

    // The refused reject changed nothing: the graph and its member are still there.
    let (status, listed) = call(
        &router,
        "/graph/members/list",
        &json!({ "GraphArn": graph_arn }),
    )
    .await;
    assert_eq!(status, 200, "ListMembers after the refusal: got {listed:?}");
    let stored = listed
        .get("MemberDetails")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        stored, 2,
        "expected the graph's invited membership and the administrator's own to survive, \
         got {listed:?}"
    );
}

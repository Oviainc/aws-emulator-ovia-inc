// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TEXT this service refuses with, and for the one
//! request class it refuses outright.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one input a
//! recording happened to carry:
//!
//! 1. `ListDatasourcePackages` is not a paginated report: it answers every package the
//!    behavior graph holds in ONE response and issues no `NextToken`, so a request
//!    carrying ANY `MaxResults` is refused with the modelled `ValidationException` (400)
//!    and that class's fixed sentence, which interpolates nothing from the request.
//! 2. `UpdateDatasourcePackages`' missing-mandatory refusal is fixed text whose only
//!    variable part is the comma-separated list of mandatory packages the request left
//!    out.
//! 3. One condition, one text: every refusal of a MEMBERSHIP this service holds no
//!    record of carries the same fixed sentence — whether it is answered as the modelled
//!    `ResourceNotFoundException` (404) or as an `UnprocessedAccount$Reason` inside a
//!    200 — and it discloses neither the account nor whether the graph exists. A record
//!    the graph DOES hold but the account never accepted is the same 404 under the
//!    sentence that names that fact.
//! 4. A refusal's text belongs to the surface that publishes it: the operations bound
//!    under this service's `/investigations/` URIs word a graph they cannot resolve with
//!    their own fixed sentence, which names the input to inspect.
//! 5. A `@pattern` refusal reports the WHOLE-VALUE form of the modelled pattern: one
//!    anchored at the start but not terminated at the end is reported with the trailing
//!    `.*` that completes it, and one that already terminates is reported as modelled.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    ACCOUNT_ID_PATTERN, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_ID_LEN,
    INVITATION_NOT_ACCEPTED_MESSAGE, MANDATORY_DATASOURCE_PACKAGE, MAX_PAGE_SIZE,
    MAX_RESULTS_MUST_BE_NULL_MESSAGE, NO_SUCH_MEMBERSHIP_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, call, create_graph_ok, emulator, error_code, error_message, graph_arn_of,
    ACCOUNT, REGION,
};

/// The URIs this suite calls, from
/// `smithy_query(query="digest")` on this service's model.
const LIST_URI: &str = "/graph/datasources/list";
const UPDATE_URI: &str = "/graph/datasources/update";
const MEMBERS_URI: &str = "/graph/members";
const GET_MEMBERS_URI: &str = "/graph/members/get";
const DELETE_MEMBERS_URI: &str = "/graph/members/removal";
const MONITORING_STATE_URI: &str = "/graph/member/monitoringstate";
const REJECT_INVITATION_URI: &str = "/invitation/removal";
const START_INVESTIGATION_URI: &str = "/investigations/startInvestigation";
const GET_INVESTIGATION_URI: &str = "/investigations/getInvestigation";

/// `StartInvestigation$EntityArn`'s modelled pattern,
/// `smithy_query(query="shape",shape="EntityArn")`: anchored at the start and NOT
/// terminated, which is what rule 5 completes.
const ENTITY_ARN_PATTERN: &str = "^arn:";

/// `GraphArn`'s modelled pattern, `smithy_query(query="shape",shape="GraphArn")`:
/// likewise already terminated.
const GRAPH_ARN_PATTERN: &str =
    r"^arn:aws[-\w]{0,10}?:detective:[-\w]{2,20}?:\d{12}?:graph:[abcdef\d]{32}?$";

/// A well-formed `EntityArn` naming an IAM user, for a row that is testing something
/// other than the entity.
fn entity_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/alice")
}

/// A `StartInvestigation` body for `graph_arn` with a well-formed scope window.
fn start_investigation_body(graph_arn: &str) -> Value {
    json!({
        "GraphArn": graph_arn,
        "EntityArn": entity_arn(),
        "ScopeStartTime": "2021-08-18T16:35:56Z",
        "ScopeEndTime": "2021-08-19T16:35:56Z",
    })
}

/// A `GetInvestigation` body for `graph_arn`, naming a well-formed `InvestigationId`
/// (modelled length 21..21, pattern `^[0-9]+$`).
fn get_investigation_body(graph_arn: &str) -> Value {
    json!({
        "GraphArn": graph_arn,
        "InvestigationId": "1".repeat(INVESTIGATION_ID_LEN),
    })
}

/// `UpdateDatasourcePackages` naming `packages`, which must be accepted.
async fn update_ok(router: &MultiServiceRouter, case: &str, graph_arn: &str, packages: &[&str]) {
    let (status, answered) = call(
        router,
        UPDATE_URI,
        &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateDatasourcePackages {packages:?} expected 200, got {status} \
         {answered:?}"
    );
}

/// `account id -> Reason`, the `UnprocessedAccounts` an answer reported.
fn unprocessed_reasons(case: &str, answered: &Value) -> BTreeMap<String, String> {
    answered
        .get("UnprocessedAccounts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected UnprocessedAccounts, got {answered:?}")
        })
        .iter()
        .map(|entry| {
            let account = entry
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an unprocessed entry carried no AccountId: {entry:?}")
                });
            let reason = entry
                .get("Reason")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an unprocessed entry carried no Reason: {entry:?}")
                });
            (account.to_string(), reason.to_string())
        })
        .collect()
}

/// The `MemberDetails` account ids an answer reported.
fn member_ids(case: &str, answered: &Value) -> Vec<String> {
    answered
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected MemberDetails, got {answered:?}"))
        .iter()
        .map(|detail| {
            detail
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a member detail carried no AccountId: {detail:?}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn any_max_results_is_refused_with_the_fixed_text_for_that_class() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let held = ["ASFF_SECURITYHUB_FINDING", MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"];
    update_ok(&router, "seed", &graph_arn, &held).await;

    // The rows differ along the size asked for: each published bound of the modelled
    // `MemberResultsLimit` range (1..200), a size smaller than what the graph holds, one
    // that exactly covers it and one larger. Every row is the same refusal, and none of
    // them interpolates the value it refused.
    for max_results in [1, 2, 3, 4, MAX_PAGE_SIZE as i32] {
        let case = format!("max-results-{max_results}");
        let (status, answered) = call(
            &router,
            LIST_URI,
            &json!({ "GraphArn": graph_arn, "MaxResults": max_results }),
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
            message, MAX_RESULTS_MUST_BE_NULL_MESSAGE,
            "case {case:?}: expected the fixed max-results message, got {message:?}"
        );
        assert!(
            !message.contains(&max_results.to_string()),
            "case {case:?}: the refusal interpolates nothing from the request, got {message:?}"
        );
        assert_eq!(
            answered.get("NextToken"),
            None,
            "case {case:?}: a refusal carries no NextToken, got {answered:?}"
        );
    }

    // The control: the same call without `MaxResults` answers every held package at once
    // and hands out no continuation token.
    let (status, answered) = call(&router, LIST_URI, &json!({ "GraphArn": graph_arn })).await;
    assert_eq!(status, 200, "no MaxResults: got {status} {answered:?}");
    assert_eq!(
        answered.get("NextToken"),
        None,
        "an unpaginated report issues no NextToken, got {answered:?}"
    );
    let reported = answered
        .get("DatasourcePackages")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("expected DatasourcePackages, got {answered:?}"));
    for package in held {
        assert!(
            reported.contains_key(package),
            "one response must carry {package}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_missing_mandatory_refusal_names_only_what_the_request_left_out() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // The rows differ along WHICH modelled packages the request names and HOW MANY —
    // every non-mandatory subset, in either order — and along a repeated value. The
    // expected text is the same in all of them, because the only variable part is the
    // list of MANDATORY packages missing, which is the same one package each time.
    let expected = format!(
        "UpdateDatasourcePackages request missing mandatory datasource package(s): \
         [{MANDATORY_DATASOURCE_PACKAGE}]"
    );
    let rows: [(&str, Value); 4] = [
        ("one-optional", json!(["EKS_AUDIT"])),
        ("the-other-optional", json!(["ASFF_SECURITYHUB_FINDING"])),
        (
            "both-optional",
            json!(["EKS_AUDIT", "ASFF_SECURITYHUB_FINDING"]),
        ),
        (
            "both-optional-reversed-and-repeated",
            json!(["ASFF_SECURITYHUB_FINDING", "EKS_AUDIT", "EKS_AUDIT"]),
        ),
    ];
    for (case, packages) in rows {
        let (status, answered) = call(
            &router,
            UPDATE_URI,
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
            message, expected,
            "case {case:?}: expected the fixed missing-mandatory text, got {message:?}"
        );
    }

    // The control: a set that names the mandatory package is accepted, so the refusal is
    // about the mandatory package and not about naming optional ones.
    update_ok(
        &router,
        "names-the-mandatory-package",
        &graph_arn,
        &[MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"],
    )
    .await;
}

#[tokio::test]
async fn every_refusal_of_a_membership_this_service_does_not_hold_carries_one_text() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(0);
    let (status, _) = call(
        &router,
        MEMBERS_URI,
        &json!({ "GraphArn": graph_arn, "Accounts": [account(&member)] }),
    )
    .await;
    assert_eq!(status, 200, "CreateMembers");

    // The 404 half of the rule. The rows differ along the OPERATION and along how the
    // membership comes to be absent: accounts that were never invited into a graph that
    // exists, and the calling account's own invitation in a graph that exists and in one
    // the service does not hold at all — an invitee-side operation resolves the caller's
    // membership, so it discloses nothing about the graph and the two rows read alike.
    let absent_graph = graph_arn_of("aws", REGION, ACCOUNT, &"c".repeat(32));
    let rows: [(&str, &str, Value); 4] = [
        (
            "start-monitoring-a-stranger",
            MONITORING_STATE_URI,
            json!({ "GraphArn": graph_arn, "AccountId": account_id(1) }),
        ),
        (
            "start-monitoring-another-stranger",
            MONITORING_STATE_URI,
            json!({ "GraphArn": graph_arn, "AccountId": "999999999999" }),
        ),
        (
            "reject-an-invitation-the-caller-does-not-hold",
            REJECT_INVITATION_URI,
            json!({ "GraphArn": graph_arn }),
        ),
        (
            "reject-an-invitation-in-a-graph-the-service-does-not-hold",
            REJECT_INVITATION_URI,
            json!({ "GraphArn": absent_graph }),
        ),
    ];
    for (case, uri, body) in rows {
        let (status, answered) = call(&router, uri, &body).await;
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
        assert_eq!(
            message, NO_SUCH_MEMBERSHIP_MESSAGE,
            "case {case:?}: expected the fixed missing-membership text, got {message:?}"
        );
    }

    // A record the graph DOES hold, but one the account never accepted: the same
    // modelled 404, under the sentence that names that fact rather than the one for an
    // account the graph holds no record of at all.
    let (status, answered) = call(
        &router,
        MONITORING_STATE_URI,
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
    let message = error_message(&answered);
    assert_eq!(
        message, INVITATION_NOT_ACCEPTED_MESSAGE,
        "a membership the account never accepted: expected its own fixed text, got \
         {message:?}"
    );

    // The partial-success half of the same rule: a batch reports the SAME text as the
    // `Reason` of an account the graph holds no membership for. The rows differ along how
    // many strangers the batch names and whether a member is named beside them.
    let strangers = [account_id(1), account_id(2), "999999999999".to_string()];
    for (case, named) in [
        ("one-stranger", vec![strangers[0].clone()]),
        (
            "strangers-only",
            vec![strangers[1].clone(), strangers[2].clone()],
        ),
        (
            "a-member-and-a-stranger",
            vec![member.clone(), strangers[0].clone()],
        ),
    ] {
        for uri in [GET_MEMBERS_URI, DELETE_MEMBERS_URI] {
            // `DeleteMembers` is the mutating one, so it runs on a graph of its own: the
            // reason it reports must not depend on what the reader did first.
            let router = emulator();
            let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
            let (status, _) = call(
                &router,
                MEMBERS_URI,
                &json!({ "GraphArn": graph_arn, "Accounts": [account(&member)] }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: CreateMembers");

            let case = format!("{case} through {uri}");
            let (status, answered) = call(
                &router,
                uri,
                &json!({ "GraphArn": graph_arn, "AccountIds": named }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a batch reports per-account outcomes at 200, got {status} \
                 {answered:?}"
            );
            let reasons = unprocessed_reasons(&case, &answered);
            for stranger in named.iter().filter(|id| **id != member) {
                assert_eq!(
                    reasons.get(stranger).map(String::as_str),
                    Some(NO_SUCH_MEMBERSHIP_MESSAGE),
                    "case {case:?}: expected the fixed missing-membership text for \
                     {stranger:?}, got {reasons:?}"
                );
            }
            for (stranger, reason) in &reasons {
                assert!(
                    !reason.contains(stranger.as_str()),
                    "case {case:?}: the reason carries the account in AccountId, not in its \
                     text, got {reason:?}"
                );
            }
        }
    }

    // And the removal the recording walks: once `DeleteMembers` has removed a membership,
    // the reader reports that account with the very same text, under an empty
    // `MemberDetails`.
    let (status, answered) = call(
        &router,
        DELETE_MEMBERS_URI,
        &json!({ "GraphArn": graph_arn, "AccountIds": [member] }),
    )
    .await;
    assert_eq!(status, 200, "DeleteMembers: got {status} {answered:?}");
    let (status, answered) = call(
        &router,
        GET_MEMBERS_URI,
        &json!({ "GraphArn": graph_arn, "AccountIds": [member] }),
    )
    .await;
    assert_eq!(status, 200, "GetMembers: got {status} {answered:?}");
    assert_eq!(
        member_ids("a removed member", &answered),
        Vec::<String>::new(),
        "a removed membership is reported by nobody, got {answered:?}"
    );
    assert_eq!(
        unprocessed_reasons("a removed member", &answered)
            .get(&member)
            .map(String::as_str),
        Some(NO_SUCH_MEMBERSHIP_MESSAGE),
        "expected the fixed missing-membership text, got {answered:?}"
    );
}

#[tokio::test]
async fn the_investigation_surface_words_an_unresolvable_graph_its_own_way() {
    let router = emulator();

    // A graph that existed and was deleted, beside ARNs nothing was ever stored under:
    // all of them are "this service does not hold this graph", so all of them must read
    // identically.
    let deleted = create_graph_ok(&router, REGION, &json!({})).await;
    let (status, _) = call(&router, "/graph/removal", &json!({ "GraphArn": deleted })).await;
    assert_eq!(status, 200, "DeleteGraph");
    let absent = [
        ("never-created", graph_arn_of("aws", REGION, ACCOUNT, &"a".repeat(32))),
        (
            "never-created-in-another-region",
            graph_arn_of("aws", "eu-west-1", ACCOUNT, &"0123456789abcdef".repeat(2)),
        ),
        ("deleted", deleted),
    ];

    for (how, arn) in &absent {
        for (operation, uri, body) in [
            (
                "StartInvestigation",
                START_INVESTIGATION_URI,
                start_investigation_body(arn),
            ),
            (
                "GetInvestigation",
                GET_INVESTIGATION_URI,
                get_investigation_body(arn),
            ),
        ] {
            let case = format!("{operation} on a {how} graph");
            let (status, answered) = call(&router, uri, &body).await;
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
            assert_eq!(
                message, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the investigation surface's fixed text, got \
                 {message:?}"
            );
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?}: the refusal must not echo the requested ARN, got {message:?}"
            );
        }
    }

    // The control: on a graph the service DOES hold, neither operation answers that text
    // — the one above was about the graph, these are not.
    let graph_arn = create_graph_ok(&router, "ap-south-1", &json!({})).await;
    let (status, answered) = call_started(&router, &graph_arn).await;
    assert_eq!(
        status, 200,
        "StartInvestigation on a stored graph: got {status} {answered:?}"
    );
    let (status, answered) = call(&router, GET_INVESTIGATION_URI, &get_investigation_body(&graph_arn)).await;
    assert_eq!(
        status, 404,
        "an investigation the graph does not hold: got {status} {answered:?}"
    );
    let message = error_message(&answered);
    assert_ne!(
        message, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
        "a missing investigation is not a missing graph, got {message:?}"
    );
}

/// `StartInvestigation` on `graph_arn` with a well-formed window, whatever it answers.
async fn call_started(router: &MultiServiceRouter, graph_arn: &str) -> (u16, Value) {
    call(router, START_INVESTIGATION_URI, &start_investigation_body(graph_arn)).await
}

/// One row of the pattern table: the request, the member path the walk names, the value
/// it echoes, and the regular expression the refusal must report.
struct PatternCase {
    name: &'static str,
    uri: &'static str,
    body: Value,
    value: String,
    path: &'static str,
    reported: String,
}

#[tokio::test]
async fn a_pattern_refusal_reports_the_whole_value_form_of_the_pattern() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // The rows differ along which member fails and along which FORM its modelled pattern
    // has: `EntityArn`'s is anchored at the start only, so the whole-value form adds the
    // trailing `.*`; `AccountId`'s and `GraphArn`'s already terminate, so they are
    // reported exactly as modelled. The rejected values differ in length, in shape and in
    // whether they are an ARN at all.
    let malformed_entities = ["not-an-arn-at-all", "x", "ARN:aws:iam::1:user/u", ""];
    let mut cases: Vec<PatternCase> = malformed_entities
        .iter()
        .map(|value| PatternCase {
            name: "StartInvestigation.EntityArn",
            uri: START_INVESTIGATION_URI,
            body: json!({
                "GraphArn": graph_arn,
                "EntityArn": value,
                "ScopeStartTime": "2021-08-18T16:35:56Z",
                "ScopeEndTime": "2021-08-19T16:35:56Z",
            }),
            value: (*value).to_string(),
            path: "entityArn",
            reported: format!("{ENTITY_ARN_PATTERN}.*"),
        })
        .collect();
    cases.push(PatternCase {
        name: "StartMonitoringMember.AccountId",
        uri: MONITORING_STATE_URI,
        body: json!({ "GraphArn": graph_arn, "AccountId": "12345678901a" }),
        value: "12345678901a".to_string(),
        path: "accountId",
        reported: ACCOUNT_ID_PATTERN.to_string(),
    });
    // The ELEMENTS of a list member are deliberately absent from this table: that one
    // class is not stated in this envelope at all, but with the service's own clause
    // naming the input shape, which `client_error_bodies` asserts.
    let not_hex = format!("arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}", "z".repeat(32));
    cases.push(PatternCase {
        name: "GetMembers.GraphArn",
        uri: GET_MEMBERS_URI,
        body: json!({ "GraphArn": not_hex, "AccountIds": [account_id(0)] }),
        value: not_hex.clone(),
        path: "graphArn",
        reported: GRAPH_ARN_PATTERN.to_string(),
    });

    for case in cases {
        let name = format!("{} rejecting {:?}", case.name, case.value);
        let (status, answered) = call(&router, case.uri, &case.body).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {answered:?}"
        );
        let message = error_message(&answered);
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            case.value, case.path, case.reported
        );
        assert_eq!(
            message, expected,
            "case {name:?}: expected the reported pattern {:?}, got {message:?}",
            case.reported
        );
    }
}

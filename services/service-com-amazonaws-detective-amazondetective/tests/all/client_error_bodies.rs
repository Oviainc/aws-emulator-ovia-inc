// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the BODY this service answers a client error with, and for
//! the partial-success body of a member removal.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one input a
//! recording happened to carry:
//!
//! 1. Every client-error body of this service carries its explanatory text under the wire
//!    key `message` — whatever the error shape, the status and the operation, and whether
//!    the refusal came from a handler, from the boundary walk or from an operation that is
//!    still a stub — and never under the capitalised spelling the model happens to give
//!    the member. A client reads that one key as the exception's user-facing text. The one
//!    refusal that states no text at all (the DELETE of a graph that is already gone)
//!    carries no message member under either spelling.
//! 2. A constraint violation of the ELEMENTS of a list member is stated with one clause
//!    naming the shape that declares the list and the list member, then the constraint
//!    class: `<InputShape>.<Member> members did not match the pattern.` It echoes neither
//!    the offending value nor the position it was sent in, so it is the same sentence
//!    however many entries violate and wherever in the batch they sit — and the request
//!    that earned it changed nothing.
//! 3. Every modelled list of a response is SERVED, empty when the call produced no entry
//!    for it: a removal that removed nothing answers an empty `AccountIds` rather than
//!    omitting it, so a caller can iterate it without a null guard.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    service_state, ACCOUNT_ID_LEN, GRAPH_NOT_FOUND_MESSAGE, INVITATION_NOT_ACCEPTED_MESSAGE,
    MAX_ACCOUNT_IDS, MAX_RESULTS_MUST_BE_NULL_MESSAGE, MEMBER_STATUS_ACCEPTED_BUT_DISABLED,
    MEMBER_STATUS_ENABLED, NO_SUCH_MEMBERSHIP_MESSAGE, NO_SUCH_MEMBERSHIP_MESSAGE as REMOVAL_REASON,
    RESERVED_TAG_KEY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    account, account_id, call, call_with, create_graph_ok, emulator, emulator_with_engine,
    error_code, graph_arn_of, ACCOUNT, REGION,
};

/// The URIs this suite calls, from `smithy_query(query="digest")` on this service's model.
const CREATE_MEMBERS_URI: &str = "/graph/members";
const GET_MEMBERS_URI: &str = "/graph/members/get";
const DELETE_MEMBERS_URI: &str = "/graph/members/removal";
const LIST_MEMBERS_URI: &str = "/graph/members/list";
const MONITORING_STATE_URI: &str = "/graph/member/monitoringstate";
const REJECT_INVITATION_URI: &str = "/invitation/removal";
const ACCEPT_INVITATION_URI: &str = "/invitation";
const LIST_DATASOURCES_URI: &str = "/graph/datasources/list";
const TAG_RESOURCE_URI: &str = "/tags";

/// The capitalised spelling the model gives the message member
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` reports
/// `{"Message": …}`) and which no body of this service serves.
const MODEL_MESSAGE_MEMBER: &str = "Message";

/// A `GraphArn` that satisfies the modelled pattern and that nothing was ever stored
/// under.
fn never_created_arn() -> String {
    graph_arn_of("aws", REGION, ACCOUNT, &"c".repeat(32))
}

/// `CreateMembers` inviting `account_ids` into `graph_arn`, which must be accepted.
async fn invite(router: &MultiServiceRouter, graph_arn: &str, account_ids: &[String]) {
    let accounts: Vec<Value> = account_ids.iter().map(|id| account(id)).collect();
    let (status, answered) = call(
        router,
        CREATE_MEMBERS_URI,
        &json!({ "GraphArn": graph_arn, "Accounts": accounts }),
    )
    .await;
    assert_eq!(
        status, 200,
        "inviting {account_ids:?}: expected 200, got {status} {answered:?}"
    );
}

/// The accounts `ListMembers` reports for `graph_arn`.
async fn listed_members(router: &MultiServiceRouter, graph_arn: &str) -> BTreeSet<String> {
    let (status, answered) = call(router, LIST_MEMBERS_URI, &json!({ "GraphArn": graph_arn })).await;
    assert_eq!(
        status, 200,
        "ListMembers({graph_arn}): expected 200, got {status} {answered:?}"
    );
    answered
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListMembers({graph_arn}) carried no MemberDetails: {answered:?}"))
        .iter()
        .filter_map(|member| member.get("AccountId").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The `message` one error body carries, read under THAT key alone — the rule under test
/// — and asserted to be the only spelling served.
fn served_message(case: &str, body: &Value) -> String {
    assert!(
        body.get(MODEL_MESSAGE_MEMBER).is_none(),
        "case {case:?}: the body must not serve the capitalised {MODEL_MESSAGE_MEMBER:?} \
         key, got {body:?}"
    );
    body.get("message")
        .unwrap_or_else(|| panic!("case {case:?}: the error body carried no message: {body:?}"))
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: message is not a string: {body:?}"))
        .to_string()
}

/// One row of the error-body table: the request, and what the answer must state.
struct ErrorCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    status: u16,
    code: &'static str,
    /// The exact text, where this service publishes a fixed one for the condition; `None`
    /// for a refusal whose wording another suite owns, where only the KEY is under test.
    message: Option<String>,
}

#[tokio::test]
async fn every_client_error_body_keys_its_message_lowercase() {
    let (router, engine) = emulator_with_engine();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let absent = never_created_arn();
    // One invited member, whose verification has not settled and which no account has
    // accepted; and one account the graph holds no record of at all.
    let invited = account_id(0);
    let stranger = account_id(1);
    invite(&router, &graph_arn, std::slice::from_ref(&invited)).await;

    // A membership in a status only `AcceptInvitation` leads to — and that operation is
    // still a stub — so the 409 row is seeded in the store rather than driven through the
    // wire. Everything about the row after that is the wire path.
    let accepted = account_id(2);
    invite(&router, &graph_arn, std::slice::from_ref(&accepted)).await;
    {
        let state = service_state(&*engine).expect("this service's store");
        let mut store = state.lock();
        let graph = store.graph_mut(&graph_arn).expect("the created graph");
        let member = graph.members.get_mut(&accepted).expect("the invited member");
        member.status = MEMBER_STATUS_ENABLED.to_string();
        // The verification the invitation started is no longer outstanding, so nothing is
        // owed that would move the status back.
        member.verification_due_millis = None;
    }

    let cases = vec![
        ErrorCase {
            name: "RejectInvitation on a graph nothing was stored under",
            method: "POST",
            uri: REJECT_INVITATION_URI.to_string(),
            body: json!({ "GraphArn": absent }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(NO_SUCH_MEMBERSHIP_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "RejectInvitation on a graph holding no invitation for the caller",
            method: "POST",
            uri: REJECT_INVITATION_URI.to_string(),
            body: json!({ "GraphArn": graph_arn }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(NO_SUCH_MEMBERSHIP_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "StartMonitoringMember naming an account the graph holds no record of",
            method: "POST",
            uri: MONITORING_STATE_URI.to_string(),
            body: json!({ "GraphArn": graph_arn, "AccountId": stranger }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(NO_SUCH_MEMBERSHIP_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "StartMonitoringMember on a membership nobody accepted",
            method: "POST",
            uri: MONITORING_STATE_URI.to_string(),
            body: json!({ "GraphArn": graph_arn, "AccountId": invited }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(INVITATION_NOT_ACCEPTED_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "StartMonitoringMember on a membership already ingesting",
            method: "POST",
            uri: MONITORING_STATE_URI.to_string(),
            body: json!({ "GraphArn": graph_arn, "AccountId": accepted }),
            status: 409,
            code: "ConflictException",
            // The handler's own sentence, which names the one status this operation acts
            // on; this suite is asserting the KEY it travels under.
            message: None,
        },
        ErrorCase {
            name: "GetMembers on a graph nothing was stored under",
            method: "POST",
            uri: GET_MEMBERS_URI.to_string(),
            body: json!({ "GraphArn": absent, "AccountIds": [account_id(3)] }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(GRAPH_NOT_FOUND_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "DeleteMembers on a graph nothing was stored under",
            method: "POST",
            uri: DELETE_MEMBERS_URI.to_string(),
            body: json!({ "GraphArn": absent, "AccountIds": [account_id(3)] }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(GRAPH_NOT_FOUND_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "ListDatasourcePackages carrying a MaxResults it does not paginate by",
            method: "POST",
            uri: LIST_DATASOURCES_URI.to_string(),
            body: json!({ "GraphArn": graph_arn, "MaxResults": 1 }),
            status: 400,
            code: "ValidationException",
            message: Some(MAX_RESULTS_MUST_BE_NULL_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "GetMembers whose batch holds a non-numeric account id",
            method: "POST",
            uri: GET_MEMBERS_URI.to_string(),
            body: json!({ "GraphArn": graph_arn, "AccountIds": ["abcdefghijkl"] }),
            status: 400,
            code: "ValidationException",
            message: Some("GetMembersRequest.AccountIds members did not match the pattern.".to_string()),
        },
        ErrorCase {
            name: "GetMembers whose GraphArn is outside its modelled pattern",
            method: "POST",
            uri: GET_MEMBERS_URI.to_string(),
            body: json!({ "GraphArn": "arn:aws:detective:::graph:nope", "AccountIds": [account_id(3)] }),
            status: 400,
            code: "ValidationException",
            // The boundary walk's own envelope, which `refusal_texts` owns the wording of.
            message: None,
        },
        ErrorCase {
            name: "TagResource carrying the reserved key prefix",
            method: "POST",
            uri: format!("{TAG_RESOURCE_URI}/{graph_arn}"),
            body: json!({ "Tags": { "aws:reserved": "v" } }),
            status: 400,
            code: "BadRequestException",
            message: Some(RESERVED_TAG_KEY_MESSAGE.to_string()),
        },
        ErrorCase {
            name: "AcceptInvitation, an operation that is still a stub",
            method: "POST",
            uri: ACCEPT_INVITATION_URI.to_string(),
            body: json!({ "GraphArn": graph_arn }),
            status: 501,
            code: "NotImplemented",
            message: None,
        },
    ];

    for case in cases {
        let name = case.name;
        let (status, answered) = call_with(&router, case.method, REGION, &case.uri, &case.body).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected {}, got {status} {answered:?}",
            case.status
        );
        assert_eq!(
            error_code(&answered),
            case.code,
            "case {name:?}: expected {}, got {answered:?}",
            case.code
        );
        let message = served_message(name, &answered);
        match case.message {
            Some(expected) => assert_eq!(
                message, expected,
                "case {name:?}: expected {expected:?}, got {message:?}"
            ),
            None => assert!(
                !message.trim().is_empty(),
                "case {name:?}: expected a non-empty message, got {message:?}"
            ),
        }
    }

    // The control for the 409 row: the sentence names the one status data ingest can be
    // enabled from, so an operator reading it learns what to do next.
    let (_, conflict) = call(
        &router,
        MONITORING_STATE_URI,
        &json!({ "GraphArn": graph_arn, "AccountId": accepted }),
    )
    .await;
    let message = served_message("the conflict sentence", &conflict);
    assert!(
        message.contains(MEMBER_STATUS_ACCEPTED_BUT_DISABLED),
        "the conflict sentence must name the status ingest is enabled from, got {message:?}"
    );
}

/// One row of the list-member table: a batch, and the account ids in it that violate the
/// element `@pattern`.
struct BatchCase {
    name: &'static str,
    batch: Vec<String>,
    offending: Vec<String>,
}

#[tokio::test]
async fn the_delete_of_a_graph_that_is_already_gone_states_no_message() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let (status, _) = call(&router, "/graph/removal", &json!({ "GraphArn": graph_arn })).await;
    assert_eq!(status, 200, "the first delete must be served");

    // The rows differ along HOW the graph came to be absent — deleted a moment ago, and
    // never minted at all — because the condition is the same one either way.
    for (how, arn) in [
        ("deleted", graph_arn.clone()),
        ("never created", never_created_arn()),
    ] {
        let case = format!("DeleteGraph on a {how} graph");
        let (status, answered) =
            call(&router, "/graph/removal", &json!({ "GraphArn": arn })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered:?}"
        );
        // This one refusal states nothing: no message member under either spelling.
        assert!(
            answered.get("message").is_none() && answered.get(MODEL_MESSAGE_MEMBER).is_none(),
            "case {case:?}: the delete of an absent graph states no message, got {answered:?}"
        );
        // The control, on the same absent graph: an operation that READS it does state
        // its surface's fixed sentence, under the one key rule 1 requires.
        let (status, read) = call(&router, LIST_MEMBERS_URI, &json!({ "GraphArn": arn })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListMembers expected 404, got {status} {read:?}"
        );
        assert_eq!(
            served_message(&case, &read),
            GRAPH_NOT_FOUND_MESSAGE,
            "case {case:?}: ListMembers must state the graph-not-found sentence, got {read:?}"
        );
    }
}

/// Account ids of the modelled length (12) that are outside the modelled `@pattern`
/// `^[0-9]+$` (`smithy_query(query="shape",shape="AccountIdList")`): the rows differ in
/// WHERE the non-digit sits, in what it is, and in whether the digits are ASCII at all.
fn non_numeric_ids() -> Vec<String> {
    let ids = vec![
        "abcdefghijkl".to_string(),
        "1234abcd9012".to_string(),
        "12345678901a".to_string(),
        "a12345678901".to_string(),
        "1234 5678901".to_string(),
        "-12345678901".to_string(),
        // Digits that are not ASCII digits: twelve Devanagari ones.
        "\u{0967}\u{0968}\u{0969}\u{096a}\u{096b}\u{096c}\u{096d}\u{096e}\u{096f}\u{0966}\u{0967}\u{0968}"
            .to_string(),
    ];
    for id in &ids {
        assert_eq!(
            id.chars().count(),
            ACCOUNT_ID_LEN,
            "a row of this table must satisfy the element length bound so the PATTERN is \
             what it breaks: {id:?}"
        );
    }
    ids
}

#[tokio::test]
async fn a_list_member_pattern_refusal_names_the_input_shape_and_echoes_nothing() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let member = account_id(0);
    invite(&router, &graph_arn, std::slice::from_ref(&member)).await;
    let held: BTreeSet<String> = [member.clone(), ACCOUNT.to_string()].into_iter().collect();

    let offenders = non_numeric_ids();
    let mut cases: Vec<BatchCase> = Vec::new();
    for offender in &offenders {
        cases.push(BatchCase {
            name: "the only entry",
            batch: vec![offender.clone()],
            offending: vec![offender.clone()],
        });
    }
    // Where in the batch the offending entry sits cannot change the clause, because the
    // clause names no position: first, in the middle, and last of a batch at the
    // modelled maximum.
    let first = offenders[0].clone();
    let last = offenders[1].clone();
    cases.push(BatchCase {
        name: "the first of three",
        batch: vec![first.clone(), member.clone(), account_id(9)],
        offending: vec![first.clone()],
    });
    cases.push(BatchCase {
        name: "the second of three",
        batch: vec![member.clone(), first.clone(), account_id(9)],
        offending: vec![first.clone()],
    });
    let mut largest: Vec<String> = (10..10 + (MAX_ACCOUNT_IDS as u64 - 1))
        .map(account_id)
        .collect();
    largest.push(last.clone());
    assert_eq!(
        largest.len(),
        MAX_ACCOUNT_IDS,
        "the largest batch row must name exactly the modelled maximum"
    );
    cases.push(BatchCase {
        name: "the last of a batch at the modelled maximum",
        batch: largest,
        offending: vec![last.clone()],
    });
    cases.push(BatchCase {
        name: "two offending entries are still one clause",
        batch: vec![first.clone(), member.clone(), last.clone()],
        offending: vec![first, last],
    });

    for case in cases {
        // The one rule, over BOTH operations of this service that declare a batch of
        // account ids: each names its OWN input shape.
        for (operation, uri, input_shape) in [
            ("GetMembers", GET_MEMBERS_URI, "GetMembersRequest"),
            ("DeleteMembers", DELETE_MEMBERS_URI, "DeleteMembersRequest"),
        ] {
            let name = format!("{operation}: {} ({:?})", case.name, case.offending);
            let (status, answered) = call(
                &router,
                uri,
                &json!({ "GraphArn": graph_arn, "AccountIds": case.batch }),
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
            let message = served_message(&name, &answered);
            assert_eq!(
                message,
                format!("{input_shape}.AccountIds members did not match the pattern."),
                "case {name:?}: expected the clause naming {input_shape}.AccountIds, got \
                 {message:?}"
            );
            for offender in &case.offending {
                assert!(
                    !message.contains(offender.as_str()),
                    "case {name:?}: the clause must not echo the rejected value, got \
                     {message:?}"
                );
            }

            // A refused request changes nothing: the memberships the graph held before it
            // are the ones it holds after.
            assert_eq!(
                listed_members(&router, &graph_arn).await,
                held,
                "case {name:?}: a refused batch must leave every membership in place"
            );
        }
    }

    // The control, and the boundary between the two constraint classes: an entry inside
    // the pattern but outside the element's 12..12 `@length` is still refused, with the
    // generic envelope this service keeps for every other member — so the clause above
    // belongs to the pattern class alone.
    let too_short = "1".repeat(ACCOUNT_ID_LEN - 1);
    let (status, answered) = call(
        &router,
        GET_MEMBERS_URI,
        &json!({ "GraphArn": graph_arn, "AccountIds": [too_short] }),
    )
    .await;
    assert_eq!(status, 400, "an entry under the length bound: got {answered:?}");
    let message = served_message("an entry under the length bound", &answered);
    assert!(
        message.contains("failed to satisfy constraint")
            && message.contains(&format!("length greater than or equal to {ACCOUNT_ID_LEN}")),
        "an entry under the length bound must keep the generic envelope, got {message:?}"
    );

    // And a batch of well-formed account ids is not refused at all, whether or not the
    // graph holds a membership for them.
    let (status, answered) = call(
        &router,
        GET_MEMBERS_URI,
        &json!({ "GraphArn": graph_arn, "AccountIds": [member, account_id(9)] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a well-formed batch must be served, got {status} {answered:?}"
    );
}

/// One row of the removal table: the accounts one call names, the memberships it must
/// remove, and the accounts it must report unprocessed.
struct RemovalCase {
    name: &'static str,
    batch: Vec<String>,
    expect_removed: Vec<String>,
    expect_unprocessed: Vec<String>,
}

#[tokio::test]
async fn a_member_removal_always_serves_the_accounts_it_removed() {
    // `member_removal` owns the removal TABLE (which memberships a batch drops, and what
    // it reports for every other account it names). What these rows add is the body's
    // shape: `AccountIds` is served whatever the outcome, and the reason an unprocessed
    // account carries is the same text the 404 for that condition carries.
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let members: Vec<String> = (0..4).map(account_id).collect();
    invite(&router, &graph_arn, &members).await;

    // The rows differ along HOW MANY of the named accounts the graph held a membership
    // for: none, some, and all of them. The member is served in every one.
    let strangers: Vec<String> = (20..23).map(account_id).collect();
    let cases = vec![
        RemovalCase {
            name: "no named account holds a membership",
            batch: strangers.clone(),
            expect_removed: Vec::new(),
            expect_unprocessed: strangers.clone(),
        },
        RemovalCase {
            name: "one of two holds a membership",
            batch: vec![members[0].clone(), strangers[0].clone()],
            expect_removed: vec![members[0].clone()],
            expect_unprocessed: vec![strangers[0].clone()],
        },
        RemovalCase {
            name: "every named account holds a membership",
            batch: vec![members[1].clone(), members[2].clone()],
            expect_removed: vec![members[1].clone(), members[2].clone()],
            expect_unprocessed: Vec::new(),
        },
    ];

    for case in cases {
        let RemovalCase {
            name,
            batch,
            expect_removed,
            expect_unprocessed,
        } = case;
        let (status, answered) = call(
            &router,
            DELETE_MEMBERS_URI,
            &json!({ "GraphArn": graph_arn, "AccountIds": batch }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200, got {status} {answered:?}"
        );
        let removed: Vec<String> = answered
            .get("AccountIds")
            .unwrap_or_else(|| {
                panic!("case {name:?}: the answer served no AccountIds at all: {answered:?}")
            })
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: AccountIds is not a list: {answered:?}"))
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        assert_eq!(
            removed.iter().cloned().collect::<BTreeSet<String>>(),
            expect_removed.iter().cloned().collect::<BTreeSet<String>>(),
            "case {name:?}: expected {expect_removed:?} removed, got {answered:?}"
        );
        // Every other account it named is reported with the ONE text this service words
        // the missing-membership condition with — the same one its 404 carries.
        let reasons: BTreeMap<String, String> = answered
            .get("UnprocessedAccounts")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {name:?}: the answer served no UnprocessedAccounts: {answered:?}")
            })
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.get("AccountId")?.as_str()?.to_string(),
                    entry.get("Reason")?.as_str()?.to_string(),
                ))
            })
            .collect();
        assert_eq!(
            reasons,
            expect_unprocessed
                .iter()
                .map(|id| (id.clone(), REMOVAL_REASON.to_string()))
                .collect::<BTreeMap<String, String>>(),
            "case {name:?}: expected {expect_unprocessed:?} unprocessed with the \
             missing-membership text, got {answered:?}"
        );
    }

    // One condition, one text: the reason a removal reports for an account the graph
    // holds no membership for is the message its 404 carries for the same condition.
    let (status, refused) = call(
        &router,
        MONITORING_STATE_URI,
        &json!({ "GraphArn": graph_arn, "AccountId": strangers[0] }),
    )
    .await;
    assert_eq!(status, 404, "a non-member account: got {status} {refused:?}");
    assert_eq!(
        served_message("the 404 for the same condition", &refused),
        REMOVAL_REASON,
        "the 404 and the unprocessed reason must be one text, got {refused:?}"
    );
}

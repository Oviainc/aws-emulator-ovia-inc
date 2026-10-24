// SPDX-License-Identifier: CC0-1.0

//! What a refused account-link call SAYS, over the awsJson1_1 wire.
//!
//! The rule this suite pins, for all six account-link operations:
//!
//! > an account-link error's `message` is a fixed sentence chosen by the ERROR
//! > CLASS, never built from the request — no member name, no rejected value, no
//! > identifier and no constraint expression from the caller's request reaches the
//! > text — and the identity of a resource that could not be found travels in
//! > `ResourceNotFoundException`'s own `ResourceId` member instead.
//!
//! That is the request-independence the recordings establish: two different link
//! ids are refused with ONE identical not-found sentence, and three operations
//! refusing two different members with values of their own are refused with ONE
//! identical validation sentence. The recorded sentences themselves are examples of
//! the rule; what every case below asserts is that the text is the class's
//! constant, and that nothing the request carried appears in it.
//!
//! The tables therefore vary what the rule generalises over — the operation, the
//! member, the value, its size, its form, and the CLASS of constraint it breaks (a
//! `@pattern`, a `@range` bound, an enum value, an absent `@required` member) —
//! rather than restating the recorded rows. The last test pins the SCOPE from the
//! other side: this service's older operations keep the framework's
//! `N validation error detected: …` wording, which no recording shows them
//! answering anything else with.

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    ACCOUNT_LINK_ALREADY_LINKED_MESSAGE, ACCOUNT_LINK_INVALID_REQUEST_MESSAGE,
    ACCOUNT_LINK_NOT_FOUND_MESSAGE, ACCOUNT_LINK_STATUS_CONFLICT_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_links::{accounts_exist, error_code};
use crate::workspace_lifecycle::{call, emulator_with_engine};

/// The `message` member of an error body, as the JSON protocols carry it.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The two class sentences the RECORDINGS carried, pinned as literals so the
/// constants every case below compares against cannot drift away from the corpus
/// unnoticed. The rule is request-independence, which the cases assert over sets of
/// inputs; this one row per class keeps the recorded example anchored here too.
#[test]
fn the_class_sentences_are_the_ones_the_recordings_carried() {
    for (case, actual, recorded) in [
        (
            "a refused account-link request",
            ACCOUNT_LINK_INVALID_REQUEST_MESSAGE,
            "The request is invalid.",
        ),
        (
            "an account link that does not exist",
            ACCOUNT_LINK_NOT_FOUND_MESSAGE,
            "Requested AccountLink could not be found.",
        ),
    ] {
        assert_eq!(
            actual, recorded,
            "case {case:?}: the class sentence must stay the one the recording carried, \
             got {actual:?}"
        );
    }
}

/// Assert that `message` carries nothing the request did: none of `sent` (the
/// values and member names the caller supplied), and none of the framework's
/// constraint-reporting vocabulary.
fn assert_request_independent(case: &str, message: &str, sent: &[&str]) {
    for fragment in sent.iter().filter(|fragment| !fragment.is_empty()) {
        assert!(
            !message.contains(fragment),
            "case {case:?}: the message must not carry the request fragment \
             {fragment:?}, got {message:?}"
        );
    }
    for leak in [
        "validation error detected",
        "failed to satisfy constraint",
        "regular expression pattern",
    ] {
        assert!(
            !message.contains(leak),
            "case {case:?}: the message must not expose framework validation detail \
             {leak:?}, got {message:?}"
        );
    }
}

/// One refused request: how it breaks the model, and the fragments of it that may
/// not reach the message.
struct Refusal {
    case: String,
    operation: &'static str,
    request: Value,
    /// What the caller sent — member names and values — none of which may appear in
    /// the answer.
    sent: Vec<String>,
}

/// Build one row, taking the case name and the fragments as anything string-like so a
/// generated value (an over-long token, an over-long id) needs no leaked `&'static str`.
fn refusal(
    case: &str,
    operation: &'static str,
    request: Value,
    sent: &[&str],
) -> Refusal {
    Refusal {
        case: case.to_string(),
        operation,
        request,
        sent: sent.iter().map(|fragment| (*fragment).to_string()).collect(),
    }
}

/// Requests the account-link API refuses as invalid, spanning every class of
/// constraint its inputs declare and every operation that declares one.
///
/// Constraints from
/// `smithy_query(query="operation",operation="AcceptAccountLinkInvitation,CreateAccountLinkInvitation,DeleteAccountLinkInvitation,GetAccountLink,ListAccountLinks,RejectAccountLinkInvitation")`:
/// `LinkId` is `^link-.{8,24}$` and `@required` on accept/reject/delete,
/// `TargetAccountId` is `AwsAccount` `^\d{12}$` and `@required`, `LinkedAccountId`
/// is the same `AwsAccount`, `ClientToken` is `^.{1,64}$`, `MaxResults` is
/// `range 1..25`, and `LinkStatusFilter` holds `AccountLinkStatusEnum`.
fn invalid_requests() -> Vec<Refusal> {
    let too_long_token = "x".repeat(65);
    let too_long_link = format!("link-{}", "z".repeat(25));
    vec![
        refusal(
            "a target account id shorter than ^\\d{12}$",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "12345" }),
            &["12345", "TargetAccountId", "targetAccountId"],
        ),
        refusal(
            "a target account id longer than ^\\d{12}$",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "1234567890123" }),
            &["1234567890123", "TargetAccountId", "targetAccountId"],
        ),
        refusal(
            "a target account id of the right length that is not digits",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "abcdefghijkl" }),
            &["abcdefghijkl", "TargetAccountId", "targetAccountId"],
        ),
        refusal(
            "an empty target account id",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "" }),
            &["TargetAccountId", "targetAccountId"],
        ),
        refusal(
            "no target account id at all (an absent @required member)",
            "CreateAccountLinkInvitation",
            json!({}),
            &["TargetAccountId", "targetAccountId"],
        ),
        refusal(
            "a client token longer than ^.{1,64}$",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "123456789012", "ClientToken": &too_long_token }),
            &[&too_long_token, "ClientToken", "clientToken"],
        ),
        refusal(
            "an empty client token",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "123456789012", "ClientToken": "" }),
            &["ClientToken", "clientToken"],
        ),
        refusal(
            "a link id with no link- prefix, read",
            "GetAccountLink",
            json!({ "LinkId": "not-a-link" }),
            &["not-a-link", "LinkId", "linkId"],
        ),
        refusal(
            "a link id one character short of ^link-.{8,24}$",
            "GetAccountLink",
            json!({ "LinkId": "link-1234567" }),
            &["link-1234567", "LinkId", "linkId"],
        ),
        refusal(
            "a link id one character past ^link-.{8,24}$",
            "GetAccountLink",
            json!({ "LinkId": &too_long_link }),
            &[&too_long_link, "LinkId", "linkId"],
        ),
        refusal(
            "a linked account id that is not twelve digits",
            "GetAccountLink",
            json!({ "LinkedAccountId": "12345" }),
            &["12345", "LinkedAccountId", "linkedAccountId"],
        ),
        refusal(
            "a link id with no link- prefix, deleted",
            "DeleteAccountLinkInvitation",
            json!({ "LinkId": "not-a-link" }),
            &["not-a-link", "LinkId", "linkId"],
        ),
        refusal(
            "no link id at all, deleted",
            "DeleteAccountLinkInvitation",
            json!({}),
            &["LinkId", "linkId"],
        ),
        refusal(
            "a link id with no link- prefix, accepted",
            "AcceptAccountLinkInvitation",
            json!({ "LinkId": "nope" }),
            &["nope", "LinkId", "linkId"],
        ),
        refusal(
            "no link id at all, accepted",
            "AcceptAccountLinkInvitation",
            json!({}),
            &["LinkId", "linkId"],
        ),
        refusal(
            "a link id with no link- prefix, rejected",
            "RejectAccountLinkInvitation",
            json!({ "LinkId": "link-x" }),
            &["link-x", "LinkId", "linkId"],
        ),
        refusal(
            "a page size one below the modelled range 1..25",
            "ListAccountLinks",
            json!({ "MaxResults": 0 }),
            &["MaxResults", "maxResults"],
        ),
        refusal(
            "a page size one above the modelled range 1..25",
            "ListAccountLinks",
            json!({ "MaxResults": 26 }),
            &["26", "MaxResults", "maxResults"],
        ),
        refusal(
            "a link status the model's enum does not declare",
            "ListAccountLinks",
            json!({ "LinkStatusFilter": ["NOT_A_STATUS"] }),
            &["NOT_A_STATUS", "LinkStatusFilter", "linkStatusFilter"],
        ),
        refusal(
            "a next token this service never minted",
            "ListAccountLinks",
            json!({ "NextToken": "not-a-token" }),
            &["not-a-token", "NextToken", "nextToken"],
        ),
        refusal(
            "a read naming neither a link nor an account",
            "GetAccountLink",
            json!({}),
            &["LinkId", "LinkedAccountId"],
        ),
    ]
}

#[tokio::test]
async fn every_invalid_account_link_request_is_refused_with_one_fixed_sentence() {
    let (router, engine) = emulator_with_engine();
    // The rows that are well-formed enough to reach a handler still must not be
    // answered on the strength of an account that does not exist, so the account
    // the valid-looking rows name exists.
    accounts_exist(&engine, &["123456789012"]);

    for Refusal {
        case,
        operation,
        request,
        sent,
    } in invalid_requests()
    {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({operation}): expected HTTP 400, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} ({operation}): expected ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, ACCOUNT_LINK_INVALID_REQUEST_MESSAGE,
            "case {case:?} ({operation}): every invalid account-link request is answered \
             with the class's one sentence {ACCOUNT_LINK_INVALID_REQUEST_MESSAGE:?}, got \
             {message:?}"
        );
        let sent: Vec<&str> = sent.iter().map(String::as_str).collect();
        assert_request_independent(&case, &message, &sent);
    }
}

/// Link ids that satisfy `^link-.{8,24}$` yet name no stored link: the two the
/// recordings used, both published length bounds, and forms the pattern's `.`
/// admits beyond the minted `link-<16 hex>` shape.
fn unknown_link_ids() -> Vec<&'static str> {
    vec![
        "link-doesnotexist0000",
        "link-doesnotexist0001",
        "link-12345678",
        "link-zzzzzzzzzzzzzzzzzzzzzzzz",
        "link-UPPER-case-1234",
    ]
}

#[tokio::test]
async fn an_unknown_link_is_reported_in_resource_id_and_never_in_the_message() {
    let (router, _engine) = emulator_with_engine();

    for operation in [
        "AcceptAccountLinkInvitation",
        "RejectAccountLinkInvitation",
        "DeleteAccountLinkInvitation",
        "GetAccountLink",
    ] {
        for link_id in unknown_link_ids() {
            let case = format!("{operation} on {link_id}");
            let (status, body) = call(&router, operation, &json!({ "LinkId": link_id })).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with body {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, ACCOUNT_LINK_NOT_FOUND_MESSAGE,
                "case {case}: every unknown link is answered with the class's one \
                 sentence {ACCOUNT_LINK_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert_request_independent(&case, &message, &[link_id, "LinkId", "linkId"]);
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(link_id),
                "case {case}: the identifier travels in the modelled ResourceId member, \
                 got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_account_that_holds_no_link_is_reported_the_same_way() {
    let (router, _engine) = emulator_with_engine();

    // Every row is a twelve-digit account this emulator stores no link for, so the
    // read names a resource that does not exist rather than a malformed one.
    for account_id in ["123456789012", "000000000001", "999999999999"] {
        let case = format!("GetAccountLink by LinkedAccountId {account_id}");
        let (status, body) = call(
            &router,
            "GetAccountLink",
            &json!({ "LinkedAccountId": account_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, ACCOUNT_LINK_NOT_FOUND_MESSAGE,
            "case {case}: expected {ACCOUNT_LINK_NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
        assert_request_independent(&case, &message, &[account_id, "LinkedAccountId"]);
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(account_id),
            "case {case}: the identifier travels in the modelled ResourceId member, got \
             {body:?}"
        );
    }
}

/// Invite `target` and return the minted `AccountLinkId`.
async fn invite(router: &MultiServiceRouter, target: &str) -> String {
    let (status, body) = call(
        router,
        "CreateAccountLinkInvitation",
        &json!({ "TargetAccountId": target }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case invite {target}: expected HTTP 200, got {status} with body {body:?}"
    );
    body["AccountLink"]["AccountLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case invite {target}: a minted AccountLinkId, got {body:?}"))
        .to_string()
}

#[tokio::test]
async fn a_link_whose_status_forbids_the_change_states_the_class_not_the_link() {
    let (router, engine) = emulator_with_engine();
    let targets = ["000000000001", "000000000002", "000000000003"];
    accounts_exist(&engine, &targets);

    // One link per terminal status the invitation workflow can reach, so the rows
    // differ in the STORED state the refusal is about as well as in the operation
    // asked for.
    let accepted = invite(&router, targets[0]).await;
    let (status, body) = call(
        &router,
        "AcceptAccountLinkInvitation",
        &json!({ "LinkId": accepted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case accept a pending invitation: expected HTTP 200, got {status} with body {body:?}"
    );

    let rejected = invite(&router, targets[1]).await;
    let (status, body) = call(
        &router,
        "RejectAccountLinkInvitation",
        &json!({ "LinkId": rejected }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case reject a pending invitation: expected HTTP 200, got {status} with body {body:?}"
    );

    for (case, operation, link_id) in [
        ("accept a link that is already linked", "AcceptAccountLinkInvitation", accepted.as_str()),
        ("reject a link that is already linked", "RejectAccountLinkInvitation", accepted.as_str()),
        ("delete a link that is already linked", "DeleteAccountLinkInvitation", accepted.as_str()),
        ("accept a link that was rejected", "AcceptAccountLinkInvitation", rejected.as_str()),
        ("reject a link that was rejected", "RejectAccountLinkInvitation", rejected.as_str()),
    ] {
        let (status, body) = call(&router, operation, &json!({ "LinkId": link_id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, ACCOUNT_LINK_STATUS_CONFLICT_MESSAGE,
            "case {case:?}: expected the class's one sentence \
             {ACCOUNT_LINK_STATUS_CONFLICT_MESSAGE:?}, got {message:?}"
        );
        // Neither the link the caller named nor the status it is in may appear.
        assert_request_independent(
            case,
            &message,
            &[
                link_id,
                "LINKED",
                "REJECTED",
                "PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT",
            ],
        );
    }

    // A target that already holds a link or an invitation is the OTHER conflict
    // class, and states its own sentence — again with no value of the request in it.
    for (case, target) in [
        ("a target that is already linked", targets[0]),
        ("a target whose invitation was rejected and re-invited", targets[1]),
    ] {
        let (status, body) = call(
            &router,
            "CreateAccountLinkInvitation",
            &json!({ "TargetAccountId": target }),
        )
        .await;
        if case == "a target that is already linked" {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, ACCOUNT_LINK_ALREADY_LINKED_MESSAGE,
                "case {case:?}: expected the class's one sentence \
                 {ACCOUNT_LINK_ALREADY_LINKED_MESSAGE:?}, got {message:?}"
            );
            assert_request_independent(case, &message, &[target, &accepted]);
        } else {
            // A rejected invitation no longer holds the target, so re-inviting it
            // is served; the row is here so the conflict above is known to be about
            // the STORED status and not about the target id.
            assert_eq!(
                status, 200,
                "case {case:?}: a rejected target may be invited again, got {status} with \
                 body {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_invalid_combination_of_members_states_the_same_validation_sentence() {
    let (router, engine) = emulator_with_engine();
    let source = engine::emulator_account_id();
    accounts_exist(&engine, &[source, "000000000001", "000000000002"]);

    // Rules the model states only in prose, so the handler owns them: a target that
    // is the source account ("You either haven't provided a TargetAccountId or are
    // using the same value for TargetAccountId and SourceAccountId"), and a client
    // token already spent on another target.
    let (status, body) = call(
        &router,
        "CreateAccountLinkInvitation",
        &json!({ "TargetAccountId": source }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a self-invitation: expected HTTP 400, got {status} with body {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a self-invitation: expected ValidationException, got {body:?}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, ACCOUNT_LINK_INVALID_REQUEST_MESSAGE,
        "case a self-invitation: expected {ACCOUNT_LINK_INVALID_REQUEST_MESSAGE:?}, got \
         {message:?}"
    );
    assert_request_independent("a self-invitation", &message, &[source, "TargetAccountId"]);

    let token = "one-shot-token";
    let (status, body) = call(
        &router,
        "CreateAccountLinkInvitation",
        &json!({ "TargetAccountId": "000000000001", "ClientToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the first use of a client token: expected HTTP 200, got {status} with body \
         {body:?}"
    );
    let (status, body) = call(
        &router,
        "CreateAccountLinkInvitation",
        &json!({ "TargetAccountId": "000000000002", "ClientToken": token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a client token reused for another target: expected HTTP 400, got {status} \
         with body {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a client token reused for another target: expected ValidationException, got \
         {body:?}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, ACCOUNT_LINK_INVALID_REQUEST_MESSAGE,
        "case a client token reused for another target: expected \
         {ACCOUNT_LINK_INVALID_REQUEST_MESSAGE:?}, got {message:?}"
    );
    assert_request_independent(
        "a client token reused for another target",
        &message,
        &[token, "000000000001", "000000000002", "ClientToken"],
    );
}

#[tokio::test]
async fn a_request_the_model_admits_is_still_served_and_reads_back() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(&engine, &["000000000001", "000000000002", "000000000003"]);

    // The wrapper that words the refusal must refuse nothing the model admits: a
    // value AT each published bound is served. `ClientToken` is `^.{1,64}$` and
    // `MaxResults` is `range 1..25`.
    let link_id = invite(&router, "000000000001").await;
    let (status, body) = call(&router, "GetAccountLink", &json!({ "LinkId": &link_id })).await;
    assert_eq!(
        status, 200,
        "case read the created link back by LinkId: expected HTTP 200, got {status} with \
         body {body:?}"
    );
    assert_eq!(
        body["AccountLink"]["AccountLinkId"].as_str(),
        Some(link_id.as_str()),
        "case read the created link back by LinkId: expected the link just created, got \
         {body:?}"
    );
    let (status, body) = call(
        &router,
        "GetAccountLink",
        &json!({ "LinkedAccountId": "000000000001" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case read the created link back by LinkedAccountId: expected HTTP 200, got \
         {status} with body {body:?}"
    );
    assert_eq!(
        body["AccountLink"]["AccountLinkId"].as_str(),
        Some(link_id.as_str()),
        "case read the created link back by LinkedAccountId: expected the link just \
         created, got {body:?}"
    );

    for (case, request) in [
        ("a one-character client token", json!({ "TargetAccountId": "000000000002", "ClientToken": "t" })),
        (
            "a sixty-four-character client token",
            json!({
                "TargetAccountId": "000000000003",
                "ClientToken": "0123456789012345678901234567890123456789012345678901234567890123",
            }),
        ),
    ] {
        let (status, body) = call(&router, "CreateAccountLinkInvitation", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value inside the modelled constraint is served, got {status} \
             with body {body:?}"
        );
    }

    for max_results in [1, 25] {
        let (status, body) = call(
            &router,
            "ListAccountLinks",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: a page size at the modelled bound is served, \
             got {status} with body {body:?}"
        );
    }
}

#[tokio::test]
async fn this_services_other_operations_keep_the_frameworks_constraint_wording() {
    let (router, _engine) = emulator_with_engine();

    // The fixed sentence is the account-link API's, and this pins that from the
    // other side: the operations whose model declares no `ValidationException`
    // still report the member, the value and the constraint the way the framework
    // (and AWS's older WorkSpaces surface) words them.
    for (case, operation, request, expected_fragment) in [
        (
            "a management CIDR range the pattern does not admit",
            "ModifyAccount",
            json!({ "DedicatedTenancyManagementCidrRange": "10.0.0.0/24" }),
            "10.0.0.0/24",
        ),
        (
            "a workspace id the pattern does not admit",
            "DescribeWorkspaces",
            json!({ "WorkspaceIds": ["nope"] }),
            "nope",
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {case:?}: an operation outside the account-link family keeps the \
             framework's envelope, got {message:?}"
        );
        assert!(
            message.contains(expected_fragment),
            "case {case:?}: the framework's envelope quotes the rejected value \
             {expected_fragment:?}, got {message:?}"
        );
        assert_ne!(
            message, ACCOUNT_LINK_INVALID_REQUEST_MESSAGE,
            "case {case:?}: the account-link sentence must not reach an operation outside \
             that family, got {message:?}"
        );
    }
}

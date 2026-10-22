// SPDX-License-Identifier: CC0-1.0

//! Which accounts an account link may name, over the awsJson1_1 wire.
//!
//! The rule under test, stated once: an account link can only be created between
//! accounts that EXIST and belong to one parent AWS Organization, so
//! `CreateAccountLinkInvitation` resolves its `TargetAccountId` against the
//! accounts this emulator holds and refuses an id that names none of them with
//! `AccessDeniedException` (HTTP 400 per the model's defaulted `httpError`),
//! persisting nothing. A well-formed twelve-digit id is only the first gate: the
//! model's `AwsAccount` pattern `^\d{12}$` says what an account id LOOKS like,
//! not that an account answers to it.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one
//! shared engine, so each assertion is about what a client would see.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_links::{accounts_exist, error_code, list_all};
use crate::workspace_lifecycle::{call, emulator_with_engine};

/// The status a link carries while it waits for the target account to answer.
const PENDING: &str = "PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT";

/// Target account ids that are well formed under `AwsAccount` (`^\d{12}$`) and
/// name no account this emulator holds.
///
/// The rows differ along what the rule generalises over — the VALUE of a
/// non-existent id — spanning the lowest and the highest twelve-digit value, a
/// leading-zero one, an arbitrary one and the one a recorded AWS call used.
fn non_existent_targets() -> Vec<&'static str> {
    vec![
        "000000000001",
        "999999999999",
        "000000012345",
        "123456789012",
        "267370265480",
    ]
}

/// A `LinkId` matching the model's `^link-.{8,24}$` that this service never
/// minted, used to show a denied invitation left no link to act on.
const FABRICATED_LINK_ID: &str = "link-0000000000000001";

/// Invite `target_account_id`, with `client_token` when one is given, and return
/// `(status, body)`.
async fn invite(
    router: &MultiServiceRouter,
    target_account_id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({ "TargetAccountId": target_account_id });
    if let Some(token) = client_token {
        request["ClientToken"] = json!(token);
    }
    call(router, "CreateAccountLinkInvitation", &request).await
}

#[tokio::test]
async fn an_invitation_to_an_account_that_does_not_exist_is_denied_and_persists_nothing() {
    let (router, _engine) = emulator_with_engine();

    // Each row is a well-formed target id naming no existing account, invited
    // both with and without a `ClientToken`: neither form is grounds for a link.
    for target in non_existent_targets() {
        for client_token in [None, Some("eligibility-probe")] {
            let case = format!("target {target}, ClientToken {client_token:?}");
            let before = list_all(&router, &json!({})).await;

            let (status, body) = invite(&router, target, client_token).await;
            assert_eq!(
                status, 400,
                "case {case}: the invitation must be refused, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case}: expected AccessDeniedException, got {body:?}"
            );
            assert!(
                body["AccountLink"].is_null(),
                "case {case}: a refused invitation reports no AccountLink, got {body:?}"
            );

            // Nothing was persisted: the listing is what it was before the call,
            // and the account is linked to nothing.
            let after = list_all(&router, &json!({})).await;
            assert_eq!(
                after, before,
                "case {case}: a denied invitation must change no stored link, got {after:?}"
            );
            let (status, body) =
                call(&router, "GetAccountLink", &json!({ "LinkedAccountId": target })).await;
            assert_eq!(
                status, 400,
                "case {case}: the denied account must be linked to nothing, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // And the link the denied invitations never created cannot be read, answered
    // or deleted: every identifier a client might hold names no stored link.
    for operation in [
        "GetAccountLink",
        "AcceptAccountLinkInvitation",
        "RejectAccountLinkInvitation",
        "DeleteAccountLinkInvitation",
    ] {
        let (status, body) =
            call(&router, operation, &json!({ "LinkId": FABRICATED_LINK_ID })).await;
        assert_eq!(
            status, 400,
            "case {operation}: a link that was never created must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {operation}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_invitation_to_an_existing_organization_account_is_created_and_reads_back() {
    let (router, engine) = emulator_with_engine();
    // The same ids the denial table uses: what changes the answer is whether the
    // account EXISTS, not how its id is spelled.
    let targets = non_existent_targets();
    accounts_exist(&engine, &targets);

    let mut created = Vec::new();
    for target in &targets {
        let (status, body) = invite(&router, target, None).await;
        assert_eq!(
            status, 200,
            "case {target}: an existing account may be invited, got {status} with {body:?}"
        );
        let link = &body["AccountLink"];
        assert_eq!(
            link["AccountLinkStatus"].as_str(),
            Some(PENDING),
            "case {target}: a new invitation awaits the target account, got {body:?}"
        );
        assert_eq!(
            link["TargetAccountId"].as_str(),
            Some(*target),
            "case {target}: the invited account is the target, got {body:?}"
        );
        assert_eq!(
            link["SourceAccountId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {target}: the inviting account is the source, got {body:?}"
        );
        let link_id = link["AccountLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {target}: a minted AccountLinkId, got {body:?}"))
            .to_string();

        // Round trip: the created link reads back through GetAccountLink.
        let (status, body) = call(&router, "GetAccountLink", &json!({ "LinkId": link_id })).await;
        assert_eq!(
            status, 200,
            "case {target}: the created link must read back, got {status} with {body:?}"
        );
        assert_eq!(
            body["AccountLink"]["TargetAccountId"].as_str(),
            Some(*target),
            "case {target}: GetAccountLink must report the invited account, got {body:?}"
        );
        created.push(link_id);
    }

    let listed = list_all(&router, &json!({})).await;
    assert_eq!(
        listed, created,
        "every created link must be listed in creation order"
    );
}

#[tokio::test]
async fn a_self_invitation_is_a_validation_error_not_a_denial() {
    let (router, _engine) = emulator_with_engine();

    // The emulator's own account EXISTS, so the eligibility gate passes and the
    // modelled refusal is the documented one: "you are using the same value for
    // TargetAccountId and SourceAccountId" (`ValidationException`).
    let source = engine::emulator_account_id();
    let (status, body) = invite(&router, source, None).await;
    assert_eq!(
        status, 400,
        "case self-invitation: the request must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case self-invitation: expected ValidationException, got {body:?}"
    );

    let listed = list_all(&router, &json!({})).await;
    assert!(
        listed.is_empty(),
        "case self-invitation: no link may be persisted, got {listed:?}"
    );
}

#[tokio::test]
async fn an_account_that_exists_is_still_refused_once_it_holds_a_link() {
    let (router, engine) = emulator_with_engine();
    let target = "000000000001";
    accounts_exist(&engine, &[target]);

    let (status, body) = invite(&router, target, None).await;
    assert_eq!(
        status, 200,
        "case {target}: the first invitation must be served, got {status} with {body:?}"
    );

    // Existence is one gate among several, not a licence: the account it names is
    // now invited, so a second invitation is the modelled conflict rather than a
    // denial or a second link.
    let (status, body) = invite(&router, target, None).await;
    assert_eq!(
        status, 400,
        "case {target}: the second invitation must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case {target}: expected ConflictException, got {body:?}"
    );
    let listed = list_all(&router, &json!({})).await;
    assert_eq!(
        listed.len(),
        1,
        "case {target}: the refused invitation must not create a second link, got {listed:?}"
    );
}

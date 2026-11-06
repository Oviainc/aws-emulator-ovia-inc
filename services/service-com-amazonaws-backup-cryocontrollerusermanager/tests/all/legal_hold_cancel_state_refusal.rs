// SPDX-License-Identifier: CC0-1.0

//! `CancelLegalHold` refused for the STATE the hold is stored in, driven as real
//! HTTP through the router.
//!
//! The rule under test: a cancellation of a legal hold whose stored
//! `LegalHoldStatus` is not `ACTIVE` is refused with the modelled
//! `InvalidResourceStateException` (HTTP 400, `smithy_query` operation
//! `CancelLegalHold`) carrying the condition's constant token in `Code`, the
//! hold's CURRENT stored status in `Context`, and that same status interpolated
//! into the sentence's one variable part — so the refusal tells a client WHICH
//! state blocked the cancel, and two holds blocked by different states are
//! refused with different `Context` and different sentences.
//!
//! The rows of the table differ in the stored status that blocks the cancel
//! (`CREATING`, the status a hold whose create has not settled is in, and
//! `CANCELED`, the status a cancellation already left it in) and in whether the
//! refused request carries `retainRecordInDays` — which is what the rule
//! generalises over: the status it reports comes from state, not from the
//! request.
//!
//! The request harness (`router`, `call`, `call_reported`, `error_code`,
//! `string_at`) is the one `vault_and_backup_job.rs` exposes to this test
//! binary, so there is ONE wire path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

use super::vault_and_backup_job::{call, call_reported, error_code, router, string_at};

/// Longer than a legal hold's settle span (`crate::state`'s
/// `LEGAL_HOLD_SETTLE_AFTER`), so a call after this wait sees the hold in the
/// `ACTIVE` status the clock has already made due.
///
/// A blocking sleep rather than `tokio::time::sleep`, for the reason given on
/// `vault_and_backup_job::PAST_SETTLE`.
const PAST_LEGAL_HOLD_SETTLE: Duration = Duration::from_millis(3_400);

/// The error shape this condition names (`smithy_query` operation
/// `CancelLegalHold`: `InvalidResourceStateException`, `httpError` 400).
const INVALID_RESOURCE_STATE: &str = "InvalidResourceStateException";

/// The condition's token, which AWS Backup reports for every state that blocks a
/// cancellation.
const CANCEL_BLOCKED_CODE: &str = "ERROR_22003";

/// The FIXED text of the sentence AWS states for the condition; the stored
/// status and the closing `.` follow it.
const CANCEL_BLOCKED_PREFIX: &str =
    "Only ACTIVE legal holds can be canceled, the current state of provided legal hold is ";

/// The statuses the model lists for `LegalHoldStatus` (`smithy_query` shape
/// `LegalHoldStatus`: `CREATING | ACTIVE | CANCELING | CANCELED`) that a hold of
/// this service is stored in while a cancellation is blocked.
const STATUS_CREATING: &str = "CREATING";
const STATUS_CANCELED: &str = "CANCELED";

/// `POST /legal-holds` — `CreateLegalHold`'s HTTP binding.
async fn create_legal_hold(router: &MultiServiceRouter, title: &str) -> (u16, Value) {
    let request = json!({
        "Title": title,
        "Description": format!("a hold whose cancellation this case blocks: {title}"),
        "RecoveryPointSelection": {"VaultNames": ["cancel-state-vault"]},
    });
    call(router, "POST", "/legal-holds", Some(&request.to_string())).await
}

/// `DELETE /legal-holds/{LegalHoldId}` — `CancelLegalHold`'s HTTP binding, with
/// the query string its `cancelDescription` and `retainRecordInDays` bind to.
/// Answers with the error shape the response NAMES beside its status and body.
async fn cancel_legal_hold(
    router: &MultiServiceRouter,
    id: &str,
    query: &str,
) -> (u16, String, Value) {
    call_reported(router, "DELETE", &format!("/legal-holds/{id}{query}"), None).await
}

/// `GET /legal-holds/{LegalHoldId}` — `GetLegalHold`'s HTTP binding, answering
/// the `Status` the hold is STORED in, which is what the refusal has to report.
async fn stored_status(router: &MultiServiceRouter, id: &str) -> String {
    let (status, read) = call(router, "GET", &format!("/legal-holds/{id}"), None).await;
    assert_eq!(
        status, 200,
        "expected GetLegalHold 200 while reading the stored status, got {status} {read:?}"
    );
    string_at(&read, "Status")
}

/// Create a hold and leave it in the status `blocked_by` names, answering the id
/// it was stored under.
async fn hold_in_status(router: &MultiServiceRouter, case: &str, blocked_by: &str) -> String {
    let (status, created) = create_legal_hold(router, case).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLegalHold 200, got {status} {created:?}"
    );
    let id = string_at(&created, "LegalHoldId");

    if blocked_by != STATUS_CREATING {
        // The hold is in force only once its creation has settled, and only a
        // hold in force can be released — so the first cancellation is what
        // leaves it in the terminal status this row is about.
        std::thread::sleep(PAST_LEGAL_HOLD_SETTLE);
        let (status, _errortype, canceled) =
            cancel_legal_hold(router, &id, "?cancelDescription=first%20cancellation").await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first CancelLegalHold 201, got {status} {canceled:?}"
        );
    }
    id
}

/// A cancellation blocked by the hold's state is refused with the condition's
/// token, the stored status in `Context`, and that status interpolated into the
/// sentence — whichever state blocks it, and whatever the request carries.
#[tokio::test]
async fn a_cancel_blocked_by_the_holds_state_reports_that_state() {
    // (case, the stored status that blocks the cancel, the refused request's
    // query string)
    let cases: [(&str, &str, &str); 4] = [
        (
            "a hold whose create has not settled",
            STATUS_CREATING,
            "?cancelDescription=too-early",
        ),
        (
            "a hold whose create has not settled, with a record retention",
            STATUS_CREATING,
            "?cancelDescription=too-early-retained&retainRecordInDays=30",
        ),
        (
            "a hold a cancellation already released",
            STATUS_CANCELED,
            "?cancelDescription=second-cancellation",
        ),
        (
            "a hold a cancellation already released, with a record retention",
            STATUS_CANCELED,
            "?cancelDescription=second-cancellation-retained&retainRecordInDays=1",
        ),
    ];

    for (case, blocked_by, query) in cases {
        // One router per row: each row's hold is the only one the account
        // holds, so a row cannot see what another row stored.
        let router = router();
        let id = hold_in_status(&router, case, blocked_by).await;

        // The status the refusal must report is the one STATE holds, read back
        // through the hold's own reader rather than assumed by this test.
        let stored = stored_status(&router, &id).await;
        assert_eq!(
            stored, blocked_by,
            "case {case:?}: the fixture must leave the hold in {blocked_by}, got {stored:?}"
        );

        let (status, errortype, refused) = cancel_legal_hold(&router, &id, query).await;
        assert_eq!(
            (status, errortype.as_str(), error_code(&refused).as_str()),
            (400, INVALID_RESOURCE_STATE, INVALID_RESOURCE_STATE),
            "case {case:?}: expected the modelled state refusal, got {status} {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "Code"),
            CANCEL_BLOCKED_CODE,
            "case {case:?}: expected the condition's token, got {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "Context"),
            stored,
            "case {case:?}: expected the stored status in Context, got {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "Message"),
            format!("{CANCEL_BLOCKED_PREFIX}{stored}."),
            "case {case:?}: expected the stored status interpolated into the sentence, \
             got {refused:?}"
        );

        // A refused cancellation writes nothing: the hold is still in the state
        // that refused it, and a second attempt is refused identically.
        let after = stored_status(&router, &id).await;
        assert_eq!(
            after, stored,
            "case {case:?}: a refused cancellation must change no state, got {after:?}"
        );
        let (repeat_status, _errortype, repeated) = cancel_legal_hold(&router, &id, query).await;
        assert_eq!(
            (repeat_status, &repeated),
            (status, &refused),
            "case {case:?}: the same refusal must repeat, got {repeat_status} {repeated:?}"
        );
    }
}

/// The reported state is READ FROM STATE, not fixed: the same request refused
/// for two different stored statuses carries two different `Context` values and
/// two different sentences, and each names its own status.
#[tokio::test]
async fn two_blocking_states_are_refused_with_their_own_state() {
    let query = "?cancelDescription=blocked";
    let mut answered: Vec<(&str, String, String)> = Vec::new();

    for blocked_by in [STATUS_CREATING, STATUS_CANCELED] {
        let router = router();
        let id = hold_in_status(&router, blocked_by, blocked_by).await;
        let (status, _errortype, refused) = cancel_legal_hold(&router, &id, query).await;
        assert_eq!(
            status, 400,
            "case {blocked_by:?}: expected the cancel to be refused, got {status} {refused:?}"
        );
        answered.push((
            blocked_by,
            string_at(&refused, "Context"),
            string_at(&refused, "Message"),
        ));
    }

    for (blocked_by, context, message) in &answered {
        assert_eq!(
            (context.as_str(), message.as_str()),
            (
                *blocked_by,
                format!("{CANCEL_BLOCKED_PREFIX}{blocked_by}.").as_str()
            ),
            "case {blocked_by:?}: the refusal must name the state that blocked it, \
             got Context {context:?} Message {message:?}"
        );
    }
    assert_ne!(
        (&answered[0].1, &answered[0].2),
        (&answered[1].1, &answered[1].2),
        "two blocking states must not be reported identically, got {answered:?}"
    );
}

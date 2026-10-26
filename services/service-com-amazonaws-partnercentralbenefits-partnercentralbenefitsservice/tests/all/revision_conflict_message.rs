// SPDX-License-Identifier: CC0-1.0

//! The optimistic-concurrency refusal both revision-checking operations serve, end to end
//! over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`).
//!
//! The rule under test: a write that names a revision other than the application's
//! current one is refused with the modelled `ConflictException` (HTTP 409) whose message
//! reports the application, the revision it is actually at and the revision the request
//! named — in the same words from whichever operation refuses, because both read that
//! sentence from the one place the service writes it
//! (`crate::state::revision_conflict_message`, beside `status_conflict_message`).
//!
//! The table's rows differ along what the sentence generalises over: which catalog the
//! application lives in, how far its current revision has been advanced (so the
//! "current" value differs from row to row rather than always being the create's), and
//! the form of the revision the request names (a revision the application has left
//! behind, one below every revision it ever had, one far above, and one that is not a
//! number at all). Each row is driven through BOTH operations that make the check, and
//! their two messages are compared with each other as well as with the rule.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    revision_conflict_message, SEEDED_CATALOGS,
};

use crate::amend_benefit_application::message_of;
use crate::benefit_application_lifecycle::{error_code, text, Harness};

/// Builds one request of a revision-checking operation, from the catalog, the application
/// id, the revision the request names and its `ClientToken`.
type RequestBuilder = fn(&str, &str, &str, &str) -> Value;

/// The two operations that check the revision a request names against the stored one.
/// Each entry is the operation name and a builder for a request of that operation that
/// changes the application's `Name`.
const REVISION_CHECKERS: &[(&str, RequestBuilder)] = &[
    ("UpdateBenefitApplication", update_body),
    ("AmendBenefitApplication", amend_body),
];

/// An `UpdateBenefitApplication` request that renames the application.
fn update_body(catalog: &str, id: &str, revision: &str, token: &str) -> Value {
    json!({
        "Catalog": catalog,
        "ClientToken": token,
        "Identifier": id,
        "Revision": revision,
        "Name": "should-not-apply",
    })
}

/// An `AmendBenefitApplication` request that renames the application.
fn amend_body(catalog: &str, id: &str, revision: &str, token: &str) -> Value {
    json!({
        "Catalog": catalog,
        "ClientToken": token,
        "Identifier": id,
        "Revision": revision,
        "AmendmentReason": "the partner corrected the figures",
        "Amendments": [{ "FieldPath": "Name", "NewValue": "should-not-apply" }],
    })
}

/// Create one `PENDING_SUBMISSION` application in `catalog` and return its id and the
/// revision the create reported.
async fn seed(harness: &Harness, case: &str, catalog: &str, token: &str) -> (String, String) {
    let benefit_id = text(case, &harness.active_benefit_of(catalog).await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": catalog,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
            }),
        )
        .await;
    (text(case, &created, "Id"), text(case, &created, "Revision"))
}

/// Advance the application's revision `bumps` times with accepted updates, and return the
/// revision it is left at.
async fn advance(
    harness: &Harness,
    case: &str,
    catalog: &str,
    id: &str,
    from: &str,
    bumps: usize,
) -> String {
    let mut revision = from.to_string();
    for bump in 0..bumps {
        let out = harness
            .ok(
                case,
                "UpdateBenefitApplication",
                &json!({
                    "Catalog": catalog,
                    "ClientToken": format!("tok-revision-bump-{id}-{bump}"),
                    "Identifier": id,
                    "Revision": revision,
                    "Description": format!("bumped {bump} times"),
                }),
            )
            .await;
        revision = text(case, &out, "Revision");
    }
    revision
}

/// Every operation that checks the revision refuses one it is not at with the same 409
/// `ConflictException` sentence, naming the application, the current revision and the
/// requested one.
#[tokio::test]
async fn every_revision_refusal_reports_the_current_and_the_requested_revision() {
    let harness = Harness::new();

    for (row, catalog) in SEEDED_CATALOGS.iter().enumerate() {
        // One extra accepted update per catalog, so the current revision the refusals
        // have to quote is not the same value on every row.
        let bumps = row + 1;
        let case = format!("{catalog}, {bumps} accepted updates");
        let (id, created_revision) =
            seed(&harness, &case, catalog, &format!("tok-revision-seed-{row}")).await;
        let current = advance(&harness, &case, catalog, &id, &created_revision, bumps).await;
        let before = harness.get_in(&case, catalog, &id).await;

        for (attempt, requested) in [
            created_revision.clone(),
            "0".to_string(),
            "99999".to_string(),
            "not-a-revision".to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let mut messages: Vec<(&str, String)> = Vec::new();
            for (operation, build) in REVISION_CHECKERS {
                let case = format!("{case}, {operation} naming revision {requested:?}");
                let token = format!("tok-revision-stale-{row}-{attempt}-{operation}");
                let (status, out) = harness
                    .call(operation, &build(catalog, &id, &requested, &token))
                    .await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected 409, got {status} {out:?}"
                );
                assert_eq!(
                    error_code(&out),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {out:?}"
                );
                let message = message_of(&out);
                assert_eq!(
                    message,
                    revision_conflict_message(&id, &current, &requested),
                    "case {case:?}: expected the shared revision-conflict sentence, got {message:?}"
                );
                let after = harness.get_in(&case, catalog, &id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused write must change nothing, got {after:?}"
                );
                messages.push((operation, message));
            }

            let (first_operation, first) = &messages[0];
            for (operation, message) in &messages[1..] {
                assert_eq!(
                    message, first,
                    "case {case:?}: {operation} and {first_operation} must refuse revision \
                     {requested:?} in the same words, got {message:?} and {first:?}"
                );
            }
        }
    }
}

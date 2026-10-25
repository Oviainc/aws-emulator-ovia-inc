// SPDX-License-Identifier: CC0-1.0

//! `CancelBenefitApplication`'s `ClientToken`, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: a `ClientToken` identifies the REQUEST, so a cancellation
//! repeating the token of a cancellation that already applied is that same request — it
//! answers success and applies nothing a second time, ahead of the status gate the first
//! call's own effect would otherwise fail — while any OTHER cancellation of an
//! application AWS has finished with is still the modelled `ConflictException`.
//!
//! `tests/all/client_token_idempotency.rs` states that rule over every write that carries
//! a token, one row per operation; these tables span what it generalises over FOR A
//! CANCELLATION: the status the cancellation applied from (every non-terminal
//! `BenefitApplicationStatus`), whether the request carried a `Reason`, the form and
//! length of the token (each published bound and one past it, and a form the pattern
//! refuses), how many replays arrive, and which application the token was spent on.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::BENEFIT_APPLICATION_STATUS_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    MAX_CLIENT_TOKEN_LEN, STATUS_PENDING_SUBMISSION, TERMINAL_STATUSES,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};

/// How many times a cancellation is replayed: a retried SDK call can arrive more than
/// twice, and the answer must not drift between replays.
const REPLAYS: usize = 3;

/// Create one application to cancel, and return its id.
async fn seed(harness: &Harness, case: &str, token: &str) -> String {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
            }),
        )
        .await;
    text(case, &created, "Id")
}

/// One `CancelBenefitApplication` request body; `Reason` is optional, so a row without
/// one omits the member entirely.
fn cancel_body(id: &str, token: &str, reason: Option<&str>) -> Value {
    let mut body = json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "Identifier": id,
    });
    if let Some(reason) = reason {
        body["Reason"] = json!(reason);
    }
    body
}

/// A cancellation replayed under its own `ClientToken` answers success and leaves the
/// record exactly as the first cancellation left it, from every status an application can
/// be cancelled from — while a cancellation under a token that applied nothing is still
/// refused with the modelled `ConflictException`.
///
/// The rows are every `BenefitApplicationStatus` that is not terminal
/// (`BENEFIT_APPLICATION_STATUS_VALUES` minus `TERMINAL_STATUSES`, so the table cannot
/// drift from the model's enum), each one cancelled with and without a `Reason`.
#[tokio::test]
async fn a_replayed_cancel_token_changes_nothing_from_every_cancellable_status() {
    let cancellable: Vec<&str> = BENEFIT_APPLICATION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !TERMINAL_STATUSES.contains(status))
        .collect();
    assert!(
        !cancellable.is_empty(),
        "the model names at least one non-terminal BenefitApplicationStatus, got {BENEFIT_APPLICATION_STATUS_VALUES:?}"
    );

    let harness = Harness::new();
    let mut row = 0usize;
    for status in cancellable {
        for reason in [Some("no longer needed"), None] {
            row += 1;
            let case = format!(
                "cancelled from {status}, {}",
                if reason.is_some() {
                    "with a reason"
                } else {
                    "without a reason"
                }
            );
            let id = seed(&harness, &case, &format!("seed-cancel-{row}")).await;
            // `ACTION_REQUIRED` and `IN_REVIEW` come from AWS's own review workflow, so
            // the status is placed in the store; the behaviour is still driven and
            // asserted over the wire.
            if status != STATUS_PENDING_SUBMISSION {
                harness.place_status(&case, CATALOG, &id, status);
            }

            let token = format!("retry-cancel-{row}");
            let body = cancel_body(&id, &token, reason);
            let first = harness.ok(&case, "CancelBenefitApplication", &body).await;
            assert_eq!(
                first,
                json!({}),
                "case {case:?}: CancelBenefitApplication models no output members, got {first:?}"
            );
            let after_first = harness.get(&case, &id).await;
            assert_eq!(
                after_first["Status"],
                json!("CANCELED"),
                "case {case:?}: the first cancellation must apply, got {after_first:?}"
            );

            for replay in 1..=REPLAYS {
                let (http, out) = harness.call("CancelBenefitApplication", &body).await;
                assert_eq!(
                    http, 200,
                    "case {case:?}: replay {replay} of the cancellation's own ClientToken must answer 200, got {http} {out:?}"
                );
                assert_eq!(
                    out, first,
                    "case {case:?}: replay {replay} must answer the first call's output {first:?}, got {out:?}"
                );
                let after_replay = harness.get(&case, &id).await;
                assert_eq!(
                    after_replay, after_first,
                    "case {case:?}: replay {replay} must change nothing, expected {after_first:?}, got {after_replay:?}"
                );
            }

            // A DIFFERENT cancellation — a token that applied nothing — is a new write of
            // an application AWS has finished with, so it is still refused.
            let fresh = cancel_body(&id, &format!("fresh-cancel-{row}"), reason);
            let (http, out) = harness.call("CancelBenefitApplication", &fresh).await;
            assert_eq!(
                http, 409,
                "case {case:?}: a cancellation under a token that applied nothing must be refused, got {http} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {out:?}"
            );
            let after_refusal = harness.get(&case, &id).await;
            assert_eq!(
                after_refusal, after_first,
                "case {case:?}: a refused cancellation must persist nothing, expected {after_first:?}, got {after_refusal:?}"
            );
        }
    }
}

/// The replay holds for every token form `CancelBenefitApplicationInput$ClientToken`
/// admits, and a form it does not admit is refused before anything is written — so the
/// application is still cancellable afterwards.
///
/// The rows span the published `@length(min: 1, max: 64)` — both bounds and one byte past
/// the maximum — and the `@pattern` `^[a-zA-Z0-9-]{1,64}$`, including a form of
/// punctuation the pattern refuses
/// (`smithy_query(query="operation",operation="CancelBenefitApplication")`).
#[tokio::test]
async fn a_cancel_replay_holds_for_every_token_form_the_model_admits() {
    let longest = "t".repeat(MAX_CLIENT_TOKEN_LEN);
    let too_long = "t".repeat(MAX_CLIENT_TOKEN_LEN + 1);
    // (case, the token, whether the model admits it)
    let rows: [(&str, &str, bool); 5] = [
        ("the shortest token the model admits", "c", true),
        (
            "an SDK-style uuid token",
            "9ec1f0e4-2f6b-4a7d-8f0e-5b3c1d2a4e6f",
            true,
        ),
        ("the longest token the model admits", longest.as_str(), true),
        ("one byte past the longest token", too_long.as_str(), false),
        (
            "a token of punctuation the pattern refuses",
            "cancel tok!",
            false,
        ),
    ];

    let harness = Harness::new();
    for (row, (case, token, admitted)) in rows.iter().enumerate() {
        let id = seed(&harness, case, &format!("seed-form-{row}")).await;
        let before = harness.get(case, &id).await;
        let body = cancel_body(&id, token, Some("no longer needed"));

        if !admitted {
            let (http, out) = harness.call("CancelBenefitApplication", &body).await;
            assert_eq!(
                http, 400,
                "case {case:?}: a token the model does not admit must be refused, got {http} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
            let after = harness.get(case, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused cancellation must persist nothing, expected {before:?}, got {after:?}"
            );
            // Nothing was written, so the application is still cancellable under a token
            // the model does admit.
            let accepted = cancel_body(&id, &format!("after-refusal-{row}"), None);
            harness.ok(case, "CancelBenefitApplication", &accepted).await;
            let cancelled = harness.get(case, &id).await;
            assert_eq!(
                cancelled["Status"],
                json!("CANCELED"),
                "case {case:?}: a cancellation after the refusal must apply, got {cancelled:?}"
            );
            continue;
        }

        let first = harness.ok(case, "CancelBenefitApplication", &body).await;
        let after_first = harness.get(case, &id).await;
        assert_eq!(
            after_first["Status"],
            json!("CANCELED"),
            "case {case:?}: the first cancellation must apply, got {after_first:?}"
        );
        let (http, out) = harness.call("CancelBenefitApplication", &body).await;
        assert_eq!(
            http, 200,
            "case {case:?}: the replay must answer 200, got {http} {out:?}"
        );
        assert_eq!(
            out, first,
            "case {case:?}: the replay must answer the first call's output {first:?}, got {out:?}"
        );
        let after_replay = harness.get(case, &id).await;
        assert_eq!(
            after_replay, after_first,
            "case {case:?}: the replay must change nothing, expected {after_first:?}, got {after_replay:?}"
        );
    }
}

/// A `ClientToken` identifies ONE request, so the same token naming a DIFFERENT
/// application is a different cancellation: it applies to that application, and replaying
/// the first request still answers what it reported.
#[tokio::test]
async fn a_cancel_token_names_one_application() {
    let case = "one token, two applications";
    let harness = Harness::new();
    let first_id = seed(&harness, case, "seed-scope-1").await;
    let second_id = seed(&harness, case, "seed-scope-2").await;
    let shared = "shared-cancel-token";

    let first = harness
        .ok(
            case,
            "CancelBenefitApplication",
            &cancel_body(&first_id, shared, Some("no longer needed")),
        )
        .await;
    let first_after = harness.get(case, &first_id).await;

    // The second request names another application, so it is not a replay of the first:
    // it must cancel that application rather than answer as a retry of one that was never
    // cancelled.
    harness
        .ok(
            case,
            "CancelBenefitApplication",
            &cancel_body(&second_id, shared, Some("no longer needed")),
        )
        .await;
    let second_after = harness.get(case, &second_id).await;
    assert_eq!(
        second_after["Status"],
        json!("CANCELED"),
        "case {case:?}: a token naming another application is another request, so the cancellation must apply, got {second_after:?}"
    );

    // And the first request is still replayable, unaffected by the second.
    let (http, out) = harness
        .call(
            "CancelBenefitApplication",
            &cancel_body(&first_id, shared, Some("no longer needed")),
        )
        .await;
    assert_eq!(
        http, 200,
        "case {case:?}: the first application's replay must answer 200, got {http} {out:?}"
    );
    assert_eq!(
        out, first,
        "case {case:?}: the replay must answer the first call's output {first:?}, got {out:?}"
    );
    let first_replayed = harness.get(case, &first_id).await;
    assert_eq!(
        first_replayed, first_after,
        "case {case:?}: the replay must change nothing, expected {first_after:?}, got {first_replayed:?}"
    );
}

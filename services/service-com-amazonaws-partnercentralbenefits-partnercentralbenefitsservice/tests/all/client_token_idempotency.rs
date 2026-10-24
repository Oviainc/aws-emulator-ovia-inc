// SPDX-License-Identifier: CC0-1.0

//! `ClientToken` idempotency, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: a `ClientToken` identifies the REQUEST, so a request repeating
//! the token of a write that already applied is that same request — it answers what the
//! first call reported and applies nothing a second time, ahead of the status and
//! optimistic-concurrency gates the first call's own effect would otherwise fail.
//!
//! The tests are tables. Their rows differ along what the rule generalises over: WHICH
//! `ClientToken`-carrying write is replayed (`UpdateBenefitApplication`,
//! `AmendBenefitApplication`, `CancelBenefitApplication`, `RecallBenefitApplication`),
//! which members the write carried, the FORM and LENGTH of the token (each published
//! bound and one past it), how many times the request is replayed, what happened to the
//! application in between, and which operation the token was spent on.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::MAX_CLIENT_TOKEN_LEN;

use crate::benefit_application_lifecycle::{error_code, instant, text, Harness, CATALOG};

/// How many times a request is replayed: a retried SDK call can arrive more than twice,
/// and the answer must not drift between replays.
const REPLAYS: usize = 3;

/// Build one request body for the replayed write, from the application it names, that
/// application's current `Revision`, and the `ClientToken` under test. A write that binds
/// no `Revision` member ignores the revision it is handed.
type BodyOf = fn(&str, &str, &str) -> Value;

/// Create one application to write to, and return its create response.
async fn seed(harness: &Harness, case: &str, token: &str) -> Value {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
                "Description": "demo app",
                "BenefitApplicationDetails": { "CampaignName": "spring" },
            }),
        )
        .await
}

/// The application's current `Revision`, read back through `GetBenefitApplication`.
async fn current_revision(harness: &Harness, case: &str, id: &str) -> String {
    text(case, &harness.get(case, id).await, "Revision")
}

/// Every write that carries a `ClientToken` answers a replay of it with the first call's
/// output and applies nothing a second time: the stored application is left exactly as
/// the first call left it, however many replays arrive.
#[tokio::test]
async fn a_replayed_client_token_applies_nothing_a_second_time() {
    // (case, operation, submit the application first, the request body)
    let rows: [(&str, &str, bool, BodyOf); 6] = [
        (
            "an update of the name",
            "UpdateBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "Name": "demo-2",
                })
            },
        ),
        (
            "an update of every updatable member at once",
            "UpdateBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "Name": "demo-3",
                    "Description": "a longer description",
                    "BenefitApplicationDetails": { "CampaignName": "summer", "RequestedAmount": 10 },
                    "PartnerContacts": [{ "Email": "new@example.com", "FirstName": "Grace" }],
                    "FileDetails": [{ "FileURI": "s3://b/updated.csv" }],
                })
            },
        ),
        (
            "an amendment",
            "AmendBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "AmendmentReason": "a correction",
                    "Amendments": [{ "FieldPath": "Name", "NewValue": "demo-amended" }],
                })
            },
        ),
        (
            "an amendment of the details document",
            "AmendBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "AmendmentReason": "a deeper correction",
                    "Amendments": [{
                        "FieldPath": "BenefitApplicationDetails.CampaignName",
                        "NewValue": "autumn",
                    }],
                })
            },
        ),
        (
            "a cancellation",
            "CancelBenefitApplication",
            false,
            |id, _revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Reason": "no longer needed",
                })
            },
        ),
        (
            "a recall",
            "RecallBenefitApplication",
            true,
            |id, _revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Reason": "more detail needed",
                })
            },
        ),
    ];

    let harness = Harness::new();
    for (index, (case, operation, submit_first, body_of)) in rows.iter().enumerate() {
        let created = seed(&harness, case, &format!("seed-{index}")).await;
        let id = text(case, &created, "Id");
        if *submit_first {
            harness
                .ok(
                    case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": CATALOG, "Identifier": id }),
                )
                .await;
        }

        let revision = current_revision(&harness, case, &id).await;
        let token = format!("retry-{index}");
        let body = body_of(&id, &revision, &token);
        let first = harness.ok(case, operation, &body).await;
        let after_first = harness.get(case, &id).await;

        for replay in 1..=REPLAYS {
            let (status, out) = harness.call(operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: replay {replay} of the same ClientToken must answer 200, got {status} {out:?}"
            );
            assert_eq!(
                out, first,
                "case {case:?}: replay {replay} must answer the first call's output {first:?}, got {out:?}"
            );
            let after_replay = harness.get(case, &id).await;
            assert_eq!(
                after_replay, after_first,
                "case {case:?}: replay {replay} must change nothing, expected {after_first:?}, got {after_replay:?}"
            );
        }
    }
}

/// An update's replay answers the first call's `Id`, `Arn` and `Revision` for every token
/// form the modelled `ClientToken` admits, and goes on answering it after the application
/// has moved past the state the update was accepted in.
///
/// The token rows span the published `@length(min: 1, max: 64)` of
/// `UpdateBenefitApplicationInput$ClientToken` — both bounds and one past the maximum —
/// and the character classes its `@pattern` `^[!-~]{1,64}$` admits
/// (`smithy_query(query="operation",operation="UpdateBenefitApplication")`).
#[tokio::test]
async fn an_update_replays_its_first_result_for_every_token_form() {
    let longest = "t".repeat(MAX_CLIENT_TOKEN_LEN);
    let too_long = "t".repeat(MAX_CLIENT_TOKEN_LEN + 1);
    let rows: [(&str, &str); 5] = [
        ("the shortest token the model admits", "u"),
        ("an SDK-style uuid token", "9ec1f0e4-2f6b-4a7d-8f0e-5b3c1d2a4e6f"),
        ("a token of punctuation the pattern admits", "retry!2026-10-24T02:08:42Z/1"),
        ("the longest token the model admits", longest.as_str()),
        ("one byte past the longest token", too_long.as_str()),
    ];

    let harness = Harness::new();
    for (index, (case, token)) in rows.iter().enumerate() {
        let created = seed(&harness, case, &format!("seed-token-{index}")).await;
        let id = text(case, &created, "Id");
        let revision = text(case, &created, "Revision");
        let body = json!({
            "Catalog": CATALOG,
            "ClientToken": token,
            "Identifier": id,
            "Revision": revision,
            "Name": format!("demo-{index}"),
        });

        // A token past the modelled maximum is refused before any of this, and nothing
        // is written for it to replay.
        if token.len() > MAX_CLIENT_TOKEN_LEN {
            let (status, out) = harness.call("UpdateBenefitApplication", &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a token past the modelled maximum must be refused, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
            let after = harness.get(case, &id).await;
            assert_eq!(
                text(case, &after, "Revision"),
                revision,
                "case {case:?}: a refused update must not bump the revision, got {after:?}"
            );
            continue;
        }

        let first = harness.ok(case, "UpdateBenefitApplication", &body).await;
        let after_first = harness.get(case, &id).await;
        assert_ne!(
            text(case, &first, "Revision"),
            revision,
            "case {case:?}: the first update must bump the revision, got {first:?}"
        );

        let replay = harness.ok(case, "UpdateBenefitApplication", &body).await;
        assert_eq!(
            replay, first,
            "case {case:?}: a replayed update must answer the first response {first:?}, got {replay:?}"
        );
        let after_replay = harness.get(case, &id).await;
        assert_eq!(
            instant(case, &after_replay, "UpdatedAt"),
            instant(case, &after_first, "UpdatedAt"),
            "case {case:?}: a replayed update must not advance UpdatedAt, got {after_replay:?}"
        );

        // The application moves on — a submission takes it out of the status an update is
        // accepted in — and the retry still answers what the first update reported.
        harness
            .ok(
                case,
                "SubmitBenefitApplication",
                &json!({ "Catalog": CATALOG, "Identifier": id }),
            )
            .await;
        let late = harness.ok(case, "UpdateBenefitApplication", &body).await;
        assert_eq!(
            late, first,
            "case {case:?}: a retry after the application moved on must still answer {first:?}, got {late:?}"
        );

        // A token that applied nothing is not a replay: the same stale revision is still
        // refused, and the submitted application is still not updatable.
        let mut fresh = body.clone();
        fresh["ClientToken"] = json!(format!("fresh-{index}"));
        let (status, out) = harness.call("UpdateBenefitApplication", &fresh).await;
        assert_eq!(
            status, 409,
            "case {case:?}: an update under a token that applied nothing must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );
    }
}

/// A `ClientToken` identifies one request, so it is not a replay of a DIFFERENT
/// operation's request, and it is case-sensitive: a token differing only in case is
/// another request and applies another write.
#[tokio::test]
async fn a_client_token_is_scoped_to_its_operation_and_is_case_sensitive() {
    let harness = Harness::new();

    // The same token spent on another operation is another request: the cancellation
    // below must actually cancel, rather than replaying the update that holds the token.
    let created = seed(&harness, "shared token", "seed-shared").await;
    let id = text("shared token", &created, "Id");
    let shared = "shared-token";
    harness
        .ok(
            "shared token",
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": shared,
                "Identifier": id,
                "Revision": text("shared token", &created, "Revision"),
                "Name": "demo-updated",
            }),
        )
        .await;
    harness
        .ok(
            "shared token",
            "CancelBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": shared,
                "Identifier": id,
                "Reason": "cancelled under the update's token",
            }),
        )
        .await;
    let cancelled = harness.get("shared token", &id).await;
    assert_eq!(
        cancelled["Status"],
        json!("CANCELED"),
        "case \"shared token\": a token spent on another operation is not a replay, so the cancel must apply, got {cancelled:?}"
    );

    // A token differing only in case is a different request, so it applies a second
    // write of its own.
    let created = seed(&harness, "token case", "seed-case").await;
    let id = text("token case", &created, "Id");
    let first = harness
        .ok(
            "token case",
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "Case-Token",
                "Identifier": id,
                "Revision": text("token case", &created, "Revision"),
                "Name": "upper-token",
            }),
        )
        .await;
    let second = harness
        .ok(
            "token case",
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "case-token",
                "Identifier": id,
                "Revision": text("token case", &first, "Revision"),
                "Name": "lower-token",
            }),
        )
        .await;
    assert_ne!(
        text("token case", &second, "Revision"),
        text("token case", &first, "Revision"),
        "case \"token case\": ClientToken is case-sensitive, so the second update must apply, got {second:?}"
    );
    let after = harness.get("token case", &id).await;
    assert_eq!(
        after["Name"],
        json!("lower-token"),
        "case \"token case\": the second update's Name must be stored, got {after:?}"
    );
}

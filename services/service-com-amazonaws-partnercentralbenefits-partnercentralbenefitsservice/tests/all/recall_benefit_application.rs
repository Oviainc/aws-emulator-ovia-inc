// SPDX-License-Identifier: CC0-1.0

//! `RecallBenefitApplication`, end to end over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.RecallBenefitApplication` — and every effect is read
//! back through `GetBenefitApplication` and `ListBenefitApplications`, so what these
//! tests pin is the emulator's HTTP behaviour on the persisted record.
//!
//! The rule under test: a recall takes a SUBMITTED application that AWS has not finished
//! with back to draft (`PENDING_SUBMISSION`), records the required `Reason` as its
//! `StatusReason` and advances its revision and `UpdatedAt`; from any other status it is
//! a `ConflictException`, for an identifier naming no application a
//! `ResourceNotFoundException`, and a retry carrying the `ClientToken` of the recall
//! still in effect changes nothing.
//!
//! The tests are tables whose rows differ along what that rule generalises over: which
//! catalog the application lives in, which form its `Identifier` is given in, whether a
//! `ClientToken` is supplied, the shape of the `Reason`, and which status the application
//! is recalled from.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::BENEFIT_APPLICATION_STATUS_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    RECALLABLE_STATUSES, SEEDED_CATALOGS, STATUS_ACTION_REQUIRED, STATUS_CANCELED,
    STATUS_IN_REVIEW, STATUS_PENDING_SUBMISSION,
};

use crate::benefit_application_lifecycle::{
    error_code, instant, text, Harness, ACCOUNT, CATALOG, REGION,
};

/// Which form of the modelled `BenefitApplicationIdentifier`
/// (`^(arn:.+|benappl-[0-9a-z]{14})$`) a row names the application by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentifierForm {
    BareId,
    Arn,
}

/// The id of the first `ACTIVE` benefit of `catalog` — the benefit every application
/// below applies for.
async fn active_benefit_id(harness: &Harness, catalog: &str) -> String {
    text(catalog, &harness.active_benefit_of(catalog).await, "Id")
}

/// Read one application of any catalog back, asserting the read succeeded.
async fn read(harness: &Harness, case: &str, catalog: &str, identifier: &str) -> Value {
    harness.get_in(case, catalog, identifier).await
}

/// Create one application in `catalog` and return `(id, arn)`.
async fn seed(harness: &Harness, case: &str, catalog: &str, token: &str) -> (String, String) {
    let benefit_id = active_benefit_id(harness, catalog).await;
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": catalog,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "recall-subject",
            }),
        )
        .await;
    (text(case, &created, "Id"), text(case, &created, "Arn"))
}

/// Submit one application, which must succeed and leave it `IN_REVIEW`.
async fn submit(harness: &Harness, case: &str, catalog: &str, identifier: &str) {
    harness
        .ok(
            case,
            "SubmitBenefitApplication",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await;
    let submitted = read(harness, case, catalog, identifier).await;
    assert_eq!(
        submitted["Status"],
        json!(STATUS_IN_REVIEW),
        "case {case:?}: a submitted application must be {STATUS_IN_REVIEW}, got {submitted:?}"
    );
}

/// One recall request body.
fn recall_body(catalog: &str, identifier: &str, reason: &str, token: Option<&str>) -> Value {
    let mut body = json!({
        "Catalog": catalog,
        "Identifier": identifier,
        "Reason": reason,
    });
    if let Some(token) = token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// A recall returns a submitted application to draft, records its `Reason` as the
/// `StatusReason`, and advances the revision and `UpdatedAt` — whichever catalog it lives
/// in, whichever identifier form names it, with or without a `ClientToken`.
///
/// The rows are every seeded catalog (`SEEDED_CATALOGS`, so the set cannot drift from the
/// store) crossed with both identifier forms the modelled pattern admits and both
/// `ClientToken` states, each with a differently shaped `Reason`.
#[tokio::test]
async fn recall_returns_a_submitted_application_to_draft() {
    let harness = Harness::new();

    // The `Reason` member is an unconstrained `String`, so the rows span its shapes: one
    // character, a sentence, non-ASCII text, and a long reason.
    let reasons: [&str; 4] = [
        "x",
        "the partner found an error in the submitted figures",
        "montant incorrect — données à revoir ✓",
        "needs rework: the attached plan is the wrong revision and the requested amount \
         has to be recalculated before AWS reviews it again",
    ];

    let mut row = 0usize;
    for catalog in SEEDED_CATALOGS {
        for form in [IdentifierForm::BareId, IdentifierForm::Arn] {
            for token in [Some("tok-recall"), None] {
                let case = format!("{catalog}, {form:?}, token {token:?}");
                let reason = reasons[row % reasons.len()];
                let token = token.map(|base| format!("{base}-{row}"));
                row += 1;

                let (id, arn) = seed(&harness, &case, catalog, &format!("tok-seed-{row}")).await;
                let identifier = match form {
                    IdentifierForm::BareId => id.clone(),
                    IdentifierForm::Arn => arn.clone(),
                };
                submit(&harness, &case, catalog, &identifier).await;
                let before = read(&harness, &case, catalog, &identifier).await;

                harness
                    .ok(
                        &case,
                        "RecallBenefitApplication",
                        &recall_body(catalog, &identifier, reason, token.as_deref()),
                    )
                    .await;

                let after = read(&harness, &case, catalog, &identifier).await;
                assert_eq!(
                    after["Status"],
                    json!(STATUS_PENDING_SUBMISSION),
                    "case {case:?}: a recalled application must be {STATUS_PENDING_SUBMISSION}, got {after:?}"
                );
                assert_eq!(
                    after["StatusReason"],
                    json!(reason),
                    "case {case:?}: the recall reason must be stored as StatusReason, got {after:?}"
                );
                assert_ne!(
                    text(&case, &after, "Revision"),
                    text(&case, &before, "Revision"),
                    "case {case:?}: a recall must advance the revision, got {after:?}"
                );
                assert!(
                    instant(&case, &after, "UpdatedAt") > instant(&case, &before, "UpdatedAt"),
                    "case {case:?}: a recall must advance UpdatedAt, got {after:?}"
                );
                assert_eq!(
                    after["Id"],
                    json!(id),
                    "case {case:?}: a recall must not move the application, got {after:?}"
                );
                assert_eq!(
                    after["CreatedAt"], before["CreatedAt"],
                    "case {case:?}: a recall must not change CreatedAt, got {after:?}"
                );

                // The transition is observable through the listing, not only the read.
                let listed = harness
                    .ok(
                        &case,
                        "ListBenefitApplications",
                        &json!({ "Catalog": catalog, "Status": [STATUS_PENDING_SUBMISSION] }),
                    )
                    .await;
                let ids: Vec<String> = listed["BenefitApplicationSummaries"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|summary| text(&case, summary, "Id"))
                    .collect();
                assert!(
                    ids.contains(&id),
                    "case {case:?}: the {STATUS_PENDING_SUBMISSION} filter must report the recalled application {id:?}, got {listed:?}"
                );

                // Draft again means submittable again: the cycle repeats, and the second
                // recall replaces the first one's StatusReason.
                submit(&harness, &case, catalog, &identifier).await;
                let resubmitted = read(&harness, &case, catalog, &identifier).await;
                harness
                    .ok(
                        &case,
                        "RecallBenefitApplication",
                        &recall_body(
                            catalog,
                            &identifier,
                            "recalled a second time",
                            Some(&format!("tok-recall-again-{row}")),
                        ),
                    )
                    .await;
                let twice = read(&harness, &case, catalog, &identifier).await;
                assert_eq!(
                    twice["Status"],
                    json!(STATUS_PENDING_SUBMISSION),
                    "case {case:?}: a second recall must return the application to draft, got {twice:?}"
                );
                assert_eq!(
                    twice["StatusReason"],
                    json!("recalled a second time"),
                    "case {case:?}: the second recall's reason must replace the first's, got {twice:?}"
                );
                assert!(
                    instant(&case, &twice, "UpdatedAt")
                        >= instant(&case, &resubmitted, "UpdatedAt"),
                    "case {case:?}: a second recall must not move UpdatedAt backwards, got {twice:?}"
                );
            }
        }
    }
}

/// A recall is refused from every status that is not a submitted, non-terminal one, and
/// the refused request changes nothing.
///
/// The reachable rows are the statuses this emulator's own operations produce: a
/// never-submitted application, one already recalled (with and without a token), and a
/// cancelled one. The modelled set itself is checked against
/// `BENEFIT_APPLICATION_STATUS_VALUES` so the rule covers the statuses AWS's review
/// workflow produces and no operation here can reach.
#[tokio::test]
async fn recall_is_refused_from_a_status_that_is_not_submitted() {
    // The recallable set, from the operation's own documentation ("Recalls a SUBMITTED
    // benefit application, returning it to draft status") over the six values of
    // `BenefitApplicationStatus` (`BENEFIT_APPLICATION_STATUS_VALUES`): a submitted
    // application is one AWS still holds, so `ACTION_REQUIRED` and `IN_REVIEW` are
    // recallable while `PENDING_SUBMISSION` (never submitted) and the terminal
    // `APPROVED`, `REJECTED`, `CANCELED` are not.
    let recallable: [&str; 2] = [STATUS_ACTION_REQUIRED, STATUS_IN_REVIEW];
    for status in BENEFIT_APPLICATION_STATUS_VALUES {
        assert_eq!(
            RECALLABLE_STATUSES.contains(status),
            recallable.contains(status),
            "case {status:?}: expected recallable == {}, got {}",
            recallable.contains(status),
            RECALLABLE_STATUSES.contains(status)
        );
    }

    let harness = Harness::new();

    // (case, how many submits before the recall under test, whether a prior recall ran,
    //  whether the application is cancelled first)
    let rows: [(&str, bool, Option<&str>, bool); 4] = [
        ("never submitted", false, None, false),
        ("already recalled, with a token", true, Some("tok-prior"), false),
        ("already recalled, tokenless", true, None, false),
        ("cancelled", false, None, true),
    ];

    for (row, (case, submit_first, prior_recall_token, cancel_first)) in rows.iter().enumerate() {
        let (id, _) = seed(&harness, case, CATALOG, &format!("tok-conflict-seed-{row}")).await;
        if *submit_first {
            submit(&harness, case, CATALOG, &id).await;
            harness
                .ok(
                    case,
                    "RecallBenefitApplication",
                    &recall_body(CATALOG, &id, "the first recall", *prior_recall_token),
                )
                .await;
        }
        if *cancel_first {
            harness
                .ok(
                    case,
                    "CancelBenefitApplication",
                    &json!({
                        "Catalog": CATALOG,
                        "ClientToken": format!("tok-conflict-cancel-{row}"),
                        "Identifier": id,
                        "Reason": "withdrawn",
                    }),
                )
                .await;
        }
        let before = read(&harness, case, CATALOG, &id).await;

        let (status, out) = harness
            .call(
                "RecallBenefitApplication",
                &recall_body(
                    CATALOG,
                    &id,
                    "recall it anyway",
                    Some(&format!("tok-conflict-{row}")),
                ),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 recalling a {:?} application, got {status} {out:?}",
            before["Status"]
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );

        let after = read(&harness, case, CATALOG, &id).await;
        assert_eq!(
            after["Status"], before["Status"],
            "case {case:?}: a refused recall must not move the status, got {after:?}"
        );
        assert_eq!(
            after["Revision"], before["Revision"],
            "case {case:?}: a refused recall must not advance the revision, got {after:?}"
        );
        assert_eq!(
            after["StatusReason"], before["StatusReason"],
            "case {case:?}: a refused recall must not record its reason, got {after:?}"
        );
        assert_eq!(
            instant(case, &after, "UpdatedAt"),
            instant(case, &before, "UpdatedAt"),
            "case {case:?}: a refused recall must not advance UpdatedAt, got {after:?}"
        );
    }

    // The cancelled row above also pins the terminal direction: a cancel after a recall
    // is still accepted, and the recalled application is cancellable.
    let (id, _) = seed(&harness, "recalled then cancelled", CATALOG, "tok-terminal").await;
    submit(&harness, "recalled then cancelled", CATALOG, &id).await;
    harness
        .ok(
            "recalled then cancelled",
            "RecallBenefitApplication",
            &recall_body(CATALOG, &id, "pulled back", Some("tok-terminal-recall")),
        )
        .await;
    harness
        .ok(
            "recalled then cancelled",
            "CancelBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-terminal-cancel",
                "Identifier": id,
            }),
        )
        .await;
    let cancelled = read(&harness, "recalled then cancelled", CATALOG, &id).await;
    assert_eq!(
        cancelled["Status"],
        json!(STATUS_CANCELED),
        "case \"recalled then cancelled\": a recalled application must still be cancellable, got {cancelled:?}"
    );
}

/// An `Identifier` that satisfies the modelled pattern but names no application of the
/// catalog is a 404; one outside the pattern, and a `Catalog` outside its own pattern,
/// never reach the handler and are a 400.
#[tokio::test]
async fn recall_refuses_an_identifier_naming_no_application() {
    let harness = Harness::new();
    let (id, arn) = seed(&harness, "seed", CATALOG, "tok-missing-seed").await;
    submit(&harness, "seed", CATALOG, &id).await;

    // Each row differs from the stored identifier in exactly one field, so a 404 cannot
    // come from an identifier that happens to be malformed.
    let unknown: Vec<(&str, String, &str)> = vec![
        (
            "an id nothing was minted for",
            "benappl-00000000000000".to_string(),
            CATALOG,
        ),
        (
            "an ARN of another region",
            arn.replace(REGION, "eu-west-1"),
            CATALOG,
        ),
        (
            "an ARN of another account",
            arn.replace(ACCOUNT, "123456789012"),
            CATALOG,
        ),
        (
            "an ARN of another partition",
            arn.replace("arn:aws:", "arn:aws-cn:"),
            CATALOG,
        ),
        ("the right ARN in the wrong catalog", arn.clone(), "Sandbox"),
        ("the right id in the wrong catalog", id.clone(), "Sandbox"),
        (
            "a catalog the service does not serve",
            id.clone(),
            "not-a-real-catalog",
        ),
    ];

    for (case, identifier, catalog) in unknown {
        let (status, out) = harness
            .call(
                "RecallBenefitApplication",
                &recall_body(catalog, &identifier, "recall the unknown", Some("tok-missing")),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {identifier:?} in {catalog:?}, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {out:?}"
        );
    }

    // The submitted application is untouched by every refusal above.
    let still = read(&harness, "seed", CATALOG, &id).await;
    assert_eq!(
        still["Status"],
        json!(STATUS_IN_REVIEW),
        "case \"seed\": a refused recall must leave the real application submitted, got {still:?}"
    );

    // An identifier outside the modelled `BenefitApplicationIdentifier` pattern, and a
    // catalog outside `CatalogName`, are refused before the handler runs.
    let malformed: [(&str, Value); 5] = [
        (
            "identifier with punctuation",
            recall_body(CATALOG, "!!! not an identifier !!!", "why", None),
        ),
        (
            "identifier in the wrong case",
            recall_body(CATALOG, "benappl-SHOUTING00000", "why", None),
        ),
        ("empty identifier", recall_body(CATALOG, "", "why", None)),
        (
            "identifier that is only the ARN prefix",
            recall_body(CATALOG, "arn:", "why", None),
        ),
        (
            "catalog with a space",
            recall_body("bad catalog!", &id, "why", None),
        ),
    ];

    for (case, body) in malformed {
        let (status, out) = harness.call("RecallBenefitApplication", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {body:?}, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
    }

    // `Reason` is required, so a recall without one is refused the same way.
    let (status, out) = harness
        .call(
            "RecallBenefitApplication",
            &json!({ "Catalog": CATALOG, "Identifier": id }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"no reason\": expected 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"no reason\": expected ValidationException, got {out:?}"
    );
}

/// `ClientToken` makes the recall idempotent: replaying the request that performed the
/// recall still in effect answers success and changes nothing, while a token whose recall
/// is no longer in effect — because the record was submitted again or changed — is
/// treated as a new request.
#[tokio::test]
async fn recall_is_idempotent_in_its_client_token() {
    let harness = Harness::new();
    let (id, _) = seed(&harness, "seed", CATALOG, "tok-idem-seed").await;
    submit(&harness, "seed", CATALOG, &id).await;

    let body = recall_body(CATALOG, &id, "retry me", Some("tok-idem"));
    harness
        .ok("first recall", "RecallBenefitApplication", &body)
        .await;
    let once = read(&harness, "first recall", CATALOG, &id).await;

    // Replaying the identical request two more times is the same one recall.
    for attempt in 1..=2 {
        let case = format!("retry {attempt}");
        harness.ok(&case, "RecallBenefitApplication", &body).await;
        let again = read(&harness, &case, CATALOG, &id).await;
        assert_eq!(
            again, once,
            "case {case:?}: a replayed recall must change nothing, got {again:?}"
        );
    }

    // A DIFFERENT token is a different request, and the application is no longer
    // recallable, so it conflicts.
    let (status, out) = harness
        .call(
            "RecallBenefitApplication",
            &recall_body(CATALOG, &id, "retry me", Some("tok-idem-other")),
        )
        .await;
    assert_eq!(
        status, 409,
        "case \"another token\": expected 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case \"another token\": expected ConflictException, got {out:?}"
    );

    // Once the application is submitted again, the first token's recall is no longer in
    // effect, so replaying it performs a NEW recall rather than answering as a retry.
    submit(&harness, "resubmitted", CATALOG, &id).await;
    let resubmitted = read(&harness, "resubmitted", CATALOG, &id).await;
    harness
        .ok("token reused after a resubmit", "RecallBenefitApplication", &body)
        .await;
    let reused = read(&harness, "token reused after a resubmit", CATALOG, &id).await;
    assert_eq!(
        reused["Status"],
        json!(STATUS_PENDING_SUBMISSION),
        "case \"token reused after a resubmit\": the recall must run again, got {reused:?}"
    );
    assert_ne!(
        text("token reused after a resubmit", &reused, "Revision"),
        text("resubmitted", &resubmitted, "Revision"),
        "case \"token reused after a resubmit\": the new recall must advance the revision, got {reused:?}"
    );

    // And a change to the record closes the window too: after an update, the token names
    // a recall that no longer describes the stored record, so the replay is a new request
    // against a draft application — which is not recallable.
    let updated = harness
        .ok(
            "updated after the recall",
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-idem-update",
                "Identifier": id,
                "Revision": text("token reused after a resubmit", &reused, "Revision"),
                "Name": "reworked",
            }),
        )
        .await;
    let (status, out) = harness.call("RecallBenefitApplication", &body).await;
    assert_eq!(
        status, 409,
        "case \"replay after an update\": expected 409 once the record moved on, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case \"replay after an update\": expected ConflictException, got {out:?}"
    );
    let after_update = read(&harness, "replay after an update", CATALOG, &id).await;
    assert_eq!(
        after_update["Revision"],
        updated["Revision"],
        "case \"replay after an update\": the refused replay must leave the updated record alone, got {after_update:?}"
    );
}

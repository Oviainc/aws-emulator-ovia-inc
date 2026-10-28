// SPDX-License-Identifier: CC0-1.0

//! The bounds on the caller strings a write PERSISTS, end to end over the wire.
//!
//! Every assertion goes through the real request path (`MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies) and reads the effect back through `GetBenefitApplication`.
//!
//! The rule under test: a caller string a write operation persists is accepted only
//! within the length the model states for that value — the bound on the member it is
//! stored in and reported through — and one past it is refused 400 `ValidationException`
//! with nothing stored, whether or not the operation's OWN input member happens to carry
//! that `@length` in the model.
//!
//! The rows span what the rule generalises over: which string (the reason that becomes
//! `StatusReason`, the `ClientToken` the record keeps), which operation writes it
//! (`RecallBenefitApplication`, whose members the model leaves unconstrained, and
//! `CancelBenefitApplication`, whose `Reason` the model bounds), the position relative to
//! each published bound (at it, one past it, and at the minimum) and the character width
//! of the value (ASCII and multi-byte, since a `@length` counts characters).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    member_validation_exception, MAX_CLIENT_TOKEN_LEN, MAX_STATUS_REASON_LEN, STATUS_IN_REVIEW,
    STATUS_PENDING_SUBMISSION, VALIDATION_REASON_FIELD,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::recall_benefit_application::{recall_body, seed, submit};
use crate::service_error_rules::{assert_validation_invariants, message_of};

/// The requirement a value longer than `max` fails, as this service words a length.
fn too_long(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a value shorter than `min` fails, as this service words a length.
fn too_short(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// `count` repetitions of `unit`, so a row can name a length in CHARACTERS whatever the
/// width of the character it is built from.
fn repeat(unit: &str, count: usize) -> String {
    unit.repeat(count)
}

/// One application of `CATALOG`, submitted so it is recallable, as `(id, its reason)`.
async fn recallable(harness: &Harness, case: &str, token_seed: &str) -> String {
    let (id, _) = seed(harness, case, CATALOG, token_seed).await;
    submit(harness, case, CATALOG, &id).await;
    id
}

/// Assert `out` is this service's one-member `ValidationException` naming `member` and
/// `requirement`.
fn assert_member_refusal(case: &str, status: u16, out: &Value, member: &str, requirement: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 outside the bound on {member:?}, got {status} {out:?}"
    );
    assert_eq!(
        error_code(out),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {out:?}"
    );
    let message = message_of(out)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {out:?}"));
    assert_validation_invariants(case, &message);
    let expected = member_validation_exception(member, requirement, VALIDATION_REASON_FIELD);
    assert_eq!(
        message, expected.message,
        "case {case:?}: expected the message {:?}, got {message:?}",
        expected.message
    );
    assert_eq!(
        out["Reason"],
        json!(expected.reason),
        "case {case:?}: expected the reason {:?}, got {out:?}",
        expected.reason
    );
}

/// A reason a write records as `StatusReason` is accepted up to the bound of the member
/// it is reported through, and refused one past it — on the operation whose model bounds
/// it and on the operation whose model does not.
#[tokio::test]
async fn a_persisted_reason_is_bounded_by_the_member_it_is_reported_through() {
    let harness = Harness::new();

    // (case, the character the reason is built from) — one byte and several, because the
    // bound counts characters.
    let widths: [(&str, &str); 2] = [("ascii", "r"), ("multi-byte", "é")];

    // AT the bound: the recall applies and the read reports exactly what was sent.
    for (row, (width, unit)) in widths.iter().enumerate() {
        let case = format!("recall, {width}, at the bound");
        let id = recallable(&harness, &case, &format!("tok-reason-at-{row}")).await;
        let reason = repeat(unit, MAX_STATUS_REASON_LEN);
        harness
            .ok(
                &case,
                "RecallBenefitApplication",
                &recall_body(CATALOG, &id, &reason, Some(&format!("tok-reason-{row}"))),
            )
            .await;
        let after = harness.get_in(&case, CATALOG, &id).await;
        assert_eq!(
            after["Status"],
            json!(STATUS_PENDING_SUBMISSION),
            "case {case:?}: a reason at the bound must be accepted, got {after:?}"
        );
        assert_eq!(
            after["StatusReason"],
            json!(reason),
            "case {case:?}: the reason at the bound must round-trip, got {after:?}"
        );
        let stored = text(&case, &after, "StatusReason").chars().count();
        assert_eq!(
            stored, MAX_STATUS_REASON_LEN,
            "case {case:?}: expected {MAX_STATUS_REASON_LEN} stored characters, got {stored}"
        );
    }

    // ONE PAST it: refused, and the record is left submitted with no reason recorded.
    for (row, (width, unit)) in widths.iter().enumerate() {
        let case = format!("recall, {width}, one past the bound");
        let id = recallable(&harness, &case, &format!("tok-reason-past-{row}")).await;
        let before = harness.get_in(&case, CATALOG, &id).await;
        let reason = repeat(unit, MAX_STATUS_REASON_LEN + 1);
        let (status, out) = harness
            .call(
                "RecallBenefitApplication",
                &recall_body(CATALOG, &id, &reason, Some(&format!("tok-reason-past-{row}"))),
            )
            .await;
        assert_member_refusal(&case, status, &out, "reason", &too_long(MAX_STATUS_REASON_LEN));
        let after = harness.get_in(&case, CATALOG, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused recall must change nothing, got {after:?}"
        );
        assert_eq!(
            after["Status"],
            json!(STATUS_IN_REVIEW),
            "case {case:?}: the application must still be submitted, got {after:?}"
        );
    }

    // The same class on the OTHER operation that writes `StatusReason`: its `Reason` is
    // modelled `@length(max: 1000)`, so the same over-long value is refused there too and
    // the record keeps no reason outside the bound either way.
    for (row, (width, unit)) in widths.iter().enumerate() {
        let case = format!("cancel, {width}, one past the bound");
        let id = recallable(&harness, &case, &format!("tok-cancel-past-{row}")).await;
        let before = harness.get_in(&case, CATALOG, &id).await;
        let (status, out) = harness
            .call(
                "CancelBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-cancel-past-tok-{row}"),
                    "Identifier": id,
                    "Reason": repeat(unit, MAX_STATUS_REASON_LEN + 1),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a reason past the bound, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
        let message = message_of(&out)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {out:?}"));
        assert_validation_invariants(&case, &message);
        assert!(
            message.contains(&too_long(MAX_STATUS_REASON_LEN)),
            "case {case:?}: expected the clause {:?}, got {message:?}",
            too_long(MAX_STATUS_REASON_LEN)
        );
        let after = harness.get_in(&case, CATALOG, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused cancellation must change nothing, got {after:?}"
        );
    }

    // And a reason at the bound IS accepted on that operation, so the rows above pin a
    // bound rather than a blanket refusal.
    let case = "cancel, at the bound";
    let id = recallable(&harness, case, "tok-cancel-at").await;
    let reason = repeat("c", MAX_STATUS_REASON_LEN);
    harness
        .ok(
            case,
            "CancelBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-cancel-at-tok",
                "Identifier": id,
                "Reason": reason,
            }),
        )
        .await;
    let after = harness.get_in(case, CATALOG, &id).await;
    assert_eq!(
        after["StatusReason"],
        json!(reason),
        "case {case:?}: a cancellation reason at the bound must round-trip, got {after:?}"
    );
}

/// The `ClientToken` a recall persists carries the length every `ClientToken` of this
/// service carries: at each bound the recall applies, outside them it is refused 400 and
/// stores nothing.
#[tokio::test]
async fn a_persisted_client_token_is_bounded_like_every_other_client_token() {
    let harness = Harness::new();

    // (case, the token, whether it is inside the bounds) — the shortest token, the
    // longest, a multi-byte token of the longest length (characters, not bytes), then
    // one past the maximum in each width and one below the minimum.
    let rows: Vec<(&str, String, bool)> = vec![
        ("the shortest token", repeat("t", 1), true),
        (
            "a token of exactly the longest length",
            repeat("t", MAX_CLIENT_TOKEN_LEN),
            true,
        ),
        (
            "a multi-byte token of the longest length",
            repeat("é", MAX_CLIENT_TOKEN_LEN),
            true,
        ),
        (
            "a token one character past its bound",
            repeat("t", MAX_CLIENT_TOKEN_LEN + 1),
            false,
        ),
        (
            "a multi-byte token one character past its bound",
            repeat("é", MAX_CLIENT_TOKEN_LEN + 1),
            false,
        ),
        ("a token far past its bound", repeat("t", 100_000), false),
        ("an empty token", String::new(), false),
    ];

    for (row, (case, token, inside)) in rows.iter().enumerate() {
        let id = recallable(&harness, case, &format!("tok-token-seed-{row}")).await;
        let before = harness.get_in(case, CATALOG, &id).await;
        let body = recall_body(CATALOG, &id, "bounded token", Some(token));

        if *inside {
            harness.ok(case, "RecallBenefitApplication", &body).await;
            let after = harness.get_in(case, CATALOG, &id).await;
            assert_eq!(
                after["Status"],
                json!(STATUS_PENDING_SUBMISSION),
                "case {case:?}: a token inside the bounds must be accepted, got {after:?}"
            );
            // The token was kept, so replaying the identical request is the same recall.
            harness
                .ok(
                    &format!("{case}, replayed"),
                    "RecallBenefitApplication",
                    &body,
                )
                .await;
            let replayed = harness.get_in(case, CATALOG, &id).await;
            assert_eq!(
                replayed, after,
                "case {case:?}: replaying the accepted token must change nothing, \
                 got {replayed:?}"
            );
            continue;
        }

        let requirement = if token.chars().count() < 1 {
            too_short(1)
        } else {
            too_long(MAX_CLIENT_TOKEN_LEN)
        };
        let (status, out) = harness.call("RecallBenefitApplication", &body).await;
        assert_member_refusal(case, status, &out, "clientToken", &requirement);
        let after = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused recall must change nothing, got {after:?}"
        );

        // The refusal stored no token either: the application is still recallable, and a
        // recall carrying a token inside the bounds applies.
        harness
            .ok(
                &format!("{case}, then a bounded token"),
                "RecallBenefitApplication",
                &recall_body(CATALOG, &id, "bounded token", Some(&format!("tok-ok-{row}"))),
            )
            .await;
        let recalled = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            recalled["Status"],
            json!(STATUS_PENDING_SUBMISSION),
            "case {case:?}: the application must still be recallable after the refusal, \
             got {recalled:?}"
        );
    }
}

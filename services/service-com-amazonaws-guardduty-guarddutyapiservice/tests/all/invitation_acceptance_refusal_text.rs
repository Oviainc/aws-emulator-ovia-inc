// SPDX-License-Identifier: CC0-1.0

//! The TEXT an invitation acceptance is refused with, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! an acceptance whose administrator account id and invitation id identify no
//! invitation pending for the account that owns the named detector is refused with
//! GuardDuty's ONE fixed input-parameter sentence — the same words whichever of the
//! two names the acceptance is bound under, whichever way the pair fails to resolve,
//! and with nothing of the request or of service state interpolated into it: neither
//! member name, neither value, and no statement of which lookup failed.
//!
//! The rows differ along the operation NAME, along the administrator account id
//! (across the account-id space and in each way a value can fail to be an account id),
//! along the invitation id's shape, and along WHY the pair resolves to nothing (no
//! invitation at all, the right administrator with the wrong invitation id, the wrong
//! administrator with the right invitation id, and a pair a previous acceptance has
//! already consumed).

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state::now_millis;

use super::client_rejections::UNUSABLE_INPUT_VALUE;
use super::detector_lifecycle::{call, enabled_detector, new_router, router_over, Router};
use super::negative_class_rejections::{assert_bad_request_message, seed_invitation};

/// The Region the shared harness signs its requests for.
const REGION: &str = "us-east-1";

/// How long a value has to be for "the message does not contain it" to mean the
/// message did not interpolate it: shorter values are substrings of ordinary English.
const DISTINCTIVE_LEN: usize = 4;

/// The two names GuardDuty binds the SAME acceptance under — "this operation is
/// deprecated, use AcceptAdministratorInvitation instead"
/// (`smithy_query(operation="AcceptInvitation")`) — as
/// `(path segment, administrator member)`. Source for the member spellings: the
/// `jsonName` of `MasterId` and of `AdministratorId`
/// (`smithy_query(operation="AcceptInvitation,AcceptAdministratorInvitation")`).
const ACCEPTANCES: [(&str, &str); 2] =
    [("master", "masterId"), ("administrator", "administratorId")];

/// One `(administrator account id, invitation id)` pair the store resolves to nothing,
/// with why it is in the table.
#[derive(Debug, Clone, Copy)]
struct Pair {
    why: &'static str,
    administrator_id: &'static str,
    invitation_id: &'static str,
}

impl Pair {
    /// How one row names itself in an assertion message: the acceptance under test,
    /// both of the row's values and why the row is in the table.
    fn label(&self, segment: &str, member: &str) -> String {
        let Self {
            why,
            administrator_id,
            invitation_id,
        } = self;
        format!(
            "{segment}: {member} {administrator_id:?} with invitationId {invitation_id:?} ({why})"
        )
    }
}

/// Pairs no invitation was ever issued for: well-formed account ids spanning the
/// account-id space, values that cannot be account ids at all, and invitation ids of
/// every shape a caller might send — including the empty string, which reaches the
/// handler because `InvitationId` is a plain String with no modelled constraint
/// (`smithy_query(operation="AcceptInvitation")`).
const UNISSUED: [Pair; 10] = [
    Pair {
        why: "the account id the GuardDuty docs use, with an id nobody issued",
        administrator_id: "111122223333",
        invitation_id: "sim2real-bogus-invitation-id",
    },
    Pair {
        why: "the low end of the account-id space",
        administrator_id: "000000000000",
        invitation_id: "0",
    },
    Pair {
        why: "the high end of the account-id space",
        administrator_id: "999999999999",
        invitation_id: "11111111-2222-3333-4444-555555555555",
    },
    Pair {
        why: "an invitation id shaped like one this service mints",
        administrator_id: "123456789012",
        invitation_id: "d4b94fc952d6912b8f3060768220e2e7",
    },
    Pair {
        why: "an empty invitation id",
        administrator_id: "123456789012",
        invitation_id: "",
    },
    Pair {
        why: "12 letters: not digits at all",
        administrator_id: "abcdefghijkl",
        invitation_id: "bogus",
    },
    Pair {
        why: "grouped with hyphens",
        administrator_id: "1111-2222-33",
        invitation_id: "bogus",
    },
    Pair {
        why: "11 digits: one short of an account id",
        administrator_id: "12345678901",
        invitation_id: "bogus",
    },
    Pair {
        why: "13 digits: one too many",
        administrator_id: "1234567890123",
        invitation_id: "bogus",
    },
    Pair {
        why: "an empty administrator account id",
        administrator_id: "",
        invitation_id: "bogus",
    },
];

/// A pair nothing issued is refused with the one fixed sentence, under either name,
/// and the refusal leaves the account with no administrator and its detector enabled.
#[tokio::test]
async fn an_unissued_pair_is_refused_with_the_fixed_input_parameter_sentence() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (segment, member) in ACCEPTANCES {
        for pair in UNISSUED {
            let case = pair.label(segment, member);
            let (status, body) = accept(
                &router,
                (segment, member),
                &detector_id,
                pair.administrator_id,
                pair.invitation_id,
            )
            .await;
            assert_fixed_refusal(&case, status, &body, &detector_id, pair);

            // The refusal established nothing: every member of the read's shape is
            // optional, so "no administrator" is their ABSENCE, through BOTH reads.
            for (read_segment, _) in ACCEPTANCES {
                let reported = administrator_account(&router, read_segment, &detector_id).await;
                assert_eq!(
                    reported["accountId"],
                    Value::Null,
                    "case {case}: a refused acceptance must leave no administrator, got {reported:?}"
                );
            }
            let status_reported = detector_status(&router, &detector_id).await;
            assert_eq!(
                status_reported,
                json!("ENABLED"),
                "case {case}: the detector must still read back enabled, got {status_reported:?}"
            );
        }
    }
}

/// The same sentence answers a pair that half resolves: the refusal is a lookup of the
/// PAIR, and it says no more about which half was wrong than it does about a pair
/// nothing issued.
#[tokio::test]
async fn a_half_matching_pair_is_refused_with_the_same_sentence() {
    // The administrator account id and invitation id actually pending, and the
    // mismatching partners each row pairs them with.
    let pending_administrator = "222233334444";
    let pending_invitation = "11111111-2222-3333-4444-555555555555";
    let mismatches = [
        Pair {
            why: "the pending administrator with an invitation id it never sent",
            administrator_id: pending_administrator,
            invitation_id: "not-the-invitation",
        },
        Pair {
            why: "the pending administrator with the empty invitation id",
            administrator_id: pending_administrator,
            invitation_id: "",
        },
        Pair {
            why: "another account with the pending invitation id",
            administrator_id: "333344445555",
            invitation_id: pending_invitation,
        },
        Pair {
            why: "a malformed account id with the pending invitation id",
            administrator_id: "not-an-account",
            invitation_id: pending_invitation,
        },
    ];

    for (segment, member) in ACCEPTANCES {
        for pair in mismatches {
            let case = pair.label(segment, member);
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = router_over(Arc::clone(&engine));
            let detector_id = enabled_detector(&router).await;
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                pending_administrator,
                pending_invitation,
                now_millis(),
            );

            let (status, body) = accept(
                &router,
                (segment, member),
                &detector_id,
                pair.administrator_id,
                pair.invitation_id,
            )
            .await;
            assert_fixed_refusal(&case, status, &body, &detector_id, pair);
            let reported = administrator_account(&router, segment, &detector_id).await;
            assert_eq!(
                reported["accountId"],
                Value::Null,
                "case {case}: a half-matching pair must establish nothing, got {reported:?}"
            );

            // And the invitation is still pending: the acceptance that DOES match it
            // succeeds right after the refusal, so the refusal above is a lookup and
            // not a constant.
            let (status, body) = accept(
                &router,
                (segment, member),
                &detector_id,
                pending_administrator,
                pending_invitation,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the matching pair expected 200, got {status} {body:?}"
            );
        }
    }
}

/// A pair a previous acceptance consumed is no longer pending, so the repeat is the
/// same fixed refusal — and the relationship the first call established is untouched.
#[tokio::test]
async fn a_consumed_pair_is_refused_with_the_same_sentence() {
    let cases = [
        ("111122223333", "d4b94fc952d6912b8f3060768220e2e7"),
        ("222233334444", "11111111-2222-3333-4444-555555555555"),
        ("333344445555", "i"),
    ];

    for (segment, member) in ACCEPTANCES {
        for (administrator_id, invitation_id) in cases {
            let pair = Pair {
                why: "a pair the first acceptance consumed",
                administrator_id,
                invitation_id,
            };
            let case = pair.label(segment, member);
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = router_over(Arc::clone(&engine));
            let detector_id = enabled_detector(&router).await;
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                administrator_id,
                invitation_id,
                now_millis(),
            );

            let (status, body) = accept(
                &router,
                (segment, member),
                &detector_id,
                administrator_id,
                invitation_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: accepting the pending invitation expected 200, got {status} {body:?}"
            );
            let established = administrator_account(&router, segment, &detector_id).await;

            let (status, body) = accept(
                &router,
                (segment, member),
                &detector_id,
                administrator_id,
                invitation_id,
            )
            .await;
            assert_fixed_refusal(&case, status, &body, &detector_id, pair);
            assert_eq!(
                administrator_account(&router, segment, &detector_id).await,
                established,
                "case {case}: a refused repeat must leave the relationship where it was"
            );
        }
    }
}

/// Assert one acceptance refusal: the envelope the whole service answers a client
/// rejection with — status, shape, class in both body places and the WHOLE message —
/// through the shared assertion the `negative_class_rejections` suite already defines,
/// plus the rule specific to this one: the message interpolates nothing, neither member
/// name, neither of the caller's values, nor the detector id.
fn assert_fixed_refusal(case: &str, status: u16, body: &Value, detector_id: &str, pair: Pair) {
    assert_bad_request_message(case, status, body, UNUSABLE_INPUT_VALUE, "");
    let message = body["message"].as_str().unwrap_or_default();
    for (what, value) in [
        ("the administrator member name", "masterId"),
        ("the administrator member name", "administratorId"),
        ("the invitation member name", "invitationId"),
        ("the administrator account id", pair.administrator_id),
        ("the invitation id", pair.invitation_id),
        ("the detector id", detector_id),
    ] {
        // A value too short to be distinctive occurs in ordinary English, so a
        // substring test says nothing about it; the whole-message assertion above
        // already pins those rows.
        assert!(
            value.len() < DISTINCTIVE_LEN || !message.contains(value),
            "case {case}: the refusal must interpolate nothing, but it carries {what} {value:?}: {message:?}"
        );
    }
}

/// One acceptance: `POST /detector/{DetectorId}/{segment}` carrying the administrator
/// member the operation names
/// (`smithy_query(operation="AcceptInvitation,AcceptAdministratorInvitation")`).
async fn accept(
    router: &Router,
    acceptance: (&str, &str),
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
) -> (u16, Value) {
    let (segment, member) = acceptance;
    let mut body = serde_json::Map::new();
    body.insert(member.to_string(), json!(administrator_id));
    body.insert("invitationId".to_string(), json!(invitation_id));
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{segment}"),
        Some(Value::Object(body)),
    )
    .await
}

/// The administrator relationship one of the two reads reports —
/// `GET /detector/{DetectorId}/{segment}`, whose output member is named for the
/// segment (`master` / `administrator`).
async fn administrator_account(router: &Router, segment: &str, detector_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{segment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading /{segment}: expected 200, got {status} {body:?}"
    );
    body[segment].clone()
}

/// The `Status` `GetDetector` reports for the detector — `GET /detector/{DetectorId}`.
async fn detector_status(router: &Router, detector_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "get-detector: expected 200, got {status} {body:?}"
    );
    body["status"].clone()
}

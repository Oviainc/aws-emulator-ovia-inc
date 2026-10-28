// SPDX-License-Identifier: CC0-1.0

//! The ISSUING half of the procurement-portal validation flow:
//! `SendProcurementPortalValidation` "initiates the validation process by issuing a
//! validation code that confirms ownership and connectivity of the configured procurement
//! portal endpoint" (`smithy_query(query="doc",
//! shape="SendProcurementPortalValidation")`), so issuing that code is the work the call
//! performs, and `VerifyProcurementPortalValidation` completes the flow on the code the
//! send issued.
//!
//! Every call goes through the same `MultiServiceRouter` the production router uses, with
//! the shared harness of `refused_procurement_portal_writes` rather than a copy of it. The
//! one thing read out of state is the issued CODE: the modelled response carries only the
//! ARN, because the real service delivers the code through the portal the preference
//! configures.
//!
//! The rules under test, each spanned by a table rather than one request:
//!
//! * a send against a preference that exists issues a code, in the six-decimal-digit form
//!   the flow's `Code` is quoted in, which the verify then accepts — over the portals the
//!   enum admits, at several configuration-version counts, and however many sends preceded
//!   it (a later send retires the code an earlier one issued);
//! * a `ClientToken` replay of a send answers with the ARN the first call returned and
//!   issues no second code, while any other send issues a fresh one;
//! * a completed validation can be started again: the next send issues a code of its own
//!   and the consumed one completes nothing;
//! * issuing a code writes no configuration, so no member a reader reports moves.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::state::ComAmazonawsInvoicingInvoicingState;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, call, emulator_with_engine, get_preference, minimal_create, ok,
    outstanding_code, PORTAL_NAMES,
};

/// How many decimal digits an issued validation code has: the form
/// `ComAmazonawsInvoicingInvoicingState::issue_portal_validation_code` mints and the
/// portal-validation flow's `Code` is quoted in.
const CODE_DIGITS: usize = 6;

/// Whether the validation of `arn` has been completed, per state — no modelled member
/// reports it.
fn is_verified(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> bool {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .get(arn)
        .and_then(|stored| stored.validation.as_ref())
        .is_some_and(|validation| validation.verified_at.is_some())
}

/// How many preferences state holds, so a refused send is shown to create none.
fn stored_preferences(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .len()
}

/// The `Version` a reader reports for a preference.
fn version(preference: &Value) -> i64 {
    preference
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a Version in {preference}"))
}

/// One send, asserting the modelled answer: HTTP 200 carrying the ARN it was asked about.
async fn send(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let body = ok(router, "SendProcurementPortalValidation", request).await;
    let answered = arn_of(&body);
    assert_eq!(
        Some(answered.as_str()),
        request
            .get("ProcurementPortalPreferenceArn")
            .and_then(Value::as_str),
        "case {case}: the send answers with the ARN it was asked about, got {body}"
    );
    answered
}

/// One row: a preference reached through a chosen number of successful configuration
/// writes, on a chosen portal, with a chosen number of sends before the code is submitted.
struct Row {
    label: &'static str,
    portal: &'static str,
    /// Successful configuration writes after the create, so the version the sends must
    /// leave alone is not the one a create happens to leave.
    writes_after_create: i64,
    /// How many times the validation is started before the outstanding code is submitted:
    /// a later send issues again and retires what the previous one issued.
    sends: usize,
}

/// The rows: every portal of the enum, at several version counts, with one send and with
/// several.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one send, straight after the create",
            portal: PORTAL_NAMES[0],
            writes_after_create: 0,
            sends: 1,
        },
        Row {
            label: "one send, after one configuration update",
            portal: PORTAL_NAMES[1],
            writes_after_create: 1,
            sends: 1,
        },
        Row {
            label: "two sends, after one configuration update",
            portal: PORTAL_NAMES[2],
            writes_after_create: 1,
            sends: 2,
        },
        Row {
            label: "three sends, after two configuration updates",
            portal: PORTAL_NAMES[3],
            writes_after_create: 2,
            sends: 3,
        },
        Row {
            label: "two sends, straight after the create",
            portal: PORTAL_NAMES[0],
            writes_after_create: 0,
            sends: 2,
        },
    ]
}

/// RULE: the call that starts a validation issues the validation code the flow runs on and
/// records it against the named preference as the outstanding one; the submission of that
/// code is what completes the validation, and a code an earlier start issued is retired by
/// the code the next one issues. Issuing writes no configuration, so nothing a reader
/// reports moves.
#[tokio::test]
async fn every_send_issues_a_code_the_verify_then_accepts() {
    let (router, engine) = emulator_with_engine();
    // Every code this test sees, so the rows are shown to get codes of their own rather
    // than one constant.
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (index, row) in rows().iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    row.portal,
                    &format!("BUYER-ISSUE-{index}"),
                    &format!("SUPPLIER-ISSUE-{index}"),
                ),
            )
            .await,
        );
        for round in 0..row.writes_after_create {
            ok(
                &router,
                "PutProcurementPortalPreference",
                &accepted_update_round(&arn, round),
            )
            .await;
        }
        let before = get_preference(&router, &arn).await;
        let expected_version = 1 + row.writes_after_create;
        assert_eq!(
            version(&before),
            expected_version,
            "case {}: the create and {} configuration writes leave Version \
             {expected_version}, got {before}",
            row.label,
            row.writes_after_create
        );

        // Nothing is outstanding before the first send, so no code could be submitted.
        assert_eq!(
            outstanding_code(&engine, &arn),
            None,
            "case {}: no code is outstanding before a send",
            row.label
        );

        let mut retired: Vec<String> = Vec::new();
        let mut outstanding = String::new();
        for attempt in 1..=row.sends {
            let case = format!("{} / send {attempt}", row.label);
            send(
                &router,
                &case,
                &json!({"ProcurementPortalPreferenceArn": arn}),
            )
            .await;
            let issued = outstanding_code(&engine, &arn)
                .unwrap_or_else(|| panic!("case {case}: the send must issue a code"));
            assert_eq!(
                issued.len(),
                CODE_DIGITS,
                "case {case}: an issued code is {CODE_DIGITS} decimal digits, got {issued:?}"
            );
            assert!(
                issued.chars().all(|digit| digit.is_ascii_digit()),
                "case {case}: an issued code is {CODE_DIGITS} decimal digits, got {issued:?}"
            );
            assert!(
                seen.insert(issued.clone()),
                "case {case}: each send issues a code of its own, got the repeated {issued:?}"
            );
            if !outstanding.is_empty() {
                retired.push(std::mem::take(&mut outstanding));
            }
            outstanding = issued;
        }

        // Issuing codes wrote no configuration: `Version` is "the version number of the
        // procurement portal preference configuration", and these calls wrote none.
        let after_sends = get_preference(&router, &arn).await;
        assert_eq!(
            after_sends, before,
            "case {}: issuing a code must leave every reported member as it was, got \
             {after_sends}",
            row.label
        );

        // A code an earlier send issued is no longer the outstanding one, so it completes
        // nothing.
        for stale in &retired {
            let (status, body) = call(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": stale}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {} / a retired code: expected HTTP 400, got {status} {body}",
                row.label
            );
            assert!(
                !is_verified(&engine, &arn),
                "case {} / a retired code: a refused submission must not complete the \
                 validation",
                row.label
            );
        }

        // The code the latest send issued is what completes the validation.
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": outstanding}),
        )
        .await;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {}: the verify answers with the ARN it validated, got {completed}",
            row.label
        );
        assert!(
            is_verified(&engine, &arn),
            "case {}: submitting the issued code completes the validation",
            row.label
        );
        let after_verify = get_preference(&router, &arn).await;
        assert_eq!(
            after_verify, before,
            "case {}: completing a validation writes no configuration either, got \
             {after_verify}",
            row.label
        );
    }
}

/// RULE: under `@idempotency`, a replay of a request — the same `ClientToken` against the
/// same resource — answers with what the first call answered and performs its work once,
/// so the code that call issued stays the outstanding one; a request that is not a replay
/// issues again.
///
/// The rows differ along the token's shape, inside `BasicStringWithoutSpace`'s
/// `@length(0..1024)` and `^\S+$` (`smithy_query(query="operation",
/// operation="SendProcurementPortalValidation")`).
#[tokio::test]
async fn a_replayed_send_issues_no_second_code() {
    let (router, engine) = emulator_with_engine();

    let longest = "t".repeat(1024);
    for (index, token) in ["t", "a-client-token", "9f2b-4c1d", longest.as_str()]
        .into_iter()
        .enumerate()
    {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    PORTAL_NAMES[index % PORTAL_NAMES.len()],
                    &format!("BUYER-REPLAY-{index}"),
                    &format!("SUPPLIER-REPLAY-{index}"),
                ),
            )
            .await,
        );
        let request = json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": token});

        send(&router, &format!("token {token:?} first"), &request).await;
        let issued = outstanding_code(&engine, &arn)
            .unwrap_or_else(|| panic!("case token {token:?}: the send must issue a code"));

        // Replayed twice: the answer is the same and no further code is issued.
        for replay in 1..=2 {
            send(
                &router,
                &format!("token {token:?} replay {replay}"),
                &request,
            )
            .await;
            assert_eq!(
                outstanding_code(&engine, &arn).as_deref(),
                Some(issued.as_str()),
                "case token {token:?} replay {replay}: a replay issues no second code"
            );
        }

        // A send that is NOT a replay issues again, so the token is what makes a replay.
        for other in [
            json!({"ProcurementPortalPreferenceArn": arn}),
            json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": format!("another-token-{index}")}),
        ] {
            send(&router, &format!("token {token:?} not a replay"), &other).await;
            let reissued = outstanding_code(&engine, &arn).unwrap_or_else(|| {
                panic!("case token {token:?}: a send that is no replay must issue a code")
            });
            assert_ne!(
                reissued, issued,
                "case token {token:?}: a send that is no replay issues a fresh code, got the \
                 previous {issued:?}"
            );

            // The code the first send issued is retired by it.
            let (status, body) = call(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": issued}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case token {token:?}: the code a later send replaced completes nothing, got \
                 {status} {body}"
            );
        }
    }
}

/// RULE: a validation that has been completed can be started again — the start issues a
/// code of its own, and the code a completed validation consumed completes nothing a
/// second time.
///
/// The rows are the portals the enum admits, so the re-start is not keyed to one portal.
#[tokio::test]
async fn a_completed_validation_can_be_started_again() {
    let (router, engine) = emulator_with_engine();

    for (index, portal) in PORTAL_NAMES.iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    portal,
                    &format!("BUYER-REDO-{index}"),
                    &format!("SUPPLIER-REDO-{index}"),
                ),
            )
            .await,
        );

        send(
            &router,
            &format!("{portal} first send"),
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        let first = outstanding_code(&engine, &arn)
            .unwrap_or_else(|| panic!("case {portal}: the send must issue a code"));
        ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": first}),
        )
        .await;
        assert_eq!(
            outstanding_code(&engine, &arn),
            None,
            "case {portal}: the completed code is no longer outstanding"
        );

        send(
            &router,
            &format!("{portal} second send"),
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        let second = outstanding_code(&engine, &arn)
            .unwrap_or_else(|| panic!("case {portal}: a send after a completed validation issues"));
        assert_ne!(
            second, first,
            "case {portal}: the second send issues a code of its own, got the consumed {first:?}"
        );

        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": first}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {portal}: a consumed code completes nothing a second time, got {status} {body}"
        );
        ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": second}),
        )
        .await;
        assert!(
            is_verified(&engine, &arn),
            "case {portal}: the code the second send issued completes the validation"
        );
    }
}

/// RULE: an operation that names a resource by ARN resolves it against state, so an ARN
/// naming no preference is the modelled `ResourceNotFoundException` (HTTP 404) and no
/// state — no validation, no preference — is created for it.
///
/// The rows are well-formed preference ARNs differing in the id's characters and length,
/// alongside one preference that does exist, so a 404 is on the row's own account.
#[tokio::test]
async fn a_send_for_an_arn_that_names_no_preference_issues_nothing() {
    let (router, engine) = emulator_with_engine();
    let held = arn_of(
        &ok(
            &router,
            "CreateProcurementPortalPreference",
            &minimal_create(PORTAL_NAMES[0], "BUYER-HELD", "SUPPLIER-HELD"),
        )
        .await,
    );
    let held_before = stored_preferences(&engine);

    for id in [
        "a",
        "does-not-exist",
        "0123456789ab",
        "A-Very-Long-Preference-Id-0123456789",
    ] {
        let arn = engine::emulator_arn(
            "invoicing",
            "",
            &format!("procurement-portal-preference/{id}"),
        );
        let (status, body) = call(
            &router,
            "SendProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {arn}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "ResourceNotFoundException",
            "case {arn}: expected the modelled not-found error, got {body}"
        );
        assert_eq!(
            stored_preferences(&engine),
            held_before,
            "case {arn}: a refused send must create no preference"
        );
        assert_eq!(
            outstanding_code(&engine, &held),
            None,
            "case {arn}: a refused send must issue no code for another preference"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The status gate `AmendBenefitApplication` applies, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication` and, for the state no output reports, through the store the
//! service itself writes.
//!
//! The rule under test: an amendment is accepted only while AWS has not finished with the
//! application, and from a status it has finished with it is refused with the modelled
//! `ConflictException` (HTTP 409) with nothing persisted — not the amended members, not
//! the revision, not the amendment record.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which
//! status the application is in, which catalog it lives in, how many members the
//! amendment names and in which form its `FieldPath` is written, and whether the request
//! carries the application's current revision or one that is not current — so the refusal
//! cannot be coming from the optimistic-concurrency check instead. The status and the
//! catalog are CROSSED; the amendment shape is ROTATED over the rows (one shape per row,
//! each shape reached at least once), and every row is retried with each revision form.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::BENEFIT_APPLICATION_STATUS_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, SEEDED_CATALOGS, STATUS_CANCELED, STATUS_IN_REVIEW, TERMINAL_STATUSES,
};

use crate::amend_benefit_application::message_of;
use crate::application_status_gates::{reach, seed};
use crate::benefit_application_lifecycle::{error_code, text, Harness};

/// The operation under test, as the model names it — also the `X-Amz-Target` suffix the
/// harness sends and the operation key the store's write record carries.
const OPERATION: &str = "AmendBenefitApplication";

/// One amendment request shape: the `Amendments` list it carries, and where in the read
/// record each amended value must land once the amendment is accepted.
///
/// The forms are the ones a `FieldPath` may take (`Name`, `Description`, or
/// `BenefitApplicationDetails` followed by a `.`-separated path into that document, each
/// optionally written in JSON-path form — see `OPERATIONS.md` for
/// `AmendBenefitApplication`), plus a request carrying more than one amendment.
struct Shape {
    case: &'static str,
    amendments: Value,
    /// Each probe is the member path to read in the `GetBenefitApplication` body and the
    /// value that path must hold after the amendment is applied.
    probes: &'static [(&'static [&'static str], &'static str)],
}

/// The amendment shapes the rows below are drawn from.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "the name",
            amendments: json!([{ "FieldPath": "Name", "NewValue": "amended-name" }]),
            probes: &[(&["Name"], "amended-name")],
        },
        Shape {
            case: "the description",
            amendments: json!([
                { "FieldPath": "Description", "NewValue": "an amended description" }
            ]),
            probes: &[(&["Description"], "an amended description")],
        },
        Shape {
            case: "a member the details document already holds",
            amendments: json!([
                { "FieldPath": "BenefitApplicationDetails.CampaignName", "NewValue": "amended-campaign" }
            ]),
            probes: &[(&["BenefitApplicationDetails", "CampaignName"], "amended-campaign")],
        },
        Shape {
            case: "a nested member the details document does not hold yet",
            amendments: json!([
                { "FieldPath": "BenefitApplicationDetails.Campaign.Owner", "NewValue": "ada" }
            ]),
            probes: &[(&["BenefitApplicationDetails", "Campaign", "Owner"], "ada")],
        },
        Shape {
            case: "a field path written in JSON-path form",
            amendments: json!([{ "FieldPath": "$.Name", "NewValue": "amended-via-json-path" }]),
            probes: &[(&["Name"], "amended-via-json-path")],
        },
        Shape {
            case: "several members at once",
            amendments: json!([
                { "FieldPath": "Name", "NewValue": "amended-everything" },
                { "FieldPath": "Description", "NewValue": "amended everything" },
                { "FieldPath": "BenefitApplicationDetails.CampaignName", "NewValue": "all" },
            ]),
            probes: &[
                (&["Name"], "amended-everything"),
                (&["Description"], "amended everything"),
                (&["BenefitApplicationDetails", "CampaignName"], "all"),
            ],
        },
    ]
}

/// One `AmendBenefitApplication` request body.
fn amend_body(catalog: &str, id: &str, revision: &str, token: &str, amendments: &Value) -> Value {
    json!({
        "Catalog": catalog,
        "ClientToken": token,
        "Identifier": id,
        "Revision": revision,
        "AmendmentReason": "the partner corrected the figures",
        "Amendments": amendments,
    })
}

/// The `ClientToken` of every `AmendBenefitApplication` write the store records for one
/// application, oldest first.
///
/// No output member reports the write record, so it is read from the store the handler
/// writes: a refusal that persisted an amendment would show up here even though the read
/// of the application looks unchanged. The record is keyed by operation as well as by
/// token, so only this operation's writes are counted.
fn recorded_amendments(harness: &Harness, case: &str, catalog: &str, id: &str) -> Vec<String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .application(catalog, id)
        .unwrap_or_else(|| panic!("case {case:?}: {id:?} names no stored application"))
        .applied_writes
        .iter()
        .filter(|applied| applied.operation == OPERATION)
        .map(|applied| applied.client_token.clone())
        .collect()
}

/// The value at `path` in a read application, as a JSON value.
fn at<'a>(body: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(body, |value, member| &value[*member])
}

/// An amendment of an application AWS has finished with is refused with a 409
/// `ConflictException` and persists nothing — in every catalog, for every terminal
/// status, whichever members it names and whichever revision it carries.
#[tokio::test]
async fn an_amendment_of_a_terminal_application_is_refused_and_persists_nothing() {
    let harness = Harness::new();
    let shapes = shapes();

    let mut row = 0usize;
    for catalog in SEEDED_CATALOGS {
        for status in TERMINAL_STATUSES {
            let shape = &shapes[row % shapes.len()];
            let case = format!("{status} in {catalog}, amending {}", shape.case);
            let id = seed(&harness, &case, catalog, &format!("tok-term-seed-{row}")).await;
            reach(&harness, &case, catalog, &id, status).await;
            row += 1;

            let before = harness.get_in(&case, catalog, &id).await;
            let recorded_before = recorded_amendments(&harness, &case, catalog, &id);
            let current = text(&case, &before, "Revision");

            for (attempt, (revision_case, revision)) in [
                ("the current revision", current.clone()),
                ("a revision that is not the current one", "1".to_string()),
                ("a revision that is not a number", "not-a-revision".to_string()),
            ]
            .into_iter()
            .enumerate()
            {
                let case = format!("{case}, with {revision_case}");
                let body = amend_body(
                    catalog,
                    &id,
                    &revision,
                    &format!("tok-term-amend-{row}-{attempt}"),
                    &shape.amendments,
                );
                let (http, out) = harness.call(OPERATION, &body).await;
                assert_eq!(
                    http, 409,
                    "case {case:?}: expected 409 amending a {status} application, got {http} {out:?}"
                );
                assert_eq!(
                    error_code(&out),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {out:?}"
                );

                // The refusal reports which application is in which status, so a client
                // learns why its amendment was not applied.
                let message = message_of(&out);
                assert!(
                    message.contains(&id) && message.contains(status),
                    "case {case:?}: expected the refusal to name {id} and {status}, got {message:?}"
                );

                let after = harness.get_in(&case, catalog, &id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused amendment must persist nothing, got {after:?}"
                );
                assert_eq!(
                    recorded_amendments(&harness, &case, catalog, &id),
                    recorded_before,
                    "case {case:?}: a refused amendment must record no amendment"
                );
            }
        }
    }
}

/// An amendment is accepted from every status AWS has NOT finished with, in every
/// catalog: the named members are persisted, the revision is bumped, the amendment is
/// recorded and the status does not move.
///
/// The rows are the modelled `BenefitApplicationStatus` values
/// (`BENEFIT_APPLICATION_STATUS_VALUES`, so the set cannot drift from the model) that are
/// not terminal, so this test and the one above together cover all six.
#[tokio::test]
async fn an_amendment_is_accepted_from_every_non_terminal_status() {
    let harness = Harness::new();
    let shapes = shapes();

    let mut row = 0usize;
    for catalog in SEEDED_CATALOGS {
        for status in BENEFIT_APPLICATION_STATUS_VALUES
            .iter()
            .filter(|status| !TERMINAL_STATUSES.contains(status))
        {
            let shape = &shapes[row % shapes.len()];
            let case = format!("{status} in {catalog}, amending {}", shape.case);
            let id = seed(&harness, &case, catalog, &format!("tok-live-seed-{row}")).await;
            reach(&harness, &case, catalog, &id, status).await;
            row += 1;

            let before = harness.get_in(&case, catalog, &id).await;
            let current = text(&case, &before, "Revision");
            let token = format!("tok-live-amend-{row}");
            let body = amend_body(catalog, &id, &current, &token, &shape.amendments);
            harness.ok(&case, OPERATION, &body).await;

            let after = harness.get_in(&case, catalog, &id).await;
            for (path, expected) in shape.probes {
                assert_eq!(
                    at(&after, path),
                    &json!(expected),
                    "case {case:?}: expected {path:?} to be {expected:?}, got {after:?}"
                );
            }
            assert_ne!(
                text(&case, &after, "Revision"),
                current,
                "case {case:?}: an accepted amendment must bump the revision, got {after:?}"
            );
            assert_eq!(
                after["Status"],
                json!(status),
                "case {case:?}: an amendment must not move the status, got {after:?}"
            );
            assert!(
                recorded_amendments(&harness, &case, catalog, &id).contains(&token),
                "case {case:?}: an accepted amendment must be recorded"
            );
        }
    }
}

/// A retry of an amendment that was already applied answers as the first call did, even
/// once the application has since become terminal — and still persists nothing a second
/// time.
///
/// The retry is the same request, so it is the `ClientToken` the service matches it on;
/// what the status gate refuses is a NEW amendment, which the second body (a different
/// token on the same record) is.
#[tokio::test]
async fn a_retried_amendment_answers_as_the_first_call_once_the_application_is_terminal() {
    let harness = Harness::new();

    for (row, catalog) in SEEDED_CATALOGS.iter().enumerate() {
        let case = format!("a retry in {catalog}");
        let id = seed(&harness, &case, catalog, &format!("tok-retry-seed-{row}")).await;
        reach(&harness, &case, catalog, &id, STATUS_IN_REVIEW).await;

        let before = harness.get_in(&case, catalog, &id).await;
        let token = format!("tok-retry-amend-{row}");
        let body = amend_body(
            catalog,
            &id,
            &text(&case, &before, "Revision"),
            &token,
            &json!([{ "FieldPath": "Name", "NewValue": "amended-then-cancelled" }]),
        );
        harness.ok(&case, OPERATION, &body).await;

        harness
            .ok(
                &case,
                "CancelBenefitApplication",
                &json!({
                    "Catalog": catalog,
                    "ClientToken": format!("tok-retry-cancel-{row}"),
                    "Identifier": id,
                    "Reason": "withdrawn after the amendment",
                }),
            )
            .await;
        let cancelled = harness.get_in(&case, catalog, &id).await;
        assert_eq!(
            cancelled["Status"],
            json!(STATUS_CANCELED),
            "case {case:?}: expected the application to be cancelled, got {cancelled:?}"
        );
        let recorded = recorded_amendments(&harness, &case, catalog, &id);

        // The retry of the amendment already applied: accepted, and nothing changes.
        harness.ok(&case, OPERATION, &body).await;
        let after = harness.get_in(&case, catalog, &id).await;
        assert_eq!(
            after, cancelled,
            "case {case:?}: a retried amendment must change nothing, got {after:?}"
        );
        assert_eq!(
            recorded_amendments(&harness, &case, catalog, &id),
            recorded,
            "case {case:?}: a retried amendment must record no second amendment"
        );

        // A different amendment of the same, now terminal, record is refused.
        let fresh = amend_body(
            catalog,
            &id,
            &text(&case, &after, "Revision"),
            &format!("tok-retry-fresh-{row}"),
            &json!([{ "FieldPath": "Name", "NewValue": "amended-after-cancel" }]),
        );
        let (http, out) = harness.call(OPERATION, &fresh).await;
        assert_eq!(
            http, 409,
            "case {case:?}: expected 409 amending a cancelled application, got {http} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );
        let refused = harness.get_in(&case, catalog, &id).await;
        assert_eq!(
            refused, after,
            "case {case:?}: a refused amendment must persist nothing, got {refused:?}"
        );
    }
}

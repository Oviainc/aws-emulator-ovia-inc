// SPDX-License-Identifier: CC0-1.0

//! What a caller-driven status transition does to a benefit application's `Revision`, end
//! to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`, so what these tests pin is the emulator's HTTP behaviour on
//! the persisted record.
//!
//! The rule under test: `Revision` is the application's optimistic-concurrency token (the
//! model documents the input member as "the current revision number of the benefit
//! application to ensure optimistic concurrency control"), so EVERY call that changes the
//! stored record advances it — a status transition the caller drove exactly as much as a
//! change to the application's own content — and a write carrying the revision read before
//! that transition is refused with the modelled `ConflictException` (HTTP 409), while a
//! read reports the token without moving it.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which of
//! this service's caller-driven transitions is driven (`SubmitBenefitApplication`,
//! `RecallBenefitApplication`, `CancelBenefitApplication`), which status it is driven
//! from, which catalog the application lives in, which form its `Identifier` is given in,
//! and which write the stale revision is then spent on.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    SEEDED_CATALOGS, STATUS_CANCELED, STATUS_IN_REVIEW, STATUS_PENDING_SUBMISSION,
};

use crate::amend_benefit_application::message_of;
use crate::benefit_application_lifecycle::{error_code, instant, text, Harness};

/// One caller-driven status transition this service serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    Submit,
    Recall,
    Cancel,
}

impl Transition {
    /// The operation that drives the transition, as the model names it.
    fn operation(self) -> &'static str {
        match self {
            Self::Submit => "SubmitBenefitApplication",
            Self::Recall => "RecallBenefitApplication",
            Self::Cancel => "CancelBenefitApplication",
        }
    }

    /// The `BenefitApplicationStatus` the transition leaves the application in.
    fn target(self) -> &'static str {
        match self {
            Self::Submit => STATUS_IN_REVIEW,
            Self::Recall => STATUS_PENDING_SUBMISSION,
            Self::Cancel => STATUS_CANCELED,
        }
    }

    /// One request body driving the transition on `identifier`.
    ///
    /// `token` is the `ClientToken` the operation binds where it binds one;
    /// `SubmitBenefitApplication` models no `ClientToken` member at all, so that arm
    /// ignores it rather than sending a member the model does not declare.
    fn body(self, catalog: &str, identifier: &str, token: &str) -> Value {
        match self {
            Self::Submit => json!({ "Catalog": catalog, "Identifier": identifier }),
            Self::Recall => json!({
                "Catalog": catalog,
                "ClientToken": token,
                "Identifier": identifier,
                "Reason": "the partner is reworking the figures",
            }),
            Self::Cancel => json!({
                "Catalog": catalog,
                "ClientToken": token,
                "Identifier": identifier,
                "Reason": "no longer needed",
            }),
        }
    }
}

/// The write a row spends the pre-transition revision on — the operations whose input
/// binds the `Revision` member the transition is supposed to have moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Amend,
    Update,
}

impl Writer {
    fn operation(self) -> &'static str {
        match self {
            Self::Amend => "AmendBenefitApplication",
            Self::Update => "UpdateBenefitApplication",
        }
    }

    /// One request body for this write, carrying `revision` as its concurrency token.
    fn body(self, catalog: &str, identifier: &str, revision: &str, token: &str) -> Value {
        match self {
            Self::Amend => json!({
                "Catalog": catalog,
                "ClientToken": token,
                "Identifier": identifier,
                "Revision": revision,
                "AmendmentReason": "the partner corrected the figures",
                "Amendments": [{ "FieldPath": "Name", "NewValue": "amended-name" }],
            }),
            Self::Update => json!({
                "Catalog": catalog,
                "ClientToken": token,
                "Identifier": identifier,
                "Revision": revision,
                "Name": "edited-name",
            }),
        }
    }
}

/// Create one draft application in `catalog` and return `(id, arn)`.
async fn seed(harness: &Harness, case: &str, catalog: &str, token: &str) -> (String, String) {
    let benefit_id = text(case, &harness.active_benefit_of(catalog).await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": catalog,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "revision-token-subject",
                "Description": "the application the revision rule is asserted on",
                "BenefitApplicationDetails": { "CampaignName": "seeded" },
            }),
        )
        .await;
    (text(case, &created, "Id"), text(case, &created, "Arn"))
}

/// A read application's `Revision` as the number it renders, so two tokens compare by
/// value and the step between them is observable.
fn revision_of(case: &str, body: &Value) -> u64 {
    let rendered = text(case, body, "Revision");
    rendered.parse().unwrap_or_else(|error| {
        panic!("case {case:?}: Revision {rendered:?} is not a number: {error}")
    })
}

/// Submit one application, which must succeed and leave it `IN_REVIEW`.
async fn submit(harness: &Harness, case: &str, catalog: &str, identifier: &str) {
    harness
        .ok(
            case,
            Transition::Submit.operation(),
            // No token: a submission models none.
            &Transition::Submit.body(catalog, identifier, ""),
        )
        .await;
}

/// Every caller-driven status transition advances the application's `Revision` by one
/// step and its `UpdatedAt`, and the revision read before the transition is no longer the
/// one a write has to carry: spending it is a 409 `ConflictException` that persists
/// nothing, while the revision the transition produced is accepted.
///
/// The rows are each transition crossed with the status it is driven from, every seeded
/// catalog (`SEEDED_CATALOGS`, so the set cannot drift from the store) and both
/// `BenefitApplicationIdentifier` forms the modelled pattern `^(arn:.+|benappl-[0-9a-z]{14})$`
/// admits. A transition into a status AWS has finished with admits no later write at all,
/// so those rows assert the revision step alone (their status refusal is pinned by
/// `tests/all/amend_terminal_refusal.rs`).
#[tokio::test]
async fn every_status_transition_advances_the_concurrency_token() {
    let harness = Harness::new();

    // (the transition, the status it is driven from, the write the stale revision is then
    // spent on — none where the transition is into a terminal status)
    let rows: [(Transition, &str, Option<Writer>); 5] = [
        (
            Transition::Submit,
            STATUS_PENDING_SUBMISSION,
            Some(Writer::Amend),
        ),
        (Transition::Recall, STATUS_IN_REVIEW, Some(Writer::Amend)),
        (Transition::Recall, STATUS_IN_REVIEW, Some(Writer::Update)),
        (Transition::Cancel, STATUS_PENDING_SUBMISSION, None),
        (Transition::Cancel, STATUS_IN_REVIEW, None),
    ];

    let mut row = 0usize;
    for (transition, from, writer) in rows {
        for catalog in SEEDED_CATALOGS {
            for by_arn in [false, true] {
                let form = if by_arn { "the ARN" } else { "the id" };
                let case = format!(
                    "{} from {from} in {catalog} by {form}, stale write {writer:?}",
                    transition.operation()
                );
                row += 1;

                let (id, arn) = seed(&harness, &case, catalog, &format!("tok-seed-{row}")).await;
                let identifier = if by_arn { arn } else { id.clone() };
                if from == STATUS_IN_REVIEW {
                    submit(&harness, &case, catalog, &identifier).await;
                }

                let before = harness.get_in(&case, catalog, &identifier).await;
                assert_eq!(
                    before["Status"],
                    json!(from),
                    "case {case:?}: expected the application to stand at {from}, got {before:?}"
                );
                let before_revision = revision_of(&case, &before);

                // A read reports the token; it does not move it.
                let read_again = harness.get_in(&case, catalog, &identifier).await;
                let read_revision = revision_of(&case, &read_again);
                assert_eq!(
                    read_revision, before_revision,
                    "case {case:?}: a read must leave the revision at {before_revision}, got {read_revision}"
                );

                harness
                    .ok(
                        &case,
                        transition.operation(),
                        &transition.body(catalog, &identifier, &format!("tok-transition-{row}")),
                    )
                    .await;

                let after = harness.get_in(&case, catalog, &identifier).await;
                assert_eq!(
                    after["Status"],
                    json!(transition.target()),
                    "case {case:?}: expected {}, got {after:?}",
                    transition.target()
                );
                let after_revision = revision_of(&case, &after);
                assert_eq!(
                    after_revision,
                    before_revision + 1,
                    "case {case:?}: a status transition must advance the revision from \
                     {before_revision} to {}, got {after_revision}",
                    before_revision + 1
                );
                assert!(
                    instant(&case, &after, "UpdatedAt") > instant(&case, &before, "UpdatedAt"),
                    "case {case:?}: a status transition must advance UpdatedAt, got {after:?}"
                );

                let Some(writer) = writer else {
                    continue;
                };

                // The revision read BEFORE the transition no longer passes the
                // concurrency check.
                let (status, out) = harness
                    .call(
                        writer.operation(),
                        &writer.body(
                            catalog,
                            &identifier,
                            &before_revision.to_string(),
                            &format!("tok-stale-{row}"),
                        ),
                    )
                    .await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: a write carrying the pre-transition revision \
                     {before_revision} must be refused, got {status} {out:?}"
                );
                assert_eq!(
                    error_code(&out),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {out:?}"
                );
                let message = message_of(&out);
                for named in [id.as_str(), &after_revision.to_string(), &before_revision.to_string()]
                {
                    assert!(
                        message.contains(named),
                        "case {case:?}: the refusal must name {named:?}, got {message:?}"
                    );
                }
                let refused = harness.get_in(&case, catalog, &identifier).await;
                let refused_revision = revision_of(&case, &refused);
                assert_eq!(
                    refused_revision, after_revision,
                    "case {case:?}: a refused write must persist nothing, expected revision \
                     {after_revision}, got {refused:?}"
                );

                // The revision the transition produced IS the one a write has to carry.
                harness
                    .ok(
                        &case,
                        writer.operation(),
                        &writer.body(
                            catalog,
                            &identifier,
                            &after_revision.to_string(),
                            &format!("tok-fresh-{row}"),
                        ),
                    )
                    .await;
                let written = harness.get_in(&case, catalog, &identifier).await;
                let written_revision = revision_of(&case, &written);
                assert_eq!(
                    written_revision,
                    after_revision + 1,
                    "case {case:?}: an accepted write must advance the revision to {}, got {written:?}",
                    after_revision + 1
                );
            }
        }
    }
}

/// A cycle of transitions moves the token once per transition and never backwards: each
/// step of submit, recall, submit, cancel leaves the revision exactly one past the step
/// before it, whichever catalog the application lives in.
#[tokio::test]
async fn a_cycle_of_transitions_moves_the_token_once_per_step() {
    let harness = Harness::new();

    for (row, catalog) in SEEDED_CATALOGS.iter().enumerate() {
        let case = format!("the transition cycle in {catalog}");
        let (id, _arn) = seed(&harness, &case, catalog, &format!("tok-cycle-{row}")).await;

        let created = harness.get_in(&case, catalog, &id).await;
        let mut expected = revision_of(&case, &created);

        let steps: [Transition; 4] = [
            Transition::Submit,
            Transition::Recall,
            Transition::Submit,
            Transition::Cancel,
        ];
        for (step, transition) in steps.iter().enumerate() {
            harness
                .ok(
                    &case,
                    transition.operation(),
                    &transition.body(catalog, &id, &format!("tok-cycle-{row}-{step}")),
                )
                .await;
            expected += 1;
            let after = harness.get_in(&case, catalog, &id).await;
            let actual = revision_of(&case, &after);
            assert_eq!(
                actual, expected,
                "case {case:?}: after {} the revision must be {expected}, got {actual}",
                transition.operation()
            );
        }
    }
}

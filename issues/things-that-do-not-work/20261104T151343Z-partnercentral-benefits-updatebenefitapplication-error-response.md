Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 63098b159edc87608c74ce824cb49d4c8580e826

# The shared benefit-schema rule is judged before the status gate on the update path and after it on the amend path, so one conflicting request gets two answers

_Filed: 2026-11-04T15:13:43Z_
_Operation: UpdateBenefitApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `63098b159edc87608c74ce824cb49d4c8580e826`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`benefit_schema_refusal` is the single rule this crate documents as "the create, the update and the amendment each apply this to the document they would store", and the application-status gate (`is_updatable`/`is_terminal`, both built over the same status constants) is the single rule that refuses a write to an application the service will not accept content changes for. The two sibling write handlers evaluate those two rules in OPPOSITE order:

* `UpdateBenefitApplication` runs the schema rule FIRST (src/ops/update_benefit_application.rs:384-389) and the status gate AFTER it (:397-408).
* `AmendBenefitApplication` runs the status gate FIRST (src/ops/amend_benefit_application.rs:840-851) and the schema rule LAST, after the amendments have been applied to a copy (:889-894) — and its module header states that order explicitly, as "the order AWS reports".

Consequence: for the same logical request — a content change carrying a document the benefit's `BenefitRequestSchema` does not accept, aimed at an application whose status refuses content changes (`CANCELED`, `APPROVED`, `REJECTED`) — `UpdateBenefitApplication` answers `ValidationException` (HTTP 400) while `AmendBenefitApplication` answers `ConflictException` (HTTP 409). A client that branches on the refusal (retry after recall vs. fix the payload) is told two different things about one state of the resource, and only one of the two orders can match the service the amend header cites. Nothing in the crate states a reason for the divergence: the update handler's own comment at :391-396 only justifies status-before-revision, and the update header (:15-30) states the two rules in separate paragraphs without ordering them.

## Evidence

The amend path, with the order its header claims is the service's — src/ops/amend_benefit_application.rs:8-23:
    //! The operation answers in this order, which is the order AWS reports:
    //! …
    //! 4. the application's status — AWS has finished with a terminal application
    //!    ([`crate::state::TERMINAL_STATUSES`]), so amending one is `ConflictException`;
    //! 5. optimistic concurrency — the request's `Revision` must be the application's
    //!    current one, else `ConflictException`;
    //! 6. the amendments themselves, applied to a copy so that a request carrying one that
    //!    cannot be applied leaves the stored application exactly as it was.
and the code that implements it — src/ops/amend_benefit_application.rs:840-851 (status gate) ahead of src/ops/amend_benefit_application.rs:886-894:
        // The amended document has to satisfy the benefit's request schema just as a
        // supplied one does: an amendment can write a field the schema declares, so the
        // rule the create applies to the document it stores is applied to this one.
        if let Some(refusal) = crate::state::benefit_schema_refusal(
            benefit_schema.as_ref(),
            amended.benefit_application_details.as_ref(),
        ) {
            return Err(AmendBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }

The update path, same two rules, reversed — src/ops/update_benefit_application.rs:382-389:
        // A document this update would store has to satisfy the benefit's request schema,
        // exactly as the create's does: one rule, applied wherever the member is written.
        if let Some(refusal) = crate::state::benefit_schema_refusal(
            benefit_schema.as_ref(),
            input.benefit_application_details.as_ref(),
        ) {
            return Err(UpdateBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }
immediately followed by the status gate, whose own comment concedes that everything above it has already run — src/ops/update_benefit_application.rs:391-408:
        // An update is the partner editing its own DRAFT: once the application has been
        // submitted the record is the one AWS is reviewing, and a terminal application is
        // finished with, so an update of anything but a `PENDING_SUBMISSION` application
        // is refused and NOTHING below runs — see [`crate::state::UPDATABLE_STATUSES`].
        // Checked before the revision, because the status refuses the request whatever
        // revision it names.
        if !application.is_updatable() {
            return Err(UpdateBenefitApplicationErrorModeled::ConflictException(

That the two handlers are applying ONE rule, not two — src/state.rs:2536-2539:
    /// The member is modelled optional wherever it is written, so a request that supplies NO
    /// document has nothing to judge; a document it does supply is judged as a whole — which
    /// is why the create, the update and the amendment each apply this to the document they
    /// would store.

Test coverage: tests/all/application_status_gates.rs pins the status refusals (its :135 note explains the rows are built so "the refusal cannot be coming from the optimistic-concurrency check"), tests/all/amend_terminal_refusal.rs pins the amend status-before-content order, and tests/all/benefit_application_lifecycle.rs:497 covers a document "`BenefitRequestSchema` does not accept — one of another JSON type, or a declared field". No test combines a schema-violating document with a non-updatable application on the update path, so nothing pins today's precedence and no comment, test or doc gives a reason for it differing from the amend path.

## Steps to Reproduce

1. Boot the emulator and record the partner entitlement for the calling account (state::grant_partner_entitlement, as tests/all/benefit_application_lifecycle.rs's Harness::new does).
2. CreateBenefitApplication in catalog `AWS` against an ACTIVE benefit whose seeded `BenefitRequestSchema` declares a field with a non-string JSON type (see state::benefit_request_schema and SEEDED_BENEFITS); note the returned `Id` and `Revision`.
3. CancelBenefitApplication for that `Id` with a fresh `ClientToken`, so the application's status is `CANCELED` (terminal: state::TERMINAL_STATUSES).
4. Call UpdateBenefitApplication with that `Catalog`/`Identifier`, the current `Revision`, a fresh `ClientToken` and a `BenefitApplicationDetails` document that puts a string where the benefit's schema declares a number. Observe HTTP 400 `ValidationException` from src/ops/update_benefit_application.rs:384-389 — the status gate at :397-408 never runs.
5. Call AmendBenefitApplication on the same CANCELED application with an `Amendments` entry whose `FieldPath` names that same declared field and whose `NewValue` is that same string. Observe HTTP 409 `ConflictException` from src/ops/amend_benefit_application.rs:840-851 — the schema rule at :889-894 never runs.
6. Compare with the order src/ops/amend_benefit_application.rs:8-23 states the service answers in (status at step 4, the content rules at step 6): one of the two handlers is reporting the wrong refusal for a request both reject.

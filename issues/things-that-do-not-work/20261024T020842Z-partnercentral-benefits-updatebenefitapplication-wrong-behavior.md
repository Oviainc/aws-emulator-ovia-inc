Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# UpdateBenefitApplication mutates submitted and terminal applications instead of refusing with ConflictException

_Filed: 2026-10-24T02:08:42Z_
_Operation: UpdateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler gates an update ONLY on the optimistic-concurrency `Revision`; it never looks at the application's `Status`. So a caller can edit `Name`, `Description`, `BenefitApplicationDetails`, `PartnerContacts` and `FileDetails` of an application that is `IN_REVIEW` (already submitted to AWS) or in a terminal status (`CANCELED`, `APPROVED`, `REJECTED`), as long as the request names the current revision; the emulator answers 200 and the next `GetBenefitApplication` reports the edited content.

Expected: an update of an application that is no longer a partner-editable draft is refused with the modelled `ConflictException` (HTTP 409) and nothing is persisted; only a `PENDING_SUBMISSION` application is updatable. That is why the API also models `RecallBenefitApplication` ("recalls a SUBMITTED application" back to `PENDING_SUBMISSION`) and `AmendBenefitApplication` (apply amendments to specific fields with an `AmendmentReason`): if `UpdateBenefitApplication` were accepted while `IN_REVIEW`, neither of those operations would have a purpose. Editing a `CANCELED` application is doubly wrong — the emulator's own `CancelBenefitApplication` treats `CANCELED` as terminal (second cancel = 409) and `RecallBenefitApplication` refuses from `CANCELED`, yet `UpdateBenefitApplication` still rewrites the record and bumps its revision.

Actual: status is never consulted; the only refusal path is the revision mismatch.

## Evidence

Handler: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/update_benefit_application.rs — after `store.application_mut(...)` the only check is `if application.revision_text() != input.revision { ConflictException }` (lines ~222-234); the member application then runs unconditionally and calls `application.revise(now)`.
Spec/behaviour grounding: the model declares `ConflictException` (HTTP 409, "the request conflicts with the current state of the resource") on `UpdateBenefitApplication`; the recorded AWS behavioural corpus `simparity-behavioral-aws-v1` carries the unit `partnercentral-benefits.update-benefit-application-after-submission-rejected` (listed in the Issue-Trace-Ids of the fixed issue 20261022T012817Z-partnercentral-benefits-validationexception-message-format-missing-partner-entitlement.md), i.e. AWS rejects an update once the application has been submitted. `BenefitApplicationStatus` models `PENDING_SUBMISSION | IN_REVIEW | ACTION_REQUIRED | APPROVED | REJECTED | CANCELED`.
Test coverage: tests/all/benefit_application_lifecycle.rs `update_applies_supplied_members_and_bumps_the_revision` only updates a `PENDING_SUBMISSION` application, and `update_refuses_a_revision_that_is_not_the_current_one` only covers the revision check. No test asserts the behaviour of an update after submission or after cancellation, so this is uncovered rather than intentionally asserted.

## Steps to Reproduce

1. Grant the emulator account the partner entitlement (the service's state gate) and call CreateBenefitApplication(Catalog='Sandbox', ClientToken='t1', BenefitIdentifier=<a seeded benefit id>, Name='demo'); note Id and Revision.
2. Call SubmitBenefitApplication(Catalog='Sandbox', Identifier=<Id>) — the application becomes IN_REVIEW.
3. Call GetBenefitApplication to read the current Revision.
4. Call UpdateBenefitApplication(Catalog='Sandbox', ClientToken='t2', Identifier=<Id>, Revision=<current>, Name='edited-while-in-review').
5. Observe HTTP 200 and a bumped Revision; GetBenefitApplication reports Name='edited-while-in-review' while Status is still IN_REVIEW. Expected: 409 ConflictException and no change.
6. Repeat with a CANCELED application (CancelBenefitApplication first): the update is again accepted instead of refused.

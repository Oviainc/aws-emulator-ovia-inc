Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# AmendBenefitApplication applies amendments to terminal applications (CANCELED/REJECTED/APPROVED)

_Filed: 2026-10-24T02:08:42Z_
_Operation: AmendBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler checks the client token (replay), the revision, the amendment list bounds and the resulting details document, but never the application's `Status`. An application in a terminal status — notably `CANCELED`, which the emulator itself treats as terminal elsewhere — can therefore be amended: `Name`, `Description` and `BenefitApplicationDetails` are rewritten, the revision is bumped and an amendment record is appended, all answered HTTP 200.

Expected: an amendment of an application AWS no longer holds open is refused with the modelled `ConflictException` (HTTP 409) and nothing is persisted. An amendment is a correction to an application under review; a cancelled or decided application is not amendable.

Actual: the amendment is applied to the terminal record.

## Evidence

Handler: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/amend_benefit_application.rs — `handle` consults `application.applied_amendment(..)` and `application.revision_text()` only; `application.status` is never read before `*application = amended`.
Internal inconsistency proving the rule is understood elsewhere in the crate: crate::state::RECALLABLE_STATUSES excludes `CANCELED`/`APPROVED`/`REJECTED` (src/state.rs, "the terminal `APPROVED`, `REJECTED`, `CANCELED` are not" recallable), CancelBenefitApplication refuses a second cancel as terminal, and SubmitBenefitApplication refuses any non-`PENDING_SUBMISSION` status with ConflictException. The model declares `ConflictException` (409) on AmendBenefitApplication for exactly this "conflicts with the current state of the resource" case.
Test coverage: tests/all/amend_benefit_application.rs exercises token replay, revision mismatch, field paths and document bounds; no case amends a cancelled or otherwise terminal application.

## Steps to Reproduce

1. Grant the partner entitlement, then CreateBenefitApplication(Catalog='Sandbox', ClientToken='a0', BenefitIdentifier=<seeded benefit id>, Name='demo'); note Id.
2. Call CancelBenefitApplication(Catalog='Sandbox', ClientToken='a1', Identifier=<Id>, Reason='withdrawn') — status becomes CANCELED.
3. Read the current Revision with GetBenefitApplication.
4. Call AmendBenefitApplication(Catalog='Sandbox', ClientToken='a2', Identifier=<Id>, Revision=<current>, AmendmentReason='fix figures', Amendments=[{FieldPath:'Name', NewValue:'amended-after-cancel'}]).
5. Observe HTTP 200 and GetBenefitApplication reporting Name='amended-after-cancel' with Status CANCELED and a bumped Revision. Expected: 409 ConflictException, record untouched.

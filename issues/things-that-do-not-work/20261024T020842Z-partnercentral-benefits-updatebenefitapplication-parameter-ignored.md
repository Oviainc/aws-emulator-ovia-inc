Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# UpdateBenefitApplication ignores its required ClientToken, so a retried update answers 409 instead of replaying the first result

_Filed: 2026-10-24T02:08:42Z_
_Operation: UpdateBenefitApplication | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ClientToken` is required on `UpdateBenefitApplicationInput` and documented as "A unique, case-sensitive identifier to ensure idempotent processing of the update request". The handler never reads it — concurrency is enforced solely through `Revision`. A client whose update succeeded but whose response was lost retries with the same `ClientToken` and the same `Revision`; because the first call already advanced the revision, the retry is answered HTTP 409 `ConflictException` instead of being recognised as the same request and answered with the first call's `Id`/`Arn`/`Revision`.

Expected: a replay carrying a `ClientToken` whose update already applied answers 200 with the output that update produced, and applies nothing a second time (the pattern this crate already implements for `CreateBenefitApplication` and `AmendBenefitApplication`, which checks the token *before* the revision for exactly this reason).

Actual: the token is dropped; the retry is indistinguishable from a stale-revision write and is refused with 409.

## Evidence

Handler: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/update_benefit_application.rs — `input.client_token` appears nowhere in `handle`; the only pre-write check is the revision comparison (lines ~222-234). Contrast src/ops/amend_benefit_application.rs, which calls `application.applied_amendment(&input.client_token)` and returns success for a replay *before* comparing the revision, with the comment "It is checked BEFORE the revision, because the first call moved the revision the retry still carries."
Spec: smithy_query(doc, shape='UpdateBenefitApplicationInput', member='ClientToken') = "A unique, case-sensitive identifier to ensure idempotent processing of the update request" (@required).
Test coverage: tests/all/benefit_application_lifecycle.rs covers only distinct tokens per update and the stale-revision 409; no test replays an update's ClientToken, so the idempotent-retry path is untested.

## Steps to Reproduce

1. Grant the partner entitlement, then CreateBenefitApplication(Catalog='Sandbox', ClientToken='u0', BenefitIdentifier=<seeded benefit id>, Name='demo'); note Id and Revision R.
2. Call UpdateBenefitApplication(Catalog='Sandbox', ClientToken='u1', Identifier=<Id>, Revision=R, Name='demo-2') — 200, Revision becomes R+1.
3. Replay the byte-identical request (ClientToken 'u1', Revision R), simulating an SDK retry of a response that was lost.
4. Observe HTTP 409 ConflictException. Expected: HTTP 200 reporting the same Id/Arn/Revision the first call returned, with the record unchanged by the replay.

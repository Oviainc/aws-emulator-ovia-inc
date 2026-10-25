Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CancelBenefitApplication ignores ClientToken, so a retried cancel answers 409 instead of succeeding idempotently

_Filed: 2026-10-24T02:08:42Z_
_Operation: CancelBenefitApplication | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ClientToken` is a required member of `CancelBenefitApplicationInput` and is documented as "A unique, case-sensitive identifier to ensure idempotent processing of the cancellation request". The handler never reads it: it is not stored on the application record and no replay check consults it. Consequently a client that retries the very same cancellation request (same `ClientToken`) — the exact case an idempotency token exists for, e.g. after a dropped response or an SDK retry — receives HTTP 409 `ConflictException` ("benefit application ... is already CANCELED") instead of the success its first, lost call produced.

Expected: a cancel replayed with the `ClientToken` of the cancel that already took effect is answered as a retry (HTTP 200, empty body) and changes nothing — the way the emulator already handles the token on `CreateBenefitApplication`, `AmendBenefitApplication` and `RecallBenefitApplication`. A *second, different* cancellation request (a new/absent token) may still be the 409.

Actual: the token is dropped entirely; every repeat cancellation, retry or not, is 409.

## Evidence

Handler: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/cancel_benefit_application.rs — `input.client_token` is never referenced anywhere in `handle`; the body goes straight from `store.application_mut(..)` to `if application.status == STATUS_CANCELED { ConflictException }` and then sets the status and `application.revise(now)`.
Spec: smithy_query(doc, shape='CancelBenefitApplicationInput', member='ClientToken') = "A unique, case-sensitive identifier to ensure idempotent processing of the cancellation request"; the member is @required.
Comparison inside this same crate: crate::state::BenefitApplication stores `recall_client_token` and `amendments[].client_token`, and Catalog stores `client_tokens` for creates, so the token-replay mechanism exists and is simply not applied here.
Test coverage: tests/all/benefit_application_lifecycle.rs `cancel_records_the_reason_and_is_terminal` replays the identical request body (same ClientToken) and asserts 409 — i.e. a test currently pins the non-idempotent behaviour, so fixing this also means correcting that assertion to distinguish a token replay from a genuinely new cancellation.

## Steps to Reproduce

1. Grant the partner entitlement, then CreateBenefitApplication(Catalog='Sandbox', ClientToken='c1', BenefitIdentifier=<seeded benefit id>, Name='demo'); note Id.
2. Call CancelBenefitApplication(Catalog='Sandbox', ClientToken='cancel-tok-1', Identifier=<Id>, Reason='no longer needed') — 200, status becomes CANCELED.
3. Replay the byte-identical request (same ClientToken 'cancel-tok-1').
4. Observe HTTP 409 ConflictException. Expected: HTTP 200 with the record left exactly as the first cancel left it (same Revision, same UpdatedAt, same StatusReason).

Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A ClientToken replay carrying different parameters silently drops the new request's Name, Description, BenefitIdentifier, Tags, contacts and files

_Filed: 2026-10-24T15:34:23Z_
_Operation: CreateBenefitApplication | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The create's idempotency check keys on `ClientToken` ALONE. When a token has already been used in this catalog the handler returns the first call's `Id`/`Arn`/`Revision` and returns early — without comparing any other member of the request to the application the token created.

Expected: `ClientToken` is documented as "a unique, case-sensitive identifier to ensure idempotent processing of the creation request". A replay of the SAME request must replay the first response (which the emulator does correctly). A reuse of the same token with a DIFFERENT request body is an idempotent-parameter mismatch: AWS refuses it (this operation declares both `ConflictException` (409) and `ValidationException` (400) for exactly this class of failure) rather than reporting success for a creation it did not perform.

Actual: the second call is answered HTTP 200 with the FIRST application's id, ARN and revision; every member of the second request — `BenefitIdentifier`, `Name`, `Description`, `FulfillmentTypes`, `BenefitApplicationDetails`, `Tags`, `AssociatedResources`, `PartnerContacts`, `FileDetails` — is discarded and nothing is persisted. A client that reuses a token by mistake is told its application was created, then `GetBenefitApplication` on the returned id reports an application for a different benefit with different content, and the application it believes it created never exists.

## Evidence

Model: `CreateBenefitApplication` is `@idempotent` with `ClientToken` `@required` ("A unique, case-sensitive identifier to ensure idempotent processing of the creation request"), and binds `ConflictException` ("the request conflicts with the current state of the resource") and `ValidationException` — so a mismatched replay has a modelled refusal available.

Handler: src/ops/create_benefit_application.rs lines ~265-275 — `catalog.client_tokens.get(&input.client_token).and_then(|id| catalog.applications.get(id))` returns `Ok(CreateBenefitApplicationOutput { id, arn, revision })` immediately. No member of `input` other than `catalog`/`client_token` is read on that path; the lookup even precedes `resolve_benefit_id`, so a replay naming a benefit that does not exist is answered 200 instead of ResourceNotFoundException. Stored state keeps no fingerprint of the original request (src/state.rs `Catalog::client_tokens` is `BTreeMap<String, String>`: token -> id), so a mismatch cannot currently be detected at all. The token also never expires.

Test coverage: none for the mismatch case. tests/all/benefit_application_lifecycle.rs ~633-697 (`create_is_idempotent_in_its_client_token`) replays the IDENTICAL body and then uses a different token for a different application; no test replays one token with a changed `BenefitIdentifier`/`Name`/`Tags`.

## Steps to Reproduce

1. Start the emulator with the partner entitlement in place and call ListBenefits for Catalog=AWS to pick two different ACTIVE benefit ids, B1 and B2.
2. POST CreateBenefitApplication with {"Catalog":"AWS","ClientToken":"tok-1","BenefitIdentifier":"B1","Name":"first"} and note the returned Id.
3. POST CreateBenefitApplication again with the SAME token but a different body: {"Catalog":"AWS","ClientToken":"tok-1","BenefitIdentifier":"B2","Name":"second","Tags":[{"Key":"k","Value":"v"}]}.
4. Observe HTTP 200 returning the FIRST application's Id/Arn/Revision instead of a ConflictException/ValidationException for the token reuse.
5. GetBenefitApplication on that Id: BenefitId is B1 and Name is "first"; the second request's benefit, name and tags were never stored, and ListTagsForResource on the ARN reports no "k" tag.
6. Repeat step 3 with "BenefitIdentifier":"ben-00000000000000" (a benefit that does not exist): still HTTP 200, no ResourceNotFoundException.

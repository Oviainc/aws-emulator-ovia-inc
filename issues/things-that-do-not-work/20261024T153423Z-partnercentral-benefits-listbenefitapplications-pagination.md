Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A well-formed NextToken this service never issued is accepted and resumes paging from an arbitrary position

_Filed: 2026-10-24T15:34:23Z_
_Operation: ListBenefitApplications | Class: pagination_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination cursor is validated for SHAPE only, never for issuance. `crate::state::decode_page_token(token, prefix)` hex-decodes the token, strips the record-kind prefix and then accepts any body of exactly `ID_BODY_LEN` lowercase-alphanumeric characters — it never checks that the id it decoded names a record this catalog actually holds, nor that this service ever issued that cursor.

Expected: a `NextToken` the service did not issue is refused with the modelled `ValidationException` (HTTP 400) naming `NextToken`, which is the rule the service's own tests state ("a NextToken this operation did not issue is refused"). AWS pagination tokens are opaque, service-minted blobs; a client-fabricated or stale one is a validation failure, not a cursor.

Actual: `hex("benappl-aaaaaaaaaaaaaa")` (or any `benappl-` + 14 lowercase-alnum body) is accepted with HTTP 200, and `catalog.applications.range(resume_from..)` silently starts the page at whatever id sorts at or after the fabricated one — so the caller gets a truncated, arbitrary window of the collection instead of a refusal. The same hole exists in the two sibling list operations through the same shared helper: `ListBenefits` accepts `hex("ben-aaaaaaaaaaaaaa")` and `ListBenefitAllocations` accepts `hex("benalloc-aaaaaaaaaaaaaa")`. A token minted by a list in one catalog is likewise accepted by a list in another catalog, because the decode carries no catalog binding.

## Evidence

Spec/model: `ListBenefitApplicationsInput$NextToken` is `String @length(1..2048)` with pattern `^[\s\S]*$` (smithy_query shape=ListBenefitApplicationsInput), so the centralized constraint walk can only refuse an empty or over-long token — issuance is necessarily the handler's job. The operation is `@paginated(inputToken=NextToken, outputToken=NextToken)`.

Handler: src/ops/list_benefit_applications.rs lines ~199-205 decode the token via `crate::state::decode_page_token(token, crate::state::APPLICATION_ID_PREFIX)` and line ~256 resumes with `catalog.applications.range(resume_from.unwrap_or_default()..)`; the decoded id is never looked up in `catalog.applications`. The helper itself is src/state.rs ~2206-2216: it validates prefix, body length and alphabet only. The identical pattern is in src/ops/list_benefits.rs ~192-198 and src/ops/list_benefit_allocations.rs ~199-205, so the fix belongs in the shared helper (or in each call site, by resolving the decoded id against the collection).

Test coverage: the closest tests stop short of this case. tests/all/benefit_allocations.rs ~741-781 ("Rule 4: a NextToken this operation did not issue is refused") only feeds non-hex text, hex of non-ids, a token of ANOTHER record kind (`hex("benappl-00000000000000")` to ListBenefitAllocations), a short body, and an upper-case body — every row fails the shape check. tests/all/benefit_application_lifecycle.rs ~1421-1440 feeds `"not-a-token"`, `"6162"`, `"benappl-00000000000000"` (unencoded). No test sends a correctly hex-encoded, correctly shaped id of the RIGHT record kind that was never issued, which is exactly the case the handler admits.

## Steps to Reproduce

1. Start the emulator with this service mounted and the partner entitlement in place (the gate every benefit operation applies).
2. Call CreateBenefitApplication a few times so the AWS catalog holds several applications.
3. Compute a cursor the service never issued: hex-encode the ASCII string `benappl-aaaaaaaaaaaaaa` -> `62656e6170706c2d6161616161616161616161616161`.
4. POST / with X-Amz-Target: PartnerCentralBenefitsService.ListBenefitApplications and body {"Catalog":"AWS","NextToken":"62656e6170706c2d6161616161616161616161616161"}.
5. Observe HTTP 200 with a page resumed from that fabricated position (some applications silently skipped), instead of HTTP 400 ValidationException naming NextToken.
6. Repeat against ListBenefits with hex("ben-aaaaaaaaaaaaaa") and ListBenefitAllocations with hex("benalloc-aaaaaaaaaaaaaa") — both are accepted the same way.
7. Also take a NextToken issued by ListBenefitApplications for Catalog=AWS and resend it with Catalog=Sandbox: it is accepted rather than refused.

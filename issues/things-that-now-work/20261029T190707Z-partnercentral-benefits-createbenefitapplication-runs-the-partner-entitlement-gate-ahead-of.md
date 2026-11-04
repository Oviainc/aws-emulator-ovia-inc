Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected
Issue-Shape-Id: com.amazonaws.partnercentralbenefits#PartnerCentralBenefitsService
Issue-Key: partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected|run#7|AssertionError|assert refusal == ("ResourceNotFoundException", f"Catalog `{catalog}` not found"), (
Issue-Key: partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected|run#4|AssertionError|assert control == ("ResourceNotFoundException", f"Catalog `{catalog}` not found"), (
Issue-Commit: ef775f68482e6d25f63f0fc1f052163f266c1a25

# CreateBenefitApplication runs the partner-entitlement gate ahead of the Catalog existence check, so a nonexistent Catalog is refused 403 AccessDeniedException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `ef775f68482e6d25f63f0fc1f052163f266c1a25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBenefitApplication

## Description
One root cause, grounded in two failing behavioral checks.

Cause 1 of 1: the order of the two refusal guards in the CreateBenefitApplication handler is inverted relative to AWS, so the account-level entitlement denial masks the resource-existence refusal. Failing checks: partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected, partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected.

AWS: CreateBenefitApplication models both refusals — Smithy (services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model): CreateBenefitApplication errors include ResourceNotFoundException (httpError 404) and AccessDeniedException (httpError 403); CreateBenefitApplicationInput$Catalog is `CatalogName`, required, pattern ^[A-Za-z0-9_-]+$ and carries no enum, so a syntactically valid but nonexistent catalog name is NOT a modelled-constraint failure and must be resolved by a business-logic existence lookup. Recorded behavior: AWS resolves the named Catalog FIRST. A request whose Catalog is syntactically valid but names no catalog is refused 404 ResourceNotFoundException with message `Catalog `<Catalog>` not found` — fixed text `Catalog ` + backtick-quoted value + ` not found`, where the backticked part is the Catalog value taken verbatim from the request and the rest is fixed. Only once the catalog resolves does the partner-entitlement gate apply: a well-formed request naming a real catalog (`AWS` or `Sandbox`) from an account with no active AWS partner benefit is refused 403 AccessDeniedException with the fixed message `This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support.` (no variable parts). The same gate fronts the read side (ListBenefitApplications), which the recording confirms.

Emulator: the handler returns the entitlement AccessDeniedException for BOTH cases. For Catalog="NotARealCatalog" (and for a random `not-a-real-catalog-<hex>`) the replay observed ('AccessDeniedException', "This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support.") — i.e. 403 with the entitlement message — where AWS answers 404 ResourceNotFoundException / `Catalog `NotARealCatalog` not found`. The code is explicit about the inverted order: create_benefit_application.rs calls `crate::state::entitlement_refusal(&*self.engine)` as the first thing in `handle` and returns AccessDeniedException from it, while `store.catalog(&input.catalog)` -> ResourceNotFoundException runs ~70 lines later, after the store lock is taken. The handler's own comment asserts the denial must come first "so the denial never leaks whether a resource exists" — that is a policy the real service does not apply to CreateBenefitApplication's Catalog member, and the behavioral recording contradicts it.

Why: a client cannot distinguish a misrouted/mistyped catalog from a missing partner entitlement. Code that branches on the error — retrying/annotating ResourceNotFoundException as a request-content bug while escalating AccessDeniedException to an onboarding/permissions path — takes the wrong branch for every bad-catalog request, and SDK retry/classification helpers see a 403 where a 404 was recorded. It also makes the entitlement assertion non-load-bearing in any test that uses a bad-catalog call as its positive control (exactly what the second failing check does), so an emulator that denies every call regardless of content passes checks intended to prove the gate exists.

Fix: the guard ordering in the CreateBenefitApplication handler — resolve the request's Catalog (the existing `store.catalog(&input.catalog)` lookup returning ResourceNotFoundException) before the `entitlement_refusal` AccessDeniedException gate, keeping both refusals and their messages unchanged. Apply the same ordering to any sibling mutating handler that fronts the catalog lookup with the entitlement gate, so a nonexistent catalog is always reported as the missing resource; the entitlement gate must still refuse a well-formed request naming a real catalog, and must still front the read side (ListBenefitApplications) as it does today.

Traces: partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected (control call with Catalog="Sandbox" correctly yields AccessDeniedException; the Catalog="NotARealCatalog" call yields AccessDeniedException instead of ResourceNotFoundException); partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected (its positive control, Catalog="not-a-real-catalog-<hex>", yields AccessDeniedException instead of ResourceNotFoundException, so the check fails before reaching its entitlement and read-side assertions). Code citations: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/create_benefit_application.rs:220-228 (entitlement gate first) and :286-299 (catalog lookup -> ResourceNotFoundException); src/state.rs:504-506 and :517-524 (catalog-not-found wording, SEEDED_CATALOGS = ["AWS", "Sandbox"]). Related but distinct open issue, which does NOT cover this ordering: 20261029T125213Z-partnercentral-benefits-createbenefitapplication-wrong-behavior--r4.md (state::insert_application writes indices ahead of its own catalog guard).

## Evidence
Check partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected:
  Expected: ('ResourceNotFoundException', 'Catalog `NotARealCatalog` not found')  [HTTP 404]
  Actual:   ('AccessDeniedException', "This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support.")  [HTTP 403]
  AssertionError: expected ResourceNotFoundException for a nonexistent Catalog, got ('AccessDeniedException', ...)
  The same check's control call (Catalog="Sandbox", same fabricated BenefitIdentifier "ben-00000000000000") PASSED with AccessDeniedException + that exact message, proving the emulator reaches the entitlement gate and that the divergence is the bad-Catalog path specifically.

Check partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected:
  Expected (positive control): ('ResourceNotFoundException', 'Catalog `not-a-real-catalog-<hex>` not found')  [HTTP 404]
  Actual:   ('AccessDeniedException', "This action requires an active AWS partner benefit. ...")  [HTTP 403]
  AssertionError: positive control: a nonexistent Catalog must be rejected on its own business-logic grounds, got ('AccessDeniedException', ...) -- if this is also AccessDeniedException, the account may be denied on every call regardless of content, and the entitlement assertion below would not be load-bearing

Message anatomy (what is fixed vs variable):
  ResourceNotFoundException message: fixed text `Catalog ` ... ` not found` with the Catalog value from the request inside backticks; the values `NotARealCatalog` / `not-a-real-catalog-<hex>` above are per-run request inputs, not part of the template.
  AccessDeniedException message: entirely fixed text, no variable parts.

Model (Smithy, CreateBenefitApplication): errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]; input Catalog -> CatalogName, required, pattern ^[A-Za-z0-9_-]+$, no enum. Both diverging refusals are modelled; only their precedence is wrong.

Emulator source (ordering is explicit):
  src/ops/create_benefit_application.rs:224  if let Some(denied) = crate::state::entitlement_refusal(&*self.engine) { return Err(...AccessDeniedException(denied)...) }
  src/ops/create_benefit_application.rs:292  let catalog = match store.catalog(&input.catalog) { Err(missing) => return Err(...ResourceNotFoundException(missing)...) }

## Steps to Reproduce
1. Boot the emulator and build a partnercentral-benefits client for us-east-1 (endpoint https://partnercentral-benefits.us-east-1.api.aws routed at the emulator).
2. Call CreateBenefitApplication with Catalog="Sandbox", a fresh ClientToken, BenefitIdentifier="ben-00000000000000", and any Name. Observe 403 AccessDeniedException with the entitlement message — correct, and the baseline that proves the gate is wired.
3. Call CreateBenefitApplication again with everything the same except Catalog="NotARealCatalog" (syntactically valid against ^[A-Za-z0-9_-]+$, but not one of the seeded catalogs AWS/Sandbox) and a fresh ClientToken.
4. Observe 403 AccessDeniedException with the same entitlement message. AWS answers 404 ResourceNotFoundException with message `Catalog `NotARealCatalog` not found`, because it resolves the Catalog before applying the entitlement gate.
5. Confirm the cause in source: src/ops/create_benefit_application.rs:220-228 returns the entitlement AccessDeniedException as the first act of `handle`, while the `store.catalog(&input.catalog)` -> ResourceNotFoundException lookup is at :286-299, after the lock is taken — so the bad-catalog path is unreachable for any caller the entitlement gate denies.
6. Re-replay both failing checks to confirm a fix: partnercentral-benefits.create-benefit-application-invalid-catalog-enum-rejected and partnercentral-benefits.create-benefit-application-requires-active-partner-benefit-rejected (the latter also asserts the entitlement AccessDeniedException still fires for Catalog="Sandbox" and that ListBenefitApplications is still refused by the same gate — both must keep passing).

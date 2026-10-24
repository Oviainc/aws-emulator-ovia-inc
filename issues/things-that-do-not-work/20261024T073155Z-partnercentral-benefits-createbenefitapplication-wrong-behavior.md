Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# CreateBenefitApplication ignores the requested benefit's catalog record: an INACTIVE benefit is applied for and BenefitRequestSchema is never checked

_Filed: 2026-10-24T07:31:55Z_
_Operation: CreateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a create names a benefit, the handler consults the catalog's benefit record for only two things: that an id resolves, and the benefit's `Programs` (copied onto the application). Two properties the same stored record carries are never read, so the create accepts applications the catalog itself says are impossible.

1. `Benefit$Status` (`ACTIVE | INACTIVE`) is not checked. A benefit that is INACTIVE is no longer offered, so no partner can apply for it; the emulator answers HTTP 200 and mints a PENDING_SUBMISSION application against it, which can then be submitted and later show up in `ListBenefitApplications`. Expected: the create is refused for a benefit that is not ACTIVE (the modelled `ValidationException` / `ConflictException` both of which this operation declares), not silently accepted.

2. `Benefit$BenefitRequestSchema` — modelled and documented as "The schema definition that describes the required fields for requesting this benefit" — is stored per seeded benefit (field name + JSON type) but never applied to the request. `CreateBenefitApplication` runs only `crate::state::details_rejection` on `BenefitApplicationDetails`, which checks size/depth/member-count bounds, nothing about the benefit's required fields. A create that omits every field the benefit's schema requires (or supplies a string where the schema says number) is answered 200. Expected: a request whose `BenefitApplicationDetails` does not satisfy the requested benefit's `BenefitRequestSchema` is refused with `ValidationException`.

Both symptoms share one root cause — the create never reads the benefit record beyond its id and programs — so they are one issue on this operation.

## Evidence

Spec: `smithy_query(shape="GetBenefitOutput", member="BenefitRequestSchema")` → "The schema definition that describes the required fields for requesting this benefit."; `BenefitStatus` is the enum `ACTIVE | INACTIVE` on the Benefit resource, and `CreateBenefitApplicationInput$BenefitIdentifier` names "the benefit being requested in this application".

Handler: services/.../src/ops/create_benefit_application.rs lines ~259-299 — `catalog.resolve_benefit_id(..)` (existence only; `Catalog::resolve_benefit_id` in src/state.rs ~1150 checks `benefits.contains_key`, not `status`), then only `benefit.programs` is read. Line ~201 is the sole check on the details document and it calls `crate::state::details_rejection` (bounds only); `benefit_request_schema` is never consulted by any handler (`grep benefit_request_schema` matches only the seeding code in src/state.rs ~1081).

State shows the emulator already models both facts: `SEEDED_BENEFITS` includes "Retired Innovation Sandbox" with `status: BENEFIT_STATUS_INACTIVE` (src/state.rs ~444-451), and `SeededBenefit::schema` is documented as "the field name and the JSON type a request for this benefit must put in `BenefitApplicationDetails`" (~398-400). `applicable_benefit_ids` (~1186-1192) even states the rule: "a benefit that is no longer offered cannot be applied for, so an INACTIVE one is not applicable" — yet the create path does not enforce it.

Test coverage: none. Every create test resolves its benefit through `Harness::active_benefit()` (tests/all/benefit_application_lifecycle.rs ~174), so no test ever creates against the seeded INACTIVE benefit, and no test sends `BenefitApplicationDetails` that violate a benefit's schema (the details tests only exercise size/depth bounds).

## Steps to Reproduce

1. Grant the partner entitlement for the emulator account (the gate every benefit operation applies) and start the emulator.
2. POST / with X-Amz-Target: PartnerCentralBenefitsService.ListBenefits and body {"Catalog":"AWS","Status":["INACTIVE"]} — note the Id of the inactive (retired) benefit it reports.
3. POST / with X-Amz-Target: PartnerCentralBenefitsService.CreateBenefitApplication and body {"Catalog":"AWS","ClientToken":"t1","BenefitIdentifier":"<that inactive benefit id>","Name":"demo"} — observe HTTP 200 and a minted benappl- id instead of a refusal for a benefit that is no longer offered.
4. GetBenefitApplication on that id: it reports BenefitId = the INACTIVE benefit, and SubmitBenefitApplication on it also succeeds.
5. Now take an ACTIVE benefit whose request schema declares required fields (e.g. the marketing-funds benefit, schema CampaignName:string / RequestedAmount:number) and call CreateBenefitApplication with {"Catalog":"AWS","ClientToken":"t2","BenefitIdentifier":"<that id>","BenefitApplicationDetails":{}} (or with RequestedAmount as a string).
6. Observe HTTP 200: the details are stored verbatim and GetBenefitApplication echoes them, with no ValidationException naming the fields the benefit's BenefitRequestSchema requires.

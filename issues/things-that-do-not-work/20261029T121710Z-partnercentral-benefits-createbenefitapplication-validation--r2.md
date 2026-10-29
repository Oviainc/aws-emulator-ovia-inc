Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 45e969ed3c6a96399c623276452bb212f0ee3327

# PartnerContacts[].Phone and FileDetails[].BusinessUseCase are persisted unbounded by create and update, while the amendment that writes the same two members refuses a value past this crate's own bound

_Filed: 2026-10-29T12:17:10Z_
_Operation: CreateBenefitApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `45e969ed3c6a96399c623276452bb212f0ee3327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model leaves two caller-supplied strings of a benefit application entirely unconstrained: `Contact$Phone` (target `ContactPhone`, a bare string with no `@length`) and `FileInput$BusinessUseCase` / `FileDetail$BusinessUseCase` (target `String`, no `@length`). This crate's documented doctrine for exactly that case is that the handler applies the bound itself through `state::length_clause` (see the `MAX_CLIENT_TOKEN_LEN`, `MAX_FILE_NAME_LEN`, `MAX_STATUS_REASON_LEN` and `MAX_DETAILS_VALUE_LEN` docs), and `AmendBenefitApplication` does exactly that for these two members: a `PartnerContacts[0].Phone` or `FileDetails[0].BusinessUseCase` longer than `MAX_DETAILS_VALUE_LEN` is refused 400 `ValidationException` and nothing is written.

`CreateBenefitApplication` and `UpdateBenefitApplication` write the SAME two stored members from their own input with no check at all. `CreateBenefitApplication` stores `input.partner_contacts` verbatim and maps each supplied `FileInput` through `state::file_detail`, which copies `business_use_case` verbatim; `UpdateBenefitApplication` does the same. So the identical value is a 200 on one path and a 400 on another, and the value a read reports (`GetBenefitApplication`) can be one that no amendment could ever have written.

Two consequences, both inside this crate's own claims:
1. The create's own comment asserts that everything but the details document is already covered — "Every string pattern and length on this input ... is a modelled constraint the centralized walk has already enforced. `BenefitApplicationDetails` is an unconstrained `Document`, so its bounds live here" — which is false for these two members: the walk enforces nothing on them, and no handler-side bound stands in.
2. `state.rs` claims the store is BOUNDED ("so no caller can grow this store without limit"), yet one create may carry 30 `FileDetails` (the modelled list max) each with an arbitrarily long `BusinessUseCase`, plus a `PartnerContacts` element with an arbitrarily long `Phone`, all persisted.

The fix is to run the two members through the bound and the refusal the crate already has (`state::length_clause` + `state::member_validation_exception`, as `amend_benefit_application::unconstrained_member_clause` does) on the two paths that supply them; no new constant is needed.

## Evidence

Smithy (centralized walk has nothing to enforce here):
- `smithy_query(query="shape",shape="Contact")` -> `"Phone": {"target": "ContactPhone", "sensitive": true}` and `smithy_query(query="shape",shape="ContactPhone")` -> `{"type": "string"}` — no `length`.
- `smithy_query(query="shape",shape="FileInput")` -> `"BusinessUseCase": {"target": "String"}`; `shape="FileDetail"` -> `"BusinessUseCase": {"target": "String"}` — no `length` on either side.

The sibling that DOES bound them — src/ops/amend_benefit_application.rs:208-219:
```
/// The member of a `Contact` the model constrains not at all: `Phone` targets
/// `ContactPhone`, which carries no `@length` ... A value written to it therefore
/// carries this service's own bound on a persisted caller value, exactly as a leaf of the
/// details document does ([`crate::state::MAX_DETAILS_VALUE_LEN`]), so no amendment can
/// grow one record without limit.
const CONTACT_UNCONSTRAINED_MEMBERS: &[&str] = &[MEMBER_PHONE];

/// The member of a `FileInput` the model constrains not at all, for the same reason:
/// `BusinessUseCase` targets a bare `String`
const FILE_UNCONSTRAINED_MEMBERS: &[&str] = &[MEMBER_BUSINESS_USE_CASE];
```
enforced at src/ops/amend_benefit_application.rs:399 (`crate::state::length_clause(value, 0, crate::state::MAX_DETAILS_VALUE_LEN)`) and called at :555-561 (contacts) and :590-596 (files).

The paths that skip it — src/ops/create_benefit_application.rs:233-237 (the claim):
```
// Every string pattern and length on this input, and the length of every list it
// carries, is a modelled constraint the centralized walk has already enforced.
// `BenefitApplicationDetails` is an unconstrained `Document`, so its bounds live
// here
```
and src/ops/create_benefit_application.rs:474-480 (the write):
```
partner_contacts: input.partner_contacts.unwrap_or_default(),
file_details: input
    .file_details
    .unwrap_or_default()
    .iter()
    .map(|file| crate::state::file_detail(file, &principal, now))
```
with src/state.rs:2578-2582 copying it verbatim: `business_use_case: input.business_use_case.clone(),`. `UpdateBenefitApplication` repeats both writes unchecked at src/ops/update_benefit_application.rs:422-440 (`application.partner_contacts = contacts;` / `crate::state::file_detail(file, &principal, now)`), and neither handler calls `length_clause` anywhere (`grep -n length_clause src/ops/` matches only amend and recall).

This crate's stated doctrine for an unconstrained persisted string — src/state.rs:356-363:
```
/// The longest value an amendment may write into `BenefitApplicationDetails`.
/// ... This is the matching bound for a leaf of
/// the document, which the model constrains not at all: the longest string member the
/// model lets a caller store on this resource.
pub const MAX_DETAILS_VALUE_LEN: usize = 1000;
```
and the bounded-store claim it breaks — src/state.rs:1515-1520: "BOUNDED: ... so no caller can grow this store without limit."

Test coverage proving the asymmetry: tests/all/amend_caller_supplied_members.rs:579-600 pins the amend refusals
```
("a phone one character past this service's bound", amendment("PartnerContacts[0].Phone", &"5".repeat(MAX_DETAILS_VALUE_LEN + 1)), clause(... "Member must have length less than or equal to {MAX_DETAILS_VALUE_LEN}")),
("a business use case one character past this service's bound", amendment("FileDetails[0].BusinessUseCase", &"u".repeat(MAX_DETAILS_VALUE_LEN + 1)), ...)
```
while tests/all/benefit_application_lifecycle.rs:524,531 and tests/all/write_that_changes_nothing.rs:60 only ever send short values on the create/update paths — no test pins create or update accepting an oversized one, so nothing in the crate justifies the divergence. (Precedent for the same class on this crate: the fixed issue "RecallBenefitApplication persists an unbounded ClientToken and Reason, breaking the bounds this crate documents and its siblings enforce".)

## Steps to Reproduce

1. Grant the partner entitlement for the emulator account (as tests/all/benefit_application_lifecycle.rs:90 does with `grant_partner_entitlement`).
2. Call CreateBenefitApplication(Catalog='Sandbox', ClientToken='t0', BenefitIdentifier=<seeded benefit id>, PartnerContacts=[{"Phone": <1001 '5' characters>}], FileDetails=[{"FileURI":"s3://bucket/a.csv","BusinessUseCase": <1001 'u' characters>}]).
3. Observe HTTP 200 and note the returned Id/Revision.
4. Call GetBenefitApplication(Catalog='Sandbox', Identifier=<Id>) and observe the 1001-character Phone and BusinessUseCase reported back out of the store.
5. Call AmendBenefitApplication(Catalog='Sandbox', ClientToken='t1', Identifier=<Id>, Revision=<Revision>, AmendmentReason='r', Amendments=[{FieldPath:'PartnerContacts[0].Phone', NewValue: <the same 1001 characters>}]) and observe 400 ValidationException 'Member must have length less than or equal to 1000' — the same value the create just stored is refused on the amendment path.
6. Repeat step 5 with FieldPath 'FileDetails[0].BusinessUseCase' for the same refusal, and repeat step 2's values through UpdateBenefitApplication to see the unchecked write a second time.

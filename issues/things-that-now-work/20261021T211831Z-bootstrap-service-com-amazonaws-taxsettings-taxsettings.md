Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the TaxSettings emulator

_Filed: 2026-10-21T21:18:31Z_
_Operation: service-com-amazonaws-taxsettings-taxsettings | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 16 TaxSettings operations is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state. Implement a first, small, stateful slice covering the tax-registration (TRN) lifecycle of the primary resource — the per-account tax registration — so a realistic client script works end to end.

### Happy path to implement (in order)

1. **PutTaxRegistration** (`POST /PutTaxRegistration`) — input `accountId` (string, exactly 12 digits, `^\d+$`; defaults to the caller's account when omitted) and required `taxRegistrationEntry` (`TaxRegistrationEntry`: required `registrationId`, required `registrationType` (VAT|GST), `legalName`, `legalAddress`, `sector`, `additionalTaxInformation`, `verificationDetails`, `certifiedEmailId`). Must STORE the entry keyed by account id and return `status` (`TaxRegistrationStatus`: Verified|Pending|Rejected|Deleted) — pick a deterministic status (e.g. `Verified`) and persist it; the same value must be observable from the reads below.
2. **GetTaxRegistration** (`POST /GetTaxRegistration`) — input `accountId`; returns `taxRegistration` (`TaxRegistration`: required `registrationId`, `registrationType`, `legalName`, `status`, `legalAddress`, plus optional `sector`, `certifiedEmailId`, `additionalTaxInformation`, `taxDocumentMetadatas`) reconstructed from what PutTaxRegistration stored. An account with no stored registration must fail with `ResourceNotFoundException` (HTTP 404), not an empty 200.
3. **ListTaxRegistrations** (`POST /ListTaxRegistrations`) — `@paginated(inputToken=nextToken, outputToken=nextToken, items=accountDetails)`; input `maxResults` (1..1000) and `nextToken` (1..2000, `^[-A-Za-z0-9_+\=\/]+$`); required output `accountDetails` (list of `AccountDetails`: `accountId`, `taxRegistration` as `TaxRegistrationWithJurisdiction`, `taxInheritanceDetails`, `accountMetaData`) plus `nextToken`. Must enumerate ALL stored registrations, honour `maxResults` as a page size, and return a `nextToken` that resumes exactly where the page stopped (absent on the last page).
4. **DeleteTaxRegistration** (`POST /DeleteTaxRegistration`) — input `accountId`, empty output. Must remove the stored registration so the following GetTaxRegistration returns `ResourceNotFoundException` and ListTaxRegistrations no longer includes the account. Deleting an unknown account returns `ResourceNotFoundException` (404).
5. **PutTaxInheritance / GetTaxInheritance** (`POST /PutTaxInheritance`, `POST /GetTaxInheritance`) — Put takes `heritageStatus` (`OptIn`|`OptOut`) and has an empty output; Get takes no input and must return the LAST value stored by Put (not a constant). This pair is the cheapest proof that state really round-trips.

All six must compute their responses from persisted state. No canned bodies, no echoing the request back without storing it, no generated identifier that is not saved. Input constraints from the model must be enforced with `ValidationException` (HTTP 400): `accountId` length 12 and `^\d+$`, `registrationId`/`legalName` length 1..200 and non-blank `^(?!\s*$)[\s\S]+$`, `maxResults` range 1..1000, `nextToken` length/pattern, `certifiedEmailId` email pattern, and missing required members (`taxRegistrationEntry`, `registrationId`, `registrationType`).

## State Model

In `src/state.rs` (`ComAmazonawsTaxsettingsTaxsettingsState`, reached via `crate::state::service_state(&*self.engine)`):

- `registrations: BTreeMap<String /* accountId (12 digits) */, StoredTaxRegistration>` — BTreeMap so ListTaxRegistrations has a stable, deterministic order for paging.
  - `StoredTaxRegistration { registration_id: String, registration_type: TaxRegistrationType /* VAT|GST */, legal_name: Option<String>, legal_address: Option<Address>, sector: Option<Sector>, additional_tax_information: Option<AdditionalInfoResponse>, verification_details: Option<VerificationDetails>, certified_email_id: Option<String>, status: TaxRegistrationStatus, tax_document_metadatas: Vec<TaxDocumentMetadata> }`
- `tax_inheritance: Option<HeritageStatus>` — set by PutTaxInheritance, read by GetTaxInheritance (`None` ⇒ return the model's absent/`OptOut` default consistently).
- `account_metadata: BTreeMap<String /* accountId */, AccountMetaData>` — optional seed data so `AccountDetails.accountMetaData` is populated rather than null.
- Pagination: `nextToken` is an opaque, base64url-safe cursor (matching `^[-A-Za-z0-9_+\=\/]+$`) encoding the last emitted `accountId`; resuming means "first key strictly greater than the decoded accountId". A token that does not decode ⇒ `ValidationException`.

## Out of scope for this first slice

BatchPutTaxRegistration, BatchDeleteTaxRegistration, PutTaxExemption, BatchGetTaxExemptions, ListTaxExemptions, GetTaxExemptionTypes, Put/Delete/ListSupplementalTaxRegistrations and GetTaxRegistrationDocument stay at 501 and will be filed as individual gap issues later. (Designing `registrations` as a map keyed by accountId makes the batch variants trivial fan-outs afterwards, and a separate `supplemental: BTreeMap<authorityId, SupplementalTaxRegistration>` is the natural follow-on.)

## Done when

A single client session can PutTaxRegistration → GetTaxRegistration (sees exactly what was written, with the stored `status`) → ListTaxRegistrations (the account appears, paging with `maxResults=1` works) → DeleteTaxRegistration → GetTaxRegistration (404 `ResourceNotFoundException`), and PutTaxInheritance(OptIn) → GetTaxInheritance returns `OptIn`.

## Evidence

`services/service-com-amazonaws-taxsettings-taxsettings/OPERATIONS.md` lists all 16 operations with routes, input/output members and errors: PutTaxRegistration (`POST /PutTaxRegistration`, input `accountId` len 12..12 pattern `^\d+$` + required `taxRegistrationEntry`, output `status`), GetTaxRegistration (output `taxRegistration`, errors incl. `ResourceNotFoundException (HTTP 404)`), ListTaxRegistrations (`@paginated(inputToken=nextToken, outputToken=nextToken, items=accountDetails)`, required output `accountDetails`), DeleteTaxRegistration (no output members, `ResourceNotFoundException`), PutTaxInheritance/GetTaxInheritance (`heritageStatus`: OptIn|OptOut). Member-level constraints quoted above come from `smithy_query` on `TaxRegistrationEntry`, `TaxRegistration`, `AccountDetails` in `smithy-model/com-amazonaws-taxsettings-taxsettings.json` (e.g. `TaxRegistration` requires `registrationId`, `registrationType`, `legalName`, `status`, `legalAddress`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-taxsettings-taxsettings/src/ops/` matches all 16 handler files, so every operation currently returns HTTP 501 and no state exists. README.md confirms the wire protocol is restJson1 and that handlers must reach the shared store via `self.engine`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `tax` (TaxSettings) client at it.
2. Call PutTaxRegistration with accountId="000000000001" and taxRegistrationEntry={registrationId:"VAT-123", registrationType:"VAT", legalName:"Acme GmbH", legalAddress:{...countryCode:"DE"...}} — expect 200 with a `status`; actual: HTTP 501 NotImplemented.
3. Call GetTaxRegistration with accountId="000000000001" — expect 200 whose `taxRegistration` carries registrationId "VAT-123", registrationType VAT, legalName "Acme GmbH" and the same `status` returned by the Put; actual: 501.
4. Call ListTaxRegistrations with maxResults=1 — expect the account in `accountDetails` and, if more accounts exist, a `nextToken` that returns the remaining ones on the next call; actual: 501.
5. Call PutTaxInheritance with heritageStatus="OptIn", then GetTaxInheritance — expect `heritageStatus` = "OptIn"; actual: 501 on both.
6. Call DeleteTaxRegistration with accountId="000000000001", then GetTaxRegistration for the same account — expect ResourceNotFoundException (HTTP 404) and the account gone from ListTaxRegistrations; actual: 501.
7. Call GetTaxRegistration with accountId="abc" — expect ValidationException (HTTP 400) for the 12-digit `^\d+$` constraint; actual: 501.

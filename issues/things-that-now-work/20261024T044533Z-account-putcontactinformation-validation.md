Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ae5324a422b43795ea9373a6578734529d7c3346

# PutContactInformation accepts a primary contact with no StateOrRegion in the seven countries that require it, and never validates CountryCode as an ISO-3166 code

_Filed: 2026-10-24T04:45:33Z_
_Operation: PutContactInformation | Class: validation_

> _Note: this issue was identified against the code base at commit `ae5324a422b43795ea9373a6578734529d7c3346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler stores whatever `ContactInformation` structure passes the modelled boundary constraints and applies none of the service's own documented rules on that structure.

Two rules are missing:

1. `StateOrRegion` is optional in the model, but the model's OWN documentation for that member states: "This field is required in the following countries: US, CA, GB, DE, JP, IN, and BR." Real AWS refuses a `PutContactInformation` whose `CountryCode` is one of those seven and whose `StateOrRegion` is absent with HTTP 400 `ValidationException`. The emulator answers HTTP 200 and stores the structure, and `GetContactInformation` then reports a US address with no state.

2. `CountryCode` is documented as "The ISO-3166 two-letter country code" and modelled only `@length(min: 2, max: 2)`. WHICH two-letter strings are country codes is service data (exactly as WHICH strings are Region codes is service data — the same crate already enforces that itself for `RegionName` via `state::REGION_CATALOG` / `state::unknown_region`). The emulator accepts an unassigned code such as `ZZ` or `QQ` with HTTP 200, where real AWS refuses it with `ValidationException`.

Expected: 400 `ValidationException` (the modelled error for this operation) for a missing `StateOrRegion` under `CountryCode` in {US, CA, GB, DE, JP, IN, BR}, and for a `CountryCode` that is not an ISO-3166 alpha-2 code. Actual: 200 and the contact is persisted.

## Evidence

Spec: Smithy model shape `ContactInformation` — member `StateOrRegion` doc: "The state or region of the primary contact address. ... This field is required in the following countries: US, CA, GB, DE, JP, IN, and BR."; member `CountryCode` doc: "The ISO-3166 two-letter country code for the primary contact address." `PutContactInformation` models `ValidationException` (HTTP 400).

Handler: services/service-com-amazonaws-account-account/src/ops/put_contact_information.rs:185-201 — the only comment is "Every modelled constraint on this input ... was already refused at the boundary, so what reaches here is a structure this service can store as it stands"; `account.contact_information = Some(input.contact_information)` with no country/state cross-check.

Tests: services/service-com-amazonaws-account-account/tests/all/contact_information.rs:134-143 `required_only()` builds `CountryCode: "US"` with NO `StateOrRegion` and the test `a_stored_contact_information_round_trips_member_for_member` asserts it is stored and read back with 200 — i.e. the current suite pins the divergent behaviour; no test exercises an invalid country code.

## Steps to Reproduce

1. Boot the emulator and point an account-service client at it.
2. Call PutContactInformation with ContactInformation = {FullName, AddressLine1, City, PostalCode, CountryCode: "US", PhoneNumber: "+1 206-555-0100"} and no StateOrRegion.
3. Observe HTTP 200; real AWS refuses with HTTP 400 ValidationException because StateOrRegion is required for CountryCode US.
4. Call GetContactInformation — the emulator reports the stored US address with StateOrRegion absent.
5. Call PutContactInformation again with CountryCode "ZZ" (not an assigned ISO-3166 alpha-2 code) and observe HTTP 200 instead of 400 ValidationException.

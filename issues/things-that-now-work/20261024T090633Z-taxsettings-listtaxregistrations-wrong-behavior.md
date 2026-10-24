Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# ListTaxRegistrations never reports AccountDetails.accountMetaData, so listed accounts carry no legal profile

_Filed: 2026-10-24T09:06:33Z_
_Operation: ListTaxRegistrations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AccountDetails` has four members: `accountId`, `taxRegistration`, `taxInheritanceDetails` and `accountMetaData` ("The meta data information associated with the account", carrying `accountName`, `seller`, `address`, `addressType` and `addressRoleMap`). AWS populates `accountMetaData` for the accounts it enumerates — it is how a caller (and the Billing console's tax-settings table) gets the account's name, its address and the address roles alongside the TRN; the listing exists to report the account's tax profile, not the TRN alone.

Expected: each entry of `accountDetails` carries `accountMetaData` with at least the members the service can report for the account (account name, the account's address and its `addressType`/`addressRoleMap`), computed from stored state.

Actual: the handler hardcodes both extra members to `None`: `src/ops/list_tax_registrations.rs` lines 198-208 builds `AccountDetails { account_id, tax_registration, tax_inheritance_details: None, account_meta_data: None }`. No `AccountMetaData` is ever constructed anywhere in the crate, and no state field exists for it, so every listed account comes back with the member absent on the wire regardless of what was stored — even though the write persisted a `legalName` and a full `legalAddress` the metadata could be projected from. A client that reads `accountDetails[].accountMetaData.accountName` or `.address` gets nothing against the emulator and a populated object against AWS.

## Evidence

Spec: `smithy_query(query="shape", shape="AccountDetails")` — members `taxInheritanceDetails` and `accountMetaData` (target `AccountMetaData` with `accountName`, `seller`, `address`, `addressType`, `addressRoleMap`); `ListTaxRegistrations` output `accountDetails` (list<AccountDetails>, required) is the only shape that carries them, so if this operation never populates them the members are unreachable in the whole emulator.

Handler: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/list_tax_registrations.rs` lines 195-209 — `tax_inheritance_details: None, account_meta_data: None` with the comment "Neither is written by any operation this service models, so there is nothing stored to report". `grep -rn "AccountMetaData" src/` finds the type only in the generated `ops/types` shapes, never constructed.

Tests: `tests/all/tax_registration_lifecycle.rs::list_reports_the_same_registration_reduced_to_its_jurisdiction` (line 423) asserts only `accountId` and the `taxRegistration`/`jurisdiction` projection; nothing asserts the presence or absence of `accountMetaData`, so no test pins the current behavior.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE123456789","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"}}} — observe 200.
2. POST /ListTaxRegistrations with {}.
3. Inspect `accountDetails[0]`: it carries only `accountId` and `taxRegistration`. `accountMetaData` is absent, so `accountDetails[0].accountMetaData.accountName` / `.address` / `.addressType` / `.addressRoleMap` are unreadable, where AWS reports the account's legal profile alongside the TRN.

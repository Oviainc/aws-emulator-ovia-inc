Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# GetTaxExemptionTypes publishes an invented exemption-type catalog, so its body diverges from AWS and PutTaxExemption only accepts the invented strings

_Filed: 2026-10-24T20:08:59Z_
_Operation: GetTaxExemptionTypes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `GetTaxExemptionTypes` answers "the supported types of tax exemptions" — a collection the real service publishes. Its `displayName`/`description` values are the tokens a client passes back as `PutTaxExemptionRequest$exemptionType` ("The exemption type. Use the supported tax exemption type description"), so both the set of entries and the exact wording of each entry are observable, client-visible API data that must match what AWS returns.

Actual: the response is a hand-written table of seven rows whose wording the emulator invents, and the crate says so in as many words: `TAX_EXEMPTION_TYPE_CATALOG`'s doc comment states "What is NOT claimed: the `displayName` and `description` WORDING, and the order of the entries ... each is this emulator's own phrasing of a documented category". The rows are `501(c)(3)`, `Direct Pay Permit`, `Educational Institution`, `Federal Government`, `Reseller`, `State or Local Government`, `Tribal Government`, each with `description` sentences composed for this emulator and a single `applicableJurisdictions` entry of `{"country":"US"}` with no `state`.

Two observable divergences follow:

1. The `GetTaxExemptionTypes` response body differs from AWS's published catalog entry-for-entry (count, order, `displayName`, `description`, and the jurisdictions each type carries), so any client that renders or matches on this list sees different data than against AWS.
2. `PutTaxExemption` admits an `exemptionType` only when it equals one of these invented `description`/`displayName` strings (`crate::state::check_exemption_type` -> `find_exemption_type`), and refuses everything else with 400 `ValidationException` `FieldValidationFailed`. A client that passes a real AWS exemption-type description — including one copied from an AWS recording or from AWS's own documentation — is refused by the emulator, even though the submission is valid against the real service. Likewise `check_exemption_authority` derives its jurisdiction admission from the invented `{"country":"US", state: None}` rows, so which `authority` values a submission may name is also derived from fabricated data.

Fix direction: replace the catalog with the collection AWS actually publishes (a `GetTaxExemptionTypes` recording is the authority for the entry set, ordering and exact `displayName`/`description` text, plus each type's real `applicableJurisdictions`), and keep `PutTaxExemption`'s accepted `exemptionType` tokens derived from that same table.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.taxsettings#GetTaxExemptionTypes")` — the operation reports the service's supported tax exemption types; `smithy_query(query="doc", shape="PutTaxExemptionRequest", member="exemptionType")` -> "The exemption type. Use the supported tax exemption type description.", which makes a catalog entry's `description` a wire token a client must be able to send; `TaxExemptionType{displayName (0..50), description (1..200), applicableJurisdictions}` are all client-visible members.

Handler/state: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/get_tax_exemption_types.rs` `handle()` lines ~131-153 returns `crate::state::tax_exemption_type_catalog()` and reads no stored state. `src/state.rs` `TAX_EXEMPTION_TYPE_CATALOG` (approx. lines 1887-1949) is the hand-written table, with the doc comment conceding the wording and ordering are the emulator's own; `find_exemption_type()` (approx. line 1999) and `check_exemption_type()` (approx. line 2016) gate `PutTaxExemption` on exactly those strings; `check_exemption_authority()` (approx. line 2039) gates `authority` on the same rows' `UNITED_STATES` jurisdiction.

Tests: `tests/all/tax_exemption_types.rs` pins the invented wire form on purpose — its `pinned_wire_form` table (lines ~75-142) is "Written out here rather than read from `state::TAX_EXEMPTION_TYPE_CATALOG`" and `the_whole_catalog_is_reported_entry_for_entry` asserts the response equals it. So the current (admittedly unverified) values are asserted-in rather than untested: a fixer replacing the catalog must update that pinned table and `the_documented_categories_are_published`.

## Steps to Reproduce

1. POST /GetTaxExemptionTypes with {} and observe 200 with exactly seven taxExemptionTypes whose displayName values are 501(c)(3), Direct Pay Permit, Educational Institution, Federal Government, Reseller, State or Local Government, Tribal Government, and whose description sentences are prose composed for the emulator; compare entry-for-entry with a real AWS GetTaxExemptionTypes response.
2. Take an exemption-type description as the real service words it (for example from an AWS GetTaxExemptionTypes recording) and POST /PutTaxExemption with {"accountIds":["000000000000"],"authority":{"country":"US"},"exemptionType":"<that description>","exemptionCertificate":{"documentName":"resale.pdf","documentFile":"<base64 bytes>"}}.
3. Observe 400 ValidationException with errorCode FieldValidationFailed and fieldList [{"name":"EXEMPTION_TYPE"}] — the emulator refuses a type the real service supports, because only its own invented strings are admitted.
4. Repeat the submission with the emulator's own string (e.g. "Reseller") and observe 200, confirming the accepted token set is the fabricated catalog rather than AWS's.

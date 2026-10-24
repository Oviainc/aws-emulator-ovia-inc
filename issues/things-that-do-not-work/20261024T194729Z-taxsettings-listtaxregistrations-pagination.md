Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# ListTaxRegistrations ignores the documented page bounds: default page is 100 (AWS: 50) and maxResults above 100 is honored verbatim up to 1000

_Filed: 2026-10-24T19:47:29Z_
_Operation: ListTaxRegistrations | Class: pagination_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own model documentation states the service's page bounds: "Retrieves the tax registration of accounts listed in a consolidated billing family. This can be used to retrieve up to 100 accounts' tax registrations in one call (default 50)."

Expected: a request that names no `maxResults` returns at most 50 entries in `accountDetails`, plus a `nextToken` when more accounts remain; a request that names a `maxResults` larger than 100 (the shared `MaxResults` shape admits 1..1000, so 1000 is wire-valid) is still served a page of at most 100 entries plus a `nextToken`.

Actual: the emulator substitutes its own bounds. `crate::state::page_size()` falls back to `DEFAULT_PAGE_SIZE = 100` when the request names no page size, and clamps to `MAX_PAGE_SIZE = 1_000`. So (a) a caller that names no `maxResults` gets up to 100 accounts in one page where AWS returns 50 and a cursor, and (b) a caller that asks for `maxResults: 1000` gets up to 1000 accounts in a single response where AWS caps the page at 100 and hands back a cursor. A client that walks pages sees a different page boundary and a different number of round-trips than against AWS, and a client that was relying on the documented 100-item ceiling to size its buffers receives ten times as many entries in one body.

Note the sibling listings are not affected by the same sentence: `ListTaxExemptions` and `ListSupplementalTaxRegistrations` document no page bound, so only `ListTaxRegistrations` has a spec-stated default/cap to honor (a fix therefore wants per-operation bounds rather than one shared pair of constants).

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.taxsettings#ListTaxRegistrations")` -> "This can be used to retrieve up to 100 accounts' tax registrations in one call (default 50)." `ListTaxRegistrationsRequest$maxResults` is documented only as "Number of accountDetails results you want in one response" and targets the shared `MaxResults` (range 1..1000), so the 100/50 figures are the service-side bounds, not request constraints.

Handler/state: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/list_tax_registrations.rs` line ~173 calls `crate::state::page_size(input.max_results)`; `src/state.rs` lines ~169-184 define `DEFAULT_PAGE_SIZE: usize = 100` ("AWS documents no default page size for this operation, so this is the emulator's own" — the operation documentation does state one) and `MAX_PAGE_SIZE: usize = 1_000`, and `page_size()` (line ~214) applies `unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)`.

Tests: `tests/all/tax_registration_lifecycle.rs::paging_emits_every_registration_exactly_once_in_order` computes its expected page bound from the same `DEFAULT_PAGE_SIZE`/`MAX_PAGE_SIZE` constants (`bound = page_size.unwrap_or(DEFAULT_PAGE_SIZE).min(MAX_PAGE_SIZE)`), so nothing pins the numeric values 100/1000 — no test asserts the current (divergent) bounds, and no test asserts the documented 50/100 either.

## Steps to Reproduce

1. Seed 200 registrations: for i in 1..=200, POST /PutTaxRegistration with {"accountId": "<12-digit i>", "taxRegistrationEntry": {"registrationId": "VAT-i", "registrationType": "VAT", "legalName": "Account i Ltd", "legalAddress": {"postalCode": "10115", "countryCode": "DE"}}}.
2. POST /ListTaxRegistrations with {} (no maxResults): observe 100 entries in accountDetails; AWS returns 50 and a nextToken.
3. POST /ListTaxRegistrations with {"maxResults": 1000}: observe all 200 entries in one response with no nextToken; AWS caps the page at 100 entries and returns a nextToken.
4. Walk the cursor in both cases and count round-trips: the emulator takes 2 pages (no maxResults) / 1 page (maxResults 1000) where AWS takes 4 / 2.

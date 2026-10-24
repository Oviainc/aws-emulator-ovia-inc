Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# ListRegions accepts RegionOptStatusContains values the RegionOptStatus enum does not admit and answers 200 with an empty list

_Filed: 2026-10-24T06:56:28Z_
_Operation: ListRegions | Class: validation_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegionOptStatusContains` is modelled as `RegionOptStatusList`, a list whose member targets the `RegionOptStatus` enum (`ENABLED | ENABLING | DISABLING | DISABLED | ENABLED_BY_DEFAULT`). The handler never checks the supplied values against that value set: it only uses them as string equality predicates against the status each Region reports (`wanted.is_none_or(|statuses| statuses.iter().any(|value| value == status))`).

So a request carrying a value the enum does not admit — `"BOGUS"`, or a correctly-spelled status in the wrong case such as `"enabled"` — is served with HTTP 200 and `"Regions": []`, as if the account simply had no Region in that state. Real AWS refuses an out-of-enum member of an input list with HTTP 400 `ValidationException` (the error this operation models), naming the offending list element; it never reports an empty result set for a value that is not a status at all.

The divergence is also internally inconsistent: this same crate enforces the *other* string enum in the service by hand, because the request boundary does not validate enum membership — `AlternateContactType::from_wire` plus `state::invalid_alternate_contact_type()` refuses `"MARKETING"` and `"billing"` with a 400 `ValidationException` carrying `reason = fieldValidationFailed` and a `fieldList` entry. `ListRegions` has no equivalent check for `RegionOptStatusContains`.

Expected: a `RegionOptStatusContains` element outside the five modelled `RegionOptStatus` values → HTTP 400 `ValidationException` (reason `fieldValidationFailed`, `fieldList` naming `RegionOptStatusContains`), nothing listed. Actual: HTTP 200 with an empty `Regions` list, which a client cannot distinguish from "no Region currently has that status".

## Evidence

Spec: `smithy_query(query="shape", shape="RegionOptStatusList")` → `{"type":"list","member":"RegionOptStatus"}` and `smithy_query(query="shape", shape="RegionOptStatus")` → values `[ENABLED, ENABLING, DISABLING, DISABLED, ENABLED_BY_DEFAULT]`; `ListRegions` models `ValidationException` (HTTP 400).

Handler: services/service-com-amazonaws-account-account/src/ops/list_regions.rs:254-273 — `let wanted = input.region_opt_status_contains.as_deref();` then `.filter(|(_, status)| wanted.is_none_or(|statuses| statuses.iter().any(|value| value == status)))`. The only input refusal this handler performs is the `NextToken` check (`invalid_next_token()`, lines 119-139); nothing inspects the filter's values.

Proof the boundary does not validate enums: services/service-com-amazonaws-account-account/src/ops/get_alternate_contact.rs:172-179, put_alternate_contact.rs:199-206 and delete_alternate_contact.rs:191-198 all validate `AlternateContactType` in the handler via `state::AlternateContactType::from_wire` / `state::invalid_alternate_contact_type` (state.rs:650-666), and tests/all/account_settings.rs:738-748 asserts `"MARKETING"` and `"billing"` are refused — so enum membership is a handler responsibility in this crate.

Tests: tests/all/region_opt_in_listing.rs:422-491 `the_filter_keeps_exactly_the_listed_statuses` exercises only admitted statuses (plus a duplicate and an empty list); grep shows no test passes an out-of-enum value to `RegionOptStatusContains`, so nothing asserts either behaviour today.

## Steps to Reproduce

1. Boot the emulator and POST /listRegions with body {} — observe HTTP 200 and the full Region list with statuses.
2. POST /listRegions with body {"RegionOptStatusContains": ["BOGUS"]}.
3. Observe HTTP 200 and "Regions": []; real AWS answers HTTP 400 ValidationException because BOGUS is not a member of the RegionOptStatus value set.
4. POST /listRegions with body {"RegionOptStatusContains": ["enabled"]} (correct status, wrong case) and observe the same silent empty 200.
5. Compare POST /getAlternateContact with {"AlternateContactType": "billing"}, which this same service already refuses with HTTP 400 ValidationException — the two enum-valued inputs are treated inconsistently.

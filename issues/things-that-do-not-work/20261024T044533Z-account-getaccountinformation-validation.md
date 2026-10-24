Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ae5324a422b43795ea9373a6578734529d7c3346

# An AccountId equal to the caller's own account is served instead of refused with AccessDeniedException

_Filed: 2026-10-24T04:45:33Z_
_Operation: GetAccountInformation | Class: validation_

> _Note: this issue was identified against the code base at commit `ae5324a422b43795ea9373a6578734529d7c3346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation here that takes an OPTIONAL `AccountId` resolves it through `state::resolve_account_id`, which treats "no AccountId" and "AccountId == the caller's own account id" as the same request landing on the same record. So GetAccountInformation(AccountId=<own id>) answers 200.

The model's `AccountId` documentation states the opposite rule explicitly: "The management account can't specify its own AccountId; it must call the operation in standalone context by not including the AccountId parameter", and "To call this operation on an account that is not a member of an organization, then don't specify this parameter." Real AWS therefore refuses a self-naming request with HTTP 403 AccessDeniedException. The emulator already implements exactly this rule on one operation — StartPrimaryEmailUpdate refuses `input.account_id == state::default_account_id()` with `state::cannot_target_own_account()` — so the service is internally inconsistent: the same guardrail is enforced on one operation and ignored on the twelve others that carry an optional `AccountId` (GetAccountInformation, PutAccountName, GetAlternateContact, PutAlternateContact, DeleteAlternateContact, GetContactInformation, PutContactInformation, GetPrimaryEmailUpdateStatus, GetRegionOptStatus, ListRegions, EnableRegion, DisableRegion).

Expected: naming one's own 12-digit account id in `AccountId` → 403 AccessDeniedException (the same refusal StartPrimaryEmailUpdate already gives), while omitting the member keeps working. Actual: 200, the call lands on the caller's own record.

## Evidence

Spec: `smithy_query(query="doc", shape="GetAccountInformationRequest", member="AccountId")` — "The management account can't specify its own AccountId; it must call the operation in standalone context by not including the AccountId parameter." `GetAccountInformation` models `AccessDeniedException` (403).

Handler: services/service-com-amazonaws-account-account/src/ops/get_account_information.rs:178 — `crate::state::resolve_account_id(input.account_id.as_deref())` with no self-target check; services/service-com-amazonaws-account-account/src/state.rs documents the choice ("a standalone call and a management-account call that names that same id land on ONE record") and already carries the correct refusal helper `cannot_target_own_account()`, used only by start_primary_email_update.rs:181-186.

Tests: tests/all/account_settings.rs:514 `the_resolved_account_id_keys_the_record` and tests/all/region_opt_in.rs pin the current (divergent) behaviour that naming one's own id is accepted; no test asserts the documented refusal outside StartPrimaryEmailUpdate.

## Steps to Reproduce

1. Boot the emulator and read the caller's own account id: GetAccountInformation with an empty body returns AccountId (e.g. 000000000000).
2. Call GetAccountInformation again with AccountId=<that same id>.
3. Observe HTTP 200 with the account's information; real AWS answers 403 AccessDeniedException because the management account can't specify its own AccountId.
4. Compare StartPrimaryEmailUpdate with AccountId=<that same id>, which the emulator already refuses with 403 AccessDeniedException — the two operations apply opposite rules to the same input.

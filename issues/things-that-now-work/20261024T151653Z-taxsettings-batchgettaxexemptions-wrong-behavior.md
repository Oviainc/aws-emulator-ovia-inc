Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# BatchGetTaxExemptions fails the whole call with 404 for an unreadable account and never populates failedAccounts

_Filed: 2026-10-24T15:16:53Z_
_Operation: BatchGetTaxExemptions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: BatchGetTaxExemptions is a batch read with a per-account failure channel. `BatchGetTaxExemptionsResponse$failedAccounts` is modeled as "The list of accounts that failed to get tax exemptions", so an account in the request that cannot be reported on (outside the caller's consolidated billing family / no tax-settings resource) is expected to come back inside `failedAccounts` on a 200 response, with the remaining accounts still reported in `taxExemptionDetailsMap`.

Actual: the handler pre-screens every requested account with `store.reports_exemptions_for(..)` and returns a call-level `ResourceNotFoundException` (HTTP 404) as soon as ONE of the up-to-5 accounts is not known to the emulator — the other accounts' exemptions are not reported at all. On the success path `failed_accounts` is hard-coded to `Some(Vec::new())`, so the modeled member can never carry a value: it is dead weight in every response. The result is that a client batching 5 account ids, one of which is not in the emulator's store, gets no data for any of them instead of a partial result plus a failure list.

## Evidence

Spec: `smithy_query(query="shape", shape="com.amazonaws.taxsettings#BatchGetTaxExemptionsResponse")` shows `failedAccounts` (target `AccountIds`) documented as "The list of accounts that failed to get tax exemptions" — a per-account failure channel that only makes sense if a partially-failing batch answers 200. The request's `accountIds` is bounded 1..=5, i.e. explicitly a multi-account read.

Handler: `src/ops/batch_get_tax_exemptions.rs` `handle()` lines ~176-183 (`return Err(... ResourceNotFoundException ...)` inside the per-account loop, before any mapping) and lines ~198-205 (`failed_accounts: Some(Vec::new())` with a comment stating "Nothing can fail on its own").

Tests: `tests/all/tax_exemptions.rs` (~lines 330-377) pins exactly this behavior — the whole-call refusal for an unknown account and `failedAccounts == []` on every 200 — so the current semantics are asserted, not accidental; no test covers a partial batch result.

## Steps to Reproduce

1. POST /PutTaxRegistration for the emulator's own account so it is known to the store, then POST /PutTaxExemption for that account with a published exemptionType (e.g. "Reseller", authority {"country":"US"}) — observe 200 and a caseId.
2. POST /BatchGetTaxExemptions with {"accountIds":["<emulator account id>","999999999999"]}.
3. Observe HTTP 404 ResourceNotFoundException for the entire call: the exemption recorded for the first (valid, readable) account is not reported.
4. POST /BatchGetTaxExemptions with only the readable account and observe 200 with "failedAccounts": [] — the member is always empty, so no request can ever produce a non-empty failure list.

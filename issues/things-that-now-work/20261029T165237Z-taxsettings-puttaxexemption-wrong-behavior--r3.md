Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a399f03ccc4f5afa40c0af0ce737308e8fb96c4f

# PutTaxExemption's access record counts raw accountIds entries, contradicting this crate's own "an id named twice names one account" rule

_Filed: 2026-10-29T16:52:37Z_
_Operation: PutTaxExemption | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a399f03ccc4f5afa40c0af0ce737308e8fb96c4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutTaxExemption writes exactly one exemption record per DISTINCT account id (the write goes through `distinct_account_ids`, and the crate states that rule for the whole service), but the access record the handler emits for the call reports `accounts = input.account_ids.len()` — the number of LIST ENTRIES, not the number of accounts the submission addressed.

So a submission naming ["000000000001", "000000000001", "000000000002"] writes 2 records and opens 1 case, while the audit line for that same call says `accounts = 3` and claims to be "the accounts a submission named" — a count the crate's own rule says is 2, because "an id named more than once names the SAME account". The handler computes the count from the raw request instead of from the same `distinct_account_ids` projection the write it is logging used, which is precisely the drift that projection exists to prevent.

No wire field is affected (the response carries only `caseId`), so the impact is confined to the operator-facing access record: it overstates how many accounts a state-changing call changed. Fix: take the count from the distinct projection the write addresses (or have the write report how many accounts it wrote for) so the record cannot disagree with the store.

## Evidence

The write addresses each id once — state.rs:1007-1009 (`store_exemptions` doc):
  "/// The accounts the submission addresses are the ids it names, each once\n/// ([`distinct_account_ids`]): one record per addressed account, whatever\n/// number of times the list repeats its id."
and state.rs:1034 does exactly that: `let distinct = distinct_account_ids(account_ids);` … state.rs:1057 `for account_id in distinct {`.

The rule is stated once for the whole service — state.rs:1684-1700 (`distinct_account_ids` doc):
  "/// Every `accountIds` member of this service is documented \"List of unique\n/// account identifiers\", so an id named more than once names the SAME account\n/// and the SAME single action on it" … "every operation whose input carries an `accountIds` list addresses its accounts through here, as does [`ComAmazonawsTaxsettingsTaxsettingsState::store_exemptions`], the write one of them performs, so none of them can disagree about what a repeated id means."

The handler disagrees — services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_exemption.rs:235-248:
  "        let accounts = input.account_ids.len();"
  "        // The access record for a state-changing call: which operation ran, what\n        // it changed and how it ended."
  "        ::tracing::info!(\n            operation = \"PutTaxExemption\",\n            outcome = \"recorded\",\n            accounts," … "            \"recorded a tax exemption for the accounts a submission named\"\n        );"

Test coverage that pins the store side (and leaves the record side unpinned): tests/all/repeated_account_ids.rs:294-301 asserts `records == distinct.len()` ("expected one exemption record per distinct account"); no test asserts the logged `accounts` field.

## Steps to Reproduce

1. Mount the emulator with tracing at info level.
2. Call PutTaxExemption with accountIds = ["000000000001", "000000000001", "000000000002"], authority = {"country": "US"}, exemptionType = "501(c)(3)" and a valid exemptionCertificate.
3. Observe the response: HTTP 200 with a single caseId, and the store holds 2 exemption records (one per distinct account), as tests/all/repeated_account_ids.rs pins.
4. Observe the emitted access record for that call: `accounts = 3`, i.e. one more account than the submission addressed or wrote for, while the message states it is "the accounts a submission named".

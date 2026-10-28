Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# GetUsageStatistics takes an AccountIds list without the one account-id reader every other account-id-taking operation in the crate uses

_Filed: 2026-10-28T21:56:22Z_
_Operation: GetUsageStatistics | Class: validation_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`usageCriteria.accountIds` targets the same `AccountIds` list shape the member operations take, and `AccountId` constrains length only (12..12) with no character rule, so a 12-character value that is not an account id at all reaches the handler. Every other operation in this crate that takes account ids refuses such a request with `crate::state::account_ids_rejection` before anything is looked up — the helper's own doc calls itself "One reader for every operation that takes account ids, so the form cannot drift between them". `GetUsageStatistics` is the one caller that skips it: it turns the list straight into a filter set with `exact_values`, so a malformed id silently matches no usage record and the call answers HTTP 200 with empty/zero statistics instead of the 400 its eight siblings answer for the identical input.

The effect is the one the siblings' comments say must not happen: a request that was not understood is answered as if it were processed. Nothing in the file, and no comment on `exact_values`, justifies the divergence — `exact_values`' doc only explains the empty-list case.

## Evidence

THE ONE READER, and its own claim to be the only one — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:1571-1593:
```
/// Classify a LIST of account ids the caller named: `Some(message)` when an entry
/// cannot be an account id at all, `None` when every entry is well formed.
///
/// One reader for every operation that takes account ids, so the form cannot drift
/// between them, and one message — [`invalid_parameter_message`] for
/// [`ACCOUNT_ID_MEMBER`] — because the member at fault is the same one whichever
/// operation carried it. The whole call is refused rather than the entry reported:
/// `UnprocessedAccounts` reports accounts that were not PROCESSED, never a request
/// that was not understood.
pub fn account_ids_rejection<I>(account_ids: I) -> Option<String>
```

SIBLING THAT APPLIES IT (a pure report over the same detector, same shape of list) — src/ops/get_remaining_free_trial_days.rs:156-167:
```
        // Parameter failures first: a value that cannot be a value of its member at
        // all is refused whatever the store holds, because `UnprocessedAccounts`
        // reports accounts that were not PROCESSED, never a request that was not
        // understood.
        if !crate::state::is_detector_id(&input.detector_id) { ... }
        if let Some(message) = crate::state::account_ids_rejection(&input.account_ids) {
            return Err(bad_request(message));
        }
```
Same call in get_members.rs:175, get_member_detectors.rs:167, invite_members.rs:201, delete_members.rs:185, disassociate_members.rs:200, start_monitoring_members.rs:184, stop_monitoring_members.rs:191, decline_invitations.rs:158, delete_invitations.rs:152, update_member_detectors.rs:173, create_members.rs:207, and even for the non-`AccountIds` list in create_custom_detection_rule_org_configuration.rs:179 / update_custom_detection_rule_org_configuration.rs:185.

THE ONE THAT SKIPS IT — src/ops/get_usage_statistics.rs:180-185:
```
        // Decided from the REQUEST alone, before the store is looked at: the currency
        // the amounts are reported in, and what the criteria restrict them to.
        let unit = reported_unit(input.unit.as_deref())?;
        let selected_accounts = exact_values(input.usage_criteria.account_ids.as_deref());
```
and `exact_values` only drops an empty list — get_usage_statistics.rs:631-639:
```
/// A list the request left EMPTY restricts nothing, the way this service's other
/// criteria treat an empty list: it names no value to aggregate FROM.
fn exact_values(values: Option<&[String]>) -> Option<BTreeSet<String>> {
```

MODEL: `UsageCriteria.AccountIds` → `{"target": "AccountIds", "jsonName": "accountIds", "length": {"min": 1, "max": 50}, "doc": "<p>The account IDs to aggregate usage statistics from.</p>"}` (`smithy_query(query="shape",shape="UsageCriteria")`) — the same `AccountIds` target the member operations use, and `is_account_id` (state.rs:1567-1569) is what the crate means by well formed.

## Steps to Reproduce

1. `POST /detector` to create a detector; note its `detectorId`.
2. `POST /detector/{detectorId}/usage/statistics` with `{"usageStatisticsType":"SUM_BY_ACCOUNT","usageCriteria":{"accountIds":["not-an-acct"]}}` — a 12-character value that is not an account id, so the modelled `len 12..12` boundary check passes.
3. Observe HTTP 200 with empty `usageStatistics`: `exact_values` made the bogus value a filter that matches nothing (get_usage_statistics.rs:183, :205-208).
4. Send the same bogus id to any sibling, e.g. `POST /detector/{detectorId}/freeTrial/accounts` with `{"accountIds":["not-an-acct"]}`, and observe HTTP 400 `InvalidInputException` from `account_ids_rejection` (get_remaining_free_trial_days.rs:165-167).
5. The two doors onto the same `AccountIds` shape therefore disagree about whether the value is an account id.

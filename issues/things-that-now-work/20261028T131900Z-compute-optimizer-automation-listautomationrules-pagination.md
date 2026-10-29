Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 930ad079f132337074cf049827d554fb10a3d200

# Bind the issuing operation into the page-token fingerprint in the three listings that omit it

_Filed: 2026-10-28T13:19:00Z_
_Operation: ListAutomationRules | Class: pagination_

> _Note: this issue was identified against the code base at commit `930ad079f132337074cf049827d554fb10a3d200`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for page tokens: a token carries a fingerprint of the walk it was issued for, and a handler refuses a token whose fingerprint is not the one it would have issued — including a token ANOTHER operation of this service minted. Six of the nine paginated handlers implement that by mixing `Self::NAME` into the fingerprint (ListAccounts, ListRecommendedActions, ListRecommendedActionSummaries, ListAutomationEventSummaries and both preview listings). Three skip it: ListAutomationRules fingerprints only the filter list, ListAutomationEvents only the (filters, startTime, endTime) tuple, and ListAutomationEventSteps only the eventId. Those three therefore accept any token whose cursor fingerprint happens to equal their own digest input regardless of which operation minted it, i.e. they implement a weaker version of the rule their siblings state and enforce. No cross-operation collision is reachable today (the digested Debug renderings of the three inputs differ from every other listing's, and `EventId`'s `^[0-9A-Za-z]{16}$` keeps an eventId from colliding with the literal operation name ListAccounts digests), so this has no behavioural impact right now — the defect is the skipped check, which makes the guarantee the siblings document hold only by accident of Debug formatting and breaks as soon as one of the three digest inputs changes shape.

## Evidence

The rule, stated and implemented by a sibling — `services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/list_accounts.rs:175-178`:
```
// This operation carries no filters, so the only thing a page token is
// bound to is the walk it was issued for: a token one of this service's
// other paginated operations minted cannot resume this listing.
let query_fingerprint = crate::state::fingerprint(&Self::NAME);
```
and `src/ops/list_recommended_actions.rs:191-195`:
```
// The page token carries this operation's name and the filters it was
// taken with, so resuming a walk with different filters, or with a token
// another operation issued, is refused rather than answered with a page
// of a different result set.
let filter_fingerprint = crate::state::fingerprint(&(Self::NAME, &filters));
```
The three handlers that skip it — `src/ops/list_automation_rules.rs:177-180`:
```
// The page token carries the filters it was taken with, so resuming a
// walk with different filters is refused rather than answered with a
// page of a different result set.
let filter_fingerprint = crate::state::fingerprint(&filters);
```
`src/ops/list_automation_events.rs:201-205`: `let query_fingerprint = crate::state::fingerprint(&(&filters, input.start_time_inclusive.map(...), input.end_time_exclusive.map(...)));` and `src/ops/list_automation_event_steps.rs:179`: `let query_fingerprint = crate::state::fingerprint(&input.event_id);` — neither mixes in `Self::NAME`.

The helper leaves the comparison to each caller, `src/state.rs:3567-3570`: "What a decoded cursor may resume is the caller's own check: it compares the cursor's fingerprint against the one it would have issued for this request."

Test coverage: `tests/all/page_token_refusals.rs:249-266` (a cursor carrying "nofingerprintofanyqueryofthisservice" is refused by every paginated operation) and `:276-338` (a minted token resumes its own query and is refused by a different query of the SAME operation). No test exercises a token minted by one operation against another, so the cross-operation half of the documented rule is unpinned in exactly the three handlers that do not implement it.

## Steps to Reproduce

1. Read src/ops/list_accounts.rs:175-178 and src/ops/list_recommended_actions.rs:191-195: both bind `Self::NAME` into the page-token fingerprint and the comments state that a token another operation issued must be refused.
2. Read src/ops/list_automation_rules.rs:180, src/ops/list_automation_events.rs:201-205 and src/ops/list_automation_event_steps.rs:179: the fingerprint is computed from the query members only, with no operation identity.
3. Conclude that for those three handlers a decoded cursor is accepted on nothing but a digest of the query members, so the 'a token another operation issued is refused' guarantee is not implemented there — it holds only because no other listing's digest input currently renders to the same Debug string.

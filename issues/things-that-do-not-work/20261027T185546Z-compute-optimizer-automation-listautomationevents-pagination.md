Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 19fca1009697525153c46de34cf9f84b5fd316c3

# Bind the issuing operation into the page-token fingerprint in ListAutomationEvents, ListAutomationRules and ListAutomationEventSteps

_Filed: 2026-10-27T18:55:46Z_
_Operation: ListAutomationEvents | Class: pagination_

> _Note: this issue was identified against the code base at commit `19fca1009697525153c46de34cf9f84b5fd316c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Six of this service's nine paginated handlers digest `Self::NAME` into the fingerprint they bind into the page token, and their comments state the rule that binding implements: a token another operation minted must be refused rather than resume this listing. Three handlers skip that term and bind only their query payload:

* `ListAutomationEvents` — `fingerprint(&(&filters, start, end))`
* `ListAutomationRules` — `fingerprint(&filters)`
* `ListAutomationEventSteps` — `fingerprint(&input.event_id)`

Because `crate::state::fingerprint` digests `format!("{value:?}")`, the only thing that keeps a token minted by one of these three from being accepted by another paginated operation is that the `Debug` renderings of the different payload types happen to differ (`"[]"` vs `"([], None, None)"` vs `"\"<eventId>\""`). Nothing in the three handlers enforces the rule their siblings' comments state, and the check is cheap and already written six times next door. Adding the `Self::NAME` term leaves every same-operation walk — and every existing test, including `tests/all/page_token_refusals.rs`, which only exercises undecodable tokens and foreign fingerprints within one operation — answering exactly as it does today.

## Evidence

Sibling that applies the check, over the SAME event store and the SAME filter list:
`src/ops/list_automation_event_summaries.rs:251-261`:
```
        // The page token carries this operation's name and the whole query — the
        // filters AND the window — because both define the sequence of groups a
        // page is a window onto, so resuming with a different query, or with a
        // token another operation issued, is refused instead of answered with a
        // page of a different result set.
        let query_fingerprint = crate::state::fingerprint(&(
            Self::NAME,
            &filters,
```
The handler that skips it, `src/ops/list_automation_events.rs:197-205`:
```
        // The page token carries a fingerprint of the whole query — the filters
        // AND the time window — because both define the sequence a page is a
        // window onto, so resuming with a different query is refused instead of
        // answered with a page of a different result set.
        let query_fingerprint = crate::state::fingerprint(&(
            &filters,
            input.start_time_inclusive.map(crate::state::instant_key),
            input.end_time_exclusive.map(crate::state::instant_key),
        ));
```
Same omission in `src/ops/list_automation_rules.rs:177-180`:
```
        // The page token carries the filters it was taken with, so resuming a
        // walk with different filters is refused rather than answered with a
        // page of a different result set.
        let filter_fingerprint = crate::state::fingerprint(&filters);
```
and `src/ops/list_automation_event_steps.rs:176-179`:
```
        // The sequence a page is a window onto is the steps of ONE event, so the
        // event this call asks about is what the token is bound to: a token
        // issued for one event cannot resume a walk over another's steps.
        let query_fingerprint = crate::state::fingerprint(&input.event_id);
```
The same rule, stated and applied, in the other siblings — `src/ops/list_accounts.rs:175-178`: "a token one of this service's other paginated operations minted cannot resume this listing. let query_fingerprint = crate::state::fingerprint(&Self::NAME);"; `src/ops/list_recommended_actions.rs:191-195` and `src/ops/list_recommended_action_summaries.rs:207-211`: "The page token carries this operation's name and the filters it was taken with, so resuming a walk with different filters, or with a token another operation issued, is refused ... fingerprint(&(Self::NAME, &filters))"; `src/state.rs:1444-1466` (`preview_selection_fingerprint`): "The fingerprint a preview page token carries: the operation that issued it and the whole selection it was taken with ... or with the other preview operation's token — is refused".
The digest that makes the separation incidental, `src/state.rs:451-459`: "A stable digest of `value`'s debug rendering, used both as a request fingerprint for the client-token ledger and as the filter fingerprint bound into a page token. ... let digest = Sha256::digest(format!(\"{value:?}\").as_bytes());".
Test coverage of the token rule today: `tests/all/page_token_refusals.rs:133-185` tables every paginated operation but presents each token only to the same operation that minted it (`a_minted_token_resumes_its_own_query_and_is_refused_by_any_other`, line 276), so no test pins the cross-operation case either way.

## Steps to Reproduce

1. Read `src/ops/list_automation_event_summaries.rs:251-261` and note `Self::NAME` is the first term of the page-token fingerprint, with the comment stating that a token another operation issued must be refused.
2. Read `src/ops/list_automation_events.rs:201-205`, `src/ops/list_automation_rules.rs:180` and `src/ops/list_automation_event_steps.rs:179`: none of the three includes the operation in the fingerprint.
3. Read `src/state.rs:456-458`: the fingerprint is a digest of the payload's `Debug` rendering only, so for these three handlers the token carries no statement of which operation issued it.
4. Note that `tests/all/page_token_refusals.rs` never presents one operation's minted token to another operation, so nothing pins the current behaviour.

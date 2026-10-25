Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# GetPersonalizedRanking does not parse filterValues written in the documented escaped-double-quote form

_Filed: 2026-10-24T07:16:25Z_
_Operation: GetPersonalizedRanking | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same root cause as on GetRecommendations, observable on its own route. `GetPersonalizedRanking` resolves `filterArn` and binds the request's `filterValues` through the shared binder, which splits the raw value on commas and trims whitespace but never strips the escaped double quotes AWS's documented request format requires around each string value.

Expected: with filter `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)` and `filterValues {"GENRE": "\"Comedy\""}`, the returned `personalizedRanking` keeps exactly the Comedy entries of `inputList` (the others are dropped, as the handler's own doc states filters do). Actual: the bound value is the token `"Comedy"` including quote characters, which matches no stored attribute, so an INCLUDE filter drops EVERY catalog item of `inputList` — the response is 200 with a `personalizedRanking` holding only the cold items (items absent from the catalog, appended with score 0.0), or an empty list when there are none. With an EXCLUDE filter the opposite happens: nothing is filtered out and the full input list comes back, so a filter that should suppress items silently does nothing.

## Evidence

Spec: `GetPersonalizedRankingRequest$filterValues` (Smithy model, len 0..25) documents the parameter-name key / comma-separated value(s) format and points at "Filtering Recommendations", whose request format encloses each string filter value in escaped double quotes.
Handler: `src/ops/get_personalized_ranking.rs` lines 220-226 pass `input.filter_values` to `crate::state::bind_filter`; the mis-parse is in `src/state/filter.rs` lines 598-616 (`resolve()`) with the equality comparison at lines 656-677 (`evaluate()`).
Test coverage: `tests/all/inference.rs` `personalized_ranking_drops_the_items_its_filter_excludes` (lines 974-1010) uses a literal-valued filter, and no test in the crate sends a quoted `filterValues` value, so the documented form is uncovered.

## Steps to Reproduce

1. Register a PERSONALIZED_RANKING campaign with an Items catalog carrying GENRE values, and a filter `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
2. POST /personalize-ranking with {"campaignArn": <campaign>, "userId": "user-1", "inputList": [<several catalog item ids, some Comedy>], "filterArn": <filter>, "filterValues": {"GENRE": "\"Comedy\""}}.
3. Observed: HTTP 200 with an empty `personalizedRanking`. Expected: the Comedy subset of `inputList`, ordered by score descending.
4. Repeat with an `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)` filter and the same quoted value: observed the complete input list (nothing excluded); expected the Comedy entries removed.

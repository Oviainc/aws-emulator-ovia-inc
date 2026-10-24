Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# GetActionRecommendations does not parse filterValues written in the documented escaped-double-quote form

_Filed: 2026-10-24T07:16:25Z_
_Operation: GetActionRecommendations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same root cause as on GetRecommendations, observable on the /action-recommendations route against the Actions catalog. The handler binds `filterValues` through the shared binder, which comma-splits and trims the raw value but never strips the escaped double quotes AWS's documented request format puts around each string value.

Expected: with an action filter `INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)` and `filterValues {"CHANNEL": "\"App\""}`, the response carries the App-channel actions. Actual: the bound token is `"App"` with quote characters, which equals no stored Actions attribute, so an INCLUDE expression keeps nothing and the operation answers HTTP 200 with an EMPTY `actionList`; an EXCLUDE expression keeps everything, so the filter silently removes no action at all.

## Evidence

Spec: `GetActionRecommendationsRequest$filterValues` (Smithy model, len 0..25) documents the parameter-name key / comma-separated value(s) format for including or excluding actions and points at "Filtering recommendations and user segments", whose request format encloses each string filter value in escaped double quotes.
Handler: `src/ops/get_action_recommendations.rs` lines 191-198 bind the request's `filterValues` over `Entity::Actions`; the mis-parse is `src/state/filter.rs` lines 598-616 (`resolve()`) plus the equality comparison at lines 656-677 (`evaluate()`).
Test coverage: `tests/all/inference.rs` `an_action_filter_removes_the_actions_it_names` (lines 1048-1098) uses a literal-valued expression (`INCLUDE ActionID WHERE Actions.CHANNEL IN ("App")`); no test sends a placeholder bound to a quoted value, so the documented form is uncovered.

## Steps to Reproduce

1. Register a PERSONALIZED_ACTIONS campaign with an Actions catalog carrying CHANNEL values, and a filter `INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
2. POST /action-recommendations with {"campaignArn": <campaign>, "userId": "user-1", "numResults": 5, "filterArn": <filter>, "filterValues": {"CHANNEL": "\"App\""}}.
3. Observed: HTTP 200 with an empty `actionList`. Expected: the App-channel actions, descending by score.
4. Repeat with an `EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)` filter and the same quoted value: observed every action still returned; expected the App-channel actions removed.

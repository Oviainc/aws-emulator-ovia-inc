Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# ActionFilter values are rejected with 400 when they are not enum members instead of simply matching nothing

_Filed: 2026-10-24T16:42:22Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler enforces an enum-membership rule on `ActionFilter$values` that the model does not express, turning a listing that the real service answers with HTTP 200 into a 400 `ValidationException`.

`compile_filters()` walks every element of each filter's `values` and, for a value that is not a member of the enum the filter's `key` names (`FEATURE`->Feature, `SEVERITY`->Severity, `TYPE`->ActionType), pushes a `ViolationKind::EnumValue` violation, so the whole request is refused with HTTP 400 `ValidationException` (reason `fieldValidationFailed`, a `fieldList` entry at `filter.actions.1.member.values.1.member`, message "... failed to satisfy constraint: Member must satisfy enum value set: [...]").

Expected: `values` is a filter *comparison* list, not an enumerated member of the request shape. A value that matches no stored record's field simply keeps no record:
- `{key: SEVERITY, matchOption: EQUALS, values: ["EXTREME"]}` -> HTTP 200 with `recommendedActions: []`;
- `{key: SEVERITY, matchOption: NOT_EQUALS, values: ["EXTREME"]}` -> HTTP 200 with every stored record (the complement of the empty matched set), which is the whole point of `NOT_EQUALS`.

Actual: both requests are refused with HTTP 400 and no records are ever reported, so a client that passes a typo'd, newly-added, or lower-cased value gets an error response instead of a (possibly empty) page, and the exclusion form loses data it should have returned.

Note the asymmetry this creates with the members the model DOES type as enums: `ActionFilter$key` and `$matchOption` are enum shapes, so a 400 for an unmodelled member of either is correct (and is what the recorded AWS responses show); `values` is not, and should not be validated the same way.

## Evidence

Model (`smithy_query(query="shape", shape="ActionFilter,FilterValues")`): `ActionFilter$values` targets `FilterValues`, a list whose member `FilterValue` is a plain **string** with `length {min:1,max:1024}` and pattern `[\S\s]*` — i.e. any character, up to 1024 bytes. The model constrains it to no closed value set (contrast `key`->`FilterName` and `matchOption`->`MatchOption`, which ARE enum shapes); the longest member of any candidate enum is 42 characters (`REVIEW_SAVINGS_OPPORTUNITY_RECOMMENDATIONS`), so the 1024-byte free-text bound is modelling a compared string, not an enumerated token. The documentation is likewise comparison-flavoured: `values` is "One or more values to match against the specified key" and `matchOption` is "EQUALS to include matching results or NOT_EQUALS to exclude matching results".

Well-established AWS behaviour for flat key/values filter lists (EC2 `--filters`, Cost Explorer `Filter.Dimensions.Values`): an unrecognised filter NAME/key is an error, an unrecognised filter VALUE is not — it simply matches nothing and the call succeeds.

Handler: `services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs` — `FilterName::accepts_value()` / `value_members()` in `src/state.rs`, used by `compile_filters()` (`if !key.accepts_value(value) { violations.push(enum_violation(filter_value_path(..), value, key.value_members())) }`), whose module doc states the intent: "A filter value that is not a member of the enum its `key` names is refused with `ValidationException`".

Test coverage: `tests/all/list_recommended_actions.rs::every_refused_member_is_stated_with_a_reason_and_a_field_list` ASSERTS the current refusal of an out-of-enum filter value, so this behaviour is deliberate and must be re-decided rather than merely added. No test covers a 200-with-empty-page for an unmatched value. Existing issues for this operation cover only pagination (`...-listrecommendedactions-pagination.md`) and the content-type/ValidationException-shape/NOT_EQUALS divergences (fixed); neither touches value validation.

## Steps to Reproduce

1. Boot the emulator with this service mounted (the store boots with its computed inventory of three recommended actions).
2. POST `/` with `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions` and body `{"filter":{"actions":[{"key":"SEVERITY","matchOption":"EQUALS","values":["EXTREME"]}]}}`. Observed: HTTP 400 ValidationException with reason `fieldValidationFailed` and a fieldList entry at `filter.actions.1.member.values.1.member`. Expected: HTTP 200 with `recommendedActions: []`.
3. POST the same request with `"matchOption":"NOT_EQUALS"`. Observed: the same HTTP 400. Expected: HTTP 200 listing every stored record, since no record's severity equals the (unmatched) value.
4. POST `{"filter":{"actions":[{"key":"SEVERITY","matchOption":"EQUALS","values":["critical"]}]}}` (lower-cased). Observed: HTTP 400. Expected: HTTP 200 with an empty list (comparison is exact, not validated).
5. Confirm the enum-typed members still behave: an `ActionFilter` whose `key` is `NOT_A_REAL_FILTER_KEY` or whose `matchOption` is `SORT_OF_EQUALS` must still be HTTP 400 ValidationException with reason `fieldValidationFailed` and a fieldList naming `filter.actions.1.member.key` / `...matchOption`.

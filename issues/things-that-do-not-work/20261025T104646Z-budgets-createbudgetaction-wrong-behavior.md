Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# CreateBudgetAction (and CreateBudget) persist ResourceTags that name one key twice, although the model says each key must be unique for the resource

_Filed: 2026-10-25T10:46:46Z_
_Operation: CreateBudgetAction | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBudgetActionRequest$ResourceTags` is documented as "An optional list of tags to associate with the specified budget action. Each tag consists of a key and a value, and each key must be unique for the resource." The handler stores the supplied list verbatim (`input.resource_tags.unwrap_or_default()`), with no check that the keys are distinct, so a create can leave an action holding two tags with the same key and different values. `CreateBudget` has the identical gap for a budget's `ResourceTags`, whose doc carries the same sentence.

The crate already owns the check and applies it on the other write path: `TagResource` calls `crate::state::duplicate_tag_key_violation` and refuses the request with `InvalidParameterException` before anything is written. The create paths never call it.

The stored duplicate is not just cosmetic — the whole tag module is written against the one-tag-per-key invariant, so a resource created with a repeated key is in a state later calls cannot repair:

* `ListTagsForResource` reports the key twice.
* `TagResource` on that key goes through `apply_tags`, which re-values the FIRST matching entry only (`held.iter_mut().find(...)`), so the second entry keeps its old value forever and the resource keeps reporting two values for one key.
* `tag_count_after` counts a supplied key as new only when no held entry carries it, so the duplicate entry is silently miscounted against the tag ceiling.

## Evidence

Model text (smithy_query doc, CreateBudgetActionRequest$ResourceTags): "An optional list of tags to associate with the specified budget action. Each tag consists of a key and a value, and each key must be unique for the resource." — the same sentence for CreateBudgetRequest$ResourceTags ("…associate with the specified budget. Each tag consists of a key and a value, and each key must be unique for the resource."), mirrored in the handler docs at services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget.rs:70-72.
The path that skips the check — src/ops/create_budget_action.rs:320-323:
```
        let mut held = crate::state::StoredAction::new(
            action,
            input.resource_tags.unwrap_or_default(),
        );
```
and src/ops/create_budget.rs:315-325 `tags: input.resource_tags.unwrap_or_default(),` (line 323). Neither handler mentions `duplicate_tag_key_violation`.
The sibling that applies it — src/ops/tag_resource.rs:174-184:
```
        // One key cannot be given two values in one call, since a resource holds at most
        // one tag per key. Checked before the resource is looked up, so a request that is
        // malformed in itself is refused the same way whichever resource it names.
        if let Some(message) = crate::state::duplicate_tag_key_violation(&input.resource_tags) {
            return Err(TagResourceErrorModeled::InvalidParameterException(
```
The helper and the invariant it protects — src/state.rs:1994-2014: "The refusal a tag set naming one key twice takes … 'each key must be unique for the resource' (`ResourceTags`), so a request that gives one key two values states no single value for it"; src/state.rs:2028-2042 `apply_tags`: "a resource holds at most one tag per key, so tagging a key twice re-values it" — implemented as `held.iter_mut().find(|existing| existing.key == tag.key)`, i.e. the first entry only; src/state.rs:2020-2026 `tag_count_after` counts only keys no held entry carries.
Test coverage: the duplicate-key rule is tested exclusively through TagResource — tests/all/resource_tags.rs:340-392 (`a_tag_set_naming_one_key_twice_is_refused_and_writes_nothing`, rows "the same key twice, adjacent" / "separated by another" / "a key the resource already holds, given twice"). No test supplies a repeated key through CreateBudget or CreateBudgetAction.

## Steps to Reproduce

1. CreateBudget {"AccountId":"<caller account>","Budget":{"BudgetName":"tagdup","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} — 200.
2. CreateBudgetAction {"AccountId":"<caller account>","BudgetName":"tagdup","NotificationType":"ACTUAL","ActionType":"APPLY_IAM_POLICY","ActionThreshold":{"ActionThresholdValue":80,"ActionThresholdType":"PERCENTAGE"},"Definition":{"IamActionDefinition":{"PolicyArn":"arn:aws:iam::<caller account>:policy/deny"}},"ExecutionRoleArn":"arn:aws:iam::<caller account>:role/budget","ApprovalModel":"AUTOMATIC","Subscribers":[{"SubscriptionType":"EMAIL","Address":"a@example.com"}],"ResourceTags":[{"Key":"env","Value":"dev"},{"Key":"env","Value":"prod"}]} — the emulator answers 200 and stores both tags.
3. ListTagsForResource {"ResourceARN":"arn:aws:budgets::<caller account>:budget/tagdup/action/<ActionId>"} — the key `env` is reported twice, with two different values.
4. TagResource {"ResourceARN":"<that ARN>","ResourceTags":[{"Key":"env","Value":"fixed"}]} then ListTagsForResource again — only the first `env` entry was re-valued; the resource still reports two `env` tags.
5. For contrast, send the same duplicate pair through TagResource: it is refused with InvalidParameterException (tag_resource.rs:177), proving the two paths disagree on the same documented rule. The same sequence through CreateBudget's own `ResourceTags` reproduces it for a budget.
